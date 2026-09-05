use pada::config::model::{Config, ModelConfig};
use pada::history::{Session, SessionContext};
use pada::storage::DataStore;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn configure(store: &DataStore, endpoint: String) {
    let mut config = Config::default_template();
    let mut model = ModelConfig::local("test", 8192);
    model.endpoint = endpoint;
    config.profiles.insert("local".into(), model);
    config.save(&store.config_path()).unwrap();
}

#[test]
#[cfg(unix)]
fn cancelling_model_wait_saves_elapsed_trace_without_partial_response() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().to_path_buf());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    configure(
        &store,
        format!("http://{}/v1", listener.local_addr().unwrap()),
    );
    let problem = temp.path().join("problem.md");
    std::fs::write(&problem, "所有权练习").unwrap();
    let code = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rust/ownership/e0382.rs");
    let (connected, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        connected.send(()).unwrap();
        let _ = released.recv_timeout(Duration::from_secs(10));
        drop(socket);
    });
    let command = format!(
        "\"{}\" --data-dir \"{}\" diagnose --problem \"{}\" --code \"{}\" --hint 3",
        env!("CARGO_BIN_EXE_pada"),
        temp.path().display(),
        problem.display(),
        code.display()
    );
    let mut child = Command::new("script")
        .args(["-qec", &command, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (prompt, ready) = mpsc::channel();
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            let n = stdout.read(&mut buffer).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..n]);
            if String::from_utf8_lossy(&bytes).contains("pada[3]>") {
                let _ = prompt.send(());
            }
        }
        bytes
    });
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let started = Instant::now();
    writeln!(child.stdin.as_mut().unwrap(), "q").unwrap();
    if ready.recv_timeout(Duration::from_secs(3)).is_err() {
        let _ = child.kill();
        let _ = release.send(());
        panic!("model cancellation did not return to tutor");
    }
    assert!(started.elapsed() < Duration::from_secs(3));
    writeln!(child.stdin.as_mut().unwrap(), "exit").unwrap();
    assert!(child.wait().unwrap().success());
    release.send(()).unwrap();
    server.join().unwrap();
    let output = String::from_utf8(output.join().unwrap()).unwrap();
    assert!(output.contains("已取消"));
    let sessions = store.recent_sessions().unwrap();
    let session = &sessions[0].session;
    let call = session
        .steps
        .iter()
        .flat_map(|step| &step.tool_calls)
        .find(|call| call.tool == "llm_failed_call")
        .unwrap();
    let failure: pada::agent::model_task::FailedModelCall =
        serde_json::from_str(&call.output).unwrap();
    assert!(failure.cancelled);
    assert!(session.usage_records.is_empty());
    assert!(session.steps.iter().all(|step| step.llm_exchange.is_none()));
}

