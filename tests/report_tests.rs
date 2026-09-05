//! 诊断报告格式化测试（第 12 步）
//!
//! 验证报告的文本与 Markdown 格式化逻辑。
//!
//! ```bash
//! cargo test --test report_tests
//! ```

use pada::analysis::error_parser::{RustcDiagnostic, Severity, SourceLocation};
use pada::analysis::hint::{generate_compile_hint, generate_test_hint, generate_test_result_hint};
use pada::models::{Diagnostic, ErrorCategory, HintLevel, KnowledgePoint, TestResult};
use pada::report::{CompileReportEntry, DiagnosticReport, TestReportEntry, format_test_run};

// ============================================================
// 辅助构造函数
// ============================================================

fn make_diag(code: Option<&str>, loc: Option<SourceLocation>) -> RustcDiagnostic {
    RustcDiagnostic {
        severity: Severity::Error,
        code: code.map(String::from),
        message: "borrow of moved value: `s`".into(),
        location: loc,
        notes: vec![],
    }
}

fn make_classified(kps: Vec<KnowledgePoint>) -> Diagnostic {
    Diagnostic {
        category: ErrorCategory::CompileError,
        knowledge_points: kps,
        confidence: 0.95,
    }
}

fn make_loc() -> SourceLocation {
    SourceLocation {
        file: "main.rs".into(),
        line: 7,
        column: 5,
    }
}

fn make_test_result(name: &str, actual: &str, expected: &str) -> TestResult {
    TestResult {
        name: name.into(),
        passed: false,
        actual_output: actual.into(),
        expected_output: expected.into(),
        runtime_error: None,
    }
}

// ============================================================
// 空报告测试
// ============================================================

#[test]
fn test_empty_report_text() {
    let report = DiagnosticReport::new();
    let text = report.to_text();
    assert!(
        text.contains("未发现问题") || text.contains("编译通过"),
        "空报告应提示无问题，实际: {}",
        text
    );
}

#[test]
fn test_empty_report_markdown() {
    let report = DiagnosticReport::new();
    let md = report.to_markdown();
    assert!(md.contains("# 诊断报告"));
    assert!(md.contains("未发现问题") || md.contains("编译通过"));
}

#[test]
fn test_empty_report_does_not_claim_unexecuted_tests_passed() {
    let report = DiagnosticReport::new();
    assert!(report.to_text_excluding(&[], &[]).contains("没有执行测试"));

    let mut tested = DiagnosticReport::new();
    tested.set_test_run(3, 3);
    assert!(tested.to_text().contains("测试 3/3 通过"));
}

#[test]
fn test_report_is_empty() {
    let report = DiagnosticReport::new();
    assert!(report.is_empty());
}

// ============================================================
// 编译诊断报告测试
// ============================================================

#[test]
fn test_compile_entry_text_format() {
    let diag = make_diag(Some("E0382"), Some(make_loc()));
    let classified = make_classified(vec![KnowledgePoint::Ownership]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Category);

    let entry = CompileReportEntry {
        diag,
        classified,
        hint,
    };
    let report = DiagnosticReport::new();
    // 通过 to_text 间接测试格式化
    let mut r = report;
    r.add_compile(entry);
    let text = r.to_text();

    // 应包含 DESIGN.md §4.1 格式的各部分
    assert!(text.contains("[编译错误]"), "应有编译错误标签");
    assert!(text.contains("main.rs:7:5"), "应有位置信息");
    assert!(text.contains("E0382"), "应有错误码");
    assert!(text.contains("知识点"), "应有知识点行");
    assert!(text.contains("所有权"), "应包含所有权知识点");
    assert!(text.contains("提示"), "应有提示行");
}

