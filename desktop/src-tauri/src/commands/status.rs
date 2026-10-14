//! 状态与设置相关命令。

use crate::state::{save_settings, AppState, Settings};
use crate::commands::err;
use nofakeip_core::recommend::{Recommendation, Recommender};
use nofakeip_core::storage::StorageStats;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct AppStatus {
    pub version: String,
    pub data_dir: String,
    pub server_count: i64,
    pub enabled_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_bench_at: Option<String>,
    pub stats: StorageStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<Recommendation>,
}

/// 基于数据库里最近的测速记录重算推荐结论。
fn compute_recommendation(state: &AppState) -> Option<Recommendation> {
    let results = state.storage.list_benchmarks(None, 300).ok()?;
    if results.is_empty() {
        return None;
    }
    let diagnoser = state.diagnoser();
    let reports = diagnoser.scan_reports(&results);
    Some(Recommender::from_benchmark(&results, &reports))
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<AppStatus, String> {
    let stats = state.storage.stats().map_err(err)?;
    let servers = state.storage.list_servers(false).map_err(err)?;
    let enabled = servers.iter().filter(|s| s.enabled).count() as i64;
    let last_bench_at = state
        .storage
        .list_benchmarks(None, 1)
        .ok()
        .and_then(|v| v.first().map(|r| r.created_at.clone()));
    let recommendation = compute_recommendation(&state);

    Ok(AppStatus {
        version: nofakeip_core::VERSION.to_string(),
        data_dir: state.data_dir.display().to_string(),
        server_count: servers.len() as i64,
        enabled_count: enabled,
        last_bench_at,
        stats,
        recommendation,
    })
}

#[tauri::command]
pub async fn get_recommendation(
    state: State<'_, AppState>,
) -> Result<Option<Recommendation>, String> {
    Ok(compute_recommendation(&state))
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings_snapshot())
}

#[tauri::command]
pub async fn set_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    save_settings(&state.storage, &settings).map_err(err)?;
    state.apply_settings(settings).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub async fn get_data_dir(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.data_dir.display().to_string())
}
