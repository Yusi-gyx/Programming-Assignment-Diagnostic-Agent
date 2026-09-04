use pada::agent::model_task::is_cancel_command;

#[test]
fn recognizes_model_cancel_commands() {
    assert!(is_cancel_command("q\n"));
    assert!(is_cancel_command("CANCEL"));
    assert!(is_cancel_command(" 取消 "));
    assert!(!is_cancel_command("exit"));
    assert!(!is_cancel_command("show"));
}
