use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

const CHANGE_CONTROL: &str = "owner sign-off + evidence update in the same change";
const FROZEN: &str = "2026-08-15";
const ARTIFACTS: [&str; 4] = [
    "capability-profile",
    "journal-semantics",
    "flux-endpoint-contract",
    "event-types",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

fn parse(bytes: &[u8], name: &str) -> Result<Value, String> {
    serde_json::from_slice(bytes).map_err(|err| format!("{name}: malformed JSON: {err}"))
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
        other => other,
    }
}

fn parse_canonical(bytes: &[u8], artifact: &str) -> Result<Value, String> {
    let value = parse(bytes, artifact)?;
    let canonical = serde_json::to_vec(&canonicalize(value.clone()))
        .map_err(|err| format!("{artifact}: canonical serialization failed: {err}"))?;
    if bytes != canonical {
        return Err(format!("{artifact}: bytes are not canonical"));
    }
    Ok(value)
}

fn load(root: &Path, artifact: &str) -> Result<Value, String> {
    let path = root.join("contracts").join(format!("{artifact}.json"));
    let bytes = std::fs::read(&path)
        .map_err(|err| format!("required root contract {}: {err}", path.display()))?;
    parse_canonical(&bytes, artifact)
}

fn common(value: &Value, artifact: &str) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{artifact}: document must be an object"))?;
    let string = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{artifact}: {key} must be a string"))
    };
    if string("$schema")? != format!("wayland-nano/contracts/{artifact}/v1") {
        return Err(format!("{artifact}: wrong schema identity"));
    }
    if string("artifact")? != artifact {
        return Err(format!("{artifact}: wrong artifact identity"));
    }
    if string("frozen")? != FROZEN {
        return Err(format!("{artifact}: wrong freeze date"));
    }
    if string("changeControl")? != CHANGE_CONTROL {
        return Err(format!("{artifact}: wrong change control"));
    }
    Ok(())
}

fn corpus_root(root: &Path) -> PathBuf {
    root.join("crates/nano-protocol/corpus/wayland-desktop-core/v1")
}

fn count_files(path: &Path) -> Result<u64, String> {
    std::fs::read_dir(path)
        .map_err(|err| format!("read corpus directory {}: {err}", path.display()))?
        .try_fold(0, |total, entry| {
            let path = entry
                .map_err(|err| format!("read corpus entry: {err}"))?
                .path();
            Ok(total
                + if path.is_dir() {
                    count_files(&path)?
                } else {
                    1
                })
        })
}

