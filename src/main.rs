//! PADA CLI：参数解析和交互入口；诊断逻辑由库模块完成。
use clap::{ArgGroup, Parser, Subcommand};
use pada::agent::export::{available_export_target, choose_export_target};
use pada::agent::interaction::{
    InteractiveCommand, help_text, parse_command, reset_hint_for_new_tests,
};
use pada::agent::llm::{LlmClient, ModelTaskKind};
use pada::agent::model_task::{ModelTaskOutcome, run_recorded_model_task};
use pada::agent::progress::{
    ProgressReporter, SilentProgress, StepChoice, StepController, parse_step_choice,
};
use pada::agent::solution::{SolutionHintService, StreamedReportEntries};
use pada::agent::test_analysis::TestKnowledgeMapper;
use pada::analysis::classifier::{classify_compile_diagnostics, classify_test_result};
use pada::analysis::error_parser::{RustcDiagnostic, Severity, parse_diagnostics};
use pada::analysis::hint::{
    generate_compile_hint, generate_test_result_hint, hint_level_as_number, hint_level_from_number,
    next_hint_level,
};
use pada::config::effort::{EffortMode, EffortPolicy, EffortSignals, ModelCallBudget};
use pada::config::wizard::WizardResult;
use pada::history::{
    AgentDecision, LlmExchange, Session, SessionContext, SessionStep, StepBuilder, ToolCall,
};
use pada::models::{Assignment, Diagnostic, HintLevel, TestResult};
use pada::report::{CompileReportEntry, DiagnosticReport, TestReportEntry, format_test_run};
use pada::storage::{DataStore, StoredSession};
use pada::telemetry::UsageTracker;
use pada::tools::compiler::{CompileOutput, CompilerTool};
use pada::tools::runner::{TestCase, TestRunner};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Default)]
struct DiagnosticTimings {
    input_ms: u128,
    compile_ms: u128,
    analysis_ms: u128,
    verification_ms: u128,
    report_ms: u128,
}

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
        /// Agent 思考模式：auto/low/medium/high/xhigh/max
        #[arg(long, default_value_t = EffortMode::Medium)]
        effort: EffortMode,
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
        /// 覆盖历史会话保存的思考模式
        #[arg(long)]
        effort: Option<EffortMode>,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        if matches!(error, pada::error::PadaError::Cancelled) {
            eprintln!("已取消诊断，子进程已停止。可使用 pada resume 重新打开历史会话。");
            return;
        }
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
            effort,
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
                effort,
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
            effort,
        } => run_resume(&store, session.as_deref(), no_interactive, effort),
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
    effort: EffortMode,
    report: Option<PathBuf>,
    history: Option<PathBuf>,
    save: Option<PathBuf>,
    config: Option<PathBuf>,
    memory: Option<PathBuf>,
    step: bool,
    interactive: bool,
}

