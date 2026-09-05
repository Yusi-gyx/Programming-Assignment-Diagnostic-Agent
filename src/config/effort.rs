//! Agent 诊断深度的统一策略。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// 调用用途只影响模型推理强度和输出上限，不改变测试、源码范围或调用预算。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelTaskKind {
    #[default]
    General,
    KnowledgeMapping,
    Hint(crate::models::HintLevel),
    TestGeneration,
    HintBatch {
        level: crate::models::HintLevel,
        count: usize,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortMode {
    Auto,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
    Max,
}

impl EffortMode {
    pub const ALL: [Self; 6] = [
        Self::Auto,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    /// Auto 在收集复杂度信号前使用 medium 作为安全的探测策略。
    pub fn initial_policy(self) -> EffortPolicy {
        match self {
            Self::Auto => EffortPolicy::for_mode(Self::Medium),
            mode => EffortPolicy::for_mode(mode),
        }
    }

    pub fn resolve(self, signals: EffortSignals) -> EffortPolicy {
        if self != Self::Auto {
            return EffortPolicy::for_mode(self);
        }
        let mut score = 0_u8;
        score += match signals.error_count {
            0..=1 => u8::from(signals.error_count == 1),
            2..=4 => 2,
            _ => 3,
        };
        score += match signals.file_count {
            0..=1 => 0,
            2..=10 => 1,
            11..=40 => 2,
            _ => 3,
        };
        score += match signals.failed_tests {
            0 => 0,
            1..=2 => 2,
            _ => 3,
        };
        score += u8::from(signals.has_runtime_error);
        score += match signals.source_bytes {
            0..=65_536 => 0,
            65_537..=262_144 => 1,
            _ => 2,
        };
        let mode = match score {
            0..=1 => Self::Low,
            2..=3 => Self::Medium,
            4..=5 => Self::High,
            6..=7 => Self::Xhigh,
            _ => Self::Max,
        };
        EffortPolicy::for_mode(mode)
    }
}

impl fmt::Display for EffortMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        })
    }
}

impl FromStr for EffortMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err(format!(
                "无效思考模式「{value}」；可选 auto/low/medium/high/xhigh/max"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceScope {
    pub max_files: usize,
    pub context_lines: usize,
    pub max_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffortPolicy {
    pub mode: EffortMode,
    pub reasoning_effort: &'static str,
    pub max_model_calls: usize,
    pub source: SourceScope,
    pub run_tests: bool,
    pub verification_passes: u8,
}

impl EffortPolicy {
    pub fn for_mode(mode: EffortMode) -> Self {
        match mode {
            EffortMode::Auto => EffortMode::Medium.initial_policy(),
            EffortMode::Low => Self::new(mode, "low", 1, 2, 21, 16 * 1024, false, 0),
            EffortMode::Medium => Self::new(mode, "medium", 4, 12, 41, 64 * 1024, true, 0),
            EffortMode::High => Self::new(mode, "high", 8, 32, 81, 192 * 1024, true, 1),
            EffortMode::Xhigh => Self::new(mode, "xhigh", 16, 96, 161, 512 * 1024, true, 1),
            EffortMode::Max => Self::new(
                mode,
                "xhigh",
                32,
                usize::MAX,
                usize::MAX,
                2 * 1024 * 1024,
                true,
                2,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    const fn new(
        mode: EffortMode,
        reasoning_effort: &'static str,
        max_model_calls: usize,
        max_files: usize,
        context_lines: usize,
        max_bytes: usize,
        run_tests: bool,
        verification_passes: u8,
    ) -> Self {
        Self {
            mode,
            reasoning_effort,
            max_model_calls,
            source: SourceScope {
                max_files,
                context_lines,
                max_bytes,
            },
            run_tests,
            verification_passes,
        }
    }

    pub fn summary(self) -> String {
        format!(
            "{}：推理基准={}（按接口/任务适配），模型调用≤{}，源码≤{} 文件/{}，测试={}，二次验证={} 次",
            self.mode,
            self.reasoning_effort,
            self.max_model_calls,
            if self.source.max_files == usize::MAX {
                "全部".into()
            } else {
                self.source.max_files.to_string()
            },
            if self.source.context_lines == usize::MAX {
                "全部行".into()
            } else {
                format!("{} 行窗口", self.source.context_lines)
            },
            if self.run_tests { "开启" } else { "关闭" },
            self.verification_passes
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EffortSignals {
    pub error_count: usize,
    pub file_count: usize,
    pub failed_tests: usize,
    pub has_runtime_error: bool,
    pub source_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct ModelCallBudget {
    limit: usize,
    used: usize,
}

impl ModelCallBudget {
    pub fn new(policy: EffortPolicy) -> Self {
        Self {
            limit: policy.max_model_calls,
            used: 0,
        }
    }

    pub fn try_take(&mut self) -> bool {
        if self.used >= self.limit {
            return false;
        }
        self.used += 1;
        true
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.used)
    }
}
