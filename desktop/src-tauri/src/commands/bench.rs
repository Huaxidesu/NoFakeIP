//! 测速相关命令。

use crate::commands::err;
use crate::state::AppState;
use nofakeip_core::benchmark::{BatchProgress, BenchmarkOptions, DomainBenchResult};
use nofakeip_core::pollution::PollutionReport;
use nofakeip_core::recommend::{Recommendation, Recommender};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tauri::{Emitter, State};

/// 前端传入的测速参数。
#[derive(Debug, Clone, Deserialize)]
pub struct BenchOptionsDto {
    pub rounds: u32,
    pub timeout_secs: u64,
    pub concurrency: usize,
    #[serde(default)]
    pub domains: Vec<String>,
}

impl BenchOptionsDto {
    fn to_options(&self) -> BenchmarkOptions {
        BenchmarkOptions::default()
            .with_rounds(self.rounds.max(1))
            .with_timeout(std::time::Duration::from_secs(self.timeout_secs.max(1)))
            .with_concurrency(self.concurrency.max(1))
    }
}

#[derive(Debug, Serialize)]
pub struct FullBenchReport {
    pub results: Vec<DomainBenchResult>,
    pub reports: Vec<PollutionReport>,
    pub recommendation: Recommendation,
    pub elapsed_ms: f64,
}

/// 指定域名 × 指定节点的测速。
#[tauri::command]
pub async fn run_benchmark(
    state: State<'_, AppState>,
    domains: Vec<String>,
    server_ids: Vec<i64>,
    opts: BenchOptionsDto,
) -> Result<Vec<DomainBenchResult>, String> {
    if domains.is_empty() {
        return Err("请至少输入一个域名".into());
    }
    let servers = pick_servers(&state, &server_ids)?;
    if servers.is_empty() {
        return Err("没有可用的 DNS 节点，请到「设置」中启用至少一个".into());
    }

    let diagnoser = state.diagnoser();
    let results = diagnoser
        .bench_batch(&domains, &servers, &opts.to_options(), None)
        .await
        .map_err(err)?;

    // 落库（失败不阻断主流程）
    for r in &results {
        let _ = state.storage.insert_benchmark(r);
        let ips: Vec<String> = r.all_ips().iter().map(|i| i.to_string()).collect();
        let _ = state.storage.touch_domain(&r.domain, r.region, None, None, &ips);
    }
    Ok(results)
}

/// 全量测速（使用设置里的样本域名）+ 汇总推荐，并推送进度事件。
#[tauri::command]
pub async fn run_full_benchmark(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    opts: BenchOptionsDto,
) -> Result<FullBenchReport, String> {
    let settings = state.settings_snapshot();
    let domains: Vec<String> = if opts.domains.is_empty() {
        settings.sample_domains.clone()
    } else {
        opts.domains.clone()
    };
    if domains.is_empty() {
        return Err("样本域名为空，请在「设置」中填写".into());
    }

    let servers = pick_servers(&state, &[])?;
    if servers.is_empty() {
        return Err("没有可用的 DNS 节点".into());
    }

    let started = Instant::now();
    let emitter = app.clone();
    let progress: nofakeip_core::benchmark::ProgressFn = Arc::new(move |p: BatchProgress| {
        // 事件推送失败不影响测速本身
        let _ = emitter.emit("bench:progress", &p);
    });

    let diagnoser = state.diagnoser();
    let results = diagnoser
        .bench_batch(&domains, &servers, &opts.to_options(), Some(progress))
        .await
        .map_err(err)?;

    for r in &results {
        let _ = state.storage.insert_benchmark(r);
        let ips: Vec<String> = r.all_ips().iter().map(|i| i.to_string()).collect();
        let _ = state.storage.touch_domain(&r.domain, r.region, None, None, &ips);
    }

    let reports = diagnoser.scan_reports(&results);
    for r in &reports {
        let ips: Vec<String> = r
            .observations
            .iter()
            .flat_map(|o| o.ips.iter().map(|i| i.to_string()))
            .collect();
        let _ = state.storage.touch_domain(
            &r.domain,
            r.region,
            Some(r.risk),
            Some(r.fake_ip.label()),
            &ips,
        );
    }
    let recommendation = Recommender::from_benchmark(&results, &reports);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    // 顺带清理过老记录，避免数据库无限增长（失败忽略）
    let _ = state.storage.prune(5000);

    Ok(FullBenchReport {
        results,
        reports,
        recommendation,
        elapsed_ms,
    })
}

/// 按 ID 取节点；`ids` 为空时取全部已启用节点。
fn pick_servers(
    state: &AppState,
    ids: &[i64],
) -> Result<Vec<nofakeip_core::models::DnsServer>, String> {
    let all = state.storage.list_servers(false).map_err(err)?;
    if ids.is_empty() {
        Ok(all.into_iter().filter(|s| s.enabled).collect())
    } else {
        Ok(all.into_iter().filter(|s| ids.contains(&s.id)).collect())
    }
}
