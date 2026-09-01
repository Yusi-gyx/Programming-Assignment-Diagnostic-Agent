//! PADA CLI：参数解析和交互入口；诊断逻辑由库模块完成。
use clap::{ArgGroup, Parser, Subcommand};
use pada::agent::interaction::{InteractiveCommand, help_text, parse_command};
use pada::agent::progress::{CliProgress, ProgressReporter};
use pada::analysis::classifier::{classify_compile_diagnostics, classify_test_failure};
use pada::analysis::error_parser::{RustcDiagnostic, Severity, parse_diagnostics};
use pada::analysis::hint::{
    generate_compile_hint, generate_test_hint, hint_level_as_number, hint_level_from_number,
    next_hint_level,
};
use pada::history::{AgentDecision, Session, StepBuilder, ToolCall};
use pada::models::{Assignment, Diagnostic, HintLevel};
use pada::report::{CompileReportEntry, DiagnosticReport, TestReportEntry};
use pada::telemetry::UsageTracker;
use pada::tools::compiler::{CompileOutput, CompilerTool};
use pada::tools::runner::{TestCase, TestRunner};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "pada", version, about = "Rust 编程作业诊断 Agent")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 诊断学生代码；终端中默认进入多轮导师模式
    #[command(group(ArgGroup::new("submission").required(true).args(["code", "project"])))]
    Diagnose {
        #[arg(long)]
        problem: PathBuf,
        #[arg(long)]
        code: Option<PathBuf>,
        #[arg(long)]
        project: Option<PathBuf>,
        /// JSON 测试用例文件（name/input/expected_output 数组）
        #[arg(long)]
        tests: Option<PathBuf>,
        /// 使用配置的模型自动生成边界测试
        #[arg(long)]
        generate_tests: bool,
        #[arg(long, default_value = "1", value_parser = clap::value_parser!(u8).range(1..=5))]
        hint: u8,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        budget: Option<usize>,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        history: Option<PathBuf>,
        #[arg(long)]
        save: Option<PathBuf>,
        #[arg(long)]
        config: Option<PathBuf>,
        /// V2 学习画像文件；不存在时自动创建
        #[arg(long)]
        memory: Option<PathBuf>,
        /// 每个阶段开始前等待确认
        #[arg(long)]
        step: bool,
        /// 即使在终端中也执行一次后退出
        #[arg(long)]
        no_interactive: bool,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("诊断失败: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> pada::error::Result<()> {
    let Commands::Diagnose {
        problem,
        code,
        project,
        tests,
        hint,
        profile,
        budget,
        report,
        history,
        save,
        config,
        step,
        no_interactive,
        generate_tests,
        memory,
    } = cli.command;
    run_diagnose(DiagnoseOptions {
        problem,
        code,
        project,
        tests,
        generate_tests,
        hint,
        profile,
        budget,
        report,
        history,
        save,
        config,
        memory,
        step,
        interactive: io::stdin().is_terminal() && !no_interactive,
    })
}

struct DiagnoseOptions {
    problem: PathBuf,
    code: Option<PathBuf>,
    project: Option<PathBuf>,
    tests: Option<PathBuf>,
    generate_tests: bool,
    hint: u8,
    profile: Option<String>,
    budget: Option<usize>,
    report: Option<PathBuf>,
    history: Option<PathBuf>,
    save: Option<PathBuf>,
    config: Option<PathBuf>,
    memory: Option<PathBuf>,
    step: bool,
    interactive: bool,
}

fn run_diagnose(options: DiagnoseOptions) -> pada::error::Result<()> {
    let description = std::fs::read_to_string(&options.problem).map_err(|e| {
        pada::error::PadaError::FileNotFound(format!("{}: {e}", options.problem.display()))
    })?;
    let assignment = Assignment {
        title: options
            .problem
            .file_stem()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名题目".into()),
        description,
    };
    let mut level = hint_level_from_number(options.hint).expect("clap 已校验提示等级");
    let model_config = load_profile(options.config.as_deref(), options.profile.as_deref())?;
    if let Some((name, _)) = &model_config {
        eprintln!("模型配置: {name}");
    }
    let mut session = match options.history.as_deref() {
        Some(path) => {
            let value = Session::load(path)?;
            eprintln!("已继续会话: {}", value.summary());
            value
        }
        None => Session::new(&assignment.title),
    };
    session.add_step(
        StepBuilder::new(session.step_count())
            .user_input(&assignment.description)
            .decision(AgentDecision::new("reading_input", "读取题目与提交路径"))
            .build(),
    );
    let mut tracker = UsageTracker::new();
    if let Some(budget) = options.budget {
        tracker.set_session_budget(budget);
    }
    let mut tests = options
        .tests
        .as_deref()
        .map(load_tests)
        .transpose()?
        .unwrap_or_default();
    let mut knowledge = match options.memory.as_deref() {
        Some(path) if path.exists() => pada::memory::KnowledgeProfile::load(path)?,
        _ => pada::memory::KnowledgeProfile::default(),
    };
    if options.generate_tests {
        let (_, config) = model_config.as_ref().ok_or_else(|| {
            pada::error::PadaError::Config(
                "--generate-tests 需要 --config（可配合 --profile）".into(),
            )
        })?;
        if !tracker.check_budget() {
            return Err(pada::error::PadaError::Llm(
                "Token 预算已用尽，未生成测试".into(),
            ));
        }
        step_gate(options.step, "生成边界测试")?;
        let generator = pada::tools::test_gen::TestGenerator::new(
            pada::agent::llm::LlmClient::new(config.clone()),
        );
        let profile_summary = knowledge.prompt_summary_at(pada::memory::now_timestamp());
        let response = generator.generate_raw_with_profile(&assignment, &profile_summary)?;
        tracker.record(&response, config);
        session.record_usage(pada::telemetry::UsageRecord::from_response(
            &response, config,
        ));
        tests.extend(pada::tools::test_gen::parse_test_cases(&response.content)?);
        eprintln!(
            "已生成 {} 个边界测试；{}",
            tests.len(),
            tracker.summary().lines().nth(1).unwrap_or("")
        );
    }

    loop {
        step_gate(options.step, "编译代码")?;
        let progress = CliProgress::new();
        progress.start(3, "诊断");
        progress.tick(1, "正在编译");
        let (output, binary) =
            compile_submission(options.code.as_deref(), options.project.as_deref())?;
        session.add_step(
            StepBuilder::new(session.step_count())
                .tool_call(ToolCall::new(
                    if options.code.is_some() {
                        "rustc"
                    } else {
                        "cargo check"
                    },
                    options
                        .code
                        .as_ref()
                        .or(options.project.as_ref())
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    format!("success={}", output.success),
                ))
                .build(),
        );
        step_gate(options.step, "分析诊断结果")?;
        progress.tick(2, "正在分析错误与测试");
        let (diags, classified) = compile_diagnostics(&output);
        let timestamp = pada::memory::now_timestamp();
        for point in classified
            .iter()
            .flat_map(|d| d.knowledge_points.iter())
            .copied()
        {
            knowledge.record_diagnostic(point, false, timestamp);
        }
        let mut report = build_compile_report(&diags, &classified, level);
        if output.success && !tests.is_empty() {
            if let Some(program) = binary.as_deref() {
                for result in TestRunner::new()
                    .run_tests(program, &tests)?
                    .into_iter()
                    .filter(|r| !r.passed)
                {
                    let _ = classify_test_failure(
                        &result.name,
                        &result.actual_output,
                        &result.expected_output,
                    );
                    let hint = generate_test_hint(
                        &result.name,
                        &result.actual_output,
                        &result.expected_output,
                        level,
                    );
                    report.add_test(TestReportEntry { result, hint });
                }
            }
        }
        if let Some(program) = binary {
            let _ = std::fs::remove_file(program);
        }
        progress.tick(3, "正在生成报告");
        progress.finish("诊断完成");
        output_report(&report, options.report.as_deref())?;
        session.add_step(
            StepBuilder::new(session.step_count())
                .decision(AgentDecision::new(
                    "reporting",
                    format!(
                        "生成 Level {} 提示，共 {} 个问题",
                        hint_level_as_number(level),
                        report.compile_entries.len() + report.test_entries.len()
                    ),
                ))
                .build(),
        );
        maybe_save(&session, options.save.as_deref());
        if let Some(path) = options.memory.as_deref() {
            knowledge.save(path)?;
        }
        if !options.interactive {
            break;
        }
        println!("\n导师模式（{}）", help_text());
        match interaction(
            &mut level,
            &diags,
            &classified,
            &mut session,
            &tracker,
            options.save.as_deref(),
            &mut knowledge,
            options.memory.as_deref(),
        )? {
            LoopAction::Recheck => continue,
            LoopAction::Exit => break,
        }
    }
    Ok(())
}

fn compile_submission(
    code: Option<&Path>,
    project: Option<&Path>,
) -> pada::error::Result<(CompileOutput, Option<PathBuf>)> {
    let compiler = CompilerTool::new();
    if let Some(source) = code {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let binary = std::env::temp_dir().join(format!("pada-{}-{stem}", std::process::id()));
        Ok((compiler.compile_file(source, Some(&binary))?, Some(binary)))
    } else if let Some(dir) = project {
        Ok((compiler.cargo_check(dir)?, None))
    } else {
        unreachable!()
    }
}

fn compile_diagnostics(output: &CompileOutput) -> (Vec<RustcDiagnostic>, Vec<Diagnostic>) {
    if output.success {
        return (Vec::new(), Vec::new());
    }
    let diags = parse_diagnostics(&output.stderr);
    let classified = classify_compile_diagnostics(&diags);
    (diags, classified)
}

fn build_compile_report(
    diags: &[RustcDiagnostic],
    classified: &[Diagnostic],
    level: HintLevel,
) -> DiagnosticReport {
    let mut report = DiagnosticReport::new();
    for (diag, class) in diags
        .iter()
        .filter(|d| d.severity != Severity::Warning)
        .zip(classified)
    {
        report.add_compile(CompileReportEntry {
            diag: diag.clone(),
            classified: class.clone(),
            hint: generate_compile_hint(diag, class, level),
        });
    }
    report
}

enum LoopAction {
    Recheck,
    Exit,
}

fn interaction(
    level: &mut HintLevel,
    diags: &[RustcDiagnostic],
    classified: &[Diagnostic],
    session: &mut Session,
    tracker: &UsageTracker,
    default_save: Option<&Path>,
    knowledge: &mut pada::memory::KnowledgeProfile,
    memory_path: Option<&Path>,
) -> pada::error::Result<LoopAction> {
    loop {
        print!("pada[{}]> ", hint_level_as_number(*level));
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            return Ok(LoopAction::Exit);
        }
        match parse_command(&line) {
            InteractiveCommand::Next => {
                if let Some(next) = next_hint_level(*level) {
                    *level = next;
                } else {
                    println!("已经是最高提示等级。");
                }
                print!(
                    "{}",
                    build_compile_report(diags, classified, *level).to_text()
                );
            }
            InteractiveCommand::Hint(Some(n)) => match hint_level_from_number(n) {
                Some(next) => {
                    *level = next;
                    print!(
                        "{}",
                        build_compile_report(diags, classified, *level).to_text()
                    );
                }
                None => println!("提示等级必须是 1 到 5。"),
            },
            InteractiveCommand::Hint(None) => {
                println!("当前提示等级: {}", hint_level_as_number(*level))
            }
            InteractiveCommand::Show => print!(
                "{}",
                build_compile_report(diags, classified, *level).to_text()
            ),
            InteractiveCommand::Recheck => return Ok(LoopAction::Recheck),
            InteractiveCommand::Usage => println!("{}", tracker.summary()),
            InteractiveCommand::Progress => {
                print!("{}", knowledge.summary_at(pada::memory::now_timestamp()))
            }
            InteractiveCommand::Feedback(understood) => {
                let timestamp = pada::memory::now_timestamp();
                let points: std::collections::HashSet<_> = classified
                    .iter()
                    .flat_map(|d| d.knowledge_points.iter())
                    .copied()
                    .collect();
                if points.is_empty() {
                    println!("当前诊断没有可映射的知识点。");
                } else {
                    for point in points {
                        knowledge.record_feedback(point, understood, timestamp);
                    }
                    if let Some(path) = memory_path {
                        knowledge.save(path)?;
                    }
                    println!("已记录反馈。\n{}", knowledge.summary_at(timestamp));
                }
            }
            InteractiveCommand::Save(path) => {
                let target = path.as_deref().map(Path::new).or(default_save);
                match target {
                    Some(p) => {
                        session.save(p)?;
                        println!("会话已保存: {}", p.display());
                    }
                    None => println!("请使用 save <文件> 指定路径。"),
                }
            }
            InteractiveCommand::Help => println!("{}", help_text()),
            InteractiveCommand::Exit => return Ok(LoopAction::Exit),
            InteractiveCommand::Unknown(value) => {
                println!("未知命令「{value}」。输入 help 查看命令。")
            }
        }
    }
}

