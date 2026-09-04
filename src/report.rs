//! 诊断报告格式化（开发计划第 12 步）
//!
//! 职责：
//! - 将编译诊断与测试失败格式化为人类可读的文本报告
//! - 支持控制台文本与 Markdown 两种格式
//! - 格式对应 DESIGN.md §4.1 的输出规范
//!
//! 设计原则：
//! - 报告格式化是确定性逻辑，由 Rust 完成
//! - LLM 仅在后续步骤负责自然语言化提示内容
//!
//! # 输出格式（DESIGN.md §4.1）
//!
//! 控制台文本：
//! ```text
//! [编译错误] main.rs:7:5  E0382 (borrow of moved value)
//!   知识点 : Ownership / Move
//!   提示   : 这是一个编译错误
//!
//! [测试失败] test_case_03
//!   期望输出 : 3 2 1
//!   实际输出 : 1 2 3
//!   知识点   : 待分析
//!   提示     : 这是一个逻辑错误（测试未通过）
//! ```
//!
//! Markdown 格式类似，使用代码块和列表语法。

use crate::analysis::error_parser::RustcDiagnostic;
use crate::analysis::hint::{Hint, format_location, knowledge_point_text};
use crate::models::{Diagnostic, TestResult};

// ============================================================
// 报告条目
// ============================================================

/// 一条编译诊断的报告条目。
#[derive(Debug, Clone)]
pub struct CompileReportEntry {
    /// 原始 rustc 诊断（含位置、错误码、消息）
    pub diag: RustcDiagnostic,
    /// 分类结果（含错误类别、知识点、置信度）
    pub classified: Diagnostic,
    /// 分层提示
    pub hint: Hint,
}

/// 一条测试失败的报告条目。
#[derive(Debug, Clone)]
pub struct TestReportEntry {
    /// 测试结果
    pub result: TestResult,
    /// 分层提示
    pub hint: Hint,
}

// ============================================================
// 诊断报告
// ============================================================

/// 完整的诊断报告，聚合编译诊断与测试失败。
#[derive(Debug, Clone, Default)]
pub struct DiagnosticReport {
    /// 编译诊断条目
    pub compile_entries: Vec<CompileReportEntry>,
    /// 测试失败条目
    pub test_entries: Vec<TestReportEntry>,
}

impl DiagnosticReport {
    /// 创建空报告。
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一条编译诊断。
    pub fn add_compile(&mut self, entry: CompileReportEntry) {
        self.compile_entries.push(entry);
    }

    /// 添加一条测试失败。
    pub fn add_test(&mut self, entry: TestReportEntry) {
        self.test_entries.push(entry);
    }

    /// 报告是否为空（无任何诊断）。
    pub fn is_empty(&self) -> bool {
        self.compile_entries.is_empty() && self.test_entries.is_empty()
    }

    /// 格式化为控制台文本（DESIGN.md §4.1 格式）。
    pub fn to_text(&self) -> String {
        let mut out = String::new();

        for entry in &self.compile_entries {
            out.push_str(&format_compile_entry_text(entry));
            out.push('\n');
        }

        for entry in &self.test_entries {
            out.push_str(&format_test_entry_text(entry));
            out.push('\n');
        }

        if self.is_empty() {
            out.push_str("未发现问题：编译通过，全部测试通过。\n");
        }

        out
    }

    /// 面向真实终端的彩色文本。重定向到文件时应继续使用 [`Self::to_text`]。
    pub fn to_colored_text(&self) -> String {
        self.to_text()
            .lines()
            .map(colorize_line)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    /// 格式化为 Markdown（供 `--report report.md` 导出）。
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();

        out.push_str("# 诊断报告\n\n");

        if self.is_empty() {
            out.push_str("✅ 未发现问题：编译通过，全部测试通过。\n");
            return out;
        }

        if !self.compile_entries.is_empty() {
            out.push_str("## 编译诊断\n\n");
            for entry in &self.compile_entries {
                out.push_str(&format_compile_entry_markdown(entry));
                out.push('\n');
            }
        }

        if !self.test_entries.is_empty() {
            out.push_str("## 测试失败\n\n");
            for entry in &self.test_entries {
                out.push_str(&format_test_entry_markdown(entry));
                out.push('\n');
            }
        }

        out
    }
}

