use crate::{
    adapter,
    store::{Checkpoint, Store},
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use walkdir::WalkDir;

#[derive(Debug, Default, Serialize, Clone)]
pub struct ScanReport {
    pub matched_files: usize,
    pub saved_records: usize,
    pub rewritten_files: usize,
    pub pending_lines: usize,
    pub errors: Vec<String>,
    pub finished_at_ms: u128,
}

pub struct Collector {
    pub project: PathBuf,
    pub sessions_root: PathBuf,
}

/// Path-component matching prevents `project-other` from entering `project`.
/// Canonicalization resolves the selected root; lexical fallback supports removed
/// source directories while still retaining an explicit project boundary.
pub fn normalized_path(path: &Path) -> String {
    let owned = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let mut path = owned.to_string_lossy().replace('\\', "/");
    if let Some(rest) = path.strip_prefix("//?/") {
        path = rest.to_owned();
    }
    if cfg!(windows) {
        path = path.to_lowercase();
    }
    let mut components = Vec::new();
    for part in path.split('/') {
        match part {
            "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(part),
        }
    }
    components.join("/").trim_end_matches('/').to_owned()
}

pub fn belongs_to_project(project: &Path, cwd: &Path) -> bool {
    if !cwd.is_absolute() {
        return false;
    }
    let project = normalized_path(project);
    let cwd = normalized_path(cwd);
    cwd == project || cwd.starts_with(&format!("{project}/"))
}

impl Collector {
    pub fn scan(&self, store: &mut Store) -> ScanReport {
        let mut report = ScanReport::default();
        for entry in WalkDir::new(&self.sessions_root).follow_links(false) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    if report.errors.len() < 20 {
                        report.errors.push(format!("无法扫描来源目录：{err}"));
                    }
                    continue;
                }
            };
            if !entry.file_type().is_file()
                || entry.path().extension().is_none_or(|ext| ext != "jsonl")
            {
                continue;
            }
            match self.read_file(entry.path(), store, &mut report) {
                Ok(()) => {}
                Err(error) => {
                    if report.errors.len() < 20 {
                        report
                            .errors
                            .push(format!("{}：{error:#}", entry.path().display()));
                    }
                }
            }
        }
        report.finished_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        report
    }

    fn read_file(&self, path: &Path, store: &mut Store, report: &mut ScanReport) -> Result<()> {
        let mut file = File::open(path).context("读取来源失败")?;
        let snapshot_len = file.metadata()?.len();
        if snapshot_len == 0 {
            return Ok(());
        }
        let mut header = String::new();
        // The first line can contain large base instructions; bound it independently.
        BufReader::new((&mut file).take(4 * 1024 * 1024)).read_line(&mut header)?;
        if !header.ends_with('\n') {
            report.pending_lines += 1;
            return Ok(());
        }
        let meta: Value =
            serde_json::from_str(&header).context("会话头无法解析，等待可识别的 session_meta")?;
        if meta["type"] != "session_meta" {
            bail!("缺少 session_meta，不能确定项目归属，未导入");
        }
        let payload = &meta["payload"];
        let cwd = payload["cwd"]
            .as_str()
            .context("会话头没有项目路径，未导入")?;
        if !belongs_to_project(&self.project, Path::new(cwd)) {
            return Ok(());
        }
        let id = payload["id"]
            .as_str()
            .or_else(|| payload["session_id"].as_str())
            .filter(|s| !s.is_empty())
            .context("会话头没有 ID")?;
        let version = payload["cli_version"].as_str().unwrap_or("未知");
        report.matched_files += 1;
        let source_path = path.to_string_lossy().into_owned();
        let previous = store.checkpoint(&source_path)?;
        let mut checkpoint = previous.clone().unwrap_or(Checkpoint {
            generation: 0,
            offset: 0,
            line: 0,
            digest: String::new(),
            turn_id: None,
        });
        let mut hasher = Sha256::new();
        file.seek(SeekFrom::Start(0))?;
        let mut remaining = checkpoint.offset.min(snapshot_len);
        let mut buffer = [0u8; 64 * 1024];
        while remaining > 0 {
            let to_read = remaining.min(buffer.len() as u64) as usize;
            let n = file.read(&mut buffer[..to_read])?;
            if n == 0 {
                bail!("来源在校验时被截断，将在下一次扫描重试");
            }
            hasher.update(&buffer[..n]);
            remaining -= n as u64;
        }
        if previous.is_some()
            && (checkpoint.offset > snapshot_len
                || format!("{:x}", hasher.clone().finalize()) != checkpoint.digest)
        {
            checkpoint = Checkpoint {
                generation: checkpoint.generation + 1,
                offset: 0,
                line: 0,
                digest: String::new(),
                turn_id: None,
            };
            hasher = Sha256::new();
            report.rewritten_files += 1;
        }
        if checkpoint.offset == snapshot_len && previous.is_some() {
            return Ok(());
        }
        file.seek(SeekFrom::Start(checkpoint.offset))?;
        // Freeze the scan boundary: an actively growing file must not monopolize ingestion.
        let mut reader = BufReader::new(file.take(snapshot_len - checkpoint.offset));
        let mut rows = Vec::new();
        loop {
            let mut bytes = Vec::new();
            let count = reader.read_until(b'\n', &mut bytes)?;
            if count == 0 {
                break;
            }
            if bytes.last() != Some(&b'\n') {
                report.pending_lines += 1;
                break;
            }
            let start = checkpoint.offset;
            checkpoint.line += 1;
            checkpoint.offset += count as u64;
            hasher.update(&bytes);
            let raw = String::from_utf8_lossy(&bytes);
            if let Some(parsed) = adapter::parse(
                raw.trim_end_matches(['\r', '\n']),
                checkpoint.line,
                checkpoint.turn_id.as_deref(),
            ) {
                if let Some(turn) = &parsed.turn_id {
                    checkpoint.turn_id = Some(turn.clone());
                }
                rows.push((checkpoint.line, start, parsed));
            }
        }
        checkpoint.digest = format!("{:x}", hasher.finalize());
        report.saved_records +=
            store.commit_batch(&source_path, id, cwd, version, &checkpoint, rows)?;
        Ok(())
    }
}
