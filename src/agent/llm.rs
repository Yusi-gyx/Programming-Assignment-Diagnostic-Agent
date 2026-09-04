//! LLM 客户端（第 9 步）
//!
//! 职责：
//! - 封装 OpenAI 兼容的 chat completions API 调用
//! - 构造请求体、解析响应、提取 token 用量
//! - 供第 7 步 Level 3-5 分层提示增强与后续测试生成使用
//!
//! 设计原则（AGENTS.md）：
//! - LLM 只处理需要语义理解的部分（理解题意、生成测试、自然语言化提示）
//! - token 用量直接取自 API 响应，不做本地估算（R6，第 10 步）
//!
//! # OpenAI 兼容接口
//!
//! 请求体：
//! ```json
//! {
//!   "model": "deepseek-chat",
//!   "messages": [
//!     {"role": "system", "content": "..."},
//!     {"role": "user", "content": "..."}
//!   ],
//!   "temperature": 0.7
//! }
//! ```
//!
//! 响应体：
//! ```json
//! {
//!   "model": "deepseek-chat",
//!   "choices": [
//!     {"message": {"role": "assistant", "content": "..."}}
//!   ],
//!   "usage": {"prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150}
//! }
//! ```

use crate::config::model::ModelConfig;
use crate::error::{PadaError, Result};
use crate::{
    analysis::error_parser::RustcDiagnostic,
    analysis::hint::{error_category_text, format_location, knowledge_point_text},
    models::{Assignment, Diagnostic, HintLevel, TestResult},
};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};

// ============================================================
// 聊天消息
// ============================================================

/// 一条聊天消息。
///
/// `role` 为 `"system"` / `"user"` / `"assistant"` 之一。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    /// 角色：`"system"` / `"user"` / `"assistant"`
    pub role: String,
    /// 消息内容
    pub content: String,
}

