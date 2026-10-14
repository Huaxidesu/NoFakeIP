//! DNS 节点管理命令。

use crate::commands::err;
use crate::state::AppState;
use nofakeip_core::dns::ServerRegistry;
use nofakeip_core::models::{DnsServer, Region};
use tauri::State;

#[tauri::command]
pub async fn list_servers(
    state: State<'_, AppState>,
    only_enabled: bool,
) -> Result<Vec<DnsServer>, String> {
    state.storage.list_servers(only_enabled).map_err(err)
}

/// 用一行文本添加节点，语法见 `doc/02-核心引擎设计.md` 第 4.3 节。
#[tauri::command]
pub async fn add_server(
    state: State<'_, AppState>,
    spec: String,
    name: Option<String>,
    region: Option<String>,
) -> Result<DnsServer, String> {
    let registry = ServerRegistry::new();
    let mut server = registry.parse_server(&spec).map_err(err)?;
    server.name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| server.address.clone());
    if let Some(r) = region.as_deref().filter(|r| !r.trim().is_empty()) {
        server.region = Region::parse(r).ok_or_else(|| format!("非法区域: {}", r))?;
    }
    let id = state.storage.upsert_server(&server).map_err(err)?;
    server.id = id;
    Ok(server)
}

#[tauri::command]
pub async fn update_server(state: State<'_, AppState>, server: DnsServer) -> Result<(), String> {
    state.storage.upsert_server(&server).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub async fn delete_server(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.storage.delete_server(id).map_err(err)
}

#[tauri::command]
pub async fn set_server_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> Result<(), String> {
    state.storage.set_server_enabled(id, enabled).map_err(err)
}

#[tauri::command]
pub async fn import_servers(
    state: State<'_, AppState>,
    path: String,
    format: String,
) -> Result<usize, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {}", path, e))?;
    let registry = ServerRegistry::new();
    let servers = if format.eq_ignore_ascii_case("json") {
        registry.import_json(&text).map_err(err)?
    } else {
        registry.import_txt(&text).map_err(err)?
    };
    let mut n = 0usize;
    for s in servers {
        if state.storage.upsert_server(&s).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

#[tauri::command]
pub async fn export_servers(
    state: State<'_, AppState>,
    path: String,
    format: String,
) -> Result<(), String> {
    let list = state.storage.list_servers(false).map_err(err)?;
    let registry = ServerRegistry::new();
    let content = if format.eq_ignore_ascii_case("json") {
        registry.export_json(&list).map_err(err)?
    } else {
        registry.export_txt(&list)
    };
    std::fs::write(&path, content).map_err(|e| format!("写入 {} 失败: {}", path, e))
}
