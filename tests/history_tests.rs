//! 会话历史与轨迹持久化测试（R5 / 第 14 步）
//!
//! 验证会话记录、保存、加载与回放逻辑。
//!
//! ```bash
//! cargo test --test history_tests
//! ```

use pada::agent::llm::{ChatMessage, LlmResponse};
use pada::config::effort::EffortMode;
use pada::config::model::ModelConfig;
use pada::history::{AgentDecision, LlmExchange, Session, StepBuilder, ToolCall};
use pada::telemetry::UsageRecord;

// ============================================================
// 辅助构造函数
// ============================================================

fn make_config() -> ModelConfig {
    ModelConfig::local("test-model", 8192)
}

fn make_response() -> LlmResponse {
    LlmResponse {
        details: Default::default(),
        timings: Default::default(),
        content: "这是一个所有权错误".into(),
        input_tokens: 100,
        output_tokens: 50,
        model: "test-model".into(),
    }
}

fn make_usage() -> UsageRecord {
    UsageRecord::from_response(&make_response(), &make_config())
}

// ============================================================
// ToolCall 测试
// ============================================================

#[test]
fn test_tool_call_creation() {
    let call = ToolCall::new("compile_file", "main.rs", "编译失败");
    assert_eq!(call.tool, "compile_file");
    assert_eq!(call.params, "main.rs");
    assert_eq!(call.output, "编译失败");
    assert!(call.timestamp > 0);
}

#[test]
fn test_tool_call_serialization() {
    let call = ToolCall::new("run_tests", "5 cases", "3 passed, 2 failed");
    let json = serde_json::to_string(&call).unwrap();
    let de: ToolCall = serde_json::from_str(&json).unwrap();
    assert_eq!(call.tool, de.tool);
    assert_eq!(call.params, de.params);
    assert_eq!(call.output, de.output);
}

// ============================================================
// AgentDecision 测试
// ============================================================

#[test]
fn test_agent_decision() {
    let decision = AgentDecision::new("compiling", "编译失败，跳过测试阶段");
    assert_eq!(decision.stage, "compiling");
    assert_eq!(decision.reasoning, "编译失败，跳过测试阶段");
    assert!(decision.timestamp > 0);
}

#[test]
fn test_agent_decision_serialization() {
    let decision = AgentDecision::new("parsing", "发现 E0382，映射到 Ownership");
    let json = serde_json::to_string(&decision).unwrap();
    let de: AgentDecision = serde_json::from_str(&json).unwrap();
    assert_eq!(decision.stage, de.stage);
    assert_eq!(decision.reasoning, de.reasoning);
}

// ============================================================
// StepBuilder 测试
// ============================================================

#[test]
fn test_step_builder_empty() {
    let step = StepBuilder::new(0).build();
    assert_eq!(step.index, 0);
    assert!(step.user_input.is_none());
    assert!(step.tool_calls.is_empty());
    assert!(step.decisions.is_empty());
    assert!(step.llm_exchange.is_none());
}

#[test]
fn test_step_builder_full() {
    let step = StepBuilder::new(1)
        .user_input("帮我分析这段代码")
        .tool_call(ToolCall::new("compile", "main.rs", "E0382"))
        .decision(AgentDecision::new("compile", "编译失败"))
        .build();

    assert_eq!(step.index, 1);
    assert_eq!(step.user_input.as_deref(), Some("帮我分析这段代码"));
    assert_eq!(step.tool_calls.len(), 1);
    assert_eq!(step.tool_calls[0].tool, "compile");
    assert_eq!(step.decisions.len(), 1);
    assert!(step.llm_exchange.is_none());
}

#[test]
fn test_step_builder_with_llm() {
    let exchange = LlmExchange {
        messages: vec![ChatMessage::user("test")],
        response: make_response(),
        usage: Some(make_usage()),
    };

    let step = StepBuilder::new(2).llm_exchange(exchange).build();

    assert!(step.llm_exchange.is_some());
    let ex = step.llm_exchange.unwrap();
    assert_eq!(ex.response.content, "这是一个所有权错误");
    assert_eq!(ex.response.input_tokens, 100);
    assert!(ex.usage.is_some());
}

// ============================================================
// Session 测试
// ============================================================

#[test]
fn test_session_creation() {
    let session = Session::new("整数求和");
    assert!(!session.id.is_empty());
    assert_eq!(session.title, "整数求和");
    assert_eq!(session.step_count(), 0);
    assert!(session.usage_records.is_empty());
}

#[test]
fn test_session_add_steps() {
    let mut session = Session::new("test");

    let step1 = StepBuilder::new(0).user_input("代码1").build();
    session.add_step(step1);

    let step2 = StepBuilder::new(1).user_input("代码2").build();
    session.add_step(step2);

    assert_eq!(session.step_count(), 2);
    assert!(session.updated_at >= session.created_at);
}

#[test]
fn test_session_record_usage() {
    let mut session = Session::new("test");
    session.record_usage(make_usage());
    session.record_usage(make_usage());
    assert_eq!(session.usage_records.len(), 2);
}

#[test]
fn test_session_summary() {
    let mut session = Session::new("所有权错误诊断");
    session.add_step(StepBuilder::new(0).build());
    session.record_usage(make_usage());

    let summary = session.summary();
    assert!(summary.contains("所有权错误诊断"));
    assert!(summary.contains("1 步"));
    assert!(summary.contains("1 条用量记录"));
}

// ============================================================
// 持久化测试
// ============================================================

