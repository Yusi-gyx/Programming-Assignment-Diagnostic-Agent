//! Level 3-5 模型增强提示生成、格式清理与会话记录。

use crate::agent::llm::{
    ChatModel, LlmClient, compile_hint_messages_with_policy, test_hint_messages,
};
use crate::agent::model_task::{ModelTaskOutcome, run_model_task_streaming};
use crate::analysis::hint::Hint;
use crate::config::effort::{EffortMode, EffortPolicy, ModelCallBudget};
use crate::config::model::ModelConfig;
use crate::history::{AgentDecision, LlmExchange, Session, StepBuilder};
use crate::memory::{KnowledgeProfile, now_timestamp};
use crate::models::{Assignment, HintLevel};
use crate::report::DiagnosticReport;
use crate::telemetry::{UsageRecord, UsageTracker};
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

pub struct SolutionHintService {
    config: Option<ModelConfig>,
    model: Option<Arc<dyn ChatModel>>,
    cache: HashMap<String, String>,
    policy: EffortPolicy,
}

#[derive(Debug, Default)]
pub struct StreamedReportEntries {
    pub compile: Vec<usize>,
    pub tests: Vec<usize>,
}

impl SolutionHintService {
    pub fn new(config: Option<ModelConfig>) -> Self {
        Self::with_effort(config, EffortPolicy::for_mode(EffortMode::Medium))
    }

    pub fn with_effort(config: Option<ModelConfig>, policy: EffortPolicy) -> Self {
        let model = config.as_ref().map(|config| {
            Arc::new(LlmClient::with_effort(config.clone(), policy)) as Arc<dyn ChatModel>
        });
        Self {
            config,
            model,
            cache: HashMap::new(),
            policy,
        }
    }

    /// 注入模型实现，供离线测试或其他兼容后端使用。
    pub fn with_model(config: ModelConfig, model: Box<dyn ChatModel>) -> Self {
        Self::with_model_and_effort(config, model, EffortPolicy::for_mode(EffortMode::Medium))
    }

