//! DNS 四种传输实现：UDP / TCP / DoT / DoH。
//!
//! **核心要求（任务书第 3 节）**：禁止用 ping 代表 DNS 性能。
//! DoH 必须完整走 `TCP Connect → TLS Handshake → HTTPS Request → DNS Response`，
//! 并分段记录 `connect_time / tls_time / query_time / total_time`。
//!
//! DoH 采用自研 HTTP/1.1 over rustls，而非 `reqwest`，原因见 `doc/01-系统架构.md` 第 3 节：
//! reqwest 不暴露分段耗时，且依赖过重。

use crate::dns::wire::{self, DnsMessage};
use crate::error::{CoreError, Result};
use crate::models::{DnsProtocol, DnsServer};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

/// 分段耗时（毫秒）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryTimings {
    /// TCP 建连耗时（UDP 为 0）
    pub connect_ms: f64,
    /// TLS 握手耗时（UDP/TCP 为 0）
    pub tls_ms: f64,
    /// 请求写出 → 首字节
    pub request_ms: f64,
    /// 请求发出 → 响应完整解析完成
    pub query_ms: f64,
    /// 端到端合计
    pub total_ms: f64,
}

/// 一次查询的完整结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsQueryOutcome {
    pub message: DnsMessage,
    pub timings: QueryTimings,
    /// 响应字节数
    pub wire_len: usize,
    /// 实际使用的传输（可能因 UDP 截断回退而变成 TCP）
    pub transport: DnsProtocol,
}

/// 查询选项。
#[derive(Debug, Clone, Copy)]
pub struct QueryOptions {
    pub timeout: Duration,
    /// 是否附加 EDNS0 OPT
    pub edns: bool,
    /// UDP 响应被截断（TC=1）时是否自动回退 TCP
    pub tcp_fallback: bool,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            edns: true,
            tcp_fallback: true,
        }
    }
}

impl QueryOptions {
    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }
}

/// DNS 客户端。可跨任务共享（内部无状态，TLS 配置用 Arc 复用）。
pub struct DnsClient {
    tls_config: Arc<rustls::ClientConfig>,
}

impl std::fmt::Debug for DnsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DnsClient").finish_non_exhaustive()
    }
}

impl Default for DnsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DnsClient {
    pub fn new() -> Self {
        Self {
            tls_config: Arc::new(build_tls_config()),
        }
    }

    /// 统一入口：按 `server.protocol` 分派。
    pub async fn query(
        &self,
        server: &DnsServer,
        domain: &str,
        qtype: u16,
        opts: &QueryOptions,
    ) -> Result<DnsQueryOutcome> {
        let id = random_id();
        let msg = wire::encode_query(id, domain, qtype, opts.edns)?;

        match server.protocol {
            DnsProtocol::Udp => {
                let addr = resolve_server_addr(server, DnsProtocol::Udp).await?;
                let out = self.query_udp(addr, &msg, opts).await?;
                if out.message.truncated && opts.tcp_fallback {
                    // 截断 → 回退 TCP，并在结果中标注实际传输
                    let mut tcp_out = self.query_tcp(addr, &msg, opts).await?;
                    tcp_out.transport = DnsProtocol::Tcp;
                    return Ok(tcp_out);
                }
                Ok(out)
            }
            DnsProtocol::Tcp => {
                let addr = resolve_server_addr(server, DnsProtocol::Tcp).await?;
                self.query_tcp(addr, &msg, opts).await
            }
            DnsProtocol::Dot => {
                let sni = server
                    .host
                    .clone()
                    .unwrap_or_else(|| host_of_address(&server.address));
                let addrs = candidate_addrs(server, &sni, DnsProtocol::Dot.default_port()).await;
                if addrs.is_empty() {
                    return Err(CoreError::Network(format!("无法解析 DoT 服务器 {}", sni)));
                }
                let mut last: Option<CoreError> = None;
                for addr in addrs {
                    match self.query_dot(addr, &sni, &msg, opts).await {
                        Ok(o) => return Ok(o),
                        Err(e) => {
                            tracing::debug!("DoT {} ({}) 失败: {}", server.name, addr, e);
                            last = Some(e);
                        }
                    }
                }
                Err(last.unwrap_or_else(|| CoreError::Network("DoT 无可用地址".into())))
            }
            DnsProtocol::Doh => {
                let ep = parse_doh_url(&server.address)?;
                // 优先用 bootstrap_ip，失败后回退到系统解析结果：
                // 有些节点的 bootstrap IP 并不提供 HTTPS 服务（实测 doh.pub + 119.29.29.29）。
                let addrs = candidate_addrs(server, &ep.host, ep.port).await;
                if addrs.is_empty() {
                    return Err(CoreError::Network(format!("无法解析 DoH 服务器 {}", ep.host)));
                }
                let mut last: Option<CoreError> = None;
                for addr in addrs {
                    match self.query_doh(&ep, addr, &msg, opts).await {
                        Ok(o) => return Ok(o),
                        Err(e) => {
                            tracing::debug!("DoH {} ({}) 失败: {}", server.name, addr, e);
                            last = Some(e);
                        }
                    }
                }
                Err(last.unwrap_or_else(|| CoreError::Network("DoH 无可用地址".into())))
            }
        }
    }

