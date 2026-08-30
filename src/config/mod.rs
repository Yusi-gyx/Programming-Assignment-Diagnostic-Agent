//! R3 模型配置模块
//!
//! 职责（开发计划第 8 步）：
//! - 定义模型配置结构（endpoint / key / 上下文长度 / reasoning / 价格）
//! - 支持命名 profile 保存与切换
//! - 支持 TOML 配置文件的加载与保存
//!
//! 设计原则（AGENTS.md R3）：
//! - 不把模型配置硬编码在业务逻辑中
//! - 支持配置文件和 CLI 设置入口
//! - 多组配置以命名 profile 形式保存与切换
//!
//! # 配置文件格式（TOML）
//!
//! ```toml
//! active_profile = "deepseek"
//!
//! [profiles.deepseek]
//! endpoint = "https://api.deepseek.com/v1/chat/completions"
//! api_key = "sk-xxx"
//! model_name = "deepseek-chat"
//! context_length = 64000
//! reasoning = false
//! input_price = 1.0
//! output_price = 2.0
//!
//! [profiles.local]
//! endpoint = "http://localhost:11434/v1/chat/completions"
//! api_key = ""
//! model_name = "qwen2.5-coder"
//! context_length = 32768
//! reasoning = false
//! input_price = 0.0
//! output_price = 0.0
//! ```

pub mod model;