#[test]
fn test_compile_entry_no_location() {
    let diag = make_diag(Some("E0382"), None);
    let classified = make_classified(vec![KnowledgePoint::Ownership]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Category);

    let entry = CompileReportEntry {
        diag,
        classified,
        hint,
    };
    let mut report = DiagnosticReport::new();
    report.add_compile(entry);
    let text = report.to_text();

    // 无位置时不应 panic
    assert!(text.contains("[编译错误]"));
    assert!(text.contains("未知位置"));
}

#[test]
fn test_compile_entry_no_knowledge_point() {
    let diag = make_diag(Some("E9999"), Some(make_loc()));
    let classified = make_classified(vec![]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Category);

    let entry = CompileReportEntry {
        diag,
        classified,
        hint,
    };
    let mut report = DiagnosticReport::new();
    report.add_compile(entry);
    let text = report.to_text();

    assert!(text.contains("待分析"), "无知识点应显示待分析");
}

#[test]
fn test_compile_entry_markdown_format() {
    let diag = make_diag(Some("E0382"), Some(make_loc()));
    let classified = make_classified(vec![KnowledgePoint::Ownership]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Concept);

    let entry = CompileReportEntry {
        diag,
        classified,
        hint,
    };
    let mut report = DiagnosticReport::new();
    report.add_compile(entry);
    let md = report.to_markdown();

    assert!(md.contains("# 诊断报告"), "Markdown 应有标题");
    assert!(md.contains("## 编译诊断"), "应有编译诊断章节");
    assert!(md.contains("**位置**"), "应有位置字段");
    assert!(md.contains("**消息**"), "应有消息字段");
    assert!(md.contains("**知识点**"), "应有知识点字段");
    assert!(md.contains("**提示**"), "应有提示字段");
}

#[test]
fn multiline_model_hint_is_indented_in_text_and_block_formatted_in_markdown() {
    let diag = make_diag(Some("E0382"), Some(make_loc()));
    let classified = make_classified(vec![KnowledgePoint::Ownership]);
    let hint = pada::analysis::hint::Hint::new(
        HintLevel::Solution,
        "### 问题原因\n说明\n\n```rust\nlet x = 1;\n```",
    );
    let mut report = DiagnosticReport::new();
    report.add_compile(CompileReportEntry {
        diag,
        classified,
        hint,
    });

    let text = report.to_text();
    assert!(text.contains("  提示：\n    ### 问题原因"));
    assert!(text.contains("    ```rust"));
    assert!(
        report
            .to_colored_text()
            .contains("\x1b[34;1m    ### 问题原因\x1b[0m")
    );
    let markdown = report.to_markdown();
    assert!(markdown.contains("- **提示**:\n\n### 问题原因"));
    assert!(markdown.contains("```rust\nlet x = 1;\n```"));
}

// ============================================================
// 测试失败报告测试
// ============================================================

#[test]
fn test_test_failure_text_format() {
    let result = make_test_result("test_case_03", "1 2 3", "3 2 1");
    let hint = generate_test_hint("test_case_03", "1 2 3", "3 2 1", HintLevel::Category);

    let entry = TestReportEntry {
        result,
        classified: make_classified(vec![]),
        hint,
    };
    let mut report = DiagnosticReport::new();
    report.add_test(entry);
    let text = report.to_text();

    assert!(text.contains("[测试失败]"), "应有测试失败标签");
    assert!(text.contains("test_case_03"), "应有用例名");
    assert!(text.contains("3 2 1"), "应有期望输出");
    assert!(text.contains("1 2 3"), "应有实际输出");
    assert!(text.contains("提示"), "应有提示行");
}

#[test]
fn test_test_failure_markdown_format() {
    let result = make_test_result("case_1", "9", "6");
    let hint = generate_test_hint("case_1", "9", "6", HintLevel::Direction);

    let entry = TestReportEntry {
        result,
        classified: make_classified(vec![]),
        hint,
    };
    let mut report = DiagnosticReport::new();
    report.add_test(entry);
    let md = report.to_markdown();

    assert!(md.contains("## 测试失败"), "应有测试失败章节");
    assert!(md.contains("case_1"), "应有用例名");
    assert!(md.contains("**期望输出**"));
    assert!(md.contains("**实际输出**"));
}

