//! 诊断报告生成与导出。
//!
//! 提供三种格式：
//! - **Markdown**：给人看（可复制到 issue / 笔记）
//! - **JSON**：给程序看（CLI `--json`、GUI 导出，字段只增不减）
//! - **CSV**：给表格看（委托 [`crate::benchmark::export_csv`]）
//!
//! **范围说明**：本模块只输出「诊断报告」，**不生成任何 Clash / OpenClash 配置**。

use crate::benchmark::{self, DomainBenchResult};
use crate::diagnose::DomainDiagnosis;
use crate::error::Result;
use crate::pollution::{PollutionReport, RiskLevel};
use crate::recommend::Recommendation;
use serde::Serialize;

/// 报告输出选项。
#[derive(Debug, Clone, Copy)]
pub struct ReportOptions {
    /// 是否包含每个服务器的原始观测
    pub include_observations: bool,
    /// 是否包含 IP 归属地列
    pub include_geo: bool,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            include_observations: true,
            include_geo: true,
        }
    }
}

/// JSON 导出外壳（`--json` 的稳定契约，见 `doc/07-CLI规格.md` 第 5 节）。
#[derive(Debug, Serialize)]
pub struct JsonEnvelope<'a> {
    pub version: u32,
    pub tool: &'a str,
    pub engine: &'a str,
    pub command: &'a str,
    pub generated_at: String,
    pub results: &'a [DomainBenchResult],
    pub reports: &'a [PollutionReport],
    pub recommendation: &'a Recommendation,
}

impl<'a> JsonEnvelope<'a> {
    pub fn new(
        command: &'a str,
        results: &'a [DomainBenchResult],
        reports: &'a [PollutionReport],
        recommendation: &'a Recommendation,
    ) -> Self {
        Self {
            version: 1,
            tool: "nofakeip",
            engine: crate::VERSION,
            command,
            generated_at: chrono::Utc::now().to_rfc3339(),
            results,
            reports,
            recommendation,
        }
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::error::CoreError::Serde(e.to_string()))
    }
}

