//! 分层提示生成
//!
//! 职责（开发计划第 7 步）：
//! - 根据诊断结果与用户选择的 [`HintLevel`] 生成对应级别的提示
//! - 提供提示级别控制（升级 / 数值转换）
//!
//! 设计原则（AGENTS.md）：
//! - 提示级别控制是确定性逻辑，由 Rust 完成
//! - Level 1-3（类别 / 位置 / 知识点）完全由程序从诊断数据中提取
//! - Level 4（修改方向）由程序基于错误码给出通用方向
//! - Level 5（参考方案）在上层配置模型后由 LLM 生成；未配置时给出配置指引
//!
//! # 提示级别（DESIGN.md §4.3）
//!
//! | 级别 | 名称     | 数据来源            |
//! |------|----------|---------------------|
//! | 1    | Category | 错误类别            |
//! | 2    | Location | 错误位置            |
//! | 3    | Concept  | 相关知识点          |
//! | 4    | Direction| 修改方向            |
//! | 5    | Solution | 参考方案            |

use crate::analysis::error_parser::{RustcDiagnostic, SourceLocation};
use crate::models::{Diagnostic, ErrorCategory, HintLevel, KnowledgePoint};

// ============================================================
// 提示级别控制（已实现，供上层调用）
// ============================================================

/// 升级到下一级提示。
///
/// `Solution` 已是最高级，返回 `None`。
///
/// 对应交互命令 `\next`（DESIGN.md §5）。
pub fn next_hint_level(level: HintLevel) -> Option<HintLevel> {
    match level {
        HintLevel::Category => Some(HintLevel::Location),
        HintLevel::Location => Some(HintLevel::Concept),
        HintLevel::Concept => Some(HintLevel::Direction),
        HintLevel::Direction => Some(HintLevel::Solution),
        HintLevel::Solution => None,
    }
}

/// 将 [`HintLevel`] 转为数值 1-5（对应 `--hint <level>` 参数）。
pub fn hint_level_as_number(level: HintLevel) -> u8 {
    match level {
        HintLevel::Category => 1,
        HintLevel::Location => 2,
        HintLevel::Concept => 3,
        HintLevel::Direction => 4,
        HintLevel::Solution => 5,
    }
}

/// 将数值 1-5 转为 [`HintLevel`]，越界返回 `None`。
pub fn hint_level_from_number(n: u8) -> Option<HintLevel> {
    match n {
        1 => Some(HintLevel::Category),
        2 => Some(HintLevel::Location),
        3 => Some(HintLevel::Concept),
        4 => Some(HintLevel::Direction),
        5 => Some(HintLevel::Solution),
        _ => None,
    }
}

// ============================================================
// 提示内容
// ============================================================

/// 一条分层提示
#[derive(Debug, Clone)]
pub struct Hint {
    /// 该提示对应的级别
    pub level: HintLevel,
    /// 提示文本
    pub content: String,
}

impl Hint {
    pub fn new(level: HintLevel, content: impl Into<String>) -> Self {
        Self {
            level,
            content: content.into(),
        }
    }
}

/// 将错误类别转为中文描述（供 Level 1 使用）。
pub fn error_category_text(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::CompileError => "编译错误",
        ErrorCategory::RuntimeError => "运行时错误",
        ErrorCategory::LogicError => "逻辑错误",
        ErrorCategory::BoundaryCondition => "边界条件错误",
        ErrorCategory::AlgorithmError => "算法错误",
    }
}

/// 将知识点转为中文描述（供 Level 3 使用）。
pub fn knowledge_point_text(point: KnowledgePoint) -> &'static str {
    match point {
        KnowledgePoint::Ownership => "所有权 / Move",
        KnowledgePoint::Borrowing => "借用 / Borrow",
        KnowledgePoint::Lifetime => "生命周期 / Lifetime",
        KnowledgePoint::Trait => "Trait",
        KnowledgePoint::Generic => "泛型 / Generic",
        KnowledgePoint::Iterator => "迭代器 / Iterator",
        KnowledgePoint::Option => "Option",
        KnowledgePoint::Result => "Result",
        KnowledgePoint::PatternMatching => "模式匹配",
        KnowledgePoint::Collection => "集合 / Collection",
        KnowledgePoint::ErrorHandling => "错误处理",
        KnowledgePoint::AlgorithmLogic => "算法逻辑",
    }
}

