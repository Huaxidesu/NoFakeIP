//! 规则数据源管理命令。

use crate::commands::err;
use crate::state::AppState;
use nofakeip_core::geodata::{DataStatus, DataUpdater, UpdateReport};
use std::sync::Arc;
use tauri::{Emitter, State};

#[tauri::command]
pub async fn list_data_sources(state: State<'_, AppState>) -> Result<Vec<DataStatus>, String> {
    Ok(DataUpdater::new(state.data_dir.clone()).status())
}

/// 更新数据源。`ids` 为空时更新「默认启用」的数据源。
///
/// 进度通过 `data:progress` 事件推送；失败项会保留本地已有数据。
#[tauri::command]
pub async fn update_data_sources(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<UpdateReport>, String> {
    let updater = DataUpdater::new(state.data_dir.clone());
    let emitter = app.clone();
    let progress: nofakeip_core::geodata::ProgressFn = Arc::new(move |done, total, name| {
        let _ = emitter.emit(
            "data:progress",
            serde_json::json!({ "done": done, "total": total, "name": name }),
        );
    });
    // 一次更新一个源，方便前端看到逐项进度
    let reports = updater.update_many(&ids, Some(progress)).await;
    if reports.is_empty() {
        return Err("没有匹配的数据源".into());
    }
    Ok(reports)
}
