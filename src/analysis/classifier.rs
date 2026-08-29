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
//! 未在表中映射的错误码返回 `None`，交由 LLM 后续推断。

use crate::analysis::error_parser::RustcDiagnostic;
use crate::models::{Diagnostic, ErrorCategory, KnowledgePoint};

// ============================================================
// 错误码 → 知识点
// ============================================================

/// 将 rustc 错误码（如 `"E0382"`）映射到知识点。
///
/// 返回 `None` 表示该错误码未在表中，需由 LLM 推断。
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
/// | E0283 | Result |
/// | E0554 | PatternMatching |
pub fn code_to_knowledge_point(code: &str) -> Option<KnowledgePoint> {
    // TODO: 实现错误码到知识点的映射
    //
    // 建议步骤：
    // 1. 用 match 匹配已知错误码
    // 2. 返回对应 KnowledgePoint
    // 3. 未知错误码返回 None
    //
    // 提示：参数 code 形如 "E0382"，可直接 match：
    //   match code {
    //       "E0382" => Some(KnowledgePoint::Ownership),
    //       "E0499" => Some(KnowledgePoint::Borrowing),
    //       ...
    //       _ => None,
    //   }
    todo!("实现错误码到知识点的映射表")
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
    // TODO: 实现编译诊断分类
    //
    // 建议步骤：
    // 1. category = ErrorCategory::CompileError
    // 2. 若 diag.code 为 Some(code)：
    //      调用 code_to_knowledge_point(code)
    //      映射成功 → knowledge_points = vec![kp], confidence = 0.95
    //      映射失败 → knowledge_points = vec![], confidence = 0.5
    //    否则（无错误码）：
    //      knowledge_points = vec![], confidence = 0.3
    // 3. 返回 Diagnostic { category, knowledge_points, confidence }
    todo!("实现编译诊断分类")
}

/// 对多条编译诊断批量分类。
///
/// 仅处理 [`crate::analysis::error_parser::Severity::Error`] 级别的诊断
/// （warning 通常不构成诊断目标），返回对应的 [`Diagnostic`] 列表。
pub fn classify_compile_diagnostics(diags: &[RustcDiagnostic]) -> Vec<Diagnostic> {
    // TODO: 实现批量分类
    //
    // 建议步骤：
    // 1. 遍历 diags
    // 2. 过滤出 severity == Error 的诊断（可跳过 warning）
    // 3. 对每条调用 classify_compile_diagnostic
    // 4. 收集为 Vec<Diagnostic> 返回
    //
    // 提示：也可选择保留 warning，由调用方决定。
    //       V1 默认只分类 error，warning 暂不产生 Diagnostic。
    todo!("实现批量编译诊断分类")
}

/// 根据测试失败结果分类，生成 [`Diagnostic`]。
///
/// 测试失败通常属于 [`ErrorCategory::LogicError`]。
/// 但具体知识点（如 Iterator / AlgorithmLogic）无法从输出确定性判断，
/// 需 LLM 后续补充，因此 knowledge_points 留空、confidence 较低。
///
/// 这是为第 7 步分层提示预留的接口。
pub fn classify_test_failure(name: &str, actual: &str, expected: &str) -> Diagnostic {
    // TODO: 实现测试失败分类
    //
    // 建议步骤：
    // 1. category = ErrorCategory::LogicError
    // 2. knowledge_points = vec![]（确定性逻辑无法判断，留待 LLM）
    // 3. confidence = 0.3
    // 4. （可选）把 name/actual/expected 记入 Diagnostic 的扩展字段
    //    当前 Diagnostic 结构无此字段，故仅返回分类信息
    let _ = (name, actual, expected);
    todo!("实现测试失败分类")
}
