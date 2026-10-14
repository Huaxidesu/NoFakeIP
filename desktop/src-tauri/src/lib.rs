//! NoFakeIP 桌面 / Android 客户端（Tauri 2）。
//!
//! 分层约定（见 `doc/01-系统架构.md`）：本 crate **只做 IPC 适配与状态管理**，
//! 全部业务逻辑在 `nofakeip-core` 中实现。
//!
//! **范围说明**：不生成、不修改任何 Clash / OpenClash / Clash Verge 配置。

mod commands;
mod state;

pub use state::{AppState, Settings};

use nofakeip_core::dns::ServerRegistry;
use nofakeip_core::storage::Storage;
use tauri::Manager;

/// 首次运行时把内置 DNS 节点写入数据库。
fn seed_builtin(storage: &Storage) -> anyhow::Result<()> {
    if storage.list_servers(false)?.is_empty() {
        let registry = ServerRegistry::new();
        for s in registry.default_set() {
            storage.upsert_server(&s)?;
        }
        tracing::info!("已写入 {} 个内置 DNS 节点", registry.default_set().len());
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| anyhow::anyhow!("无法获取应用数据目录: {}", e))?;
            std::fs::create_dir_all(&data_dir)?;

            let storage = Storage::open(&data_dir.join("nofakeip.db"))
                .map_err(|e| anyhow::anyhow!("打开数据库失败: {}", e))?;
            seed_builtin(&storage)?;

            let settings = state::load_settings(&storage);
            let st = AppState::new(data_dir, storage, settings)?;
            app.manage(st);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 状态与设置
            commands::get_status,
            commands::get_recommendation,
            commands::get_settings,
            commands::set_settings,
            commands::get_data_dir,
            // 测速
            commands::run_benchmark,
            commands::run_full_benchmark,
            // 诊断
            commands::classify_domain,
            commands::detect_domain,
            // 报告
            commands::generate_report,
            commands::save_text_file,
            // 规则数据
            commands::list_data_sources,
            commands::update_data_sources,
            // DNS 节点
            commands::list_servers,
            commands::add_server,
            commands::update_server,
            commands::delete_server,
            commands::set_server_enabled,
            commands::import_servers,
            commands::export_servers,
            // 历史
            commands::list_history,
            commands::list_benchmarks,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