impl ChatMessage {
    /// 构造 system 消息（设定 Agent 行为 / 注入画像）
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    /// 构造 user 消息（用户输入 / 题目 / 代码）
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    /// 构造 assistant 消息（历史回复）
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// 构造编译错误的模型增强提示请求。
pub fn compile_hint_messages(
    assignment: &Assignment,
    source_context: &str,
    diag: &RustcDiagnostic,
    classified: &Diagnostic,
    level: HintLevel,
    deterministic_hint: &str,
    profile_summary: &str,
) -> Vec<ChatMessage> {
    let points = classified
        .knowledge_points
        .iter()
        .map(|point| knowledge_point_text(*point))
        .collect::<Vec<_>>()
        .join("、");
    vec![
        ChatMessage::system(format!(
            "你是 Rust 编程导师，必须遵守分层教学，不得越级直接给出作业答案。不要输出思维链或 <think> 标签。{}\n{}",
            hint_level_instruction(level),
            profile_summary
        )),
        ChatMessage::user(format!(
            "题目：{}\n题目描述：\n{}\n\n提交内容：\n```rust\n{}\n```\n\n错误类别：{}\n错误位置：{}\n错误码：{}\n错误消息：{}\n相关知识点：{}\nRust 基础提示：{}",
            assignment.title,
            assignment.description,
            source_context,
            error_category_text(classified.category),
            diag.location
                .as_ref()
                .map(format_location)
                .unwrap_or_else(|| "未知".into()),
            diag.code.as_deref().unwrap_or("无"),
            diag.message,
            if points.is_empty() {
                "待分析"
            } else {
                &points
            },
            deterministic_hint,
        )),
    ]
}

/// 构造测试失败的模型增强提示请求。
pub fn test_hint_messages(
    assignment: &Assignment,
    source_context: &str,
    result: &TestResult,
    classified: &Diagnostic,
    level: HintLevel,
    deterministic_hint: &str,
    profile_summary: &str,
) -> Vec<ChatMessage> {
    let points = classified
        .knowledge_points
        .iter()
        .map(|point| knowledge_point_text(*point))
        .collect::<Vec<_>>()
        .join("、");
    vec![
        ChatMessage::system(format!(
            "你是 Rust 编程导师，必须遵守分层教学，不得越级直接给出作业答案。不要输出思维链或 <think> 标签。{}\n{}",
            hint_level_instruction(level),
            profile_summary
        )),
        ChatMessage::user(format!(
            "题目：{}\n题目描述：\n{}\n\n提交内容：\n```rust\n{}\n```\n\n失败用例：{}\n期望输出：{}\n实际输出：{}\n相关知识点：{}\nRust 基础提示：{}",
            assignment.title,
            assignment.description,
            source_context,
            result.name,
            result.expected_output.trim(),
            result.actual_output.trim(),
            if points.is_empty() {
                "待分析"
            } else {
                &points
            },
            deterministic_hint,
        )),
    ]
}

/// 保留旧调用入口：构造 Level 5 编译错误请求。
pub fn compile_solution_messages(
    assignment: &Assignment,
    source_context: &str,
    diag: &RustcDiagnostic,
    classified: &Diagnostic,
    profile_summary: &str,
) -> Vec<ChatMessage> {
    compile_hint_messages(
        assignment,
        source_context,
        diag,
        classified,
        HintLevel::Solution,
        "",
        profile_summary,
    )
}

/// 保留旧调用入口：构造 Level 5 测试失败请求。
pub fn test_solution_messages(
    assignment: &Assignment,
    source_context: &str,
    result: &TestResult,
    profile_summary: &str,
) -> Vec<ChatMessage> {
    let classified = Diagnostic {
        category: crate::models::ErrorCategory::LogicError,
        knowledge_points: Vec::new(),
        confidence: 0.0,
    };
    test_hint_messages(
        assignment,
        source_context,
        result,
        &classified,
        HintLevel::Solution,
        "",
        profile_summary,
    )
}

fn hint_level_instruction(level: HintLevel) -> &'static str {
    match level {
        HintLevel::Concept => {
            "当前是 Level 3（相关知识点）。严格使用 Markdown 的“### 知识点说明”“### 通用示例”“### 自检问题”三节。解释诊断中已有知识点，并给一个与本题变量、数据和业务场景不同的最小 Rust 示例。示例只演示概念，不分析用户代码的具体改法，不给本题答案。"
        }
        HintLevel::Direction => {
            "当前是 Level 4（修改方向）。严格使用 Markdown 的“### 修改方向”“### 经典错误模式”“### 思考提示”三节。说明应检查的方向，并用一个与本题不同的经典错误写法及通用改进写法作对照。不得生成用户提交的完整修正版，不得直接给出本题答案。"
        }
        HintLevel::Solution => {
            "当前是 Level 5（参考方案）。严格使用 Markdown 的“### 问题原因”“### 修改步骤”“### 关键代码片段”“### 自检”四节。给出可操作的参考方案，但只展示与错误有关的最小代码片段，避免重写无关代码。"
        }
        _ => "只解释当前诊断信息，不提供完整答案。",
    }
}

// ============================================================
// LLM 响应
// ============================================================

/// LLM 的响应结果。
///
/// token 用量直接取自 API 响应的 `usage` 字段（R6 要求）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    /// 生成的回复内容
    pub content: String,
    /// 输入 token 数（prompt_tokens）
    pub input_tokens: usize,
    /// 输出 token 数（completion_tokens）
    pub output_tokens: usize,
    /// 实际使用的模型名
    pub model: String,
}

// ============================================================
// LLM 客户端
// ============================================================

/// LLM 客户端，封装 OpenAI 兼容的 chat completions 调用。
///
/// 普通调用使用同步 HTTP；交互调用使用 SSE 流并通过取消标志协作终止。
pub struct LlmClient {
    /// 模型配置
    config: ModelConfig,
    /// HTTP agent（复用连接池）
    agent: ureq::Agent,
}

/// 可替换的聊天模型接口，便于诊断能力共享客户端并进行离线测试。
pub trait ChatModel: Send + Sync {
    fn chat(&self, messages: &[ChatMessage]) -> Result<LlmResponse>;

