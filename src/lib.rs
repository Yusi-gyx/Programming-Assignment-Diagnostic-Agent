//! PADA - 编程作业诊断 Agent 库入口
//!
//! 本 crate 组织诊断 Agent 的全部核心能力。
//! `main.rs` 仅负责 CLI 初始化与流程编排，业务逻辑均通过此库暴露。
//!
//! 模块划分对应 DESIGN.md §8：
//! - [`models`]:   核心数据结构
//! - [`error`]:    统一错误类型
//! - [`tools`]:    确定性工具（编译 / 运行）
//! - [`agent`]:    Agent 调度（后续步骤实现）
//! - [`analysis`]: 错误分析（后续步骤实现）
//! - [`memory`]:   学习进度记忆（V2）
//! - [`config`]:   R3 模型配置（后续步骤实现）
//! - [`history`]:  R5 会话历史（后续步骤实现）
//! - [`telemetry`]:R6 token / 成本统计（后续步骤实现）

pub mod error;
pub mod models;
pub mod report;
pub mod tools;

pub mod agent;
pub mod analysis;
pub mod config;
pub mod history;
pub mod memory;
pub mod storage;
pub mod telemetry;
