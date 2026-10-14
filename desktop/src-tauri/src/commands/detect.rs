//! 域名分类与诊断命令。

use crate::commands::err;
use crate::state::AppState;
use nofakeip_core::classifier::Classification;
use nofakeip_core::diagnose::DomainDiagnosis;
use tauri::State;

#[tauri::command]
pub async fn classify_domain(
    state: State<'_, AppState>,
    domain: String,
) -> Result<Classification, String> {
    let d = domain.trim();
    if d.is_empty() {
        return Err("域名不能为空".into());
    }
    Ok(state.diagnoser().classifier().classify(d))
}

/// 单域名完整诊断：分类 + 测速 + 污染检测 + 推荐，并写入历史。
#[tauri::command]
pub async fn detect_domain(
    state: State<'_, AppState>,
    domain: String,
    server_ids: Vec<i64>,
) -> Result<DomainDiagnosis, String> {
    let d = domain.trim().to_string();
    if d.is_empty() {
        return Err("域名不能为空".into());
    }

    let all = state.storage.list_servers(false).map_err(err)?;
    let servers: Vec<_> = if server_ids.is_empty() {
        all.into_iter().filter(|s| s.enabled).collect()
    } else {
        all.into_iter().filter(|s| server_ids.contains(&s.id)).collect()
    };
    if servers.is_empty() {
        return Err("没有可用的 DNS 节点，请到「设置」中启用至少一个".into());
    }

    let opts = state.settings_snapshot().bench_options();
    let diag = state
        .diagnoser()
        .diagnose(&d, &servers, &opts)
        .await
        .map_err(err)?;

    let ips: Vec<String> = diag.bench.all_ips().iter().map(|i| i.to_string()).collect();
    let _ = state.storage.insert_benchmark(&diag.bench);
    let _ = state.storage.touch_domain(
        &d,
        diag.classification.region,
        Some(diag.report.risk),
        Some(diag.report.fake_ip.label()),
        &ips,
    );

    Ok(diag)
}
