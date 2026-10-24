//! CLI 子命令实现。

use crate::exit;
use crate::output::Out;
use crate::{Cli, Command, DataAction, DnsAction};
use anyhow::{anyhow, Context, Result};
use nofakeip_core::benchmark::{self, BenchmarkOptions, BatchProgress, DomainBenchResult};
use nofakeip_core::classifier::DomainClassifier;
use nofakeip_core::dns::ServerRegistry;
use nofakeip_core::diagnose::{Diagnoser, DomainDiagnosis};
use nofakeip_core::geodata::DataUpdater;
use nofakeip_core::models::{DnsServer, Region};
use nofakeip_core::pollution::{PollutionDetector, RiskLevel};
use nofakeip_core::recommend::{Recommendation, Recommender};
use nofakeip_core::report::{self, JsonEnvelope, ReportOptions};
use nofakeip_core::storage::Storage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// 分派子命令，返回退出码。
pub fn dispatch(cli: &Cli, out: &Out) -> Result<u8> {
    let dir = data_dir(cli)?;
    std::fs::create_dir_all(&dir).ok();

    match &cli.command {
        Command::Doctor => cmd_doctor(cli, out, &dir),
        Command::Classify { domains } => cmd_classify(cli, out, &dir, domains),
        Command::Data { action } => cmd_data(cli, out, &dir, action),
        Command::Dns { action } => cmd_dns(cli, out, &dir, action),
        _ => {
            // 其余命令需要数据库
            let storage = open_storage(cli, &dir)?;
            match &cli.command {
                Command::Scan {
                    domains,
                    servers,
                    all: _,
                    no_detect,
                } => cmd_scan(cli, out, &dir, &storage, domains, servers, *no_detect),
                Command::Bench {
                    file,
                    domains,
                    out: out_path,
                    servers,
                } => cmd_bench(cli, out, &dir, &storage, file.as_deref(), domains, out_path.as_deref(), servers),
                Command::Detect { domains, servers } => {
                    cmd_detect(cli, out, &dir, &storage, domains, servers)
                }
                Command::Report {
                    domain,
                    out: out_path,
                    format,
                    full,
                } => cmd_report(
                    cli,
                    out,
                    &dir,
                    &storage,
                    domain.as_deref(),
                    out_path.as_deref(),
                    format,
                    *full,
                ),
                Command::History {
                    domain,
                    limit,
                    risk,
                } => cmd_history(out, &storage, domain.as_deref(), *limit, risk.as_deref()),
                _ => unreachable!("已在上面处理"),
            }
        }
    }
}

// —————————————————————— 公共辅助 ——————————————————————

fn data_dir(cli: &Cli) -> Result<PathBuf> {
    if let Some(d) = &cli.data_dir {
        return Ok(d.clone());
    }
    if let Ok(d) = std::env::var("NOFAKEIP_DATA_DIR") {
        if !d.trim().is_empty() {
            return Ok(PathBuf::from(d));
        }
    }
    Storage::default_data_dir().map_err(|e| anyhow!(e.to_string()))
}

fn open_storage(cli: &Cli, dir: &Path) -> Result<Storage> {
    let path = cli.db.clone().unwrap_or_else(|| dir.join("nofakeip.db"));
    let st = Storage::open(&path).map_err(|e| anyhow!("打开数据库失败: {}", e))?;
    ensure_seeded(&st)?;
    Ok(st)
}

/// 首次运行时把内置 DNS 节点写入数据库。
fn ensure_seeded(st: &Storage) -> Result<()> {
    if st.list_servers(false)?.is_empty() {
        let reg = ServerRegistry::new();
        for s in reg.default_set() {
            st.upsert_server(&s).map_err(|e| anyhow!(e.to_string()))?;
        }
    }
    Ok(())
}

fn options(cli: &Cli) -> BenchmarkOptions {
    BenchmarkOptions::default()
        .with_rounds(cli.rounds)
        .with_timeout(Duration::from_secs(cli.timeout))
        .with_concurrency(cli.concurrency)
}

