//! Deterministic generator for the source-derived frozen contracts.
//!
//! Repository-root `contracts/` is the mandatory authority. `--check`
//! regenerates every artifact in memory and fails on missing or byte-different
//! files; generated JSON is minified, key-sorted UTF-8 with no trailing newline.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::Value;

const CHANGE_CONTROL: &str = "owner sign-off + evidence update in the same change";
const CORPUS_RELATIVE: &str = "crates/nano-protocol/corpus/wayland-desktop-core/v1";

struct Target {
    path: PathBuf,
    bytes: Vec<u8>,
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::String(value) => {
            assert!(value.is_ascii(), "contract strings must be NFC-safe ASCII");
            Value::String(value)
        }
        other => other,
    }
}

fn render(artifact: &str, body: Value) -> Vec<u8> {
    let mut object = body
        .as_object()
        .expect("contract body is an object")
        .clone();
    object.insert(
        "$schema".into(),
        Value::String(format!("wayland-nano/contracts/{artifact}/v1")),
    );
    object.insert("artifact".into(), Value::String(artifact.into()));
    object.insert("changeControl".into(), Value::String(CHANGE_CONTROL.into()));
    object.insert("frozen".into(), Value::String("2026-08-15".into()));
    serde_json::to_vec(&canonicalize(Value::Object(object))).expect("serialize contract")
}

fn count_files(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .unwrap_or_else(|err| panic!("read corpus directory {}: {err}", path.display()))
        .map(|entry| entry.expect("read corpus entry").path())
        .map(|path| if path.is_dir() { count_files(&path) } else { 1 })
        .sum()
}

fn event_types(repo_root: &Path) -> Value {
    let corpus = repo_root.join(CORPUS_RELATIVE);
    let manifest_path = corpus.join("manifest.json");
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display())),
    )
    .expect("parse corpus manifest");

    let sorted_types = |field: &str| {
        let mut types = manifest[field]
            .as_array()
            .unwrap_or_else(|| panic!("manifest {field} is an array"))
            .iter()
            .map(|entry| {
                entry["type"]
                    .as_str()
                    .unwrap_or_else(|| panic!("manifest {field} entry has string type"))
                    .to_owned()
            })
            .collect::<Vec<_>>();
        types.sort();
        types.dedup();
        types
    };
    let command_types = sorted_types("commands");
    let event_types = sorted_types("events");
    let command_count = count_files(&corpus.join("commands"));
    let event_count = count_files(&corpus.join("events"));
    let compat_count = count_files(&corpus.join("compat"));
    let adversarial_count = count_files(&corpus.join("adversarial"));
    let total = command_count + event_count + compat_count + adversarial_count;

    assert_eq!(command_types.len() as u64, command_count);
    assert_eq!(event_types.len() as u64, event_count);
    assert_eq!(manifest["counts"]["commands"].as_u64(), Some(command_count));
    assert_eq!(manifest["counts"]["events"].as_u64(), Some(event_count));
    assert_eq!(manifest["counts"]["fixtures"].as_u64(), Some(total));

    serde_json::json!({
        "commandTypes": command_types,
        "corpus": {
            "fixtureCounts": {
                "adversarial": adversarial_count,
                "commands": command_count,
                "compat": compat_count,
                "events": event_count
            },
            "name": manifest["contract"]["name"].clone(),
            "version": format!("v{}", manifest["contract"]["major"].as_u64().expect("contract major"))
        },
        "eventTypes": event_types
    })
}

fn targets() -> Vec<Target> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("repository root");
    let contracts = repo_root.join("contracts");

    let capability = render(
        "capability-profile",
        serde_json::json!({ "profile": nano_protocol::profile::v1_capabilities() }),
    );
    let journal = render(
        "journal-semantics",
        serde_json::json!({
            "format": {
                "appendSync": "sync_data_per_append",
                "encoding": "ndjson",
                "envelopeFields": ["v", "id", "ts", "op"],
                "schemaVersion": nano_session::op::SCHEMA_VERSION
            },
            "invariants": [
                "append_only",
                "torn_tail_truncate_at_open",
                "single_writer_ownership"
            ],
            "opVocabulary": nano_session::op::OP_VOCABULARY
        }),
    );
    let events = render("event-types", event_types(repo_root));

    vec![
        Target {
            path: contracts.join("capability-profile.json"),
            bytes: capability,
        },
        Target {
            path: contracts.join("journal-semantics.json"),
            bytes: journal,
        },
        Target {
            path: contracts.join("event-types.json"),
            bytes: events,
        },
    ]
}

fn main() -> ExitCode {
    let check = std::env::args().any(|arg| arg == "--check");
    let mut failed = false;
    for target in targets() {
        if check {
            match std::fs::read(&target.path) {
                Ok(existing) if existing == target.bytes => {
                    println!("ok: {}", target.path.display());
                }
                Ok(_) => {
                    eprintln!("STALE: {} — rerun gen_contracts", target.path.display());
                    failed = true;
                }
                Err(err) => {
                    eprintln!(
                        "MISSING: {} ({err}) — run gen_contracts",
                        target.path.display()
                    );
                    failed = true;
                }
            }
        } else {
            if let Some(parent) = target.path.parent() {
                std::fs::create_dir_all(parent).expect("create contracts directory");
            }
            std::fs::write(&target.path, &target.bytes).expect("write contract");
            println!("wrote: {}", target.path.display());
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
