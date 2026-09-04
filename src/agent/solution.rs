//! Level 5 参考方案生成与会话记录。

use crate::agent::llm::{LlmClient, compile_solution_messages, test_solution_messages};
use crate::analysis::hint::Hint;
use crate::config::model::ModelConfig;
use crate::history::{AgentDecision, LlmExchange, Session, StepBuilder};
use crate::memory::{KnowledgeProfile, now_timestamp};
use crate::models::{Assignment, HintLevel};
use crate::report::DiagnosticReport;
use crate::telemetry::{UsageRecord, UsageTracker};
use std::collections::HashMap;

pub trait SolutionModel {
    fn chat(
        &self,
        messages: &[crate::agent::llm::ChatMessage],
    ) -> crate::error::Result<crate::agent::llm::LlmResponse>;
}

impl SolutionModel for LlmClient {
    fn chat(
        &self,
        messages: &[crate::agent::llm::ChatMessage],
    ) -> crate::error::Result<crate::agent::llm::LlmResponse> {
        LlmClient::chat(self, messages)
    }
}

pub struct SolutionHintService {
    config: Option<ModelConfig>,
    model: Option<Box<dyn SolutionModel>>,
    cache: HashMap<String, String>,
}

impl SolutionHintService {
    pub fn new(config: Option<ModelConfig>) -> Self {
        let model = config
            .as_ref()
            .map(|config| Box::new(LlmClient::new(config.clone())) as Box<dyn SolutionModel>);
        Self {
            config,
            model,
            cache: HashMap::new(),
        }
    }

    /// 注入模型实现，供离线测试或其他兼容后端使用。
    pub fn with_model(config: ModelConfig, model: Box<dyn SolutionModel>) -> Self {
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
        let needs_solution = report
            .compile_entries
            .iter()
            .any(|entry| entry.hint.level == HintLevel::Solution)
            || report
                .test_entries
                .iter()
                .any(|entry| entry.hint.level == HintLevel::Solution);
        if !needs_solution {
            return;
        }
        let (Some(config), Some(model)) = (self.config.as_ref(), self.model.as_ref()) else {
            return;
        };
        let profile_summary = knowledge.prompt_summary_at(now_timestamp());

        for entry in &mut report.compile_entries {
            if entry.hint.level != HintLevel::Solution {
                continue;
            }
            let key = format!(
                "compile:{}:{:?}:{}:{}",
                stable_context_key(source_context),
                entry.diag.location,
                entry.diag.code.as_deref().unwrap_or(""),
                entry.diag.message
            );
            if let Some(content) = self.cache.get(&key) {
                entry.hint = Hint::new(HintLevel::Solution, content.clone());
                continue;
            }
            if !tracker.check_budget() {
                entry.hint = budget_hint();
                continue;
            }
            let messages = compile_solution_messages(
                assignment,
                source_context,
                &entry.diag,
                &entry.classified,
                &profile_summary,
            );
            match model.chat(&messages) {
                Ok(response) => {
                    record_exchange(
                        session,
                        tracker,
                        config,
                        messages,
                        &response,
                        "使用已配置模型生成编译错误参考方案",
                    );
                    self.cache.insert(key, response.content.clone());
                    entry.hint = Hint::new(HintLevel::Solution, response.content);
                }
                Err(error) => entry.hint = failed_hint(error),
            }
        }

        for entry in &mut report.test_entries {
            if entry.hint.level != HintLevel::Solution {
                continue;
            }
            let key = format!(
                "test:{}:{}:{}:{}",
                stable_context_key(source_context),
                entry.result.name,
                entry.result.expected_output,
                entry.result.actual_output
            );
            if let Some(content) = self.cache.get(&key) {
                entry.hint = Hint::new(HintLevel::Solution, content.clone());
                continue;
            }
            if !tracker.check_budget() {
                entry.hint = budget_hint();
                continue;
            }
            let messages =
                test_solution_messages(assignment, source_context, &entry.result, &profile_summary);
            match model.chat(&messages) {
                Ok(response) => {
                    record_exchange(
                        session,
                        tracker,
                        config,
                        messages,
                        &response,
                        "使用已配置模型生成测试失败参考方案",
                    );
                    self.cache.insert(key, response.content.clone());
                    entry.hint = Hint::new(HintLevel::Solution, response.content);
                }
                Err(error) => entry.hint = failed_hint(error),
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
            .decision(AgentDecision::new("level_5_hint", decision))
            .build(),
    );
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
