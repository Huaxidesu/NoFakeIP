//! 历史记录查询命令。

use crate::commands::err;
use crate::state::AppState;
use nofakeip_core::benchmark::DomainBenchResult;
use nofakeip_core::storage::DomainHistoryRow;
use tauri::State;

#[tauri::command]
pub async fn list_history(
    state: State<'_, AppState>,
    limit: u32,
) -> Result<Vec<DomainHistoryRow>, String> {
    state.storage.list_domains(limit.max(1)).map_err(err)
}

#[tauri::command]
pub async fn list_benchmarks(
    state: State<'_, AppState>,
    domain: Option<String>,
    limit: u32,
) -> Result<Vec<DomainBenchResult>, String> {
    let d = domain.as_deref().filter(|s| !s.trim().is_empty());
    state.storage.list_benchmarks(d, limit.max(1)).map_err(err)
}
