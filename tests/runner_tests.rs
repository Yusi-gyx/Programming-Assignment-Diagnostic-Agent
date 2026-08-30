//! Runner / TestRunner 测试
//!
//! 验证程序运行与测试用例判定。
//! 完成 `src/tools/runner.rs` 中 TODO 后运行：
//!
//! ```bash
//! cargo test --test runner_tests
//! ```

use pada::tools::compiler::CompilerTool;
use pada::tools::runner::{Runner, TestCase, TestRunner};
use std::path::PathBuf;

fn fixture(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/rust");
    p.push(relative);
    p
}

/// 编译 io_program 到临时文件，返回其二进制路径。
///
/// 不同测试使用不同 `tag`，避免并行测试时输出路径冲突。
fn compile_io_program(tag: &str) -> PathBuf {
    let compiler = CompilerTool::new();
    let out = std::env::temp_dir().join(format!("pada_test_io_{}", tag));
    compiler
        .compile_file(&fixture("runner/io_program.rs"), Some(&out))
        .expect("编译 io_program 失败，请先实现 CompilerTool::compile_file");
    out
}

#[test]
fn test_run_program_with_input() {
    let program = compile_io_program("basic");
    let runner = Runner::new();

    // 1 + 2 + 3 = 6
    let output = runner
        .run(&program, "1\n2\n3\n\n")
        .expect("运行程序不应返回 IO 错误");

    assert!(output.success, "程序应正常退出");
    assert_eq!(output.stdout.trim(), "6");
}

#[test]
fn test_run_program_empty_input() {
    let program = compile_io_program("empty");
    let runner = Runner::new();

    // 空输入 -> sum = 0
    let output = runner.run(&program, "").unwrap();
    assert!(output.success);
    assert_eq!(output.stdout.trim(), "0");
}

#[test]
fn test_run_program_captures_exit_code() {
    let program = compile_io_program("exitcode");
    let runner = Runner::new();
    let output = runner.run(&program, "").unwrap();
    assert_eq!(output.exit_code, Some(0));
}

#[test]
fn test_run_tests_all_pass() {
    let program = compile_io_program("tests_pass");
    let test_runner = TestRunner::new();

    let tests = vec![
        TestCase::new("sum_1_to_3", "1\n2\n3\n\n", "6"),
        TestCase::new("sum_negatives", "-1\n-2\n-3\n\n", "-6"),
        TestCase::new("sum_zeros", "0\n0\n0\n\n", "0"),
    ];

    let results = test_runner
        .run_tests(&program, &tests)
        .expect("运行测试不应返回 IO 错误");

    assert_eq!(results.len(), 3);
    for (i, r) in results.iter().enumerate() {
        assert!(r.passed, "用例 {} ({}) 应通过", i, r.name);
    }
}

#[test]
fn test_run_tests_with_failure() {
    let program = compile_io_program("tests_fail");
    let test_runner = TestRunner::new();

    let tests = vec![
        TestCase::new("correct", "1\n2\n\n", "3"),
        TestCase::new("wrong_expected", "1\n2\n\n", "100"), // 期望错误
    ];

    let results = test_runner.run_tests(&program, &tests).unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].passed, "正确用例应通过");
    assert!(!results[1].passed, "错误用例应失败");
    assert_eq!(results[1].actual_output.trim(), "3");
    assert_eq!(results[1].expected_output, "100");
}

#[test]
fn test_run_nonexistent_program() {
    let runner = Runner::new();
    let result = runner.run(std::path::Path::new("/nonexistent/pada_program"), "");
    assert!(result.is_err(), "运行不存在的程序应返回错误");
}
