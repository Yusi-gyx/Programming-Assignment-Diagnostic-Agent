//! PADA - 编程作业诊断 Agent CLI 入口
//!
//! 职责（开发计划第 12 步）：
//! - 解析命令行参数（clap）
//! - 读取题目与代码
//! - 编排诊断工作流：编译 → 运行测试 → 分析 → 生成提示 → 输出报告
//!
//! 对应 DESIGN.md §5 的 CLI 参数与交互流程。
//!
//! # 用法
//!
//! ```bash
//! # 基本诊断
//! pada diagnose --problem problem.md --code main.rs
//!
//! # 指定提示等级与模型 profile
//! pada diagnose --problem problem.md --code main.rs --hint 2 --profile local
//!
//! # 导出 Markdown 报告
//! pada diagnose --problem problem.md --code main.rs --report report.md
//!
//! # 设置 token 预算
//! pada diagnose --problem problem.md --code main.rs --budget 20000
//! ```

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use PADA::analysis::hint::generate_compile_hint;
use PADA::history::Session;
use PADA::report::{CompileReportEntry, DiagnosticReport};

// ============================================================
// CLI 定义
// ============================================================

/// 编程作业诊断 Agent
#[derive(Parser)]
#[command(name = "pada", version, about = "Rust 编程作业诊断 Agent")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 诊断学生代码
    Diagnose {
        /// 题目描述文件（Markdown）
        #[arg(long)]
        problem: PathBuf,

        /// 单文件学生代码
        #[arg(long)]
        code: Option<PathBuf>,

        /// Cargo 多文件项目目录
        #[arg(long)]
        project: Option<PathBuf>,

        /// 初始提示等级 1-5，默认 1
        #[arg(long, default_value = "1")]
        hint: u8,

        /// 模型 profile 名称（R3）
        #[arg(long)]
        profile: Option<String>,

        /// 本次会话 token 预算（R6）
        #[arg(long)]
        budget: Option<usize>,

        /// 导出 Markdown 诊断报告路径
        #[arg(long)]
        report: Option<PathBuf>,

        /// 加载历史会话上下文（R5）
        #[arg(long)]
        history: Option<PathBuf>,

        /// 保存当前会话到指定路径（R5）
        #[arg(long)]
        save: Option<PathBuf>,

        /// 配置文件路径（默认 ~/.config/pada/config.toml）
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

// ============================================================
// 主流程
// ============================================================

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Diagnose {
            problem,
            code,
            project,
            hint,
            profile,
            budget,
            report,
            history,
            save,
            config,
        } => {
            if let Err(e) = run_diagnose(problem, code, project, hint, profile, budget, report, history, save, config) {
                eprintln!("诊断失败: {}", e);
                std::process::exit(1);
            }
        }
    }
}

