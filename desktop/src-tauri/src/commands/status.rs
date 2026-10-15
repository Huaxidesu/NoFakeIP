//! 状态与设置相关命令。

use crate::state::{save_settings, AppState, Settings};
use crate::commands::err;
use nofakeip_core::net::{resolve_proxy, ProxyMode};
use nofakeip_core::recommend::{Recommendation, Recommender};
use nofakeip_core::storage::StorageStats;
use serde::Serialize;
use tauri::State;

/// 代理状态（供设置页与功能页展示）。
#[derive(Debug, Serialize)]
pub struct ProxyStatus {
    /// 用户配置的模式："auto" / "custom" / "off"
    pub mode: String,
    /// 手动模式下用户填写的地址
    pub custom_url: String,
    /// 已解析出的代理地址（探测结果，展示用）
    pub resolved_url: Option<String>,
    /// 功能页快捷开关是否打开
    pub enabled: bool,
    /// 当前实际生效的代理（enabled && resolved）
    pub effective_url: Option<String>,
}

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
    state.apply_settings(settings.clone()).map_err(err)?;
    // 代理配置可能变化：重新解析并重建诊断器（在线归属地会带上新代理）
    re_resolve_proxy(&state, &settings).await;
    Ok(())
}

/// 把 settings 的代理配置转成 ProxyMode。
fn proxy_mode_of(settings: &Settings) -> ProxyMode {
    match settings.proxy_mode.as_str() {
        "off" => ProxyMode::Direct,
        "custom" => ProxyMode::Custom(settings.proxy_url.clone()),
        _ => ProxyMode::Auto,
    }
}

/// 按设置重新解析出站代理，解析完成后重建诊断器。
///
/// 解析失败（如手动地址非法）只记日志并回退直连，不阻塞设置保存。
pub(crate) async fn re_resolve_proxy(state: &AppState, settings: &Settings) {
    let mode = proxy_mode_of(settings);
    let resolved = match resolve_proxy(&mode).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("代理解析失败，回退直连: {}", e);
            None
        }
    };
    if let Err(e) = state.set_resolved_proxy(resolved) {
        tracing::warn!("代理更新后重建诊断器失败: {}", e);
    }
}

/// 查询当前代理状态。
#[tauri::command]
pub async fn get_proxy_status(state: State<'_, AppState>) -> Result<ProxyStatus, String> {
    let settings = state.settings_snapshot();
    let resolved = state.proxy_raw();
    let enabled = state.proxy_enabled();
    Ok(ProxyStatus {
        mode: settings.proxy_mode,
        custom_url: settings.proxy_url,
        resolved_url: resolved.as_ref().map(|p| p.to_string()),
        enabled,
        effective_url: if enabled {
            resolved.as_ref().map(|p| p.to_string())
        } else {
            None
        },
    })
}

/// 切换功能页快捷开关（仅启停，完整配置在设置页）。
#[tauri::command]
pub async fn set_proxy_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    state.set_proxy_enabled(enabled);
    // 开关影响诊断器的在线归属地是否走代理，重建一次
    state.rebuild_diagnoser().map_err(err)?;
    Ok(())
}

#[tauri::command]
pub async fn get_data_dir(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.data_dir.display().to_string())
}