    /// 明文 UDP。
    pub async fn query_udp(
        &self,
        addr: SocketAddr,
        msg: &[u8],
        opts: &QueryOptions,
    ) -> Result<DnsQueryOutcome> {
        let started = Instant::now();
        let fut = async {
            let bind: SocketAddr = if addr.is_ipv4() {
                "0.0.0.0:0".parse().unwrap()
            } else {
                "[::]:0".parse().unwrap()
            };
            let sock = UdpSocket::bind(bind).await?;
            sock.connect(addr).await?;
            let connect_ms = started.elapsed().as_secs_f64() * 1000.0;

            let send_at = Instant::now();
            sock.send(msg).await?;

            // UDP 可能收到伪造包：校验 id 与来源（已 connect，来源已被内核过滤），不匹配则继续等
            let mut buf = vec![0u8; 4096];
            let expected_id = wire::read_id(msg);
            loop {
                let n = sock.recv(&mut buf).await?;
                if n < 12 {
                    continue;
                }
                let got = u16::from_be_bytes([buf[0], buf[1]]);
                if got != expected_id {
                    continue;
                }
                let query_ms = send_at.elapsed().as_secs_f64() * 1000.0;
                let parsed = wire::parse_message(&buf[..n])?;
                let total_ms = started.elapsed().as_secs_f64() * 1000.0;
                return Ok(DnsQueryOutcome {
                    message: parsed,
                    timings: QueryTimings {
                        connect_ms,
                        tls_ms: 0.0,
                        request_ms: query_ms,
                        query_ms,
                        total_ms,
                    },
                    wire_len: n,
                    transport: DnsProtocol::Udp,
                });
            }
        };
        match timeout(opts.timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(CoreError::Timeout(opts.timeout)),
        }
    }

