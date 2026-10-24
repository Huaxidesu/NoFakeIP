//! 应用状态与设置。

use nofakeip_core::benchmark::BenchmarkOptions;
use nofakeip_core::classifier::DomainClassifier;
use nofakeip_core::diagnose::Diagnoser;
use nofakeip_core::net::ProxyUrl;
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
    /// 出站代理模式："auto"（自动探测）/ "custom"（手动地址）/ "off"（强制直连）。
    pub proxy_mode: String,
    /// 手动模式下的代理地址（`http://` 或 `socks5://`）。
    pub proxy_url: String,
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
            // 默认自动探测（环境变量 → 本机常见代理端口），照顾裸内核代理软件用户
            proxy_mode: "auto".to_string(),
            proxy_url: String::new(),
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
    /// 已解析的出站代理（按 settings 的 proxy_mode 解析一次后缓存）。
    proxy: Mutex<Option<ProxyUrl>>,
    /// 功能页快捷开关：是否启用出站代理（关闭后即使已探测到也直连）。
    proxy_enabled: std::sync::atomic::AtomicBool,
}

impl AppState {
    pub fn new(data_dir: PathBuf, storage: Storage, settings: Settings) -> anyhow::Result<Self> {
        // 启动时先以「无代理」构建诊断器；代理解析是异步操作，
        // 由 lib.rs 的 setup 里 spawn 后台任务完成后重建。
        let diagnoser = build_diagnoser(&data_dir, &settings, &storage, None)?;
        Ok(Self {
            data_dir,
            storage,
            settings: Mutex::new(settings),
            diagnoser: Mutex::new(Arc::new(diagnoser)),
            proxy: Mutex::new(None),
            proxy_enabled: std::sync::atomic::AtomicBool::new(true),
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

    /// 当前生效的出站代理（受快捷开关控制；None = 直连）。
    pub fn proxy_snapshot(&self) -> Option<ProxyUrl> {
        use std::sync::atomic::Ordering;
        if !self.proxy_enabled.load(Ordering::SeqCst) {
            return None;
        }
        self.proxy_raw()
    }

    /// 已解析的代理（不受快捷开关影响，供状态展示）。
    pub fn proxy_raw(&self) -> Option<ProxyUrl> {
        self.proxy.lock().ok().and_then(|g| g.clone())
    }

    pub fn proxy_enabled(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.proxy_enabled.load(Ordering::SeqCst)
    }

    pub fn set_proxy_enabled(&self, enabled: bool) {
        use std::sync::atomic::Ordering;
        self.proxy_enabled.store(enabled, Ordering::SeqCst);
    }

    /// 存入新解析出的代理并重建诊断器（在线归属地查询会带上代理）。
    pub fn set_resolved_proxy(&self, proxy: Option<ProxyUrl>) -> anyhow::Result<()> {
        if let Ok(mut g) = self.proxy.lock() {
            *g = proxy;
        }
        self.rebuild_diagnoser()
    }

    /// 用当前设置 + 当前代理重建诊断器。
    pub fn rebuild_diagnoser(&self) -> anyhow::Result<()> {
        let settings = self.settings_snapshot();
        let rebuilt = build_diagnoser(&self.data_dir, &settings, &self.storage, self.proxy_raw())?;
        if let Ok(mut g) = self.diagnoser.lock() {
            *g = Arc::new(rebuilt);
        }
        Ok(())
    }

    /// 更新设置并按需重建诊断器。
    pub fn apply_settings(&self, next: Settings) -> anyhow::Result<()> {
        let storage = &self.storage;
        let rebuilt = build_diagnoser(&self.data_dir, &next, storage, self.proxy_raw())?;
        if let Ok(mut g) = self.diagnoser.lock() {
            *g = Arc::new(rebuilt);
        }
        if let Ok(mut g) = self.settings.lock() {
            *g = next;
        }
        Ok(())
    }
}

/// 构造诊断器：加载用户自定义规则、Fake-IP 段、在线归属地开关与出站代理。
pub fn build_diagnoser(
    dir: &Path,
    settings: &Settings,
    storage: &Storage,
    proxy: Option<ProxyUrl>,
) -> anyhow::Result<Diagnoser> {
    let detector = PollutionDetector::with_data_dir(Some(dir))
        .with_fake_ip_range(&settings.fake_ip_range)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_online_geo_and_proxy(settings.online_geo, proxy);

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
