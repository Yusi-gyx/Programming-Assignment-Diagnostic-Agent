//! 错误分类与知识点映射测试
//!
//! 完成 `src/analysis/classifier.rs` 中 TODO 后运行：
//!
//! ```bash
//! cargo test --test classifier_tests
//! ```

use pada::analysis::classifier::{
    classify_compile_diagnostic, classify_compile_diagnostics, classify_test_failure,
    code_to_knowledge_point,
};
use pada::analysis::error_parser::{RustcDiagnostic, Severity, parse_diagnostics};
use pada::models::{ErrorCategory, KnowledgePoint};
use pada::tools::compiler::CompilerTool;
use std::path::PathBuf;

fn fixture(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/rust");
    p.push(relative);
    p
}

/// 构造一条编译诊断用于测试
fn make_diag(code: Option<&str>, severity: Severity) -> RustcDiagnostic {
    RustcDiagnostic {
        severity,
        code: code.map(String::from),
        message: "test message".into(),
        location: None,
        notes: vec![],
    }
}

// ============================================================
// code_to_knowledge_point 单元测试
// ============================================================

#[test]
fn test_code_to_knowledge_ownership() {
    assert_eq!(
        code_to_knowledge_point("E0382"),
        Some(KnowledgePoint::Ownership)
    );
}

#[test]
fn test_code_to_knowledge_borrowing() {
    assert_eq!(
        code_to_knowledge_point("E0499"),
        Some(KnowledgePoint::Borrowing)
    );
    assert_eq!(
        code_to_knowledge_point("E0502"),
        Some(KnowledgePoint::Borrowing)
    );
}

#[test]
fn test_code_to_knowledge_lifetime() {
    assert_eq!(
        code_to_knowledge_point("E0106"),
        Some(KnowledgePoint::Lifetime)
    );
    assert_eq!(
        code_to_knowledge_point("E0597"),
        Some(KnowledgePoint::Lifetime)
    );
}

#[test]
fn test_code_to_knowledge_trait() {
    assert_eq!(
        code_to_knowledge_point("E0277"),
        Some(KnowledgePoint::Trait)
    );
}

#[test]
fn test_code_to_knowledge_unknown() {
    assert!(code_to_knowledge_point("E9999").is_none());
    assert!(code_to_knowledge_point("E0000").is_none());
}

// ============================================================
// classify_compile_diagnostic 单元测试
// ============================================================

#[test]
fn test_classify_with_known_code() {
    // E0382 → Ownership，高置信度
    let diag = make_diag(Some("E0382"), Severity::Error);
    let result = classify_compile_diagnostic(&diag);
    assert_eq!(result.category, ErrorCategory::CompileError);
    assert_eq!(result.knowledge_points, vec![KnowledgePoint::Ownership]);
    assert!(
        result.confidence >= 0.9,
        "已知映射应高置信度，实际: {}",
        result.confidence
    );
}

#[test]
fn test_classify_with_unknown_code() {
    // 未知错误码 → 空知识点，中置信度
    let diag = make_diag(Some("E9999"), Severity::Error);
    let result = classify_compile_diagnostic(&diag);
    assert_eq!(result.category, ErrorCategory::CompileError);
    assert!(result.knowledge_points.is_empty());
    assert!(
        result.confidence < 0.9 && result.confidence > 0.0,
        "未知错误码应中置信度，实际: {}",
        result.confidence
    );
}

#[test]
fn test_classify_without_code() {
    // 无错误码 → 空知识点，低置信度
    let diag = make_diag(None, Severity::Error);
    let result = classify_compile_diagnostic(&diag);
    assert_eq!(result.category, ErrorCategory::CompileError);
    assert!(result.knowledge_points.is_empty());
    assert!(
        result.confidence <= 0.5,
        "无错误码应低置信度，实际: {}",
        result.confidence
    );
}

#[test]
fn test_classify_confidence_ordering() {
    // 置信度排序：已知码 > 未知码 > 无码
    let known = classify_compile_diagnostic(&make_diag(Some("E0382"), Severity::Error));
    let unknown = classify_compile_diagnostic(&make_diag(Some("E9999"), Severity::Error));
    let none = classify_compile_diagnostic(&make_diag(None, Severity::Error));
    assert!(known.confidence > unknown.confidence);
    assert!(unknown.confidence > none.confidence);
}

// ============================================================
// classify_compile_diagnostics 批量测试
// ============================================================

#[test]
fn test_classify_batch_filters_warnings() {
    // 批量分类应只处理 error，跳过 warning
    let diags = vec![
        make_diag(Some("E0382"), Severity::Error),
        make_diag(None, Severity::Warning), // 应被跳过
        make_diag(Some("E0499"), Severity::Error),
    ];
    let results = classify_compile_diagnostics(&diags);
    assert_eq!(results.len(), 2, "应只分类 2 条 error（跳过 warning）");
    assert_eq!(results[0].knowledge_points, vec![KnowledgePoint::Ownership]);
    assert_eq!(results[1].knowledge_points, vec![KnowledgePoint::Borrowing]);
}

#[test]
fn test_classify_batch_empty() {
    assert!(classify_compile_diagnostics(&[]).is_empty());
}

// ============================================================
// classify_test_failure 测试
// ============================================================

#[test]
fn test_classify_test_failure() {
    let result = classify_test_failure("case_1", "9", "6");
    assert_eq!(result.category, ErrorCategory::LogicError);
    assert!(
        result.knowledge_points.is_empty(),
        "测试失败的知识点应由 LLM 后续补充"
    );
}

// ============================================================
// 集成测试：编译 → 解析 → 分类
// ============================================================

#[test]
fn test_classify_real_e0382() {
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("ownership/e0382.rs"), None)
        .unwrap();
    let parsed = parse_diagnostics(&output.stderr);
    let diagnostics = classify_compile_diagnostics(&parsed);

    // 应至少有一条 Ownership 诊断
    let ownership = diagnostics
        .iter()
        .find(|d| d.knowledge_points.contains(&KnowledgePoint::Ownership));
    assert!(ownership.is_some(), "E0382 应映射到 Ownership");
    assert_eq!(ownership.unwrap().category, ErrorCategory::CompileError);
}

#[test]
fn test_classify_real_e0499() {
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("borrowing/e0499.rs"), None)
        .unwrap();
    let parsed = parse_diagnostics(&output.stderr);
    let diagnostics = classify_compile_diagnostics(&parsed);

    let borrowing = diagnostics
        .iter()
        .find(|d| d.knowledge_points.contains(&KnowledgePoint::Borrowing));
    assert!(borrowing.is_some(), "E0499 应映射到 Borrowing");
}
