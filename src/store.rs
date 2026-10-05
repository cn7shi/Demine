use crate::adapter::{self, Parsed};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::Path;

pub struct Store {
    pub(crate) conn: Connection,
}

#[derive(Debug, Clone)]
pub(crate) struct Checkpoint {
    pub generation: i64,
    pub offset: u64,
    pub line: u64,
    pub digest: String,
    pub turn_id: Option<String>,
}

#[derive(Serialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub cli_version: String,
    pub updated_at: String,
    pub event_count: i64,
    pub failed_count: i64,
    pub warning_count: i64,
}

#[derive(Debug, Serialize)]
pub struct TimelineEvent {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub input: String,
    pub output: String,
    pub status: String,
    pub timestamp: String,
    pub turn_id: Option<String>,
    pub call_id: Option<String>,
    pub generation: i64,
    pub revision: i64,
}

#[derive(Serialize)]
pub struct EventPage {
    pub events: Vec<TimelineEvent>,
    pub cursor: i64,
    pub has_more: bool,
    pub hidden_ids: Vec<i64>,
}

#[derive(Serialize)]
pub struct Evidence {
    pub source: String,
    pub generation: i64,
    pub line: i64,
    pub byte_offset: i64,
    pub record_type: String,
    pub raw: String,
}

// Mixed-version sessions must retain messages that have no native counterpart.
// Only coalesce adjacent representations of the same message, near each other
// in the source. Repeated messages within the same stream remain separate.
const VISIBLE: &str =
    "NOT (e.kind IN ('user','assistant') AND e.priority<2 AND e.output<>'' AND EXISTS (
    SELECT 1 FROM events x JOIN records xr ON xr.id=x.record_id JOIN records er ON er.id=e.record_id
    WHERE x.source_path=e.source_path AND x.generation=e.generation AND x.kind=e.kind
    AND x.priority>e.priority AND x.output=e.output AND ABS(xr.line-er.line)<=4
    AND (e.kind<>'user' OR x.id>e.id)
    AND (x.turn_id IS NULL OR e.turn_id IS NULL OR x.turn_id=e.turn_id)
    AND NOT EXISTS (SELECT 1 FROM events z WHERE z.source_path=e.source_path
        AND z.generation=e.generation AND z.kind=e.kind
        AND z.id>MIN(e.id,x.id) AND z.id<MAX(e.id,x.id))))";

