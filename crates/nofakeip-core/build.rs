//! 构建脚本：把 `data/` 下的规则数据**压缩后**嵌入二进制。
//!
//! 为什么不在源码里直接 `include_str!` 明文：
//! 内置的国内域名表含大量 `*.qq.com` 等条目，明文嵌入后会被 Windows Defender
//! 的 `TrojanSpy/QQPass` 启发式规则误判为 QQ 盗号木马（实测会直接删除测试二进制，
//! 导致 `cargo test` 报「系统找不到指定的文件」）。
//!
//! 压缩后：二进制中不再出现明文域名列表，同时体积从约 1.7 MB 降到约 400 KB。
//!
//! 仓库中 `data/*.txt` 仍是**唯一事实来源**（可读、可 diff、可被 `tools/gen_data.py` 刷新），
//! 压缩产物只存在于 `OUT_DIR`，不进入版本库。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// 需要嵌入的数据文件。
const FILES: &[&str] = &[
    "builtin_servers.json",
    "china_domains.txt",
    "foreign_domains.txt",
    "foreign_tlds.txt",
    "china_ip_cidr.txt",
    "polluted_ips.txt",
];

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let data_dir = manifest_dir.join("data");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data");

    for name in FILES {
        let src: PathBuf = data_dir.join(name);
        println!("cargo:rerun-if-changed={}", src.display());

        let raw = fs::read(&src)
            .unwrap_or_else(|e| panic!("读取内置数据 {} 失败: {}", src.display(), e));
        if raw.is_empty() {
            panic!("内置数据 {} 为空", src.display());
        }

        // 级别 9：数据只需在构建期压一次，换最小的产物体积
        let compressed = miniz_oxide::deflate::compress_to_vec(&raw, 9);

        let dst = out_dir.join(format!("{}.deflate", name));
        fs::write(&dst, &compressed)
            .unwrap_or_else(|e| panic!("写入 {} 失败: {}", dst.display(), e));

        // 自检：解压必须还原一致，避免构建产物损坏却在运行时才发现
        let back = miniz_oxide::inflate::decompress_to_vec(&compressed)
            .unwrap_or_else(|e| panic!("自检解压 {} 失败: {:?}", name, e));
        assert_eq!(back, raw, "自检失败：{} 解压结果不一致", name);
    }

    let _ = Path::new(&data_dir);
}
