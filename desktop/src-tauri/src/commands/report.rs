//! 报告生成与文件保存命令。
//!
//! 范围说明：只产出诊断报告，不生成任何代理软件配置
//! （见 `doc/06-推荐与报告规格.md` 第 4 节）。

use crate::commands::err;
use crate::state::AppState;
use nofakeip_core::benchmark;
use nofakeip_core::recommend::Recommender;
use nofakeip_core::report::{self, JsonEnvelope, ReportOptions};
use tauri::State;

/// 生成报告。
///
/// - `scope = "domain"`：对指定域名做一次完整诊断，输出单域名报告
/// - `scope = "last"`：基于数据库里最近的测速记录，输出汇总报告
/// - `format`：`md` / `json` / `csv`
#[tauri::command]
pub async fn generate_report(
    state: State<'_, AppState>,
    scope: String,
    domain: Option<String>,
    format: String,
) -> Result<String, String> {
    let fmt = format.to_ascii_lowercase();
    let diagnoser = state.diagnoser();
    let ropts = ReportOptions::default();

    if scope == "domain" {
        let d = domain.unwrap_or_default().trim().to_string();
        if d.is_empty() {
            return Err("请输入要生成报告的域名".into());
        }
        let all = state.storage.list_servers(false).map_err(err)?;
        let servers: Vec<_> = all.into_iter().filter(|s| s.enabled).collect();
        if servers.is_empty() {
            return Err("没有可用的 DNS 节点".into());
        }
        let opts = state.settings_snapshot().bench_options();
        let diag = diagnoser.diagnose(&d, &servers, &opts).await.map_err(err)?;
        let _ = state.storage.insert_benchmark(&diag.bench);

        return Ok(match fmt.as_str() {
            "json" => {
                let results = [diag.bench.clone()];
                let reports = [diag.report.clone()];
                JsonEnvelope::new("report", &results, &reports, &diag.recommendation)
                    .to_json()
                    .map_err(err)?
            }
            "csv" => {
                let results = [diag.bench.clone()];
                benchmark::export_csv(&results).map_err(err)?
            }
            _ => report::render_domain_markdown(&diag, &ropts),
        });
    }

    // scope = "last"：用历史记录做汇总
    let results = state.storage.list_benchmarks(None, 500).map_err(err)?;
    if results.is_empty() {
        return Err("还没有测速记录：请先执行一次全量测速或单域名检测".into());
    }
    let reports = diagnoser.scan_reports(&results);
    let rec = Recommender::from_benchmark(&results, &reports);

    Ok(match fmt.as_str() {
        "json" => JsonEnvelope::new("report", &results, &reports, &rec)
            .to_json()
            .map_err(err)?,
        "csv" => benchmark::export_csv(&results).map_err(err)?,
        _ => report::render_overview_markdown(&results, &reports, &rec),
    })
}

/// 把文本写入用户选择的路径（由前端通过文件对话框取得）。
#[tauri::command]
pub async fn save_text_file(content: String, path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    std::fs::write(&p, content).map_err(|e| format!("写入 {} 失败: {}", path, e))
}
