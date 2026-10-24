//! DNS 污染与 Fake-IP 异常检测。
//!
//! 12 项检测与风险分级算法见 `doc/05-污染与FakeIP检测规格.md`。
//! 关键区分：Fake-IP 命中不是污染，而是「本地 Fake-IP 正常工作」的证据。

use crate::data::BuiltinData;
use crate::error::Result;
use crate::geo::{self, GeoProvider, LocalGeo, OnlineGeo};
use crate::models::Region;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// 风险等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    pub fn label(&self) -> &'static str {
        match self {
            RiskLevel::Low => "低",
            RiskLevel::Medium => "中",
            RiskLevel::High => "高",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" | "低" => Some(RiskLevel::Low),
            "medium" | "mid" | "中" => Some(RiskLevel::Medium),
            "high" | "高" => Some(RiskLevel::High),
            _ => None,
        }
    }
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Fake-IP 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FakeIpStatus {
    /// 未发现 Fake-IP 应答
    Normal,
    /// 部分服务器返回 Fake-IP 段地址 → 本地 Fake-IP 生效
    Detected,
    /// 全部服务器都返回 Fake-IP 段地址 → 可疑
    Suspicious,
    /// 无有效观测
    Unknown,
}

impl FakeIpStatus {
    pub fn label(&self) -> &'static str {
        match self {
            FakeIpStatus::Normal => "正常",
            FakeIpStatus::Detected => "已生效",
            FakeIpStatus::Suspicious => "可疑",
            FakeIpStatus::Unknown => "未知",
        }
    }
}

/// 一条检测发现。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// 稳定标识，见 doc/05 第 3 节
    pub code: String,
    pub severity: RiskLevel,
    /// 面向用户的中文描述
    pub message: String,
    /// 证据（服务器名 + 具体值）
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// 单个 DNS 服务器的观测结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerObservation {
    pub server_name: String,
    pub region: Region,
    pub ok: bool,
    #[serde(default)]
    pub ips: Vec<IpAddr>,
    #[serde(default)]
    pub ttl: Option<u32>,
    #[serde(default)]
    pub rcode: Option<u8>,
    #[serde(default)]
    pub error: Option<String>,
    /// 该服务器返回的地址是否全部落在 Fake-IP 段
    #[serde(default)]
    pub is_fake_ip: bool,
}

/// 污染检测报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollutionReport {
    pub domain: String,
    pub region: Region,
    pub risk: RiskLevel,
    pub fake_ip: FakeIpStatus,
    /// 同区域服务器答案的一致率 0–1
    #[serde(default)]
    pub consistency: Option<f32>,
    #[serde(default)]
    pub fake_ip_hits: Vec<IpAddr>,
    #[serde(default)]
    pub poisoned_ips: Vec<IpAddr>,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub observations: Vec<ServerObservation>,
}

impl PollutionReport {
    /// 面向用户的一行摘要，供 CLI / GUI 直接展示。
    pub fn summary(&self) -> String {
        format!(
            "污染风险: {}    Fake-IP: {}",
            self.risk.label(),
            self.fake_ip.label()
        )
    }
}

/// 污染检测器。
pub struct PollutionDetector {
    /// 已知投毒 IP
    polluted: HashSet<IpAddr>,
    /// Fake-IP 网段
    fake_ip_net: ipnet::IpNet,
    /// 本地归属地判定
    geo: Arc<LocalGeo>,
    /// 可选的在线归属地
    online: Option<Arc<OnlineGeo>>,
    /// 判定 IP 是否属于中国大陆（用于 D4）
    china_provider: Arc<dyn GeoProvider>,
}

impl std::fmt::Debug for PollutionDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PollutionDetector")
            .field("polluted_count", &self.polluted.len())
            .field("fake_ip_net", &self.fake_ip_net)
            .finish_non_exhaustive()
    }
}

impl Default for PollutionDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl PollutionDetector {
    pub fn new() -> Self {
        Self::with_data_dir(None)
    }

