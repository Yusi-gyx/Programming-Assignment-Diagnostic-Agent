//! LLM 客户端测试（第 9 步）
//!
//! 全部离线测试（不发送真实 HTTP 请求），
//! 仅验证请求体构造与响应解析的纯函数逻辑。
//!
//! ```bash
//! cargo test --test llm_tests
//! ```

use pada::agent::llm::{
    ChatMessage, LlmClient, LlmResponse, ModelTaskKind, compile_hint_messages,
    compile_solution_messages, parse_stream_response, test_hint_messages, test_solution_messages,
};
use pada::analysis::error_parser::{RustcDiagnostic, Severity};
use pada::config::effort::{EffortMode, EffortPolicy};
use pada::config::model::{ModelConfig, ReasoningProtocol};
use pada::models::{Assignment, Diagnostic, ErrorCategory, HintLevel, KnowledgePoint, TestResult};
use serde_json::json;
use std::io::Cursor;
use std::sync::atomic::AtomicBool;

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
fn test_ollama_request_omits_incompatible_boolean_reasoning() {
    let client = make_client(true);
    let messages = vec![ChatMessage::user("test")];
    let body = client.build_request_body(&messages);

    assert!(body.get("reasoning").is_none());
    assert_eq!(body["reasoning_effort"], "medium");
}

#[test]
fn ollama_disables_thinking_and_uses_supported_levels() {
    let body = make_client(false).build_request_body(&[]);
    assert_eq!(body["reasoning_effort"], "none");
    assert!(body.get("reasoning").is_none());
    for (mode, expected) in [
        (EffortMode::Low, "low"),
        (EffortMode::Medium, "medium"),
        (EffortMode::High, "high"),
        (EffortMode::Xhigh, "high"),
        (EffortMode::Max, "high"),
    ] {
        let mut config = ModelConfig::local("qwen3:8b", 8192);
        config.reasoning = true;
        let client = LlmClient::with_effort(config, mode.initial_policy());
        assert_eq!(client.build_request_body(&[])["reasoning_effort"], expected);
        assert_eq!(
            client.build_request_body_for_task(&[], ModelTaskKind::KnowledgeMapping)["reasoning_effort"],
            "low"
        );
    }
    let client = LlmClient::new(ModelConfig::local("gpt-oss:20b", 8192));
    assert_eq!(client.build_request_body(&[])["reasoning_effort"], "low");
}

#[test]
fn explicit_protocol_controls_proxy_requests_without_mixing_fields() {
    for protocol in [
        ReasoningProtocol::Deepseek,
        ReasoningProtocol::Ollama,
        ReasoningProtocol::EnableThinking,
        ReasoningProtocol::Compatible,
    ] {
        for enabled in [false, true] {
            let mut config =
                ModelConfig::cloud("https://proxy.example/v1", "", "custom", 8192, 0.0, 0.0);
            config.reasoning_protocol = protocol;
            config.reasoning = enabled;
            let body = LlmClient::new(config).build_request_body(&[]);
            match protocol {
                ReasoningProtocol::Deepseek => {
                    assert_eq!(
                        body["thinking"]["type"],
                        if enabled { "enabled" } else { "disabled" }
                    );
                    assert_eq!(body.get("reasoning_effort").is_some(), enabled);
                }
                ReasoningProtocol::Ollama => assert_eq!(
                    body["reasoning_effort"],
                    if enabled { "medium" } else { "none" }
                ),
                ReasoningProtocol::EnableThinking => {
                    assert_eq!(body["enable_thinking"], enabled);
                    assert!(body.get("reasoning_effort").is_none());
                }
                ReasoningProtocol::Compatible => {
                    assert_eq!(body.get("reasoning").is_some(), enabled)
                }
                ReasoningProtocol::Auto => unreachable!(),
            }
            assert_eq!(
                body.get("thinking").is_some(),
                protocol == ReasoningProtocol::Deepseek
            );
            assert_eq!(
                body.get("enable_thinking").is_some(),
                protocol == ReasoningProtocol::EnableThinking
            );
            if protocol != ReasoningProtocol::Compatible {
                assert!(body.get("reasoning").is_none());
            }
        }
    }
}

#[test]
fn test_cloud_request_includes_reasoning_when_enabled() {
    let mut config = ModelConfig::cloud(
        "https://api.example.com/v1/chat/completions",
        "key",
        "reasoner",
        8192,
        0.0,
        0.0,
    );
    config.reasoning = true;
    let client = LlmClient::new(config);
    let body = client.build_request_body(&[ChatMessage::user("test")]);

    assert_eq!(body["reasoning"], true);
    assert_eq!(body["reasoning_effort"], "medium");
}

