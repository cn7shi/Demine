use std::{fs, process::Command};

#[test]
fn once_scan_reports_errors_with_a_nonzero_exit_and_preserves_json_report() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let sessions = root.path().join("sessions");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&sessions).unwrap();
    fs::write(sessions.join("invalid.jsonl"), "{}\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_demine"))
        .arg("--project")
        .arg(&project)
        .arg("--sessions-dir")
        .arg(&sessions)
        .arg("--once")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["errors"].as_array().unwrap().len(), 1);
    fs::remove_file(sessions.join("invalid.jsonl")).unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_demine"))
            .arg("--project")
            .arg(project)
            .arg("--sessions-dir")
            .arg(sessions)
            .arg("--once")
            .output()
            .unwrap()
            .status
            .success()
    );
}