    /// 从（可选）用户数据目录加载黑名单与 CN 网段。
    pub fn with_data_dir(dir: Option<&Path>) -> Self {
        let polluted_text = BuiltinData::load_or_embedded(dir, "polluted_ips.txt")
            .unwrap_or_else(|_| BuiltinData::polluted_ips().to_string());
        let polluted: HashSet<IpAddr> = BuiltinData::parse_lines(&polluted_text)
            .into_iter()
            .filter_map(|l| l.parse::<IpAddr>().ok())
            .collect();
        let geo = Arc::new(LocalGeo::with_data_dir(dir));
        let fake_ip_net = crate::DEFAULT_FAKE_IP_RANGE.parse().expect("默认 Fake-IP 段合法");
        Self {
            polluted,
            fake_ip_net,
            china_provider: geo.clone(),
            geo,
            online: None,
        }
    }

    /// 自定义 Fake-IP 段。
    pub fn with_fake_ip_range(mut self, cidr: &str) -> Result<Self> {
        self.fake_ip_net = geo::parse_net(cidr)?;
        Ok(self)
    }

    pub fn fake_ip_range(&self) -> String {
        self.fake_ip_net.to_string()
    }

    /// 启用在线归属地查询（默认关闭）。
    pub fn with_online_geo(mut self, enabled: bool) -> Self {
        self.online = if enabled {
            Some(Arc::new(OnlineGeo::new()))
        } else {
            None
        };
        self
    }

    /// 启用在线归属地查询并指定其出站代理（None = 直连）。
    pub fn with_online_geo_and_proxy(
        mut self,
        enabled: bool,
        proxy: Option<crate::net::ProxyUrl>,
    ) -> Self {
        self.online = if enabled {
            Some(Arc::new(OnlineGeo::new().with_proxy(proxy)))
        } else {
            None
        };
        self
    }

    pub fn online_geo_enabled(&self) -> bool {
        self.online.is_some()
    }

    /// 取本地归属地判定器（供上层复用，避免重复解析 CN 网段表）。
    pub fn local_geo(&self) -> &Arc<LocalGeo> {
        &self.geo
    }

