//! Level 3-5 模型增强提示生成、格式清理与会话记录。

use crate::agent::llm::{ChatModel, LlmClient, compile_hint_messages, test_hint_messages};
use crate::analysis::hint::Hint;
use crate::config::model::ModelConfig;
use crate::history::{AgentDecision, LlmExchange, Session, StepBuilder};
use crate::memory::{KnowledgeProfile, now_timestamp};
use crate::models::{Assignment, HintLevel};
use crate::report::DiagnosticReport;
use crate::telemetry::{UsageRecord, UsageTracker};
use std::collections::HashMap;

pub struct SolutionHintService {
    config: Option<ModelConfig>,
    model: Option<Box<dyn ChatModel>>,
    cache: HashMap<String, String>,
}

impl SolutionHintService {
    pub fn new(config: Option<ModelConfig>) -> Self {
        let model = config
            .as_ref()
            .map(|config| Box::new(LlmClient::new(config.clone())) as Box<dyn ChatModel>);
        Self {
            config,
            model,
            cache: HashMap::new(),
        }
    }

    /// 注入模型实现，供离线测试或其他兼容后端使用。
    pub fn with_model(config: ModelConfig, model: Box<dyn ChatModel>) -> Self {
        Self {
            config: Some(config),
            model: Some(model),
            cache: HashMap::new(),
        }
    }

    pub fn enrich(
        &mut self,
        report: &mut DiagnosticReport,
        assignment: &Assignment,
        source_context: &str,
        knowledge: &KnowledgeProfile,
        tracker: &mut UsageTracker,
        session: &mut Session,
    ) {
        let needs_model_hint = report
            .compile_entries
            .iter()
            .any(|entry| model_enriches(entry.hint.level))
            || report
                .test_entries
                .iter()
                .any(|entry| model_enriches(entry.hint.level));
        if !needs_model_hint {
            return;
        }
        let (Some(config), Some(model)) = (self.config.as_ref(), self.model.as_ref()) else {
            return;
        };
        let profile_summary = knowledge.prompt_summary_at(now_timestamp());
        let total = report
            .compile_entries
            .iter()
            .filter(|entry| model_enriches(entry.hint.level))
            .count()
            + report
                .test_entries
                .iter()
                .filter(|entry| model_enriches(entry.hint.level))
                .count();
        let mut current = 0;

        for entry in &mut report.compile_entries {
            let level = entry.hint.level;
            if !model_enriches(level) {
                continue;
            }
            current += 1;
            let key = format!(
                "compile:{level:?}:{}:{:?}:{}:{}",
                stable_context_key(source_context),
                entry.diag.location,
                entry.diag.code.as_deref().unwrap_or(""),
                entry.diag.message
            );
            if let Some(content) = self.cache.get(&key) {
                entry.hint = Hint::new(level, content.clone());
                continue;
            }
            if !tracker.check_budget() {
                if level == HintLevel::Solution {
                    entry.hint = budget_hint();
                }
                continue;
            }
            let messages = compile_hint_messages(
                assignment,
                source_context,
                &entry.diag,
                &entry.classified,
                level,
                &entry.hint.content,
                &profile_summary,
            );
            model_call_started(level, current, total);
            match model.chat(&messages) {
                Ok(response) => {
                    record_exchange(
                        session,
                        tracker,
                        config,
                        messages,
                        &response,
                        level,
                        "使用已配置模型生成编译错误分层提示",
                    );
                    let content = format_model_output(&response.content);
                    self.cache.insert(key, content.clone());
                    entry.hint = Hint::new(level, content);
                    model_call_finished(current, total);
                }
                Err(error) => {
                    model_call_failed(current, total, &error);
                    if level == HintLevel::Solution {
                        entry.hint = failed_hint(error);
                    }
                }
            }
        }

        for entry in &mut report.test_entries {
            let level = entry.hint.level;
            if !model_enriches(level) {
                continue;
            }
            current += 1;
            let key = format!(
                "test:{level:?}:{}:{}:{}:{}",
                stable_context_key(source_context),
                entry.result.name,
                entry.result.expected_output,
                entry.result.actual_output
            );
            if let Some(content) = self.cache.get(&key) {
                entry.hint = Hint::new(level, content.clone());
                continue;
            }
            if !tracker.check_budget() {
                if level == HintLevel::Solution {
                    entry.hint = budget_hint();
                }
                continue;
            }
            let messages = test_hint_messages(
                assignment,
                source_context,
                &entry.result,
                &entry.classified,
                level,
                &entry.hint.content,
                &profile_summary,
            );
            model_call_started(level, current, total);
            match model.chat(&messages) {
                Ok(response) => {
                    record_exchange(
                        session,
                        tracker,
                        config,
                        messages,
                        &response,
                        level,
                        "使用已配置模型生成测试失败分层提示",
                    );
                    let content = format_model_output(&response.content);
                    self.cache.insert(key, content.clone());
                    entry.hint = Hint::new(level, content);
                    model_call_finished(current, total);
                }
                Err(error) => {
                    model_call_failed(current, total, &error);
                    if level == HintLevel::Solution {
                        entry.hint = failed_hint(error);
                    }
                }
            }
        }
    }
}