    /// 明文 TCP（2 字节长度前缀）。
    pub async fn query_tcp(
        &self,
        addr: SocketAddr,
        msg: &[u8],
        opts: &QueryOptions,
    ) -> Result<DnsQueryOutcome> {
        let fut = async {
            let started = Instant::now();
            let mut stream = TcpStream::connect(addr).await?;
            stream.set_nodelay(true).ok();
            let connect_ms = started.elapsed().as_secs_f64() * 1000.0;

            let send_at = Instant::now();
            let mut framed = Vec::with_capacity(msg.len() + 2);
            framed.extend_from_slice(&(msg.len() as u16).to_be_bytes());
            framed.extend_from_slice(msg);
            stream.write_all(&framed).await?;
            stream.flush().await?;

            let len = stream.read_u16().await? as usize;
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).await?;
            let query_ms = send_at.elapsed().as_secs_f64() * 1000.0;

            let parsed = wire::parse_message(&body)?;
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            Ok(DnsQueryOutcome {
                message: parsed,
                timings: QueryTimings {
                    connect_ms,
                    tls_ms: 0.0,
                    request_ms: query_ms,
                    query_ms,
                    total_ms,
                },
                wire_len: len + 2,
                transport: DnsProtocol::Tcp,
            })
        };
        match timeout(opts.timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(CoreError::Timeout(opts.timeout)),
        }
    }

    /// DNS over TLS。
    pub async fn query_dot(
        &self,
        addr: SocketAddr,
        sni: &str,
        msg: &[u8],
        opts: &QueryOptions,
    ) -> Result<DnsQueryOutcome> {
        let fut = async {
            let started = Instant::now();
            let stream = TcpStream::connect(addr).await?;
            stream.set_nodelay(true).ok();
            let connect_ms = started.elapsed().as_secs_f64() * 1000.0;

            let tls_start = Instant::now();
            let connector = tokio_rustls::TlsConnector::from(self.tls_config.clone());
            let server_name = to_server_name(sni)?;
            let mut tls = connector
                .connect(server_name, stream)
                .await
                .map_err(|e| CoreError::Tls(format!("DoT 握手失败: {}", e)))?;
            let tls_ms = tls_start.elapsed().as_secs_f64() * 1000.0;

            let send_at = Instant::now();
            let mut framed = Vec::with_capacity(msg.len() + 2);
            framed.extend_from_slice(&(msg.len() as u16).to_be_bytes());
            framed.extend_from_slice(msg);
            tls.write_all(&framed).await?;
            tls.flush().await?;

            let len = tls.read_u16().await? as usize;
            let mut body = vec![0u8; len];
            tls.read_exact(&mut body).await?;
            let query_ms = send_at.elapsed().as_secs_f64() * 1000.0;

            let parsed = wire::parse_message(&body)?;
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            Ok(DnsQueryOutcome {
                message: parsed,
                timings: QueryTimings {
                    connect_ms,
                    tls_ms,
                    request_ms: query_ms,
                    query_ms,
                    total_ms,
                },
                wire_len: len + 2,
                transport: DnsProtocol::Dot,
            })
        };
        match timeout(opts.timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(CoreError::Timeout(opts.timeout)),
        }
    }

    /// DNS over HTTPS（HTTP/1.1，POST application/dns-message）。
    pub async fn query_doh(
        &self,
        ep: &DohEndpoint,
        addr: SocketAddr,
        msg: &[u8],
        opts: &QueryOptions,
    ) -> Result<DnsQueryOutcome> {
        let fut = async {
            let started = Instant::now();
            let stream = TcpStream::connect(addr).await?;
            stream.set_nodelay(true).ok();
            let connect_ms = started.elapsed().as_secs_f64() * 1000.0;

            let tls_start = Instant::now();
            let connector = tokio_rustls::TlsConnector::from(self.tls_config.clone());
            let server_name = to_server_name(&ep.host)?;
            let mut tls = connector
                .connect(server_name, stream)
                .await
                .map_err(|e| CoreError::Tls(format!("DoH 握手失败: {}", e)))?;
            let tls_ms = tls_start.elapsed().as_secs_f64() * 1000.0;

            // 构造 HTTP/1.1 请求
            let mut req = Vec::with_capacity(msg.len() + 256);
            req.extend_from_slice(format!("POST {} HTTP/1.1\r\n", ep.path).as_bytes());
            let host_header = if ep.port == 443 {
                ep.host.clone()
            } else {
                format!("{}:{}", ep.host, ep.port)
            };
            req.extend_from_slice(format!("Host: {}\r\n", host_header).as_bytes());
            req.extend_from_slice(b"Accept: application/dns-message\r\n");
            req.extend_from_slice(b"Content-Type: application/dns-message\r\n");
            req.extend_from_slice(format!("Content-Length: {}\r\n", msg.len()).as_bytes());
            req.extend_from_slice(b"User-Agent: NoFakeIP\r\n");
            req.extend_from_slice(b"Connection: close\r\n\r\n");
            req.extend_from_slice(msg);

            let send_at = Instant::now();
            tls.write_all(&req).await?;
            tls.flush().await?;
            let first_byte_at = Instant::now();
            let resp = read_http_response(&mut tls).await?;
            let query_ms = send_at.elapsed().as_secs_f64() * 1000.0;
            let request_ms = (first_byte_at - send_at).as_secs_f64() * 1000.0;

            if resp.status != 200 {
                return Err(CoreError::Protocol(format!(
                    "DoH 返回 HTTP {}",
                    resp.status
                )));
            }
            let parsed = wire::parse_message(&resp.body)?;
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            Ok(DnsQueryOutcome {
                message: parsed,
                timings: QueryTimings {
                    connect_ms,
                    tls_ms,
                    request_ms,
                    query_ms,
                    total_ms,
                },
                wire_len: resp.body.len(),
                transport: DnsProtocol::Doh,
            })
        };
        match timeout(opts.timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(CoreError::Timeout(opts.timeout)),
        }
    }
}

// —————————————————————— 辅助实现 ——————————————————————

fn random_id() -> u16 {
    use rand::Rng;
    rand::thread_rng().gen_range(1u16..=u16::MAX)
}

fn build_tls_config() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring provider 支持默认协议版本")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

