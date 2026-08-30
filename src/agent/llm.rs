//! LLM 客户端（第 9 步）
//!
//! 职责：
//! - 封装 OpenAI 兼容的 chat completions API 调用
//! - 构造请求体、解析响应、提取 token 用量
//! - 供第 7 步分层提示的 Level 5（参考方案）与后续测试生成使用
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
use serde::{Deserialize, Serialize};

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
/// 使用同步 HTTP（ureq），R4 阶段可迁移至异步。
pub struct LlmClient {
    /// 模型配置
    config: ModelConfig,
    /// HTTP agent（复用连接池）
    agent: ureq::Agent,
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

        // 构造请求，附带 Authorization header（有 key 时）
        let request = if self.config.api_key.is_empty() {
            self.agent.post(&self.config.endpoint)
        } else {
            self.agent
                .post(&self.config.endpoint)
                .set("Authorization", &format!("Bearer {}", self.config.api_key))
        };

        // 发送 JSON 请求体（send_json 接受任意 Serialize）
        let response = request
            .send_json(body)
            .map_err(|e| PadaError::Llm(format!("HTTP 请求失败: {}", e)))?;

        // 解析响应 JSON
        let json: serde_json::Value = response
            .into_json()
            .map_err(|e| PadaError::Llm(format!("解析响应 JSON 失败: {}", e)))?;

        Self::parse_response(&json)
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
    /// 当 `reasoning == true` 时额外加入 `"reasoning": true`，
    /// 兼容部分支持推理链的 API（不支持时会被忽略）。
    pub fn build_request_body(&self, messages: &[ChatMessage]) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.config.model_name,
            "messages": messages,
            "temperature": 0.7,
        });

        if self.config.reasoning {
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
