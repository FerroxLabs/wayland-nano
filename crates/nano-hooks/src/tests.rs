use super::*;
use std::fs;

fn load(text: &str) -> HookEngine {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("hooks.toml");
    fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    HookEngine::load(home.path())
}

#[test]
fn invalid_entry_rejects_entire_file() {
    let engine = load(
        r#"
[[hooks.PreToolUse]]
matcher = "Bash"
hooks = [{ command = "echo ok" }]
[[hooks.PostToolUse]]
matcher = "["
hooks = [{ command = "echo bad" }]
"#,
    );
    assert!(engine.is_empty());
    assert_eq!(engine.warnings().len(), 1);
}

#[test]
fn rejects_unknown_fields_events_async_blocking_and_timeout() {
    for text in [
        "[hooks]\nBogus = []",
        "[[hooks.PreToolUse]]\nhooks=[{command='x', async=true}]",
        "[[hooks.PostToolUse]]\nhooks=[{command='x', timeout_sec=0}]",
        "[[hooks.PostToolUse]]\nextra=true\nhooks=[{command='x'}]",
    ] {
        assert!(load(text).is_empty(), "{text}");
    }
}

#[test]
fn override_source_must_be_contained_and_not_a_symlink() {
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert!(HookEngine::load_path_strict(home.path(), outside.path()).is_err());

    let target = home.path().join("target.toml");
    fs::write(&target, "[hooks]").unwrap();
    let link = home.path().join("hooks-link.toml");
    #[cfg(windows)]
    if std::os::windows::fs::symlink_file(&target, &link).is_ok() {
        assert!(HookEngine::load_path_strict(home.path(), &link).is_err());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(HookEngine::load_path_strict(home.path(), &link).is_err());
    }
}

#[tokio::test]
async fn matcher_order_and_ignored_matcher_are_stable() {
    let engine = load(
        r#"
[[hooks.UserPromptSubmit]]
matcher = "["
hooks = [{ command = "exit 0" }, { command = "exit 0" }]
"#,
    );
    let run = engine
        .run(HookEvent::UserPromptSubmit, None, &serde_json::json!({}))
        .await;
    assert_eq!(run.decisions.len(), 2);
    assert!(run.decisions.iter().all(|d| d.outcome == HookOutcome::Pass));
}

#[tokio::test]
async fn blocking_invalid_output_fails_closed() {
    let command = if cfg!(windows) {
        "echo not-json"
    } else {
        "printf not-json"
    };
    let engine = load(&format!(
        "[[hooks.PreToolUse]]\nmatcher='.*'\nhooks=[{{command={command:?}}}]"
    ));
    let run = engine
        .run(HookEvent::PreToolUse, Some("Bash"), &serde_json::json!({}))
        .await;
    assert!(run.blocking_reason().is_some());
}