#[test]
fn test_runtime_effort_is_sent_when_reasoning_is_supported() {
    let mut config = ModelConfig::cloud(
        "https://api.example.com/v1/chat/completions",
        "key",
        "reasoner",
        8192,
        0.0,
        0.0,
    );
    config.reasoning = true;
    let client = LlmClient::with_effort(config, EffortPolicy::for_mode(EffortMode::High));
    let body = client.build_request_body(&[ChatMessage::user("test")]);
    assert_eq!(body["reasoning_effort"], "high");
}

fn deepseek_client(reasoning: bool, mode: EffortMode) -> LlmClient {
    let mut config = ModelConfig::cloud(
        "https://api.deepseek.com/v1",
        "",
        "deepseek-v4-pro",
        1_000_000,
        0.0,
        0.0,
    );
    config.reasoning = reasoning;
    LlmClient::with_effort(config, mode.initial_policy())
}

#[test]
fn deepseek_explicitly_disables_thinking_for_every_task() {
    let client = deepseek_client(false, EffortMode::Max);
    for task in [
        ModelTaskKind::General,
        ModelTaskKind::KnowledgeMapping,
        ModelTaskKind::Hint(HintLevel::Concept),
        ModelTaskKind::Hint(HintLevel::Solution),
    ] {
        let body = client.build_request_body_for_task(&[ChatMessage::user("test")], task);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("reasoning").is_none());
    }
}

#[test]
fn deepseek_uses_native_effort_without_changing_test_policy() {
    for (mode, expected) in [
        (EffortMode::Auto, "low"),
        (EffortMode::Low, "low"),
        (EffortMode::Medium, "low"),
        (EffortMode::High, "high"),
        (EffortMode::Xhigh, "high"),
        (EffortMode::Max, "max"),
    ] {
        let client = deepseek_client(true, mode);
        for task in [
            ModelTaskKind::General,
            ModelTaskKind::Hint(HintLevel::Direction),
            ModelTaskKind::Hint(HintLevel::Solution),
        ] {
            let body = client.build_request_body_for_task(&[], task);
            assert_eq!(body["thinking"], json!({"type": "enabled"}));
            assert_eq!(body["reasoning_effort"], expected, "{mode} {task:?}");
            assert!(body.get("reasoning").is_none());
        }
        assert_eq!(mode.initial_policy().run_tests, mode != EffortMode::Low);
    }
}

#[test]
fn deepseek_concepts_and_mapping_use_low_even_in_deep_diagnoses() {
    for mode in EffortMode::ALL {
        let client = deepseek_client(true, mode);
        for task in [
            ModelTaskKind::KnowledgeMapping,
            ModelTaskKind::Hint(HintLevel::Concept),
        ] {
            let body = client.build_request_body_for_task(&[], task);
            assert_eq!(body["reasoning_effort"], "low", "{mode} {task:?}");
        }
    }
}