fn run_diagnose(mut options: DiagnoseOptions, store: &DataStore) -> pada::error::Result<()> {
    let reading_started = std::time::Instant::now();
    if options.history.is_some() {
        options.hint = 1;
        options.generate_tests = false;
        eprintln!("恢复会话从 hint 1 开始；需要模型提示时请主动输入 hint 3/4/5 或 case。");
    }
    if io::stdin().is_terminal() {
        eprintln!("诊断期间输入 q / cancel / exit 并回车可停止当前任务。");
    }
    options.problem = absolute_path(&options.problem)?;
    options.code = options.code.as_deref().map(absolute_path).transpose()?;
    options.project = options.project.as_deref().map(absolute_path).transpose()?;
    options.tests = options.tests.as_deref().map(absolute_path).transpose()?;
    options.config = options.config.as_deref().map(absolute_path).transpose()?;
    options.config = store.resolve_config_path(options.config.as_deref());
    options.memory = options.memory.as_deref().map(absolute_path).transpose()?;
    if options.project.is_some() && (options.tests.is_some() || options.generate_tests) {
        return Err(pada::error::PadaError::Config(
            "Cargo 项目模式暂不支持 stdin/stdout JSON 测试；请移除 --tests/--generate-tests，或使用 --code 诊断单文件".into(),
        ));
    }
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
    let mut assignment = Assignment {
        title: options
            .problem
            .file_stem()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名题目".into()),
        description,
    };
    let mut level = hint_level_from_number(options.hint).expect("clap 已校验提示等级");
    let mut requested_effort = options.effort;
    let mut effective_policy = requested_effort.initial_policy();
    eprintln!("思考模式：{}", effective_policy.summary());
    if options.generate_tests && !effective_policy.run_tests {
        return Err(pada::error::PadaError::Config(
            "--generate-tests 在 low 模式下不会执行生成的测试；请使用 --effort medium 或更高模式"
                .into(),
        ));
    }
    let mut model_config = load_profile(options.config.as_deref(), options.profile.as_deref())?;
    if let Some((name, config)) = &model_config {
        eprintln!("模型配置: {name}");
        eprintln!(
            "Reasoning={}: {}",
            config.reasoning,
            config.reasoning_notice()
        );
    }
    let mut session = match options.history.as_deref() {
        Some(path) => {
            let value = Session::load(path)?;
            eprintln!(
                "已加载历史会话：{}（题目：{}）",
                value.display_title(),
                value.problem_path_text()
            );
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
        effort: requested_effort,
    });
    session.add_step(
        StepBuilder::new(session.step_count())
            .user_input(&assignment.description)
            .decision(AgentDecision::new("reading_input", "读取题目与提交路径"))
            .build(),
    );
    let mut tracker = UsageTracker::new();
    assignment.description = pada::agent::context::compact_rules(&assignment.description);
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
    let mut solution_hints = SolutionHintService::with_effort(
        model_config.as_ref().map(|(_, config)| config.clone()),
        effective_policy,
    );
    let mut test_mapper = TestKnowledgeMapper::with_effort(
        model_config.as_ref().map(|(_, config)| config.clone()),
        effective_policy,
    );
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
    // Keep a resumable checkpoint even if compilation or generation is cancelled.
    store.save_auto_session(&session)?;
    let input_ms = reading_started.elapsed().as_millis();
    let mut llm_step_start = session.step_count();
    let mut carried_model_calls = 0;
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
        let profile_summary = knowledge.prompt_summary_at(pada::memory::now_timestamp());
        let messages =
            pada::tools::test_gen::build_prompt_with_profile(&assignment, &profile_summary);
        let outcome = run_recorded_model_task(
            Arc::new(LlmClient::with_effort(config.clone(), effective_policy)),
            &messages,
            io::stdin().is_terminal(),
            ModelTaskKind::TestGeneration,
            &mut session,
            |_| {},
        );
        store.save_auto_session(&session)?;
        let response = match outcome {
            ModelTaskOutcome::Completed(result) => result?,
            ModelTaskOutcome::Cancelled => return Err(pada::error::PadaError::Cancelled),
        };
        tracker.record(&response, config);
        session.record_usage(pada::telemetry::UsageRecord::from_response(
            &response, config,
        ));
        session.add_step(
            StepBuilder::new(session.step_count())
                .llm_exchange(LlmExchange {
                    messages,
                    response: response.clone(),
                    usage: Some(pada::telemetry::UsageRecord::from_response(
                        &response, config,
                    )),
                })
                .decision(AgentDecision::new(
                    "test_case_generation",
                    "生成边界测试并记录模型耗时与用量",
                ))
                .build(),
        );
        store.save_auto_session(&session)?;
        response.ensure_complete()?;
        tests.extend(pada::tools::test_gen::parse_test_cases(&response.content)?);
        carried_model_calls = 1;
        eprintln!("已生成 {} 个边界测试。", tests.len());
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
        let discovery_policy = requested_effort.initial_policy();
        let mut timings = DiagnosticTimings {
            input_ms,
            ..Default::default()
        };
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
        // Persistent stage lines do not overwrite streamed model output or timing logs.
        let progress: Box<dyn ProgressReporter> = Box::new(SilentProgress);
        eprintln!(
            "{}",
            stage_line(1, "编译检查", "输入 q / cancel / exit 可停止")
        );
        progress.start(3, "诊断");
        progress.tick(1, "正在编译");
        let compile_started = std::time::Instant::now();
        let (output, binary) =
            compile_submission(options.code.as_deref(), options.project.as_deref())?;
        timings.compile_ms = compile_started.elapsed().as_millis();
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
                    format!(
                        "success={}, elapsed_ms={}",
                        output.success, timings.compile_ms
                    ),
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
            } else if !discovery_policy.run_tests {
                format!(
                    "思考模式 {} 将跳过 {} 个外部测试，只分析编译器证据。",
                    requested_effort,
                    tests.len()
                )
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
        eprintln!("{}", stage_line(2, "分析错误与测试", "正在收集确定性证据"));
        let analysis_started = std::time::Instant::now();
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
        let mut test_results = None;
        if output.success && !tests.is_empty() && !discovery_policy.run_tests {
            eprintln!(
                "思考模式 {} 已跳过 {} 个测试；使用 effort medium 或更高模式可执行测试。",
                requested_effort,
                tests.len()
            );
        } else if output.success
            && !tests.is_empty()
            && let Some(program) = binary.as_deref()
        {
            let results = TestRunner::with_runner(
                pada::tools::runner::Runner::new().with_interactive(io::stdin().is_terminal()),
            )
            .run_tests(program, &tests)?;
            let passed = results.iter().filter(|result| result.passed).count();
            report.set_test_run(results.len(), passed);
            print!(
                "{}",
                format_test_run(
                    &results,
                    io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
                )
            );
            test_results = Some(results);
        }

        let failed_tests = test_results
            .as_ref()
            .map(|results| results.iter().filter(|result| !result.passed).count())
            .unwrap_or(0);
        let has_runtime_error = test_results
            .as_ref()
            .is_some_and(|results| results.iter().any(|result| result.runtime_error.is_some()));
        let resolved_policy = requested_effort.resolve(EffortSignals {
            error_count: diags.len(),
            file_count: pada::agent::context::source_file_count(&source_context),
            failed_tests,
            has_runtime_error,
            source_bytes: source_context.len(),
        });
        if requested_effort == EffortMode::Auto || resolved_policy != effective_policy {
            eprintln!(
                "思考模式解析：{} → {}",
                requested_effort,
                resolved_policy.summary()
            );
        }
        if resolved_policy != effective_policy {
            effective_policy = resolved_policy;
            solution_hints = SolutionHintService::with_effort(
                model_config.as_ref().map(|(_, config)| config.clone()),
                effective_policy,
            );
            test_mapper = TestKnowledgeMapper::with_effort(
                model_config.as_ref().map(|(_, config)| config.clone()),
                effective_policy,
            );
        }
        let mut model_call_budget = ModelCallBudget::new(effective_policy);
        for _ in 0..std::mem::take(&mut carried_model_calls) {
            let _ = model_call_budget.try_take();
        }

        if let Some(results) = test_results.as_ref() {
            let failures = results
                .iter()
                .filter(|result| !result.passed)
                .cloned()
                .collect::<Vec<_>>();
            let mapping = if matches!(level, HintLevel::Category | HintLevel::Location) {
                Ok(failures.iter().map(classify_test_result).collect())
            } else {
                test_mapper.map_failures_with_budget(
                    &assignment,
                    &source_context,
                    &failures,
                    &mut tracker,
                    &mut session,
                    &mut model_call_budget,
                )
            };
            let mapped = match mapping {
                Ok(mapped) => mapped,
                Err(pada::error::PadaError::Cancelled) => {
                    store.save_auto_session(&session)?;
                    return Err(pada::error::PadaError::Cancelled);
                }
                Err(error) => {
                    eprintln!("测试知识点映射失败，将保留基础分类: {error}");
                    failures.iter().map(classify_test_result).collect()
                }
            };
            let mut mapped_points = std::collections::HashSet::new();
            for (result, classified) in failures.into_iter().zip(mapped) {
                let hint = generate_test_result_hint(&result, &classified, level);
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
        timings.analysis_ms = analysis_started.elapsed().as_millis();
        let verification_started = Instant::now();
        run_secondary_verification(
            &options,
            &tests,
            &output,
            test_results.as_deref(),
            effective_policy,
            &mut session,
        )?;
        timings.verification_ms = verification_started.elapsed().as_millis();
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
        eprintln!("{}", stage_line(3, "生成诊断报告", "正在整理最终输出"));
        // 模型调用使用独立的静态状态标志，避免与动态进度条相互覆盖。
        progress.finish("基础诊断完成");
        print_report_heading(level);
        let streamed = solution_hints.enrich_with_budget(
            &mut report,
            &assignment,
            &source_context,
            &knowledge,
            &mut tracker,
            &mut session,
            io::stdin().is_terminal(),
            &mut model_call_budget,
        );
        let report_started = Instant::now();
        output_report(&report, options.report.as_deref(), store, &streamed)?;
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        timings.report_ms = report_started.elapsed().as_millis();
        session.add_step(
            StepBuilder::new(session.step_count())
                .decision(AgentDecision::new(
                    "reporting",
                    format!(
                        "使用 {} 思考策略生成 Level {} 提示，共 {} 个问题",
                        effective_policy.mode,
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
        print_diagnostic_statistics(&timings, &session.steps[llm_step_start..]);
        if !persistence_announced {
            eprintln!("会话已自动保存：{}", auto_path.display());
            persistence_announced = true;
        }
        if !options.interactive {
            break;
        }
        println!("\n导师模式（{}）", help_text());
        let action = interaction(
            &mut level,
            &mut report,
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
            &test_mapper,
            model_config.as_ref().map(|(_, config)| config),
            options.code.is_some(),
            &mut requested_effort,
            effective_policy,
        );
        store.save_auto_session(&session)?;
        let action = action?;
        // Interactive hint/case calls print their own trailing statistics.
        llm_step_start = session.step_count();
        if let Some(context) = &mut session.context {
            context.hint = hint_level_as_number(level);
        }
        knowledge.save(&memory_path)?;
        store.save_auto_session(&session)?;
        match action {
            LoopAction::Recheck => continue,
            LoopAction::UseTests { path, cases } => {
                reset_hint_for_new_tests(&mut level);
                tests = cases;
                options.tests = Some(path.clone());
                if let Some(context) = &mut session.context {
                    context.tests = Some(path);
                    context.hint = 1;
                }
                store.save_auto_session(&session)?;
                println!("测试文件已应用；将从 Hint 1 输出本轮测试结果和诊断。");
                continue;
            }
            LoopAction::Reconfigure(configured) => {
                options.config = Some(configured.path.clone());
                options.profile = Some(configured.profile_name.clone());
                model_config = Some((configured.profile_name, configured.model.clone()));
                solution_hints = SolutionHintService::with_effort(
                    Some(configured.model.clone()),
                    effective_policy,
                );
                test_mapper =
                    TestKnowledgeMapper::with_effort(Some(configured.model), effective_policy);
                if let Some(context) = &mut session.context {
                    context.config = options.config.clone();
                    context.profile = options.profile.clone();
                }
                store.save_auto_session(&session)?;
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
    let compiler = CompilerTool::new().with_interactive(io::stdin().is_terminal());
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

fn run_secondary_verification(
    options: &DiagnoseOptions,
    tests: &[TestCase],
    baseline_compile: &CompileOutput,
    baseline_tests: Option<&[TestResult]>,
    policy: EffortPolicy,
    session: &mut Session,
) -> pada::error::Result<()> {
    for pass in 1..=policy.verification_passes {
        eprintln!("正在进行二次验证 {pass}/{}…", policy.verification_passes);
        let (verified_compile, binary) =
            compile_submission(options.code.as_deref(), options.project.as_deref())?;
        let compile_consistent = compile_result_signature(&verified_compile)
            == compile_result_signature(baseline_compile);
        let mut tests_consistent = true;
        if verified_compile.success
            && let (Some(program), Some(expected)) = (binary.as_deref(), baseline_tests)
        {
            let actual = TestRunner::with_runner(
                pada::tools::runner::Runner::new().with_interactive(io::stdin().is_terminal()),
            )
            .run_tests(program, tests)?;
            tests_consistent = test_result_signature(&actual) == test_result_signature(expected);
        }
        if let Some(program) = binary {
            let _ = std::fs::remove_file(program);
        }
        let consistent = compile_consistent && tests_consistent;
        session.add_step(
            StepBuilder::new(session.step_count())
                .tool_call(ToolCall::new(
                    "secondary_verification",
                    format!("effort={}, pass={pass}", policy.mode),
                    format!(
                        "compile_consistent={compile_consistent}, tests_consistent={tests_consistent}"
                    ),
                ))
                .decision(AgentDecision::new(
                    "verification",
                    if consistent {
                        "重复编译与测试结果一致"
                    } else {
                        "重复执行结果不一致，提示用户检查非确定性行为"
                    },
                ))
                .build(),
        );
        if !consistent {
            eprintln!("⚠ 二次验证结果不一致；程序可能依赖时间、随机数或外部状态。");
            break;
        }
    }
    Ok(())
}

fn compile_result_signature(output: &CompileOutput) -> Vec<String> {
    if output.success {
        return vec!["success".into()];
    }
    parse_diagnostics(&output.stderr)
        .into_iter()
        .map(|diagnostic| {
            format!(
                "{}:{}:{:?}",
                diagnostic.code.as_deref().unwrap_or(""),
                diagnostic.message,
                diagnostic.location
            )
        })
        .collect()
}

fn test_result_signature(results: &[TestResult]) -> Vec<(&str, bool, &str, &str, Option<&str>)> {
    results
        .iter()
        .map(|result| {
            (
                result.name.as_str(),
                result.passed,
                result.actual_output.as_str(),
                result.expected_output.as_str(),
                result.runtime_error.as_deref(),
            )
        })
        .collect()
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
    report_template: &mut DiagnosticReport,
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
    test_mapper: &TestKnowledgeMapper,
    model_config: Option<&pada::config::model::ModelConfig>,
    supports_external_tests: bool,
    requested_effort: &mut EffortMode,
    current_policy: EffortPolicy,
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
                    test_mapper,
                    current_policy,
                )?;
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
                        test_mapper,
                        current_policy,
                    )?;
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
                test_mapper,
                current_policy,
            )?,
            InteractiveCommand::Recheck => return Ok(LoopAction::Recheck),
            InteractiveCommand::Usage => println!("{}", tracker.summary()),
            InteractiveCommand::Effort(value) => match value {
                None => println!(
                    "当前会话模式：{}\n本轮生效策略：{}",
                    requested_effort,
                    current_policy.summary()
                ),
                Some(value) => match value.parse::<EffortMode>() {
                    Ok(mode) => {
                        *requested_effort = mode;
                        if let Some(context) = &mut session.context {
                            context.effort = mode;
                        }
                        store.save_auto_session(session)?;
                        println!(
                            "思考模式已切换为 {mode}；将在下一次诊断时生效，当前报告不会重新输出。"
                        );
                    }
                    Err(error) => println!("{error}"),
                },
            },
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
                if !supports_external_tests {
                    println!("Cargo 项目模式暂不支持 stdin/stdout JSON 测试；当前未执行任何测试。");
                    continue;
                }
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
                if !supports_external_tests {
                    println!("Cargo 项目模式暂不支持生成并执行 stdin/stdout JSON 测试用例。");
                    continue;
                }
                if let Err(error) = generate_case_file(
                    problem_path,
                    assignment,
                    knowledge,
                    tracker,
                    session,
                    model_config,
                    current_policy,
                ) {
                    println!("生成测试用例失败: {error}");
                }
                store.save_auto_session(session)?;
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
        store.save_auto_session(session)?;
    }
}

fn generate_case_file(
    problem_path: &Path,
    assignment: &Assignment,
    knowledge: &pada::memory::KnowledgeProfile,
    tracker: &mut UsageTracker,
    session: &mut Session,
    model_config: Option<&pada::config::model::ModelConfig>,
    policy: EffortPolicy,
) -> pada::error::Result<()> {
    let llm_step_start = session.step_count();
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
    let client = Arc::new(LlmClient::with_effort(config.clone(), policy));
    eprintln!("⏳ 正在调用模型生成测试用例…（输入 q 或 cancel 并回车可停止）");
    io::stderr().flush()?;

    let response = match run_recorded_model_task(
        client,
        &messages,
        io::stdin().is_terminal(),
        ModelTaskKind::TestGeneration,
        session,
        |_| {},
    ) {
        ModelTaskOutcome::Completed(Ok(response)) => response,
        ModelTaskOutcome::Completed(Err(error)) => {
            eprintln!("✗ 模型生成失败");
            session.add_step(
                StepBuilder::new(session.step_count())
                    .tool_call(ToolCall::new(
                        "test_case_generation",
                        "stream=true",
                        error.to_string(),
                    ))
                    .decision(AgentDecision::new(
                        "test_case_generation_failed",
                        "模型请求或响应读取失败，尚未进入 JSON 用例校验",
                    ))
                    .build(),
            );
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

    print_model_statistics(&session.steps[llm_step_start..]);
    response.ensure_complete()?;
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
    streamed: &StreamedReportEntries,
) -> pada::error::Result<()> {
    print_report_excluding(report, streamed);
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

fn print_report_excluding(report: &DiagnosticReport, streamed: &StreamedReportEntries) {
    if io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        print!(
            "{}",
            report.to_colored_text_excluding(&streamed.compile, &streamed.tests)
        );
    } else {
        print!(
            "{}",
            report.to_text_excluding(&streamed.compile, &streamed.tests)
        );
    }
}

fn print_report_heading(level: HintLevel) {
    let colored = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    println!(
        "\n━━ {} · {} ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
        style("诊断结果", "1;36", colored),
        style(
            &format!("Hint {}", hint_level_as_number(level)),
            "1;33",
            colored
        )
    );
}

fn print_diagnostic_statistics(timings: &DiagnosticTimings, steps: &[SessionStep]) {
    let exchanges = steps
        .iter()
        .filter_map(|step| step.llm_exchange.as_ref())
        .collect::<Vec<_>>();
    let input_tokens: usize = exchanges
        .iter()
        .map(|exchange| exchange.response.input_tokens)
        .sum();
    let output_tokens: usize = exchanges
        .iter()
        .map(|exchange| exchange.response.output_tokens)
        .sum();
    let total_cost: f64 = exchanges
        .iter()
        .filter_map(|exchange| exchange.usage.as_ref())
        .map(|usage| usage.cost)
        .fold(0.0, |total, cost| total + cost);
    let colored = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let title = style("本轮诊断统计", "1;36", colored);
    println!("\n┌─ {title} ─────────────────────────────");
    println!("│ {:<16} {:>10}", "读取输入", format_ms(timings.input_ms));
    println!("│ {:<16} {:>10}", "编译检查", format_ms(timings.compile_ms));
    println!(
        "│ {:<16} {:>10}",
        "分析与测试",
        format_ms(timings.analysis_ms)
    );
    println!(
        "│ {:<16} {:>10}",
        "二次验证",
        format_ms(timings.verification_ms)
    );
    println!("│ {:<16} {:>10}", "报告渲染", format_ms(timings.report_ms));
    for (index, exchange) in exchanges.iter().enumerate() {
        let timings = &exchange.response.timings;
        println!("│");
        println!(
            "│ {} #{}  {}",
            style("模型调用", "1;35", colored),
            index + 1,
            if exchange.response.model.is_empty() {
                "未知模型"
            } else {
                &exchange.response.model
            }
        );
        println!(
            "│   {:<14} {:>10}",
            "Prompt 构建",
            format_ms(u128::from(timings.prompt_build_ms))
        );
        println!(
            "│   {:<14} {:>10}",
            "API TTFT",
            timings
                .api_ttft_ms
                .map(|ms| format_ms(u128::from(ms)))
                .unwrap_or_else(|| "未返回".into())
        );
        println!(
            "│   {:<14} {:>10}",
            "LLM 总耗时",
            format_ms(u128::from(timings.total_ms))
        );
        println!(
            "│   {:<14} {:>10}",
            "Input Token", exchange.response.input_tokens
        );
        println!(
            "│   {:<14} {:>10}",
            "Output Token", exchange.response.output_tokens
        );
        print_response_details(&exchange.response);
    }
    print_failed_model_statistics(steps);
    println!("│");
    println!(
        "│ {}  输入 {} / 输出 {} / 合计 {} / 成本 {:.6}",
        style("Token 合计", "1;33", colored),
        input_tokens,
        output_tokens,
        input_tokens + output_tokens,
        total_cost
    );
    println!("└──────────────────────────────────────");
}

fn print_model_statistics(steps: &[SessionStep]) {
    let exchanges = steps
        .iter()
        .filter_map(|step| step.llm_exchange.as_ref())
        .collect::<Vec<_>>();
    if exchanges.is_empty()
        && !steps.iter().any(|step| {
            step.tool_calls
                .iter()
                .any(|call| call.tool == "llm_failed_call")
        })
    {
        return;
    }
    let colored = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    println!(
        "\n┌─ {} ─────────────────────────────",
        style("本次模型调用统计", "1;36", colored)
    );
    let mut input = 0;
    let mut output = 0;
    let mut cost = 0.0;
    for (index, exchange) in exchanges.iter().enumerate() {
        input += exchange.response.input_tokens;
        output += exchange.response.output_tokens;
        cost += exchange
            .usage
            .as_ref()
            .map(|usage| usage.cost)
            .unwrap_or(0.0);
        let timings = &exchange.response.timings;
        println!(
            "│ {} #{}  Prompt {} / TTFT {} / 总计 {}",
            style("调用", "1;35", colored),
            index + 1,
            format_ms(u128::from(timings.prompt_build_ms)),
            timings
                .api_ttft_ms
                .map(|ms| format_ms(u128::from(ms)))
                .unwrap_or_else(|| "未返回".into()),
            format_ms(u128::from(timings.total_ms))
        );
        println!(
            "│          Input {} / Output {} Token",
            exchange.response.input_tokens, exchange.response.output_tokens
        );
        print_response_details(&exchange.response);
    }
    print_failed_model_statistics(steps);
    println!(
        "│ {}  输入 {input} / 输出 {output} / 合计 {} / 成本 {:.6}",
        style("Token 合计", "1;33", colored),
        input + output,
        cost
    );
    println!("└──────────────────────────────────────");
}

fn format_ms(ms: u128) -> String {
    if ms >= 1_000 {
        format!("{:.2} s", ms as f64 / 1_000.0)
    } else {
        format!("{ms} ms")
    }
}

fn print_response_details(response: &pada::agent::llm::LlmResponse) {
    let time = |value: Option<u64>| {
        value
            .map(|ms| format_ms(u128::from(ms)))
            .unwrap_or_else(|| "未记录".into())
    };
    println!(
        "│   首个响应事件 {} / 首个推理块 {}",
        time(response.timings.api_first_event_ms),
        time(response.timings.api_first_reasoning_ms)
    );
    println!(
        "│   推理 Token {}（属于 Output，不重复计费）/ 结束原因 {}",
        response
            .details
            .reasoning_tokens
            .map(|n| n.to_string())
            .unwrap_or_else(|| "API 未提供".into()),
        response
            .details
            .finish_reason
            .as_deref()
            .unwrap_or("未提供")
    );
    if response.timings.json_fallback {
        println!("│   服务返回完整 JSON，首段时间包含完整生成等待。");
    }
}

fn print_failed_model_statistics(steps: &[SessionStep]) {
    for call in steps
        .iter()
        .flat_map(|step| &step.tool_calls)
        .filter(|call| call.tool == "llm_failed_call")
    {
        if let Ok(failure) =
            serde_json::from_str::<pada::agent::model_task::FailedModelCall>(&call.output)
        {
            println!(
                "│   模型{} / 耗时 {} / API 用量未知",
                if failure.cancelled {
                    "已取消"
                } else {
                    "失败"
                },
                format_ms(u128::from(failure.total_ms))
            );
        }
    }
}

fn style(text: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn stage_line(index: usize, title: &str, detail: &str) -> String {
    let colored = io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    format!(
        "{} {}  {}",
        style(&format!("▶ [{index}/3]"), "1;36", colored),
        style(title, "1", colored),
        style(detail, "2", colored)
    )
}

#[allow(clippy::too_many_arguments)]
fn show_report_at_level(
    template: &mut DiagnosticReport,
    level: HintLevel,
    assignment: &Assignment,
    source_context: &str,
    knowledge: &mut pada::memory::KnowledgeProfile,
    tracker: &mut UsageTracker,
    session: &mut Session,
    solution_hints: &mut SolutionHintService,
    test_mapper: &TestKnowledgeMapper,
    policy: EffortPolicy,
) -> pada::error::Result<()> {
    let llm_step_start = session.step_count();
    let mut model_call_budget = ModelCallBudget::new(policy);
    if matches!(
        level,
        HintLevel::Concept | HintLevel::Direction | HintLevel::Solution
    ) && template
        .test_entries
        .iter()
        .any(|entry| entry.classified.knowledge_points.is_empty())
    {
        let failures = template
            .test_entries
            .iter()
            .map(|entry| entry.result.clone())
            .collect::<Vec<_>>();
        match test_mapper.map_failures_with_budget(
            assignment,
            source_context,
            &failures,
            tracker,
            session,
            &mut model_call_budget,
        ) {
            Ok(mapped) => {
                let timestamp = pada::memory::now_timestamp();
                let evidence_key = stable_evidence_key(&format!(
                    "{}:{}",
                    source_context,
                    serde_json::to_string(&failures)
                        .map_err(|error| pada::error::PadaError::Parse(error.to_string()))?
                ));
                for (entry, classified) in template.test_entries.iter_mut().zip(mapped) {
                    for point in &classified.knowledge_points {
                        knowledge.record_diagnostic_once(
                            *point,
                            false,
                            format!("{evidence_key}:test:{point:?}:failed"),
                            timestamp,
                        );
                    }
                    entry.classified = classified;
                }
            }
            Err(pada::error::PadaError::Cancelled) => {
                return Err(pada::error::PadaError::Cancelled);
            }
            Err(error) => {
                eprintln!("测试知识点映射失败，将保留基础分类: {error}");
            }
        }
    }
    let mut report = template.clone();
    for entry in &mut report.compile_entries {
        entry.hint = generate_compile_hint(&entry.diag, &entry.classified, level);
    }
    for entry in &mut report.test_entries {
        entry.hint = generate_test_result_hint(&entry.result, &entry.classified, level);
    }
    print_report_heading(level);
    let streamed = solution_hints.enrich_with_budget(
        &mut report,
        assignment,
        source_context,
        knowledge,
        tracker,
        session,
        true,
        &mut model_call_budget,
    );
    print_report_excluding(&report, &streamed);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    print_model_statistics(&session.steps[llm_step_start..]);
    Ok(())
}

fn run_resume(
    store: &DataStore,
    requested: Option<&str>,
    no_interactive: bool,
    effort_override: Option<EffortMode>,
) -> pada::error::Result<()> {
    let sessions = store.recent_sessions()?;
    if sessions.is_empty() {
        println!(
            "还没有可恢复的自动会话。运行一次 diagnose 后，会话将保存在 {}。",
            store.auto_sessions_dir().display()
        );
        return Ok(());
    }
    let colored = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    println!(
        "╭─ {} ─────────────────────────────────────────",
        style("最近的诊断会话", "1;36", colored)
    );
    for (index, stored) in sessions.iter().enumerate() {
        if index > 0 {
            println!("│");
        }
        println!(
            "│  {}  {}",
            style(&format!("{:02}", index + 1), "1;33", colored),
            style(&stored.session.display_title(), "1", colored)
        );
        println!(
            "│      {:<8} {}",
            "题目",
            stored.session.problem_path_text()
        );
        println!(
            "│      {:<8} {}",
            "提交",
            stored.session.submission_path_text()
        );
        println!(
            "│      {:<8} {}",
            "描述",
            style(&stored.session.description_preview(100), "2", colored)
        );
        println!(
            "│      {:<8} {}",
            "更新",
            pada::memory::elapsed_text(stored.session.updated_at, pada::memory::now_timestamp())
        );
        println!(
            "│      {:<8} {}",
            "会话 ID",
            style(&stored.session.id, "2", colored)
        );
    }
    println!("╰──────────────────────────────────────────────────");

    let choice = match requested {
        Some(value) => value.to_owned(),
        None if io::stdin().is_terminal() => {
            print!("\n{} ", style("请选择会话序号：", "1;36", colored));
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
    let effort = effort_override.unwrap_or(context.effort);
    eprintln!(
        "正在恢复：{}（题目：{}，从 Hint 1 开始，思考模式：{}）",
        selected.session.display_title(),
        selected.session.problem_path_text(),
        effort
    );
    run_diagnose(
        DiagnoseOptions {
            problem: context.problem,
            code: context.code,
            project: context.project,
            tests: context.tests,
            generate_tests: false,
            hint: 1,
            profile: context.profile,
            budget: context.budget,
            effort,
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
    pada::agent::context::project_sources(project)
}

fn stable_evidence_key(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pada::agent::llm::{ChatMessage, ChatModel, LlmResponse};
    use pada::config::model::ModelConfig;
    use pada::models::KnowledgePoint;

    struct MappingModel;

    impl ChatModel for MappingModel {
        fn chat(&self, _messages: &[ChatMessage]) -> pada::error::Result<LlmResponse> {
            Ok(LlmResponse {
                content: r#"{"mappings":[{"index":0,"knowledge_points":["Iterator"]}]}"#.into(),
                input_tokens: 20,
                output_tokens: 8,
                model: "mapping-model".into(),
                details: Default::default(),
                timings: Default::default(),
            })
        }
    }

    #[test]
    fn upgrading_to_concept_maps_pending_test_knowledge() {
        let result = TestResult {
            name: "reverse".into(),
            passed: false,
            actual_output: "1 2 3".into(),
            expected_output: "3 2 1".into(),
            runtime_error: None,
        };
        let mut report = DiagnosticReport::new();
        report.add_test(TestReportEntry {
            classified: classify_test_result(&result),
            hint: generate_test_result_hint(
                &result,
                &classify_test_result(&result),
                HintLevel::Category,
            ),
            result,
        });
        let assignment = Assignment {
            title: "逆序".into(),
            description: "逆序输出整数".into(),
        };
        let mapper = TestKnowledgeMapper::with_model(
            ModelConfig::local("mapping-model", 8_192),
            Box::new(MappingModel),
        );
        let mut hints = SolutionHintService::new(None);
        let mut knowledge = pada::memory::KnowledgeProfile::default();
        let mut tracker = UsageTracker::new();
        let mut session = Session::new("test");

        show_report_at_level(
            &mut report,
            HintLevel::Concept,
            &assignment,
            "fn main() {}",
            &mut knowledge,
            &mut tracker,
            &mut session,
            &mut hints,
            &mapper,
            EffortPolicy::for_mode(EffortMode::Medium),
        )
        .unwrap();

        assert_eq!(
            report.test_entries[0].classified.knowledge_points,
            vec![KnowledgePoint::Iterator]
        );
        assert_eq!(tracker.session().total_tokens(), 28);
        assert_eq!(session.usage_records.len(), 1);
    }
}