fn to_server_name(host: &str) -> Result<rustls::pki_types::ServerName<'static>> {
    rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| CoreError::Tls(format!("非法服务器名 {}: {}", host, e)))
}

/// 从 `1.1.1.1:853` / `dns.example.com:853` 中取主机部分。
fn host_of_address(address: &str) -> String {
    if let Ok(sa) = address.parse::<SocketAddr>() {
        return sa.ip().to_string();
    }
    address
        .rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_else(|| address.to_string())
}

/// 解析 DNS 服务器自身的地址。
///
/// 优先使用 `bootstrap_ip`（避免「用域名解析域名」的鸡生蛋问题），
/// 否则用系统解析器。
pub async fn resolve_server_addr(server: &DnsServer, proto: DnsProtocol) -> Result<SocketAddr> {
    let port = proto.default_port();

    if let Some(boot) = server.bootstrap_ip.as_deref() {
        if let Ok(ip) = boot.parse::<std::net::IpAddr>() {
            return Ok(SocketAddr::new(ip, port));
        }
    }

    let raw = host_of_address(&server.address);
    let host = raw.trim_start_matches('[').trim_end_matches(']');

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }

    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| CoreError::Network(format!("解析 {} 失败: {}", host, e)))?;
    addrs
        .next()
        .ok_or_else(|| CoreError::Network(format!("{} 无可用地址", host)))
}

/// DoH 端点。
#[derive(Debug, Clone, PartialEq)]
pub struct DohEndpoint {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// 解析 DoH URL。支持 `https://host/dns-query`、`https://host:8443/x`。
pub fn parse_doh_url(url: &str) -> Result<DohEndpoint> {
    let trimmed = url.trim();
    let (scheme, rest) = trimmed
        .split_once("://")
        .ok_or_else(|| CoreError::Config(format!("DoH 地址缺少协议头: {}", url)))?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return Err(CoreError::Config(format!("DoH 仅支持 http/https: {}", url)));
    }
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/dns-query"),
    };
    if hostport.is_empty() {
        return Err(CoreError::Config(format!("DoH 地址缺少主机: {}", url)));
    }
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| CoreError::Config(format!("非法端口: {}", p)))?,
        ),
        _ => (
            hostport.to_string(),
            if scheme.eq_ignore_ascii_case("http") { 80 } else { 443 },
        ),
    };
    let path = if path.is_empty() { "/dns-query" } else { path };
    Ok(DohEndpoint {
        scheme: scheme.to_ascii_lowercase(),
        host,
        port,
        path: path.to_string(),
    })
}

/// 解析 DoH 服务器地址（优先 bootstrap_ip）。
pub async fn resolve_doh_addr(ep: &DohEndpoint, server: &DnsServer) -> Result<SocketAddr> {
    if let Some(boot) = server.bootstrap_ip.as_deref() {
        if let Ok(ip) = boot.parse::<std::net::IpAddr>() {
            return Ok(SocketAddr::new(ip, ep.port));
        }
    }
    if let Ok(ip) = ep.host.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, ep.port));
    }
    let mut addrs = tokio::net::lookup_host((ep.host.as_str(), ep.port))
        .await
        .map_err(|e| CoreError::Network(format!("解析 {} 失败: {}", ep.host, e)))?;
    addrs
        .next()
        .ok_or_else(|| CoreError::Network(format!("{} 无可用地址", ep.host)))
}