fn colorize_line(line: &str) -> String {
    const RED: &str = "\x1b[31;1m";
    const GREEN: &str = "\x1b[32;1m";
    const YELLOW: &str = "\x1b[33m";
    const BLUE: &str = "\x1b[34;1m";
    const RESET: &str = "\x1b[0m";

    let color = if line.starts_with('[') {
        Some(RED)
    } else if line.trim_start().starts_with("知识点") {
        Some(YELLOW)
    } else if line.trim_start().starts_with("提示") {
        Some(BLUE)
    } else if line.starts_with("未发现问题") {
        Some(GREEN)
    } else {
        None
    };
    color
        .map(|value| format!("{value}{line}{RESET}"))
        .unwrap_or_else(|| line.to_owned())
}

// ============================================================
// 格式化函数（内部，纯函数便于测试）
// ============================================================

/// 格式化单条编译诊断为控制台文本。
fn format_compile_entry_text(entry: &CompileReportEntry) -> String {
    let diag = &entry.diag;
    let classified = &entry.classified;

    // 第一行：[编译错误] 位置  错误码 (消息)
    let location_str = diag
        .location
        .as_ref()
        .map(format_location)
        .unwrap_or_else(|| "未知位置".into());

    let code_str = diag
        .code
        .as_ref()
        .map(|c| format!("  {} ({})", c, diag.message))
        .unwrap_or_else(|| format!("  ({})", diag.message));

    let mut out = format!(
        "[{}] {}{}\n",
        category_label(classified),
        location_str,
        code_str
    );

    // 知识点行
    let kps: Vec<&str> = classified
        .knowledge_points
        .iter()
        .map(|k| knowledge_point_text(*k))
        .collect();
    let kp_str = if kps.is_empty() {
        "待分析".to_string()
    } else {
        kps.join("、")
    };
    out.push_str(&format!("  知识点 : {}\n", kp_str));

    // 提示行
    out.push_str(&format!("  提示   : {}\n", entry.hint.content));

    out
}

/// 格式化单条测试失败为控制台文本。
fn format_test_entry_text(entry: &TestReportEntry) -> String {
    let result = &entry.result;

    let mut out = format!("[测试失败] {}\n", result.name);

    out.push_str(&format!("  期望输出 : {}\n", result.expected_output.trim()));
    out.push_str(&format!("  实际输出 : {}\n", result.actual_output.trim()));
    out.push_str("  知识点   : 待分析\n");
    out.push_str(&format!("  提示     : {}\n", entry.hint.content));

    out
}

/// 格式化单条编译诊断为 Markdown。
fn format_compile_entry_markdown(entry: &CompileReportEntry) -> String {
    let diag = &entry.diag;
    let classified = &entry.classified;

    let location_str = diag
        .location
        .as_ref()
        .map(format_location)
        .unwrap_or_else(|| "未知位置".into());

    let mut out = format!(
        "### {} `{}`\n\n",
        category_label(classified),
        diag.code.as_deref().unwrap_or("无错误码")
    );

    out.push_str(&format!("- **位置**: `{}`\n", location_str));
    out.push_str(&format!("- **消息**: {}\n", diag.message));

    let kps: Vec<&str> = classified
        .knowledge_points
        .iter()
        .map(|k| knowledge_point_text(*k))
        .collect();
    let kp_str = if kps.is_empty() {
        "待分析".to_string()
    } else {
        kps.join("、")
    };
    out.push_str(&format!("- **知识点**: {}\n", kp_str));
    out.push_str(&format!("- **提示**: {}\n\n", entry.hint.content));

    out
}

/// 格式化单条测试失败为 Markdown。
fn format_test_entry_markdown(entry: &TestReportEntry) -> String {
    let result = &entry.result;

    let mut out = format!("### 测试失败: `{}`\n\n", result.name);
    out.push_str(&format!(
        "- **期望输出**: `{}`\n",
        result.expected_output.trim()
    ));
    out.push_str(&format!(
        "- **实际输出**: `{}`\n",
        result.actual_output.trim()
    ));
    out.push_str("- **知识点**: 待分析\n");
    out.push_str(&format!("- **提示**: {}\n\n", entry.hint.content));

    out
}

/// 将错误类别转为报告标签。
fn category_label(classified: &Diagnostic) -> &'static str {
    use crate::models::ErrorCategory::*;
    match classified.category {
        CompileError => "编译错误",
        RuntimeError => "运行时错误",
        LogicError => "逻辑错误",
        BoundaryCondition => "边界条件错误",
        AlgorithmError => "算法错误",
    }
}
