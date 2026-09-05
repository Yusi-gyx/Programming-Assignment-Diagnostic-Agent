//! rustc 错误解析器
//!
//! 职责：将 rustc / cargo 的 stderr 文本输出解析为结构化的诊断信息。
//! 这是后续「错误分类」与「知识点映射」（第 6 步）的输入。
//!
//! # rustc 人类可读输出格式（节选）
//!
//! ```text
//! error[E0382]: borrow of moved value: `s`
//!  --> src/main.rs:6:20
//!   |
//! 4 |     let s = String::from("hello");
//!   |         - move occurs because `s` has type `String`
//! 5 |     let t = s;
//!   |             - value moved here
//! 6 |     println!("{}", s);
//!   |                    ^ value borrowed here after move
//!   |
//! help: consider cloning the value
//!   |
//!   = note: `String` does not implement `Copy`
//!
//! warning: unused variable: `t`
//!  --> src/main.rs:5:9
//!   |
//!   = note: `#[warn(unused_variables)]` on by default
//!
//! error: aborting due to 1 previous error; 1 warning emitted
//!
//! For more information about this error, try `rustc --explain E0382`.
//! ```
//!
//! # 解析目标
//!
//! 每条诊断提取：
//! - 严重级别（error / warning）
//! - 错误码（E0382 等，可能缺失）
//! - 主消息（冒号后的文本）
//! - 主位置（file:line:col）
//! - 附注 / 帮助（` = note:` / `help:` 等）
//!
//! # 关键陷阱：摘要行
//!
//! 末尾的 `error: aborting due to ...` 与
//! `For more information ...` 不是诊断，必须忽略。
//! 区分方法：真实诊断头部之后总会有 ` --> ` 位置行，
//! 摘要行没有。因此采用「找到位置行才提交诊断」的策略。

use serde::{Deserialize, Serialize};

// ============================================================
// 数据结构
// ============================================================

/// 诊断严重级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// 错误（编译失败）
    Error,
    /// 警告（编译可通过，但有告警）
    Warning,
}

/// 源代码位置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    /// 文件路径
    pub file: String,
    /// 行号（从 1 开始）
    pub line: usize,
    /// 列号（从 1 开始）
    pub column: usize,
}

/// 单条 rustc 诊断（解析后的结构化形式）
///
/// 第 6 步的错误分类与知识点映射将基于此结构，
/// 例如用 `code` 字段（E0382）映射到 [`crate::models::KnowledgePoint`]。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RustcDiagnostic {
    /// 严重级别
    pub severity: Severity,
    /// 错误码，如 `"E0382"`；warning 或无码错误为 `None`
    pub code: Option<String>,
    /// 主消息（头部冒号后的文本）
    pub message: String,
    /// 主位置；摘要行无位置
    pub location: Option<SourceLocation>,
    /// 附注与帮助信息
    pub notes: Vec<String>,
}

// ============================================================
// 解析函数
// ============================================================

/// 尝试将一行解析为诊断头部。
///
/// 匹配的行形如：
/// - `error[E0382]: borrow of moved value`  → `(Error, Some("E0382"), "borrow of moved value")`
/// - `warning: unused variable`            → `(Warning, None, "unused variable")`
/// - `error: aborting due to 1 previous error` → `(Error, None, "aborting due to ...")`
///
/// 非头部行返回 `None`。
///
/// 注意：本函数只做行级匹配，不区分真实诊断与摘要行
/// （摘要行的过滤在 [`parse_diagnostics`] 中完成）。
pub fn parse_header(line: &str) -> Option<(Severity, Option<String>, String)> {
    let line = line.trim_start().to_string();
    let (severity, rest) = if let Some(rest) = line.strip_prefix("error") {
        (Severity::Error, rest)
    } else {
        (Severity::Warning, line.strip_prefix("warning")?)
    };
    let mut rest = rest;
    let mut code = None;
    if rest.starts_with('[') {
        let end = rest.find(']')?;
        let inside = &rest[1..end];
        let valid_code = inside.starts_with('E')
            && inside.len() > 1
            && inside[1..].chars().all(|c| c.is_ascii_digit());
        if !valid_code {
            return None;
        }
        code = Some(inside.to_string());
        rest = &rest[end + 1..]; //去除前面已经处理的内容
    }
    let message = rest.strip_prefix(": ")?.to_string();
    Some((severity, code, message))
}

