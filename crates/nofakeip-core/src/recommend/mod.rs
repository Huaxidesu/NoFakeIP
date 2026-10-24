//! DNS 智能推荐：把实测数据汇总成「国内用哪个、国外用哪个」的结论与理由。
//!
//! 契约见 `doc/02-核心引擎设计.md` 第 9 节。
//! 注意：跨区域比较延迟没有意义（国内 20ms vs 国外 120ms 是物理现实），
//! 因此国内 / 国外分别排序，只做同区域比较。

use crate::benchmark::DomainBenchResult;
use crate::models::{DnsProtocol, Region};
use crate::pollution::{PollutionReport, RiskLevel};
use serde::{Deserialize, Serialize};

/// 被推荐的服务器。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecommendedServer {
    pub name: String,
    pub address: String,
    pub protocol: DnsProtocol,
    pub region: Region,
    /// 该区域内的平均延迟
    pub avg_ms: f64,
    /// 平均评分
    pub score: f64,
    /// 成功率 0–1
    pub success_rate: f32,
    /// 参与统计的样本数
    pub samples: u32,
}

/// 推荐结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    #[serde(default)]
    pub china: Option<RecommendedServer>,
    #[serde(default)]
    pub foreign: Option<RecommendedServer>,
    /// 中文可读理由
    #[serde(default)]
    pub reasons: Vec<String>,
    pub china_avg_ms: f64,
    pub foreign_avg_ms: f64,
    /// 污染率 0–1（本次样本中高风险占比）
    pub pollution_rate: f32,
    pub generated_at: String,
    /// 备用选项（同区域第 2、3 名），供用户手动切换
    #[serde(default)]
    pub alternates: Vec<RecommendedServer>,
}

impl Recommendation {
    /// 一行摘要。
    pub fn summary(&self) -> String {
        let c = self
            .china
            .as_ref()
            .map(|s| format!("{} ({:.0}ms)", s.name, s.avg_ms))
            .unwrap_or_else(|| "无可用".into());
        let f = self
            .foreign
            .as_ref()
            .map(|s| format!("{} ({:.0}ms)", s.name, s.avg_ms))
            .unwrap_or_else(|| "无可用".into());
        format!("国内: {}    国外: {}", c, f)
    }
}

/// 推荐引擎。
pub struct Recommender;

/// 单个服务器的聚合统计。
#[derive(Debug, Clone, Default)]
struct Agg {
    name: String,
    address: String,
    protocol: Option<DnsProtocol>,
    region: Region,
    total_ms: f64,
    total_score: f64,
    ok: u32,
    total: u32,
}

