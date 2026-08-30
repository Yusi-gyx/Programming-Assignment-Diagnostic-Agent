//! 模型配置模块测试（第 8 步）
//!
//! 全部离线测试（不依赖网络）。
//!
//! ```bash
//! cargo test --test config_tests
//! ```

use pada::config::model::{Config, ModelConfig};
use std::collections::HashMap;

// ============================================================
// ModelConfig 测试
// ============================================================

#[test]
fn test_model_config_local() {
    let cfg = ModelConfig::local("qwen2.5-coder", 32768);
    assert_eq!(cfg.model_name, "qwen2.5-coder");
    assert_eq!(cfg.context_length, 32768);
    assert!(cfg.api_key.is_empty());
    assert_eq!(cfg.input_price, 0.0);
    assert!(!cfg.reasoning);
}

#[test]
fn test_model_config_cloud() {
    let cfg = ModelConfig::cloud(
        "https://api.example.com/v1/chat/completions",
        "sk-test",
        "deepseek-chat",
        64000,
        1.0,
        2.0,
    );
    assert_eq!(cfg.endpoint, "https://api.example.com/v1/chat/completions");
    assert_eq!(cfg.api_key, "sk-test");
    assert_eq!(cfg.model_name, "deepseek-chat");
    assert_eq!(cfg.context_length, 64000);
    assert_eq!(cfg.input_price, 1.0);
    assert_eq!(cfg.output_price, 2.0);
}

#[test]
fn test_model_config_serialization() {
    // ModelConfig 应能序列化 / 反序列化为 JSON
    let cfg = ModelConfig::local("test-model", 4096);
    let json = serde_json::to_string(&cfg).unwrap();
    let de: ModelConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg, de);
}

// ============================================================
// Config 测试
// ============================================================

#[test]
fn test_default_template_has_profiles() {
    let config = Config::default_template();
    let names = config.profile_names();
    assert!(names.contains(&"local".to_string()));
    assert!(names.contains(&"deepseek".to_string()));
    assert_eq!(config.active_profile, "local");
}

#[test]
fn test_active_profile() {
    let config = Config::default_template();
    let active = config.active().expect("默认 profile 应存在");
    assert_eq!(active.model_name, "qwen2.5-coder");
}

#[test]
fn test_switch_profile() {
    let mut config = Config::default_template();
    config.switch("deepseek").expect("切换到 deepseek 应成功");
    assert_eq!(config.active_profile, "deepseek");
    let active = config.active().unwrap();
    assert_eq!(active.model_name, "deepseek-chat");
}

#[test]
fn test_switch_nonexistent_profile() {
    let mut config = Config::default_template();
    let result = config.switch("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_set_profile() {
    let mut config = Config::new();
    let cfg = ModelConfig::local("my-model", 8192);
    config.set_profile("custom", cfg);
    assert!(config.profiles.contains_key("custom"));
    config.active_profile = "custom".into();
    let active = config.active().unwrap();
    assert_eq!(active.model_name, "my-model");
}

#[test]
fn test_profile_names_sorted() {
    let mut config = Config::new();
    config.set_profile("zebra", ModelConfig::local("z", 1));
    config.set_profile("apple", ModelConfig::local("a", 1));
    config.set_profile("mango", ModelConfig::local("m", 1));
    let names = config.profile_names();
    assert_eq!(names, vec!["apple", "mango", "zebra"]);
}

// ============================================================
// 配置文件 round-trip 测试
// ============================================================

#[test]
fn test_config_toml_roundtrip() {
    let mut config = Config::default_template();
    config.switch("deepseek").unwrap();

    let toml_str = toml::to_string_pretty(&config).unwrap();
    let loaded: Config = toml::from_str(&toml_str).unwrap();

    assert_eq!(config, loaded);
    assert_eq!(loaded.active_profile, "deepseek");
}

#[test]
fn test_config_save_load_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");

    let mut config = Config::default_template();
    config.set_profile(
        "custom",
        ModelConfig::cloud("https://x.com", "key", "model", 1000, 0.5, 1.5),
    );
    config.switch("custom").unwrap();

    config.save(&path).expect("保存应成功");
    let loaded = Config::load(&path).expect("加载应成功");

    assert_eq!(config, loaded);
    let active = loaded.active().unwrap();
    assert_eq!(active.model_name, "model");
    assert_eq!(active.context_length, 1000);
}

#[test]
fn test_config_load_nonexistent() {
    let result = Config::load(std::path::Path::new("/nonexistent/config.toml"));
    assert!(result.is_err());
}

// ============================================================
// 边界情况
// ============================================================

#[test]
fn test_empty_config_active() {
    let config = Config::new();
    assert!(config.active().is_err(), "空配置无 active profile");
}

#[test]
fn test_config_with_reasoning() {
    let mut config = Config::new();
    let mut cfg = ModelConfig::local("reasoner", 64000);
    cfg.reasoning = true;
    config.set_profile("reasoner", cfg);
    config.active_profile = "reasoner".into();
    let active = config.active().unwrap();
    assert!(active.reasoning);
}

#[test]
fn test_config_from_hashmap() {
    // 确保 HashMap 序列化为 TOML 后能正确反序列化
    let mut profiles = HashMap::new();
    profiles.insert("a".into(), ModelConfig::local("a-model", 100));
    profiles.insert("b".into(), ModelConfig::local("b-model", 200));
    let config = Config {
        active_profile: "a".into(),
        profiles,
    };
    let toml_str = toml::to_string_pretty(&config).unwrap();
    let loaded: Config = toml::from_str(&toml_str).unwrap();
    assert_eq!(config, loaded);
}
