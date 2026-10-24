//! NoFakeIP 桌面 / Android 客户端（Tauri 2）。
//!
//! 分层约定（见 `doc/01-系统架构.md`）：本 crate 只做 IPC 适配与状态管理，
//! 全部业务逻辑在 `nofakeip-core` 中实现。
//!
//! 范围说明：不生成、不修改任何代理软件配置。

mod commands;
mod state;

pub use state::{AppState, Settings};

use nofakeip_core::dns::ServerRegistry;
use nofakeip_core::storage::Storage;
use tauri::Manager;

// 注意：`#[tauri::command]` 生成的 `__cmd__*` 宏是 `#[macro_export]` 的（位于 crate 根），
// 因此 `generate_handler!` 里必须用非限定名，不能用 `commands::foo`。
// 这里显式 import 一次，既满足宏解析，也让调用点更简洁。
use commands::{
    add_server, classify_domain, delete_server, detect_domain, export_servers, generate_report,
    get_data_dir, get_proxy_status, get_recommendation, get_settings, get_status, import_servers,
    list_benchmarks, list_data_sources, list_history, list_servers, run_benchmark,
    run_full_benchmark, save_text_file, set_proxy_enabled, set_server_enabled, set_settings,
    update_data_sources, update_server,
};

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

            // 启动后台解析出站代理（环境变量 → 常见端口探测），完成后重建诊断器。
            // 端口探测最坏约 2 秒，放后台不阻塞启动。
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use commands::status::re_resolve_proxy;
                let Some(state) = handle.try_state::<AppState>() else {
                    return;
                };
                let settings = state.settings_snapshot();
                re_resolve_proxy(&state, &settings).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 状态与设置
            get_status,
            get_recommendation,
            get_settings,
            set_settings,
            get_data_dir,
            get_proxy_status,
            set_proxy_enabled,
            // 测速
            run_benchmark,
            run_full_benchmark,
            // 诊断
            classify_domain,
            detect_domain,
            // 报告
            generate_report,
            save_text_file,
            // 规则数据
            list_data_sources,
            update_data_sources,
            // DNS 节点
            list_servers,
            add_server,
            update_server,
            delete_server,
            set_server_enabled,
            import_servers,
            export_servers,
            // 历史
            list_history,
            list_benchmarks,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
