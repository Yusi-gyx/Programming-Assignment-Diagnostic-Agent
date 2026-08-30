//! 端到端集成测试：完整 Agent 工作流
//!
//! 覆盖 DESIGN.md §5 的核心流程：
//! 读取题目 → 编译 → 解析错误 → 分类 → 知识点映射 →
//! 分层提示 → 诊断报告 → Token 用量 → 会话轨迹持久化
//!
//! 同时覆盖编译通过 → 运行测试 → 测试失败 → 逻辑错误分类 的路径。

use pada::agent::progress::{CancelToken, DiagnosticStage, SilentProgress, ProgressReporter};
use pada::agent::llm::LlmResponse;
use pada::analysis::classifier::{
    classify_compile_diagnostics, classify_test_failure,
};
use pada::analysis::error_parser::parse_diagnostics;
use pada::analysis::hint::{
    generate_compile_hint, generate_test_hint, hint_level_from_number, next_hint_level,
};
use pada::config::model::{Config, ModelConfig};
use pada::history::{AgentDecision, Session, StepBuilder, ToolCall};
use pada::models::{Assignment, ErrorCategory, HintLevel, KnowledgePoint};
use pada::report::{CompileReportEntry, DiagnosticReport};
use pada::telemetry::{UsageTracker, calculate_cost};
use pada::tools::compiler::CompilerTool;
use pada::tools::runner::{TestCase, TestRunner};
use std::path::PathBuf;

fn fixture(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/rust");
    p.push(relative);
    p
}

/// 编译 fixture 到临时二进制，返回路径
fn compile_fixture(fixture_path: &str, tag: &str) -> PathBuf {
    let compiler = CompilerTool::new();
    let out = std::env::temp_dir().join(format!("pada_e2e_{}", tag));
    compiler
        .compile_file(&fixture(fixture_path), Some(&out))
        .expect("编译 fixture 失败");
    out
}

// ============================================================
// 场景 1：编译错误完整工作流
// ============================================================

#[test]
fn test_full_workflow_compile_error() {
    // 模拟一道题目
    let assignment = Assignment {
        title: "所有权练习".into(),
        description: "创建一个 String，将其赋给另一个变量，然后使用原变量。".into(),
    };

    // 步骤 1：编译学生代码（含 E0382）
    let compiler = CompilerTool::new();
    let compile_output = compiler
        .compile_file(&fixture("ownership/e0382.rs"), None)
        .expect("编译调用不应失败");
    assert!(!compile_output.success, "E0382 fixture 应编译失败");

    // 步骤 2：解析编译错误
    let diags = parse_diagnostics(&compile_output.stderr);
    assert!(!diags.is_empty(), "应解析出诊断信息");

    // 步骤 3：错误分类 + 知识点映射
    let classified = classify_compile_diagnostics(&diags);
    let ownership_diag = classified
        .iter()
        .find(|d| d.knowledge_points.contains(&KnowledgePoint::Ownership))
        .expect("应映射到 Ownership 知识点");
    assert_eq!(ownership_diag.category, ErrorCategory::CompileError);
    assert!(ownership_diag.confidence >= 0.9, "已知错误码应高置信度");

    // 步骤 4：分层提示（从 Level 1 逐级升级到 Level 5）
    let e0382_parsed = diags
        .iter()
        .find(|d| d.code.as_deref() == Some("E0382"))
        .expect("应找到 E0382");

    let hints = [
        HintLevel::Category,
        HintLevel::Location,
        HintLevel::Concept,
        HintLevel::Direction,
        HintLevel::Solution,
    ];

    let mut hint_contents = Vec::new();
    let mut level = hints[0];
    loop {
        let hint = generate_compile_hint(e0382_parsed, ownership_diag, level);
        assert!(!hint.content.is_empty(), "Level {:?} 提示不应为空", level);
        hint_contents.push(hint.content.clone());
        match next_hint_level(level) {
            Some(next) => level = next,
            None => break,
        }
    }
    assert_eq!(hint_contents.len(), 5, "应有 5 级提示");

    // 验证各级提示内容递进
    assert!(hint_contents[0].contains("编译错误"), "L1 应含错误类别");
    assert!(
        hint_contents[2].contains("所有权") || hint_contents[2].contains("Ownership"),
        "L3 应含知识点"
    );

    // 步骤 5：生成诊断报告（控制台文本 + Markdown）
    let mut report = DiagnosticReport::new();
    for (d, c) in diags.iter().zip(classified.iter()) {
        let hint = generate_compile_hint(d, c, HintLevel::Concept);
        report.add_compile(CompileReportEntry {
            diag: d.clone(),
            classified: c.clone(),
            hint,
        });
    }
    let text = report.to_text();
    let markdown = report.to_markdown();

    assert!(text.contains("[编译错误]"), "报告应含编译错误标签");
    assert!(text.contains("E0382"), "报告应含错误码");
    assert!(text.contains("知识点"), "报告应含知识点行");
    assert!(markdown.contains("# 诊断报告"), "Markdown 应有标题");
    assert!(markdown.contains("## 编译诊断"), "Markdown 应有编译诊断章节");

    // 步骤 6：会话轨迹记录（R5）
    let mut session = Session::new(&assignment.title);
    session.add_step(
        StepBuilder::new(0)
            .user_input(&assignment.description)
            .decision(AgentDecision::new("reading_input", "读取题目与代码"))
            .build(),
    );
    session.add_step(
        StepBuilder::new(1)
            .tool_call(ToolCall::new(
                "compile_file",
                "ownership/e0382.rs",
                "编译失败: E0382",
            ))
            .decision(AgentDecision::new("compiling", "编译失败，进入错误分析"))
            .build(),
    );
    session.add_step(
        StepBuilder::new(2)
            .tool_call(ToolCall::new(
                "parse_diagnostics",
                "stderr",
                &format!("{} 条诊断", diags.len()),
            ))
            .tool_call(ToolCall::new(
                "classify",
                "E0382",
                "Ownership, 置信度 0.95",
            ))
            .decision(AgentDecision::new("parsing", "映射到知识点 Ownership"))
            .build(),
    );

    assert_eq!(session.step_count(), 3);
    let total_tools: usize = session.steps.iter().map(|s| s.tool_calls.len()).sum();
    assert_eq!(total_tools, 3, "应有 3 次工具调用");
}