/// 生成单域名诊断的 Markdown 报告。
pub fn render_domain_markdown(d: &DomainDiagnosis, opts: &ReportOptions) -> String {
    let mut s = String::new();
    s.push_str(&format!("# NoFakeIP 诊断报告：{}\n\n", d.domain));
    s.push_str(&format!(
        "生成时间：{}　引擎版本：{}\n\n",
        chrono::Utc::now().to_rfc3339(),
        crate::VERSION
    ));

    // —— 分类 ——
    s.push_str("## 一、域名分类\n\n");
    s.push_str(&format!(
        "| 项 | 值 |\n| --- | --- |\n| 归属 | {} |\n| 置信度 | {:.2} |\n| 判定来源 | {} |\n| 命中规则 | {} |\n| 可注册域名 | {} |\n\n",
        d.classification.region.label(),
        d.classification.confidence,
        d.classification.source.label(),
        d.classification.matched_rule.clone().unwrap_or_else(|| "—".into()),
        d.classification.registrable.clone().unwrap_or_else(|| "—".into()),
    ));

    // —— 测速 ——
    s.push_str("## 二、DNS 测速结果\n\n");
    if opts.include_geo {
        s.push_str("| 服务器 | 协议 | 区域 | 平均延迟 | 最小 | 最大 | P95 | 连接 | TLS | 查询 | 状态 | 返回 IP |\n");
        s.push_str("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |\n");
    } else {
        s.push_str("| 服务器 | 协议 | 区域 | 平均延迟 | 最小 | 最大 | P95 | 状态 | 返回 IP |\n");
        s.push_str("| --- | --- | --- | ---: | ---: | ---: | ---: | --- | --- |\n");
    }
    for r in &d.bench.results {
        let ips = if r.answers.is_empty() {
            "—".to_string()
        } else {
            r.answers
                .iter()
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if opts.include_geo {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                r.server_name,
                r.protocol.label(),
                r.region.label(),
                r.latency_display(),
                fmt_ms(r.min_ms),
                fmt_ms(r.max_ms),
                fmt_ms(r.p95_ms),
                fmt_ms(r.connect_ms),
                fmt_ms(r.tls_ms),
                fmt_ms(r.query_ms),
                r.status_display(),
                ips,
            ));
        } else {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                r.server_name,
                r.protocol.label(),
                r.region.label(),
                r.latency_display(),
                fmt_ms(r.min_ms),
                fmt_ms(r.max_ms),
                fmt_ms(r.p95_ms),
                r.status_display(),
                ips,
            ));
        }
    }
    s.push('\n');

    // —— 污染检测 ——
    s.push_str("## 三、污染与 Fake-IP 检测\n\n");
    s.push_str(&format!(
        "- **污染风险**：{}\n- **Fake-IP 状态**：{}\n",
        d.report.risk.label(),
        d.report.fake_ip.label()
    ));
    if let Some(c) = d.report.consistency {
        s.push_str(&format!("- **一致性**：{:.0}%\n", c * 100.0));
    }
    if !d.report.poisoned_ips.is_empty() {
        s.push_str(&format!(
            "- **命中投毒 IP**：{}\n",
            d.report
                .poisoned_ips
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    s.push('\n');
    if d.report.findings.is_empty() {
        s.push_str("未发现异常。\n\n");
    } else {
        s.push_str("| 级别 | 检测项 | 说明 | 证据 |\n| --- | --- | --- | --- |\n");
        for f in &d.report.findings {
            s.push_str(&format!(
                "| {} | `{}` | {} | {} |\n",
                f.severity.label(),
                f.code,
                f.message,
                if f.evidence.is_empty() {
                    "—".to_string()
                } else {
                    f.evidence.join("；")
                }
            ));
        }
        s.push('\n');
    }

    // —— 推荐 ——
    s.push_str("## 四、推荐\n\n");
    for r in &d.recommendation.reasons {
        s.push_str(&format!("- {}\n", r));
    }
    s.push('\n');

    if opts.include_observations {
        s.push_str("## 五、原始观测\n\n```json\n");
        if let Ok(j) = serde_json::to_string_pretty(&d.report.observations) {
            s.push_str(&j);
        }
        s.push_str("\n```\n");
    }

    s
}

/// 生成批量扫描的汇总 Markdown 报告。
pub fn render_overview_markdown(
    results: &[DomainBenchResult],
    reports: &[PollutionReport],
    rec: &Recommendation,
) -> String {
    let mut s = String::new();
    s.push_str("# NoFakeIP 全量测速报告\n\n");
    s.push_str(&format!(
        "生成时间：{}　引擎版本：{}\n\n",
        chrono::Utc::now().to_rfc3339(),
        crate::VERSION
    ));

    s.push_str("## 推荐结论\n\n");
    s.push_str(&format!("- 国内 DNS：{}\n", fmt_rec(&rec.china)));
    s.push_str(&format!("- 国外 DNS：{}\n", fmt_rec(&rec.foreign)));
    s.push_str(&format!("- 污染率：{:.0}%\n\n", rec.pollution_rate * 100.0));

    s.push_str("### 推荐理由\n\n");
    for r in &rec.reasons {
        s.push_str(&format!("- {}\n", r));
    }
    s.push('\n');

    if !rec.alternates.is_empty() {
        s.push_str("### 备选\n\n");
        s.push_str("| 服务器 | 区域 | 平均延迟 | 评分 |\n| --- | --- | ---: | ---: |\n");
        for a in &rec.alternates {
            s.push_str(&format!(
                "| {} | {} | {:.0}ms | {:.0} |\n",
                a.name,
                a.region.label(),
                a.avg_ms,
                a.score
            ));
        }
        s.push('\n');
    }

    s.push_str("## 各域名结果\n\n");
    s.push_str("| 域名 | 分类 | 最快（同区域） | 风险 | Fake-IP | 成功服务器 |\n");
    s.push_str("| --- | --- | --- | --- | --- | ---: |\n");
    for r in results {
        let risk = reports
            .iter()
            .find(|p| p.domain == r.domain)
            .map(|p| p.risk.label())
            .unwrap_or("—");
        let fake = reports
            .iter()
            .find(|p| p.domain == r.domain)
            .map(|p| p.fake_ip.label())
            .unwrap_or("—");
        let best = match r.region {
            crate::models::Region::China => r.best_china.clone(),
            crate::models::Region::Global => r.best_global.clone(),
            crate::models::Region::Unknown => r.recommended.clone(),
        };
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {}/{} |\n",
            r.domain,
            r.region.label(),
            best.unwrap_or_else(|| "—".into()),
            risk,
            fake,
            r.ok_count(),
            r.results.len()
        ));
    }
    s.push('\n');

    let high: Vec<&PollutionReport> = reports
        .iter()
        .filter(|p| p.risk == RiskLevel::High)
        .collect();
    if !high.is_empty() {
        s.push_str("## 高风险域名明细\n\n");
        for p in high {
            s.push_str(&format!("### {}\n\n", p.domain));
            for f in &p.findings {
                s.push_str(&format!("- **{}**（{}）：{}\n", f.message, f.severity.label(), f.evidence.join("；")));
            }
            s.push('\n');
        }
    }

    s
}

fn fmt_ms(v: f64) -> String {
    if v <= 0.0 {
        "—".to_string()
    } else {
        format!("{:.0}ms", v)
    }
}

fn fmt_rec(s: &Option<crate::recommend::RecommendedServer>) -> String {
    match s {
        Some(s) => format!("{}（{:.0}ms，成功率 {:.0}%）", s.name, s.avg_ms, s.success_rate * 100.0),
        None => "无可用".into(),
    }
}

