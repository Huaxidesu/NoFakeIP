//! 极简 HTTP/1.1 客户端（GET + 重定向），基于 tokio + rustls。
//!
//! 用途：拉取规则 / GeoIP 数据文件（见 [`crate::geodata`]）。
//! 之所以不引 `reqwest`：保持依赖精简，且 Android 上行为一致（统一用 webpki-roots）。

use crate::error::{CoreError, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

pub mod proxy;
pub use proxy::{detect_proxy_url, resolve_proxy, ProxyMode, ProxyScheme, ProxyUrl, PROBE_PORTS};

/// 可对象化的读写流（明文 TCP 或 TLS）。`Send` 约束使返回的 Future 可跨线程（Tauri async command 要求）。
trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncReadWrite for T {}

/// 简单 URL 解析结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// 含前导 `/`，含 query
    pub path: String,
}

impl Url {
    pub fn parse(raw: &str) -> Result<Self> {
        let s = raw.trim();
        let (scheme, rest) = s
            .split_once("://")
            .ok_or_else(|| CoreError::Config(format!("URL 缺少协议头: {}", raw)))?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(CoreError::Config(format!("仅支持 http/https: {}", raw)));
        }
        let (hostport, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if hostport.is_empty() {
            return Err(CoreError::Config(format!("URL 缺少主机: {}", raw)));
        }
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (
                h.to_string(),
                p.parse::<u16>()
                    .map_err(|_| CoreError::Config(format!("非法端口: {}", p)))?,
            ),
            _ => (
                hostport.to_string(),
                if scheme == "http" { 80 } else { 443 },
            ),
        };
        Ok(Url {
            scheme,
            host,
            port,
            path: if path.is_empty() {
                "/".to_string()
            } else {
                path.to_string()
            },
        })
    }

    /// 基于当前 URL 解析相对/绝对重定向目标。
    pub fn resolve(&self, location: &str) -> Result<Url> {
        let loc = location.trim();
        if loc.contains("://") {
            return Url::parse(loc);
        }
        if let Some(rest) = loc.strip_prefix("//") {
            return Url::parse(&format!("{}://{}", self.scheme, rest));
        }
        if loc.starts_with('/') {
            return Url::parse(&format!(
                "{}://{}:{}{}",
                self.scheme, self.host, self.port, loc
            ));
        }
        // 相对路径
        let base = match self.path.rfind('/') {
            Some(i) => &self.path[..=i],
            None => "/",
        };
        Ok(Url {
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            port: self.port,
            path: format!("{}{}", base, loc),
        })
    }

    pub fn origin(&self) -> String {
        format!("{}://{}", self.scheme, self.host)
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if (self.scheme == "http" && self.port == 80) || (self.scheme == "https" && self.port == 443)
        {
            write!(f, "{}://{}{}", self.scheme, self.host, self.path)
        } else {
            write!(f, "{}://{}:{}{}", self.scheme, self.host, self.port, self.path)
        }
    }
}

/// HTTP 响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub final_url: String,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

impl HttpResponse {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// HTTP 客户端。
#[derive(Clone)]
pub struct HttpClient {
    tls: Arc<rustls::ClientConfig>,
    timeout: Duration,
    max_redirects: usize,
    user_agent: String,
    /// 可选出站代理（None = 直连）
    proxy: Option<ProxyUrl>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("timeout", &self.timeout)
            .field("proxy", &self.proxy)
            .finish_non_exhaustive()
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> Self {
        Self {
            tls: Arc::new(build_tls_config()),
            timeout: Duration::from_secs(30),
            max_redirects: 5,
            user_agent: format!("NoFakeIP/{}", crate::VERSION),
            proxy: None,
        }
    }

    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// 设置出站代理（None = 直连）。
    pub fn with_proxy(mut self, proxy: Option<ProxyUrl>) -> Self {
        self.proxy = proxy;
        self
    }

