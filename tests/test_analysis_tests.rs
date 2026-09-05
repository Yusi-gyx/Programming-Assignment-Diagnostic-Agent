use pada::agent::llm::{ChatMessage, ChatModel, LlmResponse, ModelTaskKind};
use pada::agent::test_analysis::{TestKnowledgeMapper, mapping_messages, parse_mapping_response};
use pada::config::model::ModelConfig;
use pada::history::Session;
use pada::models::{Assignment, KnowledgePoint, TestResult};
use pada::telemetry::UsageTracker;

fn failure(name: &str) -> TestResult {
    TestResult {
        name: name.into(),
        passed: false,
        actual_output: "1 2 3".into(),
        expected_output: "3 2 1".into(),
        runtime_error: None,
    }
}

#[test]
fn parses_model_mappings_into_domain_knowledge_points() {
    let failures = vec![failure("normal"), failure("empty")];
    let response = r#"说明
```json
{"mappings":[
  {"index":0,"knowledge_points":["Iterator","AlgorithmLogic","unknown"]},
  {"index":1,"knowledge_points":["Option"]}
]}
```"#;
    let mapped = parse_mapping_response(response, &failures).unwrap();
    assert_eq!(
        mapped[0].knowledge_points,
        vec![KnowledgePoint::Iterator, KnowledgePoint::AlgorithmLogic]
    );
    assert_eq!(mapped[1].knowledge_points, vec![KnowledgePoint::Option]);
    assert!(mapped[0].confidence > 0.3);
}

#[test]
fn parses_top_level_array_returned_by_local_models() {
    let failures = vec![failure("normal")];
    let response = r#"[{"index":0,"knowledge_points":["Iterator","AlgorithmLogic"]}]"#;
    let mapped = parse_mapping_response(response, &failures).unwrap();
    assert_eq!(
        mapped[0].knowledge_points,
        vec![KnowledgePoint::Iterator, KnowledgePoint::AlgorithmLogic]
    );
}

#[test]
fn ignores_thinking_json_before_the_real_mapping() {
    let failures = vec![failure("normal")];
    let response = r#"<think>{"draft":true}</think>
说明文字 [不是结果]
```json
{"mappings":[{"index":0,"knowledge_points":["TypeSystem"]}]}
```"#;
    let mapped = parse_mapping_response(response, &failures).unwrap();
    assert_eq!(mapped[0].knowledge_points, vec![KnowledgePoint::TypeSystem]);
}

struct MappingModel;

impl ChatModel for MappingModel {
    fn chat_for_task(
        &self,
        messages: &[ChatMessage],
        task: ModelTaskKind,
        cancelled: &std::sync::atomic::AtomicBool,
        on_chunk: &mut (dyn FnMut(&str) + Send),
    ) -> pada::error::Result<LlmResponse> {
        assert_eq!(task, ModelTaskKind::KnowledgeMapping);
        self.chat_cancellable_streaming(messages, cancelled, on_chunk)
    }

    fn chat(&self, messages: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
        assert!(messages[0].content.contains("只输出 JSON"));
        Ok(LlmResponse {
            details: Default::default(),
            timings: Default::default(),
            content: r#"{"mappings":[{"index":0,"knowledge_points":["Iterator"]}]}"#.into(),
            input_tokens: 30,
            output_tokens: 10,
            model: "mapping-model".into(),
        })
    }
}

struct InvalidMappingModel;

impl ChatModel for InvalidMappingModel {
    fn chat(&self, _messages: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
        Ok(LlmResponse {
            details: Default::default(),
            timings: Default::default(),
            content: r#"{"unexpected":true}"#.into(),
            input_tokens: 12,
            output_tokens: 3,
            model: "mapping-model".into(),
        })
    }
}

#[test]
fn configured_mapper_calls_model_and_records_trajectory() {
    let config = ModelConfig::local("mapping-model", 8192);
    let mapper = TestKnowledgeMapper::with_model(config, Box::new(MappingModel));
    let assignment = Assignment {
        title: "逆序".into(),
        description: "逆序输出整数".into(),
    };
    let failures = vec![failure("normal")];
    let mut tracker = UsageTracker::new();
    let mut session = Session::new("test");
    let mapped = mapper
        .map_failures(
            &assignment,
            "fn main() {}",
            &failures,
            &mut tracker,
            &mut session,
        )
        .unwrap();

    assert_eq!(mapped[0].knowledge_points, vec![KnowledgePoint::Iterator]);
    assert_eq!(tracker.session().total_tokens(), 40);
    assert_eq!(session.usage_records.len(), 1);
    assert_eq!(
        session.steps[0].decisions[0].stage,
        "test_knowledge_mapping"
    );
}

#[test]
fn records_usage_and_response_even_when_mapping_shape_is_invalid() {
    let config = ModelConfig::local("mapping-model", 8192);
    let mapper = TestKnowledgeMapper::with_model(config, Box::new(InvalidMappingModel));
    let assignment = Assignment {
        title: "逆序".into(),
        description: "逆序输出整数".into(),
    };
    let mut tracker = UsageTracker::new();
    let mut session = Session::new("test");

    let error = mapper
        .map_failures(
            &assignment,
            "fn main() {}",
            &[failure("normal")],
            &mut tracker,
            &mut session,
        )
        .unwrap_err();

    assert!(error.to_string().contains("模型输出"));
    assert_eq!(tracker.session().total_tokens(), 15);
    assert_eq!(session.usage_records.len(), 1);
    assert!(session.steps[0].llm_exchange.is_some());
}

#[test]
fn mapping_prompt_contains_problem_source_and_failed_cases() {
    let assignment = Assignment {
        title: "题目".into(),
        description: "描述".into(),
    };
    let messages = mapping_messages(&assignment, "source code", &[failure("case_1")]);
    assert!(messages[1].content.contains("source code"));
    assert!(messages[1].content.contains("case_1"));
    assert!(messages[1].content.contains("3 2 1"));
}
