//! 分层提示生成
//!
//! 职责（开发计划第 7 步）：
//! - 根据诊断结果与用户选择的 [`HintLevel`] 生成对应级别的提示
//! - 提供提示级别控制（升级 / 数值转换）
//!
//! 设计原则（AGENTS.md）：
//! - 提示级别控制是确定性逻辑，由 Rust 完成
//! - Level 1-2（类别 / 位置）完全由程序从诊断数据中提取
//! - Level 3-4 先由程序给出确定性基础提示，配置模型后在上层进行教学增强
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
use crate::models::{Diagnostic, ErrorCategory, HintLevel, KnowledgePoint, TestResult};

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
        KnowledgePoint::TypeSystem => "类型系统 / TypeSystem",
        KnowledgePoint::Syntax => "语法 / Syntax",
        KnowledgePoint::NameResolution => "名称解析与模块 / NameResolution",
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
// 编译错误提示
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
            "尚未配置 LLM，无法生成参考方案。请在导师模式输入 config 配置模型。",
        ),
    }
}

// ============================================================
// 测试失败提示
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
    generate_test_hint_with_points(name, actual, expected, level, &[])
}

pub fn generate_test_hint_with_points(
    name: &str,
    actual: &str,
    expected: &str,
    level: HintLevel,
    points: &[KnowledgePoint],
) -> Hint {
    match &level {
        HintLevel::Category => Hint::new(level, "这是一个逻辑错误（测试未通过）".to_string()),
        HintLevel::Location => Hint::new(level, format!("失败的测试用例： {}", name)),
        HintLevel::Concept => {
            if points.is_empty() {
                Hint::new(level, "知识点待分析（配置 LLM 后自动映射）")
            } else {
                Hint::new(
                    level,
                    format!(
                        "知识点：{}",
                        points
                            .iter()
                            .map(|point| knowledge_point_text(*point))
                            .collect::<Vec<_>>()
                            .join("、")
                    ),
                )
            }
        }
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
            "尚未配置 LLM，无法生成参考方案。请在导师模式输入 config 配置模型。",
        ),
    }
}

/// 使用完整执行结果生成提示，使 panic/异常退出不会被误报为逻辑错误。
pub fn generate_test_result_hint(
    result: &TestResult,
    classified: &Diagnostic,
    level: HintLevel,
) -> Hint {
    match level {
        HintLevel::Category => Hint::new(
            level,
            if classified.category == ErrorCategory::RuntimeError {
                "这是一个运行时错误（程序未正常退出）"
            } else {
                "这是一个逻辑错误（测试未通过）"
            },
        ),
        HintLevel::Location => Hint::new(level, format!("失败的测试用例：{}", result.name)),
        HintLevel::Concept => {
            if classified.knowledge_points.is_empty() {
                Hint::new(level, "知识点待分析（配置 LLM 后自动映射）")
            } else {
                Hint::new(
                    level,
                    format!(
                        "知识点：{}",
                        classified
                            .knowledge_points
                            .iter()
                            .map(|point| knowledge_point_text(*point))
                            .collect::<Vec<_>>()
                            .join("、")
                    ),
                )
            }
        }
        HintLevel::Direction => {
            if let Some(error) = &result.runtime_error {
                Hint::new(
                    level,
                    format!(
                        "程序未正常退出；先根据运行错误定位 panic 或异常路径：{}",
                        error.trim()
                    ),
                )
            } else {
                Hint::new(
                    level,
                    format!(
                        "输入对应期望输出为「{}」，但实际输出为「{}」",
                        result.expected_output.trim(),
                        result.actual_output.trim()
                    ),
                )
            }
        }
        HintLevel::Solution => Hint::new(
            level,
            "尚未配置 LLM，无法生成参考方案。请在导师模式输入 config 配置模型。",
        ),
    }
}

// ============================================================
// 错误码 → 修改方向
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
    match code {
        "E0308" => Some("核对表达式的期望类型与实际类型，并在数据产生处统一类型"),
        "E0282" | "E0284" | "E0283" => Some("补充必要的类型标注或泛型参数，消除类型推断歧义"),
        "E0605" | "E0606" => Some("使用该类型支持的显式转换方式，避免无效的 as 转换"),
        "E0369" | "E0271" => Some("检查运算两侧或关联类型是否满足相同的类型约束"),
        "E0425" | "E0412" => Some("检查名称拼写、定义位置和当前作用域中的导入"),
        "E0432" | "E0433" | "E0583" | "E0603" => {
            Some("检查模块路径、mod 声明、use 导入以及条目的可见性")
        }
        "E0599" | "E0046" | "E0119" | "E0404" => {
            Some("检查 Trait 实现、约束和方法可用条件是否完整且无冲突")
        }
        "E0004" | "E0005" => Some("补全模式分支，并确保模式覆盖所有可能值"),
        "E0107" | "E0207" | "E0243" => Some("核对泛型参数数量、声明位置和实际约束"),
        "E0515" | "E0716" => Some("避免返回或长期借用临时值，延长所有者的有效作用域"),
        "E0596" | "E0506" => Some("检查绑定的可变性，并在赋值前结束冲突的借用"),
        "E0382" => Some("在移动值之前克隆，或重新设计所有权结构"),
        "E0507" => Some("不要从借用内容中直接移出值；可借用、复制或显式克隆"),
        "E0505" | "E0500" => Some("调整所有权转移时机，使现有借用先结束"),
        "E0499" => Some("确保同一时间只有一个可变借用"),
        "E0502" => Some("避免同时存在可变与不可变借用"),
        "E0106" => Some("为引用参数添加生命周期标注"),
        "E0597" => Some("检查被引用数据的生命周期是否足够长"),
        "E0277" => Some("为类型实现所需 trait 或调整 trait bound"),
        "E0554" => Some("移除稳定版不支持的 feature 属性，或改用稳定语言能力"),
        _ => None,
    }
}
