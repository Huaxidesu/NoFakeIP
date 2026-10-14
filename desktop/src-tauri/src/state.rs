//! 应用状态与设置。

use nofakeip_core::benchmark::BenchmarkOptions;
use nofakeip_core::classifier::DomainClassifier;
use nofakeip_core::diagnose::Diagnoser;
use nofakeip_core::pollution::PollutionDetector;
use nofakeip_core::storage::Storage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 用户设置（持久化在 `user_config` 表，key = "settings"）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub timeout_secs: u64,
    pub rounds: u32,
    pub concurrency: usize,
    pub fake_ip_range: String,
    pub online_geo: bool,
    pub sample_domains: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout_secs: 5,
            rounds: 3,
            // Android 上网络栈与省电策略更激进，默认并发取保守值
            concurrency: if cfg!(target_os = "android") { 8 } else { 16 },
            fake_ip_range: nofakeip_core::DEFAULT_FAKE_IP_RANGE.to_string(),
            online_geo: false,
            sample_domains: nofakeip_core::benchmark::DEFAULT_SAMPLE_DOMAINS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl Settings {
    pub fn bench_options(&self) -> BenchmarkOptions {
        BenchmarkOptions::default()
            .with_rounds(self.rounds)
            .with_timeout(Duration::from_secs(self.timeout_secs))
            .with_concurrency(self.concurrency)
    }
}

/// 应用全局状态。
pub struct AppState {
    pub data_dir: PathBuf,
    pub storage: Storage,
    pub settings: Mutex<Settings>,
    /// 诊断器：设置变化（Fake-IP 段 / 在线归属地 / 用户规则）时整体替换。
    /// 用 `Arc` + 短锁：取出 Arc 后立刻释放锁再 await，避免跨 await 持锁。
    diagnoser: Mutex<Arc<Diagnoser>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf, storage: Storage, settings: Settings) -> anyhow::Result<Self> {
        let diagnoser = build_diagnoser(&data_dir, &settings, &storage)?;
        Ok(Self {
            data_dir,
            storage,
            settings: Mutex::new(settings),
            diagnoser: Mutex::new(Arc::new(diagnoser)),
        })
    }

    pub fn diagnoser(&self) -> Arc<Diagnoser> {
        self.diagnoser
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| Arc::new(Diagnoser::new()))
    }

    pub fn settings_snapshot(&self) -> Settings {
        self.settings
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// 更新设置并按需重建诊断器。
    pub fn apply_settings(&self, next: Settings) -> anyhow::Result<()> {
        let storage = &self.storage;
        let rebuilt = build_diagnoser(&self.data_dir, &next, storage)?;
        if let Ok(mut g) = self.diagnoser.lock() {
            *g = Arc::new(rebuilt);
        }
        if let Ok(mut g) = self.settings.lock() {
            *g = next;
        }
        Ok(())
    }
}

/// 构造诊断器：加载用户自定义规则、Fake-IP 段、在线归属地开关。
pub fn build_diagnoser(
    dir: &Path,
    settings: &Settings,
    storage: &Storage,
) -> anyhow::Result<Diagnoser> {
    let detector = PollutionDetector::with_data_dir(Some(dir))
        .with_fake_ip_range(&settings.fake_ip_range)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_online_geo(settings.online_geo);

    let mut classifier = DomainClassifier::with_data_dir(Some(dir));
    for r in storage.list_rules().map_err(|e| anyhow::anyhow!(e.to_string()))? {
        classifier.add_user_override(&r.pattern, r.region);
    }

    Ok(Diagnoser::with_data_dir(Some(dir))
        .with_detector(detector)
        .with_classifier(classifier))
}

/// 从数据库读取设置（不存在则用默认值）。
pub fn load_settings(storage: &Storage) -> Settings {
    match storage.get_config("settings") {
        Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_default(),
        _ => Settings::default(),
    }
}

/// 保存设置到数据库。
pub fn save_settings(storage: &Storage, settings: &Settings) -> anyhow::Result<()> {
    let json = serde_json::to_string(settings)?;
    storage
        .set_config("settings", &json)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(())
}
