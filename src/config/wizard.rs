//! 导师模式中的交互式模型配置向导。

use crate::config::model::{Config, ModelConfig, normalize_chat_endpoint};
use crate::error::{PadaError, Result};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WizardResult {
    pub path: PathBuf,
    pub profile_name: String,
    pub model: ModelConfig,
}

#[derive(Debug, Clone, Copy)]
struct Preset {
    name: &'static str,
    profile: &'static str,
    endpoint: &'static str,
    model: &'static str,
    context_length: usize,
    input_price: f64,
    output_price: f64,
}

const PRESETS: [Preset; 3] = [
    Preset {
        name: "DeepSeek（云端）",
        profile: "deepseek",
        endpoint: "https://api.deepseek.com/v1/chat/completions",
        model: "deepseek-chat",
        context_length: 64_000,
        input_price: 1.0,
        output_price: 2.0,
    },
    Preset {
        name: "Ollama（本地）",
        profile: "local",
        endpoint: "http://localhost:11434/v1/chat/completions",
        model: "qwen2.5-coder",
        context_length: 32_768,
        input_price: 0.0,
        output_price: 0.0,
    },
    Preset {
        name: "自定义 OpenAI 兼容接口",
        profile: "custom",
        endpoint: "http://localhost:8000/v1/chat/completions",
        model: "model-name",
        context_length: 32_768,
        input_price: 0.0,
        output_price: 0.0,
    },
];

pub fn run_config_wizard<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
) -> Result<Option<WizardResult>> {
    let mut config = if path.exists() {
        Config::load(path)?
    } else {
        Config::new()
    };
    writeln!(writer, "\n┌──────────────────────────────┐")?;
    writeln!(writer, "│       PADA 模型配置向导      │")?;
    writeln!(writer, "└──────────────────────────────┘")?;
    writeln!(writer, "配置文件: {}", path.display())?;
    writeln!(writer, "  1) 新建或更新 profile")?;
    let profiles = config.profile_names();
    for (index, name) in profiles.iter().enumerate() {
        let active = if *name == config.active_profile {
            "（当前）"
        } else {
            ""
        };
        writeln!(writer, "  {}) 切换到 {}{}", index + 2, name, active)?;
    }
    writeln!(writer, "  0) 取消")?;
    loop {
        let action = prompt(writer, reader, "请选择", "1")?;
        if action == "0" {
            writeln!(writer, "已取消配置。")?;
            return Ok(None);
        }
        if action == "1" {
            break;
        }
        if let Ok(index) = action.parse::<usize>()
            && index >= 2
            && let Some(name) = profiles.get(index - 2)
        {
            config.switch(name)?;
            config.save(path)?;
            let model = config.active()?.clone();
            writeln!(writer, "✓ 已切换到 profile「{name}」。")?;
            return Ok(Some(WizardResult {
                path: path.to_owned(),
                profile_name: name.clone(),
                model,
            }));
        }
        writeln!(writer, "  无效选择，请输入菜单中的序号。")?;
    }

    writeln!(writer, "\n选择接口预设：")?;
    for (index, preset) in PRESETS.iter().enumerate() {
        writeln!(writer, "  {}) {}", index + 1, preset.name)?;
    }
    let preset_number = loop {
        let value = prompt(writer, reader, "请选择", "1")?;
        if let Some(number) = value
            .parse::<usize>()
            .ok()
            .filter(|number| (1..=PRESETS.len()).contains(number))
        {
            break number;
        }
        writeln!(writer, "  请输入 1 到 {}。", PRESETS.len())?;
    };
    let preset = PRESETS[preset_number - 1];

    let profile_name = prompt(writer, reader, "Profile 名称", preset.profile)?;
    if profile_name.trim().is_empty() {
        return Err(PadaError::Config("Profile 名称不能为空".into()));
    }
    let endpoint = normalize_chat_endpoint(&prompt(
        writer,
        reader,
        "API Endpoint（可填写服务根地址）",
        preset.endpoint,
    )?);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err(PadaError::Config(
            "API Endpoint 必须以 http:// 或 https:// 开头".into(),
        ));
    }
    let api_key = loop {
        let value = prompt(
            writer,
            reader,
            "API Key（本地模型可留空，输入内容会显示）",
            "",
        )?;
        if preset_number != 1 || !value.is_empty() {
            break value;
        }
        writeln!(writer, "  DeepSeek 云端接口需要 API Key。")?;
    };
    let model_name = prompt(writer, reader, "模型名称", preset.model)?;
    if model_name.trim().is_empty() {
        return Err(PadaError::Config("模型名称不能为空".into()));
    }
    let context_length = prompt_parse(writer, reader, "上下文长度", preset.context_length)?;
    let reasoning = prompt_bool(writer, reader, "启用 reasoning", false)?;
    let input_price = prompt_parse(writer, reader, "输入价格/百万 Token", preset.input_price)?;
    let output_price = prompt_parse(writer, reader, "输出价格/百万 Token", preset.output_price)?;
    let model = ModelConfig {
        endpoint,
        api_key,
        model_name,
        context_length,
        reasoning,
        input_price,
        output_price,
    };

    writeln!(writer, "\n配置摘要：")?;
    writeln!(writer, "  Profile   : {profile_name}")?;
    writeln!(writer, "  Endpoint  : {}", model.endpoint)?;
    writeln!(writer, "  Model     : {}", model.model_name)?;
    writeln!(writer, "  API Key   : {}", mask_key(&model.api_key))?;
    writeln!(writer, "  Context   : {}", model.context_length)?;
    writeln!(writer, "  Reasoning : {}", model.reasoning)?;
    if !prompt_bool(writer, reader, "保存并立即启用", true)? {
        writeln!(writer, "已取消配置。")?;
        return Ok(None);
    }

    config.set_profile(&profile_name, model.clone());
    config.active_profile = profile_name.clone();
    config.save(path)?;
    writeln!(writer, "✓ 配置已保存并启用: {}", path.display())?;
    Ok(Some(WizardResult {
        path: path.to_owned(),
        profile_name,
        model,
    }))
}

