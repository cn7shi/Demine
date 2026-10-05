use demine::{
    collector::{Collector, belongs_to_project, normalized_path},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    project: PathBuf,
    sessions: PathBuf,
    log: PathBuf,
    db: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let sessions = root.path().join("sessions");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        let log = sessions.join("session.jsonl");
        let db = root.path().join("test.sqlite3");
        let fixture = Self {
            _root: root,
            project,
            sessions,
            log,
            db,
        };
        fixture.append(json!({"type":"session_meta","payload":{"id":"s1","cwd":fixture.project,"cli_version":"0.160.0","base_instructions":"PRIVATE_INSTRUCTION"}}));
        fixture
    }
    fn append(&self, value: Value) {
        self.bytes(format!("{value}\n").as_bytes());
    }
    fn bytes(&self, bytes: &[u8]) {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    fn store(&self) -> Store {
        Store::open(&self.db, &normalized_path(&self.project)).unwrap()
    }
    fn collector(&self) -> Collector {
        Collector {
            project: self.project.clone(),
            sessions_root: self.sessions.clone(),
        }
    }
    fn user(&self, id: &str, content: &str) {
        self.append(json!({"timestamp":"2026-10-05T10:00:00Z","type":"response_item","payload":{"type":"message","id":id,"role":"user","content":[{"type":"input_text","text":content}]}}));
    }
}

#[test]
fn restart_and_repeated_scan_are_idempotent() {
    let f = Fixture::new();
    f.user("u1", "修复屏幕适配");
    let mut store = f.store();
    let collector = f.collector();
    assert_eq!(collector.scan(&mut store).saved_records, 2);
    assert_eq!(collector.scan(&mut store).saved_records, 0);
    drop(store);
    let mut store = f.store();
    assert_eq!(collector.scan(&mut store).saved_records, 0);
    f.user("u2", "继续检查横屏");
    collector.scan(&mut store);
    assert_eq!(store.events("s1", None, None, 100).unwrap().events.len(), 2);
    assert_eq!(store.sessions().unwrap()[0].title, "修复屏幕适配");
}

#[test]
fn half_line_waits_and_unicode_survives_resume() {
    let f = Fixture::new();
    let line=json!({"type":"response_item","payload":{"type":"message","role":"user","id":"u1","content":"中文内容"}}).to_string();
    let split = line.find('中').unwrap() + 1;
    f.bytes(&line.as_bytes()[..split]);
    let mut store = f.store();
    let report = f.collector().scan(&mut store);
    assert_eq!(report.pending_lines, 1);
    assert!(
        store
            .events("s1", None, None, 100)
            .unwrap()
            .events
            .is_empty()
    );
    drop(store);
    f.bytes(&line.as_bytes()[split..]);
    f.bytes(b"\n");
    let mut store = f.store();
    f.collector().scan(&mut store);
    assert_eq!(
        store.events("s1", None, None, 100).unwrap().events[0].output,
        "中文内容"
    );
}

#[test]
fn source_is_read_only_and_foreign_projects_are_excluded() {
    let f = Fixture::new();
    f.user("u1", "hello");
    let original = fs::read(&f.log).unwrap();
    let other = f.project.with_file_name("project-other");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        f.sessions.join("foreign.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"foreign","cwd":other}}),
            json!({"type":"foreign-secret"})
        ),
    )
    .unwrap();
    let mut store = f.store();
    let report = f.collector().scan(&mut store);
    assert_eq!(report.matched_files, 1);
    assert_eq!(store.sessions().unwrap().len(), 1);
    assert_eq!(fs::read(&f.log).unwrap(), original);
    let child = f.project.join("src");
    fs::create_dir_all(&child).unwrap();
    assert!(belongs_to_project(&f.project, &child));
    assert!(!belongs_to_project(&f.project, &other));
    assert!(!belongs_to_project(
        &f.project,
        std::path::Path::new("relative")
    ));
}

#[test]
fn missing_descendant_keeps_canonical_project_boundary() {
    let f = Fixture::new();
    let separate_project = f._root.path().join("separate-project");
    fs::create_dir(&separate_project).unwrap();
    let alias = separate_project.join("project-alias");
    // A junction needs no developer-mode/symlink privilege on Windows runners.
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&alias)
            .arg(&f.project)
            .output()
            .unwrap();
        assert!(output.status.success(), "junction creation: {output:?}");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&f.project, &alias).unwrap();

    let missing = alias.join("removed").join("src");
    assert!(!missing.exists());
    assert!(belongs_to_project(&f.project, &missing));
    assert_eq!(
        normalized_path(&missing),
        normalized_path(&f.project.join("removed").join("src"))
    );
    assert!(!belongs_to_project(
        &f.project,
        &alias.join("..").join("outside").join("src")
    ));
    // A path lexically inside the alias's parent must not enter that parent's
    // scope if its existing alias actually resolves to another project.
    assert!(!belongs_to_project(&separate_project, &missing));
}

