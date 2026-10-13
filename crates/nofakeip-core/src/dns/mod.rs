//! DNS 协议栈与服务器管理。
//!
//! - [`wire`]：DNS 报文编解码（自研）
//! - [`transport`]：UDP / TCP / DoT / DoH 四传输与分段计时
//! - [`server`]：内置节点、用户输入解析、导入导出

pub mod server;
pub mod transport;
pub mod wire;

pub use server::{ServerRegistry, BuiltinEntry};
pub use transport::{
    parse_doh_url, DnsClient, DnsQueryOutcome, DohEndpoint, QueryOptions, QueryTimings,
};
pub use wire::{DnsMessage, Question, RData, Record};
