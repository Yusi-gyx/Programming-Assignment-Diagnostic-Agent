use pada::history::Session;
use pada::storage::{DataStore, MAX_AUTO_SESSIONS};
use std::path::Path;

#[test]
fn stores_each_artifact_in_a_predictable_directory() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().join("pada-data"));
    let session = Session::new("所有权练习");

    let report = store
        .save_report(Path::new("my-report.md"), "# report")
        .unwrap();
    let exported = store
        .export_session(Path::new("my-session.json"), &session)
        .unwrap();
    let automatic = store.save_auto_session(&session).unwrap();

    assert_eq!(report, store.reports_dir().join("my-report.md"));
    assert_eq!(
        exported,
        store.exported_sessions_dir().join("my-session.json")
    );
    assert_eq!(automatic, store.auto_session_path(&session));
    assert!(report.exists() && exported.exists() && automatic.exists());
}

#[test]
fn ignores_requested_parent_directories_for_managed_exports() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().join("pada-data"));
    assert_eq!(
        store.report_path(Path::new("some/hard/to/find/report.md")),
        store.reports_dir().join("report.md")
    );
}

#[test]
fn automatic_history_keeps_only_the_latest_twenty_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().join("pada-data"));
    for index in 0..(MAX_AUTO_SESSIONS + 5) {
        let mut session = Session::new(format!("session {index}"));
        session.updated_at = index as u64;
        store.save_auto_session(&session).unwrap();
    }

    let recent = store.recent_sessions().unwrap();
    assert_eq!(recent.len(), MAX_AUTO_SESSIONS);
    assert_eq!(recent[0].session.title, "session 24");
    assert_eq!(recent.last().unwrap().session.title, "session 5");
}
