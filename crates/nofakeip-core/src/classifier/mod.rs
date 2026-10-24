//! 国内外域名自动识别。
//!
//! 判定优先级与置信度算法见 `doc/04-域名分类规格.md`：
//! `用户覆盖(1.00) → 本地规则库(0.95) → TLD 弱规则(0.75) → IP 归属地(0.60~0.85) → 默认(0.30)`。
//!
//! 冲突消解：最长规则优先，同长度时国内优先（把国内站点误判成国外会走 fallback 导致慢）。

use crate::data::BuiltinData;
use crate::error::Result;
use crate::geo::GeoProvider;
use crate::models::Region;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// 判定来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    /// 用户自定义覆盖
    UserOverride,
    /// 内置 / 外部规则库
    LocalRule,
    /// TLD 弱规则
    SuffixRule,
    /// 依据解析到的 IP 归属地
    IpGeo,
    /// 无任何依据
    Default,
}

impl RuleSource {
    pub fn label(&self) -> &'static str {
        match self {
            RuleSource::UserOverride => "用户覆盖",
            RuleSource::LocalRule => "规则库",
            RuleSource::SuffixRule => "TLD 规则",
            RuleSource::IpGeo => "IP 归属地",
            RuleSource::Default => "默认",
        }
    }
}

/// 分类结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    pub domain: String,
    pub region: Region,
    pub confidence: f32,
    pub source: RuleSource,
    #[serde(default)]
    pub matched_rule: Option<String>,
    #[serde(default)]
    pub registrable: Option<String>,
    #[serde(default)]
    pub tld: Option<String>,
}

/// 域名分类器。
pub struct DomainClassifier {
    /// 后缀规则：key = 域名后缀，value = 区域
    rules: HashMap<String, Region>,
    /// 通配规则：`*.example.com`（不匹配裸域名）
    wildcards: HashMap<String, Region>,
    /// 用户覆盖规则（精确 / 后缀匹配，优先级最高）
    user: HashMap<String, Region>,
    /// 用户通配规则：`*.example.com`（不匹配裸域名本身）
    user_wildcards: HashMap<String, Region>,
    /// 国外 TLD 弱规则
    foreign_tlds: HashSet<String>,
    /// 统计信息
    pub china_rule_count: usize,
    pub foreign_rule_count: usize,
}

impl Default for DomainClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl DomainClassifier {
    /// 使用内置数据构建（约 11 万国内 + 2 万国外规则）。
    pub fn new() -> Self {
        Self::from_texts(
            BuiltinData::china_domains(),
            BuiltinData::foreign_domains(),
            BuiltinData::foreign_tlds(),
        )
    }

    /// 优先加载用户数据目录中的同名文件（支持热更新后的数据）。
    pub fn with_data_dir(dir: Option<&Path>) -> Self {
        let cn = BuiltinData::load_or_embedded(dir, "china_domains.txt")
            .unwrap_or_else(|_| BuiltinData::china_domains().to_string());
        let fg = BuiltinData::load_or_embedded(dir, "foreign_domains.txt")
            .unwrap_or_else(|_| BuiltinData::foreign_domains().to_string());
        let tld = BuiltinData::load_or_embedded(dir, "foreign_tlds.txt")
            .unwrap_or_else(|_| BuiltinData::foreign_tlds().to_string());
        Self::from_texts(&cn, &fg, &tld)
    }

    /// 从三段文本构建规则表。
    pub fn from_texts(china: &str, foreign: &str, foreign_tlds: &str) -> Self {
        let mut rules: HashMap<String, Region> = HashMap::new();
        let mut wildcards: HashMap<String, Region> = HashMap::new();

        let mut cn_count = 0usize;
        for line in BuiltinData::parse_lines(china) {
            let (key, wild) = normalize_rule(&line);
            if key.is_empty() {
                continue;
            }
            if wild {
                wildcards.entry(key).or_insert(Region::China);
            } else if rules.insert(key, Region::China).is_none() {
                cn_count += 1;
            }
        }

        let mut fg_count = 0usize;
        for line in BuiltinData::parse_lines(foreign) {
            let (key, wild) = normalize_rule(&line);
            if key.is_empty() {
                continue;
            }
            if wild {
                // 通配规则同样「国内优先」
                wildcards.entry(key).or_insert(Region::Global);
            } else if !rules.contains_key(&key) {
                rules.insert(key, Region::Global);
                fg_count += 1;
            }
        }

        let foreign_tlds: HashSet<String> = BuiltinData::parse_lines(foreign_tlds)
            .into_iter()
            .map(|t| t.trim_start_matches('.').to_ascii_lowercase())
            .filter(|t| !t.is_empty() && !t.contains('.'))
            .collect();

        Self {
            rules,
            wildcards,
            user: HashMap::new(),
            user_wildcards: HashMap::new(),
            foreign_tlds,
            china_rule_count: cn_count,
            foreign_rule_count: fg_count,
        }
    }

