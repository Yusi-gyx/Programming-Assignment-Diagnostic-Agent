//! 分层提示测试
//!
//! 可单独运行：
//!
//! ```bash
//! cargo test --test hint_tests
//! ```

use pada::analysis::classifier::classify_compile_diagnostics;
use pada::analysis::error_parser::{RustcDiagnostic, Severity, SourceLocation, parse_diagnostics};
use pada::analysis::hint::{
    code_to_direction, error_category_text, format_location, generate_compile_hint,
    generate_test_hint, hint_level_as_number, hint_level_from_number, knowledge_point_text,
    next_hint_level,
};
use pada::models::{Diagnostic, ErrorCategory, HintLevel, KnowledgePoint};
use pada::tools::compiler::CompilerTool;
use std::path::PathBuf;

fn fixture(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/rust");
    p.push(relative);
    p
}

/// 构造一条编译诊断用于测试
fn make_diag(code: Option<&str>, loc: Option<SourceLocation>) -> RustcDiagnostic {
    RustcDiagnostic {
        severity: Severity::Error,
        code: code.map(String::from),
        message: "test error".into(),
        location: loc,
        notes: vec![],
    }
}

fn make_classified(category: ErrorCategory, kps: Vec<KnowledgePoint>) -> Diagnostic {
    Diagnostic {
        category,
        knowledge_points: kps,
        confidence: 0.95,
    }
}

// ============================================================
// 提示级别控制测试（已实现，应直接通过）
// ============================================================

#[test]
fn test_next_hint_level() {
    assert_eq!(
        next_hint_level(HintLevel::Category),
        Some(HintLevel::Location)
    );
    assert_eq!(
        next_hint_level(HintLevel::Location),
        Some(HintLevel::Concept)
    );
    assert_eq!(
        next_hint_level(HintLevel::Concept),
        Some(HintLevel::Direction)
    );
    assert_eq!(
        next_hint_level(HintLevel::Direction),
        Some(HintLevel::Solution)
    );
    assert_eq!(next_hint_level(HintLevel::Solution), None);
}

#[test]
fn test_hint_level_number_roundtrip() {
    for n in 1..=5u8 {
        let level = hint_level_from_number(n).expect("1-5 应有效");
        assert_eq!(hint_level_as_number(level), n);
    }
}

#[test]
fn test_hint_level_from_number_invalid() {
    assert!(hint_level_from_number(0).is_none());
    assert!(hint_level_from_number(6).is_none());
}

// ============================================================
// 辅助函数测试（已实现，应直接通过）
// ============================================================

#[test]
fn test_error_category_text() {
    assert_eq!(error_category_text(ErrorCategory::CompileError), "编译错误");
    assert_eq!(error_category_text(ErrorCategory::LogicError), "逻辑错误");
}

#[test]
fn test_knowledge_point_text() {
    assert_eq!(
        knowledge_point_text(KnowledgePoint::Ownership),
        "所有权 / Move"
    );
    assert_eq!(
        knowledge_point_text(KnowledgePoint::Borrowing),
        "借用 / Borrow"
    );
}

#[test]
fn test_format_location() {
    let loc = SourceLocation {
        file: "src/main.rs".into(),
        line: 6,
        column: 20,
    };
    assert_eq!(format_location(&loc), "src/main.rs:6:20");
}

// ============================================================
// code_to_direction 测试
// ============================================================

#[test]
fn test_code_to_direction_known() {
    assert!(code_to_direction("E0382").is_some());
    assert!(code_to_direction("E0499").is_some());
    assert!(code_to_direction("E0277").is_some());
}

#[test]
fn test_code_to_direction_unknown() {
    assert!(code_to_direction("E9999").is_none());
}

#[test]
fn test_code_to_direction_content_meaningful() {
    let dir = code_to_direction("E0382").expect("E0382 应有方向");
    assert!(
        dir.contains("克隆") || dir.contains("所有权") || dir.contains("移动"),
        "E0382 方向应与所有权/克隆/移动相关，实际: {}",
        dir
    );
}

// ============================================================
// generate_compile_hint 各级别测试
// ============================================================

#[test]
fn test_compile_hint_category() {
    let diag = make_diag(Some("E0382"), None);
    let classified = make_classified(ErrorCategory::CompileError, vec![]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Category);

    assert_eq!(hint.level, HintLevel::Category);
    assert!(
        hint.content.contains("编译错误"),
        "Level 1 应包含错误类别，实际: {}",
        hint.content
    );
}

#[test]
fn test_compile_hint_location() {
    let diag = make_diag(
        Some("E0382"),
        Some(SourceLocation {
            file: "main.rs".into(),
            line: 7,
            column: 5,
        }),
    );
    let classified = make_classified(ErrorCategory::CompileError, vec![]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Location);

    assert_eq!(hint.level, HintLevel::Location);
    assert!(
        hint.content.contains("main.rs") && hint.content.contains("7"),
        "Level 2 应包含文件与行号，实际: {}",
        hint.content
    );
}

#[test]
fn test_compile_hint_location_missing() {
    let diag = make_diag(Some("E0382"), None);
    let classified = make_classified(ErrorCategory::CompileError, vec![]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Location);
    // 位置缺失时不应 panic，应给出提示
    assert!(!hint.content.is_empty());
}