#[test]
fn interleaved_sessions_keep_their_own_cursors() {
    let f = Fixture::new();
    f.user("u1", "first");
    let second = f.sessions.join("nested").join("s2.jsonl");
    fs::create_dir_all(second.parent().unwrap()).unwrap();
    fs::write(&second,format!("{}\n{}\n",json!({"type":"session_meta","payload":{"id":"s2","cwd":f.project.join("src")}}),json!({"type":"response_item","payload":{"type":"message","role":"user","id":"u2","content":"second"}}))).unwrap();
    let mut store = f.store();
    f.collector().scan(&mut store);
    f.user("u3", "continue first");
    f.collector().scan(&mut store);
    assert_eq!(store.sessions().unwrap().len(), 2);
    assert_eq!(store.events("s1", None, None, 100).unwrap().events.len(), 2);
    assert_eq!(store.events("s2", None, None, 100).unwrap().events.len(), 1);
}

#[test]
fn malformed_and_unknown_records_do_not_block_following_evidence() {
    let f = Fixture::new();
    f.bytes(b"this is not json\n");
    f.append(json!({"type":"future_event","payload":{"evidence":"retained"}}));
    f.user("u1", "after errors");
    let mut store = f.store();
    f.collector().scan(&mut store);
    let events = store.events("s1", None, None, 100).unwrap().events;
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].kind, "warning");
    assert_eq!(events[1].kind, "unknown");
    assert_eq!(events[2].output, "after errors");
    assert!(
        store.evidence(events[1].id).unwrap()[0]
            .raw
            .contains("retained")
    );
    assert_eq!(store.sessions().unwrap()[0].warning_count, 2);
}

#[test]
fn same_length_rewrite_starts_a_new_generation_without_erasing_history() {
    let f = Fixture::new();
    f.user("u1", "version-A");
    let mut store = f.store();
    f.collector().scan(&mut store);
    let before = fs::read_to_string(&f.log).unwrap();
    let after = before.replace("version-A", "version-B");
    assert_eq!(before.len(), after.len());
    fs::write(&f.log, after).unwrap();
    let report = f.collector().scan(&mut store);
    assert_eq!(report.rewritten_files, 1);
    let events = store.events("s1", None, None, 100).unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].generation, 0);
    assert_eq!(events[1].generation, 1);
    assert_eq!(events[0].output, "version-A");
    assert_eq!(events[1].output, "version-B");
    assert_eq!(f.collector().scan(&mut store).saved_records, 0);
}

#[test]
fn truncation_preserves_old_records_and_restarts_at_header() {
    let f = Fixture::new();
    f.user("u1", "old content");
    let mut store = f.store();
    f.collector().scan(&mut store);
    let content = fs::read_to_string(&f.log).unwrap();
    let header = content.lines().next().unwrap();
    fs::write(&f.log, format!("{header}\n")).unwrap();
    assert_eq!(f.collector().scan(&mut store).rewritten_files, 1);
    f.user("u2", "new");
    f.collector().scan(&mut store);
    let events = store.events("s1", None, None, 100).unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].generation, 1);
}

#[test]
fn tool_result_updates_existing_event_and_delta_returns_the_revision() {
    let f = Fixture::new();
    f.append(json!({"type":"turn_context","payload":{"turn_id":"turn-one","developer_instructions":"SECRET"}}));
    f.append(json!({"type":"response_item","payload":{"type":"function_call","call_id":"call-1","name":"shell","arguments":"cargo test"}}));
    let mut store = f.store();
    f.collector().scan(&mut store);
    let initial = store.events("s1", None, None, 100).unwrap();
    assert_eq!(initial.events[0].status, "started");
    drop(store);
    f.append(json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"tests passed"}}));
    let mut store = f.store();
    f.collector().scan(&mut store);
    let delta = store.events("s1", Some(initial.cursor), None, 100).unwrap();
    assert_eq!(delta.events.len(), 1);
    assert_eq!(delta.events[0].id, initial.events[0].id);
    assert_eq!(delta.events[0].input, "cargo test");
    assert_eq!(delta.events[0].output, "tests passed");
    assert_eq!(delta.events[0].turn_id.as_deref(), Some("turn-one"));
    assert_eq!(store.evidence(delta.events[0].id).unwrap().len(), 2);
}

