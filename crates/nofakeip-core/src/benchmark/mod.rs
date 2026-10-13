//! DNS 测速引擎：单域名多轮测试、批量并发测试、CSV 导出。
//!
//! 统计口径见 `doc/02-核心引擎设计.md` 第 5 节：
//! `min/avg/max/p95` 取自 `total_ms` 样本；`warmup=true` 时第一轮不计入。

use crate::dns::wire::{self, TYPE_A};
use crate::dns::{DnsClient, QueryOptions};
use crate::error::{CoreError, Result};
use crate::geo::{GeoProvider, IpGeo};
use crate::models::{DnsProtocol, DnsServer, Region};
use crate::pollution::ServerObservation;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// 测速参数。
#[derive(Debug, Clone, Copy)]
pub struct BenchmarkOptions {
    /// 每服务器轮次
    pub rounds: u32,
    /// 单次查询超时
    pub timeout: Duration,
    /// 并发上限（Android 建议 8）
    pub concurrency: usize,
    /// 查询记录类型
    pub qtype: u16,
    /// 首轮是否丢弃（剔除冷连接）
    pub warmup: bool,
    /// UDP 截断时是否回退 TCP
    pub tcp_fallback: bool,
}

impl Default for BenchmarkOptions {
    fn default() -> Self {
        Self {
            rounds: 3,
            timeout: Duration::from_secs(5),
            concurrency: 16,
            qtype: TYPE_A,
            warmup: true,
            tcp_fallback: true,
        }
    }
}

impl BenchmarkOptions {
    pub fn with_rounds(mut self, n: u32) -> Self {
        self.rounds = n.max(1);
        self
    }
    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }
    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }
    pub fn with_qtype(mut self, t: u16) -> Self {
        self.qtype = t;
        self
    }

    fn query_options(&self) -> QueryOptions {
        QueryOptions {
            timeout: self.timeout,
            edns: true,
            tcp_fallback: self.tcp_fallback,
        }
    }
}

/// 单个服务器对一个域名的测速结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerBenchResult {
    pub server_id: i64,
    pub server_name: String,
    pub protocol: DnsProtocol,
    pub region: Region,
    /// 展示用端点
    #[serde(default)]
    pub address: String,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// 有效样本数
    pub samples: u32,
    pub min_ms: f64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub p95_ms: f64,
    /// 各阶段均值
    pub connect_ms: f64,
    pub tls_ms: f64,
    pub query_ms: f64,
    #[serde(default)]
    pub answers: Vec<IpAddr>,
    #[serde(default)]
    pub ttl: Option<u32>,
    #[serde(default)]
    pub rcode: Option<u8>,
    /// 0–100 综合评分
    pub score: f64,
    /// IP 归属地（可选，取决于是否启用在线查询）
    #[serde(default)]
    pub geo: Vec<IpGeo>,
}

impl ServerBenchResult {
    /// 转换成污染检测所需的观测结构。
    pub fn to_observation(&self) -> ServerObservation {
        ServerObservation {
            server_name: self.server_name.clone(),
            region: self.region,
            ok: self.ok,
            ips: self.answers.clone(),
            ttl: self.ttl,
            rcode: self.rcode,
            error: self.error.clone(),
            is_fake_ip: false,
        }
    }

    /// 延迟展示串（失败时显示错误摘要）。
    pub fn latency_display(&self) -> String {
        if self.ok {
            format!("{:.0}ms", self.avg_ms)
        } else {
            "失败".to_string()
        }
    }

    /// 状态展示串。
    pub fn status_display(&self) -> String {
        if self.ok {
            "正常".to_string()
        } else {
            match &self.error {
                Some(e) if e.contains("超时") => "超时".to_string(),
                Some(e) if e.contains("HTTP") => "HTTP 错误".to_string(),
                Some(e) if e.contains("TLS") => "TLS 失败".to_string(),
                Some(_) => "错误".to_string(),
                None => "失败".to_string(),
            }
        }
    }
}