    fn chat_cancellable(
        &self,
        messages: &[ChatMessage],
        cancelled: &AtomicBool,
    ) -> Result<LlmResponse> {
        if cancelled.load(Ordering::Acquire) {
            return Err(PadaError::Llm("模型生成已取消".into()));
        }
        let response = self.chat(messages)?;
        if cancelled.load(Ordering::Acquire) {
            Err(PadaError::Llm("模型生成已取消".into()))
        } else {
            Ok(response)
        }
    }
}

impl ChatModel for LlmClient {
    fn chat(&self, messages: &[ChatMessage]) -> Result<LlmResponse> {
        LlmClient::chat(self, messages)
    }

    fn chat_cancellable(
        &self,
        messages: &[ChatMessage],
        cancelled: &AtomicBool,
    ) -> Result<LlmResponse> {
        LlmClient::chat_cancellable(self, messages, cancelled)
    }
}

impl LlmClient {
    /// 创建客户端。
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(120))
                .build(),
        }
    }

    /// 发送聊天请求，返回响应。
    ///
    /// 失败原因包括：网络错误、HTTP 非 2xx、响应格式异常。
    pub fn chat(&self, messages: &[ChatMessage]) -> Result<LlmResponse> {
        let body = self.build_request_body(messages);
        let endpoint = self.config.chat_endpoint();

        // 构造请求，附带 Authorization header（有 key 时）
        let request = if self.config.api_key.is_empty() {
            self.agent.post(&endpoint)
        } else {
            self.agent
                .post(&endpoint)
                .set("Authorization", &format!("Bearer {}", self.config.api_key))
        };

        // 发送 JSON 请求体（send_json 接受任意 Serialize）
        let response = match request.send_json(body) {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                let status_text = response.status_text().to_owned();
                let response_body = response.into_string().unwrap_or_default();
                return Err(PadaError::Llm(format_http_status_error(
                    &endpoint,
                    status,
                    &status_text,
                    &response_body,
                )));
            }
            Err(error) => {
                return Err(PadaError::Llm(format!(
                    "HTTP 请求失败（{endpoint}）: {error}"
                )));
            }
        };

        // 解析响应 JSON
        let json: serde_json::Value = response
            .into_json()
            .map_err(|e| PadaError::Llm(format!("解析响应 JSON 失败: {}", e)))?;

        Self::parse_response(&json)
    }

    /// 使用 OpenAI 兼容的 SSE 流式响应；取消时丢弃 reader 以关闭 HTTP 连接。
    pub fn chat_cancellable(
        &self,
        messages: &[ChatMessage],
        cancelled: &AtomicBool,
    ) -> Result<LlmResponse> {
        let mut body = self.build_request_body(messages);
        body["stream"] = serde_json::json!(true);
        body["stream_options"] = serde_json::json!({"include_usage": true});
        let endpoint = self.config.chat_endpoint();
        let request = if self.config.api_key.is_empty() {
            self.agent.post(&endpoint)
        } else {
            self.agent
                .post(&endpoint)
                .set("Authorization", &format!("Bearer {}", self.config.api_key))
        };
        let response = match request.send_json(body) {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                let status_text = response.status_text().to_owned();
                let response_body = response.into_string().unwrap_or_default();
                return Err(PadaError::Llm(format_http_status_error(
                    &endpoint,
                    status,
                    &status_text,
                    &response_body,
                )));
            }
            Err(error) => {
                return Err(PadaError::Llm(format!(
                    "HTTP 请求失败（{endpoint}）: {error}"
                )));
            }
        };
        parse_stream_response(std::io::BufReader::new(response.into_reader()), cancelled)
    }

    /// 构造请求体（纯函数，便于离线测试）。
    ///
    /// 返回的 JSON 形如：
    /// ```json
    /// {
    ///   "model": "deepseek-chat",
    ///   "messages": [...],
    ///   "temperature": 0.7
    /// }
    /// ```
    ///
    /// 当 `reasoning == true` 时为支持该扩展的云端接口加入
    /// `"reasoning": true`；Ollama 使用自身的推理模型设置，不发送该字段。
    pub fn build_request_body(&self, messages: &[ChatMessage]) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.config.model_name,
            "messages": messages,
            "temperature": 0.7,
        });

        // Ollama 0.33.x 的 OpenAI 兼容接口不接受布尔 reasoning；本地推理模型
        // 会按模型自身配置工作，因此不发送这个云端扩展字段。
        if self.config.reasoning && !self.config.is_ollama() {
            body["reasoning"] = serde_json::json!(true);
        }

        body
    }

    /// 解析响应 JSON 为 [`LlmResponse`]（纯函数，便于离线测试）。
    ///
    /// 提取 `choices[0].message.content` 与 `usage` 中的 token 数。
    /// `usage` 缺失时 token 数记为 0（部分本地模型不返回 usage）。
    pub fn parse_response(json: &serde_json::Value) -> Result<LlmResponse> {
        // 提取回复内容
        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| PadaError::Llm("响应缺少 choices[0].message.content".into()))?
            .to_string();

        // 提取 token 用量（缺失时记为 0）
        let input_tokens = json
            .get("usage")
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let output_tokens = json
            .get("usage")
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        // 提取模型名
        let model = json
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(LlmResponse {
            content,
            input_tokens,
            output_tokens,
            model,
        })
    }

    /// 获取当前使用的模型配置（供 R6 成本统计使用）。
    pub fn config(&self) -> &ModelConfig {
        &self.config
    }
}