    pub fn with_model_and_effort(
        config: ModelConfig,
        model: Box<dyn ChatModel>,
        policy: EffortPolicy,
    ) -> Self {
        Self {
            config: Some(config),
            model: Some(Arc::from(model)),
            cache: HashMap::new(),
            policy,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn enrich(
        &mut self,
        report: &mut DiagnosticReport,
        assignment: &Assignment,
        source_context: &str,
        knowledge: &KnowledgeProfile,
        tracker: &mut UsageTracker,
        session: &mut Session,
        interactive: bool,
    ) -> StreamedReportEntries {
        let mut budget = ModelCallBudget::new(self.policy);
        self.enrich_with_budget(
            report,
            assignment,
            source_context,
            knowledge,
            tracker,
            session,
            interactive,
            &mut budget,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn enrich_with_budget(
        &mut self,
        report: &mut DiagnosticReport,
        assignment: &Assignment,
        source_context: &str,
        knowledge: &KnowledgeProfile,
        tracker: &mut UsageTracker,
        session: &mut Session,
        interactive: bool,
        call_budget: &mut ModelCallBudget,
    ) -> StreamedReportEntries {
        let mut streamed = StreamedReportEntries::default();
        let needs_model_hint = report
            .compile_entries
            .iter()
            .any(|entry| model_enriches(entry.hint.level))
            || report
                .test_entries
                .iter()
                .any(|entry| model_enriches(entry.hint.level));
        if !needs_model_hint {
            return streamed;
        }
        let (Some(config), Some(model)) = (self.config.as_ref(), self.model.as_ref()) else {
            return streamed;
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
        let colored = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        let compile_prefixes = (0..report.compile_entries.len())
            .map(|index| report.compile_stream_prefix(index, colored))
            .collect::<Vec<_>>();
        let test_prefixes = (0..report.test_entries.len())
            .map(|index| report.test_stream_prefix(index, colored))
            .collect::<Vec<_>>();

        for (index, entry) in report.compile_entries.iter_mut().enumerate() {
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
            if !call_budget.try_take() {
                entry.hint = call_limit_hint(level, self.policy.max_model_calls);
                continue;
            }
            let messages = compile_hint_messages_with_policy(
                assignment,
                source_context,
                &entry.diag,
                &entry.classified,
                level,
                &entry.hint.content,
                &profile_summary,
                self.policy,
            );
            model_call_started(level, current, total, interactive);
            print!("\n{}", compile_prefixes[index]);
            let _ = io::stdout().flush();
            streamed.compile.push(index);
            match run_model_task_streaming(Arc::clone(model), &messages, interactive, |chunk| {
                if colored {
                    print!("\x1b[34m{chunk}\x1b[0m");
                } else {
                    print!("{chunk}");
                }
                let _ = io::stdout().flush();
            }) {
                ModelTaskOutcome::Completed(Ok(response)) => {
                    println!();
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
                ModelTaskOutcome::Completed(Err(error)) => {
                    println!("模型生成失败：{error}");
                    model_call_failed(current, total, &error);
                    if level == HintLevel::Solution {
                        entry.hint = failed_hint(error);
                    }
                }
                ModelTaskOutcome::Cancelled => {
                    println!("模型生成已取消。");
                    model_call_cancelled(current, total);
                    if level == HintLevel::Solution {
                        entry.hint = cancelled_hint();
                    }
                    return streamed;
                }
            }
        }

        for (index, entry) in report.test_entries.iter_mut().enumerate() {
            let level = entry.hint.level;
            if !model_enriches(level) {
                continue;
            }
            current += 1;
            let key = format!(
                "test:{level:?}:{}:{}:{}:{}:{}",
                stable_context_key(source_context),
                entry.result.name,
                entry.result.expected_output,
                entry.result.actual_output,
                entry.result.runtime_error.as_deref().unwrap_or("")
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
            if !call_budget.try_take() {
                entry.hint = call_limit_hint(level, self.policy.max_model_calls);
                continue;
            }
            let scoped_source =
                crate::agent::context::limit_source(source_context, self.policy.source);
            let messages = test_hint_messages(
                assignment,
                &scoped_source,
                &entry.result,
                &entry.classified,
                level,
                &entry.hint.content,
                &profile_summary,
            );
            model_call_started(level, current, total, interactive);
            print!("\n{}", test_prefixes[index]);
            let _ = io::stdout().flush();
            streamed.tests.push(index);
            match run_model_task_streaming(Arc::clone(model), &messages, interactive, |chunk| {
                if colored {
                    print!("\x1b[34m{chunk}\x1b[0m");
                } else {
                    print!("{chunk}");
                }
                let _ = io::stdout().flush();
            }) {
                ModelTaskOutcome::Completed(Ok(response)) => {
                    println!();
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
                ModelTaskOutcome::Completed(Err(error)) => {
                    println!("模型生成失败：{error}");
                    model_call_failed(current, total, &error);
                    if level == HintLevel::Solution {
                        entry.hint = failed_hint(error);
                    }
                }
                ModelTaskOutcome::Cancelled => {
                    println!("模型生成已取消。");
                    model_call_cancelled(current, total);
                    if level == HintLevel::Solution {
                        entry.hint = cancelled_hint();
                    }
                    return streamed;
                }
            }
        }
        streamed
    }
}

fn call_limit_hint(level: HintLevel, limit: usize) -> Hint {
    Hint::new(
        level,
        format!("当前思考模式最多允许 {limit} 次模型调用；本轮其余问题保留 Rust 基础诊断。"),
    )
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

fn model_call_started(level: HintLevel, current: usize, total: usize, interactive: bool) {
    eprintln!(
        "⏳ 正在调用模型生成 Level {} 提示（{current}/{total}），请稍候…",
        hint_level_number(level)
    );
    if interactive {
        eprintln!("   输入 q 或 cancel 并回车，可停止本次模型生成。");
    }
}

fn model_call_finished(current: usize, total: usize) {
    eprintln!("✓ 模型提示生成完成（{current}/{total}）");
}

fn model_call_failed(current: usize, total: usize, error: &crate::error::PadaError) {
    eprintln!("⚠ 模型提示生成失败（{current}/{total}）：{error}");
}

fn model_call_cancelled(current: usize, total: usize) {
    eprintln!("■ 已取消模型提示生成（{current}/{total}）");
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

fn cancelled_hint() -> Hint {
    Hint::new(
        HintLevel::Solution,
        "模型提示生成已取消。可以再次使用 show 或 hint 5 重试。",
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
