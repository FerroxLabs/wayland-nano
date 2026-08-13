use nano_core::execrules::{ShellGrammar, TokenizeOutcome, tokenize};
use pretty_assertions::assert_eq;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

static FIXTURE: OnceLock<PathBuf> = OnceLock::new();

#[cfg(windows)]
#[test]
fn clean_cmd_corpus_matches_real_cmd_exe() {
    let shell = find_command("cmd.exe").expect("Windows rule promotion requires real cmd.exe");
    let fixture = build_fixture();
    let fixture_word = clean_path_word(&fixture, ShellGrammar::CmdExe);
    let corpus = [
        fixture_word.to_string(),
        format!("{fixture_word} alpha beta"),
        format!(r"{fixture_word} C:\work\file.txt /flag:value key=value a/b +x @file"),
        format!("{fixture_word} left && {fixture_word} right"),
        format!("{fixture_word} fail || {fixture_word} recovered"),
        format!("{fixture_word} first & {fixture_word} second"),
        format!("{fixture_word} producer | {fixture_word} consumer"),
    ];
    for command in corpus {
        assert_shell_observation(&shell, ShellGrammar::CmdExe, &command, &["/d", "/s", "/c"]);
    }
}

#[cfg(unix)]
#[test]
fn clean_posix_corpus_matches_real_sh() {
    let shell = find_command("sh").expect("POSIX rule promotion requires real sh");
    let fixture = build_fixture();
    let fixture_word = clean_path_word(&fixture, ShellGrammar::PosixSh);
    let corpus = [
        fixture_word.to_string(),
        format!("{fixture_word} alpha beta"),
        format!("{fixture_word} /tmp/file a,b key:value key=value +x @file"),
        format!("{fixture_word} 'hello world'"),
        format!("{fixture_word} left && {fixture_word} right"),
        format!("{fixture_word} fail || {fixture_word} recovered"),
        format!("{fixture_word} first; {fixture_word} second"),
        format!("{fixture_word} producer | {fixture_word} consumer"),
    ];
    for command in corpus {
        assert_shell_observation(&shell, ShellGrammar::PosixSh, &command, &["-c"]);
    }
}

fn assert_shell_observation(
    shell: &Path,
    grammar: ShellGrammar,
    command: &str,
    shell_args: &[&str],
) {
    let TokenizeOutcome::Clean(expected) = tokenize(command, grammar) else {
        panic!("differential corpus entry was not Clean: {command:?}");
    };
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("argv.log");
    let status = Command::new(shell)
        .args(shell_args)
        .arg(command)
        .env("NANO_ARGV_ECHO_LOG", &log)
        .env("MSYS2_ARG_CONV_EXCL", "*")
        .status()
        .unwrap_or_else(|error| panic!("run {} for {command:?}: {error}", shell.display()));
    assert!(
        status.success(),
        "{} failed for {command:?}: {status}",
        shell.display()
    );
    let mut observed = fs::read_to_string(&log)
        .unwrap_or_else(|error| panic!("read argv oracle for {command:?}: {error}"))
        .lines()
        .map(|line| {
            if line.is_empty() {
                Vec::new()
            } else {
                line.split('\u{1f}').map(String::from).collect()
            }
        })
        .collect::<Vec<Vec<String>>>();
    let mut expected_args = expected
        .into_iter()
        .map(|segment| segment.into_iter().skip(1).collect())
        .collect::<Vec<Vec<String>>>();
    observed.sort();
    expected_args.sort();
    assert_eq!(
        observed, expected_args,
        "real-shell mismatch for {command:?}"
    );
}

fn build_fixture() -> PathBuf {
    FIXTURE
        .get_or_init(|| {
            // Canonicalize the temp root first: CI profiles use 8.3 aliases
            // (RUNNER~1) and macOS /var is a symlink to /private/var — both
            // introduce bytes (~) or symlinked spellings the Clean grammar
            // rejects. The canonical path is safe-set only.
            let output = dunce::canonicalize(std::env::temp_dir())
                .expect("canonical temp dir")
                .join(format!(
                    "wayland-nano-argv-echo-{}{}",
                    std::process::id(),
                    std::env::consts::EXE_SUFFIX
                ));
            let _ = fs::remove_file(&output);
            let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/argv_echo.rs");
            let status = Command::new("rustc")
                .args(["--edition=2024", "-o"])
                .arg(&output)
                .arg(&source)
                .status()
                .expect("invoke rustc for argv fixture");
            assert!(status.success(), "rustc failed for {}", source.display());
            output
        })
        .clone()
}

fn clean_path_word(path: &Path, grammar: ShellGrammar) -> String {
    let word = match grammar {
        ShellGrammar::PosixSh => path.to_string_lossy().replace('\\', "/"),
        ShellGrammar::CmdExe => path.to_string_lossy().into_owned(),
    };
    assert!(
        word.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || b"._/:@-".contains(&byte)
                || (grammar == ShellGrammar::CmdExe && byte == b'\\')
        }),
        "fixture path must fit the selected Clean grammar: {word}"
    );
    word
}

fn find_command(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}