/// 候选连接地址（最多 3 个）：**先 bootstrap_ip，再系统解析**。
///
/// 为什么要回退：部分节点的 `bootstrap_ip` 只用于解析、并不提供 HTTPS/DoT 服务
/// （实测 `doh.pub` + `119.29.29.29` 会 TLS 失败），因此失败后必须能换地址重试。
/// 反过来，保留 bootstrap 优先是为了应对「本机 DNS 被 Fake-IP 劫持、系统解析不可信」的场景。
pub async fn candidate_addrs(server: &DnsServer, host: &str, port: u16) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();

    if let Some(boot) = server.bootstrap_ip.as_deref() {
        if let Ok(ip) = boot.parse::<std::net::IpAddr>() {
            out.push(SocketAddr::new(ip, port));
        }
    }

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        let sa = SocketAddr::new(ip, port);
        if !out.contains(&sa) {
            out.push(sa);
        }
    } else if let Ok(addrs) = tokio::net::lookup_host((host, port)).await {
        for a in addrs.take(3) {
            if !out.contains(&a) {
                out.push(a);
            }
        }
    }

    out.truncate(3);
    out
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// 读取一个 HTTP/1.1 响应，支持 Content-Length 与 chunked。
async fn read_http_response<S: AsyncReadExt + Unpin>(stream: &mut S) -> Result<HttpResponse> {
    const MAX_BYTES: usize = 256 * 1024;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut header_end: Option<usize> = None;
    let mut status = 0u16;
    let mut content_length: Option<usize> = None;
    let mut chunked = false;

    let mut tmp = [0u8; 8192];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);

        if header_end.is_none() {
            if let Some(p) = find_subslice(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..p]).into_owned();
                let mut lines = head.lines();
                let status_line = lines.next().unwrap_or_default();
                status = status_line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u16>().ok())
                    .ok_or_else(|| {
                        CoreError::Protocol(format!("非法 HTTP 状态行: {}", status_line))
                    })?;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        let k = k.trim().to_ascii_lowercase();
                        let v = v.trim().to_ascii_lowercase();
                        match k.as_str() {
                            "content-length" => content_length = v.parse::<usize>().ok(),
                            "transfer-encoding" => chunked = v.contains("chunked"),
                            _ => {}
                        }
                    }
                }
                header_end = Some(p + 4);
            }
        }

        if let Some(he) = header_end {
            let body = &buf[he..];
            if chunked {
                if let Some(decoded) = decode_chunked(body)? {
                    return Ok(HttpResponse {
                        status,
                        body: decoded,
                    });
                }
            } else if let Some(cl) = content_length {
                if body.len() >= cl {
                    return Ok(HttpResponse {
                        status,
                        body: body[..cl].to_vec(),
                    });
                }
            }
            // 无 Content-Length 且非 chunked：依赖 Connection: close，读到 EOF
        }

        if buf.len() > MAX_BYTES {
            return Err(CoreError::Protocol("HTTP 响应过大".into()));
        }
    }

    match header_end {
        Some(he) => Ok(HttpResponse {
            status,
            body: buf[he..].to_vec(),
        }),
        None => Err(CoreError::Protocol("HTTP 响应缺少头部".into())),
    }
}

/// chunked 解码。数据不完整时返回 `Ok(None)`（需要继续读）。
fn decode_chunked(body: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        // 读取 chunk size 行
        let line_end = match find_subslice(&body[pos..], b"\r\n") {
            Some(i) => pos + i,
            None => return Ok(None),
        };
        let size_line = String::from_utf8_lossy(&body[pos..line_end]);
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| CoreError::Protocol(format!("非法 chunk 长度: {}", size_hex)))?;
        pos = line_end + 2;
        if size == 0 {
            return Ok(Some(out));
        }
        if pos + size + 2 > body.len() {
            return Ok(None);
        }
        out.extend_from_slice(&body[pos..pos + size]);
        pos += size + 2; // 跳过数据与结尾 CRLF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_doh_url_variants() {
        let a = parse_doh_url("https://dns.alidns.com/dns-query").unwrap();
        assert_eq!(a.host, "dns.alidns.com");
        assert_eq!(a.port, 443);
        assert_eq!(a.path, "/dns-query");

        let b = parse_doh_url("https://1.1.1.1").unwrap();
        assert_eq!(b.host, "1.1.1.1");
        assert_eq!(b.path, "/dns-query");

        let c = parse_doh_url("https://dns.example.com:8443/custom").unwrap();
        assert_eq!(c.port, 8443);
        assert_eq!(c.path, "/custom");

        assert!(parse_doh_url("ftp://x/y").is_err());
        assert!(parse_doh_url("no-scheme").is_err());
    }

    #[test]
    fn host_extraction() {
        assert_eq!(host_of_address("1.1.1.1:853"), "1.1.1.1");
        assert_eq!(host_of_address("dns.google:443"), "dns.google");
        assert_eq!(host_of_address("dns.google"), "dns.google");
    }

    #[test]
    fn decode_chunked_works() {
        let raw = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(raw).unwrap().unwrap(), b"hello world");
    }

    #[test]
    fn decode_chunked_incomplete() {
        let raw = b"5\r\nhel";
        assert!(decode_chunked(raw).unwrap().is_none());
    }

    #[test]
    fn find_subslice_basic() {
        assert_eq!(find_subslice(b"abcdef", b"cd"), Some(2));
        assert_eq!(find_subslice(b"abcdef", b"zz"), None);
    }

    #[test]
    fn timings_default_is_zero() {
        let t = QueryTimings::default();
        assert_eq!(t.total_ms, 0.0);
    }
}