/// 尝试将一行解析为位置行。
///
/// 匹配的行形如 ` --> src/main.rs:6:20`（前导可有空白）。
/// 返回解析出的 [`SourceLocation`]；非位置行返回 `None`。
///
/// 提示：文件路径可能包含冒号（Windows 盘符），因此应
/// **从右向左**解析最后两个冒号分隔的数字作为 line / column。
pub fn parse_location(line: &str) -> Option<SourceLocation> {
    let line = line.trim_start().to_string();
    let rest = line.strip_prefix("-->")?.trim();
    let last_colon = rest.rfind(':')?;
    let column_str = &rest[last_colon + 1..];
    let before_colon = &rest[..last_colon].to_string();
    let second_colon = before_colon.rfind(':')?;
    let line_str = &before_colon[second_colon + 1..];
    let file = &before_colon[..second_colon].to_string();
    let column = column_str.parse::<usize>().ok()?;
    let line = line_str.parse::<usize>().ok()?;
    Some(SourceLocation {
        file: file.to_string(),
        line,
        column,
    })
}

/// 尝试将一行解析为附注行。
///
/// 匹配形如 `  = note: ...`、`  = help: ...`、`help: ...` 的行，
/// 返回去掉前缀后的附注文本；非附注行返回 `None`。
pub fn parse_note(line: &str) -> Option<String> {
    let line = line.trim();
    let message = if let Some(message) = line.strip_prefix("= note:") {
        message
    } else if let Some(message) = line.strip_prefix("= help:") {
        message
    } else {
        line.strip_prefix("help:")?
    };
    Some(message.trim().to_string())
}

/// 解析 rustc stderr，返回结构化诊断列表。
///
/// 输入为空、仅含摘要行 / 提示行时返回空 `Vec`。
///
/// # 算法：逐行状态机
///
/// 1. 用 [`parse_header`] 识别诊断头部：遇到则保存为「待提交」诊断。
/// 2. 用 [`parse_location`] 识别位置行：为当前待提交诊断补充位置。
/// 3. 用 [`parse_note`] 识别附注：追加到当前待提交诊断的 notes。
/// 4. 遇到下一个头部时：若上一条已有位置则提交，否则丢弃（摘要行）。
/// 5. 末尾同样处理最后一条。
///
/// 这样末尾的 `error: aborting due to ...` 因无位置而被自动忽略。
pub fn parse_diagnostics(stderr: &str) -> Vec<RustcDiagnostic> {
    let mut result: Vec<RustcDiagnostic> = Vec::new();
    let mut pending: Option<RustcDiagnostic> = None;
    for line in stderr.lines() {
        if let Some((severity, code, message)) = parse_header(line) {
            if let Some(diag) = pending.take()
                && diag.location.is_some()
            {
                result.push(diag);
            }
            pending = Some(RustcDiagnostic {
                severity,
                code,
                message,
                location: None,
                notes: vec![],
            });
            continue;
        }
        if let Some(location) = parse_location(line) {
            if let Some(diag) = pending.as_mut() {
                diag.location = Some(location);
            }
            continue;
        }
        if let Some(note) = parse_note(line)
            && let Some(diag) = pending.as_mut()
        {
            diag.notes.push(note);
        }
    }
    // 循环结束后提交最后一条 pending（有位置才提交，否则丢弃摘要行）
    if let Some(diag) = pending
        && diag.location.is_some()
    {
        result.push(diag);
    }
    result
}
