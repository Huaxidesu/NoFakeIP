//! 规则数据（域名 / IP 网段）的来源定义与在线更新。
//!
//! 设计目标：
//! 1. 内置一份可用数据（编译期嵌入，离线可用，见 [`crate::data`]）；
//! 2. 支持用户随时从上游拉取最新数据，落到用户数据目录，运行时优先加载用户数据；
//! 3. 每个数据源都配置多个候选地址（官方 + 国内可达镜像），逐个尝试，避免单一源不可达；
//! 4. 更新采用「先写临时文件再原子改名」，中断不会损坏已有数据。
//!
//! 范围说明：本模块只维护 NoFakeIP 自己使用的数据（国内域名表、中国 IP 网段表、
//! 国外域名表）。不涉及任何代理软件的数据文件或配置生成。

use crate::data::BuiltinData;
use crate::error::{CoreError, Result};
use crate::net::HttpClient;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 数据种类（决定解析方式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    /// 纯文本域名列表（一行一个）
    DomainList,
    /// CIDR 网段列表（一行一个）
    CidrList,
    /// dnsmasq 风格配置（`server=/domain/dns`）
    DnsmasqConf,
    /// CSV 榜单（取第 3 列域名）
    CsvDomainRank,
}

/// 一个可更新的数据源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSource {
    /// 稳定标识（CLI / GUI 用）
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: DataKind,
    /// 落地的文件名（写入用户数据目录）
    pub target_file: String,
    /// 候选下载地址，按顺序尝试
    pub candidates: Vec<String>,
    /// 上游主页（展示用）
    pub homepage: String,
    /// 是否建议默认更新（大文件默认不勾选）
    pub default_enabled: bool,
}

/// 数据源清单。
///
/// 每个源都带国内可达镜像（`ghproxy.net` / `jsDelivr`）。
/// 若全部候选失败，返回错误并保留本地已有文件（不会清空）。
pub fn sources() -> Vec<DataSource> {
    const RAW_CHINA_DOMAINS: &str =
        "https://raw.githubusercontent.com/felixonmars/dnsmasq-china-list/master/accelerated-domains.china.conf";
    const RAW_CHINA_IP: &str =
        "https://raw.githubusercontent.com/17mon/china_ip_list/master/china_ip_list.txt";
    const RAW_MAJESTIC: &str = "https://downloads.majestic.com/majestic_million.csv";

    let gh = |u: &str| format!("https://ghproxy.net/{}", u);
    let gh2 = |u: &str| format!("https://gh-proxy.com/{}", u);
    let jsd = |owner: &str, repo: &str, branch: &str, path: &str| {
        format!(
            "https://testingcf.jsdelivr.net/gh/{}/{}@{}/{}",
            owner, repo, branch, path
        )
    };

    vec![
        DataSource {
            id: "china_domains".into(),
            name: "国内域名列表".into(),
            description: "来源 dnsmasq-china-list，用于域名国内外自动识别（离线判定）。".into(),
            kind: DataKind::DnsmasqConf,
            target_file: "china_domains.txt".into(),
            candidates: vec![
                RAW_CHINA_DOMAINS.to_string(),
                gh(RAW_CHINA_DOMAINS),
                gh2(RAW_CHINA_DOMAINS),
                jsd(
                    "felixonmars",
                    "dnsmasq-china-list",
                    "master",
                    "accelerated-domains.china.conf",
                ),
            ],
            homepage: "https://github.com/felixonmars/dnsmasq-china-list".into(),
            default_enabled: true,
        },
        DataSource {
            id: "china_ip_cidr".into(),
            name: "中国 IP 网段".into(),
            description: "来源 china_ip_list，用于离线判断 IP 是否属于中国大陆。".into(),
            kind: DataKind::CidrList,
            target_file: "china_ip_cidr.txt".into(),
            candidates: vec![
                RAW_CHINA_IP.to_string(),
                gh(RAW_CHINA_IP),
                gh2(RAW_CHINA_IP),
                jsd("17mon", "china_ip_list", "master", "china_ip_list.txt"),
            ],
            homepage: "https://github.com/17mon/china_ip_list".into(),
            default_enabled: true,
        },
        DataSource {
            id: "foreign_domains".into(),
            name: "国外域名列表".into(),
            description:
                "来源 Majestic Million 榜单（剔除国内域名后取头部 2 万条）。上游文件较大（约 80MB），默认不勾选。"
                    .into(),
            kind: DataKind::CsvDomainRank,
            target_file: "foreign_domains.txt".into(),
            candidates: vec![RAW_MAJESTIC.to_string()],
            homepage: "https://majestic.com/reports/majestic-million".into(),
            default_enabled: false,
        },
    ]
}