    /// 添加用户覆盖规则（优先级最高）。
    ///
    /// `pattern` 支持 `example.com`（含子域）、`*.example.com`（仅子域）、`.cn`（TLD 后缀）。
    pub fn add_user_override(&mut self, pattern: &str, region: Region) {
        let (key, wild) = normalize_rule(pattern);
        if key.is_empty() {
            return;
        }
        if wild {
            self.user_wildcards.insert(key, region);
        } else {
            self.user.insert(key, region);
        }
    }

    pub fn user_rule_count(&self) -> usize {
        self.user.len() + self.user_wildcards.len()
    }

    /// 清空用户规则。
    pub fn clear_user_overrides(&mut self) {
        self.user.clear();
        self.user_wildcards.clear();
    }

    /// 域名归一化：小写 + 去尾点。
    fn normalize(domain: &str) -> String {
        domain.trim().trim_end_matches('.').to_ascii_lowercase()
    }

    /// 基础分类（不含 IP 判定）。
    pub fn classify(&self, domain: &str) -> Classification {
        let d = Self::normalize(domain);
        let registrable = registrable_domain(&d);
        let tld = d.rsplit('.').next().map(|s| s.to_string());

        let base = Classification {
            domain: d.clone(),
            region: Region::Unknown,
            confidence: 0.30,
            source: RuleSource::Default,
            matched_rule: None,
            registrable,
            tld,
        };

        if d.is_empty() || !d.contains('.') {
            return base;
        }

        // ① 用户覆盖（精确 → 后缀 → 通配）
        if let Some((rule, region)) = self.match_user(&d) {
            return Classification {
                region,
                confidence: 1.0,
                source: RuleSource::UserOverride,
                matched_rule: Some(rule),
                ..base
            };
        }

        // ② 本地规则库：从最长后缀开始逐级截断，命中即返回（= 最长规则优先）
        let labels: Vec<&str> = d.split('.').collect();
        for i in 0..labels.len() {
            let suffix = labels[i..].join(".");
            if suffix.is_empty() {
                continue;
            }
            if let Some(region) = self.rules.get(&suffix) {
                return Classification {
                    region: *region,
                    confidence: 0.95,
                    source: RuleSource::LocalRule,
                    matched_rule: Some(suffix),
                    ..base
                };
            }
        }

        // ③ 通配规则：`*.example.com` 只匹配子域
        for i in 1..labels.len() {
            let suffix = labels[i..].join(".");
            if let Some(region) = self.wildcards.get(&suffix) {
                return Classification {
                    region: *region,
                    confidence: 0.90,
                    source: RuleSource::LocalRule,
                    matched_rule: Some(format!("*.{}", suffix)),
                    ..base
                };
            }
        }

        // ④ TLD 弱规则（不含 com/net/org，见 data/foreign_tlds.txt 说明）
        if let Some(tld) = d.rsplit('.').next() {
            if tld == "cn" {
                return Classification {
                    region: Region::China,
                    confidence: 0.80,
                    source: RuleSource::SuffixRule,
                    matched_rule: Some(".cn".into()),
                    ..base
                };
            }
            if self.foreign_tlds.contains(tld) {
                return Classification {
                    region: Region::Global,
                    confidence: 0.75,
                    source: RuleSource::SuffixRule,
                    matched_rule: Some(format!(".{}", tld)),
                    ..base
                };
            }
        }

        base
    }

    /// 带 IP 的二次判定（规则不确定时用 IP 归属地提升置信度）。
    pub fn classify_with_ips(
        &self,
        domain: &str,
        ips: &[IpAddr],
        geo: &dyn GeoProvider,
    ) -> Classification {
        let mut c = self.classify(domain);
        // 规则库命中（≥0.90）时不覆盖，避免 CDN 双栈导致的误判
        if c.confidence >= 0.90 {
            return c;
        }
        if ips.is_empty() {
            return c;
        }

        let mut valid = 0usize;
        let mut cn = 0usize;
        for ip in ips {
            if let Some(g) = geo.lookup(*ip) {
                if g.is_private || g.is_reserved {
                    continue;
                }
                valid += 1;
                if g.is_china {
                    cn += 1;
                }
            }
        }

        if valid == 0 {
            // 全部为私网 / 保留地址：典型场景是本机 Fake-IP，无法据此判定
            c.confidence = 0.20;
            return c;
        }

        let ratio = cn as f32 / valid as f32;
        if ratio >= 0.8 {
            c.region = Region::China;
            c.confidence = 0.85;
            c.source = RuleSource::IpGeo;
            c.matched_rule = Some(format!("{}/{} 个 IP 属于中国大陆", cn, valid));
        } else if ratio <= 0.2 {
            c.region = Region::Global;
            c.confidence = 0.85;
            c.source = RuleSource::IpGeo;
            c.matched_rule = Some(format!("{}/{} 个 IP 属于中国大陆", cn, valid));
        } else {
            // 混合归属：保持原判定，降低置信度
            c.confidence = 0.50;
            c.matched_rule = Some(format!("IP 归属混合（{}/{} 属于中国大陆）", cn, valid));
        }
        c
    }

