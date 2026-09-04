//! 自动测试用例生成（开发计划第 11 步）
//!
//! 职责：
//! - 根据题目描述，让 LLM 生成边界测试用例（输入 + 期望输出）
//! - 构造提示词、解析 LLM 响应为 [`TestCase`] 列表
//! - 提供确定性边界用例类型清单（空 / 单元素 / 重复 / 负数 / 边界值 / 大规模）
//!
//! 设计原则（AGENTS.md / DESIGN.md §4.2）：
//! - LLM 负责「理解题意并生成测试」，Rust 负责提示词构造与响应解析
//! - 测试用例的执行与判定由 [`runner`] 模块的确定性逻辑完成
//!
//! # 工作流
//!
//! ```text
//! Assignment（题目）
//!        ↓
//! build_prompt() 构造提示词（Rust，确定性）
//!        ↓
//! LlmClient.chat() 调用 LLM
//!        ↓
//! parse_test_cases() 解析响应为 Vec<TestCase>（Rust，确定性）
//!        ↓
//! TestRunner.run_tests() 执行判定
//! ```
//!
//! # LLM 响应格式
//!
//! 要求 LLM 输出 JSON 数组（可包裹在 ```json 代码块中）：
//!
//! ```json
//! [
//!   {"name": "empty_input", "input": "", "expected_output": "0"},
//!   {"name": "single", "input": "5", "expected_output": "10"},
//!   {"name": "negatives", "input": "-1\n-2\n-3\n", "expected_output": "-6"}
//! ]
//! ```

use crate::agent::llm::{ChatMessage, LlmClient, LlmResponse};
use crate::error::{PadaError, Result};
use crate::models::Assignment;
use crate::tools::runner::TestCase;
use std::path::{Path, PathBuf};

// ============================================================
// 边界用例类型（确定性）
// ============================================================

/// 返回应覆盖的边界用例类型清单（DESIGN.md §4.2）。
///
/// 这些类型会写入提示词，引导 LLM 生成对应场景的测试。
pub fn boundary_case_types() -> Vec<&'static str> {
    vec![
        "空输入",
        "单元素",
        "重复元素",
        "负数",
        "边界值（如 0、最大值、最小值）",
        "大规模输入",
    ]
}

// ============================================================
// 提示词构造（确定性）
// ============================================================

/// 构造让 LLM 生成测试用例的提示词。
///
/// 返回 `[system, user]` 两条消息：
/// - system：设定测试用例生成器的角色与输出格式要求
/// - user：注入题目描述与边界用例类型清单
///
/// 这是纯函数，便于离线测试提示词结构。
pub fn build_prompt(assignment: &Assignment) -> Vec<ChatMessage> {
    let types = boundary_case_types();
    let types_text = types
        .iter()
        .map(|t| format!("- {}", t))
        .collect::<Vec<_>>()
        .join("\n");

    let system = ChatMessage::system(
        "你是一位 Rust 编程作业的测试用例生成助手。\
         你的任务是根据题目描述，生成一组边界测试用例，\
         覆盖常见边界情况（空输入、单元素、重复、负数、边界值、大规模输入等）。\n\n\
         输出要求：\n\
         1. 只输出一个 JSON 数组，不要有任何多余解释文字\n\
         2. 数组中每个元素包含三个字段：\n\
         - \"name\": 用例名称（英文 snake_case）\n\
         - \"input\": 标准输入内容（字符串，多行用 \\n 分隔）\n\
         - \"expected_output\": 期望的标准输出（字符串）\n\
         3. 用 JSON 代码块包裹，格式如下：\n\
         ```json\n\
         [\n\
           {\"name\": \"example\", \"input\": \"...\", \"expected_output\": \"...\"}\n\
         ]\n\
         ```\n\
         4. 生成 5 到 8 个用例，覆盖尽可能多的边界类型\n\
         5. 期望输出必须是程序在正确实现下应产生的输出",
    );

    let user = ChatMessage::user(format!(
        "题目：{}\n\n描述：\n{}\n\n请生成覆盖以下边界类型的测试用例：\n{}",
        assignment.title, assignment.description, types_text
    ));

    vec![system, user]
}

/// 在测试生成提示中注入由 Rust 计算的学习画像。
pub fn build_prompt_with_profile(
    assignment: &Assignment,
    profile_summary: &str,
) -> Vec<ChatMessage> {
    let mut messages = build_prompt(assignment);
    messages.insert(1, ChatMessage::system(profile_summary));
    messages
}

// ============================================================
// 响应解析（确定性）
// ============================================================

