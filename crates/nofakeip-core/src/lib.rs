//! NoFakeIP Core Engine
//!
//! 面向 OpenClash / Clash Meta Fake-IP 用户的智能 DNS 诊断引擎。
//!
//! 设计约束（见《NoFakeIP 开发任务书》第一节）：
//! - 核心不依赖任何 GUI 框架；
//! - 所有核心逻辑使用 Rust 实现；
//! - 可被 Desktop / Android / CLI / 未来插件复用。
//!
//! 模块总览：
//! - [`dns`]        DNS 协议栈（UDP / TCP / DoT / DoH）与服务器管理
//! - [`benchmark`]  DNS 性能测试（单域名 / 批量）
//! - [`classifier`] 国内外域名自动识别
//! - [`pollution`]  DNS 污染与 Fake-IP 异常检测
//! - [`geo`]        IP 归属地 / ASN 判定
//! - [`recommend`]  DNS 智能推荐
//! - [`report`]     诊断报告生成与导出（JSON / CSV / Markdown）
//! - [`geodata`]    Geo / 域名规则数据源与在线更新
//! - [`net`]        极简 HTTP 客户端（数据更新用）
//! - [`storage`]    SQLite 持久化
//! - [`models`]     跨模块共享数据模型
//!
//! **范围说明**：本工具**不生成、不修改**任何 Clash / OpenClash / Clash Verge 配置。
//! 它只负责「测出来、查清楚、给出建议」，配置怎么写由用户自己决定。

pub mod benchmark;
pub mod classifier;
pub mod data;
pub mod diagnose;
pub mod dns;
pub mod error;
pub mod geo;
pub mod geodata;
pub mod models;
pub mod net;
pub mod pollution;
pub mod recommend;
pub mod report;
pub mod storage;

pub use error::{CoreError, Result};

/// 引擎版本号（同时暴露给 GUI / CLI）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 默认 Fake-IP 地址段（Clash / Clash Meta 默认值）。
pub const DEFAULT_FAKE_IP_RANGE: &str = "198.18.0.0/15";
