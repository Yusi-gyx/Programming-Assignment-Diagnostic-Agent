//! 自动测试用例生成测试（第 11 步）
//!
//! 全部离线测试（不调用真实 LLM），
//! 验证提示词构造与响应解析的纯函数逻辑。
//!
//! ```bash
//! cargo test --test test_gen_tests
//! ```

use pada::models::Assignment;
use pada::tools::test_gen::{
    boundary_case_types, build_prompt, build_prompt_with_profile, parse_test_cases,
};

// ============================================================
// boundary_case_types 测试
// ============================================================

#[test]
fn test_boundary_case_types_not_empty() {
    let types = boundary_case_types();
    assert!(!types.is_empty(), "边界类型清单不应为空");
}

#[test]
fn test_boundary_case_types_covers_key_scenarios() {
    let types = boundary_case_types();
    let combined: String = types.join(" ");
    // DESIGN.md §4.2 要求覆盖的场景
    assert!(combined.contains("空"), "应包含空输入");
    assert!(combined.contains("单元素"), "应包含单元素");
    assert!(combined.contains("重复"), "应包含重复元素");
    assert!(combined.contains("负数"), "应包含负数");
    assert!(combined.contains("边界"), "应包含边界值");
}

// ============================================================
// build_prompt 测试
// ============================================================

#[test]
fn test_build_prompt_structure() {
    let assignment = Assignment {
        title: "整数求和".into(),
        description: "读取若干整数并输出它们的和".into(),
    };
    let messages = build_prompt(&assignment);

    // 应有 system + user 两条消息
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
}

#[test]
fn test_build_prompt_system_specifies_json_format() {
    let assignment = Assignment {
        title: "test".into(),
        description: "desc".into(),
    };
    let messages = build_prompt(&assignment);
    let system = &messages[0].content;

    // system prompt 应规定 JSON 数组格式
    assert!(system.contains("JSON"), "应要求 JSON 格式");
    assert!(system.contains("name"), "应要求 name 字段");
    assert!(system.contains("input"), "应要求 input 字段");
    assert!(
        system.contains("expected_output"),
        "应要求 expected_output 字段"
    );
}

#[test]
fn test_build_prompt_system_is_detailed() {
    let assignment = Assignment {
        title: "test".into(),
        description: "desc".into(),
    };
    let messages = build_prompt(&assignment);
    let system = &messages[0].content;

    // system prompt 应包含角色设定与输出规范
    assert!(system.contains("测试用例生成"), "应设定生成器角色");
    assert!(system.contains("边界"), "应提及边界测试");
    // 应指定用例数量范围
    assert!(
        system.contains("5") && system.contains("8"),
        "应指定用例数量范围"
    );
}

#[test]
fn test_build_prompt_user_includes_assignment() {
    let assignment = Assignment {
        title: "反转字符串".into(),
        description: "读取一个字符串并输出其反转".into(),
    };
    let messages = build_prompt(&assignment);
    let user = &messages[1].content;

    assert!(user.contains("反转字符串"), "应包含题目标题");
    assert!(user.contains("读取一个字符串"), "应包含题目描述");
}

#[test]
fn test_build_prompt_user_includes_boundary_types() {
    let assignment = Assignment {
        title: "test".into(),
        description: "desc".into(),
    };
    let messages = build_prompt(&assignment);
    let user = &messages[1].content;

    let types = boundary_case_types();
    for t in &types {
        assert!(
            user.contains(t),
            "user prompt 应包含边界类型 '{}'，实际: {}",
            t,
            user
        );
    }
}

#[test]
fn test_profile_summary_is_injected() {
    let assignment = Assignment {
        title: "练习".into(),
        description: "描述".into(),
    };
    let messages = build_prompt_with_profile(&assignment, "学习画像：Ownership 较弱");
    assert_eq!(messages[1].role, "system");
    assert!(messages[1].content.contains("Ownership"));
}

// ============================================================
// parse_test_cases 测试
// ============================================================

#[test]
fn test_parse_plain_json_array() {
    let content = r#"[
        {"name": "empty", "input": "", "expected_output": "0"},
        {"name": "single", "input": "5", "expected_output": "10"}
    ]"#;

    let cases = parse_test_cases(content).expect("解析应成功");
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].name, "empty");
    assert_eq!(cases[0].input, "");
    assert_eq!(cases[0].expected_output, "0");
    assert_eq!(cases[1].name, "single");
    assert_eq!(cases[1].expected_output, "10");
}

#[test]
fn test_parse_json_code_block() {
    let content = r#"这是一些解释文字。

```json
[
  {"name": "case1", "input": "1\n2\n", "expected_output": "3"}
]
```

更多说明。"#;

    let cases = parse_test_cases(content).expect("解析应成功");
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].name, "case1");
    assert_eq!(cases[0].input, "1\n2\n");
    assert_eq!(cases[0].expected_output, "3");
}

#[test]
fn test_parse_plain_code_block() {
    let content = "好的，以下是测试用例：\n```\n[{\"name\":\"x\",\"input\":\"\",\"expected_output\":\"\"}]\n```\n";

    let cases = parse_test_cases(content).expect("解析应成功");
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].name, "x");
}

#[test]
fn test_parse_multiple_cases_with_newlines() {
    let content = r#"```json
[
  {"name": "empty_input", "input": "", "expected_output": "0"},
  {"name": "single", "input": "42", "expected_output": "42"},
  {"name": "negatives", "input": "-1\n-2\n-3\n", "expected_output": "-6"},
  {"name": "duplicates", "input": "5\n5\n5\n", "expected_output": "15"}
]
```"#;

    let cases = parse_test_cases(content).expect("解析应成功");
    assert_eq!(cases.len(), 4);
    assert_eq!(cases[3].name, "duplicates");
    assert_eq!(cases[3].expected_output, "15");
}

#[test]
fn test_parse_empty_array() {
    let content = "[]";
    let cases = parse_test_cases(content).expect("解析应成功");
    assert!(cases.is_empty());
}

#[test]
fn test_parse_malformed_json() {
    let content = "not json at all";
    let result = parse_test_cases(content);
    assert!(result.is_err(), "非 JSON 应返回错误");
}

#[test]
fn test_parse_missing_field() {
    let content = r#"[{"name": "x", "input": "1"}]"#;
    let result = parse_test_cases(content);
    assert!(result.is_err(), "缺少 expected_output 应返回错误");
}

#[test]
fn test_parse_with_surrounding_explanation() {
    // LLM 有时会输出解释文字 + JSON
    let content = r#"我生成了以下测试用例：

```json
[
  {"name": "test1", "input": "hello", "expected_output": "olleh"}
]
```

这些用例覆盖了空输入和边界值。"#;

    let cases = parse_test_cases(content).expect("解析应成功");
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].name, "test1");
    assert_eq!(cases[0].input, "hello");
    assert_eq!(cases[0].expected_output, "olleh");
}

#[test]
fn test_parse_case_fields_preserved() {
    // 验证多行输入中的 \n 被正确保留
    let content = r#"[{"name":"multi","input":"1\n2\n3\n","expected_output":"6"}]"#;
    let cases = parse_test_cases(content).unwrap();
    assert_eq!(cases[0].input, "1\n2\n3\n");
    assert_eq!(cases[0].input.lines().count(), 3);
}
