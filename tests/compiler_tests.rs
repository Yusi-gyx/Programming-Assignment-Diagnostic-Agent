//! CompilerTool 测试
//!
//! 验证编译工具能正确调用 rustc 并捕获输出。
//! 可单独运行：
//!
//! ```bash
//! cargo test --test compiler_tests
//! ```

use pada::tools::compiler::CompilerTool;
use std::path::PathBuf;

/// 获取测试 fixture 的绝对路径
///
/// `env!("CARGO_MANIFEST_DIR")` 指向项目根目录，
/// 确保测试无论在何处运行都能找到 fixtures。
fn fixture(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/rust");
    p.push(relative);
    p
}

#[test]
fn test_compile_valid_file() {
    // 合法程序应编译成功，且 stderr 为空
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("valid/hello.rs"), None)
        .expect("编译调用不应返回 IO 错误");

    assert!(output.success, "合法程序应编译成功");
    assert!(output.stderr.is_empty(), "合法程序不应有 stderr 输出");
}

#[test]
fn test_compile_ownership_error() {
    // E0382: borrow of moved value（所有权 / 移动）
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("ownership/e0382.rs"), None)
        .expect("编译调用不应返回 IO 错误");

    assert!(!output.success, "错误程序应编译失败");
    assert!(
        output.stderr.contains("E0382"),
        "stderr 应包含错误码 E0382，实际输出:\n{}",
        output.stderr
    );
}

#[test]
fn test_compile_borrowing_error() {
    // E0499: cannot borrow as mutable more than once（可变借用冲突）
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("borrowing/e0499.rs"), None)
        .expect("编译调用不应返回 IO 错误");

    assert!(!output.success, "错误程序应编译失败");
    assert!(
        output.stderr.contains("E0499") || output.stderr.contains("E0502"),
        "stderr 应包含借用冲突错误码，实际输出:\n{}",
        output.stderr
    );
}

#[test]
fn test_cargo_check_multifile_project_with_type_error() {
    // 完整 Cargo 项目的错误位于独立模块中，应由 cargo check 捕获。
    let compiler = CompilerTool::new();
    let output = compiler
        .cargo_check(&fixture("cargo/type_mismatch_project"))
        .expect("cargo check 调用不应返回 IO 错误");

    assert!(!output.success, "有类型错误的 Cargo 项目应检查失败");
    assert!(
        output.stderr.contains("E0308") && output.stderr.contains("src/grade.rs"),
        "stderr 应包含 grade.rs 中的 E0308，实际输出:\n{}",
        output.stderr
    );
}

#[test]
fn test_compile_output_captures_exit_code() {
    // 成功编译退出码应为 0
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("valid/hello.rs"), None)
        .unwrap();

    assert_eq!(output.exit_code, Some(0));
}

#[test]
fn test_compile_nonexistent_file() {
    // 不存在的文件应返回 Err（而非 panic）
    let compiler = CompilerTool::new();
    let result = compiler.compile_file(&fixture("nonexistent.rs"), None);
    assert!(result.is_err(), "不存在的文件应返回错误");
}