impl Store {
    pub fn open(path: &Path, project: &str) -> Result<Self> {
        let mut conn = Connection::open(path).context("打开本地数据库失败")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 2 {
            bail!("数据库版本比此程序新，请使用匹配版本，不能降级写入");
        }
        let tx = conn.transaction()?;
        tx.execute_batch("
            CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '新的会话', cwd TEXT NOT NULL,
                cli_version TEXT NOT NULL, updated_at TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS sources (
                path TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id),
                generation INTEGER NOT NULL, offset INTEGER NOT NULL, line INTEGER NOT NULL,
                digest TEXT NOT NULL, turn_id TEXT
            );
            CREATE TABLE IF NOT EXISTS records (
                id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES sessions(id),
                source_path TEXT NOT NULL, generation INTEGER NOT NULL, line INTEGER NOT NULL,
                byte_offset INTEGER NOT NULL, record_type TEXT NOT NULL, raw TEXT NOT NULL,
                event_key TEXT, UNIQUE(source_path,generation,line)
            );
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES sessions(id),
                source_path TEXT NOT NULL, generation INTEGER NOT NULL, event_key TEXT NOT NULL,
                kind TEXT NOT NULL, title TEXT NOT NULL, input TEXT NOT NULL, output TEXT NOT NULL,
                status TEXT NOT NULL, timestamp TEXT NOT NULL, turn_id TEXT, call_id TEXT,
                stream TEXT NOT NULL, priority INTEGER NOT NULL,
                record_id INTEGER NOT NULL REFERENCES records(id),
                UNIQUE(source_path,generation,event_key)
            );
            CREATE INDEX IF NOT EXISTS events_session ON events(session_id,id);
            CREATE INDEX IF NOT EXISTS events_revision ON events(session_id,record_id);
            CREATE INDEX IF NOT EXISTS events_representation ON events(source_path,generation,kind,stream);
            CREATE INDEX IF NOT EXISTS records_evidence ON records(source_path,generation,event_key);")?;
        tx.execute(
            "INSERT OR IGNORE INTO settings(key,value) VALUES('project',?1)",
            [project],
        )?;
        let stored: String =
            tx.query_row("SELECT value FROM settings WHERE key='project'", [], |r| {
                r.get(0)
            })?;
        if stored != project {
            bail!("这个数据库属于另一个项目，请为当前项目选择独立的数据目录");
        }
        if version < 2 {
            // The v1 fallback copied opaque compaction fields. Reuse the current
            // allowlist without resetting source offsets or duplicating history.
            let mut stmt = tx.prepare("SELECT id,line,raw FROM records WHERE record_type='response_item'
                AND CASE WHEN json_valid(raw) THEN json_extract(raw,'$.payload.type')='compaction' ELSE 0 END")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, line, raw) = row?;
                if let Some(parsed) = adapter::parse(&raw, line, None) {
                    tx.execute(
                        "UPDATE records SET raw=?2 WHERE id=?1",
                        params![id, parsed.raw],
                    )?;
                    if let Some(event) = parsed.event {
                        tx.execute("UPDATE events SET kind=?2,title=?3,output=?4,status=?5 WHERE record_id=?1",
                            params![id,event.kind,event.title,event.output,event.status])?;
                    }
                }
            }
        }
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
        Ok(Self { conn })
    }

    pub(crate) fn checkpoint(&self, path: &str) -> Result<Option<Checkpoint>> {
        Ok(self
            .conn
            .query_row(
                "SELECT generation,offset,line,digest,turn_id FROM sources WHERE path=?1",
                [path],
                |r| {
                    Ok(Checkpoint {
                        generation: r.get(0)?,
                        offset: r.get(1)?,
                        line: r.get(2)?,
                        digest: r.get(3)?,
                        turn_id: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// All rows and the cursor share a transaction: a crash cannot advance one alone.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_batch(
        &mut self,
        path: &str,
        session_id: &str,
        cwd: &str,
        version: &str,
        checkpoint: &Checkpoint,
        rows: Vec<(u64, u64, Parsed)>,
    ) -> Result<usize> {
        let count = rows.len();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO sessions(id,cwd,cli_version) VALUES(?1,?2,?3)
            ON CONFLICT(id) DO UPDATE SET cli_version=excluded.cli_version",
            params![session_id, cwd, version],
        )?;
        for (line, offset, parsed) in rows {
            let key = parsed.event.as_ref().map(|e| e.key.as_str());
            tx.execute("INSERT OR IGNORE INTO records(session_id,source_path,generation,line,byte_offset,record_type,raw,event_key)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![session_id,path,checkpoint.generation,line,offset,parsed.record_type,parsed.raw,key])?;
            let record_id: i64 = tx.query_row(
                "SELECT id FROM records WHERE source_path=?1 AND generation=?2 AND line=?3",
                params![path, checkpoint.generation, line],
                |r| r.get(0),
            )?;
            if let Some(e) = parsed.event {
                tx.execute("INSERT INTO events(session_id,source_path,generation,event_key,kind,title,input,output,status,timestamp,turn_id,call_id,stream,priority,record_id)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                    ON CONFLICT(source_path,generation,event_key) DO UPDATE SET
                        title=CASE WHEN excluded.priority>=events.priority AND excluded.title NOT LIKE '工具返回%' THEN excluded.title ELSE events.title END,
                        kind=CASE WHEN excluded.priority>=events.priority AND excluded.title NOT LIKE '工具返回%' THEN excluded.kind ELSE events.kind END,
                        input=CASE WHEN excluded.input<>'' THEN excluded.input ELSE events.input END,
                        output=CASE WHEN excluded.output<>'' AND excluded.priority>=events.priority THEN excluded.output ELSE events.output END,
                        status=CASE WHEN excluded.priority>=events.priority THEN excluded.status ELSE events.status END,
                        turn_id=COALESCE(excluded.turn_id,events.turn_id),
                        stream=CASE WHEN excluded.priority>events.priority THEN excluded.stream ELSE events.stream END,
                        priority=MAX(events.priority,excluded.priority),record_id=excluded.record_id",
                    params![session_id,path,checkpoint.generation,e.key,e.kind,e.title,e.input,e.output,e.status,e.timestamp,e.turn_id,e.call_id,e.stream,e.priority,record_id])?;
                if !e.timestamp.is_empty() {
                    tx.execute(
                        "UPDATE sessions SET updated_at=MAX(updated_at,?2) WHERE id=?1",
                        params![session_id, e.timestamp],
                    )?;
                }
            }
        }
        tx.execute("INSERT INTO sources(path,session_id,generation,offset,line,digest,turn_id) VALUES(?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(path) DO UPDATE SET session_id=excluded.session_id,generation=excluded.generation,offset=excluded.offset,
                line=excluded.line,digest=excluded.digest,turn_id=excluded.turn_id",
            params![path,session_id,checkpoint.generation,checkpoint.offset,checkpoint.line,checkpoint.digest,checkpoint.turn_id])?;
        let title: Option<String> = tx
            .query_row(
                &format!(
                    "SELECT output FROM events e WHERE session_id=?1 AND kind='user' AND {VISIBLE}
            ORDER BY id LIMIT 1"
                ),
                [session_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(title) = title {
            let title: String = title
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(72)
                .collect();
            if !title.is_empty() {
                tx.execute(
                    "UPDATE sessions SET title=?2 WHERE id=?1",
                    params![session_id, title],
                )?;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn sessions(&self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(&format!("SELECT s.id,s.title,s.cwd,s.cli_version,s.updated_at,
            COUNT(e.id),COALESCE(SUM(e.status='failed'),0),COALESCE(SUM(e.kind IN ('warning','unknown')),0)
            FROM sessions s LEFT JOIN events e ON e.session_id=s.id AND {VISIBLE}
            GROUP BY s.id ORDER BY s.updated_at DESC,s.id"))?;
        Ok(stmt
            .query_map([], |r| {
                Ok(Session {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    cwd: r.get(2)?,
                    cli_version: r.get(3)?,
                    updated_at: r.get(4)?,
                    event_count: r.get(5)?,
                    failed_count: r.get(6)?,
                    warning_count: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn events(
        &self,
        session_id: &str,
        after: Option<i64>,
        before: Option<i64>,
        limit: usize,
    ) -> Result<EventPage> {
        let limit = limit.clamp(1, 200);
        let filter = if after.is_some() {
            "e.record_id>?2 ORDER BY e.record_id ASC"
        } else {
            "e.id<?2 ORDER BY e.id DESC"
        };
        let boundary = after.or(before).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(&format!("SELECT e.id,e.kind,e.title,e.input,e.output,e.status,e.timestamp,e.turn_id,e.call_id,e.generation,e.record_id
            FROM events e WHERE e.session_id=?1 AND {VISIBLE} AND {filter} LIMIT ?3"))?;
        let mut events = stmt
            .query_map(params![session_id, boundary, limit + 1], |r| {
                Ok(TimelineEvent {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    title: r.get(2)?,
                    input: r.get(3)?,
                    output: r.get(4)?,
                    status: r.get(5)?,
                    timestamp: r.get(6)?,
                    turn_id: r.get(7)?,
                    call_id: r.get(8)?,
                    generation: r.get(9)?,
                    revision: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = events.len() > limit;
        events.truncate(limit);
        let cursor = if after.is_some() {
            events
                .last()
                .map(|e| e.revision)
                .unwrap_or(after.unwrap_or(0))
        } else {
            self.conn.query_row(
                "SELECT COALESCE(MAX(id),0) FROM records WHERE session_id=?1",
                [session_id],
                |r| r.get(0),
            )?
        };
        if after.is_none() {
            events.reverse();
        }
        let mut hidden = self.conn.prepare(&format!(
            "SELECT e.id FROM events e WHERE session_id=?1 AND NOT ({VISIBLE})"
        ))?;
        let hidden_ids = hidden
            .query_map([session_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(EventPage {
            events,
            cursor,
            has_more,
            hidden_ids,
        })
    }

    pub fn evidence(&self, event_id: i64) -> Result<Vec<Evidence>> {
        let mut stmt = self.conn.prepare("SELECT r.source_path,r.generation,r.line,r.byte_offset,r.record_type,r.raw
            FROM records r JOIN events e ON r.source_path=e.source_path AND r.generation=e.generation AND r.event_key=e.event_key
            WHERE e.id=?1 ORDER BY r.id LIMIT 100")?;
        Ok(stmt
            .query_map([event_id], |r| {
                Ok(Evidence {
                    source: r.get(0)?,
                    generation: r.get(1)?,
                    line: r.get(2)?,
                    byte_offset: r.get(3)?,
                    record_type: r.get(4)?,
                    raw: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
}