/// 解析出站代理设置。
///
/// 优先级：`--proxy <url>` 显式指定 > 自动探测（环境变量 → 本机常见代理端口）
/// > `--no-proxy` 强制直连。
fn cli_proxy(cli: &Cli) -> Result<Option<nofakeip_core::net::ProxyUrl>> {
    let mode = if cli.no_proxy {
        nofakeip_core::net::ProxyMode::Direct
    } else {
        cli.proxy
            .clone()
            .map(nofakeip_core::net::ProxyMode::Custom)
            .unwrap_or_default() // Auto
    };
    let proxy = run_blocking(nofakeip_core::net::resolve_proxy(&mode))?
        .map_err(|e| anyhow!(e.to_string()))?;
    if let Some(p) = &proxy {
        out_note(&format!("出站代理: {}", p));
    }
    Ok(proxy)
}

/// 仅在非静默模式下向 stderr 输出一条提示。
fn out_note(msg: &str) {
    if std::env::var("NOFAKEIP_QUIET").as_deref() != Ok("1") {
        eprintln!("[i] {}", msg);
    }
}

/// 解析要使用的服务器列表。
fn resolve_servers(st: &Storage, filter: &[String]) -> Result<Vec<DnsServer>> {
    let all = st.list_servers(true).map_err(|e| anyhow!(e.to_string()))?;
    if filter.is_empty() {
        return Ok(all);
    }
    let mut out: Vec<DnsServer> = Vec::new();
    for f in filter {
        let fl = f.to_lowercase();
        let matched: Vec<DnsServer> = all
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&fl)
                    || s.address.to_lowercase().contains(&fl)
                    || s.id.to_string() == fl
            })
            .cloned()
            .collect();
        if matched.is_empty() {
            return Err(anyhow!("没有匹配的服务器: {}", f));
        }
        for m in matched {
            if !out.iter().any(|x| x.id == m.id) {
                out.push(m);
            }
        }
    }
    Ok(out)
}

/// 构造诊断器（加载用户规则、Fake-IP 段、在线归属地开关与出站代理）。
fn build_diagnoser(cli: &Cli, dir: &Path, st: &Storage) -> Result<Diagnoser> {
    let proxy = cli_proxy(cli)?;
    let detector = PollutionDetector::with_data_dir(Some(dir))
        .with_fake_ip_range(&cli.fake_ip_range)
        .map_err(|e| anyhow!(e.to_string()))?
        .with_online_geo_and_proxy(cli.online_geo, proxy);

    let mut classifier = DomainClassifier::with_data_dir(Some(dir));
    for r in st.list_rules().map_err(|e| anyhow!(e.to_string()))? {
        classifier.add_user_override(&r.pattern, r.region);
    }

    Ok(Diagnoser::with_data_dir(Some(dir))
        .with_detector(detector)
        .with_classifier(classifier))
}

fn parse_risk(s: &str) -> Option<RiskLevel> {
    RiskLevel::parse(s)
}

// —————————————————————— scan ——————————————————————

#[allow(clippy::too_many_arguments)]
fn cmd_scan(
    cli: &Cli,
    out: &Out,
    dir: &Path,
    st: &Storage,
    domains: &[String],
    server_filter: &[String],
    no_detect: bool,
) -> Result<u8> {
    let servers = resolve_servers(st, server_filter)?;
    if servers.is_empty() {
        out.error("没有可用的 DNS 服务器，请先执行 `nofakeip dns list --all` 检查");
        return Ok(exit::NO_SERVERS);
    }

    let diagnoser = build_diagnoser(cli, dir, st)?;
    let opts = options(cli);

    let mut results: Vec<DomainBenchResult> = Vec::new();
    let mut reports = Vec::new();
    let mut high_risk = false;
    let mut any_success = false;

    for d in domains {
        let diagnosis = run_blocking(diagnoser.diagnose(d, &servers, &opts))??;
        if diagnosis.bench.ok_count() > 0 {
            any_success = true;
        }
        if diagnosis.report.risk == RiskLevel::High {
            high_risk = true;
        }
        // 持久化
        let _ = st.insert_benchmark(&diagnosis.bench);
        let ips: Vec<String> = diagnosis
            .bench
            .all_ips()
            .iter()
            .map(|i| i.to_string())
            .collect();
        let _ = st.touch_domain(
            d,
            diagnosis.classification.region,
            Some(diagnosis.report.risk),
            Some(diagnosis.report.fake_ip.label()),
            &ips,
        );

        if !cli.json && !no_detect {
            render_diagnosis(out, &diagnosis);
        } else if !cli.json {
            render_bench_only(out, &diagnosis.bench);
        }
        results.push(diagnosis.bench);
        reports.push(diagnosis.report);
    }

    if cli.json {
        let rec = Recommender::from_benchmark(&results, &reports);
        let env = JsonEnvelope::new("scan", &results, &reports, &rec);
        out.json_out(&env.to_json().map_err(|e| anyhow!(e.to_string()))?);
    }

    if !any_success {
        out.error("所有服务器测试均失败，请检查网络");
        return Ok(exit::ALL_FAILED);
    }
    if high_risk && cli.fail_on_pollution {
        return Ok(exit::POLLUTION);
    }
    Ok(exit::OK)
}

