use pada::agent::export::{available_export_target, choose_export_target};
use pada::storage::DataStore;
use std::io::Cursor;
use std::path::Path;

#[test]
fn unused_name_needs_no_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().join("data"));
    let mut input = Cursor::new(Vec::<u8>::new());
    let mut output = Vec::new();

    let target =
        choose_export_target(&store, Path::new("lesson.json"), &mut input, &mut output).unwrap();
    assert_eq!(target, store.exported_sessions_dir().join("lesson.json"));
    assert!(output.is_empty());
}

#[test]
fn existing_name_can_be_overwritten_after_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().join("data"));
    std::fs::create_dir_all(store.exported_sessions_dir()).unwrap();
    let existing = store.exported_sessions_dir().join("lesson.json");
    std::fs::write(&existing, "old").unwrap();
    let mut input = Cursor::new(b"y\n");
    let mut output = Vec::new();

    let target =
        choose_export_target(&store, Path::new("lesson.json"), &mut input, &mut output).unwrap();
    assert_eq!(target, existing);
    assert!(String::from_utf8(output).unwrap().contains("是否覆盖"));
}

#[test]
fn declining_overwrite_requests_another_name_and_checks_it_again() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().join("data"));
    std::fs::create_dir_all(store.exported_sessions_dir()).unwrap();
    for name in ["lesson.json", "also-used.json"] {
        std::fs::write(store.exported_sessions_dir().join(name), "old").unwrap();
    }
    let mut input = Cursor::new(b"n\nalso-used.json\nn\nfresh.json\n");
    let mut output = Vec::new();

    let target =
        choose_export_target(&store, Path::new("lesson.json"), &mut input, &mut output).unwrap();
    assert_eq!(target, store.exported_sessions_dir().join("fresh.json"));
    let output = String::from_utf8(output).unwrap();
    assert_eq!(output.matches("是否覆盖已有文件").count(), 2);
}

#[test]
fn noninteractive_export_refuses_to_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let store = DataStore::new(temp.path().join("data"));
    std::fs::create_dir_all(store.exported_sessions_dir()).unwrap();
    std::fs::write(store.exported_sessions_dir().join("lesson.json"), "old").unwrap();

    let error = available_export_target(&store, Path::new("lesson.json")).unwrap_err();
    assert!(error.to_string().contains("导出文件已存在"));
}
