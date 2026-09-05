//! R6 Token 用量与成本统计
//!
//! 职责（开发计划第 10 步）：
//! - 精确统计每次 LLM 调用的输入 / 输出 token 数（直接取自 API 响应）
//! - 基于模型配置的价格实时换算成本
//! - 支持三级用量查询：单次调用、当前会话累计、历史累计
//! - 支持会话 / 周期预算，达到预算时阻止后续调用
//!
//! 设计原则（AGENTS.md R6）：
//! - token 数优先直接读取 API 响应，不做本地估算
//! - 预算计算、成本换算由 Rust 完成，不依赖 LLM
//!
//! # 用量数据流
//!
//! ```text
//! LLM 调用返回 LlmResponse（含 input_tokens / output_tokens）
//!          ↓
//! UsageTracker::record() 记录到会话
//!          ↓
//!   ┌──────┴──────┐
//!   │  会话累计    │ → SessionUsage（内存）
//!   │  历史累计    │ → HistoryTotal（持久化到 JSON 文件）
//!   └─────────────┘
//!          ↓
//! check_budget() 检查是否超预算 → 超出则阻止后续调用
//! ```

use crate::agent::llm::LlmResponse;
use crate::config::model::ModelConfig;
use crate::error::{PadaError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

// ============================================================
// 成本换算
// ============================================================

/// 根据模型价格与 token 用量计算成本。
///
/// 价格单位：每百万 token（与 [`ModelConfig::input_price`] 一致）。
/// 返回值单位与价格单位相同（元或美元）。
///
/// # 计算公式
///
/// ```text
/// cost = input_tokens * input_price / 1_000_000
///      + output_tokens * output_price / 1_000_000
/// ```
///
/// 这是纯函数，便于离线测试。
pub fn calculate_cost(input_tokens: usize, output_tokens: usize, config: &ModelConfig) -> f64 {
    let input_cost = input_tokens as f64 * config.input_price / 1_000_000.0;
    let output_cost = output_tokens as f64 * config.output_price / 1_000_000.0;
    input_cost + output_cost
}

// ============================================================
// 单次用量记录
// ============================================================

/// 单次 LLM 调用的用量记录。
///
/// 每次调用 [`UsageTracker::record`] 时生成一条。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageRecord {
    /// 输入 token 数（取自 API 响应）
    pub input_tokens: usize,
    /// 输出 token 数（取自 API 响应）
    pub output_tokens: usize,
    /// 本次调用成本
    pub cost: f64,
    /// 使用的模型名
    pub model: String,
    /// 记录时间（Unix 时间戳，秒）
    pub timestamp: u64,
}

impl UsageRecord {
    /// 从 LLM 响应与模型配置构造一条用量记录。
    ///
    /// 时间戳取当前系统时间。
    pub fn from_response(resp: &LlmResponse, config: &ModelConfig) -> Self {
        let cost = calculate_cost(resp.input_tokens, resp.output_tokens, config);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            input_tokens: resp.input_tokens,
            output_tokens: resp.output_tokens,
            cost,
            model: resp.model.clone(),
            timestamp,
        }
    }
}

// ============================================================
// 会话用量
// ============================================================

/// 当前会话的累计用量。
///
/// 存在于内存中，会话结束后合并到历史累计。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionUsage {
    /// 累计输入 token
    pub total_input_tokens: usize,
    /// 累计输出 token
    pub total_output_tokens: usize,
    /// 累计成本
    pub total_cost: f64,
    /// 逐条记录（供回放 / 审计）
    pub records: Vec<UsageRecord>,
}

impl SessionUsage {
    /// 创建空的会话用量。
    pub fn new() -> Self {
        Self {
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            records: Vec::new(),
        }
    }

    /// 累计总 token 数（输入 + 输出）。
    pub fn total_tokens(&self) -> usize {
        self.total_input_tokens + self.total_output_tokens
    }

    /// 追加一条用量记录并更新累计值。
    fn add(&mut self, record: UsageRecord) {
        self.total_input_tokens += record.input_tokens;
        self.total_output_tokens += record.output_tokens;
        self.total_cost += record.cost;
        self.records.push(record);
    }
}

impl Default for SessionUsage {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 历史累计用量（持久化）
// ============================================================

/// 历史累计用量，持久化到 JSON 文件。
///
/// 每次会话结束后通过 [`UsageTracker::merge_session_to_history`] 合并。
/// 下次启动时通过 [`UsageTracker::load_history`] 恢复。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryTotal {
    /// 历史累计输入 token
    pub total_input_tokens: usize,
    /// 历史累计输出 token
    pub total_output_tokens: usize,
    /// 历史累计成本
    pub total_cost: f64,
}

impl HistoryTotal {
    /// 创建空的历史累计。
    pub fn new() -> Self {
        Self {
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
        }
    }

    /// 合并一段会话用量到历史累计。
    fn merge(&mut self, session: &SessionUsage) {
        self.total_input_tokens += session.total_input_tokens;
        self.total_output_tokens += session.total_output_tokens;
        self.total_cost += session.total_cost;
    }
}