#[test]
fn resume_starts_at_one_without_any_model_request() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().to_path_buf());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    configure(
        &store,
        format!("http://{}/v1", listener.local_addr().unwrap()),
    );
    let problem = temp.path().join("problem.md");
    let code = temp.path().join("main.rs");
    let tests = temp.path().join("tests.json");
    std::fs::write(&problem, "输出 42").unwrap();
    std::fs::write(&code, "fn main() { println!(\"0\"); }").unwrap();
    std::fs::write(
        &tests,
        r#"[{"name":"wrong","input":"","expected_output":"42"}]"#,
    )
    .unwrap();
    let mut session = Session::new("历史 hint 5");
    session.set_context(SessionContext {
        problem,
        code: Some(code),
        tests: Some(tests),
        hint: 5,
        generate_tests: true,
        ..Default::default()
    });
    store.save_auto_session(&session).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pada"))
        .args([
            "--data-dir",
            temp.path().to_str().unwrap(),
            "resume",
            "1",
            "--no-interactive",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("resume blocked, possibly on an implicit model request");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("从 hint 1 开始"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "╭─ 最近的诊断会话",
        "│  01",
        "题目",
        "提交",
        "描述",
        "更新",
        "会话 ID",
        "╰─",
    ] {
        assert!(
            stdout.contains(expected),
            "resume output missing {expected}: {stdout}"
        );
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let saved = store.recent_sessions().unwrap();
    assert_eq!(saved[0].session.context.as_ref().unwrap().hint, 1);
    assert!(!saved[0].session.context.as_ref().unwrap().generate_tests);
    assert!(saved[0].session.usage_records.is_empty());
}

#[test]
#[cfg(unix)]
fn loading_tests_replaces_the_round_output_and_resets_to_hint_one() {
    let temp = tempfile::tempdir().unwrap();
    let problem = temp.path().join("problem.md");
    let code = temp.path().join("main.rs");
    let tests = temp.path().join("generated.json");
    std::fs::write(&problem, "输出 42").unwrap();
    std::fs::write(&code, "fn main() { println!(\"0\"); }").unwrap();
    std::fs::write(
        &tests,
        r#"[{"name":"generated_case","input":"","expected_output":"42"}]"#,
    )
    .unwrap();

    let command = format!(
        "\"{}\" --data-dir \"{}\" diagnose --problem \"{}\" --code \"{}\" --hint 5",
        env!("CARGO_BIN_EXE_pada"),
        temp.path().display(),
        problem.display(),
        code.display()
    );
    let mut child = Command::new("script")
        .args(["-qec", &command, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (prompt_seen, wait_for_prompt) = mpsc::channel();
    let output_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        let mut initial_announced = false;
        let mut new_round_announced = false;
        loop {
            let count = stdout.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            let current = String::from_utf8_lossy(&bytes);
            if !initial_announced && current.contains("pada[5]>") {
                initial_announced = true;
                let _ = prompt_seen.send(5);
            }
            if !new_round_announced && current.contains("pada[1]>") {
                new_round_announced = true;
                let _ = prompt_seen.send(1);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    wait_for_prompt
        .recv_timeout(Duration::from_secs(10))
        .expect("initial tutor prompt not shown");
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "test {}", tests.display()).unwrap();
    assert_eq!(
        wait_for_prompt
            .recv_timeout(Duration::from_secs(10))
            .expect("new test round prompt not shown"),
        1
    );
    writeln!(stdin, "exit").unwrap();
    drop(stdin);
    let status = child.wait().unwrap();
    let stdout = output_reader.join().unwrap();
    assert!(
        status.success(),
        "scripted interactive diagnosis failed: {stdout}"
    );
    let applied = stdout
        .find("测试文件已应用")
        .unwrap_or_else(|| panic!("missing applied marker: {stdout}"));
    let new_round = &stdout[applied..];
    assert!(
        new_round.contains("Hint 1"),
        "new round did not reset: {stdout}"
    );
    assert!(
        new_round.contains("测试结果"),
        "test results missing: {stdout}"
    );
    assert!(
        new_round.contains("generated_case"),
        "case missing: {stdout}"
    );
    assert!(
        new_round.contains("实际：0"),
        "actual output missing: {stdout}"
    );
}

#[test]
fn cargo_project_rejects_external_json_tests_instead_of_silently_skipping_them() {
    let temp = tempfile::tempdir().unwrap();
    let problem = temp.path().join("problem.md");
    let project = temp.path().join("project");
    let tests = temp.path().join("tests.json");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(&problem, "题目").unwrap();
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname='p'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::write(&tests, "[]").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pada"))
        .args([
            "--data-dir",
            temp.path().to_str().unwrap(),
            "diagnose",
            "--problem",
            problem.to_str().unwrap(),
            "--project",
            project.to_str().unwrap(),
            "--tests",
            tests.to_str().unwrap(),
            "--no-interactive",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cargo 项目模式暂不支持"), "{stderr}");
}

#[test]
fn cli_effort_controls_test_execution_and_verification() {
    let temp = tempfile::tempdir().unwrap();
    let problem = temp.path().join("problem.md");
    let code = temp.path().join("main.rs");
    let tests = temp.path().join("tests.json");
    std::fs::write(&problem, "输出 42").unwrap();
    std::fs::write(&code, "fn main() { println!(\"42\"); }").unwrap();
    std::fs::write(
        &tests,
        r#"[{"name":"answer","input":"","expected_output":"42"}]"#,
    )
    .unwrap();

    let run = |effort: &str| {
        Command::new(env!("CARGO_BIN_EXE_pada"))
            .args([
                "--data-dir",
                temp.path().to_str().unwrap(),
                "diagnose",
                "--problem",
                problem.to_str().unwrap(),
                "--code",
                code.to_str().unwrap(),
                "--tests",
                tests.to_str().unwrap(),
                "--effort",
                effort,
                "--no-interactive",
            ])
            .output()
            .unwrap()
    };

    let low = run("low");
    assert!(low.status.success());
    assert!(!String::from_utf8_lossy(&low.stdout).contains("测试结果"));
    assert!(String::from_utf8_lossy(&low.stderr).contains("已跳过 1 个测试"));

    let medium = run("medium");
    assert!(medium.status.success());
    assert!(String::from_utf8_lossy(&medium.stdout).contains("测试结果"));
    assert!(!String::from_utf8_lossy(&medium.stderr).contains("正在进行二次验证"));

    let high = run("high");
    assert!(high.status.success());
    assert!(String::from_utf8_lossy(&high.stdout).contains("二次验证"));
    assert!(String::from_utf8_lossy(&high.stderr).contains("正在进行二次验证 1/1"));
}

#[cfg(unix)]
#[test]
fn tutor_effort_command_defers_policy_without_reprinting_report() {
    let temp = tempfile::tempdir().unwrap();
    let problem = temp.path().join("problem.md");
    let code = temp.path().join("main.rs");
    std::fs::write(&problem, "输出 42").unwrap();
    std::fs::write(&code, "fn main() { println!(\"42\"); }").unwrap();
    let command = format!(
        "\"{}\" --data-dir \"{}\" diagnose --problem \"{}\" --code \"{}\"",
        env!("CARGO_BIN_EXE_pada"),
        temp.path().display(),
        problem.display(),
        code.display()
    );
    let mut child = Command::new("script")
        .args(["-qec", &command, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (prompt_sender, prompts) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        let mut announced = 0;
        loop {
            let count = stdout.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            let prompt_count = String::from_utf8_lossy(&bytes)
                .match_indices("pada[1]>")
                .count();
            while announced < prompt_count {
                announced += 1;
                let _ = prompt_sender.send(announced);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    assert_eq!(prompts.recv_timeout(Duration::from_secs(10)).unwrap(), 1);
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "effort low").unwrap();
    assert_eq!(prompts.recv_timeout(Duration::from_secs(10)).unwrap(), 2);
    writeln!(stdin, "exit").unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let output = reader.join().unwrap();
    assert!(output.contains("思考模式已切换为 low"), "{output}");
    assert!(output.contains("将在下一次诊断时生效"), "{output}");
    assert_eq!(output.matches("诊断结果").count(), 1, "{output}");
    let store = DataStore::new(temp.path().to_path_buf());
    let saved = store.recent_sessions().unwrap();
    assert_eq!(
        saved[0].session.context.as_ref().unwrap().effort,
        pada::config::effort::EffortMode::Low
    );
}

#[test]
fn cli_streams_before_completion_and_prints_usage_and_timings() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().to_path_buf());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    configure(
        &store,
        format!("http://{}/v1", listener.local_addr().unwrap()),
    );
    let problem = temp.path().join("problem.md");
    let code = temp.path().join("main.rs");
    std::fs::write(&problem, "练习类型系统").unwrap();
    std::fs::write(&code, "fn main() { let x: u32 = \"wrong\"; }").unwrap();
    let (release, proceed) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
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
        let mut request = vec![0; length];
        reader.read_exact(&mut request).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&request).unwrap();
        assert_eq!(request["stream"], true);
        let first = "data: {\"choices\":[{\"delta\":{\"content\":\"STREAM-FIRST\\n\"}}]}\n\n";
        let last = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":32,\"completion_tokens\":4}}\n\ndata: [DONE]\n\n";
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{first}", first.len() + last.len()).unwrap();
        socket.flush().unwrap();
        proceed.recv_timeout(Duration::from_secs(10)).unwrap();
        socket.write_all(last.as_bytes()).unwrap();
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_pada"))
        .args([
            "--data-dir",
            temp.path().to_str().unwrap(),
            "diagnose",
            "--problem",
            problem.to_str().unwrap(),
            "--code",
            code.to_str().unwrap(),
            "--hint",
            "3",
            "--no-interactive",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (observed, arrived) = mpsc::channel();
    let output = std::thread::spawn(move || {
        let mut text = String::new();
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if line.contains("STREAM-FIRST") {
                let _ = observed.send(());
            }
            text.push_str(&line);
            text.push('\n');
        }
        text
    });
    arrived.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        child.try_wait().unwrap().is_none(),
        "first chunk must precede completion"
    );
    release.send(()).unwrap();
    assert!(child.wait().unwrap().success());
    server.join().unwrap();
    let output = output.join().unwrap();
    assert_eq!(
        output.matches("STREAM-FIRST").count(),
        1,
        "model content was printed twice: {output}"
    );
    for label in [
        "本轮诊断统计",
        "读取输入",
        "编译检查",
        "分析与测试",
        "报告渲染",
        "Prompt 构建",
        "API TTFT",
        "LLM 总耗时",
        "Input Token",
        "Output Token",
        "Token 合计",
    ] {
        assert!(output.contains(label), "missing {label}: {output}");
    }
    assert!(output.find("STREAM-FIRST").unwrap() < output.find("本轮诊断统计").unwrap());
}

#[cfg(unix)]
#[test]
fn cancellation_interrupts_blocked_input_and_reaps_child_group() {
    let started = Instant::now();
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 30"]);
    let result =
        pada::tools::process::run_command(&mut command, &vec![b'x'; 1024 * 1024], None, || {
            started.elapsed() > Duration::from_millis(100)
        });
    assert!(matches!(result, Err(pada::error::PadaError::Cancelled)));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[cfg(unix)]
#[test]
fn runner_timeout_is_enforced() {
    let started = Instant::now();
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 30"]);
    let result = pada::tools::process::run_command(
        &mut command,
        &[],
        Some(Duration::from_millis(100)),
        || false,
    );
    assert!(matches!(result, Err(pada::error::PadaError::Run(_))));
    assert!(started.elapsed() < Duration::from_secs(3));
}
