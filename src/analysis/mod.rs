//! 错误分析模块
//!
//! 对应开发计划第 5-7 步：
//! - [`error_parser`]: 解析 rustc 错误输出（第 5 步）
//! - [`classifier`]:  错误分类与知识点映射（第 6 步）
//! - [`hint`]:         分层提示生成（第 7 步）

pub mod classifier;
pub mod error_parser;
pub mod hint;
