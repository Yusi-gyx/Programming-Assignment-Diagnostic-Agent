//! Agent 调度模块
//!
//! 当前包含：
//! - [`llm`]:      LLM 客户端（第 9 步）
//! - [`progress`]: 进度渲染与任务打断 R4（第 13 步）
//!
//! 后续将补充完整的工作流编排：
//! - 编排 编译 → 运行 → 诊断 → 提示 的完整流程
//! - 多轮交互与状态管理

pub mod export;
pub mod interaction;
pub mod llm;
pub mod progress;
pub mod solution;
pub mod test_analysis;
