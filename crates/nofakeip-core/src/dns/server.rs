//! DNS 服务器管理：内置节点、用户输入解析、导入导出。
//!
//! 用户输入解析规则见 `doc/02-核心引擎设计.md` 第 4.3 节。

use crate::data::BuiltinData;
use crate::error::{CoreError, Result};
use crate::models::{DnsProtocol, DnsServer, Region};
use serde::{Deserialize, Serialize};

/// 内置节点文件里的一条记录（比 [`DnsServer`] 多一个 `default_enabled`）。
#[derive(Debug, Clone, Deserialize)]
pub struct BuiltinEntry {
    pub name: String,
    pub protocol: String,
    pub address: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub bootstrap_ip: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub remark: Option<String>,
    /// 是否默认参与测速
    #[serde(default)]
    pub default_enabled: bool,
}

#[derive(Debug, Deserialize)]
struct BuiltinFile {
    #[serde(default)]
    #[allow(dead_code)]
    version: u32,
    #[serde(default)]
    #[allow(dead_code)]
    updated_at: Option<String>,
    servers: Vec<BuiltinEntry>,
}

/// 导入/导出用的 JSON 包装。
#[derive(Debug, Serialize, Deserialize)]
pub struct ServerExport {
    pub version: u32,
    pub exported_at: String,
    pub servers: Vec<DnsServer>,
}

/// DNS 服务器注册表。
#[derive(Debug, Clone)]
pub struct ServerRegistry {
    builtin: Vec<DnsServer>,
    /// 与 `builtin` 一一对应：是否默认启用
    default_flags: Vec<bool>,
}

impl Default for ServerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerRegistry {
    /// 从内置数据加载。解析失败时退化为空表并记录日志（不应发生）。
    pub fn new() -> Self {
        match Self::from_json(BuiltinData::builtin_servers_json()) {
            Ok((servers, flags)) => Self {
                builtin: servers,
                default_flags: flags,
            },
            Err(e) => {
                tracing::error!("内置 DNS 节点解析失败: {}", e);
                Self {
                    builtin: Vec::new(),
                    default_flags: Vec::new(),
                }
            }
        }
    }

    fn from_json(json: &str) -> Result<(Vec<DnsServer>, Vec<bool>)> {
        let file: BuiltinFile =
            serde_json::from_str(json).map_err(|e| CoreError::Serde(e.to_string()))?;
        let mut servers = Vec::with_capacity(file.servers.len());
        let mut flags = Vec::with_capacity(file.servers.len());
        for (i, e) in file.servers.into_iter().enumerate() {
            let protocol = DnsProtocol::parse(&e.protocol)
                .ok_or_else(|| CoreError::Config(format!("未知协议: {}", e.protocol)))?;
            let region = e
                .region
                .as_deref()
                .and_then(Region::parse)
                .unwrap_or(Region::Unknown);
            servers.push(DnsServer {
                id: -(i as i64 + 1), // 内置节点用负数 ID，避免与数据库自增 ID 冲突
                name: e.name,
                protocol,
                address: e.address,
                host: e.host,
                bootstrap_ip: e.bootstrap_ip,
                region,
                enabled: true,
                builtin: true,
                tags: e.tags,
                remark: e.remark,
            });
            flags.push(e.default_enabled);
        }
        Ok((servers, flags))
    }

    /// 全部内置节点。
    pub fn builtin(&self) -> Vec<DnsServer> {
        self.builtin.clone()
    }

    /// 默认参与测速的节点（国内 + 国外各若干）。
    pub fn default_set(&self) -> Vec<DnsServer> {
        self.builtin
            .iter()
            .zip(self.default_flags.iter())
            .filter(|(_, f)| **f)
            .map(|(s, _)| s.clone())
            .collect()
    }

