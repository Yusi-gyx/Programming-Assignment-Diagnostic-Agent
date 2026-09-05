//! 程序运行器（Runner / TestRunner）
//!
//! 职责：
//! - 运行编译后的可执行文件并传入标准输入
//! - 批量运行测试用例并判定通过 / 失败
//!
//! 与 `CompilerTool` 配合使用：
//!   CompilerTool 产出二进制 → Runner 运行 → TestRunner 判定
//!
//! 设计原则：运行结果判断是确定性逻辑，由 Rust 完成，不依赖 LLM。

use crate::error::{PadaError, Result};
use crate::models::TestResult;
use std::path::{Path, PathBuf};

// ============================================================
// 测试用例定义
// ============================================================

/// 单个测试用例定义
///
/// 描述「给定输入应当产生期望输出」的判定契约。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestCase {
    /// 用例名称
    pub name: String,
    /// 标准输入内容
    pub input: String,
    /// 期望的标准输出
    pub expected_output: String,
}

impl TestCase {
    /// 快速构造测试用例
    pub fn new(
        name: impl Into<String>,
        input: impl Into<String>,
        expected_output: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            input: input.into(),
            expected_output: expected_output.into(),
        }
    }
}

// ============================================================
// 单次运行结果
// ============================================================

/// 单次运行的可观察结果
///
/// 由 [`Runner::run`] 返回，记录程序运行的全部可观察信息。
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// 程序是否正常退出（退出码 0）
    pub success: bool,
    /// 标准输出
    pub stdout: String,
    /// 标准错误
    pub stderr: String,
    /// 退出码；被信号杀死时为 None
    pub exit_code: Option<i32>,
}

// ============================================================
// 程序运行器
// ============================================================

/// 程序运行器：运行单个可执行文件
///
/// 负责将输入喂给程序并捕获其输出。
/// 支持显式超时及终端协作式取消。
pub struct Runner {
    interactive: bool,
    /// 运行超时时间（秒），None 表示不限制
    timeout_secs: Option<u64>,
    /// 工作目录，None 表示继承当前进程
    workdir: Option<PathBuf>,
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner {
    /// 创建默认运行器（无超时、无自定义工作目录）
    pub fn new() -> Self {
        Self {
            interactive: false,
            timeout_secs: None,
            workdir: None,
        }
    }

    /// 允许终端输入取消命令；库调用默认关闭。
    pub fn with_interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// 设置运行超时（秒）。
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
        self
    }

    /// 设置程序运行的工作目录
    pub fn with_workdir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.workdir = Some(dir.into());
        self
    }

    /// 运行 `program`，将 `input` 写入其标准输入
    ///
    /// 返回程序的完整输出；超时或取消时终止并回收子进程。
    pub fn run(&self, program: &Path, input: &str) -> Result<RunOutput> {
        if !program.is_file() {
            return Err(PadaError::FileNotFound(program.display().to_string()));
        }

        use std::process::{Command, Stdio};
        let mut cmd = Command::new(program);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &self.workdir {
            cmd.current_dir(dir);
        } //如果有工作目录就设置

        let output = super::process::run_command(
            &mut cmd,
            input.as_bytes(),
            self.timeout_secs.map(std::time::Duration::from_secs),
            || self.interactive && super::process::terminal_cancelled(),
        )?;

        Ok(RunOutput {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

// ============================================================
// 测试运行器
// ============================================================

/// 测试运行器：对同一程序批量运行测试用例
///
/// 复用 [`Runner`] 逐个执行用例，并将结果归一化为 [`TestResult`]。
pub struct TestRunner {
    runner: Runner,
}

impl Default for TestRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl TestRunner {
    /// 创建默认测试运行器
    pub fn new() -> Self {
        Self {
            runner: Runner::new(),
        }
    }

    /// 指定底层 Runner（用于复用超时 / 工作目录等配置）
    pub fn with_runner(runner: Runner) -> Self {
        Self { runner }
    }

    /// 运行全部测试用例，返回每个用例的结果
    ///
    /// 判定通过条件：
    /// - 程序正常退出（exit code 0）
    /// - stdout 去除首尾空白后等于 expected_output 去除首尾空白
    ///
    /// 若程序运行失败（非零退出），该用例 passed = false，
    /// stdout 仍保存在 actual_output，运行错误单独保存在 runtime_error。
    pub fn run_tests(&self, program: &Path, tests: &[TestCase]) -> Result<Vec<TestResult>> {
        let mut vec: Vec<TestResult> = Vec::new();
        for test in tests {
            let run = match self.runner.run(program, &test.input) {
                Ok(result) => result,
                Err(e) => match e {
                    PadaError::Cancelled => return Err(PadaError::Cancelled),
                    PadaError::FileNotFound(message) => {
                        return Err(PadaError::FileNotFound(message));
                    }
                    _ => {
                        vec.push(TestResult {
                            name: test.name.clone(),
                            passed: false,
                            actual_output: "".to_string(),
                            expected_output: test.expected_output.clone(),
                            runtime_error: Some(e.to_string()),
                        });
                        continue;
                    }
                },
            };
            let passed = run.success && run.stdout.trim() == test.expected_output.trim();
            let runtime_error = (!run.success).then(|| {
                if run.stderr.trim().is_empty() {
                    format!(
                        "程序异常退出（退出码：{}）",
                        run.exit_code
                            .map(|code| code.to_string())
                            .unwrap_or_else(|| "信号终止".into())
                    )
                } else {
                    run.stderr.trim().to_owned()
                }
            });
            vec.push(TestResult {
                name: test.name.clone(),
                passed,
                actual_output: run.stdout,
                expected_output: test.expected_output.clone(),
                runtime_error,
            });
        }
        Ok(vec)
    }
}
