//! 使用配置模型为黑盒测试失败映射 Rust 知识点。

use crate::agent::llm::{ChatMessage, ChatModel, LlmClient, LlmResponse, ModelTaskKind};
use crate::agent::model_task::{ModelTaskOutcome, run_recorded_model_task};
use crate::analysis::classifier::classify_test_result;
use crate::config::effort::{EffortMode, EffortPolicy, ModelCallBudget};
use crate::config::model::ModelConfig;
use crate::error::{PadaError, Result};
use crate::history::{AgentDecision, LlmExchange, Session, StepBuilder};
use crate::models::{Assignment, Diagnostic, KnowledgePoint, TestResult};
use crate::telemetry::{UsageRecord, UsageTracker};
use serde::Deserialize;
use std::io::IsTerminal;
use std::sync::Arc;

pub struct TestKnowledgeMapper {
    config: Option<ModelConfig>,
    model: Option<Arc<dyn ChatModel>>,
    policy: EffortPolicy,
    cache: std::cell::RefCell<std::collections::HashMap<String, Vec<Diagnostic>>>,
}

impl TestKnowledgeMapper {
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
            policy,
            cache: Default::default(),
        }
    }

    pub fn with_model(config: ModelConfig, model: Box<dyn ChatModel>) -> Self {
        Self {
            config: Some(config),
            model: Some(Arc::from(model)),
            policy: EffortPolicy::for_mode(EffortMode::Medium),
            cache: Default::default(),
        }
    }

    pub fn map_failures(
        &self,
        assignment: &Assignment,
        source_context: &str,
        failures: &[TestResult],
        tracker: &mut UsageTracker,
        session: &mut Session,
    ) -> Result<Vec<Diagnostic>> {
        let mut budget = ModelCallBudget::new(self.policy);
        self.map_failures_with_budget(
            assignment,
            source_context,
            failures,
            tracker,
            session,
            &mut budget,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn map_failures_with_budget(
        &self,
        assignment: &Assignment,
        source_context: &str,
        failures: &[TestResult],
        tracker: &mut UsageTracker,
        session: &mut Session,
        call_budget: &mut ModelCallBudget,
    ) -> Result<Vec<Diagnostic>> {
        let fallback = || {
            failures
                .iter()
                .map(classify_test_result)
                .collect::<Vec<_>>()
        };
        if failures.is_empty() {
            return Ok(Vec::new());
        }
        let (Some(config), Some(model)) = (self.config.as_ref(), self.model.as_ref()) else {
            return Ok(fallback());
        };
        let full_source = source_context;
        let source_context =
            crate::agent::context::limit_source(source_context, self.policy.source);
        let messages = mapping_messages(assignment, &source_context, failures);
        let key = serde_json::json!({"messages": messages, "failures": failures, "full_source": full_source}).to_string();
        if let Some(mapped) = self.cache.borrow().get(&key) {
            session.add_step(
                StepBuilder::new(session.step_count())
                    .decision(AgentDecision::new(
                        "test_mapping_cache",
                        "题目、源码和失败结果未变化，复用已校验知识点映射，不消耗模型调用预算",
                    ))
                    .build(),
            );
            return Ok(mapped.clone());
        }
        if !tracker.check_budget() {
            return Ok(fallback());
        }
        if !call_budget.try_take() {
            return Ok(fallback());
        }

        eprintln!("正在调用模型映射测试知识点（输入 q / cancel 并回车可停止）");
        let response = match run_recorded_model_task(
            Arc::clone(model),
            &messages,
            std::io::stdin().is_terminal(),
            ModelTaskKind::KnowledgeMapping,
            session,
            |_| {},
        ) {
            ModelTaskOutcome::Completed(result) => result?,
            ModelTaskOutcome::Cancelled => return Err(PadaError::Cancelled),
        };
        let mapped = response
            .ensure_complete()
            .and_then(|()| parse_mapping_response(&response.content, failures));
        record_exchange(session, tracker, config, messages, response);
        if let Ok(mapped) = &mapped {
            let mut cache = self.cache.borrow_mut();
            if cache.len() >= 16 {
                cache.clear();
            }
            cache.insert(key, mapped.clone());
        }
        mapped
    }
}

pub fn mapping_messages(
    assignment: &Assignment,
    source_context: &str,
    failures: &[TestResult],
) -> Vec<ChatMessage> {
    let cases = failures
        .iter()
        .enumerate()
        .map(|(index, result)| {
            format!(
                "[{index}] {}\n期望: {:?}\n实际: {:?}\n运行错误: {}",
                result.name,
                result.expected_output.trim(),
                result.actual_output.trim(),
                result.runtime_error.as_deref().unwrap_or("无")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    vec![
        ChatMessage::system(
            "你负责把 Rust 程序的黑盒测试失败映射到知识点。只输出 JSON，不要给修改方案。knowledge_points 只能使用：TypeSystem, Syntax, NameResolution, Ownership, Borrowing, Lifetime, Trait, Generic, Iterator, Option, Result, PatternMatching, Collection, ErrorHandling, AlgorithmLogic。每个失败最多选择 3 个最相关知识点；证据不足时使用 AlgorithmLogic。格式：{\"mappings\":[{\"index\":0,\"knowledge_points\":[\"Iterator\"]}]}。",
        ),
        ChatMessage::user(format!(
            "题目：{}\n{}\n\nRust 提交：\n```rust\n{}\n```\n\n失败用例：\n{}",
            assignment.title, assignment.description, source_context, cases
        )),
    ]
}

#[derive(Deserialize)]
struct MappingEnvelope {
    mappings: Vec<MappingItem>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MappingPayload {
    Envelope(MappingEnvelope),
    Items(Vec<MappingItem>),
}

#[derive(Deserialize)]
struct MappingItem {
    index: usize,
    knowledge_points: Vec<String>,
}

pub fn parse_mapping_response(content: &str, failures: &[TestResult]) -> Result<Vec<Diagnostic>> {
    let cleaned = crate::agent::solution::format_model_output(content);
    let payload = parse_mapping_payload(&cleaned).ok_or_else(|| {
        let preview = content.trim().chars().take(300).collect::<String>();
        PadaError::Parse(format!(
            "解析模型知识点映射失败：未找到有效的 JSON 映射；模型输出: {preview}"
        ))
    })?;
    let mappings = match payload {
        MappingPayload::Envelope(envelope) => envelope.mappings,
        MappingPayload::Items(items) => items,
    };
    let mut diagnostics = failures
        .iter()
        .map(classify_test_result)
        .collect::<Vec<_>>();

    for item in mappings {
        let Some(diagnostic) = diagnostics.get_mut(item.index) else {
            continue;
        };
        let mut points = Vec::new();
        for label in item.knowledge_points {
            if let Some(point) = parse_knowledge_point(&label)
                && !points.contains(&point)
                && points.len() < 3
            {
                points.push(point);
            }
        }
        if !points.is_empty() {
            for point in points {
                if !diagnostic.knowledge_points.contains(&point)
                    && diagnostic.knowledge_points.len() < 3
                {
                    diagnostic.knowledge_points.push(point);
                }
            }
            diagnostic.confidence = diagnostic.confidence.max(0.7);
        }
    }
    Ok(diagnostics)
}

fn parse_knowledge_point(label: &str) -> Option<KnowledgePoint> {
    match label.trim() {
        "类型系统" => return Some(KnowledgePoint::TypeSystem),
        "语法" => return Some(KnowledgePoint::Syntax),
        "名称解析" | "模块" => return Some(KnowledgePoint::NameResolution),
        "所有权" => return Some(KnowledgePoint::Ownership),
        "借用" | "可变借用" => return Some(KnowledgePoint::Borrowing),
        "生命周期" => return Some(KnowledgePoint::Lifetime),
        "泛型" => return Some(KnowledgePoint::Generic),
        "特征" => return Some(KnowledgePoint::Trait),
        "迭代器" => return Some(KnowledgePoint::Iterator),
        "可选值" => return Some(KnowledgePoint::Option),
        "结果" => return Some(KnowledgePoint::Result),
        "模式匹配" => return Some(KnowledgePoint::PatternMatching),
        "集合" => return Some(KnowledgePoint::Collection),
        "错误处理" => return Some(KnowledgePoint::ErrorHandling),
        "算法逻辑" => return Some(KnowledgePoint::AlgorithmLogic),
        _ => {}
    }
    match label.trim().to_ascii_lowercase().as_str() {
        "typesystem" | "type_system" | "type system" => Some(KnowledgePoint::TypeSystem),
        "syntax" => Some(KnowledgePoint::Syntax),
        "nameresolution" | "name_resolution" | "name resolution" | "module" | "modules" => {
            Some(KnowledgePoint::NameResolution)
        }
        "ownership" => Some(KnowledgePoint::Ownership),
        "borrowing" | "borrow" => Some(KnowledgePoint::Borrowing),
        "lifetime" => Some(KnowledgePoint::Lifetime),
        "trait" => Some(KnowledgePoint::Trait),
        "generic" | "generics" => Some(KnowledgePoint::Generic),
        "iterator" | "iterators" => Some(KnowledgePoint::Iterator),
        "option" => Some(KnowledgePoint::Option),
        "result" => Some(KnowledgePoint::Result),
        "patternmatching" | "pattern_matching" | "pattern matching" => {
            Some(KnowledgePoint::PatternMatching)
        }
        "collection" | "collections" => Some(KnowledgePoint::Collection),
        "errorhandling" | "error_handling" | "error handling" => {
            Some(KnowledgePoint::ErrorHandling)
        }
        "algorithmlogic" | "algorithm_logic" | "algorithm logic" => {
            Some(KnowledgePoint::AlgorithmLogic)
        }
        _ => None,
    }
}

fn parse_mapping_payload(content: &str) -> Option<MappingPayload> {
    content
        .char_indices()
        .filter(|(_, ch)| matches!(ch, '{' | '['))
        .find_map(|(index, _)| {
            serde_json::Deserializer::from_str(&content[index..])
                .into_iter::<MappingPayload>()
                .next()
                .and_then(std::result::Result::ok)
        })
}

fn record_exchange(
    session: &mut Session,
    tracker: &mut UsageTracker,
    config: &ModelConfig,
    messages: Vec<ChatMessage>,
    response: LlmResponse,
) {
    let usage = UsageRecord::from_response(&response, config);
    tracker.record(&response, config);
    session.record_usage(usage.clone());
    session.add_step(
        StepBuilder::new(session.step_count())
            .llm_exchange(LlmExchange {
                messages,
                response,
                usage: Some(usage),
            })
            .decision(AgentDecision::new(
                "test_knowledge_mapping",
                "使用已配置模型为测试失败映射知识点",
            ))
            .build(),
    );
}