fn record_exchange(
    session: &mut Session,
    tracker: &mut UsageTracker,
    config: &ModelConfig,
    messages: Vec<crate::agent::llm::ChatMessage>,
    response: &crate::agent::llm::LlmResponse,
    level: HintLevel,
    decision: &str,
) {
    let usage = UsageRecord::from_response(response, config);
    tracker.record(response, config);
    session.record_usage(usage.clone());
    session.add_step(
        StepBuilder::new(session.step_count())
            .llm_exchange(LlmExchange {
                messages,
                response: response.clone(),
                usage: Some(usage),
            })
            .decision(AgentDecision::new(
                format!("level_{}_hint", hint_level_number(level)),
                decision,
            ))
            .build(),
    );
}

fn model_enriches(level: HintLevel) -> bool {
    matches!(
        level,
        HintLevel::Concept | HintLevel::Direction | HintLevel::Solution
    )
}

fn hint_level_number(level: HintLevel) -> u8 {
    match level {
        HintLevel::Category => 1,
        HintLevel::Location => 2,
        HintLevel::Concept => 3,
        HintLevel::Direction => 4,
        HintLevel::Solution => 5,
    }
}

fn model_call_started(level: HintLevel, current: usize, total: usize) {
    eprintln!(
        "⏳ 正在调用模型生成 Level {} 提示（{current}/{total}），请稍候…",
        hint_level_number(level)
    );
}

fn model_call_finished(current: usize, total: usize) {
    eprintln!("✓ 模型提示生成完成（{current}/{total}）");
}

fn model_call_failed(current: usize, total: usize, error: &crate::error::PadaError) {
    eprintln!("⚠ 模型提示生成失败（{current}/{total}）：{error}");
}

/// 清理本地推理模型常见的思维链标签和多余 Markdown 外层，使终端输出稳定可读。
pub fn format_model_output(content: &str) -> String {
    let mut content = content.replace("\r\n", "\n").replace('\r', "\n");
    while let Some(start) = content.find("<think>") {
        if let Some(relative_end) = content[start + 7..].find("</think>") {
            let end = start + 7 + relative_end + 8;
            content.replace_range(start..end, "");
        } else {
            content.truncate(start);
            break;
        }
    }
    let trimmed = content.trim();
    let content = ["```markdown\n", "```md\n"]
        .iter()
        .find_map(|prefix| {
            trimmed
                .strip_prefix(prefix)
                .and_then(|value| value.strip_suffix("```"))
        })
        .unwrap_or(trimmed);

    let mut rendered: Vec<String> = Vec::new();
    let mut previous_blank = false;
    let mut in_code = false;
    for raw_line in content.lines() {
        let line = raw_line.trim_end();
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if !in_code && line.starts_with('#') && rendered.last().is_some_and(|line| !line.is_empty())
        {
            rendered.push(String::new());
        }
        if line.is_empty() {
            if !previous_blank {
                rendered.push(String::new());
            }
            previous_blank = true;
        } else {
            rendered.push(line.to_owned());
            previous_blank = false;
        }
    }
    while rendered.last().is_some_and(String::is_empty) {
        rendered.pop();
    }
    let rendered = rendered.join("\n");
    if rendered.trim().is_empty() {
        "模型未返回可展示内容。".into()
    } else {
        rendered
    }
}

fn budget_hint() -> Hint {
    Hint::new(
        HintLevel::Solution,
        "Token 预算已用尽，未调用 LLM。请提高 --budget 后重试。",
    )
}

fn failed_hint(error: crate::error::PadaError) -> Hint {
    Hint::new(
        HintLevel::Solution,
        format!("LLM 参考方案生成失败：{error}。请检查 endpoint、API key 和网络。"),
    )
}

fn stable_context_key(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