#[test]
fn identical_commands_with_different_ids_are_distinct_attempts() {
    let f = Fixture::new();
    for id in ["a", "b"] {
        f.append(json!({"type":"response_item","payload":{"type":"function_call","call_id":id,"name":"shell","arguments":"same command"}}));
    }
    let mut store = f.store();
    f.collector().scan(&mut store);
    assert_eq!(store.events("s1", None, None, 100).unwrap().events.len(), 2);
}

#[test]
fn modern_and_transport_assistant_messages_share_one_entry_and_keep_both_sources() {
    let f = Fixture::new();
    f.append(json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"m1","content":"done"}}}));
    f.append(json!({"type":"response_item","payload":{"type":"message","id":"m1","role":"assistant","content":"done"}}));
    let mut store = f.store();
    f.collector().scan(&mut store);
    let events = store.events("s1", None, None, 100).unwrap().events;
    assert_eq!(events.len(), 1);
    assert_eq!(store.evidence(events[0].id).unwrap().len(), 2);
}

#[test]
fn late_matching_native_user_record_hides_its_nearby_transport_representation() {
    let f = Fixture::new();
    f.user("response-id", "same input");
    let mut store = f.store();
    f.collector().scan(&mut store);
    let initial = store.events("s1", None, None, 100).unwrap();
    f.append(json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"different-id","content":"same input"}}}));
    f.collector().scan(&mut store);
    let page = store.events("s1", None, None, 100).unwrap();
    assert_eq!(page.events.len(), 1);
    assert!(page.hidden_ids.contains(&initial.events[0].id));
}

#[test]
fn internal_instructions_and_reasoning_are_not_copied() {
    let f = Fixture::new();
    f.append(json!({"type":"response_item","payload":{"type":"reasoning","encrypted_content":"SECRET_REASONING"}}));
    f.append(json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","raw_content":"SECRET_REASONING"}}}));
    f.append(json!({"type":"response_item","payload":{"type":"message","role":"system","content":"SECRET_SYSTEM"}}));
    f.append(json!({"type":"turn_context","payload":{"turn_id":"t1","developer_instructions":"SECRET_CONTEXT"}}));
    let mut store = f.store();
    f.collector().scan(&mut store);
    assert!(
        store
            .events("s1", None, None, 100)
            .unwrap()
            .events
            .is_empty()
    );
    let db = rusqlite::Connection::open(&f.db).unwrap();
    let mut stmt = db.prepare("SELECT raw FROM records").unwrap();
    for raw in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        let raw = raw.unwrap();
        assert!(!raw.contains("SECRET"));
        assert!(!raw.contains("PRIVATE_INSTRUCTION"));
    }
}

#[test]
fn command_exit_and_turn_completion_are_not_problem_resolution() {
    let f = Fixture::new();
    for (id, exit) in [("c1", 1), ("c2", 0)] {
        f.append(json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":id,"command":"test","exit_code":exit,"aggregated_output":"observed output"}}}));
    }
    f.append(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"fixed"}}));
    let mut store = f.store();
    f.collector().scan(&mut store);
    let events = store.events("s1", None, None, 100).unwrap().events;
    assert_eq!(events[0].status, "failed");
    assert_eq!(events[1].status, "succeeded");
    assert_eq!(events[2].status, "completed");
    assert!(events[2].output.contains("不代表"));
    assert_eq!(store.sessions().unwrap()[0].failed_count, 1);
}

#[test]
fn pagination_returns_older_rows_and_incremental_pages_without_loss() {
    let f = Fixture::new();
    for index in 0..7 {
        f.user(&format!("u{index}"), &format!("input {index}"));
    }
    let mut store = f.store();
    f.collector().scan(&mut store);
    let latest = store.events("s1", None, None, 3).unwrap();
    assert_eq!(latest.events.len(), 3);
    assert!(latest.has_more);
    assert_eq!(latest.events[0].output, "input 4");
    let older = store
        .events("s1", None, Some(latest.events[0].id), 3)
        .unwrap();
    assert_eq!(older.events[0].output, "input 1");
    assert!(older.has_more);
    let mut cursor = 0;
    let mut all = Vec::new();
    loop {
        let page = store.events("s1", Some(cursor), None, 2).unwrap();
        cursor = page.cursor;
        all.extend(page.events.into_iter().map(|e| e.id));
        if !page.has_more {
            break;
        }
    }
    assert_eq!(all.len(), 7);
}

#[test]
fn database_cannot_be_reused_for_another_project() {
    let f = Fixture::new();
    let _store = f.store();
    assert!(Store::open(&f.db, "different-project").is_err());
}

