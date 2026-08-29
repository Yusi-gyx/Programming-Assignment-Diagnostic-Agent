//! 编译工具（CompilerTool）
//!
//! 职责：
//! - 调用 `rustc` 编译单个 Rust 源文件
//! - 调用 `cargo check` 检查 Cargo 项目
//! - 捕获编译输出（stdout / stderr / 退出码）
//!
//! 本模块只负责「触发编译并收集原始输出」，
//! 错误信息的解析与分类在 `analysis` 模块完成（开发计划第 5 步）。
//!
//! 设计原则：确定性逻辑由 Rust 完成，不依赖 LLM。

use crate::error::{PadaError, Result};
use std::path::{Path, PathBuf};

// ============================================================
// 编译输出
// ============================================================

/// 编译输出结果
///
/// 封装一次编译调用的全部原始信息。
/// 后续由错误解析模块（`analysis`）进一步处理。
#[derive(Debug, Clone)]
pub struct CompileOutput {
    /// 编译是否成功（退出码为 0 视为成功）
    pub success: bool,
    /// 标准输出
    pub stdout: String,
    /// 标准错误（rustc 的错误信息通常在此）
    pub stderr: String,
    /// 退出码；被信号杀死时为 None
    pub exit_code: Option<i32>,
}

// ============================================================
// 编译工具
// ============================================================

/// 编译工具
///
/// 封装 rustc / cargo 的调用细节，使上层无需关心命令行构造。
pub struct CompilerTool {
    /// rustc 可执行文件路径，默认 "rustc"
    rustc_path: PathBuf,
    /// cargo 可执行文件路径，默认 "cargo"
    cargo_path: PathBuf,
    /// 编译使用的 edition，默认 "2021"
    edition: String,
}

impl Default for CompilerTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CompilerTool {
    /// 创建默认配置的编译工具
    pub fn new() -> Self {
        Self {
            rustc_path: PathBuf::from("rustc"),
            cargo_path: PathBuf::from("cargo"),
            edition: String::from("2021"),
        }
    }

    /// 自定义 rustc 路径（便于测试或使用特定工具链）
    pub fn with_rustc(mut self, path: impl Into<PathBuf>) -> Self {
        self.rustc_path = path.into();
        self
    }

    /// 自定义 cargo 路径
    pub fn with_cargo(mut self, path: impl Into<PathBuf>) -> Self {
        self.cargo_path = path.into();
        self
    }

    /// 自定义 edition（如 "2021" / "2024"）
    pub fn with_edition(mut self, edition: impl Into<String>) -> Self {
        self.edition = edition.into();
        self
    }

    /// 使用 `rustc` 编译单个 Rust 源文件
    ///
    /// # 参数
    /// - `source`: 源文件路径（必须存在）
    /// - `output`: 可选输出二进制路径；为 None 时仅做编译检查
    ///
    /// # 返回
    /// 编译的原始输出。调用方根据 `success` 判断是否继续。
    /// 注意：学生代码的编译错误不会作为 Err 返回，
    /// 而是体现在 `CompileOutput.success == false` 与 stderr 中。
    pub fn compile_file(
        &self,
        source: &Path,
        output: Option<&Path>,
    ) -> Result<CompileOutput> {
        // TODO: 实现 rustc 编译调用
        //
        // 建议步骤：
        // 1. 检查 source 文件是否存在，不存在返回 PadaError::FileNotFound
        // 2. 构造 Command：
        //      rustc --edition <edition> <source>
        //    若 output 为 Some，追加: -o <output>
        // 3. 捕获 stdout / stderr（建议用 output.output()? 一次性获取）
        // 4. 组装 CompileOutput：
        //      success = exit_code == 0
        //      stdout/stderr = String::from_utf8_lossy(...)
        //      exit_code = status.code()
        //
        // 提示：使用 std::process::Command
        //       命令形如：
        //       Command::new(&self.rustc_path)
        //           .arg("--edition").arg(&self.edition)
        //           .arg(source)
        //           .arg("-o").arg(output)   // 仅当 output 为 Some

        //1、检查文件
        if !source.is_file() {
            return Err(PadaError::FileNotFound(source.display().to_string()));
        }

        use std::process::Command;
        //2、组装命令
        let mut cmd = Command::new(&self.rustc_path);
        cmd.arg("--edition").arg(&self.edition).arg(source);
        if let Some(output) = output {
            cmd.arg("-o").arg("output");
        }

        //3、运行命令并提取输出
        let output = cmd.output()?;

        //4、组装结果
        let success = output.status.success();
        let exit_code = output.status.code();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        let compile_output = CompileOutput {success, stdout, stderr, exit_code};
        Ok(compile_output)
    }

    /// 使用 `cargo check` 检查 Cargo 项目
    ///
    /// # 参数
    /// - `project_dir`: Cargo 项目根目录（应包含 Cargo.toml）
    ///
    /// # 返回
    /// `cargo check` 的原始输出。退出码 0 视为通过。
    pub fn cargo_check(&self, project_dir: &Path) -> Result<CompileOutput> {
        // TODO: 实现 cargo check 调用
        //
        // 建议步骤：
        // 1. 检查 project_dir/Cargo.toml 是否存在
        // 2. 构造 Command：cargo check
        //      .current_dir(project_dir)
        // 3. 捕获输出并组装 CompileOutput
        
        //1、检查目录和文件是否存在
        if !project_dir.is_dir() {return Err(PadaError::FileNotFound(project_dir.display().to_string()));}
        let file = project_dir.join("Cargo.toml");
        if !file.is_file() {return Err(PadaError::FileNotFound(file.display().to_string()));}

        use std::process::Command;
        //2、构造命令
        let mut cmd = Command::new(&self.cargo_path);

        cmd.arg("check").current_dir(project_dir);

        //3、运行指令并获得输出
        let output = cmd.output()?;

        //4、组装CompileOutput
        Ok(CompileOutput {
            success: output.status.success(),
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        })
    }
}