fn format_http_status_error(endpoint: &str, status: u16, status_text: &str, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|json| {
            json.pointer("/error/message")
                .and_then(|value| value.as_str())
                .or_else(|| json.get("error").and_then(|value| value.as_str()))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_owned());
    let detail = detail.chars().take(800).collect::<String>();
    if detail.is_empty() {
        format!("HTTP 请求失败（{endpoint}）: {status} {status_text}")
    } else {
        format!("HTTP 请求失败（{endpoint}）: {status} {status_text}；服务端信息: {detail}")
    }
}

/// 解析 OpenAI 兼容的 SSE 流式响应，并在每个数据块之间检查取消标志。
pub fn parse_stream_response<R: BufRead>(
    mut reader: R,
    cancelled: &AtomicBool,
) -> Result<LlmResponse> {
    let mut content = String::new();
    let mut model = String::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut line = String::new();

    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(PadaError::Llm("模型生成已取消".into()));
        }
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|error| PadaError::Llm(format!("读取模型流式响应失败: {error}")))?;
        if bytes == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with("event:") {
            continue;
        }
        let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if data == "[DONE]" {
            break;
        }
        let json: serde_json::Value = serde_json::from_str(data).map_err(|error| {
            PadaError::Llm(format!("解析模型流式响应失败: {error}；数据: {data}"))
        })?;
        if let Some(message) = json
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str)
            .or_else(|| json.get("error").and_then(serde_json::Value::as_str))
        {
            return Err(PadaError::Llm(format!("模型流式响应错误: {message}")));
        }
        if let Some(value) = json.get("model").and_then(serde_json::Value::as_str) {
            model = value.to_owned();
        }
        if let Some(value) = json
            .pointer("/choices/0/delta/content")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                json.pointer("/choices/0/message/content")
                    .and_then(serde_json::Value::as_str)
            })
        {
            content.push_str(value);
        }
        if let Some(usage) = json.get("usage") {
            input_tokens = usage
                .get("prompt_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(input_tokens as u64) as usize;
            output_tokens = usage
                .get("completion_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(output_tokens as u64) as usize;
        }
    }

    if cancelled.load(Ordering::Acquire) {
        return Err(PadaError::Llm("模型生成已取消".into()));
    }
    if content.is_empty() {
        return Err(PadaError::Llm("模型流式响应没有返回文本内容".into()));
    }
    Ok(LlmResponse {
        content,
        input_tokens,
        output_tokens,
        model,
    })
}

#[cfg(test)]
mod internal_tests {
    use super::format_http_status_error;

    #[test]
    fn http_status_error_includes_json_message() {
        let message = format_http_status_error(
            "http://localhost:11434/v1/chat/completions",
            400,
            "Bad Request",
            r#"{"error":{"message":"invalid reasoning value"}}"#,
        );
        assert!(message.contains("400 Bad Request"));
        assert!(message.contains("invalid reasoning value"));
    }
}
