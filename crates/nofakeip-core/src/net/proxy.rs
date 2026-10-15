//! 出站 HTTP 请求的代理支持。
//!
//! 目标用户：使用 mihomo / Clash 等裸内核、系统里没有系统代理的用户。
//! 本模块只负责让 NoFakeIP 自身的联网请求（规则数据更新、在线归属地查询）
//! 走用户指定的代理，**不生成、不修改任何代理软件的配置文件**。
//!
//! 支持：
//! - HTTP 代理（对 https 目标用 CONNECT 隧道，对 http 目标用绝对 URI）
//! - SOCKS5 代理（无认证；域名交给代理端解析，等效 socks5h）
//!
//! 探测优先级：环境变量（ALL_PROXY / HTTPS_PROXY / HTTP_PROXY 等）
//! → 本机常见代理端口（7890 / 7897 / 1080 / 10809）TCP 探测。

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// 代理协议类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyScheme {
    /// HTTP 代理（mihomo mixed-port 亦兼容）
    Http,
    /// SOCKS5（无认证）
    Socks5,
}

/// 已解析的代理地址。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyUrl {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
}

impl ProxyUrl {
    /// 解析代理 URL，如 `http://127.0.0.1:7890`、`socks5://127.0.0.1:7891`。
    ///
    /// `https://` 前缀按明文 HTTP 代理处理（本工具与代理之间一般走本机回环，
    /// 极少需要 TLS 到代理）。
    pub fn parse(raw: &str) -> Result<Self> {
        let s = raw.trim();
        let (scheme, rest) = s
            .split_once("://")
            .ok_or_else(|| CoreError::Config(format!("代理地址缺少协议头: {}", raw)))?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "http" | "https" => ProxyScheme::Http,
            "socks5" | "socks5h" => ProxyScheme::Socks5,
            other => {
                return Err(CoreError::Config(format!(
                    "不支持的代理协议 `{}`（支持 http / socks5）",
                    other
                )))
            }
        };
        let hostport = rest.split(['/', '?']).next().unwrap_or("");
        // 忽略用户信息段（如 user:pass@host:port；当前不支持认证，仅容忍其存在）
        let hostport = hostport.rsplit_once('@').map(|(_, h)| h).unwrap_or(hostport);
        if hostport.is_empty() {
            return Err(CoreError::Config(format!("代理地址缺少主机: {}", raw)));
        }
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (
                h.trim_matches(|c| c == '[' || c == ']').to_string(),
                p.parse::<u16>()
                    .map_err(|_| CoreError::Config(format!("非法代理端口: {}", p)))?,
            ),
            _ => (hostport.to_string(), 80),
        };
        if port == 0 {
            return Err(CoreError::Config(format!("非法代理端口: {}", raw)));
        }
        Ok(ProxyUrl { scheme, host, port })
    }
}

impl std::fmt::Display for ProxyUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = match self.scheme {
            ProxyScheme::Http => "http",
            ProxyScheme::Socks5 => "socks5",
        };
        write!(f, "{}://{}:{}", scheme, self.host, self.port)
    }
}

/// 代理模式（用户显式配置）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProxyMode {
    /// 自动探测：环境变量 → 本机常见代理端口 → 直连（默认）
    #[default]
    Auto,
    /// 强制直连（--no-proxy）
    Direct,
    /// 自定义代理地址（如 `http://127.0.0.1:7890`）
    Custom(String),
}

/// 自动探测时依次尝试的本机端口（mihomo/Clash 7890、7897，SS 1080，v2rayN 10809）。
pub const PROBE_PORTS: [u16; 4] = [7890, 7897, 1080, 10809];

/// 单个端口的 TCP 探测超时。
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// 按优先级解析最终使用的代理。
///
/// 优先级：显式配置 > 环境变量 > 端口探测 > 直连。
pub async fn resolve_proxy(mode: &ProxyMode) -> Result<Option<ProxyUrl>> {
    match mode {
        ProxyMode::Direct => Ok(None),
        ProxyMode::Auto => Ok(detect_proxy_url().await),
        ProxyMode::Custom(u) => ProxyUrl::parse(u).map(Some),
    }
}

/// 自动探测可用代理。
///
/// 依次尝试环境变量与本机常见端口；全部未命中则返回 None（直连）。
pub async fn detect_proxy_url() -> Option<ProxyUrl> {
    for key in [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if v.is_empty() {
                continue;
            }
            match ProxyUrl::parse(v) {
                Ok(p) => {
                    tracing::debug!("使用环境变量 {} 指定的代理: {}", key, p);
                    return Some(p);
                }
                Err(e) => tracing::warn!("环境变量 {} 不是合法代理地址（{}）", key, e),
            }
        }
    }
    for port in PROBE_PORTS {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let ok = timeout(PROBE_TIMEOUT, TcpStream::connect(addr))
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false);
        if ok {
            tracing::info!("探测到本机代理端口 127.0.0.1:{}，后续联网请求将走该代理", port);
            return Some(ProxyUrl {
                scheme: ProxyScheme::Http,
                host: "127.0.0.1".to_string(),
                port,
            });
        }
    }
    None
}