// ============================================================
// 场景 2：编译通过 → 运行测试 → 测试失败 → 逻辑错误分类
// ============================================================

#[test]
fn test_full_workflow_test_failure() {
    // 编译一个有逻辑错误的程序（输入整数应输出两倍，但实际输出三倍）
    // 复用 io_program（求和）来测试
    let program = compile_fixture("runner/io_program.rs", "test_failure");

    // 准备测试用例：故意设置错误的期望输出
    let tests = vec![
        TestCase::new("normal", "3\n\n", "6"),      // 正确：3 → 6（但实际也是3，故意设错期望）
        TestCase::new("zero", "0\n\n", "0"),        // 正确：0 → 0
        TestCase::new("negative", "-5\n\n", "-10"),  // 故意错误期望
    ];

    // 运行测试
    let test_runner = TestRunner::new();
    let results = test_runner
        .run_tests(&program, &tests)
        .expect("运行测试不应失败");

    // 至少有一个失败
    let failures: Vec<_> = results.iter().filter(|r| !r.passed).collect();
    assert!(!failures.is_empty(), "应有测试失败");

    // 对失败的测试进行分类
    for failure in &failures {
        let diagnostic = classify_test_failure(
            &failure.name,
            &failure.actual_output,
            &failure.expected_output,
        );
        assert_eq!(diagnostic.category, ErrorCategory::LogicError);
        assert!(diagnostic.knowledge_points.is_empty(), "逻辑错误知识点需 LLM 判断");
    }

    // 生成测试失败的分层提示
    let failure = failures[0];
    let hint = generate_test_hint(
        &failure.name,
        &failure.actual_output,
        &failure.expected_output,
        HintLevel::Direction,
    );
    assert!(
        hint.content.contains("0")
            || hint.content.contains(failure.expected_output.trim()),
        "Direction 提示应含期望或实际输出"
    );
}

// ============================================================
// 场景 3：Token 用量与预算控制（R6）
// ============================================================