impl Recommender {
    /// 基于批量测速结果（可附带污染报告）生成推荐。
    pub fn from_benchmark(
        results: &[DomainBenchResult],
        reports: &[PollutionReport],
    ) -> Recommendation {
        let mut aggs: Vec<Agg> = Vec::new();

        for r in results {
            for s in &r.results {
                let entry = aggs
                    .iter_mut()
                    .find(|a| a.name == s.server_name && a.region == s.region);
                let a = match entry {
                    Some(a) => a,
                    None => {
                        aggs.push(Agg {
                            name: s.server_name.clone(),
                            address: s.address.clone(),
                            protocol: Some(s.protocol),
                            region: s.region,
                            ..Default::default()
                        });
                        aggs.last_mut().unwrap()
                    }
                };
                a.total += 1;
                if s.ok {
                    a.ok += 1;
                    a.total_ms += s.avg_ms;
                    a.total_score += s.score;
                    a.address = s.address.clone();
                    a.protocol = Some(s.protocol);
                }
            }
        }

        let to_recommended = |a: &Agg| -> RecommendedServer {
            let ok = a.ok.max(1) as f64;
            RecommendedServer {
                name: a.name.clone(),
                address: a.address.clone(),
                protocol: a.protocol.unwrap_or(DnsProtocol::Udp),
                region: a.region,
                avg_ms: a.total_ms / ok,
                score: a.total_score / ok,
                success_rate: if a.total == 0 {
                    0.0
                } else {
                    a.ok as f32 / a.total as f32
                },
                samples: a.ok,
            }
        };

        let mut china: Vec<RecommendedServer> = aggs
            .iter()
            .filter(|a| a.region == Region::China && a.ok > 0)
            .map(&to_recommended)
            .collect();
        let mut foreign: Vec<RecommendedServer> = aggs
            .iter()
            .filter(|a| a.region == Region::Global && a.ok > 0)
            .map(&to_recommended)
            .collect();

        // 同区域内：先比综合评分，再比延迟
        let cmp = |x: &RecommendedServer, y: &RecommendedServer| {
            y.score
                .partial_cmp(&x.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    x.avg_ms
                        .partial_cmp(&y.avg_ms)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        };
        china.sort_by(cmp);
        foreign.sort_by(cmp);

        let pollution_rate = if reports.is_empty() {
            0.0
        } else {
            reports
                .iter()
                .filter(|r| r.risk == RiskLevel::High)
                .count() as f32
                / reports.len() as f32
        };

        let mut reasons: Vec<String> = Vec::new();
        let china_avg_ms = china.first().map(|s| s.avg_ms).unwrap_or(0.0);
        let foreign_avg_ms = foreign.first().map(|s| s.avg_ms).unwrap_or(0.0);

        if let Some(c) = china.first() {
            reasons.push(format!(
                "国内平均延迟最低：{} {:.0}ms（成功率 {:.0}%）",
                c.name,
                c.avg_ms,
                c.success_rate * 100.0
            ));
        } else {
            reasons.push("没有可用的国内 DNS（全部测试失败）".into());
        }
        if let Some(f) = foreign.first() {
            reasons.push(format!(
                "国外平均延迟最低：{} {:.0}ms（成功率 {:.0}%）",
                f.name,
                f.avg_ms,
                f.success_rate * 100.0
            ));
        } else {
            reasons.push("没有可用的国外 DNS（全部测试失败）".into());
        }
        reasons.push(format!("本次样本污染率：{:.0}%", pollution_rate * 100.0));

        if let Some(c) = china.first() {
            if c.success_rate < 1.0 {
                reasons.push(format!(
                    "注意：{} 有 {:.0}% 的查询失败，建议准备备用 DNS",
                    c.name,
                    (1.0 - c.success_rate) * 100.0
                ));
            }
        }

        let mut alternates: Vec<RecommendedServer> = Vec::new();
        alternates.extend(china.iter().skip(1).take(2).cloned());
        alternates.extend(foreign.iter().skip(1).take(2).cloned());

        Recommendation {
            china: china.into_iter().next(),
            foreign: foreign.into_iter().next(),
            reasons,
            china_avg_ms,
            foreign_avg_ms,
            pollution_rate,
            generated_at: chrono::Utc::now().to_rfc3339(),
            alternates,
        }
    }

    /// 单域名推荐：只关心这个域名应该用哪个 DNS。
    pub fn for_domain(domain: &str, region: Region, result: &DomainBenchResult) -> Recommendation {
        let mut reasons = Vec::new();
        let target = match region {
            Region::China => Region::China,
            Region::Global => Region::Global,
            Region::Unknown => Region::Unknown,
        };

        let pick = |r: Region| -> Option<RecommendedServer> {
            result
                .results
                .iter()
                .filter(|s| s.ok && (r == Region::Unknown || s.region == r))
                .max_by(|a, b| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|s| RecommendedServer {
                    name: s.server_name.clone(),
                    address: s.address.clone(),
                    protocol: s.protocol,
                    region: s.region,
                    avg_ms: s.avg_ms,
                    score: s.score,
                    success_rate: if result.results.iter().any(|x| x.server_name == s.server_name) {
                        1.0
                    } else {
                        0.0
                    },
                    samples: s.samples,
                })
        };

        let best = pick(target).or_else(|| pick(Region::Unknown));
        if let Some(b) = &best {
            reasons.push(format!(
                "{} 属于{}域名，实测最快的是 {}（{:.0}ms）",
                domain,
                region.label(),
                b.name,
                b.avg_ms
            ));
        } else {
            reasons.push(format!("{} 的所有服务器测试均失败", domain));
        }

        Recommendation {
            china: if target == Region::China { best.clone() } else { None },
            foreign: if target == Region::Global { best.clone() } else { None },
            reasons,
            china_avg_ms: if target == Region::China {
                best.as_ref().map(|b| b.avg_ms).unwrap_or(0.0)
            } else {
                0.0
            },
            foreign_avg_ms: if target == Region::Global {
                best.as_ref().map(|b| b.avg_ms).unwrap_or(0.0)
            } else {
                0.0
            },
            pollution_rate: 0.0,
            generated_at: chrono::Utc::now().to_rfc3339(),
            alternates: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::ServerBenchResult;
    use std::net::IpAddr;

    fn server(name: &str, region: Region, ok: bool, avg: f64, score: f64) -> ServerBenchResult {
        ServerBenchResult {
            server_id: 0,
            server_name: name.into(),
            protocol: DnsProtocol::Doh,
            region,
            address: format!("https://{}/dns-query", name),
            ok,
            error: if ok { None } else { Some("超时".into()) },
            samples: if ok { 3 } else { 0 },
            min_ms: avg,
            avg_ms: avg,
            max_ms: avg,
            p95_ms: avg,
            connect_ms: 1.0,
            tls_ms: 1.0,
            query_ms: avg,
            answers: if ok {
                vec!["1.2.3.4".parse::<IpAddr>().unwrap()]
            } else {
                vec![]
            },
            ttl: Some(60),
            rcode: if ok { Some(0) } else { None },
            score,
            geo: vec![],
        }
    }

    fn domain_result(domain: &str, region: Region, servers: Vec<ServerBenchResult>) -> DomainBenchResult {
        DomainBenchResult {
            domain: domain.into(),
            region,
            confidence: 0.95,
            results: servers,
            best_china: None,
            best_global: None,
            recommended: None,
            batch_id: None,
            created_at: "2026-10-14T00:00:00Z".into(),
        }
    }

    #[test]
    fn picks_fastest_per_region() {
        let results = vec![
            domain_result(
                "www.baidu.com",
                Region::China,
                vec![
                    server("阿里 DNS", Region::China, true, 18.0, 95.0),
                    server("DNSPod", Region::China, true, 25.0, 90.0),
                ],
            ),
            domain_result(
                "github.com",
                Region::Global,
                vec![
                    server("Cloudflare", Region::Global, true, 120.0, 80.0),
                    server("Google DNS", Region::Global, true, 140.0, 70.0),
                ],
            ),
        ];
        let rec = Recommender::from_benchmark(&results, &[]);
        assert_eq!(rec.china.as_ref().unwrap().name, "阿里 DNS");
        assert_eq!(rec.foreign.as_ref().unwrap().name, "Cloudflare");
        assert!(!rec.reasons.is_empty());
        assert_eq!(rec.pollution_rate, 0.0);
    }

    #[test]
    fn region_does_not_cross_compare() {
        // 国内 18ms 的服务器不应被推荐给国外场景
        let results = vec![domain_result(
            "github.com",
            Region::Global,
            vec![
                server("阿里 DNS", Region::China, true, 18.0, 95.0),
                server("Cloudflare", Region::Global, true, 120.0, 80.0),
            ],
        )];
        let rec = Recommender::from_benchmark(&results, &[]);
        assert_eq!(rec.foreign.as_ref().unwrap().name, "Cloudflare");
        assert_eq!(rec.china.as_ref().unwrap().name, "阿里 DNS");
    }

    #[test]
    fn failed_servers_excluded() {
        let results = vec![domain_result(
            "x.com",
            Region::Global,
            vec![
                server("Broken", Region::Global, false, 0.0, 0.0),
                server("Good", Region::Global, true, 100.0, 60.0),
            ],
        )];
        let rec = Recommender::from_benchmark(&results, &[]);
        assert_eq!(rec.foreign.as_ref().unwrap().name, "Good");
    }

    #[test]
    fn no_data_yields_empty_recommendation() {
        let rec = Recommender::from_benchmark(&[], &[]);
        assert!(rec.china.is_none());
        assert!(rec.foreign.is_none());
        assert!(!rec.reasons.is_empty());
    }

    #[test]
    fn pollution_rate_computed() {
        let reports = vec![
            PollutionReport {
                domain: "a.com".into(),
                region: Region::Global,
                risk: RiskLevel::High,
                fake_ip: crate::pollution::FakeIpStatus::Normal,
                consistency: None,
                fake_ip_hits: vec![],
                poisoned_ips: vec![],
                findings: vec![],
                observations: vec![],
            },
            PollutionReport {
                domain: "b.com".into(),
                region: Region::China,
                risk: RiskLevel::Low,
                fake_ip: crate::pollution::FakeIpStatus::Normal,
                consistency: None,
                fake_ip_hits: vec![],
                poisoned_ips: vec![],
                findings: vec![],
                observations: vec![],
            },
        ];
        let rec = Recommender::from_benchmark(&[], &reports);
        assert!((rec.pollution_rate - 0.5).abs() < 1e-6);
    }

    #[test]
    fn for_domain_prefers_matching_region() {
        let r = domain_result(
            "github.com",
            Region::Global,
            vec![
                server("阿里 DNS", Region::China, true, 18.0, 95.0),
                server("Cloudflare", Region::Global, true, 120.0, 80.0),
            ],
        );
        let rec = Recommender::for_domain("github.com", Region::Global, &r);
        assert_eq!(rec.foreign.as_ref().unwrap().name, "Cloudflare");
        assert!(rec.china.is_none());
    }

    #[test]
    fn summary_format() {
        let results = vec![domain_result(
            "x.com",
            Region::China,
            vec![server("阿里 DNS", Region::China, true, 18.0, 95.0)],
        )];
        let rec = Recommender::from_benchmark(&results, &[]);
        assert!(rec.summary().contains("阿里 DNS"));
    }
}
