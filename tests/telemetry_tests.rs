//! R6 Token 用量与成本统计测试（第 10 步）
//!
//! 全部离线测试，不依赖网络。
//!
//! ```bash
//! cargo test --test telemetry_tests
//! ```

use pada::agent::llm::LlmResponse;
use pada::config::model::ModelConfig;
use pada::telemetry::{UsageRecord, UsageTracker, calculate_cost};

/// 构造一个模型配置（价格便于心算：输入 1 元/百万，输出 2 元/百万）
fn make_config() -> ModelConfig {
    ModelConfig::cloud("https://x.com", "key", "test-model", 8192, 1.0, 2.0)
}

fn make_response(input: usize, output: usize) -> LlmResponse {
    LlmResponse {
        details: Default::default(),
        timings: Default::default(),
        content: "reply".into(),
        input_tokens: input,
        output_tokens: output,
        model: "test-model".into(),
    }
}

// ============================================================
// calculate_cost 测试
// ============================================================

#[test]
fn test_calculate_cost_basic() {
    // 输入 1M * 1元 = 1.0；输出 1M * 2元 = 2.0；合计 3.0
    let cost = calculate_cost(1_000_000, 1_000_000, &make_config());
    assert!((cost - 3.0).abs() < 1e-9, "成本应为 3.0，实际: {}", cost);
}

#[test]
fn test_calculate_cost_small() {
    // 输入 100 * 1/1M = 0.0001；输出 50 * 2/1M = 0.0001；合计 0.0002
    let cost = calculate_cost(100, 50, &make_config());
    assert!(
        (cost - 0.0002).abs() < 1e-9,
        "成本应为 0.0002，实际: {}",
        cost
    );
}

#[test]
fn test_calculate_cost_zero() {
    let cost = calculate_cost(0, 0, &make_config());
    assert!((cost - 0.0).abs() < 1e-9);
}

#[test]
fn test_calculate_cost_local_model() {
    // 本地模型价格为 0
    let config = ModelConfig::local("local-model", 4096);
    let cost = calculate_cost(1000, 500, &config);
    assert!((cost - 0.0).abs() < 1e-9, "本地模型成本应为 0");
}

// ============================================================
// UsageRecord 测试
// ============================================================

#[test]
fn test_usage_record_from_response() {
    let resp = make_response(100, 50);
    let config = make_config();
    let record = UsageRecord::from_response(&resp, &config);

    assert_eq!(record.input_tokens, 100);
    assert_eq!(record.output_tokens, 50);
    assert_eq!(record.model, "test-model");
    assert!((record.cost - 0.0002).abs() < 1e-9);
    assert!(record.timestamp > 0, "时间戳应非零");
}

#[test]
fn test_usage_record_serialization() {
    let record = UsageRecord::from_response(&make_response(10, 5), &make_config());
    let json = serde_json::to_string(&record).unwrap();
    let de: UsageRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(record, de);
}

// ============================================================
// SessionUsage 测试（通过 UsageTracker 间接）
// ============================================================

#[test]
fn test_session_usage_empty() {
    let tracker = UsageTracker::new();
    let session = tracker.session();
    assert_eq!(session.total_input_tokens, 0);
    assert_eq!(session.total_output_tokens, 0);
    assert_eq!(session.total_tokens(), 0);
    assert!(session.records.is_empty());
}

#[test]
fn test_session_usage_accumulation() {
    // 通过 UsageTracker 间接测试 SessionUsage 的累加
    let mut tracker = UsageTracker::new();
    let config = make_config();

    tracker.record(&make_response(100, 50), &config);
    tracker.record(&make_response(200, 100), &config);

    let session = tracker.session();
    assert_eq!(session.total_input_tokens, 300);
    assert_eq!(session.total_output_tokens, 150);
    assert_eq!(session.total_tokens(), 450);
    assert!((session.total_cost - (0.0002 + 0.0004)).abs() < 1e-9);
    assert_eq!(session.records.len(), 2);
}

// ============================================================
// HistoryTotal 测试
// ============================================================

#[test]
fn test_history_merge() {
    // 通过 UsageTracker 测试会话合并到历史累计
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history.json");
    let config = make_config();

    // 第一个会话
    let mut tracker = UsageTracker::new();
    tracker.record(&make_response(100, 50), &config);
    tracker.record(&make_response(200, 100), &config);
    tracker.merge_session_to_history();
    tracker.save_history(&path).unwrap();

    assert_eq!(tracker.history().total_input_tokens, 300);
    assert_eq!(tracker.history().total_output_tokens, 150);

    // 模拟第二个会话：从文件加载历史，记录新用量
    let mut tracker2 = UsageTracker::load_history(&path).unwrap();
    tracker2.record(&make_response(50, 25), &config);
    tracker2.merge_session_to_history();

    assert_eq!(tracker2.history().total_input_tokens, 350);
    assert_eq!(tracker2.history().total_output_tokens, 175);
}

// ============================================================
// UsageTracker 基本功能
// ============================================================