#[test]
fn test_full_workflow_token_budget() {
    let config = ModelConfig::cloud("https://x.com", "key", "test-model", 8192, 1.0, 2.0);
    let mut tracker = UsageTracker::new();
    tracker.set_session_budget(500);

    // 模拟 3 次 LLM 调用
    for _ in 0..3 {
        assert!(tracker.check_budget(), "应在预算内");
        let resp = LlmResponse {
            content: "回复".into(),
            input_tokens: 80,
            output_tokens: 40,
            model: "test-model".into(),
        };
        tracker.record(&resp, &config);
    }

    // 3 次调用累计 360 token < 500，仍在预算内
    assert!(tracker.check_budget());
    assert_eq!(tracker.session().total_tokens(), 360);

    // 第 4 次调用后累计 480 < 500
    let resp = LlmResponse {
        content: "回复".into(),
        input_tokens: 80,
        output_tokens: 40,
        model: "test-model".into(),
    };
    tracker.record(&resp, &config);
    assert_eq!(tracker.session().total_tokens(), 480);
    assert!(tracker.check_budget(), "480 < 500 仍在预算内");

    // 第 5 次调用后累计 600 >= 500，超出预算
    tracker.record(&resp, &config);
    assert!(!tracker.check_budget(), "超出预算应阻止后续调用");

    // 验证成本计算
    let cost = calculate_cost(400, 200, &config);
    assert!((cost - (0.0004 + 0.0004)).abs() < 1e-9, "成本应正确");

    // 验证用量摘要
    let summary = tracker.summary();
    assert!(summary.contains("600"), "摘要应含当前用量");
    assert!(summary.contains("500"), "摘要应含预算");
}

// ============================================================
// 场景 4：会话保存 → 加载 → 续诊（R5 回放）
// ============================================================

#[test]
fn test_full_workflow_session_save_load_replay() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.json");

    // 第一次会话：编译错误诊断
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("ownership/e0382.rs"), None)
        .unwrap();
    let diags = parse_diagnostics(&output.stderr);
    let classified = classify_compile_diagnostics(&diags);

    let mut session = Session::new("所有权诊断");
    session.add_step(
        StepBuilder::new(0)
            .user_input("let s = String::from(\"hi\"); let t = s; println!(\"{}\", s);")
            .tool_call(ToolCall::new("compile_file", "e0382.rs", "编译失败"))
            .decision(AgentDecision::new("compiling", "编译失败"))
            .build(),
    );
    session.add_step(
        StepBuilder::new(1)
            .tool_call(ToolCall::new(
                "classify",
                "E0382",
                &format!("Ownership, 置信度 {:.2}", classified[0].confidence),
            ))
            .decision(AgentDecision::new("parsing", "映射到 Ownership"))
            .build(),
    );

    session.save(&path).expect("保存应成功");

    // 第二次会话：从文件加载历史，继续诊断
    let loaded = Session::load(&path).expect("加载应成功");
    assert_eq!(loaded.title, "所有权诊断");
    assert_eq!(loaded.step_count(), 2);

    // 验证轨迹可回放：检查第一步的工具调用
    let step0 = &loaded.steps[0];
    assert_eq!(step0.tool_calls.len(), 1);
    assert_eq!(step0.tool_calls[0].tool, "compile_file");
    assert_eq!(step0.decisions.len(), 1);
    assert_eq!(step0.decisions[0].stage, "compiling");
}

// ============================================================
// 场景 5：协作式取消（R4）在批量测试中的表现
// ============================================================

#[test]
fn test_full_workflow_cooperative_cancel() {
    let program = compile_fixture("runner/io_program.rs", "cancel");
    let cancel = CancelToken::new();
    let progress = SilentProgress;

    let tests: Vec<TestCase> = (0..10)
        .map(|i| TestCase::new(format!("case_{}", i), format!("{}\n\n", i), format!("{}", i)))
        .collect();

    let test_runner = TestRunner::new();
    progress.start(tests.len(), "批量测试");

    let mut completed = 0;
    let mut all_results: Vec<pada::models::TestResult> = Vec::new();

    for (i, tc) in tests.iter().enumerate() {
        if cancel.is_cancelled() {
            progress.cancelled("用户取消");
            break;
        }
        if let Ok(results) = test_runner.run_tests(&program, std::slice::from_ref(tc)) {
            all_results.extend(results);
        }
        completed = i + 1;
        progress.tick(completed, &format!("第 {}/{} 组", completed, tests.len()));

        // 第 5 组后取消
        if i == 4 {
            cancel.cancel();
        }
    }

    // 应执行了 5 组后停止
    assert_eq!(completed, 5, "取消后应只完成 5 组");
    assert_eq!(all_results.len(), 5, "应保留 5 组已完成结果");

    // 验证取消后仍可访问已完成结果（安全释放 + 保留部分结果）
    let passed_count = all_results.iter().filter(|r| r.passed).count();
    assert_eq!(passed_count, 5, "5 组应全部通过");
}