#[test]
fn deepseek_adapter_uses_host_not_model_name_or_url_substrings() {
    for (endpoint, direct) in [
        ("https://api.deepseek.com", true),
        ("https://API.DEEPSEEK.COM:443/v1/chat/completions", true),
        ("https://proxy.example/v1", false),
        ("https://api.deepseek.com.example/v1", false),
        ("https://proxy.example/api.deepseek.com", false),
        ("https://api.deepseek.com@proxy.example/v1", false),
        ("http://localhost:11434/v1", false),
    ] {
        let mut config = ModelConfig::cloud(endpoint, "", "deepseek-v4-pro", 8192, 0.0, 0.0);
        config.reasoning = true;
        assert_eq!(config.is_deepseek(), direct, "{endpoint}");
        let body = LlmClient::new(config).build_request_body(&[]);
        assert_eq!(body.get("thinking").is_some(), direct, "{endpoint}");
        if !direct && !endpoint.contains("11434") {
            assert_eq!(body["reasoning"], true);
            assert_eq!(body["reasoning_effort"], "medium");
        }
    }
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

#[test]
fn test_parse_stream_response_combines_chunks_and_usage() {
    let stream = concat!(
        "data: {\"model\":\"local-model\",\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"好\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n"
    );
    let cancelled = AtomicBool::new(false);

    let response = parse_stream_response(Cursor::new(stream), &cancelled).unwrap();

    assert_eq!(response.content, "你好");
    assert_eq!(response.model, "local-model");
    assert_eq!(response.input_tokens, 12);
    assert_eq!(response.output_tokens, 2);
}

#[test]
fn test_parse_stream_response_honors_preexisting_cancellation() {
    let cancelled = AtomicBool::new(true);
    let result = parse_stream_response(Cursor::new("data: [DONE]\n"), &cancelled);

    assert!(result.is_err());
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

#[test]
fn task_output_caps_are_configurable_and_batches_are_bounded() {
    let mut config = ModelConfig::local("test-model", 8192);
    config.output_limits.mapping = 333;
    config.output_limits.concept = 444;
    config.output_limits.direction = 555;
    config.output_limits.solution = 666;
    config.output_limits.test_generation = 777;
    let client = LlmClient::new(config);
    for (task, expected) in [
        (ModelTaskKind::KnowledgeMapping, 333),
        (ModelTaskKind::Hint(HintLevel::Concept), 444),
        (ModelTaskKind::Hint(HintLevel::Direction), 555),
        (ModelTaskKind::Hint(HintLevel::Solution), 666),
        (ModelTaskKind::TestGeneration, 777),
        (
            ModelTaskKind::HintBatch {
                level: HintLevel::Direction,
                count: 3,
            },
            1665,
        ),
        (
            ModelTaskKind::HintBatch {
                level: HintLevel::Direction,
                count: usize::MAX,
            },
            4440,
        ),
    ] {
        assert_eq!(
            client.build_request_body_for_task(&[], task)["max_tokens"],
            expected
        );
    }
}

#[test]
fn truncated_json_and_sse_keep_usage_but_fail_completeness_checks() {
    let json = json!({"choices":[{"message":{"content":null},"finish_reason":"length"}],
        "usage":{"prompt_tokens":10,"completion_tokens":40,"completion_tokens_details":{"reasoning_tokens":39}}});
    let response = LlmClient::parse_response(&json).unwrap();
    assert!(response.ensure_complete().is_err());
    assert_eq!(response.output_tokens, 40);
    assert_eq!(response.details.reasoning_tokens, Some(39));
    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hidden\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":40,\"completion_tokens_details\":{\"reasoning_tokens\":39}}}\n\n",
        "data: [DONE]\n\n"
    );
    let response = parse_stream_response(Cursor::new(stream), &AtomicBool::new(false)).unwrap();
    assert!(response.content.is_empty());
    assert!(response.ensure_complete().is_err());
    assert_eq!(response.output_tokens, 40);
    assert_eq!(response.details.reasoning_tokens, Some(39));
}

#[test]
fn old_response_history_defaults_new_metrics_without_inventing_counts() {
    let json = json!({"content":"old", "model":"model", "input_tokens":5, "output_tokens":2,
        "timings":{"prompt_build_ms":0,"api_ttft_ms":12,"total_ms":15}});
    let response: LlmResponse = serde_json::from_value(json).unwrap();
    assert_eq!(response.timings.api_ttft_ms, Some(12));
    assert_eq!(response.timings.api_first_reasoning_ms, None);
    assert_eq!(response.details.reasoning_tokens, None);
    assert!(response.ensure_complete().is_ok());
}

// ============================================================
// LlmResponse 相等性测试
// ============================================================

#[test]
fn test_llm_response_eq() {
    let r1 = LlmResponse {
        details: Default::default(),
        timings: Default::default(),
        content: "hello".into(),
        input_tokens: 10,
        output_tokens: 5,
        model: "m".into(),
    };
    let r2 = LlmResponse {
        details: Default::default(),
        timings: Default::default(),
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
        runtime_error: None,
    };
    let messages = test_solution_messages(&assignment, "fn main() {}", &failed, "画像");
    assert!(messages[1].content.contains("empty"));
    assert!(messages[1].content.contains("期望输出：0"));
}

#[test]
fn level_three_prompt_requires_generic_example_without_answer() {
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
    let messages = compile_hint_messages(
        &assignment,
        "fn main() {}",
        &diagnostic,
        &classified,
        HintLevel::Concept,
        "知识点：所有权",
        "学习画像：暂无",
    );
    assert!(messages[0].content.contains("Level 3"));
    assert!(messages[0].content.contains("通用示例"));
    assert!(messages[0].content.contains("不给本题答案"));
    assert!(messages[1].content.contains("Rust 基础提示"));
}

#[test]
fn level_four_test_prompt_requires_classic_error_pattern() {
    let assignment = Assignment {
        title: "偶数求和".into(),
        description: "计算所有偶数之和".into(),
    };
    let result = TestResult {
        name: "mixed".into(),
        passed: false,
        actual_output: "9".into(),
        expected_output: "6".into(),
        runtime_error: None,
    };
    let classified = Diagnostic {
        category: ErrorCategory::LogicError,
        knowledge_points: vec![KnowledgePoint::Iterator],
        confidence: 0.7,
    };
    let messages = test_hint_messages(
        &assignment,
        "fn main() {}",
        &result,
        &classified,
        HintLevel::Direction,
        "检查筛选条件",
        "学习画像：暂无",
    );
    assert!(messages[0].content.contains("Level 4"));
    assert!(messages[0].content.contains("经典错误写法"));
    assert!(messages[0].content.contains("不得生成用户提交的完整修正版"));
    assert!(messages[1].content.contains("迭代器 / Iterator"));
}
