//! CLI 多轮交互所需的确定性状态与命令解析。

use crate::models::HintLevel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractiveCommand {
    Next,
    Hint(Option<u8>),
    Recheck,
    Show,
    Usage,
    Progress,
    Feedback(bool),
    Save(Option<String>),
    Help,
    Exit,
    Unknown(String),
}

pub fn parse_command(input: &str) -> InteractiveCommand {
    let mut parts = input.split_whitespace();
    match parts.next().unwrap_or("") {
        "\\next" | "next" => InteractiveCommand::Next,
        "\\hint" | "hint" => InteractiveCommand::Hint(parts.next().and_then(|v| v.parse().ok())),
        "\\recheck" | "recheck" => InteractiveCommand::Recheck,
        "\\show" | "show" => InteractiveCommand::Show,
        "\\usage" | "usage" => InteractiveCommand::Usage,
        "\\process" | "\\progress" | "process" | "progress" => InteractiveCommand::Progress,
        "understood" | "懂了" => InteractiveCommand::Feedback(true),
        "notyet" | "还不会" => InteractiveCommand::Feedback(false),
        "\\save" | "save" => InteractiveCommand::Save(parts.next().map(str::to_owned)),
        "\\help" | "help" | "?" => InteractiveCommand::Help,
        "\\exit" | "exit" | "quit" => InteractiveCommand::Exit,
        other => InteractiveCommand::Unknown(other.to_owned()),
    }
}

pub fn hint_number(level: HintLevel) -> u8 {
    match level {
        HintLevel::Category => 1,
        HintLevel::Location => 2,
        HintLevel::Concept => 3,
        HintLevel::Direction => 4,
        HintLevel::Solution => 5,
    }
}

pub fn help_text() -> &'static str {
    "命令: next  hint [1-5]  recheck  show  progress  understood/notyet  usage  save [文件]  help  exit"
}
