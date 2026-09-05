use pada::agent::model_task::is_cancel_command;

#[test]
fn recognizes_model_cancel_commands() {
    assert!(is_cancel_command("q\n"));
    assert!(is_cancel_command("CANCEL"));
    assert!(is_cancel_command(" 取消 "));
    assert!(!is_cancel_command("exit"));
    assert!(!is_cancel_command("show"));
}

#[test]
fn failed_call_records_elapsed_time_and_inputs_without_fake_usage() {
    use pada::agent::llm::{ChatMessage, ChatModel, LlmResponse, ModelTaskKind};
    use pada::agent::model_task::{FailedModelCall, ModelTaskOutcome, run_recorded_model_task};
    struct Fails;
    impl ChatModel for Fails {
        fn chat(&self, _: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
            std::thread::sleep(std::time::Duration::from_millis(20));
            Err(pada::error::PadaError::Llm("simulated timeout".into()))
        }
    }
    let mut session = pada::history::Session::new("failure");
    let outcome = run_recorded_model_task(
        std::sync::Arc::new(Fails),
        &[ChatMessage::user("request")],
        false,
        ModelTaskKind::KnowledgeMapping,
        &mut session,
        |_| {},
    );
    assert!(matches!(outcome, ModelTaskOutcome::Completed(Err(_))));
    let call = &session.steps[0].tool_calls[0];
    let failure: FailedModelCall = serde_json::from_str(&call.output).unwrap();
    assert!(failure.total_ms >= 20);
    assert!(!failure.cancelled);
    assert!(call.params.contains("request"));
    assert!(session.usage_records.is_empty());
    assert!(session.steps[0].llm_exchange.is_none());
}
