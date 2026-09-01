//! 模型配置数据结构与 profile 管理
//!
//! 参见 [`super`](crate::config) 模块文档了解配置文件格式。

use crate::error::{PadaError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ============================================================
// 单个模型配置
// ============================================================

/// 单个模型的配置。
///
/// 对应 DESIGN.md §3.1 的 R3 要求：
/// endpoint、API key、上下文长度、reasoning、价格。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelConfig {
    /// API endpoint URL（OpenAI 兼容的 chat completions 接口）
    pub endpoint: String,

    /// API Key（本地模型可留空）
    pub api_key: String,

    /// 模型名称（如 `"deepseek-chat"`、`"qwen2.5-coder"`）
    pub model_name: String,

    /// 上下文长度（token 数），用于控制输入截断与预算
    pub context_length: usize,

    /// 是否启用推理链 / reasoning
    pub reasoning: bool,

    /// 输入 token 单价（每百万 token，单位：元或美元）
    pub input_price: f64,

    /// 输出 token 单价（每百万 token）
    pub output_price: f64,
}

impl ModelConfig {
    /// 创建本地 Ollama 模型的默认配置。
    pub fn local(model_name: impl Into<String>, context_length: usize) -> Self {
        Self {
            endpoint: "http://localhost:11434/v1/chat/completions".into(),
            api_key: String::new(),
            model_name: model_name.into(),
            context_length,
            reasoning: false,
            input_price: 0.0,
            output_price: 0.0,
        }
    }

    /// 创建云端模型的默认配置。
    pub fn cloud(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        model_name: impl Into<String>,
        context_length: usize,
        input_price: f64,
        output_price: f64,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            model_name: model_name.into(),
            context_length,
            reasoning: false,
            input_price,
            output_price,
        }
    }
}

// ============================================================
// 多 profile 配置
// ============================================================

/// 多 profile 配置，支持命名 profile 之间切换。
///
/// 通过 [`Config::load`] 从 TOML 文件加载，[`Config::save`] 保存。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// 当前激活的 profile 名
    pub active_profile: String,

    /// 命名 profile 表
    pub profiles: HashMap<String, ModelConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self::default_template()
    }
}

impl Config {
    /// 创建空配置（无 profile）。
    pub fn new() -> Self {
        Self {
            active_profile: String::new(),
            profiles: HashMap::new(),
        }
    }

    /// 生成默认配置模板，包含本地与云端两个示例 profile。
    ///
    /// 云端 profile 的 api_key 为空，需用户填写后使用。
    pub fn default_template() -> Self {
        let mut profiles = HashMap::new();

        // 本地模型 profile（Ollama，无需 key）
        profiles.insert("local".into(), ModelConfig::local("qwen2.5-coder", 32768));

        // 云端模型 profile（DeepSeek，需填写 key）
        profiles.insert(
            "deepseek".into(),
            ModelConfig::cloud(
                "https://api.deepseek.com/v1/chat/completions",
                "", // 用户需填写
                "deepseek-chat",
                64000,
                1.0,
                2.0,
            ),
        );

        Self {
            active_profile: "local".into(),
            profiles,
        }
    }

    /// 从 TOML 文件加载配置。
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| PadaError::Config(format!("读取配置文件失败: {}", e)))?;
        let config: Config = toml::from_str(&content)
            .map_err(|e| PadaError::Config(format!("解析配置文件失败: {}", e)))?;
        Ok(config)
    }

    /// 保存配置为 TOML 文件。
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| PadaError::Config(format!("序列化配置失败: {}", e)))?;
        std::fs::write(path, content)
            .map_err(|e| PadaError::Config(format!("写入配置文件失败: {}", e)))?;
        Ok(())
    }

    /// 获取当前激活的模型配置。
    ///
    /// 若激活的 profile 不存在则返回错误。
    pub fn active(&self) -> Result<&ModelConfig> {
        self.profiles
            .get(&self.active_profile)
            .ok_or_else(|| PadaError::Config(format!("profile '{}' 不存在", self.active_profile)))
    }

    /// 切换激活 profile。
    ///
    /// 若指定的 profile 不存在则返回错误。
    pub fn switch(&mut self, name: &str) -> Result<()> {
        if !self.profiles.contains_key(name) {
            return Err(PadaError::Config(format!(
                "profile '{}' 不存在，可用: {}",
                name,
                self.profile_names().join(", ")
            )));
        }
        self.active_profile = name.into();
        Ok(())
    }

    /// 添加或替换 profile。
    pub fn set_profile(&mut self, name: impl Into<String>, config: ModelConfig) {
        self.profiles.insert(name.into(), config);
    }

    /// 列出所有 profile 名称。
    pub fn profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.profiles.keys().cloned().collect();
        names.sort();
        names
    }
}
