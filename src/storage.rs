//! PADA 用户数据的统一存储目录。

use crate::error::{PadaError, Result};
use crate::history::Session;
use std::path::{Path, PathBuf};

pub const MAX_AUTO_SESSIONS: usize = 20;

#[derive(Debug, Clone)]
pub struct DataStore {
    root: PathBuf,
}

#[derive(Debug)]
pub struct StoredSession {
    pub path: PathBuf,
    pub session: Session,
}

impl DataStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 优先级：CLI `--data-dir` > `PADA_HOME` > `~/.pada` > 当前目录 `.pada`。
    pub fn discover(cli_root: Option<PathBuf>) -> Result<Self> {
        let root = cli_root
            .or_else(|| std::env::var_os("PADA_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".pada")))
            .unwrap_or(std::env::current_dir()?.join(".pada"));
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()?.join(root)
        };
        Ok(Self::new(root))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn reports_dir(&self) -> PathBuf {
        self.root.join("reports")
    }

    pub fn auto_sessions_dir(&self) -> PathBuf {
        self.root.join("sessions").join("auto")
    }

    pub fn exported_sessions_dir(&self) -> PathBuf {
        self.root.join("sessions").join("exported")
    }

    pub fn learning_profile_path(&self) -> PathBuf {
        self.root.join("learning").join("profile.json")
    }

    pub fn report_path(&self, requested: &Path) -> PathBuf {
        self.reports_dir()
            .join(safe_file_name(requested, "report.md"))
    }

    pub fn exported_session_path(&self, requested: &Path) -> PathBuf {
        self.exported_sessions_dir()
            .join(safe_file_name(requested, "session.json"))
    }

    pub fn auto_session_path(&self, session: &Session) -> PathBuf {
        self.auto_sessions_dir()
            .join(format!("{}.json", session.id))
    }

    pub fn save_report(&self, requested: &Path, markdown: &str) -> Result<PathBuf> {
        let path = self.report_path(requested);
        create_parent(&path)?;
        std::fs::write(&path, markdown)
            .map_err(|e| PadaError::Config(format!("写入报告失败: {e}")))?;
        Ok(path)
    }

    pub fn export_session(&self, requested: &Path, session: &Session) -> Result<PathBuf> {
        let path = self.exported_session_path(requested);
        create_parent(&path)?;
        session.save(&path)?;
        Ok(path)
    }

    pub fn save_auto_session(&self, session: &Session) -> Result<PathBuf> {
        let path = self.auto_session_path(session);
        create_parent(&path)?;
        session.save(&path)?;
        self.prune_auto_sessions(MAX_AUTO_SESSIONS)?;
        Ok(path)
    }

    pub fn recent_sessions(&self) -> Result<Vec<StoredSession>> {
        let dir = self.auto_sessions_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| PadaError::Config(format!("读取自动会话目录失败: {e}")))?;
        let mut sessions = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            if let Ok(session) = Session::load(&path) {
                sessions.push(StoredSession { path, session });
            }
        }
        sessions.sort_by(|a, b| {
            b.session
                .updated_at
                .cmp(&a.session.updated_at)
                .then_with(|| b.session.id.cmp(&a.session.id))
        });
        Ok(sessions)
    }

    fn prune_auto_sessions(&self, keep: usize) -> Result<()> {
        let sessions = self.recent_sessions()?;
        for old in sessions.into_iter().skip(keep) {
            std::fs::remove_file(&old.path).map_err(|e| {
                PadaError::Config(format!("清理旧自动会话 {} 失败: {e}", old.path.display()))
            })?;
        }
        Ok(())
    }
}

fn safe_file_name(requested: &Path, fallback: &str) -> PathBuf {
    requested
        .file_name()
        .filter(|name| !name.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(fallback))
}

fn create_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| PadaError::Config("存储路径缺少父目录".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| PadaError::Config(format!("创建存储目录 {} 失败: {e}", parent.display())))
}