fn sorted_manifest_types(manifest: &Value, field: &str) -> Result<Vec<String>, String> {
    let mut types = manifest[field]
        .as_array()
        .ok_or_else(|| format!("manifest {field} must be an array"))?
        .iter()
        .map(|entry| {
            entry["type"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("manifest {field} entry lacks string type"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    types.sort();
    types.dedup();
    Ok(types)
}

fn string_array(value: &Value, key: &str, artifact: &str) -> Result<Vec<String>, String> {
    value[key]
        .as_array()
        .ok_or_else(|| format!("{artifact}: {key} must be an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{artifact}: {key} entries must be strings"))
        })
        .collect()
}

fn validate_capability(value: &Value) -> Result<(), String> {
    let expected = serde_json::to_value(nano_protocol::profile::v1_capabilities())
        .map_err(|err| err.to_string())?;
    if value["profile"] != expected {
        return Err("capability-profile: profile drift".into());
    }
    Ok(())
}

fn validate_journal(value: &Value) -> Result<(), String> {
    let expected = nano_session::op::OP_VOCABULARY
        .iter()
        .map(|tag| (*tag).to_owned())
        .collect::<Vec<_>>();
    if string_array(value, "opVocabulary", "journal-semantics")? != expected {
        return Err("journal-semantics: op vocabulary drift".into());
    }
    let format = value["format"]
        .as_object()
        .ok_or("journal-semantics: format must be an object")?;
    if format.get("encoding").and_then(Value::as_str) != Some("ndjson")
        || format.get("schemaVersion").and_then(Value::as_u64)
            != Some(u64::from(nano_session::op::SCHEMA_VERSION))
        || format.get("appendSync").and_then(Value::as_str) != Some("sync_data_per_append")
        || format.get("envelopeFields") != Some(&serde_json::json!(["v", "id", "ts", "op"]))
    {
        return Err("journal-semantics: format drift".into());
    }
    if value["invariants"]
        != serde_json::json!([
            "append_only",
            "torn_tail_truncate_at_open",
            "single_writer_ownership"
        ])
    {
        return Err("journal-semantics: invariant drift".into());
    }
    Ok(())
}

fn validate_events(value: &Value, root: &Path) -> Result<(), String> {
    let corpus = corpus_root(root);
    let manifest: Value = parse(
        &std::fs::read(corpus.join("manifest.json")).map_err(|err| err.to_string())?,
        "corpus manifest",
    )?;
    if string_array(value, "commandTypes", "event-types")?
        != sorted_manifest_types(&manifest, "commands")?
        || string_array(value, "eventTypes", "event-types")?
            != sorted_manifest_types(&manifest, "events")?
    {
        return Err("event-types: command/event vocabulary drift".into());
    }
    let counts = &value["corpus"]["fixtureCounts"];
    for (name, directory) in [
        ("commands", "commands"),
        ("events", "events"),
        ("compat", "compat"),
        ("adversarial", "adversarial"),
    ] {
        if counts[name].as_u64() != Some(count_files(&corpus.join(directory))?) {
            return Err(format!("event-types: {name} count drift"));
        }
    }
    if value["corpus"]["name"].as_str() != Some("wayland-desktop-core")
        || value["corpus"]["version"].as_str() != Some("v1")
    {
        return Err("event-types: corpus identity drift".into());
    }
    Ok(())
}

fn validate_fixture_path(path: &str) -> Result<(), String> {
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || !path.starts_with("shared/fixtures/flux/")
        || !path.ends_with('/')
        || parsed
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("invalid fixture path: {path}"));
    }
    Ok(())
}

fn detect_monorepo(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .find(|candidate| candidate.join("shared/fixtures/flux").is_dir())
        .map(Path::to_path_buf)
}

fn validate_endpoints(value: &Value, monorepo: Option<&Path>) -> Result<(), String> {
    let endpoints = value["endpoints"]
        .as_array()
        .ok_or("flux-endpoint-contract: endpoints must be an array")?;
    if endpoints.len() != 6 {
        return Err("flux-endpoint-contract: expected exactly six endpoints".into());
    }
    let expected = [
        ("GET", "/v1/models"),
        ("POST", "/v1/chat/completions"),
        ("POST", "/anthropic/v1/messages"),
        ("POST", "/anthropic/v1/messages/count_tokens"),
        ("POST", "/v1/responses"),
        ("POST", "/mcp/"),
    ];
    let mut actual = BTreeSet::new();
    let evidence_root = monorepo.map(|root| root.join("shared/fixtures/flux"));
    let canonical_evidence = evidence_root
        .as_ref()
        .map(|path| path.canonicalize().map_err(|err| err.to_string()))
        .transpose()?;
    for endpoint in endpoints {
        let object = endpoint
            .as_object()
            .ok_or("flux-endpoint-contract: endpoint must be an object")?;
        let required = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("flux-endpoint-contract: endpoint {key} missing"))
        };
        let method = required("method")?;
        let path = required("path")?;
        let fixtures = required("fixtures")?;
        required("notes")?;
        if required("verified")? != "200" {
            return Err("flux-endpoint-contract: endpoint not verified 200".into());
        }
        validate_fixture_path(fixtures)?;
        if !actual.insert((method.to_owned(), path.to_owned())) {
            return Err("flux-endpoint-contract: duplicate endpoint".into());
        }
        if let (Some(monorepo), Some(evidence)) = (monorepo, canonical_evidence.as_ref()) {
            let resolved = monorepo.join(fixtures);
            if !resolved.is_dir() {
                return Err(format!("fixture directory missing: {}", resolved.display()));
            }
            let canonical = resolved.canonicalize().map_err(|err| err.to_string())?;
            if !canonical.starts_with(evidence) {
                return Err(format!("fixture path escapes evidence root: {fixtures}"));
            }
        }
    }
    let expected = expected
        .into_iter()
        .map(|(method, path)| (method.to_owned(), path.to_owned()))
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err("flux-endpoint-contract: endpoint inventory drift".into());
    }
    Ok(())
}