/// 将位置格式化为 `file:line:col` 字符串（供 Level 2 使用）。
pub fn format_location(loc: &SourceLocation) -> String {
    format!("{}:{}:{}", loc.file, loc.line, loc.column)
}

// ============================================================
// 编译错误提示（核心逻辑，TODO）
// ============================================================

/// 为编译诊断生成指定级别的提示。
///
/// 各级别内容来源：
/// - **Category (1)**：来自 `classified.category`（如「这是一个编译错误」）
/// - **Location (2)**：来自 `diag.location`（如「位置：file:line:col」）
/// - **Concept (3)**：来自 `classified.knowledge_points`（如「知识点：所有权 / Move」）
/// - **Direction (4)**：来自 [`code_to_direction`] 基于错误码的通用方向
/// - **Solution (5)**：先返回模型配置指引，上层配置模型时替换为实际生成内容
pub fn generate_compile_hint(
    diag: &RustcDiagnostic,
    classified: &Diagnostic,
    level: HintLevel,
) -> Hint {
    // TODO: 实现编译诊断分层提示
    //
    // 建议步骤：按 level 分支
    //
    // HintLevel::Category =>
    //   Hint::new(level, format!("这是一个{}", error_category_text(classified.category)))
    //
    // HintLevel::Location =>
    //   match &diag.location {
    //       Some(loc) => Hint::new(level, format!("位置：{}", format_location(loc))),
    //       None => Hint::new(level, "位置信息缺失".to_string()),
    //   }
    //
    // HintLevel::Concept =>
    //   if classified.knowledge_points.is_empty() {
    //       Hint::new(level, "知识点待分析（无明确映射）".to_string())
    //   } else {
    //       let kps: Vec<_> = classified.knowledge_points
    //           .iter().map(|k| knowledge_point_text(*k)).collect();
    //       Hint::new(level, format!("知识点：{}", kps.join("、")))
    //   }
    //
    // HintLevel::Direction =>
    //   match &diag.code {
    //       Some(code) => match code_to_direction(code) {
    //           Some(dir) => Hint::new(level, format!("修改方向：{}", dir)),
    //           None => Hint::new(level, "修改方向待分析".to_string()),
    //       },
    //       None => Hint::new(level, "修改方向待分析（无错误码）".to_string()),
    //   }
    //
    // HintLevel::Solution => 返回模型配置指引；配置模型后由 SolutionHintService 替换
    match &level {
        HintLevel::Category => Hint::new(
            level,
            format!("这是一个{}", error_category_text(classified.category)),
        ),
        HintLevel::Location => match &diag.location {
            Some(loc) => Hint::new(level, format!("位置：{}", format_location(loc))),
            None => Hint::new(level, "位置信息缺失".to_string()),
        },
        HintLevel::Concept => {
            if classified.knowledge_points.is_empty() {
                Hint::new(level, "知识点待分析（无明确映射）".to_string())
            } else {
                let kps: Vec<_> = classified
                    .knowledge_points
                    .iter()
                    .map(|k| knowledge_point_text(*k))
                    .collect();
                Hint::new(level, format!("知识点：{}", kps.join("、")))
            }
        }
        HintLevel::Direction => match &diag.code {
            Some(code) => match code_to_direction(code) {
                Some(dir) => Hint::new(level, format!("修改方向：{}", dir)),
                None => Hint::new(level, "修改方向待分析".to_string()),
            },
            None => Hint::new(level, "修改方向待分析（无错误码）".to_string()),
        },
        HintLevel::Solution => Hint::new(
            level,
            "尚未配置 LLM，无法生成参考方案。请使用 --config <config.toml>（可配合 --profile）配置模型。",
        ),
    }
}

// ============================================================
// 测试失败提示（核心逻辑，TODO）
// ============================================================

