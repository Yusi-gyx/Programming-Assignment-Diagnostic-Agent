//! rustc 错误解析器测试
//!
//! 可单独运行：
//!
//! ```bash
//! cargo test --test diagnostic_tests
//! ```

use pada::analysis::error_parser::{
    Severity, parse_diagnostics, parse_header, parse_location, parse_note,
};
use pada::tools::compiler::CompilerTool;
use std::path::PathBuf;

fn fixture(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/rust");
    p.push(relative);
    p
}

// ============================================================
// parse_header 单元测试
// ============================================================

#[test]
fn test_parse_header_error_with_code() {
    let (sev, code, msg) =
        parse_header("error[E0382]: borrow of moved value: `s`").expect("应识别为头部行");
    assert_eq!(sev, Severity::Error);
    assert_eq!(code.as_deref(), Some("E0382"));
    assert_eq!(msg, "borrow of moved value: `s`");
}

#[test]
fn test_parse_header_warning_no_code() {
    let (sev, code, msg) = parse_header("warning: unused variable: `t`").expect("应识别为头部行");
    assert_eq!(sev, Severity::Warning);
    assert!(code.is_none());
    assert_eq!(msg, "unused variable: `t`");
}

#[test]
fn test_parse_header_error_no_code() {
    // 无错误码的 error（如摘要行），parse_header 仍应解析
    let (sev, code, msg) =
        parse_header("error: aborting due to 1 previous error").expect("应识别为头部行");
    assert_eq!(sev, Severity::Error);
    assert!(code.is_none());
    assert_eq!(msg, "aborting due to 1 previous error");
}

#[test]
fn test_parse_header_non_header() {
    assert!(parse_header("  --> src/main.rs:6:20").is_none());
    assert!(parse_header("4 |     let s = String::from(\"hello\");").is_none());
    assert!(parse_header("  = note: something").is_none());
    assert!(parse_header("For more information about this error").is_none());
}

// ============================================================
// parse_location 单元测试
// ============================================================

#[test]
fn test_parse_location_basic() {
    let loc = parse_location(" --> src/main.rs:6:20").expect("应识别为位置行");
    assert_eq!(loc.file, "src/main.rs");
    assert_eq!(loc.line, 6);
    assert_eq!(loc.column, 20);
}

#[test]
fn test_parse_location_with_spaces() {
    // 前导空白与路径中含空格
    let loc = parse_location("   --> my project/main.rs:15:5").expect("应识别为位置行");
    assert_eq!(loc.file, "my project/main.rs");
    assert_eq!(loc.line, 15);
    assert_eq!(loc.column, 5);
}

#[test]
fn test_parse_location_non_location() {
    assert!(parse_location("error[E0382]: borrow of moved value").is_none());
    assert!(parse_location("  |").is_none());
    assert!(parse_location("4 |     let s = 1;").is_none());
}

// ============================================================
// parse_note 单元测试
// ============================================================

#[test]
fn test_parse_note_note() {
    let note = parse_note("  = note: `String` does not implement `Copy`").expect("应识别为附注");
    assert_eq!(note, "`String` does not implement `Copy`");
}

#[test]
fn test_parse_note_help() {
    let note = parse_note("help: consider cloning the value").expect("应识别为附注");
    assert_eq!(note, "consider cloning the value");
}

#[test]
fn test_parse_note_non_note() {
    assert!(parse_note("error[E0382]: borrow of moved value").is_none());
    assert!(parse_note("  --> src/main.rs:6:20").is_none());
    assert!(parse_note("4 |     let s = 1;").is_none());
}

// ============================================================
// parse_diagnostics 单元测试（内联样例）
// ============================================================

/// 一段典型的 rustc 输出：1 个 error + 1 个 warning + 摘要行
const SAMPLE_E0382: &str = "\
error[E0382]: borrow of moved value: `s`
 --> src/main.rs:6:20
  |
