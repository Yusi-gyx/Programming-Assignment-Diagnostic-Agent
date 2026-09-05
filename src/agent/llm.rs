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

pub use crate::config::effort::ModelTaskKind;
use crate::config::effort::{EffortMode, EffortPolicy};
use crate::config::model::{ModelConfig, ReasoningProtocol};
use crate::error::{PadaError, Result};
use crate::{
    analysis::error_parser::RustcDiagnostic,
    analysis::hint::{error_category_text, format_location, knowledge_point_text},
    models::{Assignment, Diagnostic, HintLevel, TestResult},
};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

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
    compile_hint_messages_with_policy(
        assignment,
        source_context,
        diag,
        classified,
        level,
        deterministic_hint,
        profile_summary,
        EffortPolicy::for_mode(EffortMode::Medium),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn compile_hint_messages_with_policy(
    assignment: &Assignment,
    source_context: &str,
    diag: &RustcDiagnostic,
    classified: &Diagnostic,
    level: HintLevel,
    deterministic_hint: &str,
    profile_summary: &str,
    policy: EffortPolicy,
) -> Vec<ChatMessage> {
    let source_context = crate::agent::context::relevant_source_with_scope(
        source_context,
        diag.location.as_ref(),
        policy.source,
    );
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
            "题目：{}\n题目描述：\n{}\n\n提交内容：\n```rust\n{}\n```\n\n失败用例：{}\n期望输出：{}\n实际输出：{}\n运行错误：{}\n相关知识点：{}\nRust 基础提示：{}",
            assignment.title,
            assignment.description,
            source_context,
            result.name,
            result.expected_output.trim(),
            result.actual_output.trim(),
            result.runtime_error.as_deref().unwrap_or("无"),
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

pub(crate) fn hint_level_instruction(level: HintLevel) -> &'static str {
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

/// Level 3 只讲授已确定的知识点；相同知识点可共享通用示例，原始证据留在报告。
pub fn concept_hint_messages(classified: &Diagnostic, profile_summary: &str) -> Vec<ChatMessage> {
    let mut points = classified
        .knowledge_points
        .iter()
        .map(|p| knowledge_point_text(*p))
        .collect::<Vec<_>>();
    points.sort_unstable();
    points.dedup();
    vec![
        ChatMessage::system(format!(
            "你是 Rust 编程导师。不要输出思维链或 <think> 标签。{}\n{}",
            hint_level_instruction(HintLevel::Concept),
            profile_summary
        )),
        ChatMessage::user(format!(
            "解释以下已由诊断确定的知识点：{}。只给通用概念、最小示例和自检问题，不推断具体提交的修改方案。",
            points.join("、")
        )),
    ]
}

// ============================================================
// LLM 响应
// ============================================================

/// LLM 的响应结果。
///
/// token 用量直接取自 API 响应的 `usage` 字段（R6 要求）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    #[serde(default)]
    pub details: ResponseDetails,
    #[serde(default)]
    pub timings: CallTimings,
    /// 生成的回复内容
    pub content: String,
    /// 输入 token 数（prompt_tokens）
    pub input_tokens: usize,
    /// 输出 token 数（completion_tokens）
    pub output_tokens: usize,
    /// 实际使用的模型名
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResponseDetails {
    pub reasoning_tokens: Option<usize>,
    pub finish_reason: Option<String>,
}

impl LlmResponse {
    /// 先记录 API 用量，再检查完整性；截断结果不能进入缓存或测试文件。
    pub fn ensure_complete(&self) -> Result<()> {
        match self.details.finish_reason.as_deref() {
            Some("length") => Err(PadaError::Llm(
                "模型输出达到长度上限，结果不完整；请提高 output_limits 后重试".into(),
            )),
            Some(reason) if reason != "stop" => {
                Err(PadaError::Llm(format!("模型未正常完成输出：{reason}")))
            }
            _ if self.content.trim().is_empty() => {
                Err(PadaError::Llm("模型没有返回有效正文".into()))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CallTimings {
    pub prompt_build_ms: u64,
    pub api_ttft_ms: Option<u64>,
    pub total_ms: u64,
    pub api_first_event_ms: Option<u64>,
    pub api_first_reasoning_ms: Option<u64>,
    pub json_fallback: bool,
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
    reasoning_effort: &'static str,
    effort_mode: EffortMode,
}

/// 可替换的聊天模型接口，便于诊断能力共享客户端并进行离线测试。
pub trait ChatModel: Send + Sync {
    fn chat(&self, messages: &[ChatMessage]) -> Result<LlmResponse>;

    fn chat_for_task(
        &self,
        messages: &[ChatMessage],
        _task: ModelTaskKind,
        cancelled: &AtomicBool,
        on_chunk: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmResponse> {
        self.chat_cancellable_streaming(messages, cancelled, on_chunk)
    }

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

    fn chat_cancellable_streaming(
        &self,
        messages: &[ChatMessage],
        cancelled: &AtomicBool,
        on_chunk: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmResponse> {
        let response = self.chat_cancellable(messages, cancelled)?;
        on_chunk(&response.content);
        Ok(response)
    }
}

impl ChatModel for LlmClient {
    fn chat_for_task(
        &self,
        messages: &[ChatMessage],
        task: ModelTaskKind,
        cancelled: &AtomicBool,
        on_chunk: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmResponse> {
        self.chat_streaming_for_task(messages, task, cancelled, on_chunk)
    }

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

    fn chat_cancellable_streaming(
        &self,
        messages: &[ChatMessage],
        cancelled: &AtomicBool,
        on_chunk: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmResponse> {
        self.chat_cancellable_streaming(messages, cancelled, on_chunk)
    }
}

impl LlmClient {
    /// 创建客户端。
    pub fn new(config: ModelConfig) -> Self {
        Self::with_effort(config, EffortPolicy::for_mode(EffortMode::Medium))
    }

    pub fn with_effort(config: ModelConfig, policy: EffortPolicy) -> Self {
        Self {
            config,
            agent: HTTP_AGENT
                .get_or_init(|| {
                    ureq::AgentBuilder::new()
                        .timeout(std::time::Duration::from_secs(120))
                        .build()
                })
                .clone(),
            reasoning_effort: policy.reasoning_effort,
            effort_mode: policy.mode,
        }
    }

    /// 发送聊天请求，返回响应。
    ///
    /// 失败原因包括：网络错误、HTTP 非 2xx、响应格式异常。
    pub fn chat(&self, messages: &[ChatMessage]) -> Result<LlmResponse> {
        self.chat_cancellable(messages, &AtomicBool::new(false))
    }

    pub fn chat_json(&self, messages: &[ChatMessage]) -> Result<LlmResponse> {
        let started = Instant::now();
        let body = self.build_request_body(messages);
        let prompt_build_ms = started.elapsed().as_millis() as u64;
        let api_started = Instant::now();
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

        let mut response = Self::parse_response(&json)?;
        let elapsed = api_started.elapsed().as_millis() as u64;
        response.timings = CallTimings {
            prompt_build_ms,
            api_ttft_ms: Some(elapsed),
            api_first_event_ms: Some(elapsed),
            total_ms: started.elapsed().as_millis() as u64,
            json_fallback: true,
            ..Default::default()
        };
        Ok(response)
    }

    /// 使用 OpenAI 兼容的 SSE 流式响应；取消时丢弃 reader 以关闭 HTTP 连接。
    pub fn chat_cancellable(
        &self,
        messages: &[ChatMessage],
        cancelled: &AtomicBool,
    ) -> Result<LlmResponse> {
        self.chat_cancellable_streaming(messages, cancelled, &mut |_| {})
    }

    pub fn chat_cancellable_streaming(
        &self,
        messages: &[ChatMessage],
        cancelled: &AtomicBool,
        on_chunk: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmResponse> {
        self.chat_streaming_for_task(messages, ModelTaskKind::General, cancelled, on_chunk)
    }

    fn chat_streaming_for_task(
        &self,
        messages: &[ChatMessage],
        task: ModelTaskKind,
        cancelled: &AtomicBool,
        on_chunk: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmResponse> {
        if cancelled.load(Ordering::Acquire) {
            return Err(PadaError::Llm("模型生成已取消".into()));
        }
        let started = Instant::now();
        let mut body = self.build_request_body_for_task(messages, task);
        body["stream"] = serde_json::json!(true);
        body["stream_options"] = serde_json::json!({"include_usage": true});
        let prompt_build_ms = started.elapsed().as_millis() as u64;
        let api_started = Instant::now();
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
        let mut ttft = None;
        let mut first_event = None;
        let first_reasoning = std::cell::Cell::new(None);
        let mut visible = StreamText::default();
        let is_json = response
            .header("Content-Type")
            .is_some_and(|value| value.contains("application/json"));
        let mut emit = |chunk: &str| {
            let text = visible.push(chunk);
            if (visible.thinking || chunk.contains("<think>")) && first_reasoning.get().is_none() {
                first_reasoning.set(Some(api_started.elapsed().as_millis() as u64));
            }
            if !text.is_empty() {
                ttft.get_or_insert(api_started.elapsed().as_millis() as u64);
                on_chunk(&text);
            }
        };
        let mut result = if is_json {
            response
                .into_json::<serde_json::Value>()
                .map_err(|error| PadaError::Llm(format!("解析响应 JSON 失败: {error}")))
                .and_then(|json| Self::parse_response(&json))
                .inspect(|response| {
                    if !cancelled.load(Ordering::Acquire) {
                        emit(&response.content);
                    }
                })
        } else {
            parse_stream_response_observed(
                std::io::BufReader::new(response.into_reader()),
                cancelled,
                &mut emit,
                |json| {
                    let elapsed = api_started.elapsed().as_millis() as u64;
                    first_event.get_or_insert(elapsed);
                    if json
                        .pointer("/choices/0/delta/reasoning_content")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                        && first_reasoning.get().is_none()
                    {
                        first_reasoning.set(Some(elapsed));
                    }
                },
            )
        };
        if cancelled.load(Ordering::Acquire) {
            return Err(PadaError::Llm("模型生成已取消".into()));
        }
        if let Ok(response) = &mut result {
            response.timings = CallTimings {
                prompt_build_ms,
                api_ttft_ms: ttft,
                total_ms: started.elapsed().as_millis() as u64,
                api_first_event_ms: if is_json { ttft } else { first_event },
                api_first_reasoning_ms: first_reasoning.get(),
                json_fallback: is_json,
            };
        }
        result
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
    /// 官方 DeepSeek 使用 thinking 开关及原生强度；其他接口保留兼容行为。
    pub fn build_request_body(&self, messages: &[ChatMessage]) -> serde_json::Value {
        self.build_request_body_for_task(messages, ModelTaskKind::General)
    }

    pub fn build_request_body_for_task(
        &self,
        messages: &[ChatMessage],
        task: ModelTaskKind,
    ) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.config.model_name,
            "messages": messages,
            "temperature": 0.7,
            "max_tokens": self.config.output_limits.for_task(task),
        });

        if self.config.resolved_reasoning_protocol() == ReasoningProtocol::Deepseek {
            body["thinking"] = serde_json::json!({
                "type": if self.config.reasoning { "enabled" } else { "disabled" }
            });
            // 关闭思考时不附带 effort，避免接口拒绝矛盾的参数组合。
            if self.config.reasoning {
                let effort = match (task, self.effort_mode) {
                    (
                        ModelTaskKind::KnowledgeMapping | ModelTaskKind::Hint(HintLevel::Concept),
                        _,
                    ) => "low",
                    (_, EffortMode::Auto | EffortMode::Low | EffortMode::Medium) => "low",
                    (_, EffortMode::High | EffortMode::Xhigh) => "high",
                    (_, EffortMode::Max) => "max",
                };
                body["reasoning_effort"] = serde_json::json!(effort);
            }
        } else if self.config.resolved_reasoning_protocol() == ReasoningProtocol::Ollama {
            let effort = if !self.config.reasoning {
                if self.config.is_ollama_gpt_oss() {
                    "low"
                } else {
                    "none"
                }
            } else if matches!(
                task,
                ModelTaskKind::KnowledgeMapping | ModelTaskKind::Hint(HintLevel::Concept)
            ) {
                "low"
            } else {
                match self.effort_mode {
                    EffortMode::Auto | EffortMode::Medium => "medium",
                    EffortMode::Low => "low",
                    EffortMode::High | EffortMode::Xhigh | EffortMode::Max => "high",
                }
            };
            body["reasoning_effort"] = serde_json::json!(effort);
        } else if self.config.resolved_reasoning_protocol() == ReasoningProtocol::EnableThinking {
            body["enable_thinking"] = serde_json::json!(self.config.reasoning);
        } else if self.config.reasoning {
            body["reasoning"] = serde_json::json!(true);
            body["reasoning_effort"] = serde_json::json!(self.reasoning_effort);
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
            .or_else(|| {
                (json
                    .pointer("/choices/0/finish_reason")
                    .and_then(serde_json::Value::as_str)
                    == Some("length"))
                .then_some("")
            })
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
            details: ResponseDetails {
                reasoning_tokens: json
                    .pointer("/usage/completion_tokens_details/reasoning_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .map(|n| n as usize),
                finish_reason: json
                    .pointer("/choices/0/finish_reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            },
            timings: CallTimings::default(),
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

// All services share one process-lifetime connection pool, including profile switches.
static HTTP_AGENT: OnceLock<ureq::Agent> = OnceLock::new();

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
pub fn parse_stream_response<R: BufRead>(reader: R, cancelled: &AtomicBool) -> Result<LlmResponse> {
    parse_stream_response_with_callback(reader, cancelled, |_| {})
}

pub fn parse_stream_response_with_callback<R: BufRead>(
    reader: R,
    cancelled: &AtomicBool,
    on_chunk: impl FnMut(&str),
) -> Result<LlmResponse> {
    parse_stream_response_observed(reader, cancelled, on_chunk, |_| {})
}

fn parse_stream_response_observed<R: BufRead>(
    mut reader: R,
    cancelled: &AtomicBool,
    mut on_chunk: impl FnMut(&str),
    mut on_event: impl FnMut(&serde_json::Value),
) -> Result<LlmResponse> {
    let mut details = ResponseDetails::default();
    let mut content = String::new();
    let mut model = String::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut line = String::new();
    let mut completed = false;

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
        if line.is_empty()
            || line.starts_with(':')
            || line.starts_with("event:")
            || line.starts_with("id:")
            || line.starts_with("retry:")
        {
            continue;
        }
        let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if data == "[DONE]" {
            completed = true;
            // Consume the HTTP body to EOF so ureq can return the socket to its pool.
            std::io::copy(&mut reader, &mut std::io::sink())
                .map_err(|error| PadaError::Llm(format!("读取响应结尾失败: {error}")))?;
            break;
        }
        let json: serde_json::Value = serde_json::from_str(data).map_err(|error| {
            PadaError::Llm(format!("解析模型流式响应失败: {error}；数据: {data}"))
        })?;
        on_event(&json);
        if let Some(reason) = json
            .pointer("/choices/0/finish_reason")
            .and_then(serde_json::Value::as_str)
        {
            details.finish_reason = Some(reason.to_owned());
        }
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
        completed |= json
            .pointer("/choices/0/finish_reason")
            .is_some_and(|value| !value.is_null())
            || json.pointer("/choices/0/message/content").is_some();
        if let Some(value) = json
            .pointer("/choices/0/delta/content")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                json.pointer("/choices/0/message/content")
                    .and_then(serde_json::Value::as_str)
            })
        {
            content.push_str(value);
            if !value.is_empty() && !cancelled.load(Ordering::Acquire) {
                on_chunk(value);
            }
        }
        if let Some(usage) = json.get("usage") {
            if let Some(tokens) = usage
                .pointer("/completion_tokens_details/reasoning_tokens")
                .and_then(serde_json::Value::as_u64)
            {
                details.reasoning_tokens = Some(tokens as usize);
            }
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
    if content.is_empty() && details.finish_reason.as_deref() != Some("length") {
        return Err(PadaError::Llm("模型流式响应没有返回文本内容".into()));
    }
    if !completed {
        return Err(PadaError::Llm(
            "模型流式响应提前结束，未收到完成标记；请重试".into(),
        ));
    }
    Ok(LlmResponse {
        details,
        timings: CallTimings::default(),
        content,
        input_tokens,
        output_tokens,
        model,
    })
}

/// Hide reasoning tags even when a tag spans several SSE chunks.
#[derive(Default)]
pub struct StreamText {
    pending: String,
    thinking: bool,
}

impl StreamText {
    pub fn push(&mut self, chunk: &str) -> String {
        self.pending.push_str(chunk);
        let mut visible = String::new();
        loop {
            let tag = if self.thinking { "</think>" } else { "<think>" };
            if let Some(index) = self.pending.find(tag) {
                if !self.thinking {
                    visible.push_str(&self.pending[..index]);
                }
                self.pending.drain(..index + tag.len());
                self.thinking = !self.thinking;
                continue;
            }
            let keep = (1..tag.len())
                .rev()
                .find(|&len| self.pending.ends_with(&tag[..len]))
                .unwrap_or(0);
            let end = self.pending.len() - keep;
            if !self.thinking {
                visible.push_str(&self.pending[..end]);
            }
            self.pending.drain(..end);
            return visible;
        }
    }
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