    /// GET 请求，自动跟随重定向。
    pub async fn get(&self, url: &str) -> Result<HttpResponse> {
        let mut current = Url::parse(url)?;
        for _ in 0..=self.max_redirects {
            let resp = self.get_once(&current).await?;
            if (300..400).contains(&resp.status) {
                let location = resp
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                    .map(|(_, v)| v.clone());
                match location {
                    Some(loc) => {
                        current = current.resolve(&loc)?;
                        continue;
                    }
                    None => return Err(CoreError::Protocol("重定向缺少 Location".into())),
                }
            }
            return Ok(HttpResponse {
                status: resp.status,
                final_url: current.to_string(),
                body: resp.body,
                content_type: resp.content_type,
            });
        }
        Err(CoreError::Protocol("重定向次数过多".into()))
    }

    /// GET 并返回文本。
    pub async fn get_text(&self, url: &str) -> Result<String> {
        let r = self.get(url).await?;
        if r.status != 200 {
            return Err(CoreError::Protocol(format!(
                "{} 返回 HTTP {}",
                url, r.status
            )));
        }
        Ok(r.text())
    }

    async fn get_once(&self, url: &Url) -> Result<RawResponse> {
        let fut = async {
            let mut stream = self.connect(url).await?;
            // 经 HTTP 代理访问 http 目标时使用绝对 URI；其余情况沿用原路径
            let target = match (&self.proxy, url.scheme.as_str()) {
                (Some(p), "http") if p.scheme == ProxyScheme::Http => {
                    format!("http://{}:{}{}", url.host, url.port, url.path)
                }
                _ => url.path.clone(),
            };
            let request = format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
                target,
                if url.port == 80 || url.port == 443 {
                    url.host.clone()
                } else {
                    format!("{}:{}", url.host, url.port)
                },
                self.user_agent
            );

            if url.scheme == "https" {
                let connector = tokio_rustls::TlsConnector::from(self.tls.clone());
                let sni = rustls::pki_types::ServerName::try_from(url.host.clone())
                    .map_err(|e| CoreError::Tls(format!("非法主机名 {}: {}", url.host, e)))?;
                let mut tls = connector
                    .connect(sni, stream)
                    .await
                    .map_err(|e| CoreError::Tls(format!("TLS 握手失败: {}", e)))?;
                tls.write_all(request.as_bytes()).await?;
                tls.flush().await?;
                read_response(&mut tls).await
            } else {
                stream.write_all(request.as_bytes()).await?;
                stream.flush().await?;
                read_response(&mut stream).await
            }
        };
        match timeout(self.timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(CoreError::Timeout(self.timeout)),
        }
    }

    /// 建立到目标的 TCP 连接（直连，或经代理：HTTP CONNECT / SOCKS5）。
    ///
    /// 走代理时把目标主机名原样交给代理解析（HTTP CONNECT 天然如此，
    /// SOCKS5 使用 ATYP=3，等效 socks5h）。
    async fn connect(&self, url: &Url) -> Result<Box<dyn AsyncReadWrite>> {
        let p = match &self.proxy {
            None => {
                let addr = resolve(url).await?;
                let s = TcpStream::connect(addr).await?;
                s.set_nodelay(true).ok();
                return Ok(Box::new(s));
            }
            Some(p) => p,
        };
        let mut s = TcpStream::connect(resolve_host(&p.host, p.port).await?).await?;
        s.set_nodelay(true).ok();
        match p.scheme {
            ProxyScheme::Http => {
                if url.scheme == "https" {
                    proxy::http_connect(&mut s, &url.host, url.port).await?;
                }
                Ok(Box::new(s))
            }
            ProxyScheme::Socks5 => {
                proxy::socks5_connect(&mut s, &url.host, url.port).await?;
                Ok(Box::new(s))
            }
        }
    }
}

async fn resolve(url: &Url) -> Result<std::net::SocketAddr> {
    resolve_host(&url.host, url.port).await
}