fn render_bench_only(out: &Out, r: &DomainBenchResult) {
    out.heading(&format!(
        "域名: {}    分类: {} ({:.2})",
        r.domain,
        r.region.label(),
        r.confidence
    ));
    let rows: Vec<Vec<String>> = r
        .results
        .iter()
        .map(|s| {
            vec![
                s.server_name.clone(),
                s.protocol.label().to_string(),
                s.region.label().to_string(),
                s.latency_display(),
                s.status_display(),
                s.answers
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ]
        })
        .collect();
    out.table(
        &["服务器", "协议", "区域", "延迟", "状态", "返回 IP"],
        &rows,
    );
}

fn render_diagnosis(out: &Out, d: &DomainDiagnosis) {
    out.blank();
    out.heading(&format!(
        "域名: {}    分类: {} ({:.2}, {})",
        d.domain,
        d.classification.region.label(),
        d.classification.confidence,
        d.classification.source.label()
    ));

    let rows: Vec<Vec<String>> = d
        .bench
        .results
        .iter()
        .map(|s| {
            vec![
                s.server_name.clone(),
                s.protocol.label().to_string(),
                s.region.label().to_string(),
                s.latency_display(),
                s.status_display(),
                s.answers
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ]
        })
        .collect();
    out.table(
        &["服务器", "协议", "区域", "延迟", "状态", "返回 IP"],
        &rows,
    );

    println!(
        "污染风险: {}    Fake-IP: {}",
        d.report.risk.label(),
        d.report.fake_ip.label()
    );
    for f in &d.report.findings {
        let ev = if f.evidence.is_empty() {
            String::new()
        } else {
            format!("（{}）", f.evidence.join("；"))
        };
        println!("  [{}] {}{}", f.severity.label(), f.message, ev);
    }

    println!();
    println!("Recommendation:");
    if let Some(b) = &d.recommendation.foreign {
        if d.classification.region == Region::Global {
            println!("  {} → {} ({:.0}ms)", d.domain, b.name, b.avg_ms);
        }
    }
    if let Some(b) = &d.recommendation.china {
        if d.classification.region == Region::China {
            println!("  {} → {} ({:.0}ms)", d.domain, b.name, b.avg_ms);
        }
    }
    if d.recommendation.china.is_none() && d.recommendation.foreign.is_none() {
        println!("  无可用推荐（全部测试失败）");
    }
}

/// 在独立线程里跑 async 任务（CLI 没有外层 runtime，避免多处 `block_on` 嵌套）。
fn run_blocking<F: std::future::Future>(fut: F) -> Result<F::Output> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("创建 tokio 运行时失败")?;
    Ok(rt.block_on(fut))
}

// —————————————————————— bench ——————————————————————