fn prompt<R: BufRead, W: Write>(
    writer: &mut W,
    reader: &mut R,
    label: &str,
    default: &str,
) -> Result<String> {
    if default.is_empty() {
        write!(writer, "{label}: ")?;
    } else {
        write!(writer, "{label} [{default}]: ")?;
    }
    writer.flush()?;
    let mut value = String::new();
    if reader.read_line(&mut value)? == 0 {
        return Err(PadaError::Config("配置输入提前结束".into()));
    }
    let value = value.trim();
    Ok(if value.is_empty() {
        default.to_owned()
    } else {
        value.to_owned()
    })
}

fn prompt_parse<T, R, W>(writer: &mut W, reader: &mut R, label: &str, default: T) -> Result<T>
where
    T: std::str::FromStr + std::fmt::Display + Copy,
    R: BufRead,
    W: Write,
{
    loop {
        let value = prompt(writer, reader, label, &default.to_string())?;
        if let Ok(parsed) = value.parse() {
            return Ok(parsed);
        }
        writeln!(writer, "  {label} 的数值格式无效，请重新输入。")?;
    }
}

fn prompt_bool<R: BufRead, W: Write>(
    writer: &mut W,
    reader: &mut R,
    label: &str,
    default: bool,
) -> Result<bool> {
    let default_text = if default { "Y/n" } else { "y/N" };
    loop {
        let value = prompt(writer, reader, label, default_text)?;
        if value == default_text {
            return Ok(default);
        }
        match value.to_ascii_lowercase().as_str() {
            "y" | "yes" | "true" | "1" => return Ok(true),
            "n" | "no" | "false" | "0" => return Ok(false),
            _ => writeln!(writer, "  {label} 请输入 y 或 n。")?,
        }
    }
}

fn mask_key(key: &str) -> String {
    if key.is_empty() {
        "（未设置）".into()
    } else if key.chars().count() <= 8 {
        "********".into()
    } else {
        let prefix = key.chars().take(4).collect::<String>();
        let suffix = key
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        format!("{prefix}…{suffix}")
    }
}