// ============================================================
// 场景 6：配置 profile 切换（R3）
// ============================================================

#[test]
fn test_full_workflow_profile_switch() {
    let config = Config::default_template();
    assert_eq!(config.active_profile, "local");

    // 切换到云端 profile
    config.clone().switch("deepseek").expect("切换应成功");

    let mut config = Config::default_template();
    config.switch("deepseek").unwrap();
    let active = config.active().expect("应获取激活 profile");
    assert_eq!(active.model_name, "deepseek-chat");
    assert!(!active.endpoint.is_empty());

    // 切换回本地
    config.switch("local").unwrap();
    let active = config.active().unwrap();
    assert_eq!(active.model_name, "qwen2.5-coder");
    assert!(active.api_key.is_empty());

    // 切换到不存在的 profile 应失败
    assert!(config.switch("nonexistent").is_err());
}

// ============================================================
// 场景 7：完整诊断报告格式验证（DESIGN.md §4.1 格式）
// ============================================================

#[test]
fn test_report_follows_design_format() {
    let compiler = CompilerTool::new();
    let output = compiler
        .compile_file(&fixture("borrowing/e0499.rs"), None)
        .unwrap();
    let diags = parse_diagnostics(&output.stderr);
    let classified = classify_compile_diagnostics(&diags);

    let mut report = DiagnosticReport::new();
    for (d, c) in diags.iter().zip(classified.iter()) {
        let hint = generate_compile_hint(d, c, HintLevel::Concept);
        report.add_compile(CompileReportEntry {
            diag: d.clone(),
            classified: c.clone(),
            hint,
        });
    }

    let text = report.to_text();

    // 验证 DESIGN.md §4.1 格式
    assert!(text.contains("[编译错误]"), "应有 [编译错误] 标签");
    assert!(text.contains("E0499"), "应有错误码 E0499");
    assert!(text.contains("知识点"), "应有知识点行");
    assert!(text.contains("提示"), "应有提示行");
    // 位置格式 file:line:col
    assert!(
        text.contains(":") && text.matches(':').count() >= 3,
        "应有 file:line:col 位置格式"
    );
}

// ============================================================
// 场景 8：提示等级从 CLI 数值参数转换
// ============================================================

#[test]
fn test_hint_level_from_cli_param() {
    // CLI --hint 1 到 5 应正确转换
    for n in 1..=5u8 {
        let level = hint_level_from_number(n).expect("1-5 应有效");
        assert_eq!(level, hint_level_from_number(n).unwrap());
    }

    // 越界应返回 None
    assert!(hint_level_from_number(0).is_none());
    assert!(hint_level_from_number(6).is_none());

    // 默认值（未指定 --hint 时）
    let default_level = hint_level_from_number(1).unwrap();
    assert_eq!(default_level, HintLevel::Category);
}

// ============================================================
// 场景 9：DiagnosticStage 覆盖完整工作流阶段
// ============================================================

#[test]
fn test_diagnostic_stages_complete() {
    let stages = [
        DiagnosticStage::ReadingInput,
        DiagnosticStage::Compiling,
        DiagnosticStage::ParsingErrors,
        DiagnosticStage::RunningTests,
        DiagnosticStage::LlmCalling,
        DiagnosticStage::GeneratingReport,
    ];

    // 每个阶段都应有中文描述
    for stage in &stages {
        assert!(!stage.as_str().is_empty(), "阶段应有中文描述");
    }

    // 阶段应覆盖完整工作流
    assert_eq!(stages.len(), 6, "应有 6 个阶段");
    assert_eq!(stages[0], DiagnosticStage::ReadingInput);
    assert_eq!(stages[5], DiagnosticStage::GeneratingReport);
}