#[test]
fn test_session_save_load_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.json");

    let mut session = Session::new("所有权诊断");
    session.add_step(
        StepBuilder::new(0)
            .user_input(
                "fn main() { let s = String::from(\"hi\"); let t = s; println!(\"{}\", s); }",
            )
            .tool_call(ToolCall::new("compile", "main.rs", "E0382"))
            .decision(AgentDecision::new("compile", "编译失败，分析错误"))
            .build(),
    );
    session.add_step(
        StepBuilder::new(1)
            .llm_exchange(LlmExchange {
                messages: vec![
                    ChatMessage::system("你是 Rust 导师"),
                    ChatMessage::user("分析 E0382"),
                ],
                response: make_response(),
                usage: Some(make_usage()),
            })
            .build(),
    );
    session.record_usage(make_usage());

    session.save(&path).expect("保存应成功");
    let loaded = Session::load(&path).expect("加载应成功");

    assert_eq!(session.title, loaded.title);
    assert_eq!(loaded.step_count(), 2);
    assert_eq!(loaded.usage_records.len(), 1);

    // 验证步骤内容
    assert!(loaded.steps[0].user_input.is_some());
    assert_eq!(loaded.steps[0].tool_calls.len(), 1);
    assert_eq!(loaded.steps[0].tool_calls[0].tool, "compile");

    // 验证 LLM 交互
    assert!(loaded.steps[1].llm_exchange.is_some());
    let ex = loaded.steps[1].llm_exchange.as_ref().unwrap();
    assert_eq!(ex.messages.len(), 2);
    assert_eq!(ex.response.content, "这是一个所有权错误");
}

#[test]
fn test_old_session_without_resume_context_still_loads() {
    let session = Session::new("旧会话");
    let mut value = serde_json::to_value(&session).unwrap();
    value.as_object_mut().unwrap().remove("context");
    let loaded: Session = serde_json::from_value(value).unwrap();
    assert!(loaded.context.is_none());
    assert_eq!(loaded.title, "旧会话");
}

#[test]
fn old_resume_context_defaults_to_medium_effort() {
    let context: pada::history::SessionContext = serde_json::from_value(serde_json::json!({
        "problem": "problem.md",
        "code": "main.rs",
        "project": null,
        "tests": null,
        "config": null,
        "profile": null,
        "memory": null,
        "hint": 1,
        "budget": null,
        "generate_tests": false
    }))
    .unwrap();
    assert_eq!(context.effort, EffortMode::Medium);
}

#[test]
fn test_session_load_nonexistent() {
    let result = Session::load(std::path::Path::new("/nonexistent/session.json"));
    assert!(result.is_err());
}

#[test]
fn test_session_save_invalid_path() {
    let session = Session::new("test");
    let result = session.save(std::path::Path::new("/nonexistent/dir/session.json"));
    assert!(result.is_err(), "写入不存在的目录应失败");
}

// ============================================================
// 完整工作流轨迹测试
// ============================================================

#[test]
fn test_full_workflow_trajectory() {
    // 模拟一次完整的诊断会话轨迹
    let mut session = Session::new("E0382 诊断");

    // 步骤 1：读取输入
    session.add_step(
        StepBuilder::new(0)
            .user_input("题目：所有权测试\n代码：let s = String::from(\"hi\"); let t = s; println!(\"{}\", s);")
            .decision(AgentDecision::new("reading_input", "成功读取题目与代码"))
            .build(),
    );

    // 步骤 2：编译
    session.add_step(
        StepBuilder::new(1)
            .tool_call(ToolCall::new("compile_file", "main.rs", "编译失败: E0382"))
            .decision(AgentDecision::new("compiling", "编译失败，进入错误分析"))
            .build(),
    );

    // 步骤 3：解析与分类
    session.add_step(
        StepBuilder::new(2)
            .tool_call(ToolCall::new("parse_diagnostics", "stderr", "1 条 E0382"))
            .tool_call(ToolCall::new("classify", "E0382", "Ownership, 置信度 0.95"))
            .decision(AgentDecision::new("parsing", "映射到知识点 Ownership"))
            .build(),
    );

    // 步骤 4：LLM 生成提示
    session.add_step(
        StepBuilder::new(3)
            .llm_exchange(LlmExchange {
                messages: vec![
                    ChatMessage::system("你是 Rust 导师"),
                    ChatMessage::user("代码有 E0382 错误，请指导"),
                ],
                response: make_response(),
                usage: Some(make_usage()),
            })
            .decision(AgentDecision::new("llm_calling", "LLM 生成了诊断提示"))
            .build(),
    );
    session.record_usage(make_usage());

    // 验证轨迹完整性
    assert_eq!(session.step_count(), 4);
    assert_eq!(session.usage_records.len(), 1);

    // 验证每步都有时间戳
    for step in &session.steps {
        assert!(step.timestamp > 0, "每步应有时间戳");
    }

    // 验证工具调用总数
    let total_tool_calls: usize = session.steps.iter().map(|s| s.tool_calls.len()).sum();
    assert_eq!(total_tool_calls, 3, "应有 3 次工具调用");

    // 验证决策总数
    let total_decisions: usize = session.steps.iter().map(|s| s.decisions.len()).sum();
    assert_eq!(total_decisions, 4, "应有 4 次决策记录");

    // 保存并重新加载，验证轨迹不丢失
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("full_session.json");
    session.save(&path).unwrap();
    let loaded = Session::load(&path).unwrap();

    assert_eq!(loaded.step_count(), 4);
    let loaded_tool_calls: usize = loaded.steps.iter().map(|s| s.tool_calls.len()).sum();
    assert_eq!(loaded_tool_calls, 3, "加载后工具调用数应一致");
}
