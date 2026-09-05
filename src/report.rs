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
//!   知识点：Ownership / Move
//!   提示：这是一个编译错误
//!
//! [测试失败] test_case_03
//!   期望输出：3 2 1
//!   实际输出：1 2 3
//!   知识点：待分析
//!   提示：这是一个逻辑错误（测试未通过）
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
    /// 测试失败分类及由模型映射的知识点
    pub classified: Diagnostic,
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
    /// 本轮实际执行的测试总数与通过数；None 表示没有执行测试。
    pub test_run: Option<(usize, usize)>,
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

    pub fn set_test_run(&mut self, total: usize, passed: usize) {
        self.test_run = Some((total, passed));
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
            match self.test_run {
                Some((total, passed)) => out.push_str(&format!(
                    "✓ 未发现问题：编译通过，测试 {passed}/{total} 通过。\n"
                )),
                None => out.push_str("✓ 编译通过；本轮没有执行测试。\n"),
            }
        }

        out
    }

    /// 面向真实终端的彩色文本。重定向到文件时应继续使用 [`Self::to_text`]。
    pub fn to_colored_text(&self) -> String {
        self.to_colored_text_excluding(&[], &[])
    }

    pub fn to_text_excluding(&self, compile: &[usize], tests: &[usize]) -> String {
        let mut out = String::new();
        for (index, entry) in self.compile_entries.iter().enumerate() {
            if !compile.contains(&index) {
                out.push_str(&format_compile_entry_text(entry));
                out.push('\n');
            }
        }
        for (index, entry) in self.test_entries.iter().enumerate() {
            if !tests.contains(&index) {
                out.push_str(&format_test_entry_text(entry));
                out.push('\n');
            }
        }
        if self.is_empty() {
            match self.test_run {
                Some((total, passed)) => out.push_str(&format!(
                    "✓ 未发现问题：编译通过，测试 {passed}/{total} 通过。\n"
                )),
                None => out.push_str("✓ 编译通过；本轮没有执行测试。\n"),
            }
        }
        out
    }

    pub fn to_colored_text_excluding(&self, compile: &[usize], tests: &[usize]) -> String {
        let mut in_multiline_hint = false;
        self.to_text_excluding(compile, tests)
            .lines()
            .map(|line| {
                if line.starts_with('[') {
                    in_multiline_hint = false;
                }
                if line.trim_start().starts_with("提示") {
                    in_multiline_hint = line.trim_end().ends_with([':', '：']);
                    return colorize_line(line);
                }
                if in_multiline_hint && !line.is_empty() {
                    return format!("\x1b[34;1m{line}\x1b[0m");
                }
                colorize_line(line)
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    pub fn compile_stream_prefix(&self, index: usize, colored: bool) -> String {
        stream_prefix_compile(&self.compile_entries[index], colored)
    }

    pub fn test_stream_prefix(&self, index: usize, colored: bool) -> String {
        stream_prefix_test(&self.test_entries[index], colored)
    }

    /// 格式化为 Markdown（供 `--report report.md` 导出）。
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();

        out.push_str("# 诊断报告\n\n");

        if self.is_empty() {
            match self.test_run {
                Some((total, passed)) => out.push_str(&format!(
                    "✅ 未发现问题：编译通过，测试 {passed}/{total} 通过。\n"
                )),
                None => out.push_str("✅ 编译通过；本轮没有执行测试。\n"),
            }
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

pub fn format_test_run(results: &[TestResult], colored: bool) -> String {
    let passed = results.iter().filter(|result| result.passed).count();
    let mut out = format!(
        "\n┌─ {}  {passed}/{} 通过 ─────────────────────\n",
        terminal_style("测试结果", "1;36", colored),
        results.len()
    );
    for result in results {
        if result.passed {
            out.push_str(&format!(
                "│ {} {}\n",
                terminal_style("✓", "1;32", colored),
                result.name
            ));
        } else {
            out.push_str(&format!(
                "│ {} {}\n",
                terminal_style("✗", "1;31", colored),
                result.name
            ));
            out.push_str(&format!(
                "│   期望：{}\n",
                visible_output(&result.expected_output)
            ));
            out.push_str(&format!(
                "│   实际：{}\n",
                visible_output(&result.actual_output)
            ));
            if let Some(error) = &result.runtime_error {
                out.push_str(&format!("│   运行错误：{}\n", visible_output(error)));
            }
        }
    }
    out.push_str("└────────────────────────────────────────────\n");
    out
}

fn visible_output(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "<空输出>".into()
    } else {
        trimmed.replace('\n', "\\n")
    }
}

fn terminal_style(text: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn stream_prefix_compile(entry: &CompileReportEntry, colored: bool) -> String {
    let location = entry
        .diag
        .location
        .as_ref()
        .map(format_location)
        .unwrap_or_else(|| "未知位置".into());
    let heading = format!(
        "[{}] {}  {} ({})",
        category_label(&entry.classified),
        location,
        entry.diag.code.as_deref().unwrap_or("无错误码"),
        entry.diag.message
    );
    let knowledge = entry
        .classified
        .knowledge_points
        .iter()
        .map(|point| knowledge_point_text(*point))
        .collect::<Vec<_>>()
        .join("、");
    let value = if knowledge.is_empty() {
        "待分析"
    } else {
        &knowledge
    };
    let text = format!("{}\n  知识点：{}\n  提示：\n", heading, value);
    if colored {
        format!(
            "\x1b[31;1m{heading}\x1b[0m\n  \x1b[33;1m知识点：\x1b[0m{}\n  \x1b[34;1m提示：\x1b[0m\n",
            value
        )
    } else {
        text
    }
}

fn stream_prefix_test(entry: &TestReportEntry, colored: bool) -> String {
    let heading = format!("[{}] {}", test_label(entry), entry.result.name);
    let mut text = format!(
        "{}\n  期望输出：{}\n  实际输出：{}\n",
        heading,
        visible_output(&entry.result.expected_output),
        visible_output(&entry.result.actual_output),
    );
    if let Some(error) = &entry.result.runtime_error {
        text.push_str(&format!("  运行错误：{}\n", visible_output(error)));
    }
    text.push_str(&format!(
        "  知识点：{}\n  提示：\n",
        knowledge_points_text(&entry.classified)
    ));
    if colored {
        format!(
            "{}\n",
            text.lines()
                .map(colorize_line)
                .collect::<Vec<_>>()
                .join("\n")
        )
    } else {
        text
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
    } else if line.starts_with('✓') || line.starts_with("未发现问题") {
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
    out.push_str(&format!("  知识点：{}\n", kp_str));

    push_text_hint(&mut out, "  提示：", &entry.hint.content);

    out
}

/// 格式化单条测试失败为控制台文本。
fn format_test_entry_text(entry: &TestReportEntry) -> String {
    let result = &entry.result;

    let mut out = format!("[{}] {}\n", test_label(entry), result.name);

    out.push_str(&format!(
        "  期望输出：{}\n",
        visible_output(&result.expected_output)
    ));
    out.push_str(&format!(
        "  实际输出：{}\n",
        visible_output(&result.actual_output)
    ));
    if let Some(error) = &result.runtime_error {
        out.push_str(&format!("  运行错误：{}\n", error.trim()));
    }
    out.push_str(&format!(
        "  知识点：{}\n",
        knowledge_points_text(&entry.classified)
    ));
    push_text_hint(&mut out, "  提示：", &entry.hint.content);

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
    push_markdown_hint(&mut out, &entry.hint.content);

    out
}

/// 格式化单条测试失败为 Markdown。
fn format_test_entry_markdown(entry: &TestReportEntry) -> String {
    let result = &entry.result;

    let mut out = format!("### {}: `{}`\n\n", test_label(entry), result.name);
    out.push_str(&format!(
        "- **期望输出**: `{}`\n",
        result.expected_output.trim()
    ));
    out.push_str(&format!(
        "- **实际输出**: `{}`\n",
        result.actual_output.trim()
    ));
    if let Some(error) = &result.runtime_error {
        out.push_str(&format!("- **运行错误**: `{}`\n", error.trim()));
    }
    out.push_str(&format!(
        "- **知识点**: {}\n",
        knowledge_points_text(&entry.classified)
    ));
    push_markdown_hint(&mut out, &entry.hint.content);

    out
}

fn test_label(entry: &TestReportEntry) -> &'static str {
    if entry.classified.category == crate::models::ErrorCategory::RuntimeError {
        "运行时错误"
    } else {
        "测试失败"
    }
}

fn knowledge_points_text(classified: &Diagnostic) -> String {
    let points = classified
        .knowledge_points
        .iter()
        .map(|point| knowledge_point_text(*point))
        .collect::<Vec<_>>();
    if points.is_empty() {
        "待分析（配置 LLM 后自动映射）".into()
    } else {
        points.join("、")
    }
}

fn push_text_hint(out: &mut String, label: &str, content: &str) {
    if content.contains('\n') {
        out.push_str(label);
        out.push('\n');
        for line in content.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    } else {
        out.push_str(label);
        out.push_str(content);
        out.push('\n');
    }
}

fn push_markdown_hint(out: &mut String, content: &str) {
    if content.contains('\n') {
        out.push_str("- **提示**:\n\n");
        out.push_str(content.trim());
        out.push_str("\n\n");
    } else {
        out.push_str(&format!("- **提示**: {content}\n\n"));
    }
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