/// 单个域名的完整测速结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainBenchResult {
    pub domain: String,
    pub region: Region,
    pub confidence: f32,
    pub results: Vec<ServerBenchResult>,
    /// 国内最优服务器名
    #[serde(default)]
    pub best_china: Option<String>,
    /// 国外最优服务器名
    #[serde(default)]
    pub best_global: Option<String>,
    /// 综合推荐（按域名区域）
    #[serde(default)]
    pub recommended: Option<String>,
    /// 本批次标识（同一次批量任务共享）
    #[serde(default)]
    pub batch_id: Option<String>,
    pub created_at: String,
}

impl DomainBenchResult {
    /// 成功返回的服务器数量。
    pub fn ok_count(&self) -> usize {
        self.results.iter().filter(|r| r.ok).count()
    }

    /// 汇总所有成功服务器的应答 IP（去重）。
    pub fn all_ips(&self) -> Vec<IpAddr> {
        let mut set = std::collections::BTreeSet::new();
        for r in &self.results {
            for ip in &r.answers {
                set.insert(*ip);
            }
        }
        set.into_iter().collect()
    }

    /// 转成污染检测的观测列表。
    pub fn observations(&self) -> Vec<ServerObservation> {
        self.results.iter().map(|r| r.to_observation()).collect()
    }

    /// 按区域取最优服务器（同区域内比较 score，跨区域无意义）。
    pub fn best_in_region(&self, region: Region) -> Option<&ServerBenchResult> {
        self.results
            .iter()
            .filter(|r| r.ok && r.region == region)
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
    }
}

/// 批量进度。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchProgress {
    pub done: usize,
    pub total: usize,
    pub current_domain: String,
}

/// 进度回调。
pub type ProgressFn = Arc<dyn Fn(BatchProgress) + Send + Sync>;

/// 测速引擎。
pub struct BenchmarkEngine {
    client: Arc<DnsClient>,
}

impl BenchmarkEngine {
    pub fn new(client: Arc<DnsClient>) -> Self {
        Self { client }
    }

    /// 取底层 DNS 客户端（供上层复用，避免重复建立 TLS 配置）。
    pub fn client(&self) -> Arc<DnsClient> {
        self.client.clone()
    }

    /// 默认构造（内部新建一个 [`DnsClient`]）。
    pub fn create() -> Self {
        Self::new(Arc::new(DnsClient::new()))
    }