/// 导出 CSV（委托给 benchmark 模块，保证列顺序一致）。
pub fn export_csv(results: &[DomainBenchResult]) -> Result<String> {
    benchmark::export_csv(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::ServerBenchResult;
    use crate::classifier::{Classification, RuleSource};
    use crate::models::{DnsProtocol, Region};
    use crate::pollution::{FakeIpStatus, ServerObservation};
    use crate::recommend::RecommendedServer;

    fn sample_diagnosis() -> DomainDiagnosis {
        let server = ServerBenchResult {
            server_id: 1,
            server_name: "Cloudflare".into(),
            protocol: DnsProtocol::Doh,
            region: Region::Global,
            address: "https://cloudflare-dns.com/dns-query".into(),
            ok: true,
            error: None,
            samples: 3,
            min_ms: 110.0,
            avg_ms: 120.0,
            max_ms: 130.0,
            p95_ms: 130.0,
            connect_ms: 20.0,
            tls_ms: 40.0,
            query_ms: 60.0,
            answers: vec!["142.250.72.4".parse().unwrap()],
            ttl: Some(300),
            rcode: Some(0),
            score: 85.0,
            geo: vec![],
        };
        let bench = DomainBenchResult {
            domain: "github.com".into(),
            region: Region::Global,
            confidence: 0.95,
            results: vec![server],
            best_china: None,
            best_global: Some("Cloudflare".into()),
            recommended: Some("Cloudflare".into()),
            batch_id: None,
            created_at: "2026-10-14T00:00:00Z".into(),
        };
        let report = PollutionReport {
            domain: "github.com".into(),
            region: Region::Global,
            risk: RiskLevel::Low,
            fake_ip: FakeIpStatus::Normal,
            consistency: Some(0.95),
            fake_ip_hits: vec![],
            poisoned_ips: vec![],
            findings: vec![],
            observations: vec![ServerObservation {
                server_name: "Cloudflare".into(),
                region: Region::Global,
                ok: true,
                ips: vec!["142.250.72.4".parse().unwrap()],
                ttl: Some(300),
                rcode: Some(0),
                error: None,
                is_fake_ip: false,
            }],
        };
        let recommendation = Recommendation {
            china: None,
            foreign: Some(RecommendedServer {
                name: "Cloudflare".into(),
                address: "https://cloudflare-dns.com/dns-query".into(),
                protocol: DnsProtocol::Doh,
                region: Region::Global,
                avg_ms: 120.0,
                score: 85.0,
                success_rate: 1.0,
                samples: 3,
            }),
            reasons: vec!["国外平均延迟最低：Cloudflare 120ms".into()],
            china_avg_ms: 0.0,
            foreign_avg_ms: 120.0,
            pollution_rate: 0.0,
            generated_at: "2026-10-14T00:00:00Z".into(),
            alternates: vec![],
        };
        DomainDiagnosis {
            domain: "github.com".into(),
            classification: Classification {
                domain: "github.com".into(),
                region: Region::Global,
                confidence: 0.95,
                source: RuleSource::LocalRule,
                matched_rule: Some("github.com".into()),
                registrable: Some("github.com".into()),
                tld: Some("com".into()),
            },
            bench,
            report,
            recommendation,
        }
    }

    #[test]
    fn domain_markdown_contains_key_sections() {
        let d = sample_diagnosis();
        let md = render_domain_markdown(&d, &ReportOptions::default());
        assert!(md.contains("NoFakeIP 诊断报告：github.com"));
        assert!(md.contains("域名分类"));
        assert!(md.contains("DNS 测速结果"));
        assert!(md.contains("污染与 Fake-IP 检测"));
        assert!(md.contains("推荐"));
        assert!(md.contains("Cloudflare"));
        assert!(md.contains("原始观测"));
    }

    #[test]
    fn domain_markdown_can_omit_observations() {
        let d = sample_diagnosis();
        let md = render_domain_markdown(
            &d,
            &ReportOptions {
                include_observations: false,
                include_geo: false,
            },
        );
        assert!(!md.contains("原始观测"));
        assert!(!md.contains("| 连接 |"));
    }

    #[test]
    fn overview_markdown_lists_results() {
        let d = sample_diagnosis();
        let md = render_overview_markdown(&[d.bench.clone()], &[d.report.clone()], &d.recommendation);
        assert!(md.contains("全量测速报告"));
        assert!(md.contains("推荐结论"));
        assert!(md.contains("github.com"));
    }

    #[test]
    fn json_envelope_is_valid() {
        let d = sample_diagnosis();
        let results = [d.bench.clone()];
        let reports = [d.report.clone()];
        let env = JsonEnvelope::new("scan", &results, &reports, &d.recommendation);
        let j = env.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["command"], "scan");
        assert_eq!(v["tool"], "nofakeip");
        assert!(v["results"].is_array());
        assert!(v["reports"].is_array());
        assert!(v["recommendation"].is_object());
    }

    #[test]
    fn no_prohibited_output_anywhere() {
        let d = sample_diagnosis();
        let md = render_domain_markdown(&d, &ReportOptions::default());
        let lower = md.to_ascii_lowercase();
        for banned in ["clash", "openclash", "fallback-filter", "enhanced-mode", "fake-ip-range:"] {
            assert!(!lower.contains(banned), "报告不应包含 Clash 相关内容: {}", banned);
        }
    }
}
