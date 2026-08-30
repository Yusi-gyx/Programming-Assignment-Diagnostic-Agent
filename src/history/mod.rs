//! R5 会话历史与轨迹持久化（开发计划第 14 步）
//!
//! 职责：
//! - 记录诊断会话的每一步输入、工具调用及参数、工具输出、Agent 决策依据
//! - 支持会话保存为 JSON 文件、从文件加载、回放
//! - 供 CLI 命令 `\save <file>` / `--history <file>` 使用
//!
//! 设计原则（AGENTS.md R5 / DESIGN.md §3.3）：
//! - 用户能够查看历史任务列表、查看单次会话的完整工作流程
//! - 轨迹应至少记录：每一步的输入、调用的工具与参数、工具输出、Agent 决策依据
//! - 不把 Agent 当作黑盒，便于用户与开发者审计和回放
//!
//! # 数据结构
//!
//! 一个会话由若干步骤组成，每步记录：
//! - 输入（用户提交的代码 / 题目 / 反馈）
//! - 工具调用（工具名 + 参数 + 输出）
//! - Agent 决策（如选择编译路径还是测试路径）
//!
//! 保存为 JSON 文件，格式参见 [`Session`] 的序列化。

use crate::agent::llm::{ChatMessage, LlmResponse};
use crate::telemetry::UsageRecord;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

// ============================================================
// 轨迹记录
// ============================================================

/// 一次工具调用的轨迹记录。
///
/// 记录调用了哪个工具、传入了什么参数、得到了什么输出。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// 工具名称（如 `"compile_file"`、`"run_tests"`、`"llm_chat"`）
    pub tool: String,
    /// 参数摘要（JSON 字符串，便于序列化）
    pub params: String,
    /// 输出摘要
    pub output: String,
    /// 调用时的时间戳（Unix 秒）
    pub timestamp: u64,
}

impl ToolCall {
    /// 创建一条工具调用记录。
    pub fn new(tool: impl Into<String>, params: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            params: params.into(),
            output: output.into(),
            timestamp: now_ts(),
        }
    }
}

/// Agent 的一次决策记录。
///
/// 记录为何选择某条路径（如编译失败→分析错误，而非运行测试）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDecision {
    /// 决策描述
    pub reasoning: String,
    /// 决策时的阶段
    pub stage: String,
    /// 时间戳
    pub timestamp: u64,
}

impl AgentDecision {
    pub fn new(stage: impl Into<String>, reasoning: impl Into<String>) -> Self {
        Self {
            reasoning: reasoning.into(),
            stage: stage.into(),
            timestamp: now_ts(),
        }
    }
}

/// 会话中的一个步骤。
///
/// 一个步骤可能包含：用户输入、工具调用、Agent 决策、LLM 响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStep {
    /// 步骤序号（从 0 开始）
    pub index: usize,
    /// 用户输入（如有）
    pub user_input: Option<String>,
    /// 工具调用（如有）
    pub tool_calls: Vec<ToolCall>,
    /// Agent 决策（如有）
    pub decisions: Vec<AgentDecision>,
    /// LLM 调用的消息与响应（如有）
    pub llm_exchange: Option<LlmExchange>,
    /// 时间戳
    pub timestamp: u64,
}

/// LLM 的一次交互（请求消息 + 响应）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmExchange {
    /// 发送给 LLM 的消息
    pub messages: Vec<ChatMessage>,
    /// LLM 的响应
    pub response: LlmResponse,
    /// token 用量记录
    pub usage: Option<UsageRecord>,
}

// ============================================================
// 会话
// ============================================================

/// 完整的诊断会话，包含全部步骤与用量统计。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// 会话 ID
    pub id: String,
    /// 题目标题
    pub title: String,
    /// 创建时间戳
    pub created_at: u64,
    /// 最后更新时间戳
    pub updated_at: u64,
    /// 全部步骤
    pub steps: Vec<SessionStep>,
    /// 会话内的 token 用量记录
    pub usage_records: Vec<UsageRecord>,
}

impl Session {
    /// 创建新会话。
    pub fn new(title: impl Into<String>) -> Self {
        let ts = now_ts();
        Self {
            id: format!("session_{}", ts),
            title: title.into(),
            created_at: ts,
            updated_at: ts,
            steps: Vec::new(),
            usage_records: Vec::new(),
        }
    }

    /// 添加一个步骤并更新时间戳。
    pub fn add_step(&mut self, step: SessionStep) {
        self.updated_at = now_ts();
        self.steps.push(step);
    }

    /// 记录一次 LLM 用量。
    pub fn record_usage(&mut self, record: UsageRecord) {
        self.usage_records.push(record);
    }

    /// 获取步骤数。
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// 保存为 JSON 文件。
    pub fn save(&self, path: &Path) -> crate::error::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| crate::error::PadaError::Parse(format!("序列化会话失败: {}", e)))?;
        std::fs::write(path, json)
            .map_err(|e| crate::error::PadaError::Config(format!("写入会话文件失败: {}", e)))?;
        Ok(())
    }

    /// 从 JSON 文件加载会话。
    pub fn load(path: &Path) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::error::PadaError::Config(format!("读取会话文件失败: {}", e)))?;
        let session: Session = serde_json::from_str(&content)
            .map_err(|e| crate::error::PadaError::Parse(format!("解析会话 JSON 失败: {}", e)))?;
        Ok(session)
    }

    /// 生成会话摘要（用于历史列表）。
    pub fn summary(&self) -> String {
        format!(
            "[{}] {} ({} 步, {} 条用量记录, 创建于 {})",
            self.id,
            self.title,
            self.step_count(),
            self.usage_records.len(),
            self.created_at
        )
    }
}

// ============================================================
// 步骤构建器
// ============================================================

/// 用于逐步构建 [`SessionStep`]。
pub struct StepBuilder {
    index: usize,
    user_input: Option<String>,
    tool_calls: Vec<ToolCall>,
    decisions: Vec<AgentDecision>,
    llm_exchange: Option<LlmExchange>,
}

impl StepBuilder {
    /// 创建步骤构建器。
    pub fn new(index: usize) -> Self {
        Self {
            index,
            user_input: None,
            tool_calls: Vec::new(),
            decisions: Vec::new(),
            llm_exchange: None,
        }
    }

    /// 设置用户输入。
    pub fn user_input(mut self, input: impl Into<String>) -> Self {
        self.user_input = Some(input.into());
        self
    }

    /// 添加工具调用。
    pub fn tool_call(mut self, call: ToolCall) -> Self {
        self.tool_calls.push(call);
        self
    }

    /// 添加 Agent 决策。
    pub fn decision(mut self, decision: AgentDecision) -> Self {
        self.decisions.push(decision);
        self
    }

    /// 设置 LLM 交互。
    pub fn llm_exchange(mut self, exchange: LlmExchange) -> Self {
        self.llm_exchange = Some(exchange);
        self
    }

    /// 构建步骤。
    pub fn build(self) -> SessionStep {
        SessionStep {
            index: self.index,
            user_input: self.user_input,
            tool_calls: self.tool_calls,
            decisions: self.decisions,
            llm_exchange: self.llm_exchange,
            timestamp: now_ts(),
        }
    }
}

// ============================================================
// 辅助函数
// ============================================================

/// 获取当前 Unix 时间戳（秒）。
fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
