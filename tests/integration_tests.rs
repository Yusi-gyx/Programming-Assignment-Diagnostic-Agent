//! 集成测试
//!
//! 验证 编译 → 运行 → 测试 的完整流程，
//! 以及核心数据结构能正常构造与使用。
//!
//! 运行：
//! ```bash
//! cargo test --test integration_tests
//! ```

use PADA::models::{Assignment, Diagnostic, ErrorCategory, HintLevel, KnowledgePoint, Submission};
use PADA::tools::compiler::CompilerTool;
use PADA::tools::runner::{TestCase, TestRunner};
use std::path::PathBuf;

fn fixture(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/rust");
    p.push(relative);
    p
}

#[test]
fn test_data_structures_basic_usage() {
    // 验证核心数据结构能正常构造与使用（不依赖任何 TODO 实现）
    let assignment = Assignment {
        title: "求和".into(),
        description: "读取整数并求和".into(),
    };
    assert_eq!(assignment.title, "求和");

    let submission = Submission {
        source_code: "fn main() {}".into(),
        test_results: vec![],
    };
    assert!(submission.test_results.is_empty());

    let diagnostic = Diagnostic {
        category: ErrorCategory::CompileError,
        knowledge_points: vec![KnowledgePoint::Ownership],
        confidence: 0.9,
    };
    assert_eq!(diagnostic.category, ErrorCategory::CompileError);
    assert_eq!(diagnostic.knowledge_points, vec![KnowledgePoint::Ownership]);

    let hint = HintLevel::Category;
    assert_eq!(hint, HintLevel::Category);
}

#[test]
fn test_full_flow_valid_program() {
    // 完整流程：编译合法程序 → 运行测试 → 全部通过
    let compiler = CompilerTool::new();
    let program = std::env::temp_dir().join("pada_integration_valid");
    let compile_out = compiler
        .compile_file(&fixture("valid/hello.rs"), Some(&program))
        .unwrap();
    assert!(compile_out.success, "合法程序应编译成功");

    let test_runner = TestRunner::new();
    let tests = vec![TestCase::new("hello", "", "Hello, world!")];
    let results = test_runner.run_tests(&program, &tests).unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].passed, "hello 用例应通过");
    assert_eq!(results[0].actual_output.trim(), "Hello, world!");
}

#[test]
fn test_compile_then_run_workflow() {
    // 编译运行型程序 → 执行测试用例 → 验证结果一致性
    let compiler = CompilerTool::new();
    let program = std::env::temp_dir().join("pada_integration_io");
    compiler
        .compile_file(&fixture("runner/io_program.rs"), Some(&program))
        .unwrap();

    let test_runner = TestRunner::new();
    let tests = vec![
        TestCase::new("case_a", "10\n20\n\n", "30"),
        TestCase::new("case_b", "5\n5\n5\n\n", "15"),
    ];
    let results = test_runner.run_tests(&program, &tests).unwrap();
    assert!(results.iter().all(|r| r.passed), "全部用例应通过");
}