impl Default for HistoryTotal {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 用量追踪器
// ============================================================

/// 用量追踪器，整合会话级、历史级统计与预算控制。
///
/// 典型生命周期：
/// 1. 启动时 `load_history()` 恢复历史累计
/// 2. 每次调用后 `record()` 记录用量
/// 3. 调用前 `check_budget()` 判断是否超预算
/// 4. 会话结束 `save_history()` 持久化
pub struct UsageTracker {
    /// 当前会话用量
    session: SessionUsage,
    /// 历史累计用量
    history: HistoryTotal,
    /// 会话 token 预算（None 表示不限制）
    session_budget: Option<usize>,
    /// 周期 token 预算（None 表示不限制）
    period_budget: Option<usize>,
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageTracker {
    /// 创建空的用量追踪器（无预算限制、无历史）。
    pub fn new() -> Self {
        Self {
            session: SessionUsage::new(),
            history: HistoryTotal::new(),
            session_budget: None,
            period_budget: None,
        }
    }

    /// 设置会话 token 预算（总 token = 输入 + 输出）。
    ///
    /// 对应 CLI 参数 `--budget <n>`（DESIGN.md §5）。
    pub fn set_session_budget(&mut self, tokens: usize) {
        self.session_budget = Some(tokens);
    }

    /// 设置周期 token 预算。
    pub fn set_period_budget(&mut self, tokens: usize) {
        self.period_budget = Some(tokens);
    }

    /// 记录一次 LLM 调用的用量。
    ///
    /// 会更新会话累计。历史累计需在会话结束时显式合并。
    pub fn record(&mut self, resp: &LlmResponse, config: &ModelConfig) {
        let record = UsageRecord::from_response(resp, config);
        self.session.add(record);
    }

    /// 检查是否仍在预算内（可以继续调用）。
    ///
    /// - 会话预算：当前会话总 token 是否 < 预算
    /// - 周期预算：历史 + 会话总 token 是否 < 预算
    ///
    /// 返回 `true` 表示可以调用，`false` 表示已超预算应中断。
    pub fn check_budget(&self) -> bool {
        // 检查会话预算
        if let Some(budget) = self.session_budget
            && self.session.total_tokens() >= budget
        {
            return false;
        }
        // 检查周期预算（历史 + 会话）
        if let Some(budget) = self.period_budget {
            let period_used = self.history.total_input_tokens
                + self.history.total_output_tokens
                + self.session.total_tokens();
            if period_used >= budget {
                return false;
            }
        }
        true
    }

    /// 获取当前会话用量。
    pub fn session(&self) -> &SessionUsage {
        &self.session
    }

    /// 获取历史累计用量。
    pub fn history(&self) -> &HistoryTotal {
        &self.history
    }

    /// 将当前会话用量合并到历史累计。
    ///
    /// 通常在会话结束时调用，配合 [`save_history`] 持久化。
    pub fn merge_session_to_history(&mut self) {
        self.history.merge(&self.session);
    }

    /// 从 JSON 文件加载历史累计。
    pub fn load_history(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| PadaError::Config(format!("读取历史用量失败: {}", e)))?;
        let history: HistoryTotal = serde_json::from_str(&content)
            .map_err(|e| PadaError::Parse(format!("解析历史用量 JSON 失败: {}", e)))?;
        Ok(Self {
            session: SessionUsage::new(),
            history,
            session_budget: None,
            period_budget: None,
        })
    }

    /// 将历史累计保存为 JSON 文件。
    ///
    /// 注意：保存前应先调用 [`merge_session_to_history`]。
    pub fn save_history(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.history)
            .map_err(|e| PadaError::Parse(format!("序列化历史用量失败: {}", e)))?;
        std::fs::write(path, json)
            .map_err(|e| PadaError::Config(format!("写入历史用量失败: {}", e)))?;
        Ok(())
    }

    /// 生成人类可读的用量摘要（对应 CLI 命令 `\usage`，DESIGN.md §5）。
    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str("┌─ Token 用量 ──────────────────────────\n");
        s.push_str(&format!(
            "│ 本次会话  输入 {} / 输出 {} / 合计 {} / 成本 {:.6}\n",
            self.session.total_input_tokens,
            self.session.total_output_tokens,
            self.session.total_tokens(),
            self.session.total_cost
        ));
        s.push_str(&format!(
            "│ 历史累计  输入 {} / 输出 {} / 合计 {} / 成本 {:.6}\n",
            self.history.total_input_tokens,
            self.history.total_output_tokens,
            self.history.total_input_tokens + self.history.total_output_tokens,
            self.history.total_cost
        ));
        if let Some(budget) = self.session_budget {
            s.push_str(&format!(
                "│ 会话预算  总额 {} / 已用 {} / 剩余 {}\n",
                budget,
                self.session.total_tokens(),
                budget.saturating_sub(self.session.total_tokens())
            ));
        }
        s.push_str("└──────────────────────────────────────\n");
        s
    }
}
