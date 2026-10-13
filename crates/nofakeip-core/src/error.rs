//! 统一错误模型。
//!
//! 设计约定（见 `doc/02-核心引擎设计.md` 第 2 节）：
//! - 单个 DNS 服务器查询失败**不是** `Err`，而是结构化结果中的 `ok=false`；
//! - `Err` 只表示「整个操作无法继续」（参数非法、数据库打不开、资源缺失等）。

use std::time::Duration;

/// 核心引擎错误类型。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("网络错误: {0}")]
    Network(String),

    #[error("DNS 协议错误: {0}")]
    Protocol(String),

    #[error("DNS 报文解析失败: {0}")]
    Malformed(String),

    #[error("查询超时 ({0:?})")]
    Timeout(Duration),

    #[error("DNS 返回错误码: {0}")]
    Rcode(String),

    #[error("无效配置: {0}")]
    Config(String),

    #[error("数据库错误: {0}")]
    Storage(String),

    #[error("资源缺失: {0}")]
    NotFound(String),

    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误: {0}")]
    Serde(String),

    #[error("TLS 错误: {0}")]
    Tls(String),

    #[error("无效域名: {0}")]
    InvalidDomain(String),
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::Serde(e.to_string())
    }
}

impl From<serde_yaml::Error> for CoreError {
    fn from(e: serde_yaml::Error) -> Self {
        CoreError::Serde(e.to_string())
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Storage(e.to_string())
    }
}

impl From<csv::Error> for CoreError {
    fn from(e: csv::Error) -> Self {
        CoreError::Storage(e.to_string())
    }
}

impl From<ipnet::AddrParseError> for CoreError {
    fn from(e: ipnet::AddrParseError) -> Self {
        CoreError::Config(e.to_string())
    }
}

impl From<std::net::AddrParseError> for CoreError {
    fn from(e: std::net::AddrParseError) -> Self {
        CoreError::Network(e.to_string())
    }
}

impl From<CoreError> for String {
    fn from(e: CoreError) -> Self {
        e.to_string()
    }
}

/// 核心引擎统一返回类型。
pub type Result<T> = std::result::Result<T, CoreError>;
