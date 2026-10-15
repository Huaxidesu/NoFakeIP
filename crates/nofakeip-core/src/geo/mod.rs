//! IP 归属地 / ASN 判定。
//!
//! 两级策略（见 `doc/05-污染与FakeIP检测规格.md` 第 6 节）：
//! 1. [`LocalGeo`]：**默认启用**，完全离线。内置中国主要网段（约 7400 条）+ 私网/保留段判定；
//! 2. [`OnlineGeo`]：**默认关闭**，需用户显式开启。开启后会把 IP 发送给第三方查询服务。
//!
//! 之所以默认离线：本工具承诺「数据本地化」，且 Android 上不应有额外出网依赖。

use crate::data::BuiltinData;
use crate::error::Result;
use crate::net::HttpClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Mutex;

/// 归属地来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeoSource {
    /// 内置离线数据
    Local,
    /// 在线查询
    Online,
    /// 未判定
    Unknown,
}

/// 单个 IP 的归属信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpGeo {
    pub ip: IpAddr,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub asn: Option<u32>,
    #[serde(default)]
    pub as_name: Option<String>,
    /// 是否属于中国大陆
    pub is_china: bool,
    /// 是否私网地址（RFC1918 等）
    pub is_private: bool,
    /// 是否保留 / 特殊用途地址（含 0.0.0.0、127.0.0.0/8、224/4、240/4 等）
    pub is_reserved: bool,
    pub source: GeoSource,
}

impl IpGeo {
    pub fn unknown(ip: IpAddr) -> Self {
        Self {
            ip,
            country_code: None,
            country: None,
            asn: None,
            as_name: None,
            is_china: false,
            is_private: false,
            is_reserved: false,
            source: GeoSource::Unknown,
        }
    }
}

/// 归属地查询能力（同步，供分类器与污染检测使用）。
pub trait GeoProvider: Send + Sync {
    fn lookup(&self, ip: IpAddr) -> Option<IpGeo>;

    /// 批量查询（默认逐个 lookup）。
    fn lookup_many(&self, ips: &[IpAddr]) -> Vec<IpGeo> {
        ips.iter()
            .map(|ip| self.lookup(*ip).unwrap_or_else(|| IpGeo::unknown(*ip)))
            .collect()
    }
}

/// 离线归属地判定。
pub struct LocalGeo {
    /// 中国 IPv4 网段，按起始地址排序，便于二分
    v4: Vec<(u32, u32)>,
    /// 中国 IPv6 网段
    v6: Vec<ipnet::Ipv6Net>,
}

impl Default for LocalGeo {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalGeo {
    /// 使用内置数据构建。
    pub fn new() -> Self {
        Self::from_text(BuiltinData::china_ip_cidr())
    }

    /// 优先加载用户数据目录里的 `china_ip_cidr.txt`。
    pub fn with_data_dir(dir: Option<&Path>) -> Self {
        match BuiltinData::load_or_embedded(dir, "china_ip_cidr.txt") {
            Ok(text) => Self::from_text(&text),
            Err(_) => Self::from_text(BuiltinData::china_ip_cidr()),
        }
    }

    /// 从「一行一个 CIDR」文本构建。
    pub fn from_text(text: &str) -> Self {
        let mut v4: Vec<(u32, u32)> = Vec::new();
        let mut v6: Vec<ipnet::Ipv6Net> = Vec::new();
        for line in BuiltinData::parse_lines(text) {
            match line.parse::<ipnet::IpNet>() {
                Ok(net) => match net {
                    ipnet::IpNet::V4(n) => {
                        let start = u32::from(n.network());
                        let end = u32::from(n.broadcast());
                        v4.push((start, end));
                    }
                    ipnet::IpNet::V6(n) => v6.push(n.trunc()),
                },
                Err(_) => continue,
            }
        }
        v4.sort_unstable();
        Self { v4, v6 }
    }

    pub fn v4_net_count(&self) -> usize {
        self.v4.len()
    }

    fn in_china(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                let target = u32::from(v4);
                // 二分找最后一个 start <= target
                let idx = self.v4.partition_point(|(s, _)| *s <= target);
                if idx == 0 {
                    return false;
                }
                let (s, e) = self.v4[idx - 1];
                target >= s && target <= e
            }
            IpAddr::V6(v6) => self.v6.iter().any(|n| n.contains(&v6)),
        }
    }
}