#[allow(clippy::too_many_arguments)]
fn cmd_bench(
    cli: &Cli,
    out: &Out,
    dir: &Path,
    st: &Storage,
    file: Option<&Path>,
    inline: &[String],
    out_path: Option<&Path>,
    server_filter: &[String],
) -> Result<u8> {
    let mut domains: Vec<String> = inline.to_vec();
    if let Some(f) = file {
        let text = std::fs::read_to_string(f)
            .with_context(|| format!("读取 {} 失败", f.display()))?;
        domains.extend(nofakeip_core::data::BuiltinData::parse_lines(&text));
    }
    domains.retain(|d| !d.trim().is_empty());
    if domains.is_empty() {
        return Err(anyhow!("没有待测域名：请用 --file 或 --domains 指定"));
    }

    let servers = resolve_servers(st, server_filter)?;
    if servers.is_empty() {
        out.error("没有可用的 DNS 服务器");
        return Ok(exit::NO_SERVERS);
    }

    let diagnoser = build_diagnoser(cli, dir, st)?;
    let opts = options(cli);
    let out_cb = *out;
    let progress: benchmark::ProgressFn = Arc::new(move |p: BatchProgress| {
        out_cb.progress(p.done, p.total, &p.current_domain);
    });

    out.info(&format!(
        "开始测速：{} 个域名 × {} 个服务器（每台 {} 轮）",
        domains.len(),
        servers.len(),
        cli.rounds
    ));
    let results = run_blocking(diagnoser.bench_batch(&domains, &servers, &opts, Some(progress)))??;
    out.progress_done();

    if results.len() < domains.len() {
        out.warn(&format!(
            "有 {} 个域名未能完成测速（详见 -v 日志）",
            domains.len() - results.len()
        ));
    }

    for r in &results {
        let _ = st.insert_benchmark(r);
        let ips: Vec<String> = r.all_ips().iter().map(|i| i.to_string()).collect();
        let _ = st.touch_domain(&r.domain, r.region, None, None, &ips);
    }

    if let Some(p) = out_path {
        let csv = benchmark::export_csv(&results).map_err(|e| anyhow!(e.to_string()))?;
        std::fs::write(p, csv).with_context(|| format!("写入 {} 失败", p.display()))?;
        out.success(&format!("已导出 CSV: {}", p.display()));
    }

    let rec = Recommender::from_benchmark(&results, &[]);
    render_bench_summary(out, &results, &rec);
    Ok(exit::OK)
}

