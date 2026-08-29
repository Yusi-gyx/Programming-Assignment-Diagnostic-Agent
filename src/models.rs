//! 核心数据结构
//!
//! 对应 DESIGN.md §7。这些类型贯穿整个诊断流程，
//! 是各模块之间交换信息的基础。
//!
//! 本模块只定义数据形状，不含业务逻辑。
//! 业务逻辑（知识点映射、提示生成等）在后续步骤实现。

use serde::{Deserialize, Serialize};

// ============================================================
// 题目与提交
// ============================================================

/// 编程作业题目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    /// 题目标题
    pub title: String,
    /// 题目描述（Markdown）
    pub description: String,
}

/// 学生提交
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    /// 源代码内容
    pub source_code: String,
    /// 关联的测试结果
    pub test_results: Vec<TestResult>,
}

// ============================================================
// 测试结果
// ============================================================

/// 单个测试用例的执行结果
///
/// 在 DESIGN.md §7 基础上增加 `expected_output`，
/// 以支持 §4.1 中「期望输出 / 实际输出」对照展示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// 用例名称
    pub name: String,
    /// 是否通过
    pub passed: bool,
    /// 实际输出
    pub actual_output: String,
    /// 期望输出（用于诊断展示）
    pub expected_output: String,
}

// ============================================================
// 诊断结果
// ============================================================

/// 诊断结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// 错误类别
    pub category: ErrorCategory,
    /// 关联的 Rust 知识点
    pub knowledge_points: Vec<KnowledgePoint>,
    /// 置信度 [0, 1]
    pub confidence: f32,
}

/// 主要错误类别
///
/// 对应 DESIGN.md §7。用于区分问题性质，
/// 决定后续诊断路径（编译错误优先于运行测试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCategory {
    /// 编译错误（语法 / 类型 / 所有权等）
    CompileError,
    /// 运行时错误（panic / 越界等）
    RuntimeError,
    /// 逻辑错误（输出不符合预期）
    LogicError,
    /// 边界条件错误
    BoundaryCondition,
    /// 算法错误
    AlgorithmError,
}

/// Rust 知识点
///
/// 用于将错误映射到具体学习内容，
/// 是分层提示与学习进度记忆的基础。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KnowledgePoint {
    Ownership,
    Borrowing,
    Lifetime,
    Trait,
    Generic,
    Iterator,
    Option,
    Result,
    PatternMatching,
    Collection,
    ErrorHandling,
    AlgorithmLogic,
}

/// 分层提示级别（由低到高）
///
/// 对应 DESIGN.md §4.3。默认从低级别开始，
/// 用户逐步请求更详细的提示，直至参考方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HintLevel {
    /// 1. 错误类别
    Category,
    /// 2. 错误位置
    Location,
    /// 3. 相关知识点
    Concept,
    /// 4. 修改方向
    Direction,
    /// 5. 参考方案
    Solution,
}
