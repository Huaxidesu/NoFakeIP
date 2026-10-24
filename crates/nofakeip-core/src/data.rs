//! 内置数据资源加载。
//!
//! 设计要点（见 `doc/02-核心引擎设计.md` 第 13 节、`doc/14-数据源与更新.md`）：
//! - 全部内置数据在构建期压缩后嵌入（见 `build.rs`），运行时首次访问时解压并缓存，
//!   保证程序在没有外部文件的干净环境（如 Android）下也能完整工作；
//! - 运行时若用户数据目录下存在同名文件，则优先使用用户文件，实现规则热覆盖；
//! - 数据文件仍以明文形式保存在仓库 `data/` 目录，便于审阅与刷新（`tools/gen_data.py`）。
//!
//! 为什么压缩嵌入而不明文嵌入：明文会触发 Windows Defender 的
//! `TrojanSpy/QQPass` 启发式误报（内置域名表含大量 `*.qq.com`），导致构建产物被删除。
//! 详见 `doc/11-构建发布与Android.md` 第 6 节。

use crate::error::{CoreError, Result};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// 压缩后的数据由 build.rs 生成到 OUT_DIR
const BUILTIN_SERVERS_JSON_Z: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/builtin_servers.json.deflate"));
const CHINA_DOMAINS_Z: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/china_domains.txt.deflate"));
const FOREIGN_DOMAINS_Z: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/foreign_domains.txt.deflate"));
const FOREIGN_TLDS_Z: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/foreign_tlds.txt.deflate"));
const CHINA_IP_CIDR_Z: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/china_ip_cidr.txt.deflate"));
const POLLUTED_IPS_Z: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/polluted_ips.txt.deflate"));

/// 内置资源。
pub struct BuiltinData;

impl BuiltinData {
    /// 全部内置资源的文件名清单（用于导出/覆盖）。
    pub const FILES: &'static [&'static str] = &[
        "builtin_servers.json",
        "china_domains.txt",
        "foreign_domains.txt",
        "foreign_tlds.txt",
        "china_ip_cidr.txt",
        "polluted_ips.txt",
    ];

    /// 内置 DNS 节点 JSON（国内 6 + 国外 7，其中部分默认不参与测速）。
    pub fn builtin_servers_json() -> &'static str {
        static V: OnceLock<String> = OnceLock::new();
        V.get_or_init(|| inflate_or_empty(BUILTIN_SERVERS_JSON_Z))
    }

    /// 国内域名规则库（约 11 万条可注册域名）。
    pub fn china_domains() -> &'static str {
        static V: OnceLock<String> = OnceLock::new();
        V.get_or_init(|| inflate_or_empty(CHINA_DOMAINS_Z))
    }

    /// 国外域名规则库（Majestic Million 头部，已剔除国内域名）。
    pub fn foreign_domains() -> &'static str {
        static V: OnceLock<String> = OnceLock::new();
        V.get_or_init(|| inflate_or_empty(FOREIGN_DOMAINS_Z))
    }

    /// 国外 TLD 弱判定规则。
    pub fn foreign_tlds() -> &'static str {
        static V: OnceLock<String> = OnceLock::new();
        V.get_or_init(|| inflate_or_empty(FOREIGN_TLDS_Z))
    }

    /// 中国 IP 网段（离线归属地判定）。
    pub fn china_ip_cidr() -> &'static str {
        static V: OnceLock<String> = OnceLock::new();
        V.get_or_init(|| inflate_or_empty(CHINA_IP_CIDR_Z))
    }

    /// 已知 DNS 投毒 IP 黑名单。
    pub fn polluted_ips() -> &'static str {
        static V: OnceLock<String> = OnceLock::new();
        V.get_or_init(|| inflate_or_empty(POLLUTED_IPS_Z))
    }

    /// 按名称取内置内容。
    pub fn embedded(name: &str) -> Option<&'static str> {
        match name {
            "builtin_servers.json" => Some(Self::builtin_servers_json()),
            "china_domains.txt" => Some(Self::china_domains()),
            "foreign_domains.txt" => Some(Self::foreign_domains()),
            "foreign_tlds.txt" => Some(Self::foreign_tlds()),
            "china_ip_cidr.txt" => Some(Self::china_ip_cidr()),
            "polluted_ips.txt" => Some(Self::polluted_ips()),
            _ => None,
        }
    }

    /// 加载资源：优先用户目录文件，缺失时回退到内置内容。
    ///
    /// `data_dir` 为空或文件不存在 → 返回内置内容。
    pub fn load_or_embedded(data_dir: Option<&Path>, name: &str) -> Result<String> {
        if let Some(dir) = data_dir {
            let p = dir.join(name);
            if p.is_file() {
                return std::fs::read_to_string(&p).map_err(|e| {
                    CoreError::NotFound(format!("读取 {} 失败: {}", p.display(), e))
                });
            }
        }
        Self::embedded(name)
            .map(|s| s.to_string())
            .ok_or_else(|| CoreError::NotFound(format!("未知内置资源: {}", name)))
    }

    /// 把全部内置资源释放到指定目录（供用户查看/编辑）。
    ///
    /// 已存在的文件不会被覆盖，避免冲掉用户改动。
    pub fn extract_to(dir: &Path) -> Result<Vec<PathBuf>> {
        std::fs::create_dir_all(dir)?;
        let mut written = Vec::new();
        for name in Self::FILES {
            let target = dir.join(name);
            if target.exists() {
                continue;
            }
            if let Some(content) = Self::embedded(name) {
                std::fs::write(&target, content)?;
                written.push(target);
            }
        }
        Ok(written)
    }

    /// 解析「# 注释 + 一行一条」格式的文本文件为条目列表。
    pub fn parse_lines(text: &str) -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.to_string())
            .collect()
    }
}

