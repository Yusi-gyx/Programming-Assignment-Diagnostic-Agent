//! PADA CLI：参数解析和交互入口；诊断逻辑由库模块完成。
use clap::{ArgGroup, Parser, Subcommand};
use pada::agent::interaction::{InteractiveCommand, help_text, parse_command};
use pada::agent::progress::{CliProgress, ProgressReporter};
use pada::agent::solution::SolutionHintService;
use pada::analysis::classifier::{classify_compile_diagnostics, classify_test_failure};
use pada::analysis::error_parser::{RustcDiagnostic, Severity, parse_diagnostics};
use pada::analysis::hint::{
    generate_compile_hint, generate_test_hint, hint_level_as_number, hint_level_from_number,
    next_hint_level,
};
use pada::history::{AgentDecision, Session, SessionContext, StepBuilder, ToolCall};
use pada::models::{Assignment, Diagnostic, HintLevel};
use pada::report::{CompileReportEntry, DiagnosticReport, TestReportEntry};
use pada::storage::{DataStore, StoredSession};
use pada::telemetry::UsageTracker;
use pada::tools::compiler::{CompileOutput, CompilerTool};
use pada::tools::runner::{TestCase, TestRunner};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "pada", version, about = "Rust 编程作业诊断 Agent")]
struct Cli {
    /// 用户数据根目录（默认 ~/.pada，也可通过 PADA_HOME 设置）
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
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
    /// 列出最近自动保存的会话并继续其中一个
    Resume {
        /// 会话序号（列表中的 1-20）或会话 ID；省略时在终端中交互选择
        session: Option<String>,
        /// 即使在终端中也执行一次后退出
        #[arg(long)]
        no_interactive: bool,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        if io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
            eprintln!("\x1b[31;1m操作失败: {error}\x1b[0m");
        } else {
            eprintln!("操作失败: {error}");
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> pada::error::Result<()> {
    let store = DataStore::discover(cli.data_dir)?;
    match cli.command {
        Commands::Diagnose {
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
        } => run_diagnose(
            DiagnoseOptions {
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
            },
            &store,
        ),
        Commands::Resume {
            session,
            no_interactive,
        } => run_resume(&store, session.as_deref(), no_interactive),
    }
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

fn run_diagnose(mut options: DiagnoseOptions, store: &DataStore) -> pada::error::Result<()> {
    options.problem = absolute_path(&options.problem)?;
    options.code = options.code.as_deref().map(absolute_path).transpose()?;
    options.project = options.project.as_deref().map(absolute_path).transpose()?;
    options.tests = options.tests.as_deref().map(absolute_path).transpose()?;
    options.config = options.config.as_deref().map(absolute_path).transpose()?;
    options.memory = options.memory.as_deref().map(absolute_path).transpose()?;
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
    session.set_context(SessionContext {
        problem: options.problem.clone(),
        code: options.code.clone(),
        project: options.project.clone(),
        tests: options.tests.clone(),
        config: options.config.clone(),
        profile: options.profile.clone(),
        memory: options.memory.clone(),
        hint: options.hint,
        budget: options.budget,
        generate_tests: options.generate_tests,
    });
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
    let memory_path = options
        .memory
        .clone()
        .unwrap_or_else(|| store.learning_profile_path());
    let mut knowledge = match Some(memory_path.as_path()) {
        Some(path) if path.exists() => pada::memory::KnowledgeProfile::load(path)?,
        _ => pada::memory::KnowledgeProfile::default(),
    };
    eprintln!(
        "学习画像: {}（自动记录练习证据；在导师模式输入 progress 查看用途与掌握度）",
        memory_path.display()
    );
    let mut solution_hints =
        SolutionHintService::new(model_config.as_ref().map(|(_, config)| config.clone()));
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
        let source_context =
            submission_context(options.code.as_deref(), options.project.as_deref())?;
        let submission_key = stable_evidence_key(&source_context);
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
            knowledge.record_diagnostic_once(
                point,
                false,
                format!("{submission_key}:{point:?}:failed"),
                timestamp,
            );
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
        solution_hints.enrich(
            &mut report,
            &assignment,
            &source_context,
            &knowledge,
            &mut tracker,
            &mut session,
        );
        progress.finish("诊断完成");
        output_report(&report, options.report.as_deref(), store)?;
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
        maybe_export(&session, options.save.as_deref(), store);
        knowledge.save(&memory_path)?;
        let auto_path = store.save_auto_session(&session)?;
        eprintln!("会话已自动保存: {}", auto_path.display());
        if !options.interactive {
            break;
        }
        println!("\n导师模式（{}）", help_text());
        let action = interaction(
            &mut level,
            &report,
            &mut session,
            &mut tracker,
            options.save.as_deref(),
            &mut knowledge,
            &memory_path,
            store,
            &assignment,
            &source_context,
            &mut solution_hints,
        )?;
        if let Some(context) = &mut session.context {
            context.hint = hint_level_as_number(level);
        }
        knowledge.save(&memory_path)?;
        store.save_auto_session(&session)?;
        match action {
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

#[allow(clippy::too_many_arguments)]
fn interaction(
    level: &mut HintLevel,
    report_template: &DiagnosticReport,
    session: &mut Session,
    tracker: &mut UsageTracker,
    default_save: Option<&Path>,
    knowledge: &mut pada::memory::KnowledgeProfile,
    memory_path: &Path,
    store: &DataStore,
    assignment: &Assignment,
    source_context: &str,
    solution_hints: &mut SolutionHintService,
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
                show_report_at_level(
                    report_template,
                    *level,
                    assignment,
                    source_context,
                    knowledge,
                    tracker,
                    session,
                    solution_hints,
                );
            }
            InteractiveCommand::Hint(Some(n)) => match hint_level_from_number(n) {
                Some(next) => {
                    *level = next;
                    show_report_at_level(
                        report_template,
                        *level,
                        assignment,
                        source_context,
                        knowledge,
                        tracker,
                        session,
                        solution_hints,
                    );
                }
                None => println!("提示等级必须是 1 到 5。"),
            },
            InteractiveCommand::Hint(None) => {
                println!("当前提示等级: {}", hint_level_as_number(*level))
            }
            InteractiveCommand::Show => show_report_at_level(
                report_template,
                *level,
                assignment,
                source_context,
                knowledge,
                tracker,
                session,
                solution_hints,
            ),
            InteractiveCommand::Recheck => return Ok(LoopAction::Recheck),
            InteractiveCommand::Usage => println!("{}", tracker.summary()),
            InteractiveCommand::Progress => {
                print!("{}", knowledge.summary_at(pada::memory::now_timestamp()))
            }
            InteractiveCommand::Feedback(understood) => {
                let timestamp = pada::memory::now_timestamp();
                let points: std::collections::HashSet<_> = report_template
                    .compile_entries
                    .iter()
                    .flat_map(|entry| entry.classified.knowledge_points.iter())
                    .copied()
                    .collect();
                if points.is_empty() {
                    println!("当前诊断没有可映射的知识点。");
                } else {
                    for point in points {
                        knowledge.record_feedback(point, understood, timestamp);
                    }
                    knowledge.save(memory_path)?;
                    println!("已记录反馈。\n{}", knowledge.summary_at(timestamp));
                }
            }
            InteractiveCommand::Save(path) => {
                let target = path.as_deref().map(Path::new).or(default_save);
                match target {
                    Some(p) => {
                        let saved = store.export_session(p, session)?;
                        println!("会话已导出: {}", saved.display());
                    }
                    None => println!(
                        "请使用 save <文件名> 导出；文件会统一保存到 {}。",
                        store.exported_sessions_dir().display()
                    ),
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

fn output_report(
    report: &DiagnosticReport,
    path: Option<&Path>,
    store: &DataStore,
) -> pada::error::Result<()> {
    print_report(report);
    if let Some(name) = path {
        let saved = store.save_report(name, &report.to_markdown())?;
        eprintln!("诊断报告已导出: {}", saved.display());
    }
    Ok(())
}

fn maybe_export(session: &Session, path: Option<&Path>, store: &DataStore) {
    if let Some(name) = path {
        match store.export_session(name, session) {
            Ok(path) => eprintln!("会话已导出: {}", path.display()),
            Err(e) => eprintln!("保存会话失败: {e}"),
        }
    }
}

fn print_report(report: &DiagnosticReport) {
    if io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        print!("{}", report.to_colored_text());
    } else {
        print!("{}", report.to_text());
    }
}

#[allow(clippy::too_many_arguments)]
fn show_report_at_level(
    template: &DiagnosticReport,
    level: HintLevel,
    assignment: &Assignment,
    source_context: &str,
    knowledge: &pada::memory::KnowledgeProfile,
    tracker: &mut UsageTracker,
    session: &mut Session,
    solution_hints: &mut SolutionHintService,
) {
    let mut report = template.clone();
    for entry in &mut report.compile_entries {
        entry.hint = generate_compile_hint(&entry.diag, &entry.classified, level);
    }
    for entry in &mut report.test_entries {
        entry.hint = generate_test_hint(
            &entry.result.name,
            &entry.result.actual_output,
            &entry.result.expected_output,
            level,
        );
    }
    solution_hints.enrich(
        &mut report,
        assignment,
        source_context,
        knowledge,
        tracker,
        session,
    );
    print_report(&report);
}

fn run_resume(
    store: &DataStore,
    requested: Option<&str>,
    no_interactive: bool,
) -> pada::error::Result<()> {
    let sessions = store.recent_sessions()?;
    if sessions.is_empty() {
        println!(
            "还没有可恢复的自动会话。运行一次 diagnose 后，会话将保存在 {}。",
            store.auto_sessions_dir().display()
        );
        return Ok(());
    }
    println!("最近的对话记录（最多 20 条）：");
    for (index, stored) in sessions.iter().enumerate() {
        let source = stored
            .session
            .context
            .as_ref()
            .and_then(|context| context.code.as_ref().or(context.project.as_ref()))
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "旧版记录".into());
        println!(
            "  {:>2}. {} · {} · {} · 更新于 {}",
            index + 1,
            stored.session.title,
            stored.session.id,
            source,
            pada::memory::elapsed_text(stored.session.updated_at, pada::memory::now_timestamp())
        );
    }

    let choice = match requested {
        Some(value) => value.to_owned(),
        None if io::stdin().is_terminal() => {
            print!("请选择要继续的序号: ");
            io::stdout().flush()?;
            let mut value = String::new();
            io::stdin().read_line(&mut value)?;
            value.trim().to_owned()
        }
        None => {
            println!("请再次运行 `pada resume <序号>` 继续指定会话。");
            return Ok(());
        }
    };
    let selected = select_session(&sessions, &choice)
        .ok_or_else(|| pada::error::PadaError::Parse(format!("找不到会话选择「{choice}」")))?;
    let context = selected.session.context.clone().ok_or_else(|| {
        pada::error::PadaError::Parse(
            "该记录来自旧版本，缺少恢复所需的文件路径；仍可通过 --history 手动回放".into(),
        )
    })?;
    eprintln!("正在继续会话: {}", selected.session.summary());
    run_diagnose(
        DiagnoseOptions {
            problem: context.problem,
            code: context.code,
            project: context.project,
            tests: context.tests,
            generate_tests: context.generate_tests,
            hint: context.hint,
            profile: context.profile,
            budget: context.budget,
            report: None,
            history: Some(selected.path.clone()),
            save: None,
            config: context.config,
            memory: context.memory,
            step: false,
            interactive: io::stdin().is_terminal() && !no_interactive,
        },
        store,
    )
}

fn select_session<'a>(sessions: &'a [StoredSession], choice: &str) -> Option<&'a StoredSession> {
    choice
        .parse::<usize>()
        .ok()
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| sessions.get(index))
        .or_else(|| sessions.iter().find(|item| item.session.id == choice))
}

fn absolute_path(path: &Path) -> pada::error::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn submission_context(code: Option<&Path>, project: Option<&Path>) -> pada::error::Result<String> {
    if let Some(path) = code {
        return std::fs::read_to_string(path)
            .map_err(|e| pada::error::PadaError::FileNotFound(format!("{}: {e}", path.display())));
    }
    let project = project.expect("clap 已保证 code/project 至少一个");
    let main = project.join("src/main.rs");
    let lib = project.join("src/lib.rs");
    for source in [main, lib] {
        if source.exists() {
            return std::fs::read_to_string(&source).map_err(|e| {
                pada::error::PadaError::FileNotFound(format!("{}: {e}", source.display()))
            });
        }
    }
    Ok(format!("<Cargo 项目：{}>", project.display()))
}

fn stable_evidence_key(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
