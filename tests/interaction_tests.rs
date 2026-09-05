use pada::agent::interaction::{InteractiveCommand, hint_number, parse_command};
use pada::models::HintLevel;

#[test]
fn parses_friendly_and_backslash_commands() {
    assert_eq!(parse_command("next"), InteractiveCommand::Next);
    assert_eq!(parse_command("\\next"), InteractiveCommand::Next);
    assert_eq!(parse_command("hint 4"), InteractiveCommand::Hint(Some(4)));
    assert_eq!(
        parse_command("save session.json"),
        InteractiveCommand::Save(Some("session.json".into()))
    );
    assert_eq!(parse_command("progress"), InteractiveCommand::Progress);
    assert_eq!(parse_command("config"), InteractiveCommand::Config);
    assert_eq!(
        parse_command("effort xhigh"),
        InteractiveCommand::Effort(Some("xhigh".into()))
    );
    assert_eq!(parse_command("effort"), InteractiveCommand::Effort(None));
    assert_eq!(
        parse_command("test cases.json"),
        InteractiveCommand::Tests(Some("cases.json".into()))
    );
    assert_eq!(
        parse_command("tests cases.json"),
        InteractiveCommand::Tests(Some("cases.json".into()))
    );
    assert_eq!(parse_command("test"), InteractiveCommand::Tests(None));
    assert_eq!(parse_command("case"), InteractiveCommand::Case);
    assert_eq!(parse_command("懂了"), InteractiveCommand::Feedback(true));
}

#[test]
fn hint_levels_have_stable_numbers() {
    assert_eq!(hint_number(HintLevel::Category), 1);
    assert_eq!(hint_number(HintLevel::Solution), 5);
}
