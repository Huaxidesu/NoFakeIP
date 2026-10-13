//! NoFakeIP 命令行工具。
//!
//! 命令与输出规范见 `doc/07-CLI规格.md`。
//!
//! **范围说明**：本工具只做 DNS 测速、域名分类、污染检测与报告输出，
//! **不生成、不修改任何 Clash / OpenClash / Clash Verge 配置**。

mod commands;
mod output;

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

/// 退出码（见 `doc/07-CLI规格.md` 第 4 节）。
pub mod exit {
    pub const OK: u8 = 0;
    pub const GENERAL: u8 = 1;
    pub const NO_SERVERS: u8 = 2;
    pub const ALL_FAILED: u8 = 3;
    pub const POLLUTION: u8 = 4;
    pub const STORAGE: u8 = 5;
}

#[derive(Parser, Debug)]
#[command(
    name = "nofakeip",
    version,
    about = "NoFakeIP —— 面向 Fake-IP 用户的智能 DNS 诊断、测速与污染检测工具",
    long_about = "NoFakeIP 帮助你：\n  \
        · 测出国内 / 国外各 DNS 的真实性能（含 TCP/TLS/查询分段耗时）\n  \
        · 自动识别域名属于国内还是国外\n  \
        · 检测 DNS 污染与本地 Fake-IP 是否正常\n  \
        · 给出基于实测的 DNS 使用建议与可读诊断报告\n\n\
        本工具不生成、不修改任何 Clash / OpenClash / Clash Verge 配置。",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// 以 JSON 输出（稳定契约，见 doc/06 第 2.3 节）
    #[arg(long, global = true)]
    pub json: bool,

    /// 指定 SQLite 数据库路径
    #[arg(long, global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// 指定数据目录（规则文件与数据库所在目录）
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// 单次查询超时（秒）
    #[arg(long, global = true, default_value_t = 5, value_name = "SECS")]
    pub timeout: u64,

    /// 每服务器测试轮次
    #[arg(long, global = true, default_value_t = 3, value_name = "N")]
    pub rounds: u32,

    /// 并发上限
    #[arg(long, global = true, default_value_t = 16, value_name = "N")]
    pub concurrency: usize,

    /// 启用在线 IP 归属地查询（默认关闭；开启后会把 IP 发送给第三方服务）
    #[arg(long, global = true)]
    pub online_geo: bool,

    /// Fake-IP 地址段
    #[arg(long, global = true, default_value = "198.18.0.0/15", value_name = "CIDR")]
    pub fake_ip_range: String,

    /// 静默模式（只输出结果）
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// 详细日志（可重复：-v / -vv）
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// 颜色：auto / always / never
    #[arg(long, global = true, default_value = "auto", value_name = "WHEN")]
    pub color: String,

    /// 当检测到高污染风险时以退出码 4 结束（便于脚本判断）
    #[arg(long, global = true)]
    pub fail_on_pollution: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 单/多域名快速扫描（测速 + 分类 + 污染检测 + 推荐）
    Scan {
        /// 待检测域名，可多个
        #[arg(required = true, value_name = "DOMAIN")]
        domains: Vec<String>,
        /// 只测指定服务器（名称模糊匹配，逗号分隔）
        #[arg(long, value_delimiter = ',', value_name = "NAME")]
        servers: Vec<String>,
        /// 测试全部已启用服务器（默认行为）
        #[arg(long)]
        all: bool,
        /// 跳过污染检测
        #[arg(long)]
        no_detect: bool,
    },

    /// 批量测速（域名列表 → CSV）
    Bench {
        /// 域名列表文件（一行一个，# 开头为注释）
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// 直接给出域名（逗号分隔）
        #[arg(long, value_delimiter = ',', value_name = "DOMAINS")]
        domains: Vec<String>,
        /// 导出 CSV 路径
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// 只测指定服务器
        #[arg(long, value_delimiter = ',', value_name = "NAME")]
        servers: Vec<String>,
    },

    /// 域名国内外分类
    Classify {
        #[arg(required = true, value_name = "DOMAIN")]
        domains: Vec<String>,
    },

    /// 污染与 Fake-IP 检测
    Detect {
        #[arg(required = true, value_name = "DOMAIN")]
        domains: Vec<String>,
        /// 只测指定服务器
        #[arg(long, value_delimiter = ',', value_name = "NAME")]
        servers: Vec<String>,
    },

    /// 生成诊断报告（Markdown / JSON / CSV）
    Report {
        /// 指定域名（缺省时基于最近一次批量测速结果生成汇总报告）
        #[arg(long, value_name = "DOMAIN")]
        domain: Option<String>,
        /// 输出文件路径（缺省打印到 stdout）
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// 格式：md / json / csv
        #[arg(long, default_value = "md", value_name = "FORMAT")]
        format: String,
        /// 汇总模式（使用最近一次批量测速的全部结果）
        #[arg(long)]
        full: bool,
    },

    /// 规则数据（域名表 / IP 网段）管理
    Data {
        #[command(subcommand)]
        action: DataAction,
    },

    /// DNS 服务器管理
    Dns {
        #[command(subcommand)]
        action: DnsAction,
    },

    /// 历史记录
    History {
        #[arg(long, value_name = "DOMAIN")]
        domain: Option<String>,
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: u32,
        /// 只显示指定风险等级：low / medium / high
        #[arg(long, value_name = "LEVEL")]
        risk: Option<String>,
    },

    /// 环境自检
    Doctor,
}

#[derive(Subcommand, Debug)]
pub enum DataAction {
    /// 查看各数据源状态
    List,
    /// 更新数据源（不带 ID 时更新默认数据源）
    Update {
        #[arg(value_name = "ID")]
        ids: Vec<String>,
        /// 更新全部（含大文件）
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DnsAction {
    /// 列出服务器
    List {
        /// 包含已禁用的服务器
        #[arg(long)]
        all: bool,
    },
    /// 添加服务器
    Add(AddServerArgs),
    /// 删除服务器
    Remove {
        #[arg(value_name = "NAME|ID")]
        key: String,
    },
    /// 启用服务器
    Enable {
        #[arg(value_name = "NAME|ID")]
        key: String,
    },
    /// 禁用服务器
    Disable {
        #[arg(value_name = "NAME|ID")]
        key: String,
    },
    /// 从文件导入
    Import {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// json / txt
        #[arg(long, default_value = "txt", value_name = "FORMAT")]
        format: String,
    },
    /// 导出到文件
    Export {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// json / txt
        #[arg(long, default_value = "txt", value_name = "FORMAT")]
        format: String,
    },
}

#[derive(Args, Debug)]
pub struct AddServerArgs {
    /// 服务器地址，如 223.5.5.5 / tls://1.1.1.1:853#cloudflare-dns.com / https://dns.alidns.com/dns-query
    #[arg(value_name = "SPEC")]
    pub spec: String,
    /// 展示名
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
    /// 归属：china / global / unknown
    #[arg(long, value_name = "REGION")]
    pub region: Option<String>,
    /// 是否启用（默认启用）
    #[arg(long)]
    pub disabled: bool,
}

fn main() -> ExitCode {
    output::setup_console();

    let cli = Cli::parse();
    let out = output::Out::new(cli.json, cli.quiet, &cli.color);
    init_tracing(cli.verbose);

    let code = match commands::dispatch(&cli, &out) {
        Ok(code) => code,
        Err(e) => {
            out.error(&format!("错误: {}", e));
            exit::GENERAL
        }
    };
    ExitCode::from(code)
}

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}
