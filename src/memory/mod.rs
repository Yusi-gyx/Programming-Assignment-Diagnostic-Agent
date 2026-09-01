//! V2 学习画像：掌握度、遗忘衰减、薄弱点识别与持久化。
use crate::{
    analysis::hint::knowledge_point_text,
    error::{PadaError, Result},
    models::KnowledgePoint,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_DECAY_SECS: f64 = 30.0 * 86_400.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MasteryEvent {
    Diagnostic { passed: bool, timestamp: u64 },
    UserFeedback { understood: bool, timestamp: u64 },
}
impl MasteryEvent {
    fn evidence(&self) -> f32 {
        match self {
            Self::Diagnostic { passed: true, .. } => 0.9,
            Self::Diagnostic { passed: false, .. } => 0.1,
            Self::UserFeedback {
                understood: true, ..
            } => 1.0,
            Self::UserFeedback {
                understood: false, ..
            } => 0.0,
        }
    }
    fn timestamp(&self) -> u64 {
        match self {
            Self::Diagnostic { timestamp, .. } | Self::UserFeedback { timestamp, .. } => *timestamp,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mastery {
    pub point: KnowledgePoint,
    pub score: f32,
    pub confidence: f32,
    pub last_seen: u64,
    pub history: Vec<MasteryEvent>,
}
impl Mastery {
    pub fn new(point: KnowledgePoint, timestamp: u64) -> Self {
        Self {
            point,
            score: 0.5,
            confidence: 0.0,
            last_seen: timestamp,
            history: vec![],
        }
    }
    pub fn update(&mut self, event: MasteryEvent) {
        let alpha = 0.5 - 0.3 * self.confidence;
        self.score = (alpha * event.evidence() + (1. - alpha) * self.score).clamp(0., 1.);
        self.confidence = (1.0 - 0.8_f32.powi((self.history.len() + 1) as i32)).clamp(0.0, 1.0);
        self.last_seen = event.timestamp();
        self.history.push(event);
    }
    pub fn effective_score_at(&self, timestamp: u64, decay_secs: f64) -> f32 {
        if decay_secs <= 0.0 {
            return self.score;
        }
        (self.score as f64
            * (-(timestamp.saturating_sub(self.last_seen) as f64) / decay_secs).exp())
        .clamp(0.0, 1.0) as f32
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeProfile {
    pub mastery: HashMap<KnowledgePoint, Mastery>,
}
impl KnowledgeProfile {
    pub fn record_diagnostic(&mut self, point: KnowledgePoint, passed: bool, timestamp: u64) {
        self.record(point, MasteryEvent::Diagnostic { passed, timestamp });
    }
    pub fn record_feedback(&mut self, point: KnowledgePoint, understood: bool, timestamp: u64) {
        self.record(
            point,
            MasteryEvent::UserFeedback {
                understood,
                timestamp,
            },
        );
    }
    fn record(&mut self, point: KnowledgePoint, event: MasteryEvent) {
        self.mastery
            .entry(point)
            .or_insert_with(|| Mastery::new(point, event.timestamp()))
            .update(event);
    }
    pub fn weak_points_at(&self, timestamp: u64, threshold: f32) -> Vec<(KnowledgePoint, f32)> {
        let mut values: Vec<_> = self
            .mastery
            .values()
            .map(|m| (m.point, m.effective_score_at(timestamp, DEFAULT_DECAY_SECS)))
            .filter(|(_, s)| *s < threshold)
            .collect();
        values.sort_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then_with(|| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)))
        });
        values
    }
    pub fn summary_at(&self, timestamp: u64) -> String {
        if self.mastery.is_empty() {
            return "知识点掌握度：暂无学习记录\n".into();
        }
        let mut rows: Vec<_> = self.mastery.values().collect();
        rows.sort_by_key(|m| format!("{:?}", m.point));
        let mut out = String::from("知识点掌握度：\n");
        for m in rows {
            let score = m.effective_score_at(timestamp, DEFAULT_DECAY_SECS);
            let n = ((score * 20.).round() as usize).min(20);
            out.push_str(&format!(
                "  {:<18}: [{}{}] {:>3}% (上次练习: {} 天前)\n",
                knowledge_point_text(m.point),
                "#".repeat(n),
                "-".repeat(20 - n),
                (score * 100.).round() as u8,
                timestamp.saturating_sub(m.last_seen) / 86_400
            ));
        }
        let weak: Vec<_> = self
            .weak_points_at(timestamp, 0.6)
            .into_iter()
            .map(|(p, _)| knowledge_point_text(p))
            .collect();
        out.push_str(&format!(
            "薄弱点: {}\n",
            if weak.is_empty() {
                "暂无".into()
            } else {
                weak.join(", ")
            }
        ));
        out
    }
    pub fn prompt_summary_at(&self, timestamp: u64) -> String {
        let weak: Vec<_> = self
            .weak_points_at(timestamp, 0.6)
            .into_iter()
            .map(|(p, s)| format!("{}({:.0}%)", knowledge_point_text(p), s * 100.0))
            .collect();
        if weak.is_empty() {
            "学习画像：目前没有明确薄弱点。".into()
        } else {
            format!(
                "学习画像：薄弱点为 {}。请优先引导理解，不要直接给答案。",
                weak.join("、")
            )
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| PadaError::Parse(format!("序列化学习画像失败: {e}")))?;
        std::fs::write(path, json).map_err(|e| PadaError::Config(format!("写入学习画像失败: {e}")))
    }
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| PadaError::Config(format!("读取学习画像失败: {e}")))?;
        serde_json::from_str(&text).map_err(|e| PadaError::Parse(format!("解析学习画像失败: {e}")))
    }
}
pub fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
