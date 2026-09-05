use pada::agent::llm::{ChatMessage, ChatModel, LlmResponse, ModelTaskKind, ResponseDetails};
use pada::agent::solution::SolutionHintService;
use pada::agent::test_analysis::TestKnowledgeMapper;
use pada::analysis::hint::generate_test_result_hint;
use pada::config::model::ModelConfig;
use pada::history::Session;
use pada::memory::KnowledgeProfile;
use pada::models::{Assignment, Diagnostic, ErrorCategory, HintLevel, KnowledgePoint, TestResult};
use pada::report::{DiagnosticReport, TestReportEntry};
use pada::telemetry::UsageTracker;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Model {
    calls: Arc<AtomicUsize>,
    truncated: bool,
}
impl ChatModel for Model {
    fn chat(&self, _: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
        panic!("task kind must be forwarded")
    }
    fn chat_for_task(
        &self,
        messages: &[ChatMessage],
        task: ModelTaskKind,
        _: &AtomicBool,
        _: &mut (dyn FnMut(&str) + Send),
    ) -> pada::error::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let content = match task {
            ModelTaskKind::HintBatch { count, .. } => {
                let request: serde_json::Value =
                    serde_json::from_str(&messages[1].content).unwrap();
                let items = request["failures"].as_array().unwrap();
                assert_eq!(count, items.len());
                assert!(count <= 8);
                serde_json::json!({"hints":items.iter().rev().map(|item| serde_json::json!({
                    "index":item["index"], "content":format!("提示 {}",item["result"]["name"].as_str().unwrap()),
                })).collect::<Vec<_>>()}).to_string()
            }
            ModelTaskKind::KnowledgeMapping => {
                r#"{"mappings":[{"index":0,"knowledge_points":["Iterator"]}]}"#.into()
            }
            ModelTaskKind::Hint(HintLevel::Concept) => "通用迭代器概念".into(),
            other => panic!("unexpected task {other:?}"),
        };
        Ok(LlmResponse {
            content,
            input_tokens: 10,
            output_tokens: 5,
            model: "mock".into(),
            timings: Default::default(),
            details: ResponseDetails {
                finish_reason: Some(if self.truncated { "length" } else { "stop" }.into()),
                reasoning_tokens: Some(2),
            },
        })
    }
}

fn assignment() -> Assignment {
    Assignment {
        title: "迭代器".into(),
        description: "逆序输出".into(),
    }
}
fn failure(i: usize) -> TestResult {
    TestResult {
        name: format!("case_{i}"),
        passed: false,
        expected_output: "2 1".into(),
        actual_output: "1 2".into(),
        runtime_error: None,
    }
}
fn report(level: HintLevel, count: usize) -> DiagnosticReport {
    let mut report = DiagnosticReport::new();
    for i in 0..count {
        let result = failure(i);
        let classified = Diagnostic {
            category: ErrorCategory::LogicError,
            knowledge_points: vec![KnowledgePoint::Iterator],
            confidence: 0.8,
        };
        let hint = generate_test_result_hint(&result, &classified, level);
        report.add_test(TestReportEntry {
            result,
            classified,
            hint,
        });
    }
    report
}

#[test]
fn concepts_share_one_call_and_keep_every_case_evidence() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut service = SolutionHintService::with_model(
        ModelConfig::local("mock", 8192),
        Box::new(Model {
            calls: calls.clone(),
            truncated: false,
        }),
    );
    let mut report = report(HintLevel::Concept, 6);
    let mut tracker = UsageTracker::new();
    let mut session = Session::new("test");
    service.enrich(
        &mut report,
        &assignment(),
        "source",
        &KnowledgeProfile::default(),
        &mut tracker,
        &mut session,
        false,
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(tracker.session().total_tokens(), 15);
    for (i, entry) in report.test_entries.iter().enumerate() {
        assert_eq!(entry.result.name, format!("case_{i}"));
        assert_eq!(entry.hint.content, "通用迭代器概念");
    }
}

#[test]
fn ten_case_hints_take_two_calls_cache_and_invalidate_on_source_change() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut service = SolutionHintService::with_model(
        ModelConfig::local("mock", 8192),
        Box::new(Model {
            calls: calls.clone(),
            truncated: false,
        }),
    );
    let mut tracker = UsageTracker::new();
    let mut session = Session::new("test");
    for (source, expected_calls) in [("source", 2), ("source", 2), ("changed source", 4)] {
        let mut report = report(HintLevel::Direction, 10);
        service.enrich(
            &mut report,
            &assignment(),
            source,
            &KnowledgeProfile::default(),
            &mut tracker,
            &mut session,
            false,
        );
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        for entry in report.test_entries {
            assert_eq!(entry.hint.content, format!("提示 {}", entry.result.name));
        }
    }
    assert_eq!(session.usage_records.len(), 4);
}

#[test]
fn truncated_batch_is_billed_but_never_replaces_hints_or_enters_cache() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut service = SolutionHintService::with_model(
        ModelConfig::local("mock", 8192),
        Box::new(Model {
            calls: calls.clone(),
            truncated: true,
        }),
    );
    let mut tracker = UsageTracker::new();
    let mut session = Session::new("test");
    for _ in 0..2 {
        let mut report = report(HintLevel::Direction, 2);
        let base = report.test_entries[0].hint.content.clone();
        service.enrich(
            &mut report,
            &assignment(),
            "source",
            &KnowledgeProfile::default(),
            &mut tracker,
            &mut session,
            false,
        );
        assert_eq!(report.test_entries[0].hint.content, base);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(tracker.session().total_tokens(), 30);
}

#[test]
fn unchanged_mapping_is_cached_but_problem_code_and_results_invalidate_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mapper = TestKnowledgeMapper::with_model(
        ModelConfig::local("mock", 8192),
        Box::new(Model {
            calls: calls.clone(),
            truncated: false,
        }),
    );
    let mut tracker = UsageTracker::new();
    let mut session = Session::new("test");
    let mut problem = assignment();
    for (source, expected) in [("source", 1), ("source", 1), ("new source", 2)] {
        mapper
            .map_failures(&problem, source, &[failure(0)], &mut tracker, &mut session)
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), expected);
    }
    problem.description = "新题意".into();
    mapper
        .map_failures(
            &problem,
            "new source",
            &[failure(0)],
            &mut tracker,
            &mut session,
        )
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let mut changed = failure(0);
    changed.actual_output = "panic".into();
    mapper
        .map_failures(
            &problem,
            "new source",
            &[changed],
            &mut tracker,
            &mut session,
        )
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(tracker.session().total_tokens(), 60);
}