#[test]
fn test_tracker_record_and_totals() {
    let mut tracker = UsageTracker::new();
    let config = make_config();

    tracker.record(&make_response(100, 50), &config);
    tracker.record(&make_response(200, 100), &config);

    let session = tracker.session();
    assert_eq!(session.total_input_tokens, 300);
    assert_eq!(session.total_output_tokens, 150);
    assert_eq!(session.records.len(), 2);

    // 未合并前历史应为空
    assert_eq!(tracker.history().total_input_tokens, 0);
}

#[test]
fn test_tracker_merge_session_to_history() {
    let mut tracker = UsageTracker::new();
    let config = make_config();

    tracker.record(&make_response(100, 50), &config);
    tracker.record(&make_response(200, 100), &config);
    tracker.merge_session_to_history();

    assert_eq!(tracker.history().total_input_tokens, 300);
    assert_eq!(tracker.history().total_output_tokens, 150);

    // 合并后会话累计不变（合并是累加到历史，不清空会话）
    assert_eq!(tracker.session().total_input_tokens, 300);
}

// ============================================================
// 预算控制测试
// ============================================================

#[test]
fn test_budget_no_limit() {
    let tracker = UsageTracker::new();
    // 无预算限制，应始终可调用
    assert!(tracker.check_budget());
}

#[test]
fn test_budget_within_limit() {
    let mut tracker = UsageTracker::new();
    tracker.set_session_budget(1000);
    let config = make_config();
    tracker.record(&make_response(100, 50), &config); // 150 token
    assert!(tracker.check_budget(), "150 < 1000 应在预算内");
}

#[test]
fn test_budget_exceeded() {
    let mut tracker = UsageTracker::new();
    tracker.set_session_budget(150);
    let config = make_config();
    tracker.record(&make_response(100, 50), &config); // 150 token == budget
    assert!(!tracker.check_budget(), "达到预算上限应阻止后续调用");
}

#[test]
fn test_budget_period() {
    let mut tracker = UsageTracker::new();
    tracker.set_period_budget(1000);
    let config = make_config();

    // 历史已有 300 token
    tracker.merge_session_to_history(); // 合并空会话，无变化
    // 模拟历史已有用量：先记录再合并
    tracker.record(&make_response(200, 100), &config); // 会话 300
    assert!(tracker.check_budget(), "周期已用 300 < 1000 应可调用");

    tracker.record(&make_response(500, 200), &config); // 会话 1000，周期 1000
    assert!(!tracker.check_budget(), "周期达到 1000 应阻止");
}

#[test]
fn test_budget_summary() {
    let mut tracker = UsageTracker::new();
    tracker.set_session_budget(1000);
    let config = make_config();
    tracker.record(&make_response(100, 50), &config);

    let summary = tracker.summary();
    assert!(summary.contains("150"), "摘要应含当前用量");
    assert!(summary.contains("1000"), "摘要应含预算");
    assert!(summary.contains("850"), "摘要应含剩余预算");
}

// ============================================================
// 历史持久化测试
// ============================================================

#[test]
fn test_history_save_load_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history.json");

    let mut tracker = UsageTracker::new();
    let config = make_config();
    tracker.record(&make_response(100, 50), &config);
    tracker.record(&make_response(200, 100), &config);
    tracker.merge_session_to_history();

    tracker.save_history(&path).expect("保存应成功");

    let loaded = UsageTracker::load_history(&path).expect("加载应成功");
    assert_eq!(loaded.history().total_input_tokens, 300);
    assert_eq!(loaded.history().total_output_tokens, 150);
    // 加载后会话用量应为空
    assert_eq!(loaded.session().total_input_tokens, 0);
}

#[test]
fn test_history_load_nonexistent() {
    let result = UsageTracker::load_history(std::path::Path::new("/nonexistent/history.json"));
    assert!(result.is_err());
}

// ============================================================
// 集成测试：模拟连续调用 + 预算中断
// ============================================================

#[test]
fn test_integration_budget_stops_calls() {
    // 模拟场景：预算 500 token，每次调用用 150 token
    let mut tracker = UsageTracker::new();
    tracker.set_session_budget(500);
    let config = make_config();

    // 第 1 次调用（150 token）
    assert!(tracker.check_budget());
    tracker.record(&make_response(100, 50), &config);

    // 第 2 次调用（累计 300 token）
    assert!(tracker.check_budget());
    tracker.record(&make_response(100, 50), &config);

    // 第 3 次调用（累计 450 token）
    assert!(tracker.check_budget());
    tracker.record(&make_response(100, 50), &config);

    // 第 4 次调用前检查：累计 450 + 150 = 600 > 500
    // 但 check_budget 检查的是当前已用 >= 预算
    // 当前已用 450 < 500，仍可调用
    assert!(tracker.check_budget());
    tracker.record(&make_response(100, 50), &config);

    // 第 5 次调用前：已用 600 >= 500，应阻止
    assert!(!tracker.check_budget(), "超出预算后应阻止后续调用");
}
