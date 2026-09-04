//! 进度渲染与任务打断（R4，开发计划第 13 步）
//!
//! 职责：
//! - 为耗时任务（编译、测试、LLM 调用）提供进度报告接口
//! - 支持协作式取消（cooperative cancellation）
//! - CLI 端使用 [`indicatif`](https://docs.rs/indicatif) 渲染进度条
//!
//! 设计原则（AGENTS.md R4 / DESIGN.md §3.2）：
//! - 执行时间可能超过 3 秒的任务应展示具体进度，而非笼统的「处理中」
//! - 进度应体现阶段与计数，如「正在运行第 3/8 组测试」
//! - 打断后需安全释放子进程、文件句柄等资源
//! - 协作式取消：任务在关键检查点主动检查取消状态
//!
//! # 用法
//!
//! ```ignore
//! use pada::agent::progress::{CancelToken, SilentProgress, ProgressReporter};
//!
//! let cancel = CancelToken::new();
//! let progress = SilentProgress;
//! progress.start(8, "运行测试");
//! for i in 0..8 {
//!     if cancel.is_cancelled() { break; }
//!     // 执行一组测试...
//!     progress.tick(i + 1, &format!("第 {}/8 组测试", i + 1));
//! }
//! progress.finish("测试完成");
//! ```

use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// ============================================================
// 取消令牌
// ============================================================

/// 协作式取消令牌。
///
/// 基于原子布尔值，线程安全。任务在关键检查点调用 [`is_cancelled`]
/// 检查是否应停止。取消后任务应安全释放资源并尽可能保留已完成结果。
///
/// [`is_cancelled`]: CancelToken::is_cancelled
#[derive(Debug, Clone)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// 创建未取消的令牌。
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 标记为已取消。后续所有 [`is_cancelled`] 调用返回 `true`。
    ///
    /// [`is_cancelled`]: CancelToken::is_cancelled
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// 检查是否已被取消。
    ///
    /// 任务应在循环 / 关键检查点调用此方法，
    /// 返回 `true` 时应尽早安全退出。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 进度报告 trait
// ============================================================

/// 进度报告接口。
///
/// 不同实现：
/// - [`CliProgress`]：CLI 端用 indicatif 渲染进度条
/// - [`SilentProgress`]：测试用，不输出
pub trait ProgressReporter: Send + Sync {
    /// 开始一个有 `total` 步进度的任务。
    fn start(&self, total: usize, message: &str);

    /// 更新进度到 `current`，附带阶段描述。
    fn tick(&self, current: usize, message: &str);

    /// 任务完成。
    fn finish(&self, message: &str);

    /// 任务被取消。
    fn cancelled(&self, message: &str);
}

// ============================================================
// SilentProgress（测试用）
// ============================================================

/// 静默进度报告器，不输出任何内容。
///
/// 用于单元测试，或不需要进度展示的场景。
#[derive(Debug, Clone)]
pub struct SilentProgress;

impl ProgressReporter for SilentProgress {
    fn start(&self, _total: usize, _message: &str) {}
    fn tick(&self, _current: usize, _message: &str) {}
    fn finish(&self, _message: &str) {}
    fn cancelled(&self, _message: &str) {}
}

// ============================================================
// CliProgress（CLI 端）
// ============================================================

/// CLI 进度报告器，使用 indicatif 渲染进度条。
///
/// 示例输出：
/// ```text
/// 运行测试  [████████████░░░░░░░░] 3/8 正在第 3/8 组测试
/// ```
pub struct CliProgress {
    bar: ProgressBar,
}

impl CliProgress {
    /// 创建 CLI 进度报告器。
    pub fn new() -> Self {
        let bar = ProgressBar::new(0);
        bar.set_style(
            ProgressStyle::with_template("{prefix} {bar:20.cyan/blue} {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("█░"),
        );
        Self { bar }
    }
}

impl Default for CliProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressReporter for CliProgress {
    fn start(&self, total: usize, message: &str) {
        self.bar.set_length(total as u64);
        self.bar.set_prefix(message.to_string());
        self.bar.set_message("");
    }

    fn tick(&self, current: usize, message: &str) {
        self.bar.set_position(current as u64);
        self.bar.set_message(message.to_string());
    }

    fn finish(&self, message: &str) {
        self.bar.finish_with_message(message.to_string());
    }

    fn cancelled(&self, message: &str) {
        self.bar.finish_with_message(format!("已取消: {}", message));
    }
}

// ============================================================
// 诊断工作流的阶段定义
// ============================================================

/// 诊断工作流的标准阶段。
///
/// 用于在 CLI 中展示当前执行到哪个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticStage {
    /// 读取题目与代码
    ReadingInput,
    /// 编译学生代码
    Compiling,
    /// 解析编译错误
    ParsingErrors,
    /// 运行测试用例
    RunningTests,
    /// 调用 LLM 生成测试 / 分析
    LlmCalling,
    /// 生成诊断报告
    GeneratingReport,
}

impl DiagnosticStage {
    /// 转为中文描述。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadingInput => "读取输入",
            Self::Compiling => "编译代码",
            Self::ParsingErrors => "解析错误",
            Self::RunningTests => "运行测试",
            Self::LlmCalling => "调用 LLM",
            Self::GeneratingReport => "生成报告",
        }
    }
}

// ============================================================
// 逐步执行模式
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepChoice {
    Continue,
    RunRemaining,
    Cancel,
    Help,
    Invalid,
}

pub fn parse_step_choice(input: &str) -> StepChoice {
    match input.trim().to_ascii_lowercase().as_str() {
        "" | "c" | "continue" => StepChoice::Continue,
        "a" | "all" => StepChoice::RunRemaining,
        "q" | "quit" | "cancel" => StepChoice::Cancel,
        "h" | "help" | "?" => StepChoice::Help,
        _ => StepChoice::Invalid,
    }
}

#[derive(Debug)]
pub struct StepController {
    requested: bool,
    interactive: bool,
    run_remaining: bool,
    current: usize,
    total: usize,
}

impl StepController {
    pub fn new(requested: bool, interactive: bool) -> Self {
        Self {
            requested,
            interactive,
            run_remaining: false,
            current: 0,
            total: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.requested && self.interactive
    }

    pub fn requested_without_terminal(&self) -> bool {
        self.requested && !self.interactive
    }

    pub fn begin_round(&mut self, total: usize) {
        self.run_remaining = false;
        self.current = 0;
        self.total = total;
    }

    pub fn next_position(&mut self) -> (usize, usize) {
        self.current = (self.current + 1).min(self.total);
        (self.current, self.total)
    }

    pub fn should_prompt(&self) -> bool {
        self.is_active() && !self.run_remaining
    }

    pub fn run_remaining(&mut self) {
        self.run_remaining = true;
    }
}