#[test]
fn test_run_summary_and_runtime_error_are_visible() {
    let result = TestResult {
        name: "panic_case".into(),
        passed: false,
        actual_output: "partial".into(),
        expected_output: "complete".into(),
        runtime_error: Some("panicked at fixture".into()),
    };
    let classified = Diagnostic {
        category: ErrorCategory::RuntimeError,
        knowledge_points: vec![KnowledgePoint::ErrorHandling],
        confidence: 0.9,
    };
    let hint = generate_test_result_hint(&result, &classified, HintLevel::Category);
    let mut report = DiagnosticReport::new();
    report.add_test(TestReportEntry {
        result: result.clone(),
        classified,
        hint,
    });

    let summary = format_test_run(&[result], false);
    assert!(summary.contains("测试结果"));
    assert!(summary.contains("panicked at fixture"));
    let text = report.to_text();
    assert!(text.contains("[运行时错误]"));
    assert!(text.contains("程序未正常退出"));
}

#[test]
fn test_failure_report_displays_model_mapped_knowledge_points() {
    let result = make_test_result("reverse", "1 2 3", "3 2 1");
    let classified = Diagnostic {
        category: ErrorCategory::LogicError,
        knowledge_points: vec![KnowledgePoint::Iterator, KnowledgePoint::AlgorithmLogic],
        confidence: 0.7,
    };
    let hint = generate_test_hint("reverse", "1 2 3", "3 2 1", HintLevel::Concept);
    let mut report = DiagnosticReport::new();
    report.add_test(TestReportEntry {
        result,
        classified,
        hint,
    });

    assert!(report.to_text().contains("迭代器 / Iterator、算法逻辑"));
    assert!(report.to_markdown().contains("迭代器 / Iterator、算法逻辑"));
}

// ============================================================
// 混合报告测试
// ============================================================

#[test]
fn test_mixed_report_text() {
    let mut report = DiagnosticReport::new();

    // 添加编译诊断
    let diag = make_diag(Some("E0382"), Some(make_loc()));
    let classified = make_classified(vec![KnowledgePoint::Ownership]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Location);
    report.add_compile(CompileReportEntry {
        diag,
        classified,
        hint,
    });

    // 添加测试失败
    let result = make_test_result("case_2", "wrong", "right");
    let hint = generate_test_hint("case_2", "wrong", "right", HintLevel::Category);
    report.add_test(TestReportEntry {
        result,
        classified: make_classified(vec![]),
        hint,
    });

    let text = report.to_text();
    assert!(text.contains("[编译错误]"), "应包含编译诊断");
    assert!(text.contains("[测试失败]"), "应包含测试失败");
    assert!(!report.is_empty());
}

#[test]
fn test_mixed_report_markdown_has_both_sections() {
    let mut report = DiagnosticReport::new();

    let diag = make_diag(Some("E0382"), Some(make_loc()));
    let classified = make_classified(vec![KnowledgePoint::Ownership]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Category);
    report.add_compile(CompileReportEntry {
        diag,
        classified,
        hint,
    });

    let result = make_test_result("case_1", "x", "y");
    let hint = generate_test_hint("case_1", "x", "y", HintLevel::Category);
    report.add_test(TestReportEntry {
        result,
        classified: make_classified(vec![]),
        hint,
    });

    let md = report.to_markdown();
    assert!(md.contains("## 编译诊断"));
    assert!(md.contains("## 测试失败"));
}

// ============================================================
// 多条目测试
// ============================================================