async fn resolve_host(host: &str, port: u16) -> Result<std::net::SocketAddr> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(std::net::SocketAddr::new(ip, port));
    }
    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| CoreError::Network(format!("解析 {} 失败: {}", host, e)))?;
    addrs
        .next()
        .ok_or_else(|| CoreError::Network(format!("{} 无可用地址", host)))
}

struct RawResponse {
    status: u16,
    headers: Vec<(String, String)>,
    content_type: Option<String>,
    body: Vec<u8>,
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

async fn read_response<S: AsyncReadExt + Unpin>(stream: &mut S) -> Result<RawResponse> {
    const MAX_BYTES: usize = 64 * 1024 * 1024; // 64 MB，GeoIP 数据文件可能较大
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut tmp = vec![0u8; 32 * 1024];
    let mut header_end: Option<usize> = None;
    let mut status = 0u16;
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    let mut content_type = None;

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
                        let k = k.trim().to_string();
                        let v = v.trim().to_string();
                        let lk = k.to_ascii_lowercase();
                        if lk == "content-length" {
                            content_length = v.parse::<usize>().ok();
                        } else if lk == "transfer-encoding" {
                            chunked = v.to_ascii_lowercase().contains("chunked");
                        } else if lk == "content-type" {
                            content_type = Some(v.clone());
                        }
                        headers.push((k, v));
                    }
                }
                header_end = Some(p + 4);
            }
        }

        if let Some(he) = header_end {
            let body = &buf[he..];
            if chunked {
                if let Some(decoded) = decode_chunked(body)? {
                    return Ok(RawResponse {
                        status,
                        headers,
                        content_type,
                        body: decoded,
                    });
                }
            } else if let Some(cl) = content_length {
                if body.len() >= cl {
                    return Ok(RawResponse {
                        status,
                        headers,
                        content_type,
                        body: body[..cl].to_vec(),
                    });
                }
            }
        }

        if buf.len() > MAX_BYTES {
            return Err(CoreError::Protocol("响应体过大".into()));
        }
    }

    match header_end {
        Some(he) => Ok(RawResponse {
            status,
            headers,
            content_type,
            body: buf[he..].to_vec(),
        }),
        None => Err(CoreError::Protocol("响应缺少 HTTP 头部".into())),
    }
}

fn decode_chunked(body: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
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
        pos += size + 2;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_parse_full() {
        let u = Url::parse("https://raw.githubusercontent.com/a/b/master/c.txt").unwrap();
        assert_eq!(u.scheme, "https");
        assert_eq!(u.host, "raw.githubusercontent.com");
        assert_eq!(u.port, 443);
        assert_eq!(u.path, "/a/b/master/c.txt");
    }

    #[test]
    fn url_parse_with_port_and_query() {
        let u = Url::parse("http://example.com:8080/p?a=1").unwrap();
        assert_eq!(u.port, 8080);
        assert_eq!(u.path, "/p?a=1");
    }

    #[test]
    fn url_parse_no_path() {
        let u = Url::parse("https://example.com").unwrap();
        assert_eq!(u.path, "/");
    }

    #[test]
    fn url_parse_rejects_ftp() {
        assert!(Url::parse("ftp://example.com/x").is_err());
        assert!(Url::parse("example.com/x").is_err());
    }

    #[test]
    fn url_resolve_redirects() {
        let base = Url::parse("https://a.com/x/y.txt").unwrap();
        assert_eq!(base.resolve("https://b.com/z").unwrap().host, "b.com");
        assert_eq!(base.resolve("/root").unwrap().path, "/root");
        assert_eq!(base.resolve("z.txt").unwrap().path, "/x/z.txt");
        assert_eq!(base.resolve("//cdn.com/p").unwrap().host, "cdn.com");
    }

    #[test]
    fn chunked_decode() {
        assert_eq!(
            decode_chunked(b"4\r\nabcd\r\n0\r\n\r\n").unwrap().unwrap(),
            b"abcd"
        );
        assert!(decode_chunked(b"4\r\nab").unwrap().is_none());
    }
}