    /// 核心：分析一个域名的观测结果。
    pub fn analyze(
        &self,
        domain: &str,
        region: Region,
        obs: &[ServerObservation],
    ) -> PollutionReport {
        let mut findings: Vec<Finding> = Vec::new();
        let mut poisoned_ips: BTreeSet<IpAddr> = BTreeSet::new();
        let mut fake_ip_hits: BTreeSet<IpAddr> = BTreeSet::new();

        let ok_obs: Vec<&ServerObservation> = obs.iter().filter(|o| o.ok).collect();
        let failed = obs.len() - ok_obs.len();

        // 无任何成功观测
        if ok_obs.is_empty() {
            if !obs.is_empty() {
                findings.push(Finding {
                    code: "ALL_SERVERS_FAILED".into(),
                    severity: RiskLevel::High,
                    message: "所有 DNS 服务器均查询失败，可能是本地网络问题而非污染".into(),
                    evidence: obs
                        .iter()
                        .take(5)
                        .map(|o| {
                            format!(
                                "{}: {}",
                                o.server_name,
                                o.error.clone().unwrap_or_else(|| "无响应".into())
                            )
                        })
                        .collect(),
                });
            }
            return PollutionReport {
                domain: domain.to_string(),
                region,
                risk: RiskLevel::Medium,
                fake_ip: FakeIpStatus::Unknown,
                consistency: None,
                fake_ip_hits: Vec::new(),
                poisoned_ips: Vec::new(),
                findings,
                observations: obs.to_vec(),
            };
        }

        // ——— D1 已知投毒 IP ———
        for o in &ok_obs {
            for ip in &o.ips {
                if self.polluted.contains(ip) {
                    poisoned_ips.insert(*ip);
                    findings.push(Finding {
                        code: "POISONED_IP_HIT".into(),
                        severity: RiskLevel::High,
                        message: "返回了已知的 DNS 投毒 IP".into(),
                        evidence: vec![format!("{} → {}", o.server_name, ip)],
                    });
                }
            }
        }

        // ——— D2 / D9 私网、黑洞地址 ———
        for o in &ok_obs {
            for ip in &o.ips {
                if ip.is_unspecified() || ip.is_loopback() {
                    findings.push(Finding {
                        code: "BLACKHOLE_ANSWER".into(),
                        severity: RiskLevel::High,
                        message: "返回了黑洞地址（0.0.0.0 / 127.0.0.1），通常意味着被劫持或域名被屏蔽"
                            .into(),
                        evidence: vec![format!("{} → {}", o.server_name, ip)],
                    });
                } else if geo::is_private(*ip) {
                    findings.push(Finding {
                        code: "PRIVATE_IP_ANSWER".into(),
                        severity: RiskLevel::High,
                        message: "返回了私网地址，正常公网 DNS 不应如此".into(),
                        evidence: vec![format!("{} → {}", o.server_name, ip)],
                    });
                }
            }
        }

        // ——— D3 Fake-IP 检测 ———
        for o in &ok_obs {
            for ip in &o.ips {
                if self.fake_ip_net.contains(ip) {
                    fake_ip_hits.insert(*ip);
                }
            }
        }
        let fake_ip = if fake_ip_hits.is_empty() {
            FakeIpStatus::Normal
        } else if ok_obs.iter().all(|o| {
            !o.ips.is_empty() && o.ips.iter().all(|ip| self.fake_ip_net.contains(ip))
        }) {
            FakeIpStatus::Suspicious
        } else {
            FakeIpStatus::Detected
        };
        if !fake_ip_hits.is_empty() {
            findings.push(Finding {
                code: "FAKE_IP_DETECTED".into(),
                severity: RiskLevel::Low,
                message: format!(
                    "检测到 Fake-IP 应答（{}），说明本机 Clash Fake-IP 已生效；如需真实解析结果请改用 DoH/DoT 测试",
                    self.fake_ip_net
                ),
                evidence: fake_ip_hits
                    .iter()
                    .map(|ip| ip.to_string())
                    .collect(),
            });
        }

        // ——— D5 同区域一致性 + D4 跨境不一致 ———
        let cn_answers: Vec<&ServerObservation> =
            ok_obs.iter().copied().filter(|o| o.region == Region::China).collect();
        let gl_answers: Vec<&ServerObservation> = ok_obs
            .iter()
            .copied()
            .filter(|o| o.region == Region::Global)
            .collect();

        let cn_set = union_ips(&cn_answers);
        let gl_set = union_ips(&gl_answers);

        let consistency = compute_consistency(&ok_obs);

        // D4：国外域名被国内 DNS 解析到国内 IP，且与国外结果无交集
        if region == Region::Global && !cn_set.is_empty() && !gl_set.is_empty() {
            let cn_all_china = cn_set.iter().all(|ip| {
                self.china_provider
                    .lookup(*ip)
                    .map(|g| g.is_china)
                    .unwrap_or(false)
            });
            let disjoint = cn_set.is_disjoint(&gl_set);
            if cn_all_china && disjoint {
                findings.push(Finding {
                    code: "CROSS_REGION_MISMATCH".into(),
                    severity: RiskLevel::High,
                    message: "国外域名经国内 DNS 解析到国内地址，且与国外 DNS 结果完全无交集，疑似污染"
                        .into(),
                    evidence: vec![
                        format!("国内 DNS: {}", join_ips(&cn_set)),
                        format!("国外 DNS: {}", join_ips(&gl_set)),
                    ],
                });
            }
        }

        // ——— D6 NXDOMAIN 矛盾 ———
        let nx = ok_obs.iter().filter(|o| o.rcode == Some(3)).count();
        let answered = ok_obs.iter().filter(|o| !o.ips.is_empty()).count();
        if nx > 0 && answered > 0 {
            findings.push(Finding {
                code: "NXDOMAIN_CONFLICT".into(),
                severity: RiskLevel::High,
                message: "部分 DNS 返回 NXDOMAIN（域名不存在），另一部分正常返回地址，结果矛盾".into(),
                evidence: ok_obs
                    .iter()
                    .filter(|o| o.rcode == Some(3))
                    .map(|o| format!("{} → NXDOMAIN", o.server_name))
                    .collect(),
            });
        }

        // ——— D7 TTL 异常 ———
        for o in &ok_obs {
            if let Some(ttl) = o.ttl {
                if ttl == 0 || ttl < 5 || ttl > 86_400 {
                    findings.push(Finding {
                        code: "TTL_ANOMALY".into(),
                        severity: RiskLevel::Medium,
                        message: format!("TTL 异常（{} 秒），可能被中间设备改写", ttl),
                        evidence: vec![format!("{} → TTL={}", o.server_name, ttl)],
                    });
                }
            }
        }

        // ——— D8 大量失败 ———
        if obs.len() >= 3 && failed * 2 >= obs.len() {
            findings.push(Finding {
                code: "SERVFAIL_CLUSTER".into(),
                severity: RiskLevel::Medium,
                message: format!("{}/{} 个服务器查询失败，网络链路可能存在问题", failed, obs.len()),
                evidence: obs
                    .iter()
                    .filter(|o| !o.ok)
                    .take(5)
                    .map(|o| format!("{}: {}", o.server_name, o.error.clone().unwrap_or_default()))
                    .collect(),
            });
        }

        // ——— D10 空应答 ———
        let empty_noerror = ok_obs
            .iter()
            .filter(|o| o.rcode == Some(0) && o.ips.is_empty())
            .count();
        if empty_noerror > 0 && empty_noerror == answered.max(1) && answered == 0 {
            findings.push(Finding {
                code: "EMPTY_ANSWER".into(),
                severity: RiskLevel::Medium,
                message: "NOERROR 但没有任何 A/AAAA 记录".into(),
                evidence: ok_obs
                    .iter()
                    .filter(|o| o.rcode == Some(0))
                    .map(|o| o.server_name.clone())
                    .collect(),
            });
        }

        // ——— 服务器数量不足（提示级） ———
        if ok_obs.len() == 1 {
            findings.push(Finding {
                code: "INSUFFICIENT_SERVERS".into(),
                severity: RiskLevel::Low,
                message: "只有 1 个服务器有有效应答，无法做交叉比对，结论仅供参考".into(),
                evidence: Vec::new(),
            });
        }

        // ——— 风险分级 ———
        let mut risk = findings
            .iter()
            .map(|f| f.severity)
            .max()
            .unwrap_or(RiskLevel::Low);

        if risk == RiskLevel::High {
            // 全部失败时降级为 Medium，避免误报
            if findings.iter().any(|f| f.code == "ALL_SERVERS_FAILED") {
                risk = RiskLevel::Medium;
            }
        } else if let Some(c) = consistency {
            if c >= 0.9 {
                risk = RiskLevel::Low;
            }
        }

        PollutionReport {
            domain: domain.to_string(),
            region,
            risk,
            fake_ip,
            consistency,
            fake_ip_hits: fake_ip_hits.into_iter().collect(),
            poisoned_ips: poisoned_ips.into_iter().collect(),
            findings,
            observations: obs.to_vec(),
        }
    }
}