fn step_gate(enabled: bool, stage: &str) -> pada::error::Result<()> {
    if !enabled || !io::stdin().is_terminal() {
        return Ok(());
    }
    print!("\n下一步：{stage}。按 Enter 继续，输入 q 退出: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim().eq_ignore_ascii_case("q") {
        return Err(pada::error::PadaError::Run("用户取消逐步诊断".into()));
    }
    Ok(())
}

fn load_tests(path: &Path) -> pada::error::Result<Vec<TestCase>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| pada::error::PadaError::FileNotFound(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| pada::error::PadaError::Parse(format!("测试文件格式错误: {e}")))
}

fn load_profile(
    config_path: Option<&Path>,
    requested: Option<&str>,
) -> pada::error::Result<Option<(String, pada::config::model::ModelConfig)>> {
    let Some(path) = config_path else {
        if requested.is_some() {
            return Err(pada::error::PadaError::Config(
                "使用 --profile 时还需通过 --config 指定配置文件".into(),
            ));
        }
        return Ok(None);
    };
    let config = pada::config::model::Config::load(path)?;
    let name = requested.unwrap_or(&config.active_profile);
    let model = config
        .profiles
        .get(name)
        .ok_or_else(|| pada::error::PadaError::Config(format!("profile 不存在: {name}")))?;
    Ok(Some((name.to_owned(), model.clone())))
}

fn output_report(report: &DiagnosticReport, path: Option<&Path>) -> pada::error::Result<()> {
    print!("{}", report.to_text());
    if let Some(path) = path {
        std::fs::write(path, report.to_markdown())
            .map_err(|e| pada::error::PadaError::Config(format!("写入报告失败: {e}")))?;
        eprintln!("诊断报告已导出: {}", path.display());
    }
    Ok(())
}

fn maybe_save(session: &Session, path: Option<&Path>) {
    if let Some(path) = path {
        match session.save(path) {
            Ok(()) => eprintln!("会话已保存: {}", path.display()),
            Err(e) => eprintln!("保存会话失败: {e}"),
        }
    }
}