    /// 按名称或 ID 查找内置节点。
    pub fn find_builtin(&self, key: &str) -> Option<DnsServer> {
        if let Ok(id) = key.parse::<i64>() {
            return self.builtin.iter().find(|s| s.id == id).cloned();
        }
        self.builtin
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(key))
            .cloned()
    }

    /// 解析用户输入的一行 DNS 配置。
    ///
    /// 支持形式：
    /// - `223.5.5.5` / `223.5.5.5:53`
    /// - `udp://8.8.8.8`、`tcp://8.8.8.8`
    /// - `tls://1.1.1.1:853`、`dot://dns.quad9.net#dns.quad9.net`
    /// - `https://dns.alidns.com/dns-query`、`doh://dns.quad9.net`
    /// - `1.1.1.1@853#cloudflare-dns.com`（通用 `ip@端口#SNI` 风格）
    pub fn parse_server(&self, raw: &str) -> Result<DnsServer> {
        let input = raw.trim();
        if input.is_empty() {
            return Err(CoreError::Config("DNS 配置为空".into()));
        }

        // 通用风格：ip@port#sni
        if let Some((main, sni)) = input.split_once('#') {
            let (host, port) = match main.split_once('@') {
                Some((h, p)) => (
                    h.to_string(),
                    p.parse::<u16>()
                        .map_err(|_| CoreError::Config(format!("非法端口: {}", p)))?,
                ),
                None => (main.to_string(), 853u16),
            };
            let proto = if port == 853 {
                DnsProtocol::Dot
            } else {
                DnsProtocol::Udp
            };
            let mut s = DnsServer::new(host.clone(), proto, format!("{}:{}", host, port));
            s.host = Some(sni.trim().to_string());
            return Ok(s);
        }

        if input.starts_with("https://") || input.starts_with("http://") {
            let ep = crate::dns::transport::parse_doh_url(input)?;
            let mut s = DnsServer::new(ep.host.clone(), DnsProtocol::Doh, input);
            s.host = Some(ep.host);
            return Ok(s);
        }
        if let Some(rest) = input.strip_prefix("doh://") {
            let host = rest.split('/').next().unwrap_or(rest).to_string();
            let address = format!("https://{}/dns-query", host);
            let mut s = DnsServer::new(host.clone(), DnsProtocol::Doh, address);
            s.host = Some(host);
            return Ok(s);
        }
        if let Some(rest) = input.strip_prefix("tls://") {
            return Self::parse_host_port(rest, DnsProtocol::Dot, 853);
        }
        if let Some(rest) = input.strip_prefix("dot://") {
            return Self::parse_host_port(rest, DnsProtocol::Dot, 853);
        }
        if let Some(rest) = input.strip_prefix("tcp://") {
            return Self::parse_host_port(rest, DnsProtocol::Tcp, 53);
        }
        if let Some(rest) = input.strip_prefix("udp://") {
            return Self::parse_host_port(rest, DnsProtocol::Udp, 53);
        }

        Self::parse_host_port(input, DnsProtocol::Udp, 53)
    }

    fn parse_host_port(rest: &str, proto: DnsProtocol, default_port: u16) -> Result<DnsServer> {
        let rest = rest.trim().trim_end_matches('/');
        if rest.is_empty() {
            return Err(CoreError::Config("缺少主机地址".into()));
        }
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (
                h.to_string(),
                p.parse::<u16>()
                    .map_err(|_| CoreError::Config(format!("非法端口: {}", p)))?,
            ),
            _ => (rest.to_string(), default_port),
        };
        if host.is_empty() {
            return Err(CoreError::Config("缺少主机地址".into()));
        }
        let mut s = DnsServer::new(host.clone(), proto, format!("{}:{}", host, port));
        if proto == DnsProtocol::Dot {
            // DoT 的 SNI 默认取主机名（若本身就是 IP，则留空由调用方决定）
            if host.parse::<std::net::IpAddr>().is_err() {
                s.host = Some(host);
            }
        }
        Ok(s)
    }

    /// 导出为 JSON。
    pub fn export_json(&self, servers: &[DnsServer]) -> Result<String> {
        let doc = ServerExport {
            version: 1,
            exported_at: chrono::Utc::now().to_rfc3339(),
            servers: servers.to_vec(),
        };
        serde_json::to_string_pretty(&doc).map_err(|e| CoreError::Serde(e.to_string()))
    }

    /// 从 JSON 导入。
    pub fn import_json(&self, json: &str) -> Result<Vec<DnsServer>> {
        let doc: ServerExport =
            serde_json::from_str(json).map_err(|e| CoreError::Serde(e.to_string()))?;
        Ok(doc.servers)
    }

    /// 导出为 TXT（一行一个，便于手改）。
    pub fn export_txt(&self, servers: &[DnsServer]) -> String {
        let mut out = String::from("# NoFakeIP DNS 列表\n# 格式参考 doc/03-数据模型与存储.md 第 7.2 节\n\n");
        for s in servers {
            out.push_str(&format!(
                "# {} | {} | {}\n",
                s.name,
                s.region.as_str(),
                s.protocol.label()
            ));
            out.push_str(&format!("{}\n", s.address));
        }
        out
    }

    /// 从 TXT 导入（忽略注释与空行）。
    pub fn import_txt(&self, txt: &str) -> Result<Vec<DnsServer>> {
        let mut out = Vec::new();
        for line in txt.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            match self.parse_server(l) {
                Ok(s) => out.push(s),
                Err(e) => tracing::warn!("跳过无法解析的行 `{}`: {}", l, e),
            }
        }
        if out.is_empty() {
            return Err(CoreError::Config("未解析出任何有效 DNS".into()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_loaded() {
        let reg = ServerRegistry::new();
        assert!(reg.builtin().len() >= 9);
        assert!(reg.default_set().len() >= 6);
        assert!(reg.builtin().iter().all(|s| s.builtin));
    }

    #[test]
    fn default_set_contains_both_regions() {
        let reg = ServerRegistry::new();
        let d = reg.default_set();
        assert!(d.iter().any(|s| s.region == Region::China));
        assert!(d.iter().any(|s| s.region == Region::Global));
    }

    #[test]
    fn parse_plain_ip() {
        let reg = ServerRegistry::new();
        let s = reg.parse_server("223.5.5.5").unwrap();
        assert_eq!(s.protocol, DnsProtocol::Udp);
        assert_eq!(s.address, "223.5.5.5:53");
    }

    #[test]
    fn parse_tcp_scheme() {
        let reg = ServerRegistry::new();
        let s = reg.parse_server("tcp://8.8.8.8").unwrap();
        assert_eq!(s.protocol, DnsProtocol::Tcp);
        assert_eq!(s.address, "8.8.8.8:53");
    }

    #[test]
    fn parse_dot_with_scheme() {
        let reg = ServerRegistry::new();
        let s = reg.parse_server("tls://dns.quad9.net:853").unwrap();
        assert_eq!(s.protocol, DnsProtocol::Dot);
        assert_eq!(s.host.as_deref(), Some("dns.quad9.net"));
    }

    #[test]
    fn parse_at_hash_style() {
        let reg = ServerRegistry::new();
        let s = reg.parse_server("1.1.1.1@853#cloudflare-dns.com").unwrap();
        assert_eq!(s.protocol, DnsProtocol::Dot);
        assert_eq!(s.address, "1.1.1.1:853");
        assert_eq!(s.host.as_deref(), Some("cloudflare-dns.com"));
    }

    #[test]
    fn parse_doh_url_and_shortcut() {
        let reg = ServerRegistry::new();
        let a = reg.parse_server("https://dns.alidns.com/dns-query").unwrap();
        assert_eq!(a.protocol, DnsProtocol::Doh);
        assert_eq!(a.host.as_deref(), Some("dns.alidns.com"));

        let b = reg.parse_server("doh://dns.quad9.net").unwrap();
        assert_eq!(b.address, "https://dns.quad9.net/dns-query");
    }

    #[test]
    fn parse_empty_errors() {
        let reg = ServerRegistry::new();
        assert!(reg.parse_server("   ").is_err());
    }

    #[test]
    fn export_import_json_roundtrip() {
        let reg = ServerRegistry::new();
        let servers = reg.default_set();
        let json = reg.export_json(&servers).unwrap();
        let back = reg.import_json(&json).unwrap();
        assert_eq!(servers.len(), back.len());
        assert_eq!(servers[0].address, back[0].address);
    }

    #[test]
    fn export_import_txt_roundtrip() {
        let reg = ServerRegistry::new();
        let servers = vec![
            reg.parse_server("223.5.5.5").unwrap(),
            reg.parse_server("https://dns.google/dns-query").unwrap(),
        ];
        let txt = reg.export_txt(&servers);
        let back = reg.import_txt(&txt).unwrap();
        assert_eq!(back.len(), 2);
    }

    #[test]
    fn import_txt_rejects_garbage() {
        let reg = ServerRegistry::new();
        assert!(reg.import_txt("# only comments\n\n").is_err());
    }
}
