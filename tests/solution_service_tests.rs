use pada::agent::llm::{ChatMessage, ChatModel, LlmResponse};
use pada::agent::solution::{SolutionHintService, format_model_output};
use pada::analysis::error_parser::{RustcDiagnostic, Severity};
use pada::analysis::hint::generate_compile_hint;
use pada::config::model::ModelConfig;
use pada::history::Session;
use pada::memory::KnowledgeProfile;
use pada::models::{Assignment, Diagnostic, ErrorCategory, HintLevel, KnowledgePoint};
use pada::report::{CompileReportEntry, DiagnosticReport};
use pada::telemetry::UsageTracker;

struct MockModel;

impl ChatModel for MockModel {
    fn chat(&self, messages: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
        assert!(
            messages
                .iter()
                .any(|message| message.content.contains("E0382"))
        );
        Ok(LlmResponse {
            timings: Default::default(),
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
        false,
    );

    assert_eq!(report.compile_entries[0].hint.content, "模型生成的参考方案");
    assert_eq!(tracker.session().total_tokens(), 20);
    assert_eq!(session.usage_records.len(), 1);
    assert!(session.steps[0].llm_exchange.is_some());
}

struct ConceptModel;

impl ChatModel for ConceptModel {
    fn chat(&self, messages: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
        assert!(messages[0].content.contains("Level 3"));
        assert!(messages[0].content.contains("不给本题答案"));
        Ok(LlmResponse {
        timings: Default::default(),
            content: "<think>内部推理</think>\n```markdown\n### 知识点说明\n所有权决定值由谁释放。\n\n### 通用示例\n```rust\nlet name = String::from(\"Ada\");\nlet copy = name.clone();\n```\n### 自检问题\n哪个变量拥有数据？\n```"
                .into(),
            input_tokens: 10,
            output_tokens: 10,
            model: "mock".into(),
        })
    }
}

#[test]
fn configured_level_three_adds_structured_model_explanation() {
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
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Concept);
    let mut report = DiagnosticReport::new();
    report.add_compile(CompileReportEntry {
        diag,
        classified,
        hint,
    });
    let config = ModelConfig::cloud("https://unused", "key", "mock", 8192, 1.0, 2.0);
    let mut service = SolutionHintService::with_model(config, Box::new(ConceptModel));
    let mut tracker = UsageTracker::new();
    let mut session = Session::new("test");

    service.enrich(
        &mut report,
        &assignment,
        "fn main() {}",
        &KnowledgeProfile::default(),
        &mut tracker,
        &mut session,
        false,
    );

    let content = &report.compile_entries[0].hint.content;
    assert!(content.starts_with("### 知识点说明"));
    assert!(content.contains("```rust"));
    assert!(!content.contains("内部推理"));
    assert_eq!(session.steps[0].decisions[0].stage, "level_3_hint");

    service.enrich(
        &mut report,
        &assignment,
        "fn main() {}",
        &KnowledgeProfile::default(),
        &mut tracker,
        &mut session,
        false,
    );
    assert_eq!(report.compile_entries[0].hint.level, HintLevel::Concept);
    assert_eq!(
        tracker.session().total_tokens(),
        20,
        "缓存命中不应再次调用模型"
    );
}

#[test]
fn model_output_formatter_removes_outer_fence_and_think_block() {
    let rendered =
        format_model_output("<think>do not show</think>\r\n```markdown\r\n### 原因\r\n内容\r\n```");
    assert_eq!(rendered, "### 原因\n内容");
}