/// 从 LLM 响应文本中解析测试用例列表。
///
/// 支持两种格式：
/// - 直接 JSON 数组
/// - 包裹在 ```json ... ``` 代码块中的 JSON
///
/// 解析失败（格式不符 / 字段缺失）返回错误。
pub fn parse_test_cases(content: &str) -> Result<Vec<TestCase>> {
    let json_str = extract_json_array(content)?;
    let cases: Vec<TestCaseJson> = serde_json::from_str(&json_str)
        .map_err(|e| PadaError::Parse(format!("解析测试用例 JSON 失败: {}", e)))?;
    Ok(cases.into_iter().map(TestCase::from).collect())
}

/// 从文本中提取 JSON 数组字符串。
///
/// 优先查找 ```json 代码块，找不到则尝试整体解析。
fn extract_json_array(content: &str) -> Result<String> {
    // 尝试提取 ```json ... ``` 代码块
    if let Some(start) = content.find("```json") {
        let after = &content[start + 7..];
        if let Some(end) = after.find("```") {
            return Ok(after[..end].trim().to_string());
        }
    }
    // 尝试提取 ``` ... ``` 代码块（无语言标注）
    if let Some(start) = content.find("```") {
        let after = &content[start + 3..];
        // 跳过可能的语言标注行
        let after = if after.starts_with("json") || after.starts_with('\n') {
            &after[after.find('\n').unwrap_or(0)..]
        } else {
            after
        };
        if let Some(end) = after.find("```") {
            return Ok(after[..end].trim().to_string());
        }
    }
    // 尝试直接作为 JSON 解析（找第一个 [ 到最后一个 ]）
    let start = content.find('[');
    let end = content.rfind(']');
    match (start, end) {
        (Some(s), Some(e)) if s < e => Ok(content[s..=e].to_string()),
        _ => Err(PadaError::Parse("响应中未找到 JSON 数组".into())),
    }
}

/// 用于反序列化的中间结构。
#[derive(serde::Deserialize)]
struct TestCaseJson {
    name: String,
    input: String,
    expected_output: String,
}

impl From<TestCaseJson> for TestCase {
    fn from(j: TestCaseJson) -> Self {
        TestCase {
            name: j.name,
            input: j.input,
            expected_output: j.expected_output,
        }
    }
}

// ============================================================
// 测试用例生成器
// ============================================================

/// 测试用例生成器，封装 LLM 调用 + 解析流程。
pub struct TestGenerator {
    client: LlmClient,
}

impl TestGenerator {
    /// 创建生成器，需传入已配置好的 LLM 客户端。
    pub fn new(client: LlmClient) -> Self {
        Self { client }
    }

    /// 根据题目生成测试用例。
    ///
    /// 内部流程：构造提示词 → 调用 LLM → 解析响应。
    pub fn generate(&self, assignment: &Assignment) -> Result<Vec<TestCase>> {
        let messages = build_prompt(assignment);
        let response: LlmResponse = self.client.chat(&messages)?;
        parse_test_cases(&response.content)
    }

    /// 获取 LLM 响应（不解析），供调用方自行处理或调试。
    pub fn generate_raw(&self, assignment: &Assignment) -> Result<LlmResponse> {
        let messages = build_prompt(assignment);
        self.client.chat(&messages)
    }

    pub fn generate_raw_with_profile(
        &self,
        assignment: &Assignment,
        profile_summary: &str,
    ) -> Result<LlmResponse> {
        self.client
            .chat(&build_prompt_with_profile(assignment, profile_summary))
    }
}

/// 将生成用例保存到题目文件所在目录；不覆盖已有生成文件。
pub fn save_generated_test_cases(problem_path: &Path, cases: &[TestCase]) -> Result<PathBuf> {
    if cases.is_empty() {
        return Err(PadaError::Parse("模型没有生成可保存的测试用例".into()));
    }
    let directory = problem_path.parent().unwrap_or_else(|| Path::new("."));
    let mut index = 1_usize;
    let path = loop {
        let name = if index == 1 {
            "generated_tests.json".to_owned()
        } else {
            format!("generated_tests_{index}.json")
        };
        let candidate = directory.join(name);
        if !candidate.exists() {
            break candidate;
        }
        index += 1;
    };
    let json = serde_json::to_string_pretty(cases)
        .map_err(|error| PadaError::Parse(format!("序列化生成测试失败: {error}")))?;
    std::fs::write(&path, json).map_err(|error| {
        PadaError::Config(format!("保存生成测试 {} 失败: {error}", path.display()))
    })?;
    Ok(path)
}
