//! 会话导出文件名冲突检测与交互式选择。

use crate::error::{PadaError, Result};
use crate::storage::DataStore;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// 非交互模式下解析导出目标；已有文件不会被静默覆盖。
pub fn available_export_target(store: &DataStore, requested: &Path) -> Result<PathBuf> {
    let target = store.exported_session_path(requested);
    if target.exists() {
        return Err(PadaError::Config(format!(
            "导出文件已存在: {}。请更换 --save 文件名，或在导师模式中确认覆盖",
            target.display()
        )));
    }
    Ok(target)
}

/// 交互式解析导出目标。重名时询问是否覆盖；拒绝后继续要求新文件名。
pub fn choose_export_target<R: BufRead, W: Write>(
    store: &DataStore,
    requested: &Path,
    reader: &mut R,
    writer: &mut W,
) -> Result<PathBuf> {
    let mut requested = requested.to_owned();
    loop {
        let target = store.exported_session_path(&requested);
        if !target.exists() {
            return Ok(target);
        }

        writeln!(writer, "导出文件已存在: {}", target.display())?;
        loop {
            write!(writer, "是否覆盖已有文件？[y/N]: ")?;
            writer.flush()?;
            let answer = read_line(reader)?;
            match answer.to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(target),
                "" | "n" | "no" => break,
                _ => writeln!(writer, "请输入 y 或 n。")?,
            }
        }

        loop {
            write!(writer, "请输入新的导出文件名: ")?;
            writer.flush()?;
            let name = read_line(reader)?;
            if name.is_empty() {
                writeln!(writer, "文件名不能为空。")?;
                continue;
            }
            requested = PathBuf::from(name);
            break;
        }
    }
}

fn read_line<R: BufRead>(reader: &mut R) -> Result<String> {
    let mut value = String::new();
    if reader.read_line(&mut value)? == 0 {
        return Err(PadaError::Config("导出文件名输入提前结束".into()));
    }
    Ok(value.trim().to_owned())
}