    fn match_user(&self, domain: &str) -> Option<(String, Region)> {
        if self.user.is_empty() && self.user_wildcards.is_empty() {
            return None;
        }
        let labels: Vec<&str> = domain.split('.').collect();
        // 精确 / 后缀规则：命中自身与所有子域
        for i in 0..labels.len() {
            let suffix = labels[i..].join(".");
            if let Some(region) = self.user.get(&suffix) {
                return Some((suffix, *region));
            }
        }
        // 通配规则：只匹配严格子域（i 从 1 开始，跳过裸域名）
        for i in 1..labels.len() {
            let suffix = labels[i..].join(".");
            if let Some(region) = self.user_wildcards.get(&suffix) {
                return Some((format!("*.{}", suffix), *region));
            }
        }
        None
    }
}

/// 把规则行拆成 (key, is_wildcard)。
fn normalize_rule(line: &str) -> (String, bool) {
    let l = line.trim().to_ascii_lowercase();
    if l.is_empty() || l.starts_with('#') {
        return (String::new(), false);
    }
    if let Some(rest) = l.strip_prefix("*.") {
        return (rest.trim_end_matches('.').to_string(), true);
    }
    if let Some(rest) = l.strip_prefix('.') {
        // `.cn` 这类 TLD 规则：作为后缀规则处理
        return (rest.trim_end_matches('.').to_string(), false);
    }
    (l.trim_end_matches('.').to_string(), false)
}

/// 取可注册域名（PSL），失败时退化为最后两段。
pub fn registrable_domain(domain: &str) -> Option<String> {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if d.is_empty() || !d.contains('.') {
        return None;
    }
    if let Some(root) = psl::domain_str(&d) {
        return Some(root.to_string());
    }
    let parts: Vec<&str> = d.split('.').collect();
    if parts.len() >= 2 {
        Some(format!(
            "{}.{}",
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        ))
    } else {
        None
    }
}

/// 全局默认分类器（进程内只构建一次，避免重复解析 13 万条规则）。
pub fn default_classifier() -> Arc<DomainClassifier> {
    static INSTANCE: OnceLock<Arc<DomainClassifier>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(DomainClassifier::new())).clone()
}

/// 便捷函数：用全局分类器分类。
pub fn classify(domain: &str) -> Classification {
    default_classifier().classify(domain)
}

