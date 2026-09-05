use pada::agent::context::{compact_rules, project_sources, relevant_source};
use pada::agent::interaction::reset_hint_for_new_tests;
use pada::agent::llm::{LlmClient, StreamText, parse_stream_response_with_callback};
use pada::analysis::classifier::classify_compile_diagnostic;
use pada::analysis::error_parser::{SourceLocation, parse_diagnostics};
use pada::history::{AgentDecision, Session, SessionContext, StepBuilder};
use pada::models::{HintLevel, KnowledgePoint};
use pada::tools::{compiler::CompilerTool, test_gen::parse_test_cases};
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::sync::atomic::AtomicBool;

#[test]
fn cargo_module_error_maps_without_a_model() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rust/cargo/type_mismatch_project");
    let output = CompilerTool::new().cargo_check(&root).unwrap();
    let diagnostics = parse_diagnostics(&output.stderr);
    let error = diagnostics
        .iter()
        .find(|d| d.code.as_deref() == Some("E0308"))
        .unwrap();
    assert_eq!(
        classify_compile_diagnostic(error).knowledge_points,
        vec![KnowledgePoint::TypeSystem]
    );
    let source = project_sources(&root).unwrap();
    let relevant = relevant_source(&source, error.location.as_ref());
    assert!(relevant.contains("grade.rs"));
    assert!(relevant.contains("优秀"));
    assert!(!relevant.contains("mod grade;"));
}

#[test]
fn rules_preserve_constraints_and_code_whitespace() {
    let rules =
        compact_rules("# 输入\n\n0 <= n <= 10\n0 <= n <= 10\n# 样例\n```text\n  a\n\n  b\n```\n");
    let parsed: serde_json::Value = serde_json::from_str(&rules).unwrap();
    assert_eq!(
        parsed["assignment_rules"][0]["rules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(rules.contains("  a"));
    assert!(rules.contains("0 <= n <= 10"));
}

#[test]
fn snippets_keep_original_line_numbers() {
    let source = (1..=200).map(|n| format!("line{n}\n")).collect::<String>();
    let snippet = relevant_source(
        &source,
        Some(&SourceLocation {
            file: "main.rs".into(),
            line: 100,
            column: 1,
        }),
    );
    assert!(snippet.contains("100: line100"));
    assert!(!snippet.contains("200: line200"));
}

#[test]
fn case_parser_ignores_thinking_and_unrelated_brackets() {
    let result = parse_test_cases("<think>```json\n[not valid]\n```</think>\n说明 [边界]：\n```JSON\n[{\"name\":\"brackets\",\"input\":\"[]\",\"expected_output\":\"[ok]\"}]\n```\n[结束]").unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].expected_output, "[ok]");
    assert!(parse_test_cases("[{\"name\":\"bad\",\"input\":\"x\"}]").is_err());
}

#[test]
fn applying_new_tests_resets_diagnosis_to_hint_one() {
    for initial in [
        HintLevel::Location,
        HintLevel::Concept,
        HintLevel::Direction,
        HintLevel::Solution,
    ] {
        let mut level = initial;
        reset_hint_for_new_tests(&mut level);
        assert_eq!(level, HintLevel::Category);
    }
}

#[test]
fn history_summary_uses_saved_problem_not_current_file() {
    let mut session = Session::new("作业");
    session.set_context(SessionContext {
        problem: "/deleted/problem.md".into(),
        ..Default::default()
    });
    session.add_step(
        StepBuilder::new(0)
            .user_input("求数组中的最大值")
            .decision(AgentDecision::new("reading_input", "读取题目"))
            .build(),
    );
    assert!(session.summary().contains("/deleted/problem.md"));
    assert!(session.summary().contains("求数组中的最大值"));
    assert!(Session::new("旧记录").summary().contains("旧版记录"));
}

#[test]
fn stream_delivers_chunks_and_hides_split_thinking_tags() {
    let stream = ": heartbeat\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"好\"}}]}\n\ndata: [DONE]\n\n";
    let mut chunks = Vec::new();
    let result = parse_stream_response_with_callback(
        Cursor::new(stream),
        &AtomicBool::new(false),
        |chunk| chunks.push(chunk.to_owned()),
    )
    .unwrap();
    assert_eq!(chunks, vec!["你", "好"]);
    assert_eq!(result.content, "你好");
    let mut text = StreamText::default();
    assert_eq!(text.push("<thi"), "");
    assert_eq!(text.push("nk>隐藏</thi"), "");
    assert_eq!(text.push("nk>可见"), "可见");
}

#[test]
fn clients_reuse_http_socket_and_record_stream_usage() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        for _ in 0..2 {
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["stream"], true);
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}],\"usage\":{\"prompt_tokens\":8,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}", body.len(), body).unwrap();
            socket.flush().unwrap();
        }
    });
    let mut config = pada::config::model::ModelConfig::local("test-model", 8192);
    config.endpoint = endpoint;
    for _ in 0..2 {
        let client = LlmClient::new(config.clone());
        let response = client
            .chat(&[pada::agent::llm::ChatMessage::user("hello")])
            .unwrap();
        assert_eq!((response.input_tokens, response.output_tokens), (8, 1));
        assert!(response.timings.api_ttft_ms.is_some());
    }
    server.join().unwrap();
}
