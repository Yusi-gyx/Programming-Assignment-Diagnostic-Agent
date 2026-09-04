//! 使用配置模型为黑盒测试失败映射 Rust 知识点。

use crate::agent::llm::{ChatMessage, ChatModel, LlmClient, LlmResponse};
use crate::analysis::classifier::classify_test_failure;
use crate::config::model::ModelConfig;
use crate::error::{PadaError, Result};
use crate::history::{AgentDecision, LlmExchange, Session, StepBuilder};
use crate::models::{Assignment, Diagnostic, ErrorCategory, KnowledgePoint, TestResult};
use crate::telemetry::{UsageRecord, UsageTracker};
use serde::Deserialize;

pub struct TestKnowledgeMapper {
    config: Option<ModelConfig>,
    model: Option<Box<dyn ChatModel>>,
}

impl TestKnowledgeMapper {
    pub fn new(config: Option<ModelConfig>) -> Self {
        let model = config
            .as_ref()
            .map(|config| Box::new(LlmClient::new(config.clone())) as Box<dyn ChatModel>);
        Self { config, model }
    }

    pub fn with_model(config: ModelConfig, model: Box<dyn ChatModel>) -> Self {
        Self {
            config: Some(config),
            model: Some(model),
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
        let fallback = || {
            failures
                .iter()
                .map(|result| {
                    classify_test_failure(
                        &result.name,
                        &result.actual_output,
                        &result.expected_output,
                    )
                })
                .collect::<Vec<_>>()
        };
        if failures.is_empty() {
            return Ok(Vec::new());
        }
        let (Some(config), Some(model)) = (self.config.as_ref(), self.model.as_ref()) else {
            return Ok(fallback());
        };
        if !tracker.check_budget() {
            return Ok(fallback());
        }

        let messages = mapping_messages(assignment, source_context, failures);
        let response = model.chat(&messages)?;
        let mapped = parse_mapping_response(&response.content, failures);
        record_exchange(session, tracker, config, messages, response);
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
                "[{index}] {}\n期望: {:?}\n实际: {:?}",
                result.name,
                result.expected_output.trim(),
                result.actual_output.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    vec![
        ChatMessage::system(
            "你负责把 Rust 程序的黑盒测试失败映射到知识点。只输出 JSON，不要给修改方案。knowledge_points 只能使用：Ownership, Borrowing, Lifetime, Trait, Generic, Iterator, Option, Result, PatternMatching, Collection, ErrorHandling, AlgorithmLogic。每个失败最多选择 3 个最相关知识点；证据不足时使用 AlgorithmLogic。格式：{\"mappings\":[{\"index\":0,\"knowledge_points\":[\"Iterator\"]}]}。",
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
    let json = extract_json_payload(content)
        .ok_or_else(|| PadaError::Parse("模型知识点映射未返回 JSON 对象或数组".into()))?;
    let payload: MappingPayload = serde_json::from_str(json).map_err(|error| {
        let preview = content.trim().chars().take(300).collect::<String>();
        PadaError::Parse(format!(
            "解析模型知识点映射失败: {error}；模型输出: {preview}"
        ))
    })?;
    let mappings = match payload {
        MappingPayload::Envelope(envelope) => envelope.mappings,
        MappingPayload::Items(items) => items,
    };
    let mut diagnostics = failures
        .iter()
        .map(|_| Diagnostic {
            category: ErrorCategory::LogicError,
            knowledge_points: Vec::new(),
            confidence: 0.3,
        })
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
            diagnostic.knowledge_points = points;
            diagnostic.confidence = 0.7;
        }
    }
    Ok(diagnostics)
}

fn parse_knowledge_point(label: &str) -> Option<KnowledgePoint> {
    match label.trim() {
        "所有权" => return Some(KnowledgePoint::Ownership),
        "借用" | "可变借用" => return Some(KnowledgePoint::Borrowing),
        "生命周期" => return Some(KnowledgePoint::Lifetime),
        "泛型" => return Some(KnowledgePoint::Generic),
        "迭代器" => return Some(KnowledgePoint::Iterator),
        "模式匹配" => return Some(KnowledgePoint::PatternMatching),
        "集合" => return Some(KnowledgePoint::Collection),
        "错误处理" => return Some(KnowledgePoint::ErrorHandling),
        "算法逻辑" => return Some(KnowledgePoint::AlgorithmLogic),
        _ => {}
    }
    match label.trim().to_ascii_lowercase().as_str() {
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

fn extract_json_payload(content: &str) -> Option<&str> {
    let object_start = content.find('{').map(|index| (index, '}'));
    let array_start = content.find('[').map(|index| (index, ']'));
    let (start, closing) = match (object_start, array_start) {
        (Some(object), Some(array)) => {
            if object.0 <= array.0 {
                object
            } else {
                array
            }
        }
        (Some(object), None) => object,
        (None, Some(array)) => array,
        (None, None) => return None,
    };
    let end = content.rfind(closing)?;
    (end >= start).then_some(&content[start..=end])
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
