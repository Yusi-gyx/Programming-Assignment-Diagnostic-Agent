use pada::agent::context::{limit_source, relevant_source_with_scope};
use pada::agent::llm::{ChatMessage, ChatModel, LlmResponse};
use pada::agent::solution::SolutionHintService;
use pada::analysis::error_parser::{RustcDiagnostic, Severity, SourceLocation};
use pada::analysis::hint::generate_compile_hint;
use pada::config::effort::{EffortMode, EffortPolicy, EffortSignals, ModelCallBudget};
use pada::config::model::ModelConfig;
use pada::history::Session;
use pada::memory::KnowledgeProfile;
use pada::models::{Assignment, Diagnostic, ErrorCategory, HintLevel, KnowledgePoint};
use pada::report::{CompileReportEntry, DiagnosticReport};
use pada::telemetry::UsageTracker;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn modes_have_monotonic_depth_and_medium_is_default() {
    assert_eq!(EffortMode::default(), EffortMode::Medium);
    let policies = [
        EffortMode::Low,
        EffortMode::Medium,
        EffortMode::High,
        EffortMode::Xhigh,
        EffortMode::Max,
    ]
    .map(EffortPolicy::for_mode);
    for pair in policies.windows(2) {
        assert!(pair[0].max_model_calls < pair[1].max_model_calls);
        assert!(pair[0].source.max_bytes < pair[1].source.max_bytes);
        assert!(pair[0].source.context_lines < pair[1].source.context_lines);
        assert!(pair[0].verification_passes <= pair[1].verification_passes);
    }
    assert!(!policies[0].run_tests);
    assert!(policies[1..].iter().all(|policy| policy.run_tests));
    assert_eq!(policies[0].reasoning_effort, "low");
    assert_eq!(policies[4].reasoning_effort, "xhigh");
    for mode in EffortMode::ALL {
        assert_eq!(mode.to_string().parse::<EffortMode>().unwrap(), mode);
    }
    assert!("extreme".parse::<EffortMode>().is_err());
}

#[test]
fn auto_uses_observed_complexity() {
    let simple = EffortMode::Auto.resolve(EffortSignals::default());
    assert_eq!(simple.mode, EffortMode::Low);
    let failed_test = EffortMode::Auto.resolve(EffortSignals {
        failed_tests: 1,
        ..Default::default()
    });
    assert_eq!(failed_test.mode, EffortMode::Medium);
    let complex = EffortMode::Auto.resolve(EffortSignals {
        error_count: 9,
        file_count: 60,
        failed_tests: 6,
        has_runtime_error: true,
        source_bytes: 400_000,
    });
    assert_eq!(complex.mode, EffortMode::Max);
}

#[test]
fn source_scope_limits_files_and_keeps_diagnosed_file() {
    let source = "// file: src/a.rs\nfn a() {}\n// file: src/b.rs\nline1\nline2\nline3\n";
    let low = EffortPolicy::for_mode(EffortMode::Low);
    let limited = limit_source(source, low.source);
    assert!(limited.contains("src/a.rs"));
    let focused = relevant_source_with_scope(
        source,
        Some(&SourceLocation {
            file: "src/b.rs".into(),
            line: 2,
            column: 1,
        }),
        low.source,
    );
    assert!(focused.contains("src/b.rs"));
    assert!(focused.contains("2: line2"));
}

#[test]
fn shared_model_budget_enforces_limit() {
    let mut budget = ModelCallBudget::new(EffortPolicy::for_mode(EffortMode::Low));
    assert!(budget.try_take());
    assert!(!budget.try_take());
    assert_eq!(budget.used(), 1);
    assert_eq!(budget.remaining(), 0);
}

struct CountingModel {
    calls: Arc<AtomicUsize>,
}

impl ChatModel for CountingModel {
    fn chat(&self, _messages: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(LlmResponse {
            details: Default::default(),
            timings: Default::default(),
            content: "诊断".into(),
            input_tokens: 1,
            output_tokens: 1,
            model: "counting".into(),
        })
    }
}

#[test]
fn low_service_calls_model_at_most_once_for_multiple_issues() {
    let policy = EffortPolicy::for_mode(EffortMode::Low);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut service = SolutionHintService::with_model_and_effort(
        ModelConfig::local("counting", 8192),
        Box::new(CountingModel {
            calls: Arc::clone(&calls),
        }),
        policy,
    );
    let mut report = DiagnosticReport::new();
    for (code, point) in [
        ("E0308", KnowledgePoint::TypeSystem),
        ("E0382", KnowledgePoint::Ownership),
    ] {
        let diagnostic = RustcDiagnostic {
            severity: Severity::Error,
            code: Some(code.into()),
            message: "error".into(),
            location: None,
            notes: vec![],
        };
        let classified = Diagnostic {
            category: ErrorCategory::CompileError,
            knowledge_points: vec![point],
            confidence: 0.9,
        };
        let hint = generate_compile_hint(&diagnostic, &classified, HintLevel::Concept);
        report.add_compile(CompileReportEntry {
            diag: diagnostic,
            classified,
            hint,
        });
    }
    service.enrich(
        &mut report,
        &Assignment {
            title: "test".into(),
            description: "test".into(),
        },
        "fn main() {}",
        &KnowledgeProfile::default(),
        &mut UsageTracker::new(),
        &mut Session::new("test"),
        false,
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        report.compile_entries[1]
            .hint
            .content
            .contains("最多允许 1 次")
    );
}
