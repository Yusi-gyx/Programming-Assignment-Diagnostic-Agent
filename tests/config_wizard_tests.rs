use pada::config::model::Config;
use pada::config::wizard::run_config_wizard;
use std::io::Cursor;

#[test]
fn wizard_creates_and_activates_a_profile() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/config.toml");
    let input = b"1\n2\nmy-local\nhttp://localhost:11434\n\nllama3\n\ny\n\n\ny\n";
    let mut reader = Cursor::new(input);
    let mut output = Vec::new();

    let result = run_config_wizard(&mut reader, &mut output, &path)
        .unwrap()
        .unwrap();
    assert_eq!(result.profile_name, "my-local");
    assert_eq!(result.model.model_name, "llama3");
    assert_eq!(
        result.model.endpoint,
        "http://localhost:11434/v1/chat/completions"
    );
    assert!(result.model.reasoning);
    assert!(path.exists());

    let saved = Config::load(&path).unwrap();
    assert_eq!(saved.active_profile, "my-local");
    assert_eq!(saved.active().unwrap().model_name, "llama3");
    let rendered = String::from_utf8(output).unwrap();
    assert!(rendered.contains("PADA 模型配置向导"));
    assert!(rendered.contains("配置已保存并启用"));
}

#[test]
fn wizard_switches_an_existing_profile_without_reentering_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let config = Config::default_template();
    config.save(&path).unwrap();
    let profiles = config.profile_names();
    let target_index = profiles.iter().position(|name| name == "deepseek").unwrap() + 2;
    let mut reader = Cursor::new(format!("{target_index}\n"));
    let mut output = Vec::new();

    let result = run_config_wizard(&mut reader, &mut output, &path)
        .unwrap()
        .unwrap();
    assert_eq!(result.profile_name, "deepseek");
    assert_eq!(Config::load(&path).unwrap().active_profile, "deepseek");
}

#[test]
fn wizard_can_be_cancelled_without_creating_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let mut reader = Cursor::new(b"0\n");
    let mut output = Vec::new();
    assert!(
        run_config_wizard(&mut reader, &mut output, &path)
            .unwrap()
            .is_none()
    );
    assert!(!path.exists());
}