// —————————————————————— 传输层握手（供 HttpClient 使用） ——————————————————————

/// 经 SOCKS5 代理请求连接到 `host:port`（域名交给代理端解析）。
pub(crate) async fn socks5_connect(stream: &mut TcpStream, host: &str, port: u16) -> Result<()> {
    // 握手：声明无认证
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut greet = [0u8; 2];
    stream.read_exact(&mut greet).await?;
    if greet != [0x05, 0x00] {
        return Err(CoreError::Network(
            "SOCKS5 握手失败：服务器不支持无认证模式".into(),
        ));
    }
    // CONNECT 请求：优先用域名（ATYP=3），等效 socks5h
    let mut req = vec![0x05, 0x01, 0x00];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            req.push(0x01);
            req.extend_from_slice(&v4.octets());
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            req.push(0x04);
            req.extend_from_slice(&v6.octets());
        }
        Err(_) => {
            if host.len() > 255 {
                return Err(CoreError::Config(format!("代理目标主机名过长: {}", host)));
            }
            req.push(0x03);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&req).await?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[1] != 0x00 {
        return Err(CoreError::Network(format!(
            "SOCKS5 连接失败（应答码 {}）",
            head[1]
        )));
    }
    // 丢弃 BND.ADDR / BND.PORT
    let skip = match head[3] {
        0x01 => 4 + 2,
        0x04 => 16 + 2,
        0x03 => {
            let mut l = [0u8; 1];
            stream.read_exact(&mut l).await?;
            l[0] as usize + 2
        }
        other => {
            return Err(CoreError::Network(format!(
                "SOCKS5 非法应答地址类型: {}",
                other
            )))
        }
    };
    let mut rest = vec![0u8; skip];
    stream.read_exact(&mut rest).await?;
    Ok(())
}

/// 经 HTTP 代理建立 CONNECT 隧道（用于 https 目标）。
pub(crate) async fn http_connect(stream: &mut TcpStream, host: &str, port: u16) -> Result<()> {
    let req = format!(
        "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await?;
    stream.flush().await?;
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut tmp = [0u8; 512];
    let head_end = loop {
        if let Some(p) = find_header_end(&buf) {
            break p;
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err(CoreError::Protocol("HTTP 代理未返回 CONNECT 应答".into()));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 64 * 1024 {
            return Err(CoreError::Protocol("HTTP 代理 CONNECT 应答过大".into()));
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| CoreError::Protocol("HTTP 代理返回非法状态行".into()))?;
    if status != 200 {
        return Err(CoreError::Network(format!(
            "HTTP 代理 CONNECT 被拒绝（HTTP {}）",
            status
        )));
    }
    Ok(())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    (0..=buf.len() - 4).find(|&i| &buf[i..i + 4] == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_url_parse_http() {
        let p = ProxyUrl::parse("http://127.0.0.1:7890").unwrap();
        assert_eq!(p.scheme, ProxyScheme::Http);
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 7890);
    }

    #[test]
    fn proxy_url_parse_socks5() {
        let p = ProxyUrl::parse("socks5://localhost:1080/").unwrap();
        assert_eq!(p.scheme, ProxyScheme::Socks5);
        assert_eq!(p.host, "localhost");
        assert_eq!(p.port, 1080);
        // socks5h 等效 socks5
        let p = ProxyUrl::parse("SOCKS5H://192.168.1.1:1080").unwrap();
        assert_eq!(p.scheme, ProxyScheme::Socks5);
    }

    #[test]
    fn proxy_url_parse_rejects_bad() {
        assert!(ProxyUrl::parse("127.0.0.1:7890").is_err()); // 缺协议头
        assert!(ProxyUrl::parse("ftp://1.2.3.4:21").is_err()); // 不支持的协议
        assert!(ProxyUrl::parse("http://").is_err()); // 缺主机
    }

    #[test]
    fn proxy_url_parse_tolerates_userinfo_and_trailing() {
        // 容忍带认证段与结尾斜杠（当前不支持认证，仅解析主机端口）
        let p = ProxyUrl::parse("http://user:pass@127.0.0.1:7897/path?x=1").unwrap();
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 7897);
    }

    #[test]
    fn proxy_url_display_roundtrip() {
        let p = ProxyUrl::parse("http://127.0.0.1:7890").unwrap();
        assert_eq!(p.to_string(), "http://127.0.0.1:7890");
    }

    #[test]
    fn resolve_proxy_direct() {
        // Direct 模式永远直连
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let r = rt.block_on(resolve_proxy(&ProxyMode::Direct)).unwrap();
        assert!(r.is_none());
    }
}