/// 加载用户自定义规则到分类器。
pub fn apply_user_rules(
    classifier: &mut DomainClassifier,
    rules: &[(String, Region)],
) -> Result<()> {
    for (pattern, region) in rules {
        classifier.add_user_override(pattern, *region);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::LocalGeo;

    fn clf() -> DomainClassifier {
        DomainClassifier::new()
    }

    #[test]
    fn rule_counts_are_reasonable() {
        let c = clf();
        assert!(c.china_rule_count > 50_000, "国内规则数 {}", c.china_rule_count);
        assert!(c.foreign_rule_count > 5_000, "国外规则数 {}", c.foreign_rule_count);
    }

    #[test]
    fn classify_china_domain() {
        let c = clf();
        let r = c.classify("www.taobao.com");
        assert_eq!(r.region, Region::China);
        assert_eq!(r.source, RuleSource::LocalRule);
        assert_eq!(r.matched_rule.as_deref(), Some("taobao.com"));
        assert!(r.confidence >= 0.95);
        assert_eq!(r.registrable.as_deref(), Some("taobao.com"));
        assert_eq!(r.tld.as_deref(), Some("com"));
    }

    #[test]
    fn classify_global_domain() {
        let c = clf();
        let r = c.classify("github.com");
        assert_eq!(r.region, Region::Global);
        assert_eq!(r.source, RuleSource::LocalRule);
        assert_eq!(r.matched_rule.as_deref(), Some("github.com"));
        assert!((r.confidence - 0.95).abs() < 1e-6);
    }

    #[test]
    fn classify_tld_rule() {
        let c = clf();
        let r = c.classify("some-brand-new-site.xyz");
        assert_eq!(r.region, Region::Global);
        assert_eq!(r.source, RuleSource::SuffixRule);
        assert_eq!(r.matched_rule.as_deref(), Some(".xyz"));
    }

    #[test]
    fn classify_cn_tld() {
        let c = clf();
        let r = c.classify("unknown-random-site.cn");
        assert_eq!(r.region, Region::China);
        assert_eq!(r.source, RuleSource::SuffixRule);
    }

    #[test]
    fn classify_unknown() {
        let c = clf();
        let r = c.classify("zzz-not-exist-anywhere.com");
        assert_eq!(r.region, Region::Unknown);
        assert_eq!(r.source, RuleSource::Default);
        assert!((r.confidence - 0.30).abs() < 1e-6);
    }

    #[test]
    fn user_override_beats_rules() {
        let mut c = clf();
        c.add_user_override("taobao.com", Region::Global);
        let r = c.classify("www.taobao.com");
        assert_eq!(r.region, Region::Global);
        assert_eq!(r.source, RuleSource::UserOverride);
        assert!((r.confidence - 1.0).abs() < 1e-6);
        assert_eq!(c.user_rule_count(), 1);
    }

    #[test]
    fn user_override_supports_wildcard() {
        let mut c = clf();
        c.add_user_override("*.internal.example.com", Region::China);
        assert_eq!(
            c.classify("a.internal.example.com").region,
            Region::China
        );
        // 裸域名不命中通配（退化为规则库/TLD 判定）
        assert_ne!(
            c.classify("internal.example.com").source,
            RuleSource::UserOverride
        );
    }

    #[test]
    fn china_priority_on_conflict() {
        // 同一后缀同时出现在两个列表时，国内优先
        let c = DomainClassifier::from_texts("conflict.com\n", "conflict.com\nother.com\n", "");
        assert_eq!(c.classify("conflict.com").region, Region::China);
        assert_eq!(c.classify("other.com").region, Region::Global);
    }

    #[test]
    fn longest_rule_wins() {
        let c = DomainClassifier::from_texts("example.com\n", "sub.example.com\n", "");
        assert_eq!(c.classify("sub.example.com").region, Region::Global);
        assert_eq!(c.classify("other.example.com").region, Region::China);
    }

    #[test]
    fn classify_with_ips_uses_geo() {
        let c = DomainClassifier::from_texts("", "", "");
        let geo = LocalGeo::new();
        // 阿里 DNS 的 IP 属于国内
        let r = c.classify_with_ips(
            "mystery-domain.com",
            &["223.5.5.5".parse().unwrap()],
            &geo,
        );
        assert_eq!(r.region, Region::China);
        assert_eq!(r.source, RuleSource::IpGeo);
        assert!((r.confidence - 0.85).abs() < 1e-6);

        let r2 = c.classify_with_ips("mystery-domain.com", &["1.1.1.1".parse().unwrap()], &geo);
        assert_eq!(r2.region, Region::Global);
    }

    #[test]
    fn classify_with_ips_ignores_private() {
        let c = DomainClassifier::from_texts("", "", "");
        let geo = LocalGeo::new();
        let r = c.classify_with_ips("x.com", &["192.168.1.5".parse().unwrap()], &geo);
        assert_eq!(r.region, Region::Unknown);
        assert!((r.confidence - 0.20).abs() < 1e-6);
    }

    #[test]
    fn strong_rule_not_overridden_by_geo() {
        let c = DomainClassifier::new();
        let geo = LocalGeo::new();
        // github.com 规则命中 0.95，不应被国内 CDN 的 IP 覆盖
        let r = c.classify_with_ips("github.com", &["223.5.5.5".parse().unwrap()], &geo);
        assert_eq!(r.region, Region::Global);
        assert_eq!(r.source, RuleSource::LocalRule);
    }

    #[test]
    fn registrable_helper() {
        assert_eq!(
            registrable_domain("www.news.baidu.com").as_deref(),
            Some("baidu.com")
        );
        assert_eq!(registrable_domain("localhost"), None);
    }

    #[test]
    fn normalize_rule_variants() {
        assert_eq!(normalize_rule("Example.COM"), ("example.com".into(), false));
        assert_eq!(normalize_rule("*.x.com"), ("x.com".into(), true));
        assert_eq!(normalize_rule(".cn"), ("cn".into(), false));
        assert_eq!(normalize_rule("# comment"), (String::new(), false));
    }
}