4 |     let s = String::from(\"hello\");
  |         - move occurs because `s` has type `String`
5 |     let t = s;
  |             - value moved here
6 |     println!(\"{}\", s);
  |                    ^ value borrowed here after move
  |
help: consider cloning the value
  |
  = note: `String` does not implement `Copy`

warning: unused variable: `t`
 --> src/main.rs:5:9
  |
5 |     let t = s;
  |         ^ help: if this is intentional, prefix it with `_t`
  |
  = note: `#[warn(unused_variables)]` on by default

error: aborting due to 1 previous error; 1 warning emitted

For more information about this error, try `rustc --explain E0382`.
";

#[test]
fn test_parse_diagnostics_single_error_with_warning() {
    let diags = parse_diagnostics(SAMPLE_E0382);
    // 应解析出 2 条：1 error + 1 warning；摘要行被忽略
    assert_eq!(diags.len(), 2, "应为 2 条诊断（1 error + 1 warning）");

    // 第一条：error E0382
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(diags[0].code.as_deref(), Some("E0382"));
    assert_eq!(diags[0].message, "borrow of moved value: `s`");
    let loc = diags[0].location.as_ref().expect("应有位置");
    assert_eq!(loc.file, "src/main.rs");
    assert_eq!(loc.line, 6);
    assert_eq!(loc.column, 20);

    // 第二条：warning 无码
    assert_eq!(diags[1].severity, Severity::Warning);
    assert!(diags[1].code.is_none());
    assert_eq!(diags[1].message, "unused variable: `t`");
    assert!(diags[1].location.is_some(), "warning 也应有位置");
}

#[test]
fn test_parse_diagnostics_summary_ignored() {
    // 仅含摘要行，应返回空
    let diags = parse_diagnostics("error: aborting due to 2 previous errors\n");
    assert!(diags.is_empty(), "摘要行不应产生诊断");
}

#[test]
fn test_parse_diagnostics_no_trailing_summary() {
    // 输入不以摘要行结尾时，最后一条诊断不应丢失
    let mut s = String::new();
    s.push_str("error[E0382]: borrow of moved value: `s`\n");
    s.push_str(" --> src/main.rs:6:20\n");
    s.push_str("  |\n");
    s.push_str("6 |     println!(\"{}\", s);\n");
    s.push_str("  |                    ^ value borrowed here after move\n");

    let diags = parse_diagnostics(&s);
    assert_eq!(diags.len(), 1, "最后一条诊断不应丢失");
    assert_eq!(diags[0].code.as_deref(), Some("E0382"));
    assert!(diags[0].location.is_some());
}

#[test]
fn test_parse_diagnostics_empty() {
    assert!(parse_diagnostics("").is_empty());
    assert!(parse_diagnostics("   \n  \n").is_empty());
}

#[test]
fn test_parse_diagnostics_notes_collected() {
    let diags = parse_diagnostics(SAMPLE_E0382);
    // 第一条诊断应收集到附注（note / help）
    assert!(!diags[0].notes.is_empty(), "E0382 诊断应包含附注信息");
    // 验证附注内容包含关键词
    let all_notes = diags[0].notes.join(" ");
    assert!(
        all_notes.contains("cloning") || all_notes.contains("Copy"),
        "附注应包含 cloning 或 Copy 相关内容，实际: {}",
        all_notes
    );
}

/// 仅一条 error 无 warning 的输出
const SAMPLE_E0499: &str = "\
error[E0499]: cannot borrow `s` as mutable more than once at a time
 --> src/main.rs:6:14
  |
5 |     let r1 = &mut s;
  |              ------ first mutable borrow occurs here
6 |     let r2 = &mut s;
  |              ^^^^^^ second mutable borrow occurs here
7 |     println!(\"{} {}\", r1, r2);
  |                       -- first borrow later used here

error: aborting due to 1 previous error

For more information about this error, try `rustc --explain E0499`.
";

#[test]
fn test_parse_diagnostics_single_error_no_warning() {
    let diags = parse_diagnostics(SAMPLE_E0499);
    assert_eq!(diags.len(), 1, "应为 1 条诊断");
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(diags[0].code.as_deref(), Some("E0499"));
    assert_eq!(
        diags[0].message,
        "cannot borrow `s` as mutable more than once at a time"
    );
    let loc = diags[0].location.as_ref().expect("应有位置");
    assert_eq!(loc.line, 6);
    assert_eq!(loc.column, 14);
}

// ============================================================
// 集成测试：编译真实 fixture 后解析
// ============================================================

#[test]
fn test_parse_real_e0382_output() {
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("ownership/e0382.rs"), None)
        .expect("编译调用不应失败");
    assert!(!output.success, "E0382 fixture 应编译失败");

    let diags = parse_diagnostics(&output.stderr);
    // 至少有一条 E0382 错误
    let e0382 = diags
        .iter()
        .find(|d| d.code.as_deref() == Some("E0382"))
        .expect("应解析出 E0382 错误");
    assert_eq!(e0382.severity, Severity::Error);
    assert!(e0382.location.is_some(), "E0382 应有位置信息");
    // 位置文件路径应包含 fixture 文件名
    let loc = e0382.location.as_ref().unwrap();
    assert!(
        loc.file.contains("e0382.rs"),
        "位置文件应包含 e0382.rs，实际: {}",
        loc.file
    );
}

#[test]
fn test_parse_real_e0499_output() {
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("borrowing/e0499.rs"), None)
        .expect("编译调用不应失败");
    assert!(!output.success, "E0499 fixture 应编译失败");

    let diags = parse_diagnostics(&output.stderr);
    let e0499 = diags
        .iter()
        .find(|d| d.code.as_deref() == Some("E0499"))
        .expect("应解析出 E0499 错误");
    assert_eq!(e0499.severity, Severity::Error);
    assert!(e0499.location.is_some());
}

#[test]
fn test_parse_real_valid_no_errors() {
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("valid/hello.rs"), None)
        .expect("编译调用不应失败");
    assert!(output.success, "合法程序应编译成功");

    let diags = parse_diagnostics(&output.stderr);
    assert!(diags.is_empty(), "合法程序不应有诊断信息");
}