impl GeoProvider for LocalGeo {
    fn lookup(&self, ip: IpAddr) -> Option<IpGeo> {
        let mut g = IpGeo::unknown(ip);
        g.is_private = is_private(ip);
        g.is_reserved = is_reserved(ip);
        g.is_china = !g.is_private && !g.is_reserved && self.in_china(ip);
        g.source = GeoSource::Local;
        g.country_code = if g.is_china {
            Some("CN".into())
        } else if g.is_private || g.is_reserved {
            None
        } else {
            Some("??".into()) // 离线表只能确认「不属于中国大陆」
        };
        g.country = if g.is_china {
            Some("中国".into())
        } else {
            None
        };
        Some(g)
    }
}

/// 在线归属地查询（默认关闭）。
///
/// 使用 ip-api.com 的免费接口（HTTP，无需 key，限速 45 次/分钟）。
/// **隐私提示**：开启后会把解析到的 IP 发送给第三方，GUI 上必须提示用户。
pub struct OnlineGeo {
    http: HttpClient,
    cache: Mutex<HashMap<IpAddr, IpGeo>>,
}

impl Default for OnlineGeo {
    fn default() -> Self {
        Self::new()
    }
}

impl OnlineGeo {
    pub fn new() -> Self {
        Self {
            http: HttpClient::new().with_timeout(std::time::Duration::from_secs(8)),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// 设置在线查询使用的出站代理（None = 直连）。
    pub fn with_proxy(mut self, proxy: Option<crate::net::ProxyUrl>) -> Self {
        self.http = self.http.with_proxy(proxy);
        self
    }

    /// 查询单个 IP（带内存缓存）。
    pub async fn lookup_async(&self, ip: IpAddr) -> Option<IpGeo> {
        if let Ok(cache) = self.cache.lock() {
            if let Some(hit) = cache.get(&ip) {
                return Some(hit.clone());
            }
        }
        if is_private(ip) || is_reserved(ip) {
            let mut g = IpGeo::unknown(ip);
            g.is_private = is_private(ip);
            g.is_reserved = is_reserved(ip);
            g.source = GeoSource::Local;
            return Some(g);
        }
        let url = format!(
            "http://ip-api.com/json/{}?fields=status,country,countryCode,as",
            ip
        );
        let text = self.http.get_text(&url).await.ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        if v.get("status").and_then(|s| s.as_str()) != Some("success") {
            return None;
        }
        let country_code = v
            .get("countryCode")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let country = v
            .get("country")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let (asn, as_name) = parse_as_field(v.get("as").and_then(|s| s.as_str()));
        let g = IpGeo {
            ip,
            country_code: country_code.clone(),
            country,
            asn,
            as_name,
            is_china: country_code.as_deref() == Some("CN"),
            is_private: false,
            is_reserved: false,
            source: GeoSource::Online,
        };
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(ip, g.clone());
        }
        Some(g)
    }

    /// 批量查询（串行 + 缓存，避免触发限速）。
    pub async fn lookup_many_async(&self, ips: &[IpAddr]) -> Vec<IpGeo> {
        let mut out = Vec::with_capacity(ips.len());
        for ip in ips {
            out.push(
                self.lookup_async(*ip)
                    .await
                    .unwrap_or_else(|| IpGeo::unknown(*ip)),
            );
        }
        out
    }
}

/// 解析 ip-api 的 `as` 字段，形如 `AS15169 Google LLC`。
fn parse_as_field(s: Option<&str>) -> (Option<u32>, Option<String>) {
    let Some(s) = s else {
        return (None, None);
    };
    let mut parts = s.splitn(2, ' ');
    let asn = parts
        .next()
        .and_then(|p| p.trim_start_matches("AS").parse::<u32>().ok());
    let name = parts.next().map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    (asn, name)
}

/// 是否私网地址。
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_link_local() || v4.is_loopback() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
        }
    }
}

