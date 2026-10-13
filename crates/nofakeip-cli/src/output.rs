//! CLI 输出辅助：表格对齐、颜色、进度、JSON。
//!
//! 跨平台要点（见 `doc/07-CLI规格.md` 第 6 节）：
//! - 非 TTY 或 `--quiet` 时关闭颜色与进度条，避免污染管道输出；
//! - 中文按 2 个字符宽度计算，保证表格对齐；
//! - Windows 启动时切换到 UTF-8 代码页。

use std::io::{IsTerminal, Write};

/// 输出控制器。
#[derive(Debug, Clone, Copy)]
pub struct Out {
    pub json: bool,
    pub quiet: bool,
    pub color: bool,
}

impl Out {
    pub fn new(json: bool, quiet: bool, color_mode: &str) -> Self {
        let is_tty = std::io::stdout().is_terminal();
        let color = match color_mode {
            "always" => true,
            "never" => false,
            _ => is_tty && !json && !quiet,
        };
        Self {
            json,
            quiet,
            color,
        }
    }

    /// 普通输出（stdout）。`--json` 模式下抑制，避免污染 JSON。
    pub fn line(&self, s: &str) {
        if self.json {
            return;
        }
        println!("{}", s);
    }

    /// 空行。
    pub fn blank(&self) {
        if !self.json && !self.quiet {
            println!();
        }
    }

    /// 提示信息（stderr），`--quiet` 时抑制。
    pub fn info(&self, s: &str) {
        if !self.quiet {
            eprintln!("{}", s);
        }
    }

    /// 警告（stderr，始终输出）。
    pub fn warn(&self, s: &str) {
        eprintln!("{}", self.paint(s, "33"));
    }

    /// 错误（stderr，始终输出）。
    pub fn error(&self, s: &str) {
        eprintln!("{}", self.paint(s, "31"));
    }

    /// 成功（stdout）。
    pub fn success(&self, s: &str) {
        self.line(&self.paint(s, "32"));
    }

    /// 章节标题。
    pub fn heading(&self, s: &str) {
        if self.json {
            return;
        }
        println!("{}", self.paint(s, "36;1"));
    }

    fn paint(&self, s: &str, code: &str) -> String {
        if self.color {
            format!("\x1b[{}m{}\x1b[0m", code, s)
        } else {
            s.to_string()
        }
    }

    /// 输出纯 JSON（唯一 stdout 内容）。
    pub fn json_out(&self, s: &str) {
        println!("{}", s);
    }

    /// 进度（stderr，覆盖同一行）。
    pub fn progress(&self, done: usize, total: usize, label: &str) {
        if self.json || self.quiet || !std::io::stderr().is_terminal() {
            return;
        }
        let width = 30usize;
        let ratio = if total == 0 {
            0.0
        } else {
            done as f64 / total as f64
        };
        let filled = (ratio * width as f64).round() as usize;
        let bar = format!(
            "{}{}",
            "=".repeat(filled.min(width)),
            " ".repeat(width.saturating_sub(filled.min(width)))
        );
        let label = truncate_display(label, 30);
        eprint!("\r  [{}] {}/{} {}", bar, done, total, label);
        let _ = std::io::stderr().flush();
    }

    /// 结束进度行。
    pub fn progress_done(&self) {
        if self.json || self.quiet || !std::io::stderr().is_terminal() {
            return;
        }
        eprintln!();
    }

    /// 打印等宽表格（中文按 2 宽度对齐）。
    pub fn table(&self, headers: &[&str], rows: &[Vec<String>]) {
        if self.json || rows.is_empty() {
            return;
        }
        let cols = headers.len();
        let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
        for row in rows {
            for i in 0..cols {
                let w = row.get(i).map(|s| display_width(s)).unwrap_or(0);
                if w > widths[i] {
                    widths[i] = w;
                }
            }
        }

        let sep: String = widths
            .iter()
            .map(|w| "-".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("+");
        println!("{}", sep);
        println!("{}", format_row(headers.iter().map(|s| s.to_string()).collect(), &widths));
        println!("{}", sep);
        for row in rows {
            println!("{}", format_row(row.clone(), &widths));
        }
        println!("{}", sep);
    }
}

fn format_row(cells: Vec<String>, widths: &[usize]) -> String {
    let mut out = String::new();
    for (i, w) in widths.iter().enumerate() {
        let cell = cells.get(i).cloned().unwrap_or_default();
        out.push(' ');
        out.push_str(&cell);
        let pad = w.saturating_sub(display_width(&cell));
        out.push_str(&" ".repeat(pad));
        out.push(' ');
        if i + 1 < widths.len() {
            out.push('|');
        }
    }
    out
}

/// 字符串显示宽度（CJK 字符按 2 计）。
pub fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| if is_wide(c) { 2 } else { 1 })
        .sum::<usize>()
}

fn is_wide(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x1100..=0x115F |   // 韩文字母
        0x2E80..=0x303E |   // CJK 部首
        0x3041..=0x33FF |   // 假名 / CJK 符号
        0x3400..=0x4DBF |   // CJK 扩展 A
        0x4E00..=0x9FFF |   // CJK 统一表意
        0xA000..=0xA4CF |
        0xAC00..=0xD7A3 |   // 韩文音节
        0xF900..=0xFAFF |
        0xFE30..=0xFE6F |
        0xFF00..=0xFF60 |   // 全角
        0xFFE0..=0xFFE6 |
        0x1F300..=0x1F64F | // 表情
        0x1F900..=0x1F9FF |
        0x20000..=0x2FFFD
    )
}

/// 按显示宽度截断。
pub fn truncate_display(s: &str, max: usize) -> String {
    let mut w = 0usize;
    let mut out = String::new();
    for c in s.chars() {
        let cw = if is_wide(c) { 2 } else { 1 };
        if w + cw > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// Windows 下把控制台切到 UTF-8 代码页（失败静默忽略）。
pub fn setup_console() {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("chcp")
            .arg("65001")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_cjk_as_two() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("国内"), 4);
        assert_eq!(display_width("阿里 DNS"), 8);
    }

    #[test]
    fn truncate_respects_width() {
        assert_eq!(truncate_display("abcdef", 3), "abc");
        assert_eq!(truncate_display("阿里巴巴", 4), "阿里");
    }

    #[test]
    fn row_formatting_pads_correctly() {
        let row = format_row(vec!["国内".into(), "x".into()], &[4, 3]);
        assert_eq!(row, " 国内 | x   ");
    }
}