#[test]
fn mixed_versions_and_repeated_user_inputs_are_not_silently_hidden() {
    let f = Fixture::new();
    f.user("old-input", "older message with no native representation");
    for i in 0..2 {
        f.user(&format!("response-{i}"), "repeat this request");
        f.append(json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":format!("native-{i}"),"content":"repeat this request"}}}));
    }
    let mut store = f.store();
    f.collector().scan(&mut store);
    let page = store.events("s1", None, None, 100).unwrap();
    assert_eq!(page.events.len(), 3);
    assert_eq!(
        page.events[0].output,
        "older message with no native representation"
    );
    assert_eq!(
        page.events
            .iter()
            .filter(|e| e.output == "repeat this request")
            .count(),
        2
    );
}

#[test]
fn user_input_before_next_turn_is_not_assigned_to_previous_turn() {
    let f = Fixture::new();
    f.append(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"previous"}}));
    f.user("u-new", "a new task");
    let mut store = f.store();
    f.collector().scan(&mut store);
    let events = store.events("s1", None, None, 100).unwrap().events;
    assert!(
        events
            .iter()
            .find(|e| e.kind == "user")
            .unwrap()
            .turn_id
            .is_none()
    );
}

#[test]
fn missing_source_and_invalid_header_are_reported() {
    let f = Fixture::new();
    fs::write(f.sessions.join("bad.jsonl"), "{}\n").unwrap();
    let mut store = f.store();
    assert_eq!(f.collector().scan(&mut store).errors.len(), 1);
    let collector = Collector {
        project: f.project.clone(),
        sessions_root: f.sessions.join("missing"),
    };
    assert!(!collector.scan(&mut store).errors.is_empty());
}

#[test]
fn compaction_keeps_only_a_marker_and_never_copies_opaque_internal_content() {
    let f = Fixture::new();
    f.append(
        json!({"timestamp":"2026-10-05T10:00:00Z","type":"response_item","payload":{
            "type":"compaction","id":"compact-1","encrypted_content":"PRIVATE_CIPHERTEXT",
            "internal_chat_message_metadata_passthrough":{"data":"PRIVATE_METADATA"}
        }}),
    );
    let mut store = f.store();
    f.collector().scan(&mut store);
    let events = store.events("s1", None, None, 100).unwrap().events;
    let evidence = store.evidence(events[0].id).unwrap();
    assert!(!evidence[0].raw.contains("PRIVATE"));
    assert_eq!(events[0].kind, "context");
    assert_eq!(events[0].status, "recorded");
    assert_eq!(evidence[0].line, 2);
}

#[test]
fn opening_v1_database_sanitizes_existing_compaction_without_losing_progress() {
    let f = Fixture::new();
    f.append(json!({"type":"response_item","payload":{"type":"compaction","id":"c1"}}));
    let mut store = f.store();
    f.collector().scan(&mut store);
    let original = store
        .events("s1", None, None, 100)
        .unwrap()
        .events
        .remove(0);
    drop(store);
    // Emulate a persisted v1 row, including fields that the old adapter copied.
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    conn.execute("UPDATE records SET raw=?1 WHERE record_type='response_item'", [
        json!({"type":"response_item","payload":{"type":"compaction","encrypted_content":"PRIVATE_OLD","internal_chat_message_metadata_passthrough":"PRIVATE_META"}}).to_string()
    ]).unwrap();
    conn.execute("UPDATE events SET kind='unknown',status='unknown'", [])
        .unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    drop(conn);
    let mut store = f.store();
    let updated = store
        .events("s1", None, None, 100)
        .unwrap()
        .events
        .remove(0);
    assert_eq!(updated.id, original.id);
    assert_eq!(updated.kind, "context");
    assert!(
        !store.evidence(updated.id).unwrap()[0]
            .raw
            .contains("PRIVATE")
    );
    assert_eq!(f.collector().scan(&mut store).saved_records, 0);
    drop(store);
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn explicit_native_failure_is_retained_without_an_exit_code() {
    let f = Fixture::new();
    for (id, kind) in [
        ("failed-command", "CommandExecution"),
        ("failed-patch", "FileChange"),
    ] {
        f.append(
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{
                "id":id,"type":kind,"status":"failed","command":"unavailable command",
                "changes":[],"aggregated_output":"unable to start"
            }}}),
        );
    }
    let mut store = f.store();
    f.collector().scan(&mut store);
    assert_eq!(store.sessions().unwrap()[0].failed_count, 2);
    assert!(
        store
            .events("s1", None, None, 100)
            .unwrap()
            .events
            .iter()
            .all(|e| e.status == "failed")
    );
}
