//! 模型配置数据结构与 profile 管理
//!
//! 参见 [`super`](crate::config) 模块文档了解配置文件格式。

use crate::error::{PadaError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Chat Completions 服务使用的思考控制协议。代理端点可显式选择。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningProtocol {
    #[default]
    Auto,
    Deepseek,
    Ollama,
    EnableThinking,
    Compatible,
}

/// 服务端输出 Token 上限，不用于估算实际用量；推理模型需为推理 Token 留余量。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OutputLimits {
    pub mapping: usize,
    pub concept: usize,
    pub direction: usize,
    pub solution: usize,
    pub test_generation: usize,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            mapping: 2048,
            concept: 2048,
            direction: 3072,
            solution: 4096,
            test_generation: 8192,
        }
    }
}

impl OutputLimits {
    pub fn for_task(&self, task: crate::config::effort::ModelTaskKind) -> usize {
        use crate::config::effort::ModelTaskKind;
        use crate::models::HintLevel;
        match task {
            ModelTaskKind::KnowledgeMapping => self.mapping,
            ModelTaskKind::Hint(HintLevel::Concept) => self.concept,
            ModelTaskKind::Hint(HintLevel::Direction) => self.direction,
            ModelTaskKind::HintBatch { level, count } => self
                .for_task(ModelTaskKind::Hint(level))
                .saturating_mul(count.clamp(1, 8)),
            ModelTaskKind::TestGeneration => self.test_generation,
            _ => self.solution,
        }
        .max(1)
    }
}

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

    /// 是否启用推理链 / reasoning；省略时默认关闭。
    #[serde(default)]
    pub reasoning: bool,

    #[serde(default)]
    pub reasoning_protocol: ReasoningProtocol,

    #[serde(default)]
    pub output_limits: OutputLimits,

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
            reasoning_protocol: ReasoningProtocol::Auto,
            output_limits: OutputLimits::default(),
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
            reasoning_protocol: ReasoningProtocol::Auto,
            output_limits: OutputLimits::default(),
            input_price,
            output_price,
        }
    }

    /// 返回可直接调用的 OpenAI 兼容 Chat Completions 地址。
    ///
    /// 配置向导允许用户填写服务根地址（如 Ollama 的
    /// `http://localhost:11434`）或 API 根地址（如 `.../v1`）。这里在
    /// 运行时补全标准路径，同时保留已经填写完整路径的配置。
    pub fn chat_endpoint(&self) -> String {
        normalize_chat_endpoint(&self.endpoint)
    }

    /// 仅官方 DeepSeek 主机使用其专有参数；同名模型的代理服务保持兼容行为。
    pub fn is_deepseek(&self) -> bool {
        let endpoint = self.endpoint.trim();
        let Some((_, rest)) = endpoint.split_once("://") else {
            return false;
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = authority.rsplit('@').next().unwrap_or_default();
        host.split(':')
            .next()
            .is_some_and(|host| host.eq_ignore_ascii_case("api.deepseek.com"))
    }

    /// 当前 endpoint 是否指向 Ollama 的默认服务端口。
    ///
    /// 旧配置没有 provider 字段，因此通过 Ollama 的默认端口保持向后兼容。
    pub fn is_ollama(&self) -> bool {
        let endpoint = self.endpoint.trim().to_ascii_lowercase();
        let authority = endpoint
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(&endpoint)
            .split('/')
            .next()
            .unwrap_or_default();
        authority
            .rsplit_once(':')
            .is_some_and(|(_, port)| port == "11434")
    }

    pub fn resolved_reasoning_protocol(&self) -> ReasoningProtocol {
        match self.reasoning_protocol {
            ReasoningProtocol::Auto if self.is_deepseek() => ReasoningProtocol::Deepseek,
            ReasoningProtocol::Auto if self.is_ollama() => ReasoningProtocol::Ollama,
            ReasoningProtocol::Auto => ReasoningProtocol::Compatible,
            protocol => protocol,
        }
    }

    pub fn is_ollama_gpt_oss(&self) -> bool {
        let model = self.model_name.to_ascii_lowercase();
        let name = model.rsplit('/').next().unwrap_or(&model);
        self.resolved_reasoning_protocol() == ReasoningProtocol::Ollama
            && (name == "gpt-oss" || name.starts_with("gpt-oss:") || name.starts_with("gpt-oss-"))
    }

    /// 显示配置意图和已知限制，不把省略参数误称为已关闭服务端思考。
    pub fn reasoning_notice(&self) -> &'static str {
        if self.is_ollama_gpt_oss() && !self.reasoning {
            return "GPT-OSS 无法完全关闭思考，将使用 low 降低推理强度。";
        }
        match (self.resolved_reasoning_protocol(), self.reasoning) {
            (ReasoningProtocol::Compatible | ReasoningProtocol::Auto, false) => {
                "当前兼容协议仅省略推理参数，无法保证服务端关闭思考；如需控制，请选择服务支持的思考协议。"
            }
            (ReasoningProtocol::Compatible | ReasoningProtocol::Auto, true) => {
                "将请求兼容推理参数；请确认服务支持，开启后可能增加等待时间和 Token 费用。"
            }
            (_, true) => "将请求开启思考，可能增加等待时间和 Token 费用；复杂分析时再开启。",
            _ => "将显式请求关闭思考；是否生效取决于服务版本和模型是否支持切换。",
        }
    }
}

/// 将服务根地址或 `/v1` API 根地址补全为 Chat Completions endpoint。
pub fn normalize_chat_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.ends_with("/chat/completions") {
        return endpoint.to_owned();
    }
    if endpoint.ends_with("/v1") {
        return format!("{endpoint}/chat/completions");
    }

    let has_path = endpoint
        .split_once("://")
        .map(|(_, rest)| rest.contains('/'))
        .unwrap_or_else(|| endpoint.contains('/'));
    if !has_path {
        format!("{endpoint}/v1/chat/completions")
    } else {
        endpoint.to_owned()
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
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| PadaError::Config(format!("创建配置目录失败: {e}")))?;
        }
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
