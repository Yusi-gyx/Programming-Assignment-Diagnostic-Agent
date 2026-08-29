//! 统一错误类型
//!
//! 全 crate 使用 [`PadaError`] 与 [`Result`] 作为错误处理基础，
//! 避免各模块自定义互不兼容的错误类型。

use thiserror::Error;

/// PADA 错误类型
///
/// 各模块按需通过 `#[from]` 转换底层错误。
#[derive(Debug, Error)]
pub enum PadaError {
    /// 文件 / 进程 IO 错误
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 编译过程出错（非学生代码错误，而是工具自身无法完成编译调用）
    #[error("编译失败: {0}")]
    Compile(String),

    /// 程序运行出错
    #[error("程序运行失败: {0}")]
    Run(String),

    /// 指定文件不存在
    #[error("文件未找到: {0}")]
    FileNotFound(String),

    /// 解析错误（如解析 rustc 输出失败）
    #[error("解析错误: {0}")]
    Parse(String),
}

/// 统一 Result 别名
pub type Result<T> = std::result::Result<T, PadaError>;