/// 执行完整诊断流程。
#[allow(clippy::too_many_arguments)]
fn run_diagnose(
    problem_path: PathBuf,
    code: Option<PathBuf>,
    project: Option<PathBuf>,
    hint_level: u8,
    _profile: Option<String>,
    budget: Option<usize>,
    report_path: Option<PathBuf>,
    history_path: Option<PathBuf>,
    save_path: Option<PathBuf>,
    config_path: Option<PathBuf>,
) -> PADA::error::Result<()> {
    use PADA::analysis::classifier::classify_compile_diagnostics;
    use PADA::analysis::error_parser::parse_diagnostics;
    use PADA::analysis::hint::hint_level_from_number;
    use PADA::config::model::Config;
    use PADA::history::{AgentDecision, StepBuilder, ToolCall};
    use PADA::models::Assignment;
    use PADA::tools::compiler::CompilerTool;

    // 1. 读取题目
    let problem_content = std::fs::read_to_string(&problem_path).map_err(|e| {
        PADA::error::PadaError::FileNotFound(format!("读取题目失败: {}", e))
    })?;
    let assignment = Assignment {
        title: problem_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "未知题目".into()),
        description: problem_content,
    };

    // 2. 确定提示等级
    let level = hint_level_from_number(hint_level).unwrap_or(PADA::models::HintLevel::Category);

    // 3. 加载模型配置（用于后续 LLM 调用）
    let _model_config = config_path.as_ref().and_then(|p| {
        Config::load(p).ok().and_then(|c| {
            c.profiles.get(c.active_profile.as_str()).cloned()
        })
    });

    // 4. 初始化会话轨迹（R5）：若指定 --history 则加载历史
    let mut session = if let Some(ref hist_path) = history_path {
        let loaded = Session::load(hist_path)?;
        eprintln!("已加载历史会话: {}", loaded.summary());
        loaded
    } else {
        Session::new(&assignment.title)
    };

    // 5. 记录读取输入步骤
    session.add_step(
        StepBuilder::new(0)
            .user_input(&assignment.description)
            .decision(AgentDecision::new("reading_input", "成功读取题目与代码"))
            .build(),
    );

    // 6. 编译学生代码
    let compiler = CompilerTool::new();
    let source = match (&code, &project) {
        (Some(path), None) => Some(path.clone()),
        (None, Some(dir)) => {
            let output = compiler.cargo_check(dir)?;
            session.add_step(
                StepBuilder::new(1)
                    .tool_call(ToolCall::new("cargo_check", &dir.display().to_string(), &format!("success={}", output.success)))
                    .build(),
            );
            if !output.success {
                let diags = parse_diagnostics(&output.stderr);
                let classified = classify_compile_diagnostics(&diags);
                let report = build_report(&diags, &classified, level);
                print_report(&report, report_path.as_deref())?;
                session.add_step(
                    StepBuilder::new(2)
                        .decision(AgentDecision::new("compiling", "编译失败，分析错误"))
                        .build(),
                );
                maybe_save(&session, save_path.as_deref());
                return Ok(());
            }
            None
        }
        _ => {
            eprintln!("请指定 --code <file> 或 --project <dir>");
            return Ok(());
        }
    };

    // 7. 编译单文件
    if let Some(src) = &source {
        let output = compiler.compile_file(src, None)?;

        session.add_step(
            StepBuilder::new(1)
                .tool_call(ToolCall::new("compile_file", &src.display().to_string(), &format!("success={}", output.success)))
                .build(),
        );

        if !output.success {
            let diags = parse_diagnostics(&output.stderr);
            let classified = classify_compile_diagnostics(&diags);
            let report = build_report(&diags, &classified, level);
            print_report(&report, report_path.as_deref())?;
            session.add_step(
                StepBuilder::new(2)
                    .decision(AgentDecision::new("compiling", "编译失败，分析错误并生成提示"))
                    .build(),
            );
            maybe_save(&session, save_path.as_deref());
            return Ok(());
        }

        println!("✅ 编译通过。");
        println!("题目: {}", assignment.title);
    }

    // 8. 预算控制（R6）
    if let Some(budget) = budget {
        use PADA::telemetry::UsageTracker;
        let mut tracker = UsageTracker::new();
        tracker.set_session_budget(budget);
        println!("{}", tracker.summary());
    }

    // 9. 保存会话（R5）
    maybe_save(&session, save_path.as_deref());

    Ok(())
}

/// 从诊断与分类结果构建报告。
fn build_report(
    diags: &[PADA::analysis::error_parser::RustcDiagnostic],
    classified: &[PADA::models::Diagnostic],
    level: PADA::models::HintLevel,
) -> DiagnosticReport {
    let mut report = DiagnosticReport::new();
    for (d, c) in diags.iter().zip(classified.iter()) {
        let hint = generate_compile_hint(d, c, level);
        report.add_compile(CompileReportEntry {
            diag: d.clone(),
            classified: c.clone(),
            hint,
        });
    }
    report
}

/// 若指定了 --save 路径，保存会话（R5）。
fn maybe_save(session: &Session, save_path: Option<&std::path::Path>) {
    if let Some(path) = save_path {
        match session.save(path) {
            Ok(()) => eprintln!("会话已保存: {}", path.display()),
            Err(e) => eprintln!("保存会话失败: {}", e),
        }
    }
}

/// 输出报告到控制台或导出为 Markdown 文件。
fn print_report(report: &DiagnosticReport, report_path: Option<&std::path::Path>) -> PADA::error::Result<()> {
    print!("{}", report.to_text());

    if let Some(path) = report_path {
        let markdown = report.to_markdown();
        std::fs::write(path, markdown)
            .map_err(|e| PADA::error::PadaError::Config(format!("写入报告失败: {}", e)))?;
        eprintln!("诊断报告已导出: {}", path.display());
    }

    Ok(())
}
