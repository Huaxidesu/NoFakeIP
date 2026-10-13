//! 单域名诊断编排：把「分类 → 测速 → 污染检测 → 推荐」串成一次调用。
//!
//! 这是 CLI / GUI 最常用的入口。之所以单独成模块，是为了让上层（CLI、Tauri command）
//! 只依赖一个稳定的门面，而不是自己拼装四个模块。

use crate::benchmark::{BenchmarkEngine, BenchmarkOptions, DomainBenchResult};
use crate::classifier::{Classification, DomainClassifier};
use crate::dns::DnsClient;
use crate::error::Result;
use crate::models::DnsServer;
use crate::pollution::{PollutionDetector, PollutionReport};
use crate::recommend::{Recommendation, Recommender};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 一次完整的域名诊断结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainDiagnosis {
    pub domain: String,
    pub classification: Classification,
    pub bench: DomainBenchResult,
    pub report: PollutionReport,
    pub recommendation: Recommendation,
}

/// 诊断器。
pub struct Diagnoser {
    engine: BenchmarkEngine,
    detector: Arc<PollutionDetector>,
    classifier: Arc<DomainClassifier>,
}

impl Diagnoser {
    /// 使用默认配置构造（内置规则、默认 Fake-IP 段、离线归属地）。
    pub fn new() -> Self {
        Self {
            engine: BenchmarkEngine::create(),
            detector: crate::pollution::default_detector(),
            classifier: crate::classifier::default_classifier(),
        }
    }

    /// 从用户数据目录构造（会优先加载用户更新过的规则数据）。
    pub fn with_data_dir(dir: Option<&std::path::Path>) -> Self {
        Self {
            engine: BenchmarkEngine::create(),
            detector: Arc::new(PollutionDetector::with_data_dir(dir)),
            classifier: Arc::new(DomainClassifier::with_data_dir(dir)),
        }
    }

    /// 替换检测器（用于自定义 Fake-IP 段 / 启用在线归属地）。
    pub fn with_detector(mut self, detector: PollutionDetector) -> Self {
        self.detector = Arc::new(detector);
        self
    }

    /// 替换分类器（用于注入用户自定义规则）。
    pub fn with_classifier(mut self, classifier: DomainClassifier) -> Self {
        self.classifier = Arc::new(classifier);
        self
    }

    pub fn detector(&self) -> &Arc<PollutionDetector> {
        &self.detector
    }

    pub fn classifier(&self) -> &Arc<DomainClassifier> {
        &self.classifier
    }

    pub fn client(&self) -> Arc<DnsClient> {
        self.engine.client()
    }

    /// 诊断单个域名。
    pub async fn diagnose(
        &self,
        domain: &str,
        servers: &[DnsServer],
        opts: &BenchmarkOptions,
    ) -> Result<DomainDiagnosis> {
        let bench = self.engine.bench_domain(domain, servers, opts).await?;

        // 用解析到的 IP 提升分类置信度（规则库命中时不会覆盖）
        let geo = crate::geo::LocalGeo::new();
        let classification = self
            .classifier
            .classify_with_ips(domain, &bench.all_ips(), &geo);

        let report = self
            .detector
            .analyze(domain, classification.region, &bench.observations());
        let recommendation = Recommender::for_domain(domain, classification.region, &bench);

        Ok(DomainDiagnosis {
            domain: domain.to_string(),
            classification,
            bench,
            report,
            recommendation,
        })
    }

    /// 依次诊断多个域名（串行，避免把上游打爆）。
    pub async fn diagnose_many(
        &self,
        domains: &[String],
        servers: &[DnsServer],
        opts: &BenchmarkOptions,
    ) -> Result<Vec<DomainDiagnosis>> {
        let mut out = Vec::with_capacity(domains.len());
        for d in domains {
            match self.diagnose(d, servers, opts).await {
                Ok(r) => out.push(r),
                Err(e) => tracing::warn!("诊断 {} 失败: {}", d, e),
            }
        }
        Ok(out)
    }

    /// 批量测速指定域名（不做污染检测，供 `bench` 使用）。
    pub async fn bench_batch(
        &self,
        domains: &[String],
        servers: &[DnsServer],
        opts: &BenchmarkOptions,
        progress: Option<crate::benchmark::ProgressFn>,
    ) -> Result<Vec<DomainBenchResult>> {
        self.engine.bench_batch(domains, servers, opts, progress).await
    }

    /// 全量测速 + 汇总推荐（不做逐域名污染检测，避免耗时过长）。
    pub async fn full_scan(
        &self,
        servers: &[DnsServer],
        opts: &BenchmarkOptions,
        progress: Option<crate::benchmark::ProgressFn>,
    ) -> Result<(Vec<DomainBenchResult>, Recommendation)> {
        let results = self.engine.bench_full(servers, opts, progress).await?;
        let reports: Vec<PollutionReport> = results
            .iter()
            .map(|r| {
                self.detector
                    .analyze(&r.domain, r.region, &r.observations())
            })
            .collect();
        let rec = Recommender::from_benchmark(&results, &reports);
        Ok((results, rec))
    }

    /// 取全量扫描过程中产生的污染报告（供 GUI 展示）。
    pub fn scan_reports(&self, results: &[DomainBenchResult]) -> Vec<PollutionReport> {
        results
            .iter()
            .map(|r| self.detector.analyze(&r.domain, r.region, &r.observations()))
            .collect()
    }
}

impl Default for Diagnoser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnoser_constructs_with_defaults() {
        let d = Diagnoser::new();
        assert!(!d.detector().online_geo_enabled());
        assert!(d.classifier().china_rule_count > 1000);
    }

    #[test]
    fn custom_fake_ip_range_applies() {
        let det = PollutionDetector::new().with_fake_ip_range("10.0.0.0/8").unwrap();
        let d = Diagnoser::new().with_detector(det);
        assert_eq!(d.detector().fake_ip_range(), "10.0.0.0/8");
    }
}