#[test]
fn test_multiple_compile_entries() {
    let mut report = DiagnosticReport::new();

    for code in ["E0382", "E0499", "E0277"] {
        let diag = make_diag(Some(code), Some(make_loc()));
        let classified = make_classified(vec![]);
        let hint = generate_compile_hint(&diag, &classified, HintLevel::Category);
        report.add_compile(CompileReportEntry {
            diag,
            classified,
            hint,
        });
    }

    let text = report.to_text();
    assert_eq!(text.matches("[编译错误]").count(), 3, "应有 3 条编译诊断");
}

// ============================================================
// Hint 内容一致性测试
// ============================================================

#[test]
fn test_hint_level_reflected_in_report() {
    // 不同提示等级应反映在报告内容中
    let diag = make_diag(Some("E0382"), Some(make_loc()));
    let classified = make_classified(vec![KnowledgePoint::Ownership]);

    let hint_l1 = generate_compile_hint(&diag, &classified, HintLevel::Category);
    let hint_l3 = generate_compile_hint(&diag, &classified, HintLevel::Concept);

    let entry1 = CompileReportEntry {
        diag: diag.clone(),
        classified: classified.clone(),
        hint: hint_l1,
    };
    let entry3 = CompileReportEntry {
        diag,
        classified,
        hint: hint_l3,
    };

    let mut r1 = DiagnosticReport::new();
    r1.add_compile(entry1);
    let text1 = r1.to_text();

    let mut r3 = DiagnosticReport::new();
    r3.add_compile(entry3);
    let text3 = r3.to_text();

    // Level 1 提到「编译错误」，Level 3 提到「知识点」
    assert!(text1.contains("编译错误"));
    assert!(text3.contains("所有权"));
    // 两份报告的提示内容不同
    assert_ne!(text1, text3);
}

#[test]
fn colored_report_marks_errors_knowledge_and_hints() {
    let diag = make_diag(Some("E0382"), Some(make_loc()));
    let classified = make_classified(vec![KnowledgePoint::Ownership]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Concept);
    let mut report = DiagnosticReport::new();
    report.add_compile(CompileReportEntry {
        diag,
        classified,
        hint,
    });

    let colored = report.to_colored_text();
    assert!(colored.contains("\x1b[31;1m[编译错误]"));
    assert!(colored.contains("\x1b[33m  知识点"));
    assert!(colored.contains("\x1b[34;1m  提示"));
}

#[test]
fn streamed_entry_prefix_and_remaining_report_do_not_duplicate_content() {
    let diag = make_diag(Some("E0382"), Some(make_loc()));
    let classified = make_classified(vec![KnowledgePoint::Ownership]);
    let hint = generate_compile_hint(&diag, &classified, HintLevel::Concept);
    let mut report = DiagnosticReport::new();
    report.add_compile(CompileReportEntry {
        diag,
        classified,
        hint,
    });

    let prefix = report.compile_stream_prefix(0, false);
    let remaining = report.to_text_excluding(&[0], &[]);
    assert!(prefix.contains("[编译错误]"));
    assert!(prefix.ends_with("提示：\n"));
    assert!(remaining.is_empty());
    assert_eq!(format!("{prefix}STREAMED").matches("STREAMED").count(), 1);
}

#[test]
fn streamed_test_hint_starts_on_the_line_after_its_label() {
    let result = make_test_result("reverse", "1 2 3", "3 2 1");
    let mut report = DiagnosticReport::new();
    report.add_test(TestReportEntry {
        classified: Diagnostic {
            category: ErrorCategory::LogicError,
            knowledge_points: vec![KnowledgePoint::Iterator],
            confidence: 0.7,
        },
        hint: generate_test_result_hint(
            &result,
            &Diagnostic {
                category: ErrorCategory::LogicError,
                knowledge_points: vec![KnowledgePoint::Iterator],
                confidence: 0.7,
            },
            HintLevel::Concept,
        ),
        result,
    });

    assert!(report.test_stream_prefix(0, false).ends_with("提示：\n"));
    assert!(report.test_stream_prefix(0, true).ends_with("\x1b[0m\n"));
}
