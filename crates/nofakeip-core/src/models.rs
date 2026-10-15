//! 跨模块共享的数据模型。
//!
//! 契约见 `doc/02-核心引擎设计.md` 第 3 节。所有新增字段必须带 `#[serde(default)]`，
//! 以保证旧数据库 / 旧 JSON 仍可反序列化。

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// DNS 传输协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DnsProtocol {
    /// 明文 UDP，默认 53
    Udp,
    /// 明文 TCP，默认 53
    Tcp,
    /// DNS over TLS，默认 853
    Dot,
    /// DNS over HTTPS，默认 443
    Doh,
}

impl DnsProtocol {
    pub fn default_port(&self) -> u16 {
        match self {
            DnsProtocol::Udp | DnsProtocol::Tcp => 53,
            DnsProtocol::Dot => 853,
            DnsProtocol::Doh => 443,
        }
    }

    /// 展示用短名，用于 CLI / GUI 表格。
    pub fn scheme(&self) -> &'static str {
        match self {
            DnsProtocol::Udp => "udp",
            DnsProtocol::Tcp => "tcp",
            DnsProtocol::Dot => "tls",
            DnsProtocol::Doh => "https",
        }
    }

    /// 人类可读名称。
    pub fn label(&self) -> &'static str {
        match self {
            DnsProtocol::Udp => "UDP",
            DnsProtocol::Tcp => "TCP",
            DnsProtocol::Dot => "DoT",
            DnsProtocol::Doh => "DoH",
        }
    }

    /// 是否为加密传输（用于评分加权）。
    pub fn is_encrypted(&self) -> bool {
        matches!(self, DnsProtocol::Dot | DnsProtocol::Doh)
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "udp" => Some(DnsProtocol::Udp),
            "tcp" => Some(DnsProtocol::Tcp),
            "dot" | "tls" => Some(DnsProtocol::Dot),
            "doh" | "https" => Some(DnsProtocol::Doh),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            DnsProtocol::Udp => "udp",
            DnsProtocol::Tcp => "tcp",
            DnsProtocol::Dot => "dot",
            DnsProtocol::Doh => "doh",
        }
    }
}

impl std::fmt::Display for DnsProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// 归属区域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    /// 国内
    China,
    /// 国外 / 全球
    Global,
    /// 无法判定
    Unknown,
}

impl Default for Region {
    fn default() -> Self {
        Region::Unknown
    }
}

impl Region {
    pub fn label(&self) -> &'static str {
        match self {
            Region::China => "国内",
            Region::Global => "国外",
            Region::Unknown => "未知",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Region::China => "china",
            Region::Global => "global",
            Region::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "china" | "cn" | "国内" => Some(Region::China),
            "global" | "foreign" | "国外" => Some(Region::Global),
            "unknown" | "未知" => Some(Region::Unknown),
            _ => None,
        }
    }
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// 一个 DNS 服务器节点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DnsServer {
    /// 0 表示尚未持久化
    #[serde(default)]
    pub id: i64,
    /// 展示名，如 "阿里 DoH"
    pub name: String,
    pub protocol: DnsProtocol,
    /// UDP/TCP: `223.5.5.5:53`；DoT: `1.1.1.1:853`；DoH: `https://dns.alidns.com/dns-query`
    pub address: String,
    /// DoT 的 SNI / DoH 的 Host 头；缺省时从 address 推导
    #[serde(default)]
    pub host: Option<String>,
    /// 服务器域名自身解析用（避免 bootstrap 解析的循环依赖）
    #[serde(default)]
    pub bootstrap_ip: Option<String>,
    #[serde(default = "default_region")]
    pub region: Region,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 内置节点不可删除，只能禁用
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub remark: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_region() -> Region {
    Region::Unknown
}

impl DnsServer {
    /// 便捷构造：仅名称 + 地址（协议由地址推导）。
    pub fn new(name: impl Into<String>, protocol: DnsProtocol, address: impl Into<String>) -> Self {
        Self {
            id: 0,
            name: name.into(),
            protocol,
            address: address.into(),
            host: None,
            bootstrap_ip: None,
            region: Region::Unknown,
            enabled: true,
            builtin: false,
            tags: Vec::new(),
            remark: None,
        }
    }

    pub fn with_region(mut self, region: Region) -> Self {
        self.region = region;
        self
    }

    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    pub fn with_tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// 该服务器是否可用于解析「国内域名」。
    pub fn suits_china(&self) -> bool {
        matches!(self.region, Region::China)
    }

    /// 该服务器是否可用于解析「国外域名」。
    pub fn suits_global(&self) -> bool {
        matches!(self.region, Region::Global)
    }

    /// 展示用端点字符串，如 `https://dns.alidns.com/dns-query`、`tls://1.1.1.1:853`。
    pub fn display_endpoint(&self) -> String {
        match self.protocol {
            DnsProtocol::Doh => self.address.clone(),
            DnsProtocol::Dot => format!("tls://{}", self.address),
            DnsProtocol::Tcp => format!("tcp://{}", self.address),
            DnsProtocol::Udp => self.address.clone(),
        }
    }
}

/// 一个 DNS 应答中的 IP + TTL 组合（便于后续扩展）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpAnswer {
    pub ip: IpAddr,
    #[serde(default)]
    pub ttl: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_default_ports() {
        assert_eq!(DnsProtocol::Udp.default_port(), 53);
        assert_eq!(DnsProtocol::Tcp.default_port(), 53);
        assert_eq!(DnsProtocol::Dot.default_port(), 853);
        assert_eq!(DnsProtocol::Doh.default_port(), 443);
    }

    #[test]
    fn protocol_roundtrip_serde() {
        for p in [
            DnsProtocol::Udp,
            DnsProtocol::Tcp,
            DnsProtocol::Dot,
            DnsProtocol::Doh,
        ] {
            let json = serde_json::to_string(&p).unwrap();
            let back: DnsProtocol = serde_json::from_str(&json).unwrap();
            assert_eq!(p, back);
        }
    }

    #[test]
    fn region_serde_lowercase() {
        assert_eq!(serde_json::to_string(&Region::China).unwrap(), "\"china\"");
        assert_eq!(
            serde_json::to_string(&Region::Global).unwrap(),
            "\"global\""
        );
        assert_eq!(
            serde_json::to_string(&Region::Unknown).unwrap(),
            "\"unknown\""
        );
    }

    #[test]
    fn server_display_endpoint() {
        let doh = DnsServer::new("阿里", DnsProtocol::Doh, "https://dns.alidns.com/dns-query");
        assert_eq!(doh.display_endpoint(), "https://dns.alidns.com/dns-query");

        let udp = DnsServer::new("阿里", DnsProtocol::Udp, "223.5.5.5:53");
        assert_eq!(udp.display_endpoint(), "223.5.5.5:53");

        let dot = DnsServer::new("CF", DnsProtocol::Dot, "1.1.1.1:853");
        assert_eq!(dot.display_endpoint(), "tls://1.1.1.1:853");
    }

    #[test]
    fn server_deserialize_with_missing_optional_fields() {
        // 兼容性：老 JSON 缺少可选字段也应能解析
        let json = r#"{"name":"x","protocol":"doh","address":"https://a/dns-query"}"#;
        let s: DnsServer = serde_json::from_str(json).unwrap();
        assert_eq!(s.id, 0);
        assert!(s.enabled);
        assert_eq!(s.region, Region::Unknown);
        assert!(s.tags.is_empty());
    }
}