/// 是否保留 / 特殊用途地址。
pub fn is_reserved(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            let u = u32::from(v4);
            // 0.0.0.0/8
            o[0] == 0
            // 100.64.0.0/10 (CGNAT)
            || (u & 0xFFC0_0000) == 0x6440_0000
            // 192.0.0.0/24
            || (o[0] == 192 && o[1] == 0 && o[2] == 0)
            // 192.0.2.0/24 (TEST-NET-1)
            || (o[0] == 192 && o[1] == 0 && o[2] == 2)
            // 198.18.0.0/15 (benchmark / Fake-IP)
            || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
            // 198.51.100.0/24 (TEST-NET-2)
            || (o[0] == 198 && o[1] == 51 && o[2] == 100)
            // 203.0.113.0/24 (TEST-NET-3)
            || (o[0] == 203 && o[1] == 0 && o[2] == 113)
            // 224.0.0.0/4 组播 + 240.0.0.0/4 保留
            || o[0] >= 224
            || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            v6.is_multicast()
                || v6.is_unspecified()
                || (v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8) // 2001:db8::/32
        }
    }
}

/// 判断 IP 是否落在某个 Fake-IP 网段内。
pub fn in_net(ip: IpAddr, net: &ipnet::IpNet) -> bool {
    net.contains(&ip)
}

/// 便捷：解析 CIDR。
pub fn parse_net(cidr: &str) -> Result<ipnet::IpNet> {
    cidr.trim()
        .parse::<ipnet::IpNet>()
        .map_err(|e| crate::error::CoreError::Config(format!("非法 CIDR `{}`: {}", cidr, e)))
}

/// 判断 IPv4 是否属于同一 /24（用于判断「同网段不同 IP」这类轻微不一致）。
pub fn same_24(a: Ipv4Addr, b: Ipv4Addr) -> bool {
    a.octets()[..3] == b.octets()[..3]
}

/// 判断两个 IPv6 是否属于同一 /48。
pub fn same_48(a: Ipv6Addr, b: Ipv6Addr) -> bool {
    a.segments()[..3] == b.segments()[..3]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn local_geo_loaded() {
        let g = LocalGeo::new();
        assert!(g.v4_net_count() > 1000, "内置 CN 网段应有数千条");
    }

    #[test]
    fn china_ip_detected() {
        let g = LocalGeo::new();
        // 223.5.5.5 是阿里 DNS，属于国内
        let r = g.lookup(ip("223.5.5.5")).unwrap();
        assert!(r.is_china, "223.5.5.5 应判定为中国");
        assert_eq!(r.country_code.as_deref(), Some("CN"));
        // 1.1.1.1 属于 Cloudflare
        let r2 = g.lookup(ip("1.1.1.1")).unwrap();
        assert!(!r2.is_china);
    }

    #[test]
    fn private_and_reserved() {
        assert!(is_private(ip("192.168.1.1")));
        assert!(is_private(ip("10.0.0.1")));
        assert!(is_private(ip("172.16.0.1")));
        assert!(is_private(ip("127.0.0.1")));
        assert!(is_private(ip("fe80::1")));
        assert!(!is_private(ip("8.8.8.8")));

        assert!(is_reserved(ip("198.18.0.5")));
        assert!(is_reserved(ip("240.0.0.1")));
        assert!(is_reserved(ip("0.0.0.0")));
        assert!(is_reserved(ip("100.64.0.1")));
        assert!(!is_reserved(ip("8.8.8.8")));
    }

    #[test]
    fn private_ip_not_china() {
        let g = LocalGeo::new();
        let r = g.lookup(ip("192.168.1.1")).unwrap();
        assert!(!r.is_china);
        assert!(r.is_private);
    }

    #[test]
    fn parse_as_field_formats() {
        assert_eq!(
            parse_as_field(Some("AS15169 Google LLC")),
            (Some(15169), Some("Google LLC".into()))
        );
        assert_eq!(parse_as_field(Some("AS13335")), (Some(13335), None));
        assert_eq!(parse_as_field(None), (None, None));
    }

    #[test]
    fn cidr_helpers() {
        let n = parse_net("198.18.0.0/15").unwrap();
        assert!(in_net(ip("198.18.5.5"), &n));
        assert!(!in_net(ip("198.20.0.1"), &n));
        assert!(parse_net("bad").is_err());
    }

    #[test]
    fn same_prefix_helpers() {
        assert!(same_24(
            "1.2.3.4".parse().unwrap(),
            "1.2.3.99".parse().unwrap()
        ));
        assert!(!same_24(
            "1.2.3.4".parse().unwrap(),
            "1.2.4.4".parse().unwrap()
        ));
    }
}