fn render_bench_summary(out: &Out, results: &[DomainBenchResult], rec: &Recommendation) {
    // 按服务器聚合
    #[derive(Default)]
    struct Agg {
        region: Region,
        protocol: String,
        total: f64,
        n: u32,
        ok: u32,
        all: u32,
    }
    let mut map: std::collections::BTreeMap<String, Agg> = std::collections::BTreeMap::new();
    for r in results {
        for s in &r.results {
            let e = map.entry(s.server_name.clone()).or_default();
            e.region = s.region;
            e.protocol = s.protocol.label().to_string();
            e.all += 1;
            if s.ok {
                e.ok += 1;
                e.total += s.avg_ms;
                e.n += 1;
            }
        }
    }
    let mut rows: Vec<(f64, Vec<String>)> = map
        .into_iter()
        .map(|(name, a)| {
            let avg = if a.n == 0 { f64::MAX } else { a.total / a.n as f64 };
            (
                avg,
                vec![
                    name,
                    a.protocol,
                    a.region.label().to_string(),
                    if a.n == 0 {
                        "—".to_string()
                    } else {
                        format!("{:.0}ms", avg)
                    },
                    format!("{}/{}", a.ok, a.all),
                ],
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let rows: Vec<Vec<String>> = rows.into_iter().map(|(_, r)| r).collect();

    out.blank();
    out.heading("服务器汇总");
    out.table(&["服务器", "协议", "区域", "平均延迟", "成功"], &rows);
    out.blank();
    out.heading("推荐");
    for r in &rec.reasons {
        println!("  · {}", r);
    }
}

// —————————————————————— classify ——————————————————————

fn cmd_classify(cli: &Cli, out: &Out, dir: &Path, domains: &[String]) -> Result<u8> {
    let classifier = DomainClassifier::with_data_dir(Some(dir));
    if cli.json {
        let list: Vec<_> = domains.iter().map(|d| classifier.classify(d)).collect();
        out.json_out(&serde_json::to_string_pretty(&list)?);
        return Ok(exit::OK);
    }
    let rows: Vec<Vec<String>> = domains
        .iter()
        .map(|d| {
            let c = classifier.classify(d);
            vec![
                c.domain.clone(),
                c.region.label().to_string(),
                format!("{:.2}", c.confidence),
                c.source.label().to_string(),
                c.matched_rule.clone().unwrap_or_else(|| "—".into()),
            ]
        })
        .collect();
    out.table(&["域名", "归属", "置信度", "判定来源", "命中规则"], &rows);
    Ok(exit::OK)
}

// —————————————————————— detect ——————————————————————

fn cmd_detect(
    cli: &Cli,
    out: &Out,
    dir: &Path,
    st: &Storage,
    domains: &[String],
    server_filter: &[String],
) -> Result<u8> {
    let servers = resolve_servers(st, server_filter)?;
    if servers.is_empty() {
        out.error("没有可用的 DNS 服务器");
        return Ok(exit::NO_SERVERS);
    }
    let diagnoser = build_diagnoser(cli, dir, st)?;
    let opts = options(cli);

    let mut results = Vec::new();
    let mut reports = Vec::new();
    let mut high = false;

    for d in domains {
        let diag = run_blocking(diagnoser.diagnose(d, &servers, &opts))??;
        if diag.report.risk == RiskLevel::High {
            high = true;
        }
        if !cli.json {
            render_diagnosis(out, &diag);
        }
        results.push(diag.bench);
        reports.push(diag.report);
    }

    if cli.json {
        let rec = Recommender::from_benchmark(&results, &reports);
        let env = JsonEnvelope::new("detect", &results, &reports, &rec);
        out.json_out(&env.to_json().map_err(|e| anyhow!(e.to_string()))?);
    }

    if high && cli.fail_on_pollution {
        return Ok(exit::POLLUTION);
    }
    Ok(exit::OK)
}

// —————————————————————— report ——————————————————————

#[allow(clippy::too_many_arguments)]
fn cmd_report(
    cli: &Cli,
    out: &Out,
    dir: &Path,
    st: &Storage,
    domain: Option<&str>,
    out_path: Option<&Path>,
    format: &str,
    full: bool,
) -> Result<u8> {
    let diagnoser = build_diagnoser(cli, dir, st)?;
    let opts = options(cli);
    let ropts = ReportOptions::default();

    let content: String = if let Some(d) = domain {
        let servers = resolve_servers(st, &[])?;
        if servers.is_empty() {
            out.error("没有可用的 DNS 服务器");
            return Ok(exit::NO_SERVERS);
        }
        let diag = run_blocking(diagnoser.diagnose(d, &servers, &opts))??;
        let _ = st.insert_benchmark(&diag.bench);
        match format {
            "json" => {
                let results = [diag.bench.clone()];
                let reports = [diag.report.clone()];
                JsonEnvelope::new("report", &results, &reports, &diag.recommendation)
                    .to_json()
                    .map_err(|e| anyhow!(e.to_string()))?
            }
            "csv" => {
                let results = [diag.bench.clone()];
                benchmark::export_csv(&results).map_err(|e| anyhow!(e.to_string()))?
            }
            _ => report::render_domain_markdown(&diag, &ropts),
        }
    } else {
        let results = st
            .list_benchmarks(None, 500)
            .map_err(|e| anyhow!(e.to_string()))?;
        if results.is_empty() {
            return Err(anyhow!(
                "没有历史测速数据：请先执行 `nofakeip bench --file domains.txt` 或 `nofakeip scan <域名>`"
            ));
        }
        let reports = diagnoser.scan_reports(&results);
        let rec = Recommender::from_benchmark(&results, &reports);
        match format {
            "json" => JsonEnvelope::new("report", &results, &reports, &rec)
                .to_json()
                .map_err(|e| anyhow!(e.to_string()))?,
            "csv" => benchmark::export_csv(&results).map_err(|e| anyhow!(e.to_string()))?,
            _ => report::render_overview_markdown(&results, &reports, &rec),
        }
    };

    let _ = full;
    match out_path {
        Some(p) => {
            std::fs::write(p, content).with_context(|| format!("写入 {} 失败", p.display()))?;
            out.success(&format!("报告已保存: {}", p.display()));
        }
        None => print!("{}", content),
    }
    Ok(exit::OK)
}

// —————————————————————— data ——————————————————————

fn cmd_data(cli: &Cli, out: &Out, dir: &Path, action: &DataAction) -> Result<u8> {
    let updater = DataUpdater::new(dir);
    match action {
        DataAction::List => {
            let statuses = updater.status();
            if cli.json {
                out.json_out(&serde_json::to_string_pretty(&statuses)?);
                return Ok(exit::OK);
            }
            let rows: Vec<Vec<String>> = statuses
                .iter()
                .map(|s| {
                    vec![
                        s.id.clone(),
                        s.name.clone(),
                        if s.using_builtin {
                            "内置数据".to_string()
                        } else {
                            "已更新".to_string()
                        },
                        s.entries.to_string(),
                        s.modified_at
                            .clone()
                            .unwrap_or_else(|| "—".to_string()),
                    ]
                })
                .collect();
            out.table(
                &["ID", "名称", "状态", "条目数", "更新时间"],
                &rows,
            );
            Ok(exit::OK)
        }
        DataAction::Update { ids, all } => {
            let ids: Vec<String> = if *all { Vec::new() } else { ids.clone() };
            let use_all = *all;
            out.info(if use_all {
                "开始更新全部数据源…"
            } else if ids.is_empty() {
                "开始更新默认数据源…"
            } else {
                "开始更新指定数据源…"
            });

            // 数据下载走出站代理（--proxy / 自动探测），照顾无系统代理的用户
            let updater = updater.with_proxy(cli_proxy(cli)?);
            let reports = run_blocking(async {
                if use_all {
                    let all_ids: Vec<String> = updater.sources().iter().map(|s| s.id.clone()).collect();
                    updater.update_many(&all_ids, None).await
                } else {
                    updater.update_many(&ids, None).await
                }
            })?;

            if cli.json {
                out.json_out(&serde_json::to_string_pretty(&reports)?);
            } else {
                let rows: Vec<Vec<String>> = reports
                    .iter()
                    .map(|r| {
                        vec![
                            r.id.clone(),
                            if r.ok { "成功".into() } else { "失败".into() },
                            r.entries.to_string(),
                            format!("{:.1}s", r.duration_ms / 1000.0),
                            r.url_used
                                .clone()
                                .unwrap_or_else(|| r.error.clone().unwrap_or_default()),
                        ]
                    })
                    .collect();
                out.table(&["ID", "结果", "条目数", "耗时", "地址 / 错误"], &rows);
            }

            if reports.iter().any(|r| !r.ok) {
                let failed = reports.iter().filter(|r| !r.ok).count();
                out.warn(&format!(
                    "{}/{} 个数据源更新失败（已保留本地原有数据，未做任何破坏性写入）",
                    failed,
                    reports.len()
                ));
                return Ok(exit::GENERAL);
            }
            Ok(exit::OK)
        }
    }
}

// —————————————————————— dns ——————————————————————

fn cmd_dns(cli: &Cli, out: &Out, dir: &Path, action: &DnsAction) -> Result<u8> {
    let st = open_storage(cli, dir)?;
    let reg = ServerRegistry::new();

    match action {
        DnsAction::List { all } => {
            let list = st
                .list_servers(!*all)
                .map_err(|e| anyhow!(e.to_string()))?;
            if cli.json {
                out.json_out(&serde_json::to_string_pretty(&list)?);
                return Ok(exit::OK);
            }
            let rows: Vec<Vec<String>> = list
                .iter()
                .map(|s| {
                    vec![
                        s.id.to_string(),
                        s.name.clone(),
                        s.protocol.label().to_string(),
                        s.region.label().to_string(),
                        s.address.clone(),
                        if s.enabled { "启用".into() } else { "禁用".into() },
                    ]
                })
                .collect();
            out.table(
                &["ID", "名称", "协议", "区域", "地址", "状态"],
                &rows,
            );
            Ok(exit::OK)
        }
        DnsAction::Add(args) => {
            let mut server = reg
                .parse_server(&args.spec)
                .map_err(|e| anyhow!(e.to_string()))?;
            if let Some(n) = &args.name {
                server.name = n.clone();
            } else {
                server.name = server.address.clone();
            }
            if let Some(r) = &args.region {
                server.region = Region::parse(r).ok_or_else(|| anyhow!("非法区域: {}", r))?;
            }
            server.enabled = !args.disabled;
            let id = st.upsert_server(&server).map_err(|e| anyhow!(e.to_string()))?;
            out.success(&format!(
                "已添加 #{} {} [{}] {}",
                id,
                server.name,
                server.protocol.label(),
                server.address
            ));
            Ok(exit::OK)
        }
        DnsAction::Remove { key } => {
            let id = find_server_id(&st, &reg, key)?;
            st.delete_server(id).map_err(|e| anyhow!(e.to_string()))?;
            out.success(&format!("已删除 #{}", id));
            Ok(exit::OK)
        }
        DnsAction::Enable { key } => {
            let id = find_server_id(&st, &reg, key)?;
            st.set_server_enabled(id, true)
                .map_err(|e| anyhow!(e.to_string()))?;
            out.success(&format!("已启用 #{}", id));
            Ok(exit::OK)
        }
        DnsAction::Disable { key } => {
            let id = find_server_id(&st, &reg, key)?;
            st.set_server_enabled(id, false)
                .map_err(|e| anyhow!(e.to_string()))?;
            out.success(&format!("已禁用 #{}", id));
            Ok(exit::OK)
        }
        DnsAction::Import { file, format } => {
            let text =
                std::fs::read_to_string(file).with_context(|| format!("读取 {} 失败", file.display()))?;
            let servers = if format == "json" {
                reg.import_json(&text).map_err(|e| anyhow!(e.to_string()))?
            } else {
                reg.import_txt(&text).map_err(|e| anyhow!(e.to_string()))?
            };
            let n = servers.len();
            for s in servers {
                st.upsert_server(&s).map_err(|e| anyhow!(e.to_string()))?;
            }
            out.success(&format!("已导入 {} 个服务器", n));
            Ok(exit::OK)
        }
        DnsAction::Export { file, format } => {
            let list = st.list_servers(false).map_err(|e| anyhow!(e.to_string()))?;
            let content = if format == "json" {
                reg.export_json(&list).map_err(|e| anyhow!(e.to_string()))?
            } else {
                reg.export_txt(&list)
            };
            std::fs::write(file, content).with_context(|| format!("写入 {} 失败", file.display()))?;
            out.success(&format!("已导出到 {}", file.display()));
            Ok(exit::OK)
        }
    }
}

fn find_server_id(st: &Storage, reg: &ServerRegistry, key: &str) -> Result<i64> {
    if let Ok(id) = key.parse::<i64>() {
        if st.get_server(id).map_err(|e| anyhow!(e.to_string()))?.is_some() {
            return Ok(id);
        }
    }
    let list = st.list_servers(false).map_err(|e| anyhow!(e.to_string()))?;
    let kl = key.to_lowercase();
    if let Some(s) = list.iter().find(|s| s.name.to_lowercase() == kl) {
        return Ok(s.id);
    }
    if let Some(s) = list.iter().find(|s| s.name.to_lowercase().contains(&kl)) {
        return Ok(s.id);
    }
    let _ = reg;
    Err(anyhow!("找不到服务器: {}", key))
}

// —————————————————————— history ——————————————————————

fn cmd_history(
    out: &Out,
    st: &Storage,
    domain: Option<&str>,
    limit: u32,
    risk: Option<&str>,
) -> Result<u8> {
    let risk_filter = risk.and_then(parse_risk);
    let rows = st.list_domains(limit).map_err(|e| anyhow!(e.to_string()))?;
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|r| domain.map(|d| r.domain.contains(d)).unwrap_or(true))
        .filter(|r| risk_filter.map(|f| r.risk == Some(f)).unwrap_or(true))
        .collect();

    if rows.is_empty() {
        out.info("没有历史记录");
        return Ok(exit::OK);
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.domain.clone(),
                r.region.label().to_string(),
                r.risk.map(|x| x.label().to_string()).unwrap_or_else(|| "—".into()),
                r.fake_ip.clone().unwrap_or_else(|| "—".into()),
                r.query_count.to_string(),
                r.last_seen_at.clone(),
            ]
        })
        .collect();
    out.table(
        &["域名", "归属", "风险", "Fake-IP", "次数", "最近时间"],
        &table,
    );
    Ok(exit::OK)
}

