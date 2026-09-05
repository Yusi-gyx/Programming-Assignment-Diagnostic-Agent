//! 错误分类与知识点映射
//!
//! 职责（开发计划第 6 步）：
//! - 将 rustc 错误码映射到 [`KnowledgePoint`]
//! - 对编译诊断分类为 [`ErrorCategory`]
//! - 生成 [`Diagnostic`] 结构（供第 7 步分层提示使用）
//!
//! 设计原则（AGENTS.md）：错误分类与知识点映射属于确定性逻辑，
//! 由 Rust 完成，不依赖 LLM。LLM 仅在后续把结果自然语言化。
//!
//! # 错误码 → 知识点映射依据
//!
//! rustc 错误码与知识点的对应是相对稳定的（DESIGN.md §4.1）：
//!
//! | 错误码 | 知识点 |
//! |--------|--------|
//! | E0382  | Ownership（使用了已移动的值）|
//! | E0499  | Borrowing（多次可变借用）|
//! | E0502  | Borrowing（借用冲突）|
//! | E0106  | Lifetime（缺少生命周期标注）|
//! | E0597  | Lifetime（生命周期不够长）|
//! | E0277  | Trait（trait bound 未满足）|
//!
//! 未在表中映射的错误码返回 `None`；默认保持待分析，上层可按提示等级选择模型增强。

use crate::analysis::error_parser::RustcDiagnostic;
use crate::models::{Diagnostic, ErrorCategory, KnowledgePoint, TestResult};

// ============================================================
// 错误码 → 知识点
// ============================================================

/// 将 rustc 错误码（如 `"E0382"`）映射到知识点。
///
/// 返回 `None` 表示该错误码未在静态表中。
///
/// # 已知映射（DESIGN.md §4.1 + 常见 rustc 错误）
///
/// | 错误码 | 知识点 |
/// |--------|--------|
/// | E0382 | Ownership |
/// | E0507 | Ownership |
/// | E0505 | Borrowing |
/// | E0499 | Borrowing |
/// | E0502 | Borrowing |
/// | E0500 | Borrowing |
/// | E0106 | Lifetime |
/// | E0597 | Lifetime |
/// | E0277 | Trait |
/// | E0404 | Trait |
/// | E0243 | Generic |
/// | E0283 | TypeSystem |
/// | E0554 | Syntax |
pub fn code_to_knowledge_point(code: &str) -> Option<KnowledgePoint> {
    match code {
        "E0308" | "E0282" | "E0284" | "E0605" | "E0606" | "E0369" | "E0271" => {
            Some(KnowledgePoint::TypeSystem)
        }
        "E0425" | "E0432" | "E0433" | "E0412" | "E0603" | "E0583" => {
            Some(KnowledgePoint::NameResolution)
        }
        "E0599" | "E0046" | "E0119" => Some(KnowledgePoint::Trait),
        "E0004" | "E0005" => Some(KnowledgePoint::PatternMatching),
        "E0107" | "E0207" => Some(KnowledgePoint::Generic),
        "E0515" | "E0716" => Some(KnowledgePoint::Lifetime),
        "E0596" | "E0506" => Some(KnowledgePoint::Borrowing),
        "E0382" => Some(KnowledgePoint::Ownership),
        "E0507" => Some(KnowledgePoint::Ownership),
        "E0505" => Some(KnowledgePoint::Borrowing),
        "E0499" => Some(KnowledgePoint::Borrowing),
        "E0502" => Some(KnowledgePoint::Borrowing),
        "E0500" => Some(KnowledgePoint::Borrowing),
        "E0106" => Some(KnowledgePoint::Lifetime),
        "E0597" => Some(KnowledgePoint::Lifetime),
        "E0277" => Some(KnowledgePoint::Trait),
        "E0404" => Some(KnowledgePoint::Trait),
        "E0243" => Some(KnowledgePoint::Generic),
        "E0283" => Some(KnowledgePoint::TypeSystem),
        "E0554" => Some(KnowledgePoint::Syntax),
        _ => None,
    }
}

// ============================================================
// 分类
// ============================================================

/// 对单条编译诊断分类，生成 [`Diagnostic`]。
///
/// 分类规则：
/// - 来自 rustc 的诊断（无论 error / warning）均为 [`ErrorCategory::CompileError`]
/// - 通过 [`code_to_knowledge_point`] 映射知识点；未映射则为空
/// - 置信度（confidence）启发式：
///   - 有错误码且已映射知识点：高置信度（如 0.95，编译器是权威）
///   - 有错误码但未映射：中置信度（如 0.5，需 LLM 补充）
///   - 无错误码：低置信度（如 0.3，需 LLM 判断）
pub fn classify_compile_diagnostic(diag: &RustcDiagnostic) -> Diagnostic {
    let category = ErrorCategory::CompileError;
    let mut knowledge_points: Vec<KnowledgePoint> = Vec::new();
    let mut confidence: f32 = 0.95;
    match diag.code.as_ref() {
        Some(code) => match code_to_knowledge_point(code) {
            Some(knowledge_point) => {
                knowledge_points.push(knowledge_point);
            }
            None => {
                confidence = 0.5;
            }
        },
        None => {
            confidence = 0.3;
        }
    }
    if knowledge_points.is_empty() {
        let message = diag.message.to_lowercase();
        let point = if message.contains("mismatched types")
            || message.contains("type annotations needed")
        {
            Some(KnowledgePoint::TypeSystem)
        } else if message.contains("expected")
            || message.contains("delimiter")
            || message.contains("unexpected token")
        {
            Some(KnowledgePoint::Syntax)
        } else if message.contains("cannot find") || message.contains("unresolved import") {
            Some(KnowledgePoint::NameResolution)
        } else {
            None
        };
        if let Some(point) = point {
            knowledge_points.push(point);
            confidence = 0.75;
        }
    }
    Diagnostic {
        category,
        knowledge_points,
        confidence,
    }
}

/// 对多条编译诊断批量分类。
///
/// 仅处理 [`crate::analysis::error_parser::Severity::Error`] 级别的诊断
/// （warning 通常不构成诊断目标），返回对应的 [`Diagnostic`] 列表。
pub fn classify_compile_diagnostics(diags: &[RustcDiagnostic]) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for diag in diags.iter() {
        if diag.severity == crate::analysis::error_parser::Severity::Warning {
            continue;
        }
        diagnostics.push(classify_compile_diagnostic(diag));
    }
    diagnostics
}

/// 根据测试失败结果分类，生成 [`Diagnostic`]。
///
/// 测试失败通常属于 [`ErrorCategory::LogicError`]。
/// 但具体知识点（如 Iterator / AlgorithmLogic）无法从输出确定性判断，
/// 需 LLM 后续补充，因此 knowledge_points 留空、confidence 较低。
///
/// 这是为第 7 步分层提示预留的接口。
pub fn classify_test_failure(name: &str, actual: &str, expected: &str) -> Diagnostic {
    let _ = (name, actual, expected);
    Diagnostic {
        category: ErrorCategory::LogicError,
        knowledge_points: vec![],
        confidence: 0.3,
    }
}

/// 根据执行状态确定性地区分运行时错误和输出不匹配。
pub fn classify_test_result(result: &TestResult) -> Diagnostic {
    if result.runtime_error.is_some() {
        Diagnostic {
            category: ErrorCategory::RuntimeError,
            knowledge_points: vec![KnowledgePoint::ErrorHandling],
            confidence: 0.9,
        }
    } else {
        classify_test_failure(&result.name, &result.actual_output, &result.expected_output)
    }
}