fn validate_all(root: &Path, monorepo: Option<&Path>) -> Result<(), String> {
    for artifact in ARTIFACTS {
        let value = load(root, artifact)?;
        common(&value, artifact)?;
        match artifact {
            "capability-profile" => validate_capability(&value)?,
            "journal-semantics" => validate_journal(&value)?,
            "flux-endpoint-contract" => validate_endpoints(&value, monorepo)?,
            "event-types" => validate_events(&value, root)?,
            _ => unreachable!(),
        }
    }
    Ok(())
}

#[test]
fn frozen_contracts_match_sources_and_evidence() {
    let root = repo_root();
    let monorepo = detect_monorepo(&root);
    if monorepo.is_none() {
        eprintln!(
            "external fixture existence not applicable: no ancestor has shared/fixtures/flux"
        );
    }
    validate_all(&root, monorepo.as_deref()).expect("frozen contracts must validate");
}

#[test]
fn missing_root_and_malformed_contracts_fail_closed() {
    let missing = repo_root().join("target/contract-test-missing-root");
    assert!(
        validate_all(&missing, None)
            .unwrap_err()
            .contains("required root contract")
    );
    assert!(
        parse(b"{not-json", "tampered")
            .unwrap_err()
            .contains("malformed JSON")
    );
    let root = repo_root();
    let canonical = std::fs::read(root.join("contracts/capability-profile.json")).unwrap();
    let mut whitespace_tamper = canonical;
    whitespace_tamper.push(b'\n');
    assert_eq!(
        parse_canonical(&whitespace_tamper, "tampered").unwrap_err(),
        "tampered: bytes are not canonical"
    );
}

#[test]
fn semantic_tampering_fails_for_intended_reasons() {
    let root = repo_root();
    let mut capability = load(&root, "capability-profile").unwrap();
    capability.as_object_mut().unwrap().remove("frozen");
    assert!(
        common(&capability, "capability-profile")
            .unwrap_err()
            .contains("frozen must be a string")
    );
    let mut wrong_type = load(&root, "capability-profile").unwrap();
    wrong_type["frozen"] = serde_json::json!(20260815);
    assert!(
        common(&wrong_type, "capability-profile")
            .unwrap_err()
            .contains("frozen must be a string")
    );
    let mut wrong_metadata = load(&root, "capability-profile").unwrap();
    wrong_metadata["$schema"] = serde_json::json!("wrong");
    assert_eq!(
        common(&wrong_metadata, "capability-profile").unwrap_err(),
        "capability-profile: wrong schema identity"
    );

    let mut journal = load(&root, "journal-semantics").unwrap();
    journal["opVocabulary"].as_array_mut().unwrap().pop();
    assert_eq!(
        validate_journal(&journal).unwrap_err(),
        "journal-semantics: op vocabulary drift"
    );

    let mut events = load(&root, "event-types").unwrap();
    events["corpus"]["fixtureCounts"]["compat"] = serde_json::json!(24);
    assert_eq!(
        validate_events(&events, &root).unwrap_err(),
        "event-types: compat count drift"
    );

    let endpoint = load(&root, "flux-endpoint-contract").unwrap();
    let mut duplicate = endpoint.clone();
    duplicate["endpoints"][5] = duplicate["endpoints"][0].clone();
    assert!(
        validate_endpoints(&duplicate, None)
            .unwrap_err()
            .contains("duplicate endpoint")
    );
    let mut traversal = endpoint.clone();
    traversal["endpoints"][0]["fixtures"] = serde_json::json!("shared/fixtures/flux/../escape/");
    assert!(
        validate_endpoints(&traversal, None)
            .unwrap_err()
            .contains("invalid fixture path")
    );
    let mut absolute = endpoint.clone();
    absolute["endpoints"][0]["fixtures"] = serde_json::json!("C:/absolute/");
    assert!(
        validate_endpoints(&absolute, None)
            .unwrap_err()
            .contains("invalid fixture path")
    );
    assert!(validate_endpoints(&endpoint, Some(Path::new("Z:/missing-monorepo"))).is_err());
}
