//! Compact, deterministic prompt context. Original requirements remain in history.
use crate::analysis::error_parser::SourceLocation;
use crate::config::effort::{EffortMode, EffortPolicy, SourceScope};
use crate::error::Result;
use std::path::Path;

/// Preserve every nonempty requirement line; remove Markdown decoration and duplicates.
/// No semantic constraints or examples are guessed or truncated.
pub fn compact_rules(description: &str) -> String {
    #[derive(serde::Serialize)]
    struct Section {
        section: String,
        rules: Vec<String>,
    }
    let mut sections = vec![Section {
        section: "requirements".into(),
        rules: Vec::new(),
    }];
    let mut in_code = false;
    for line in description.lines() {
        let trimmed = line.trim();
        if !in_code && trimmed.starts_with('#') {
            sections.push(Section {
                section: trimmed.trim_start_matches('#').trim().into(),
                rules: Vec::new(),
            });
            continue;
        }
        let Some(section) = sections.last_mut() else {
            continue;
        };
        let rules = &mut section.rules;
        if trimmed.starts_with("```") {
            in_code = !in_code;
            rules.push(line.to_owned());
            continue;
        }
        if in_code {
            rules.push(line.to_owned());
        } else if !trimmed.is_empty() {
            let rule = trimmed.trim_start_matches('#').trim().to_owned();
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
    sections.retain(|section| !section.rules.is_empty());
    serde_json::json!({ "assignment_rules": sections }).to_string()
}

pub fn project_sources(root: &Path) -> Result<String> {
    fn visit(root: &Path, dir: &Path, output: &mut String) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some("target" | ".git" | ".pada")
                ) {
                    visit(root, &path, output)?;
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                output.push_str(&format!(
                    "// file: {}\n{}\n",
                    path.strip_prefix(root).unwrap_or(&path).display(),
                    std::fs::read_to_string(&path)?
                ));
            }
        }
        Ok(())
    }
    let mut output = String::new();
    visit(root, root, &mut output)?;
    Ok(output)
}

/// Select the diagnosed module and nearby lines, retaining file and line coordinates.
pub fn relevant_source(source: &str, location: Option<&SourceLocation>) -> String {
    relevant_source_with_scope(
        source,
        location,
        EffortPolicy::for_mode(EffortMode::Medium).source,
    )
}

/// Select source according to one effort policy while always prioritizing the diagnosed file.
pub fn relevant_source_with_scope(
    source: &str,
    location: Option<&SourceLocation>,
    scope: SourceScope,
) -> String {
    let Some(location) = location else {
        return limit_source(source, scope);
    };
    let selected = if source.starts_with("// file: ") {
        source
            .split("// file: ")
            .skip(1)
            .find_map(|part| {
                let (file, body) = part.split_once('\n')?;
                (file == location.file || Path::new(&location.file).ends_with(file)).then_some(body)
            })
            .unwrap_or(source)
    } else {
        source
    };
    let radius = scope.context_lines.saturating_sub(1) / 2;
    let start = if scope.context_lines == usize::MAX {
        0
    } else {
        location.line.saturating_sub(radius + 1)
    };
    let end = if scope.context_lines == usize::MAX {
        usize::MAX
    } else {
        start.saturating_add(scope.context_lines)
    };
    let lines = selected
        .lines()
        .enumerate()
        .filter(|(index, _)| *index >= start && *index < end)
        .map(|(index, line)| format!("{}: {line}", index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    truncate_utf8(
        format!("// {} (相关源码片段)\n{lines}", location.file),
        scope.max_bytes,
    )
}

/// Limit a whole-project context by file count and UTF-8 byte size.
pub fn limit_source(source: &str, scope: SourceScope) -> String {
    if !source.starts_with("// file: ") {
        return truncate_utf8(source.to_owned(), scope.max_bytes);
    }
    let selected = source
        .split("// file: ")
        .skip(1)
        .take(scope.max_files)
        .map(|part| format!("// file: {part}"))
        .collect::<String>();
    truncate_utf8(selected, scope.max_bytes)
}

pub fn source_file_count(source: &str) -> usize {
    let count = source.match_indices("// file: ").count();
    count.max(1)
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str("\n// …源码上下文已按思考模式截断");
    value
}