/// 按 id 查数据源。
pub fn source_by_id(id: &str) -> Option<DataSource> {
    sources().into_iter().find(|s| s.id == id)
}

/// 本地数据文件状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataStatus {
    pub id: String,
    pub name: String,
    pub target_file: String,
    pub exists: bool,
    pub size_bytes: u64,
    pub entries: usize,
    #[serde(default)]
    pub modified_at: Option<String>,
    /// 是否正在使用内置数据（尚未下载过用户副本）
    pub using_builtin: bool,
}

/// 单次更新结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateReport {
    pub id: String,
    pub name: String,
    pub ok: bool,
    /// 实际成功的地址
    #[serde(default)]
    pub url_used: Option<String>,
    /// 尝试过的地址数量
    pub attempts: usize,
    pub bytes: usize,
    pub entries: usize,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub duration_ms: f64,
}

/// 更新进度回调：(已完成, 总数, 当前项名称)。
pub type ProgressFn = std::sync::Arc<dyn Fn(usize, usize, &str) + Send + Sync>;

/// 数据更新器。
pub struct DataUpdater {
    http: HttpClient,
    data_dir: PathBuf,
}

impl DataUpdater {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            http: HttpClient::new(),
            data_dir: data_dir.into(),
        }
    }

    pub fn with_timeout(mut self, d: std::time::Duration) -> Self {
        self.http = self.http.with_timeout(d);
        self
    }

    /// 设置数据下载使用的出站代理（None = 直连）。
    pub fn with_proxy(mut self, proxy: Option<crate::net::ProxyUrl>) -> Self {
        self.http = self.http.with_proxy(proxy);
        self
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn sources(&self) -> Vec<DataSource> {
        sources()
    }

    /// 查看各数据源本地状态。
    pub fn status(&self) -> Vec<DataStatus> {
        sources()
            .into_iter()
            .map(|s| {
                let path = self.data_dir.join(&s.target_file);
                let (exists, size, modified, entries) = match std::fs::metadata(&path) {
                    Ok(md) => {
                        let entries = std::fs::read_to_string(&path)
                            .map(|t| count_entries(&t))
                            .unwrap_or(0);
                        let modified = md
                            .modified()
                            .ok()
                            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
                        (true, md.len(), modified, entries)
                    }
                    Err(_) => {
                        // 未下载过用户副本：条目数取内置数据，避免显示为 0 造成误解
                        let entries = BuiltinData::embedded(&s.target_file)
                            .map(count_entries)
                            .unwrap_or(0);
                        (false, 0, None, entries)
                    }
                };
                DataStatus {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    target_file: s.target_file.clone(),
                    exists,
                    size_bytes: size,
                    entries,
                    modified_at: modified,
                    using_builtin: !exists,
                }
            })
            .collect()
    }

    /// 更新单个数据源（多候选地址依次尝试）。
    pub async fn update(&self, id: &str) -> Result<UpdateReport> {
        let src = source_by_id(id).ok_or_else(|| CoreError::NotFound(format!("未知数据源: {}", id)))?;
        self.update_source(&src).await
    }

    /// 更新多个数据源。`ids` 为空时更新全部「默认启用」的源。
    pub async fn update_many(
        &self,
        ids: &[String],
        progress: Option<ProgressFn>,
    ) -> Vec<UpdateReport> {
        let all = sources();
        let list: Vec<DataSource> = if ids.is_empty() {
            all.into_iter().filter(|s| s.default_enabled).collect()
        } else {
            all.into_iter()
                .filter(|s| ids.iter().any(|i| i == &s.id))
                .collect()
        };

        let mut reports = Vec::with_capacity(list.len());
        let total = list.len();
        for (i, src) in list.iter().enumerate() {
            if let Some(cb) = &progress {
                cb(i, total, &src.name);
            }
            let rep = match self.update_source(src).await {
                Ok(r) => r,
                Err(e) => UpdateReport {
                    id: src.id.clone(),
                    name: src.name.clone(),
                    ok: false,
                    url_used: None,
                    attempts: 0,
                    bytes: 0,
                    entries: 0,
                    target: None,
                    error: Some(e.to_string()),
                    duration_ms: 0.0,
                },
            };
            reports.push(rep);
        }
        if let Some(cb) = &progress {
            cb(total, total, "完成");
        }
        reports
    }

    async fn update_source(&self, src: &DataSource) -> Result<UpdateReport> {
        let started = Instant::now();
        std::fs::create_dir_all(&self.data_dir)?;

        let mut attempts = 0usize;
        let mut last_err: Option<String> = None;

        for url in &src.candidates {
            attempts += 1;
            match self.fetch_and_convert(src, url).await {
                Ok((bytes, entries, payload)) => {
                    let target = self.data_dir.join(&src.target_file);
                    // 原子写：先写临时文件再改名，避免中断损坏已有数据
                    let tmp = self.data_dir.join(format!("{}.tmp", src.target_file));
                    std::fs::write(&tmp, &payload)?;
                    std::fs::rename(&tmp, &target)?;
                    return Ok(UpdateReport {
                        id: src.id.clone(),
                        name: src.name.clone(),
                        ok: true,
                        url_used: Some(url.clone()),
                        attempts,
                        bytes,
                        entries,
                        target: Some(target.display().to_string()),
                        error: None,
                        duration_ms: started.elapsed().as_secs_f64() * 1000.0,
                    });
                }
                Err(e) => {
                    tracing::warn!("数据源 {} 地址 {} 失败: {}", src.id, url, e);
                    last_err = Some(format!("{}: {}", url, e));
                }
            }
        }

        Err(CoreError::Network(format!(
            "数据源 {} 的 {} 个候选地址全部失败，最后错误: {}",
            src.id,
            attempts,
            last_err.unwrap_or_else(|| "未知".into())
        )))
    }

    /// 下载并转换成目标格式，返回 (原始字节数, 条目数, 落地内容)。
    async fn fetch_and_convert(
        &self,
        src: &DataSource,
        url: &str,
    ) -> Result<(usize, usize, Vec<u8>)> {
        let resp = self.http.get(url).await?;
        if resp.status != 200 {
            return Err(CoreError::Protocol(format!("HTTP {}", resp.status)));
        }
        if resp.body.is_empty() {
            return Err(CoreError::Protocol("响应为空".into()));
        }
        let raw_len = resp.body.len();

        let (entries, payload): (usize, Vec<u8>) = match src.kind {
            DataKind::DnsmasqConf => {
                let list = parse_dnsmasq_conf(&resp.text());
                let body = render_list(
                    "NoFakeIP 国内域名规则库（在线更新）",
                    url,
                    &list,
                );
                (list.len(), body.into_bytes())
            }
            DataKind::CidrList => {
                let list = parse_cidr_list(&resp.text());
                let body = render_list("NoFakeIP 中国 IP 网段（在线更新）", url, &list);
                (list.len(), body.into_bytes())
            }
            DataKind::CsvDomainRank => {
                let cn = load_existing_set(&self.data_dir.join("china_domains.txt"));
                let list = parse_csv_rank(&resp.text(), &cn, 20_000);
                let body = render_list("NoFakeIP 国外域名规则库（在线更新）", url, &list);
                (list.len(), body.into_bytes())
            }
            DataKind::DomainList => {
                let list: Vec<String> = BuiltinData::parse_lines(&resp.text())
                    .into_iter()
                    .filter_map(|d| registrable(&d))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let body = render_list("NoFakeIP 域名规则库（在线更新）", url, &list);
                (list.len(), body.into_bytes())
            }
        };

        if entries == 0 {
            return Err(CoreError::Protocol(
                "解析后条目数为 0，上游数据格式可能已变更".into(),
            ));
        }
        Ok((raw_len, entries, payload))
    }
}