/// 为测试失败生成指定级别的提示。
///
/// 各级别内容来源：
/// - **Category (1)**：「这是一个逻辑错误（测试未通过）」
/// - **Location (2)**：失败的测试用例名称
/// - **Concept (3)**：知识点无法从输出确定性判断，提示「待分析」
/// - **Direction (4)**：描述输入 / 期望 / 实际的对照
/// - **Solution (5)**：先返回模型配置指引，上层配置模型时替换为实际生成内容
pub fn generate_test_hint(name: &str, actual: &str, expected: &str, level: HintLevel) -> Hint {
    // TODO: 实现测试失败分层提示
    //
    // 建议步骤：按 level 分支
    //
    // HintLevel::Category =>
    //   Hint::new(level, "这是一个逻辑错误（测试未通过）")
    //
    // HintLevel::Location =>
    //   Hint::new(level, format!("失败的测试用例：{}", name))
    //
    // HintLevel::Concept =>
    //   Hint::new(level, "知识点待分析（需结合代码与题目判断）")
    //
    // HintLevel::Direction =>
    //   Hint::new(level,
    //     format!("输入对应期望输出为「{}」，但实际输出「{}」", expected.trim(), actual.trim()))
    //
    // HintLevel::Solution => 返回模型配置指引；配置模型后由 SolutionHintService 替换
    match &level {
        HintLevel::Category => Hint::new(level, "这是一个逻辑错误（测试未通过）".to_string()),
        HintLevel::Location => Hint::new(level, format!("失败的测试用例： {}", name)),
        HintLevel::Concept => Hint::new(level, "知识点待分析（需结合代码与题目判断）".to_string()),
        HintLevel::Direction => Hint::new(
            level,
            format!(
                "输入对应期望输出为「{}」，但实际输出为「{}」",
                expected.trim(),
                actual.trim()
            ),
        ),
        HintLevel::Solution => Hint::new(
            level,
            "尚未配置 LLM，无法生成参考方案。请使用 --config <config.toml>（可配合 --profile）配置模型。",
        ),
    }
}

// ============================================================
// 错误码 → 修改方向（核心逻辑，TODO）
// ============================================================

/// 将 rustc 错误码映射到通用修改方向。
///
/// 这是 Level 4 提示的数据来源，属于确定性逻辑。
/// 返回 `None` 表示无通用方向，需 LLM 分析。
///
/// # 已知方向（参考示例）
///
/// | 错误码 | 修改方向 |
/// |--------|----------|
/// | E0382 | 在移动值之前克隆，或重新设计所有权结构 |
/// | E0499 | 确保同一时间只有一个可变借用 |
/// | E0502 | 避免同时存在可变与不可变借用 |
/// | E0106 | 为引用参数添加生命周期标注 |
/// | E0597 | 检查被引用数据的生命周期是否足够长 |
/// | E0277 | 为类型实现所需 trait 或调整 trait bound |
pub fn code_to_direction(code: &str) -> Option<&'static str> {
    // TODO: 实现错误码到修改方向的映射
    //
    // 建议步骤：
    //   match code {
    //       "E0382" => Some("在移动值之前克隆，或重新设计所有权结构"),
    //       "E0499" => Some("确保同一时间只有一个可变借用"),
    //       "E0502" => Some("避免同时存在可变与不可变借用"),
    //       "E0106" => Some("为引用参数添加生命周期标注"),
    //       "E0597" => Some("检查被引用数据的生命周期是否足够长"),
    //       "E0277" => Some("为类型实现所需 trait 或调整 trait bound"),
    //       _ => None,
    //   }
    match code {
        "E0382" => Some("在移动值之前克隆，或重新设计所有权结构"),
        "E0499" => Some("确保同一时间只有一个可变借用"),
        "E0502" => Some("避免同时存在可变与不可变借用"),
        "E0106" => Some("为引用参数添加生命周期标注"),
        "E0597" => Some("检查被引用数据的生命周期是否足够长"),
        "E0277" => Some("为类型实现所需 trait 或调整 trait bound"),
        _ => None,
    }
}
