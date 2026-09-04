//! LLM 客户端测试（第 9 步）
//!
//! 全部离线测试（不发送真实 HTTP 请求），
//! 仅验证请求体构造与响应解析的纯函数逻辑。
//!
//! ```bash
//! cargo test --test llm_tests
//! ```

use pada::agent::llm::{
    ChatMessage, LlmClient, LlmResponse, compile_solution_messages, test_solution_messages,
};
use pada::analysis::error_parser::{RustcDiagnostic, Severity};
use pada::config::model::ModelConfig;
use pada::models::{Assignment, Diagnostic, ErrorCategory, KnowledgePoint, TestResult};
use serde_json::json;

// ============================================================
// ChatMessage 测试
// ============================================================

#[test]
fn test_chat_message_roles() {
    let sys = ChatMessage::system("你是一位 Rust 导师");
    assert_eq!(sys.role, "system");
    assert_eq!(sys.content, "你是一位 Rust 导师");

    let usr = ChatMessage::user("帮我分析这段代码");
    assert_eq!(usr.role, "user");

    let ast = ChatMessage::assistant("好的");
    assert_eq!(ast.role, "assistant");
}

#[test]
fn test_chat_message_serialization() {
    let msg = ChatMessage::user("hello");
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "user");
    assert_eq!(json["content"], "hello");
}

// ============================================================
// build_request_body 测试
// ============================================================

fn make_client(reasoning: bool) -> LlmClient {
    let mut cfg = ModelConfig::local("test-model", 8192);
    cfg.reasoning = reasoning;
    LlmClient::new(cfg)
}

#[test]
fn test_build_request_body_basic() {
    let client = make_client(false);
    let messages = vec![
        ChatMessage::system("你是一位 Rust 导师"),
        ChatMessage::user("这段代码错在哪？"),
    ];
    let body = client.build_request_body(&messages);

    assert_eq!(body["model"], "test-model");
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "你是一位 Rust 导师");
    assert_eq!(body["messages"][1]["role"], "user");
    assert!(body["temperature"].is_number());
    // 默认不包含 reasoning
    assert!(body.get("reasoning").is_none());
}

#[test]
fn test_build_request_body_with_reasoning() {
    let client = make_client(true);
    let messages = vec![ChatMessage::user("test")];
    let body = client.build_request_body(&messages);

    assert_eq!(body["reasoning"], true);
}

#[test]
fn test_build_request_body_empty_messages() {
    let client = make_client(false);
    let body = client.build_request_body(&[]);
    assert_eq!(body["model"], "test-model");
    assert!(body["messages"].as_array().unwrap().is_empty());
}

// ============================================================
// parse_response 测试
// ============================================================

#[test]
fn test_parse_response_success() {
    let json = json!({
        "model": "deepseek-chat",
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": "这是一个所有权错误。"
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_tokens": 150
        }
    });

    let resp = LlmClient::parse_response(&json).expect("解析应成功");
    assert_eq!(resp.content, "这是一个所有权错误。");
    assert_eq!(resp.input_tokens, 100);
    assert_eq!(resp.output_tokens, 50);
    assert_eq!(resp.model, "deepseek-chat");
}

#[test]
fn test_parse_response_missing_usage() {
    // 部分本地模型不返回 usage，token 数应记为 0
    let json = json!({
        "model": "local-model",
        "choices": [
            {"message": {"role": "assistant", "content": "reply"}}
        ]
    });

    let resp = LlmClient::parse_response(&json).expect("解析应成功");
    assert_eq!(resp.content, "reply");
    assert_eq!(resp.input_tokens, 0);
    assert_eq!(resp.output_tokens, 0);
    assert_eq!(resp.model, "local-model");
}

#[test]
fn test_parse_response_missing_choices() {
    let json = json!({"model": "x"});
    let result = LlmClient::parse_response(&json);
    assert!(result.is_err(), "缺少 choices 应返回错误");
}

#[test]
fn test_parse_response_empty_choices() {
    let json = json!({
        "model": "x",
        "choices": [],
        "usage": {"prompt_tokens": 10, "completion_tokens": 0}
    });
    let result = LlmClient::parse_response(&json);
    assert!(result.is_err(), "空 choices 应返回错误");
}

#[test]
fn test_parse_response_missing_content() {
    let json = json!({
        "model": "x",
        "choices": [{"message": {"role": "assistant"}}]
    });
    let result = LlmClient::parse_response(&json);
    assert!(result.is_err(), "缺少 content 应返回错误");
}

#[test]
fn test_parse_response_usage_partial() {
    // usage 部分字段缺失
    let json = json!({
        "model": "x",
        "choices": [{"message": {"content": "ok"}}],
        "usage": {"prompt_tokens": 50}
    });
    let resp = LlmClient::parse_response(&json).unwrap();
    assert_eq!(resp.input_tokens, 50);
    assert_eq!(resp.output_tokens, 0, "缺失的 completion_tokens 应为 0");
}

// ============================================================
// 集成：构造请求 → 解析响应
// ============================================================

#[test]
fn test_build_then_parse_roundtrip() {
    // 构造请求体不等于直接解析，但可验证两端数据结构一致
    let client = make_client(false);
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("user input"),
    ];
    let body = client.build_request_body(&messages);

    // 请求体中的 messages 应与原始消息一致
    let body_messages: Vec<ChatMessage> = serde_json::from_value(body["messages"].clone()).unwrap();
    assert_eq!(body_messages, messages);

    // 构造一个模拟响应并解析
    let mock_resp = json!({
        "model": "test-model",
        "choices": [{"message": {"content": "分析结果"}}],
        "usage": {"prompt_tokens": 20, "completion_tokens": 10}
    });
    let parsed = LlmClient::parse_response(&mock_resp).unwrap();
    assert_eq!(parsed.content, "分析结果");
    assert_eq!(parsed.model, "test-model");
}

#[test]
fn test_client_config_access() {
    let client = make_client(true);
    let config = client.config();
    assert_eq!(config.model_name, "test-model");
    assert!(config.reasoning);
}

// ============================================================
// LlmResponse 相等性测试
// ============================================================

#[test]
fn test_llm_response_eq() {
    let r1 = LlmResponse {
        content: "hello".into(),
        input_tokens: 10,
        output_tokens: 5,
        model: "m".into(),
    };
    let r2 = LlmResponse {
        content: "hello".into(),
        input_tokens: 10,
        output_tokens: 5,
        model: "m".into(),
    };
    assert_eq!(r1, r2);
}

#[test]
fn level_five_prompts_include_real_diagnostic_context() {
    let assignment = Assignment {
        title: "移动值".into(),
        description: "修复所有权错误".into(),
    };
    let diagnostic = RustcDiagnostic {
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
    let messages = compile_solution_messages(
        &assignment,
        "fn main() {}",
        &diagnostic,
        &classified,
        "学习画像：暂无",
    );
    assert_eq!(messages.len(), 2);
    assert!(messages[1].content.contains("E0382"));
    assert!(messages[1].content.contains("fn main()"));
    assert!(messages[0].content.contains("Level 5"));

    let failed = TestResult {
        name: "empty".into(),
        passed: false,
        actual_output: "1".into(),
        expected_output: "0".into(),
    };
    let messages = test_solution_messages(&assignment, "fn main() {}", &failed, "画像");
    assert!(messages[1].content.contains("empty"));
    assert!(messages[1].content.contains("期望输出：0"));
}
