//! PADA CLI：参数解析和交互入口；诊断逻辑由库模块完成。
use clap::{ArgGroup, Parser, Subcommand};
use pada::agent::export::{available_export_target, choose_export_target};
use pada::agent::interaction::{InteractiveCommand, help_text, parse_command};
use pada::agent::llm::LlmClient;
use pada::agent::model_task::{ModelTaskOutcome, run_model_task};
use pada::agent::progress::{
    CliProgress, ProgressReporter, SilentProgress, StepChoice, StepController, parse_step_choice,
};
use pada::agent::solution::SolutionHintService;
use pada::agent::test_analysis::TestKnowledgeMapper;
use pada::analysis::classifier::{classify_compile_diagnostics, classify_test_failure};
use pada::analysis::error_parser::{RustcDiagnostic, Severity, parse_diagnostics};
use pada::analysis::hint::{
    generate_compile_hint, generate_test_hint_with_points, hint_level_as_number,
    hint_level_from_number, next_hint_level,
};
use pada::config::wizard::WizardResult;
use pada::history::{AgentDecision, LlmExchange, Session, SessionContext, StepBuilder, ToolCall};
use pada::models::{Assignment, Diagnostic, HintLevel};
use pada::report::{CompileReportEntry, DiagnosticReport, TestReportEntry};
use pada::storage::{DataStore, StoredSession};
use pada::telemetry::UsageTracker;
use pada::tools::compiler::{CompileOutput, CompilerTool};
use pada::tools::runner::{TestCase, TestRunner};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        #[arg(long, visible_alias = "test")]
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
        /// V2 学习画像文件；不存在时自动创建
        #[arg(long)]
        memory: Option<PathBuf>,
        /// 引导式逐步执行，显示阶段说明并支持连续执行或安全取消
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
                config: None,
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
    options.config = store.resolve_config_path(options.config.as_deref());
    options.memory = options.memory.as_deref().map(absolute_path).transpose()?;
    if let Some(requested) = options.save.as_deref() {
        options.save = Some(if options.interactive {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            let stdout = io::stdout();
            let mut writer = stdout.lock();
            choose_export_target(store, requested, &mut reader, &mut writer)?
        } else {
            available_export_target(store, requested)?
        });
    }
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
    let mut model_config = load_profile(options.config.as_deref(), options.profile.as_deref())?;
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
    let mut test_mapper =
        TestKnowledgeMapper::new(model_config.as_ref().map(|(_, config)| config.clone()));
    let mut stepper = StepController::new(options.step, io::stdin().is_terminal());
    if stepper.is_active() {
        println!(
            "\n┌─ 逐步执行模式 ─────────────────────────┐\n\
             │ 每一步都会说明即将执行的操作和用途。 │\n\
             │ Enter/c: 执行  a: 本轮全部执行       │\n\
             │ h: 查看说明  q: 安全取消             │\n\
             └──────────────────────────────────────┘"
        );
    } else if stepper.requested_without_terminal() {
        eprintln!("--step 需要交互式终端；当前按连续模式执行。 ");
    }
    stepper.begin_round(3 + usize::from(options.generate_tests));
    if options.generate_tests {
        let (_, config) = model_config.as_ref().ok_or_else(|| {
            pada::error::PadaError::Config(
                "--generate-tests 需要模型配置；请先在导师模式运行 config".into(),
            )
        })?;
        if !tracker.check_budget() {
            return Err(pada::error::PadaError::Llm(
                "Token 预算已用尽，未生成测试".into(),
            ));
        }
        if !step_gate(
            &mut stepper,
            "生成边界测试",
            "调用当前模型，根据题目生成额外边界输入；这一步会消耗 Token。",
        )? {
            return Ok(());
        }
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

    let mut first_round = true;
    let mut persistence_announced = false;
    loop {
        if first_round {
            first_round = false;
        } else {
            stepper.begin_round(3);
        }
        let source_context =
            submission_context(options.code.as_deref(), options.project.as_deref())?;
        let submission_key = stable_evidence_key(&source_context);
        let test_suite_key = stable_evidence_key(
            &serde_json::to_string(&tests)
                .map_err(|error| pada::error::PadaError::Parse(error.to_string()))?,
        );
        if !step_gate(
            &mut stepper,
            "编译提交",
            if options.code.is_some() {
                "使用 rustc 检查单文件，并在成功时生成临时可执行程序。"
            } else {
                "使用 cargo check 检查整个多文件项目。"
            },
        )? {
            return Ok(());
        }
        let progress: Box<dyn ProgressReporter> = if stepper.is_active() {
            Box::new(SilentProgress)
        } else {
            Box::new(CliProgress::new())
        };
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
        if stepper.is_active() {
            println!(
                "  ✓ 编译阶段完成：{}",
                if output.success {
                    "编译通过"
                } else {
                    "发现编译错误"
                }
            );
        }
        if !step_gate(
            &mut stepper,
            "分析错误并运行测试",
            if tests.is_empty() {
                "解析编译器证据并映射知识点；当前没有外部测试用例。".into()
            } else if model_config.is_some() {
                format!(
                    "解析编译器证据并运行 {} 个测试；失败用例会调用模型映射知识点并消耗 Token。",
                    tests.len()
                )
            } else {
                format!(
                    "解析编译器证据；编译成功后运行 {} 个测试，并分析失败用例。",
                    tests.len()
                )
            },
        )? {
            return Ok(());
        }
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
        if output.success
            && !tests.is_empty()
            && let Some(program) = binary.as_deref()
        {
            let failures = TestRunner::new()
                .run_tests(program, &tests)?
                .into_iter()
                .filter(|r| !r.passed)
                .collect::<Vec<_>>();
            let mapped = match test_mapper.map_failures(
                &assignment,
                &source_context,
                &failures,
                &mut tracker,
                &mut session,
            ) {
                Ok(mapped) => mapped,
                Err(error) => {
                    eprintln!("测试知识点映射失败，将保留基础分类: {error}");
                    failures
                        .iter()
                        .map(|result| {
                            classify_test_failure(
                                &result.name,
                                &result.actual_output,
                                &result.expected_output,
                            )
                        })
                        .collect()
                }
            };
            let mut mapped_points = std::collections::HashSet::new();
            for (result, classified) in failures.into_iter().zip(mapped) {
                let hint = generate_test_hint_with_points(
                    &result.name,
                    &result.actual_output,
                    &result.expected_output,
                    level,
                    &classified.knowledge_points,
                );
                mapped_points.extend(classified.knowledge_points.iter().copied());
                report.add_test(TestReportEntry {
                    result,
                    classified,
                    hint,
                });
            }
            for point in mapped_points {
                knowledge.record_diagnostic_once(
                    point,
                    false,
                    format!("{submission_key}:{test_suite_key}:test:{point:?}:failed"),
                    timestamp,
                );
            }
        }
        if let Some(program) = binary {
            let _ = std::fs::remove_file(program);
        }
        let issue_count = report.compile_entries.len() + report.test_entries.len();
        if stepper.is_active() {
            println!(
                "  ✓ 分析阶段完成：{} 个编译问题，{} 个失败用例",
                report.compile_entries.len(),
                report.test_entries.len()
            );
        }
        if !step_gate(
            &mut stepper,
            "生成诊断报告",
            if matches!(
                level,
                HintLevel::Concept | HintLevel::Direction | HintLevel::Solution
            ) && model_config.is_some()
            {
                format!(
                    "整理 {issue_count} 个问题，并调用模型增强 Level {} 提示。",
                    hint_level_as_number(level)
                )
            } else {
                format!("按当前提示等级整理 {issue_count} 个问题及相应提示。")
            },
        )? {
            return Ok(());
        }
        progress.tick(3, "正在生成报告");
        // 模型调用使用独立的静态状态标志，避免与动态进度条相互覆盖。
        progress.finish("基础诊断完成");
        solution_hints.enrich(
            &mut report,
            &assignment,
            &source_context,
            &knowledge,
            &mut tracker,
            &mut session,
            options.interactive,
        );
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
        maybe_export(
            &session,
            options.save.as_deref(),
            store,
            !persistence_announced,
        );
        knowledge.save(&memory_path)?;
        let auto_path = store.save_auto_session(&session)?;
        if !persistence_announced {
            eprintln!("会话将自动保存到: {}", auto_path.display());
            persistence_announced = true;
        }
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
            options.config.as_deref(),
            &options.problem,
            &assignment,
            &source_context,
            &mut solution_hints,
            model_config.as_ref().map(|(_, config)| config),
        )?;
        if let Some(context) = &mut session.context {
            context.hint = hint_level_as_number(level);
        }
        knowledge.save(&memory_path)?;
        store.save_auto_session(&session)?;
        match action {
            LoopAction::Recheck => continue,
            LoopAction::UseTests { path, cases } => {
                tests = cases;
                options.tests = Some(path.clone());
                if let Some(context) = &mut session.context {
                    context.tests = Some(path);
                }
                println!("测试文件已应用，正在重新诊断。");
                continue;
            }
            LoopAction::Reconfigure(configured) => {
                options.config = Some(configured.path.clone());
                options.profile = Some(configured.profile_name.clone());
                model_config = Some((configured.profile_name, configured.model.clone()));
                solution_hints = SolutionHintService::new(Some(configured.model.clone()));
                test_mapper = TestKnowledgeMapper::new(Some(configured.model));
                if let Some(context) = &mut session.context {
                    context.config = options.config.clone();
                    context.profile = options.profile.clone();
                }
                println!("模型配置已生效，正在重新诊断以更新知识点映射。");
                continue;
            }
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
    UseTests { path: PathBuf, cases: Vec<TestCase> },
    Reconfigure(WizardResult),
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
    config_path: Option<&Path>,
    problem_path: &Path,
    assignment: &Assignment,
    source_context: &str,
    solution_hints: &mut SolutionHintService,
    model_config: Option<&pada::config::model::ModelConfig>,
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
                    .chain(
                        report_template
                            .test_entries
                            .iter()
                            .flat_map(|entry| entry.classified.knowledge_points.iter()),
                    )
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
                        let chosen = {
                            let stdin = io::stdin();
                            let mut reader = stdin.lock();
                            let stdout = io::stdout();
                            let mut writer = stdout.lock();
                            choose_export_target(store, p, &mut reader, &mut writer)?
                        };
                        let saved = store.export_session(&chosen, session)?;
                        println!("会话已导出: {}", saved.display());
                    }
                    None => println!(
                        "请使用 save <文件名> 导出；文件会统一保存到 {}。",
                        store.exported_sessions_dir().display()
                    ),
                }
            }
            InteractiveCommand::Tests(path) => {
                let Some(path) = path else {
                    println!("用法: test <file.json> 或 tests <file.json>");
                    continue;
                };
                let path = match absolute_path(Path::new(&path)) {
                    Ok(path) => path,
                    Err(error) => {
                        println!("测试文件路径无效: {error}");
                        continue;
                    }
                };
                match load_tests(&path) {
                    Ok(cases) => {
                        println!("已加载 {} 个测试用例: {}", cases.len(), path.display());
                        return Ok(LoopAction::UseTests { path, cases });
                    }
                    Err(error) => println!("加载测试文件失败: {error}"),
                }
            }
            InteractiveCommand::Case => {
                if let Err(error) = generate_case_file(
                    problem_path,
                    assignment,
                    knowledge,
                    tracker,
                    session,
                    model_config,
                ) {
                    println!("生成测试用例失败: {error}");
                }
            }
            InteractiveCommand::Config => {
                let target = config_path
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| store.config_path());
                let stdin = io::stdin();
                let mut reader = stdin.lock();
                let stdout = io::stdout();
                let mut writer = stdout.lock();
                match pada::config::wizard::run_config_wizard(&mut reader, &mut writer, &target) {
                    Ok(Some(configured)) => return Ok(LoopAction::Reconfigure(configured)),
                    Ok(None) => {}
                    Err(error) => println!("配置未保存: {error}"),
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

fn generate_case_file(
    problem_path: &Path,
    assignment: &Assignment,
    knowledge: &pada::memory::KnowledgeProfile,
    tracker: &mut UsageTracker,
    session: &mut Session,
    model_config: Option<&pada::config::model::ModelConfig>,
) -> pada::error::Result<()> {
    let config = model_config.ok_or_else(|| {
        pada::error::PadaError::Config("尚未配置模型；请先在导师模式输入 config 完成配置".into())
    })?;
    if !tracker.check_budget() {
        return Err(pada::error::PadaError::Llm(
            "Token 预算已用尽，未生成测试用例".into(),
        ));
    }

    let profile_summary = knowledge.prompt_summary_at(pada::memory::now_timestamp());
    let messages = pada::tools::test_gen::build_prompt_with_profile(assignment, &profile_summary);
    let client = Arc::new(LlmClient::new(config.clone()));
    eprintln!("⏳ 正在调用模型生成测试用例…（输入 q 或 cancel 并回车可停止）");
    io::stderr().flush()?;

    let response = match run_model_task(client, &messages, true) {
        ModelTaskOutcome::Completed(Ok(response)) => response,
        ModelTaskOutcome::Completed(Err(error)) => {
            eprintln!("✗ 模型生成失败");
            return Err(error);
        }
        ModelTaskOutcome::Cancelled => {
            eprintln!("■ 已取消测试用例生成，没有写入文件。");
            return Ok(());
        }
    };
    eprintln!("✓ 模型生成完成，正在校验 JSON…");

    let usage = pada::telemetry::UsageRecord::from_response(&response, config);
    tracker.record(&response, config);
    session.record_usage(usage.clone());
    session.add_step(
        StepBuilder::new(session.step_count())
            .llm_exchange(LlmExchange {
                messages,
                response: response.clone(),
                usage: Some(usage),
            })
            .decision(AgentDecision::new(
                "test_case_generation",
                "根据当前题目与学习画像生成测试用例，并由 Rust 校验 JSON 结构",
            ))
            .build(),
    );

    let cases = pada::tools::test_gen::parse_test_cases(&response.content)?;
    let path = pada::tools::test_gen::save_generated_test_cases(problem_path, &cases)?;
    println!(
        "已生成 {} 个测试用例并保存到: {}\n输入 test {} 可立即用于当前代码。",
        cases.len(),
        path.display(),
        path.display()
    );
    Ok(())
}

fn step_gate(
    controller: &mut StepController,
    stage: &str,
    detail: impl AsRef<str>,
) -> pada::error::Result<bool> {
    let (current, total) = controller.next_position();
    if !controller.should_prompt() {
        return Ok(true);
    }
    loop {
        println!("\n[逐步模式 {current}/{total}] {stage}");
        println!("  {}", detail.as_ref());
        print!("  Enter/c 执行 · a 执行本轮剩余步骤 · h 说明 · q 取消 > ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            println!("\n输入已结束，安全取消诊断。");
            return Ok(false);
        }
        match parse_step_choice(&input) {
            StepChoice::Continue => return Ok(true),
            StepChoice::RunRemaining => {
                controller.run_remaining();
                println!("本轮剩余步骤将连续执行；下次 recheck 会恢复逐步确认。");
                return Ok(true);
            }
            StepChoice::Cancel => {
                println!("已安全取消，当前源文件不会被修改。");
                return Ok(false);
            }
            StepChoice::Help => println!(
                "  逐步模式只控制何时开始阶段，不修改诊断逻辑。可在执行前阅读说明并决定继续或取消。"
            ),
            StepChoice::Invalid => println!("  无法识别该输入，请使用 Enter、c、a、h 或 q。"),
        }
    }
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
                "找不到模型配置；请先进入导师模式运行 config".into(),
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

fn maybe_export(session: &Session, path: Option<&Path>, store: &DataStore, announce: bool) {
    if let Some(name) = path {
        match store.export_session(name, session) {
            Ok(path) if announce => eprintln!("会话已导出: {}", path.display()),
            Ok(_) => {}
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
        entry.hint = generate_test_hint_with_points(
            &entry.result.name,
            &entry.result.actual_output,
            &entry.result.expected_output,
            level,
            &entry.classified.knowledge_points,
        );
    }
    solution_hints.enrich(
        &mut report,
        assignment,
        source_context,
        knowledge,
        tracker,
        session,
        true,
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
