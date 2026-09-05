//! 工具模块
//!
//! 提供 Agent 可调用的确定性工具。
//! 这些工具负责「与外部世界交互」（编译器、子进程），
//! 不包含语义判断逻辑。
//!
//! - [`compiler`]: 编译工具（rustc / cargo check）
//! - [`runner`][]: 程序运行与测试运行器
//! - [`test_gen`][]: 自动测试用例生成

pub mod compiler;
pub mod process;
pub mod runner;
pub mod test_gen;