fn union_ips(obs: &[&ServerObservation]) -> BTreeSet<IpAddr> {
    obs.iter().flat_map(|o| o.ips.iter().copied()).collect()
}

fn join_ips(set: &BTreeSet<IpAddr>) -> String {
    set.iter()
        .map(|ip| ip.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Jaccard 相似度。
fn jaccard(a: &BTreeSet<IpAddr>, b: &BTreeSet<IpAddr>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count() as f32;
    let uni = a.union(b).count() as f32;
    if uni == 0.0 {
        1.0
    } else {
        inter / uni
    }
}

/// 计算一致性：按区域分组，区域内两两 Jaccard 平均，再按区域平均。
fn compute_consistency(obs: &[&ServerObservation]) -> Option<f32> {
    if obs.len() < 2 {
        return None;
    }
    let mut by_region: HashMap<&str, Vec<BTreeSet<IpAddr>>> = HashMap::new();
    for o in obs {
        let key = match o.region {
            Region::China => "china",
            Region::Global => "global",
            Region::Unknown => "unknown",
        };
        by_region
            .entry(key)
            .or_default()
            .push(o.ips.iter().copied().collect());
    }

    let mut region_scores: Vec<f32> = Vec::new();
    for (region, sets) in &by_region {
        if sets.len() < 2 {
            continue;
        }
        let mut sum = 0.0f32;
        let mut pairs = 0usize;
        for i in 0..sets.len() {
            for j in (i + 1)..sets.len() {
                sum += jaccard(&sets[i], &sets[j]);
                pairs += 1;
            }
        }
        let score = if pairs == 0 { 1.0 } else { sum / pairs as f32 };
        let weight = if *region == "unknown" { 0.5 } else { 1.0 };
        region_scores.push(score * weight);
    }

    if region_scores.is_empty() {
        return None;
    }
    Some(region_scores.iter().sum::<f32>() / region_scores.len() as f32)
}

/// 全局默认检测器（进程内构建一次）。
pub fn default_detector() -> Arc<PollutionDetector> {
    static INSTANCE: OnceLock<Arc<PollutionDetector>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(PollutionDetector::new())).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(name: &str, region: Region, ips: &[&str]) -> ServerObservation {
        ServerObservation {
            server_name: name.into(),
            region,
            ok: true,
            ips: ips.iter().map(|s| s.parse().unwrap()).collect(),
            ttl: Some(120),
            rcode: Some(0),
            error: None,
            is_fake_ip: false,
        }
    }

    fn det() -> PollutionDetector {
        PollutionDetector::new()
    }

    #[test]
    fn clean_domain_is_low_risk() {
        let d = det();
        let obs = vec![
            obs("阿里 DNS", Region::China, &["110.242.68.66"]),
            obs("DNSPod", Region::China, &["110.242.68.66"]),
            obs("Cloudflare", Region::Global, &["110.242.68.66"]),
        ];
        let r = d.analyze("www.baidu.com", Region::China, &obs);
        assert_eq!(r.risk, RiskLevel::Low);
        assert_eq!(r.fake_ip, FakeIpStatus::Normal);
        assert!(r.consistency.unwrap() > 0.9);
    }

    #[test]
    fn poisoned_ip_triggers_high_risk() {
        let d = det();
        let obs = vec![
            obs("阿里 DNS", Region::China, &["243.185.187.39"]),
            obs("Cloudflare", Region::Global, &["142.250.72.4"]),
        ];
        let r = d.analyze("www.google.com", Region::Global, &obs);
        assert_eq!(r.risk, RiskLevel::High);
        assert!(r.findings.iter().any(|f| f.code == "POISONED_IP_HIT"));
        assert_eq!(r.poisoned_ips.len(), 1);
    }

    #[test]
    fn fake_ip_detected() {
        let d = det();
        let obs = vec![
            obs("系统 DNS", Region::Unknown, &["198.18.0.5"]),
            obs("Cloudflare", Region::Global, &["140.82.121.3"]),
        ];
        let r = d.analyze("github.com", Region::Global, &obs);
        assert_eq!(r.fake_ip, FakeIpStatus::Detected);
        assert_eq!(r.fake_ip_hits, vec!["198.18.0.5".parse::<IpAddr>().unwrap()]);
        assert!(r.findings.iter().any(|f| f.code == "FAKE_IP_DETECTED"));
    }

    #[test]
    fn all_fake_ip_is_suspicious() {
        let d = det();
        let obs = vec![
            obs("A", Region::China, &["198.18.0.5"]),
            obs("B", Region::Global, &["198.18.0.6"]),
        ];
        let r = d.analyze("x.com", Region::Global, &obs);
        assert_eq!(r.fake_ip, FakeIpStatus::Suspicious);
    }

    #[test]
    fn private_ip_flagged() {
        let d = det();
        let obs = vec![obs("A", Region::China, &["192.168.1.1"])];
        let r = d.analyze("x.com", Region::China, &obs);
        assert!(r.findings.iter().any(|f| f.code == "PRIVATE_IP_ANSWER"));
    }

    #[test]
    fn blackhole_flagged() {
        let d = det();
        let obs = vec![obs("A", Region::China, &["0.0.0.0"])];
        let r = d.analyze("x.com", Region::China, &obs);
        assert!(r.findings.iter().any(|f| f.code == "BLACKHOLE_ANSWER"));
        assert_eq!(r.risk, RiskLevel::High);
    }

    #[test]
    fn cross_region_mismatch() {
        let d = det();
        let obs = vec![
            obs("阿里 DNS", Region::China, &["110.242.68.66"]),
            obs("Cloudflare", Region::Global, &["140.82.121.3"]),
        ];
        let r = d.analyze("github.com", Region::Global, &obs);
        assert!(r.findings.iter().any(|f| f.code == "CROSS_REGION_MISMATCH"));
        assert_eq!(r.risk, RiskLevel::High);
    }

    #[test]
    fn nxdomain_conflict() {
        let d = det();
        let mut a = obs("A", Region::China, &[]);
        a.rcode = Some(3);
        let b = obs("B", Region::Global, &["1.2.3.4"]);
        let r = d.analyze("x.com", Region::Global, &[a, b]);
        assert!(r.findings.iter().any(|f| f.code == "NXDOMAIN_CONFLICT"));
    }

    #[test]
    fn ttl_anomaly() {
        let d = det();
        let mut a = obs("A", Region::China, &["1.2.3.4"]);
        a.ttl = Some(0);
        let mut b = obs("B", Region::China, &["1.2.3.4"]);
        b.ttl = Some(999_999);
        let r = d.analyze("x.com", Region::China, &[a, b]);
        let n = r
            .findings
            .iter()
            .filter(|f| f.code == "TTL_ANOMALY")
            .count();
        assert_eq!(n, 2);
    }

    #[test]
    fn all_failed_is_medium_not_high() {
        let d = det();
        let obs = vec![
            ServerObservation {
                server_name: "A".into(),
                region: Region::China,
                ok: false,
                ips: vec![],
                ttl: None,
                rcode: None,
                error: Some("超时".into()),
                is_fake_ip: false,
            },
            ServerObservation {
                server_name: "B".into(),
                region: Region::Global,
                ok: false,
                ips: vec![],
                ttl: None,
                rcode: None,
                error: Some("超时".into()),
                is_fake_ip: false,
            },
        ];
        let r = d.analyze("x.com", Region::Global, &obs);
        assert_eq!(r.risk, RiskLevel::Medium);
        assert_eq!(r.fake_ip, FakeIpStatus::Unknown);
        assert!(r.findings.iter().any(|f| f.code == "ALL_SERVERS_FAILED"));
    }

    #[test]
    fn single_server_adds_hint() {
        let d = det();
        let obs = vec![obs("A", Region::China, &["1.2.3.4"])];
        let r = d.analyze("x.com", Region::China, &obs);
        assert!(r.findings.iter().any(|f| f.code == "INSUFFICIENT_SERVERS"));
        assert!(r.consistency.is_none());
    }

    #[test]
    fn custom_fake_ip_range() {
        let d = det().with_fake_ip_range("10.0.0.0/8").unwrap();
        assert_eq!(d.fake_ip_range(), "10.0.0.0/8");
        let obs = vec![obs("A", Region::China, &["10.1.2.3"])];
        let r = d.analyze("x.com", Region::China, &obs);
        assert_eq!(r.fake_ip, FakeIpStatus::Suspicious);
        // 同时会命中私网检测
        assert!(r.findings.iter().any(|f| f.code == "PRIVATE_IP_ANSWER"));
    }

    #[test]
    fn jaccard_edge_cases() {
        let e: BTreeSet<IpAddr> = BTreeSet::new();
        assert_eq!(jaccard(&e, &e), 1.0);
        let a: BTreeSet<IpAddr> = ["1.1.1.1".parse().unwrap()].into_iter().collect();
        assert_eq!(jaccard(&a, &a), 1.0);
        assert_eq!(jaccard(&a, &e), 0.0);
    }
}