// —————————————————————— 解析与工具函数 ——————————————————————

fn count_entries(text: &str) -> usize {
    text.lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .count()
}

/// 归约到「可注册域名」。使用 psl；未知 TLD 时退化为取最后两段。
pub fn registrable(domain: &str) -> Option<String> {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if d.is_empty() || d.contains(' ') {
        return None;
    }
    if let Some(root) = psl::domain_str(&d) {
        return Some(root.to_string());
    }
    let parts: Vec<&str> = d.split('.').collect();
    if parts.len() >= 2 && parts.iter().all(|p| !p.is_empty()) {
        Some(format!(
            "{}.{}",
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        ))
    } else {
        None
    }
}

fn render_list(title: &str, source: &str, items: &[String]) -> String {
    let mut out = String::with_capacity(items.len() * 24 + 256);
    out.push_str(&format!("# {}\n", title));
    out.push_str(&format!("# 来源: {}\n", source));
    out.push_str(&format!("# 更新时间: {}\n", chrono::Utc::now().to_rfc3339()));
    out.push_str("# 本文件由 NoFakeIP 自动生成，手工修改会在下次更新时被覆盖。\n\n");
    for it in items {
        out.push_str(it);
        out.push('\n');
    }
    out
}

fn parse_dnsmasq_conf(text: &str) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(rest) = l.strip_prefix("server=/") {
            if let Some((domain, _)) = rest.split_once('/') {
                if let Some(r) = registrable(domain) {
                    set.insert(r);
                }
            }
        }
    }
    set.into_iter().collect()
}