    /// 单域名 × 多服务器测速。
    pub async fn bench_domain(
        &self,
        domain: &str,
        servers: &[DnsServer],
        opts: &BenchmarkOptions,
    ) -> Result<DomainBenchResult> {
        wire::validate_domain(domain)?;
        if servers.is_empty() {
            return Err(CoreError::Config("没有可用的 DNS 服务器".into()));
        }

        let sem = Arc::new(Semaphore::new(opts.concurrency));
        let mut set: JoinSet<ServerBenchResult> = JoinSet::new();

        for server in servers {
            let sem = sem.clone();
            let client = self.client.clone();
            let domain = domain.to_string();
            let server = server.clone();
            let opts = *opts;
            set.spawn(async move {
                let _permit = sem.acquire().await;
                bench_one(&client, &server, &domain, &opts).await
            });
        }

        let mut results = Vec::with_capacity(servers.len());
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(r) => results.push(r),
                Err(e) => tracing::error!("测速任务 panic: {}", e),
            }
        }
        // 保持与输入一致的顺序，输出更稳定
        results.sort_by_key(|r| {
            servers
                .iter()
                .position(|s| s.id == r.server_id && s.name == r.server_name)
                .unwrap_or(usize::MAX)
        });

        let classification = crate::classifier::default_classifier().classify(domain);
        let best_china = results
            .iter()
            .filter(|r| r.ok && r.region == Region::China)
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
            .map(|r| r.server_name.clone());
        let best_global = results
            .iter()
            .filter(|r| r.ok && r.region == Region::Global)
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
            .map(|r| r.server_name.clone());

        let recommended = match classification.region {
            Region::China => best_china.clone().or_else(|| best_global.clone()),
            Region::Global => best_global.clone().or_else(|| best_china.clone()),
            Region::Unknown => results
                .iter()
                .filter(|r| r.ok)
                .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
                .map(|r| r.server_name.clone()),
        };

        Ok(DomainBenchResult {
            domain: domain.to_string(),
            region: classification.region,
            confidence: classification.confidence,
            results,
            best_china,
            best_global,
            recommended,
            batch_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// 批量测速（域名维度并发受 `opts.concurrency` 之外单独控制，这里按域名串行 + 服务器并发）。
    pub async fn bench_batch(
        &self,
        domains: &[String],
        servers: &[DnsServer],
        opts: &BenchmarkOptions,
        progress: Option<ProgressFn>,
    ) -> Result<Vec<DomainBenchResult>> {
        let batch_id = format!("b{}", chrono::Utc::now().timestamp_millis());
        let total = domains.len();
        let mut out = Vec::with_capacity(total);
        for (i, d) in domains.iter().enumerate() {
            if let Some(cb) = &progress {
                cb(BatchProgress {
                    done: i,
                    total,
                    current_domain: d.clone(),
                });
            }
            match self.bench_domain(d, servers, opts).await {
                Ok(mut r) => {
                    r.batch_id = Some(batch_id.clone());
                    out.push(r);
                }
                Err(e) => {
                    tracing::warn!("域名 {} 测速失败: {}", d, e);
                }
            }
        }
        if let Some(cb) = &progress {
            cb(BatchProgress {
                done: total,
                total,
                current_domain: String::new(),
            });
        }
        Ok(out)
    }

    /// 对内置样本域名做全量测速（GUI 首页「开始全量测速」用）。
    pub async fn bench_full(
        &self,
        servers: &[DnsServer],
        opts: &BenchmarkOptions,
        progress: Option<ProgressFn>,
    ) -> Result<Vec<DomainBenchResult>> {
        let domains: Vec<String> = DEFAULT_SAMPLE_DOMAINS.iter().map(|s| s.to_string()).collect();
        self.bench_batch(&domains, servers, opts, progress).await
    }
}

/// 默认样本域名（国内 5 + 国外 5）。
pub const DEFAULT_SAMPLE_DOMAINS: &[&str] = &[
    "www.baidu.com",
    "www.taobao.com",
    "www.bilibili.com",
    "www.qq.com",
    "www.jd.com",
    "www.google.com",
    "github.com",
    "www.youtube.com",
    "www.cloudflare.com",
    "www.wikipedia.org",
];

/// 单个服务器的多轮测速。
async fn bench_one(
    client: &DnsClient,
    server: &DnsServer,
    domain: &str,
    opts: &BenchmarkOptions,
) -> ServerBenchResult {
    let qopts = opts.query_options();
    let mut totals: Vec<f64> = Vec::with_capacity(opts.rounds as usize);
    let mut connects = Vec::new();
    let mut tls = Vec::new();
    let mut queries = Vec::new();
    let mut answers: Vec<IpAddr> = Vec::new();
    let mut ttl: Option<u32> = None;
    let mut rcode: Option<u8> = None;
    let mut last_error: Option<String> = None;
    let mut last_transport = server.protocol;

    for round in 0..opts.rounds {
        let is_warmup = opts.warmup && round == 0;
        match client.query(server, domain, opts.qtype, &qopts).await {
            Ok(out) => {
                last_transport = out.transport;
                if !is_warmup {
                    totals.push(out.timings.total_ms);
                    connects.push(out.timings.connect_ms);
                    tls.push(out.timings.tls_ms);
                    queries.push(out.timings.query_ms);
                }
                answers = out.message.ip_answers();
                ttl = out.message.min_ttl();
                rcode = Some(out.message.rcode);
                last_error = None;
            }
            Err(e) => {
                last_error = Some(e.to_string());
            }
        }
    }

    if totals.is_empty() {
        return ServerBenchResult {
            server_id: server.id,
            server_name: server.name.clone(),
            protocol: server.protocol,
            region: server.region,
            address: server.display_endpoint(),
            ok: false,
            error: last_error.or_else(|| Some("全部轮次失败".into())),
            samples: 0,
            min_ms: 0.0,
            avg_ms: 0.0,
            max_ms: 0.0,
            p95_ms: 0.0,
            connect_ms: 0.0,
            tls_ms: 0.0,
            query_ms: 0.0,
            answers: Vec::new(),
            ttl: None,
            rcode: None,
            score: 0.0,
            geo: Vec::new(),
        };
    }

    let samples = totals.len() as u32;
    let min_ms = totals.iter().cloned().fold(f64::MAX, f64::min);
    let max_ms = totals.iter().cloned().fold(f64::MIN, f64::max);
    let avg_ms = totals.iter().sum::<f64>() / totals.len() as f64;
    let p95_ms = percentile(&totals, 0.95);
    let connect_ms = mean(&connects);
    let tls_ms = mean(&tls);
    let query_ms = mean(&queries);
    let score = compute_score(avg_ms, samples, opts.rounds, last_transport);

    ServerBenchResult {
        server_id: server.id,
        server_name: server.name.clone(),
        protocol: server.protocol,
        region: server.region,
        address: server.display_endpoint(),
        ok: true,
        error: None,
        samples,
        min_ms,
        avg_ms,
        max_ms,
        p95_ms,
        connect_ms,
        tls_ms,
        query_ms,
        answers,
        ttl,
        rcode,
        score,
        geo: Vec::new(),
    }
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// 百分位（样本不足 5 时退化为最大值）。
pub fn percentile(sorted_input: &[f64], p: f64) -> f64 {
    if sorted_input.is_empty() {
        return 0.0;
    }
    let mut v = sorted_input.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if v.len() < 5 {
        return *v.last().unwrap();
    }
    let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[idx.min(v.len() - 1)]
}

/// 综合评分（0–100，越大越好）。
///
/// `0.7 * 延迟分 + 0.3 * 成功率分 + 加密传输加分`，详见 `doc/02-核心引擎设计.md` 第 5 节。
pub fn compute_score(avg_ms: f64, samples: u32, rounds: u32, transport: DnsProtocol) -> f64 {
    let latency_score = (100.0 - avg_ms * 0.5).clamp(0.0, 100.0);
    let success_score = if rounds == 0 {
        0.0
    } else {
        (samples as f64 / rounds as f64 * 100.0).clamp(0.0, 100.0)
    };
    let bonus = if transport.is_encrypted() { 5.0 } else { 0.0 };
    (0.7 * latency_score + 0.3 * success_score + bonus).clamp(0.0, 100.0)
}

/// 导出 CSV（UTF-8 带 BOM，Excel 兼容）。
///
/// 列顺序见 `doc/03-数据模型与存储.md` 第 7.3 节。
pub fn export_csv(results: &[DomainBenchResult]) -> Result<String> {
    let mut out = String::from("\u{feff}");
    out.push_str(
        "domain,region,server_name,protocol,ok,avg_ms,min_ms,max_ms,p95_ms,connect_ms,tls_ms,query_ms,ttl,rcode,answers,score,error\n",
    );
    for r in results {
        for s in &r.results {
            let answers = s
                .answers
                .iter()
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
                .join("|");
            if s.ok {
                out.push_str(&format!(
                    "{},{},{},{},true,{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{},{},{},{:.1},\n",
                    csv_escape(&r.domain),
                    r.region.as_str(),
                    csv_escape(&s.server_name),
                    s.protocol.as_str(),
                    s.avg_ms,
                    s.min_ms,
                    s.max_ms,
                    s.p95_ms,
                    s.connect_ms,
                    s.tls_ms,
                    s.query_ms,
                    s.ttl.map(|t| t.to_string()).unwrap_or_default(),
                    s.rcode.map(|c| c.to_string()).unwrap_or_default(),
                    csv_escape(&answers),
                    s.score,
                ));
            } else {
                out.push_str(&format!(
                    "{},{},{},{},false,,,,,,,,,,,0.0,{}\n",
                    csv_escape(&r.domain),
                    r.region.as_str(),
                    csv_escape(&s.server_name),
                    s.protocol.as_str(),
                    csv_escape(s.error.as_deref().unwrap_or("未知错误")),
                ));
            }
        }
    }
    Ok(out)
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 为测速结果补充 IP 归属地（可选步骤，GUI/CLI 在启用在线查询时调用）。
pub fn enrich_geo(results: &mut [DomainBenchResult], geo: &dyn GeoProvider) {
    for r in results.iter_mut() {
        for s in r.results.iter_mut() {
            s.geo = s
                .answers
                .iter()
                .map(|ip| geo.lookup(*ip).unwrap_or_else(|| IpGeo::unknown(*ip)))
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_basic() {
        // 最近秩法：idx = round((n-1) * p)
        let v = vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0];
        assert!((percentile(&v, 0.95) - 100.0).abs() < 1e-6);
        assert!((percentile(&v, 0.5) - 60.0).abs() < 1e-6);
        assert!((percentile(&v, 0.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn percentile_small_sample_falls_back_to_max() {
        let v = vec![5.0, 10.0, 15.0];
        assert!((percentile(&v, 0.5) - 15.0).abs() < 1e-6);
    }

    #[test]
    fn percentile_empty() {
        assert_eq!(percentile(&[], 0.9), 0.0);
    }

    #[test]
    fn score_prefers_low_latency() {
        let fast = compute_score(20.0, 3, 3, DnsProtocol::Doh);
        let slow = compute_score(180.0, 3, 3, DnsProtocol::Doh);
        assert!(fast > slow);
        assert!(fast <= 100.0);
        assert!(slow >= 0.0);
    }

    #[test]
    fn score_penalises_failures() {
        let all_ok = compute_score(100.0, 3, 3, DnsProtocol::Udp);
        let some_fail = compute_score(100.0, 1, 3, DnsProtocol::Udp);
        assert!(all_ok > some_fail);
    }

    #[test]
    fn encrypted_transport_gets_bonus() {
        let udp = compute_score(100.0, 3, 3, DnsProtocol::Udp);
        let doh = compute_score(100.0, 3, 3, DnsProtocol::Doh);
        assert!((doh - udp - 5.0).abs() < 1e-6);
    }

    #[test]
    fn csv_has_bom_and_header() {
        let results = vec![DomainBenchResult {
            domain: "a.com".into(),
            region: Region::Global,
            confidence: 0.95,
            results: vec![ServerBenchResult {
                server_id: 1,
                server_name: "CF".into(),
                protocol: DnsProtocol::Doh,
                region: Region::Global,
                address: "https://x/dns-query".into(),
                ok: true,
                error: None,
                samples: 3,
                min_ms: 10.0,
                avg_ms: 12.0,
                max_ms: 15.0,
                p95_ms: 15.0,
                connect_ms: 5.0,
                tls_ms: 3.0,
                query_ms: 4.0,
                answers: vec!["1.2.3.4".parse().unwrap()],
                ttl: Some(300),
                rcode: Some(0),
                score: 90.0,
                geo: vec![],
            }],
            best_china: None,
            best_global: Some("CF".into()),
            recommended: Some("CF".into()),
            batch_id: None,
            created_at: "2026-10-14T00:00:00Z".into(),
        }];
        let csv = export_csv(&results).unwrap();
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("domain,region,server_name"));
        assert!(csv.contains("a.com,global,CF,doh,true"));
    }

    #[test]
    fn csv_escapes_commas() {
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn observation_conversion() {
        let r = ServerBenchResult {
            server_id: 1,
            server_name: "X".into(),
            protocol: DnsProtocol::Udp,
            region: Region::China,
            address: "1.1.1.1:53".into(),
            ok: true,
            error: None,
            samples: 1,
            min_ms: 1.0,
            avg_ms: 1.0,
            max_ms: 1.0,
            p95_ms: 1.0,
            connect_ms: 0.0,
            tls_ms: 0.0,
            query_ms: 1.0,
            answers: vec!["110.242.68.66".parse().unwrap()],
            ttl: Some(60),
            rcode: Some(0),
            score: 1.0,
            geo: vec![],
        };
        let o = r.to_observation();
        assert!(o.ok);
        assert_eq!(o.ips.len(), 1);
        assert_eq!(o.ttl, Some(60));
    }

    #[test]
    fn sample_domains_cover_both_regions() {
        assert!(DEFAULT_SAMPLE_DOMAINS.len() >= 10);
        assert!(DEFAULT_SAMPLE_DOMAINS.iter().any(|d| d.contains("baidu")));
        assert!(DEFAULT_SAMPLE_DOMAINS.iter().any(|d| d.contains("google")));
    }
}
