//! Tauri command 适配层。
//!
//! 设计约定（见 `doc/08-GUI规格.md` 第 3 节）：
//! - 所有 command 返回 `Result<T, String>`（Rust 错误统一 `to_string()`）；
//! - 本层**只做参数转换与状态存取**，业务逻辑一律在 `nofakeip-core` 中实现；
//! - 长任务通过 Tauri event 推送进度（`bench:progress` / `data:progress`）。

pub mod bench;
pub mod data;
pub mod detect;
pub mod dns;
pub mod report;
pub mod status;
pub mod storage;

pub use bench::{run_benchmark, run_full_benchmark};
pub use data::{list_data_sources, update_data_sources};
pub use detect::{classify_domain, detect_domain};
pub use dns::{
    add_server, delete_server, export_servers, import_servers, list_servers, set_server_enabled,
    update_server,
};
pub use report::{generate_report, save_text_file};
pub use status::{get_data_dir, get_recommendation, get_settings, get_status, set_settings};
pub use storage::{list_benchmarks, list_history};

/// 把核心错误统一转成前端可读的中文消息。
pub(crate) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
