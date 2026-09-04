use pada::agent::llm::{ChatMessage, LlmResponse};
use pada::agent::solution::{SolutionHintService, SolutionModel};
use pada::analysis::error_parser::{RustcDiagnostic, Severity};
use pada::analysis::hint::generate_compile_hint;
use pada::config::model::ModelConfig;
use pada::history::Session;
use pada::memory::KnowledgeProfile;
use pada::models::{Assignment, Diagnostic, ErrorCategory, HintLevel, KnowledgePoint};
use pada::report::{CompileReportEntry, DiagnosticReport};
use pada::telemetry::UsageTracker;

struct MockModel;

impl SolutionModel for MockModel {
    fn chat(&self, messages: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
        assert!(
            messages
                .iter()
                .any(|message| message.content.contains("E0382"))
        );
        Ok(LlmResponse {
            content: "模型生成的参考方案".into(),
            input_tokens: 12,
            output_tokens: 8,
            model: "mock".into(),
        })
    }
}

#[test]
fn configured_level_five_uses_model_response_and_records_usage() {
    let assignment = Assignment {
        title: "所有权练习".into(),
        description: "修复移动后使用".into(),
    };
    let diag = RustcDiagnostic {
        severity: Severity::Error,
        code: Some("E0382".into()),
        message: "borrow of moved value".into(),
        location: None,
        notes: vec![],
    };
    let classified = Diagnostic {
        category: ErrorCategory::CompileError,
        knowledge_points: vec![KnowledgePoint::Ownership],
        confidence: 0.95,
    };
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Solution);
    let mut report = DiagnosticReport::new();
    report.add_compile(CompileReportEntry {
        diag,
        classified,
        hint,
    });
    let config = ModelConfig::cloud("https://unused", "key", "mock", 8192, 1.0, 2.0);
    let mut service = SolutionHintService::with_model(config, Box::new(MockModel));
    let mut tracker = UsageTracker::new();
    let mut session = Session::new("test");

    service.enrich(
        &mut report,
        &assignment,
        "fn main() {}",
        &KnowledgeProfile::default(),
        &mut tracker,
        &mut session,
    );

    assert_eq!(report.compile_entries[0].hint.content, "模型生成的参考方案");
    assert_eq!(tracker.session().total_tokens(), 20);
    assert_eq!(session.usage_records.len(), 1);
    assert!(session.steps[0].llm_exchange.is_some());
}