fn parse_cidr_list(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if l.contains('/') && l.parse::<ipnet::IpNet>().is_ok() {
            out.push(l.to_string());
        }
    }
    out
}

fn parse_csv_rank(
    text: &str,
    exclude: &std::collections::HashSet<String>,
    limit: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines().skip(1) {
        if out.len() >= limit {
            break;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 3 {
            continue;
        }
        if let Some(r) = registrable(cols[2]) {
            if exclude.contains(&r) || !seen.insert(r.clone()) {
                continue;
            }
            out.push(r);
        }
    }
    out
}

fn load_existing_set(path: &Path) -> std::collections::HashSet<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => BuiltinData::parse_lines(&text).into_iter().collect(),
        Err(_) => BuiltinData::parse_lines(BuiltinData::china_domains())
            .into_iter()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_sources_have_candidates() {
        for s in sources() {
            assert!(!s.candidates.is_empty(), "{} 缺少候选地址", s.id);
            assert!(!s.target_file.is_empty());
        }
    }

    #[test]
    fn source_ids_unique() {
        let mut ids: Vec<String> = sources().into_iter().map(|s| s.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn no_prohibited_data_sources() {
        // 明确约束：不包含任何代理软件生态的数据文件
        for s in sources() {
            let blob = format!("{} {} {}", s.id, s.target_file, s.candidates.join(" "));
            let lower = blob.to_ascii_lowercase();
            for banned in [".dat", ".mmdb", ".metadb", "geox", "meta-rules-dat"] {
                assert!(
                    !lower.contains(banned),
                    "数据源 {} 仍包含 Clash 相关内容: {}",
                    s.id,
                    banned
                );
            }
        }
    }

    #[test]
    fn registrable_reduces_subdomains() {
        assert_eq!(registrable("www.baidu.com").as_deref(), Some("baidu.com"));
        assert_eq!(
            registrable("a.b.example.co.uk").as_deref(),
            Some("example.co.uk")
        );
        assert_eq!(registrable("").as_deref(), None);
    }

    #[test]
    fn parse_dnsmasq_lines() {
        let text = "# c\nserver=/0.zone/114.114.114.114\nserver=/www.taobao.com/223.5.5.5\n";
        let list = parse_dnsmasq_conf(text);
        assert!(list.contains(&"0.zone".to_string()));
        assert!(list.contains(&"taobao.com".to_string()));
    }

    #[test]
    fn parse_cidr_filters_invalid() {
        let text = "# c\n1.0.1.0/24\nnot-a-cidr\n8.8.8.8\n";
        let list = parse_cidr_list(text);
        assert_eq!(list, vec!["1.0.1.0/24".to_string()]);
    }

    #[test]
    fn parse_csv_rank_excludes_and_dedups() {
        let text = "GlobalRank,TldRank,Domain,TLD\n1,1,google.com,com\n2,2,www.google.com,com\n3,3,baidu.com,com\n";
        let mut exclude = std::collections::HashSet::new();
        exclude.insert("baidu.com".to_string());
        let list = parse_csv_rank(text, &exclude, 10);
        assert_eq!(list, vec!["google.com".to_string()]);
    }

    #[test]
    fn render_list_has_header() {
        let s = render_list("标题", "https://x", &["a.com".into()]);
        assert!(s.contains("# 标题"));
        assert!(s.ends_with("a.com\n"));
    }

    #[test]
    fn count_entries_skips_comments() {
        assert_eq!(count_entries("# a\n\nx.com\ny.com\n"), 2);
    }
}