#[test]
fn test_compile_hint_concept_with_kp() {
    let diag = make_diag(Some("E0382"), None);
    let classified = make_classified(ErrorCategory::CompileError, vec![KnowledgePoint::Ownership]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Concept);

    assert_eq!(hint.level, HintLevel::Concept);
    assert!(
        hint.content.contains("所有权") || hint.content.contains("Ownership"),
        "Level 3 应包含知识点，实际: {}",
        hint.content
    );
}

#[test]
fn test_compile_hint_concept_empty() {
    let diag = make_diag(Some("E9999"), None);
    let classified = make_classified(ErrorCategory::CompileError, vec![]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Concept);
    // 无知识点时不应 panic
    assert!(!hint.content.is_empty());
}

#[test]
fn test_compile_hint_direction_known_code() {
    let diag = make_diag(Some("E0382"), None);
    let classified = make_classified(ErrorCategory::CompileError, vec![]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Direction);

    assert_eq!(hint.level, HintLevel::Direction);
    // E0382 有已知方向，内容应非空且有意义
    assert!(
        !hint.content.is_empty() && hint.content.len() > 4,
        "Level 4 已知码应有方向，实际: {}",
        hint.content
    );
}

#[test]
fn test_compile_hint_solution() {
    let diag = make_diag(Some("E0382"), None);
    let classified = make_classified(ErrorCategory::CompileError, vec![]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Solution);

    assert_eq!(hint.level, HintLevel::Solution);
    assert!(
        hint.content.contains("config"),
        "未配置模型时应说明配置方式"
    );
}

// ============================================================
// generate_test_hint 各级别测试
// ============================================================

#[test]
fn test_test_hint_category() {
    let hint = generate_test_hint("case_1", "9", "6", HintLevel::Category);
    assert_eq!(hint.level, HintLevel::Category);
    assert!(
        hint.content.contains("逻辑错误") || hint.content.contains("测试"),
        "测试失败 Level 1 应提及逻辑错误或测试，实际: {}",
        hint.content
    );
}

#[test]
fn test_test_hint_location() {
    let hint = generate_test_hint("case_03", "9", "6", HintLevel::Location);
    assert_eq!(hint.level, HintLevel::Location);
    assert!(
        hint.content.contains("case_03"),
        "Level 2 应包含用例名，实际: {}",
        hint.content
    );
}

#[test]
fn test_test_hint_concept() {
    let hint = generate_test_hint("case_1", "9", "6", HintLevel::Concept);
    assert_eq!(hint.level, HintLevel::Concept);
    assert!(!hint.content.is_empty(), "Level 3 应有提示");
}

#[test]
fn test_test_hint_direction() {
    let hint = generate_test_hint("case_1", "9", "6", HintLevel::Direction);
    assert_eq!(hint.level, HintLevel::Direction);
    assert!(
        hint.content.contains("6") && hint.content.contains("9"),
        "Level 4 应包含期望与实际输出，实际: {}",
        hint.content
    );
}

#[test]
fn test_test_hint_solution() {
    let hint = generate_test_hint("case_1", "9", "6", HintLevel::Solution);
    assert_eq!(hint.level, HintLevel::Solution);
    assert!(
        hint.content.contains("config"),
        "未配置模型时应说明配置方式"
    );
}

// ============================================================
// 级别递进一致性测试
// ============================================================

#[test]
fn test_compile_hint_levels_progress() {
    // 从 Level 1 逐步升级到 Level 5，每级都应有内容
    let diag = make_diag(
        Some("E0382"),
        Some(SourceLocation {
            file: "main.rs".into(),
            line: 7,
            column: 5,
        }),
    );
    let classified = make_classified(ErrorCategory::CompileError, vec![KnowledgePoint::Ownership]);

    let mut level = HintLevel::Category;
    loop {
        let hint = generate_compile_hint(&diag, &classified, level);
        assert!(!hint.content.is_empty(), "Level {:?} 提示不应为空", level);
        assert_eq!(hint.level, level);
        match next_hint_level(level) {
            Some(next) => level = next,
            None => break,
        }
    }
}

// ============================================================
// 集成测试：编译 → 解析 → 分类 → 生成提示
// ============================================================

#[test]
fn test_hint_real_e0382_full_flow() {
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("ownership/e0382.rs"), None)
        .unwrap();
    let parsed = parse_diagnostics(&output.stderr);
    let diagnostics = classify_compile_diagnostics(&parsed);
    assert!(!diagnostics.is_empty(), "应有诊断结果");

    // 找到 E0382 诊断并生成各级提示
    let e0382_parsed = parsed
        .iter()
        .find(|d| d.code.as_deref() == Some("E0382"))
        .expect("应解析出 E0382");
    let e0382_classified = diagnostics
        .iter()
        .find(|d| d.knowledge_points.contains(&KnowledgePoint::Ownership))
        .expect("应分类出 Ownership");

    // Level 3 应包含「所有权」
    let hint = generate_compile_hint(e0382_parsed, e0382_classified, HintLevel::Concept);
    assert!(
        hint.content.contains("所有权") || hint.content.contains("Ownership"),
        "E0382 Level 3 应包含所有权知识点，实际: {}",
        hint.content
    );
}