/// 解压内置数据。理论上不可能失败（build.rs 已自检），失败时记录错误并返回空串。
fn inflate_or_empty(compressed: &[u8]) -> String {
    match miniz_oxide::inflate::decompress_to_vec(compressed) {
        Ok(v) => String::from_utf8_lossy(&v).into_owned(),
        Err(e) => {
            // 用 eprintln 而不是 tracing，避免在 tracing 未初始化时丢失信息
            eprintln!("[NoFakeIP] 内置数据解压失败（构建产物可能损坏）: {:?}", e);
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_resources_are_non_empty() {
        assert!(BuiltinData::china_domains().lines().count() > 1000);
        assert!(BuiltinData::foreign_domains().lines().count() > 1000);
        assert!(BuiltinData::china_ip_cidr().lines().count() > 1000);
        assert!(BuiltinData::polluted_ips().lines().count() > 10);
        assert!(!BuiltinData::builtin_servers_json().is_empty());
    }

    #[test]
    fn builtin_servers_json_is_valid() {
        let v: serde_json::Value =
            serde_json::from_str(BuiltinData::builtin_servers_json()).unwrap();
        let arr = v["servers"].as_array().unwrap();
        assert!(arr.len() >= 9, "内置节点至少应有 9 个");
    }

    #[test]
    fn compression_is_effective() {
        // 压缩后体积应显著小于原文（否则 build.rs 的压缩没生效）
        let plain = BuiltinData::china_domains().len();
        assert!(plain > 500_000, "国内域名表应有较大体积，实际 {}", plain);
        assert!(
            CHINA_DOMAINS_Z.len() < plain / 2,
            "压缩后 {} 字节，原文 {} 字节，压缩率不足",
            CHINA_DOMAINS_Z.len(),
            plain
        );
    }

    #[test]
    fn parse_lines_skips_comments_and_blanks() {
        let text = "# comment\n\nbaidu.com\n  qq.com  \n";
        assert_eq!(BuiltinData::parse_lines(text), vec!["baidu.com", "qq.com"]);
    }

    #[test]
    fn load_or_embedded_falls_back() {
        let s = BuiltinData::load_or_embedded(
            Some(Path::new("Z:/definitely/not/exists")),
            "polluted_ips.txt",
        )
        .unwrap();
        assert!(s.contains("243.185.187.39"));
    }

    #[test]
    fn unknown_resource_errors() {
        assert!(BuiltinData::load_or_embedded(None, "nope.txt").is_err());
    }
}