// —————————————————————— doctor ——————————————————————

fn cmd_doctor(cli: &Cli, out: &Out, dir: &Path) -> Result<u8> {
    let mut items: Vec<(String, String, String)> = Vec::new(); // (状态, 检查项, 说明)
    let mut fatal = false;

    // 1. 数据目录可写
    let probe = dir.join(".write_probe");
    let write_ok = std::fs::write(&probe, b"ok").is_ok() && std::fs::remove_file(&probe).is_ok();
    items.push((
        sym(write_ok),
        "数据目录可写".into(),
        dir.display().to_string(),
    ));
    if !write_ok {
        fatal = true;
    }

    // 2. SQLite
    let db = cli.db.clone().unwrap_or_else(|| dir.join("nofakeip.db"));
    match Storage::open(&db).and_then(|s| s.schema_version()) {
        Ok(v) => items.push((
            sym(true),
            "数据库".into(),
            format!("{}（schema v{}）", db.display(), v),
        )),
        Err(e) => {
            fatal = true;
            items.push((sym(false), "数据库".into(), e.to_string()));
        }
    }

    // 3. 内置规则
    let classifier = DomainClassifier::with_data_dir(Some(dir));
    let rules_ok = classifier.china_rule_count > 1000 && classifier.foreign_rule_count > 100;
    items.push((
        sym(rules_ok),
        "内置规则".into(),
        format!(
            "国内 {} 条 / 国外 {} 条",
            classifier.china_rule_count, classifier.foreign_rule_count
        ),
    ));

    // 4. 内置 DNS 节点
    let reg = ServerRegistry::new();
    let builtin = reg.builtin();
    items.push((
        sym(!builtin.is_empty()),
        "内置 DNS 节点".into(),
        format!("{} 个（默认启用 {} 个）", builtin.len(), reg.default_set().len()),
    ));

    // 5. 规则数据版本
    let updater = DataUpdater::new(dir);
    let statuses = updater.status();
    let updated = statuses.iter().filter(|s| !s.using_builtin).count();
    items.push((
        sym(true),
        "规则数据".into(),
        if updated == 0 {
            "全部使用内置数据".to_string()
        } else {
            format!("{}/{} 个数据源已在线更新", updated, statuses.len())
        },
    ));

    // 6. 系统时间（TLS 证书校验依赖）
    let now = chrono::Utc::now();
    let time_ok = now.format("%Y").to_string().parse::<i32>().unwrap_or(0) >= 2024;
    items.push((
        sym(time_ok),
        "系统时间".into(),
        now.to_rfc3339(),
    ));

    // 7. 网络可达性 + Fake-IP 环境检测
    let probe_server = builtin
        .iter()
        .find(|s| s.region == Region::China && s.protocol == nofakeip_core::models::DnsProtocol::Udp)
        .cloned();
    let fake_net = nofakeip_core::geo::parse_net(&cli.fake_ip_range).ok();
    match probe_server {
        Some(srv) => {
            let client = nofakeip_core::dns::DnsClient::new();
            let qopts = nofakeip_core::dns::QueryOptions::default().with_timeout(Duration::from_secs(3));
            let r = run_blocking(client.query(&srv, "www.baidu.com", 1, &qopts))?;
            match r {
                Ok(outcome) => {
                    let ips = outcome.message.ip_answers();
                    items.push((
                        sym(true),
                        "网络可达性".into(),
                        format!("{} 查询成功，返回 {} 个地址", srv.name, ips.len()),
                    ));
                    let fake_hit = fake_net
                        .map(|n| ips.iter().any(|ip| nofakeip_core::geo::in_net(*ip, &n)))
                        .unwrap_or(false);
                    items.push((
                        if fake_hit { "⚠".to_string() } else { sym(true) },
                        "Fake-IP 环境".into(),
                        if fake_hit {
                            format!(
                                "检测到本机处于 Fake-IP 环境（返回 {}），明文 DNS 测试结果会被本地劫持；建议使用 DoH/DoT 节点测试",
                                ips.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
                            )
                        } else {
                            "未检测到 Fake-IP 应答".to_string()
                        },
                    ));
                }
                Err(e) => {
                    items.push((
                        "⚠".into(),
                        "网络可达性".into(),
                        format!("{} 查询失败: {}", srv.name, e),
                    ));
                }
            }
        }
        None => items.push(("⚠".into(), "网络可达性".into(), "没有可用的内置节点".into())),
    }

    if cli.json {
        let list: Vec<serde_json::Value> = items
            .iter()
            .map(|(s, n, d)| serde_json::json!({"status": s, "check": n, "detail": d}))
            .collect();
        out.json_out(&serde_json::to_string_pretty(&list)?);
    } else {
        let rows: Vec<Vec<String>> = items
            .iter()
            .map(|(s, n, d)| vec![s.clone(), n.clone(), d.clone()])
            .collect();
        out.table(&["状态", "检查项", "说明"], &rows);
    }

    if fatal {
        return Ok(exit::STORAGE);
    }
    Ok(exit::OK)
}

fn sym(ok: bool) -> String {
    if ok {
        "✅".to_string()
    } else {
        "❌".to_string()
    }
}
