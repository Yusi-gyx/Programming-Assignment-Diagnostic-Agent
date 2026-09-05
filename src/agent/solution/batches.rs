use super::*;
use crate::agent::llm::{ChatMessage, hint_level_instruction};
use crate::error::{PadaError, Result};
use crate::report::TestReportEntry;

impl SolutionHintService {
    /// 四五级测试提示每批最多 8 项，共用题目和源码；每条输出仍按原编号校验。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn enrich_test_batches(
        &mut self,
        report: &mut DiagnosticReport,
        assignment: &Assignment,
        source: &str,
        profile: &str,
        tracker: &mut UsageTracker,
        session: &mut Session,
        interactive: bool,
        budget: &mut ModelCallBudget,
    ) -> (Vec<usize>, bool) {
        let mut handled = Vec::new();
        let (Some(config), Some(model)) = (&self.config, &self.model) else {
            return (handled, false);
        };
        let source = crate::agent::context::limit_source(source, self.policy.source);
        for level in [HintLevel::Direction, HintLevel::Solution] {
            let indices = report
                .test_entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.hint.level == level)
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            if indices.len() < 2 {
                continue;
            }
            handled.extend(&indices);
            let mut pending = Vec::new();
            for index in indices {
                let entry = &mut report.test_entries[index];
                let base = crate::analysis::hint::generate_test_result_hint(
                    &entry.result,
                    &entry.classified,
                    level,
                );
                let key = serde_json::json!(test_hint_messages(
                    assignment,
                    &source,
                    &entry.result,
                    &entry.classified,
                    level,
                    &base.content,
                    profile
                ))
                .to_string();
                if let Some(content) = self.cache.get(&key) {
                    entry.hint = Hint::new(level, content.clone());
                    record_cache_hit(session, level);
                } else {
                    pending.push((index, key));
                }
            }
            for chunk in pending.chunks(8) {
                if !tracker.check_budget() || !budget.try_take() {
                    break;
                }
                let items = chunk
                    .iter()
                    .map(|(i, _)| (*i, &report.test_entries[*i]))
                    .collect::<Vec<_>>();
                let messages = batch_messages(assignment, &source, profile, level, &items);
                eprintln!(
                    "正在批量生成 Level {} 提示：本批 {} 个失败用例。",
                    hint_level_number(level),
                    chunk.len()
                );
                match run_recorded_model_task(
                    Arc::clone(model),
                    &messages,
                    interactive,
                    ModelTaskKind::HintBatch {
                        level,
                        count: chunk.len(),
                    },
                    session,
                    |_| {},
                ) {
                    ModelTaskOutcome::Completed(Ok(response)) => {
                        record_exchange(
                            session,
                            tracker,
                            config,
                            messages,
                            &response,
                            level,
                            "批量分析失败用例，共享题目与源码，并按原始用例编号校验提示",
                        );
                        let expected = chunk.iter().map(|(i, _)| *i).collect::<Vec<_>>();
                        let parsed = response
                            .ensure_complete()
                            .and_then(|()| parse_batch(&response.content, &expected));
                        match parsed {
                            Ok(hints) => {
                                for ((index, key), content) in chunk.iter().zip(hints) {
                                    self.cache.insert(key.clone(), content.clone());
                                    report.test_entries[*index].hint = Hint::new(level, content);
                                }
                            }
                            Err(error) => eprintln!("批量提示无效，保留基础诊断：{error}"),
                        }
                    }
                    ModelTaskOutcome::Completed(Err(error)) => {
                        eprintln!("批量提示失败，保留基础诊断：{error}")
                    }
                    ModelTaskOutcome::Cancelled => return (handled, true),
                }
            }
        }
        (handled, false)
    }
}

fn batch_messages(
    assignment: &Assignment,
    source: &str,
    profile: &str,
    level: HintLevel,
    entries: &[(usize, &TestReportEntry)],
) -> Vec<ChatMessage> {
    let items = entries.iter().map(|(index, entry)| serde_json::json!({
        "index": index, "result": entry.result, "diagnostic": entry.classified,
        "base_hint": crate::analysis::hint::generate_test_result_hint(&entry.result, &entry.classified, level).content,
    })).collect::<Vec<_>>();
    vec![
        ChatMessage::system(format!(
            "你是 Rust 编程导师。不要输出思维链。对每个用例分别生成当前等级的提示，不混淆不同用例的证据。{}\n{}\n只输出 JSON：{{\"hints\":[{{\"index\":原始编号,\"content\":\"Markdown 提示\"}}]}}。必须覆盖全部输入编号且各一次。每项保持简洁。",
            hint_level_instruction(level),
            profile
        )),
        ChatMessage::user(
            serde_json::json!({"assignment": assignment, "source": source, "failures": items})
                .to_string(),
        ),
    ]
}

fn parse_batch(content: &str, expected: &[usize]) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Payload {
        hints: Vec<Item>,
    }
    #[derive(serde::Deserialize)]
    struct Item {
        index: usize,
        content: String,
    }
    let content = format_model_output(content);
    let trimmed = content.trim();
    let json = trimmed
        .strip_prefix("```json\n")
        .or_else(|| trimmed.strip_prefix("```\n"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(trimmed);
    let payload: Payload = serde_json::from_str(json.trim())
        .map_err(|e| PadaError::Parse(format!("批量提示 JSON 无效：{e}")))?;
    let mut hints = HashMap::new();
    for item in payload.hints {
        let content = format_model_output(&item.content);
        if crate::agent::llm::StreamText::default()
            .push(&item.content)
            .trim()
            .is_empty()
            || !expected.contains(&item.index)
            || content.trim().is_empty()
            || hints.insert(item.index, content).is_some()
        {
            return Err(PadaError::Parse("批量提示含未知/重复编号或空内容".into()));
        }
    }
    expected
        .iter()
        .map(|i| {
            hints
                .remove(i)
                .ok_or_else(|| PadaError::Parse(format!("批量提示缺少编号 {i}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batch_parser_requires_exact_ids_and_reorders_by_evidence() {
        assert_eq!(
            parse_batch(
                r#"{"hints":[{"index":4,"content":"B"},{"index":1,"content":"A"}]}"#,
                &[1, 4]
            )
            .unwrap(),
            ["A", "B"]
        );
        for text in [
            r#"{"hints":[{"index":1,"content":"A"}]}"#,
            r#"{"hints":[{"index":1,"content":"A"},{"index":1,"content":"B"}]}"#,
            r#"{"hints":[{"index":1,"content":"A"},{"index":9,"content":"B"}]}"#,
            r#"{"hints":[{"index":1,"content":"A"},{"index":4,"content":" "}]}"#,
        ] {
            assert!(parse_batch(text, &[1, 4]).is_err());
        }
    }
}
