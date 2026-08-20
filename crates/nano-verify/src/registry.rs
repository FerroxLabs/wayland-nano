//! Canonical gate registry and closure-pin primitives.

use crate::{VerifyError, gate::FailCategory};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GateClosure {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd_policy: CwdPolicy,
    pub wrapped_tools: Vec<ToolPin>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CwdPolicy {
    RepoRoot,
    GateDir,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolPin {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GateRegistryEntry {
    pub card: String,
    pub script: String,
    pub closure: GateClosure,
    pub closure_digest: String,
    pub run_artifact: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GateRegistry {
    pub schema: u32,
    pub gates: BTreeMap<String, GateRegistryEntry>,
    pub requirements: BTreeMap<String, String>,
}

pub fn closure_digest(closure: &GateClosure) -> Result<String, VerifyError> {
    let bytes = canonical_json(&serde_json::to_value(closure).map_err(registry_error)?)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>, VerifyError> {
    fn normalize(value: &serde_json::Value) -> Result<serde_json::Value, VerifyError> {
        Ok(match value {
            serde_json::Value::Null => serde_json::Value::Null,
            serde_json::Value::Bool(v) => serde_json::Value::Bool(*v),
            serde_json::Value::Number(v) if v.is_i64() || v.is_u64() => {
                serde_json::Value::Number(v.clone())
            }
            serde_json::Value::Number(_) => {
                return Err(VerifyError::Registry(
                    "canonical JSON permits integers only".into(),
                ));
            }
            serde_json::Value::String(v) => serde_json::Value::String(v.nfc().collect()),
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(normalize).collect::<Result<_, _>>()?)
            }
            serde_json::Value::Object(values) => {
                let mut sorted = BTreeMap::new();
                for (key, value) in values {
                    let key: String = key.nfc().collect();
                    if sorted.insert(key, normalize(value)?).is_some() {
                        return Err(VerifyError::Registry(
                            "NFC-normalized object keys collide".into(),
                        ));
                    }
                }
                serde_json::Value::Object(sorted.into_iter().collect())
            }
        })
    }
    serde_json::to_vec(&normalize(value)?).map_err(registry_error)
}

pub fn load_registry(repo_root: &Path) -> Result<GateRegistry, VerifyError> {
    let root = repo_root.canonicalize().map_err(registry_error)?;
    let registry_path = confined_existing(&root, Path::new("gates/registry.json"), "registry")?;
    let registry: GateRegistry =
        serde_json::from_slice(&fs::read(registry_path).map_err(registry_error)?)
            .map_err(registry_error)?;
    if registry.schema != 1 {
        return Err(VerifyError::Registry(format!(
            "unsupported registry schema {}",
            registry.schema
        )));
    }
    if registry.gates.is_empty() || registry.requirements.is_empty() {
        return Err(VerifyError::Registry(
            "registry gates and requirements must be nonempty".into(),
        ));
    }
    for (requirement, gate_id) in &registry.requirements {
        if requirement.is_empty() || !registry.gates.contains_key(gate_id) {
            return Err(VerifyError::Registry(format!(
                "requirement {requirement} has dangling gate {gate_id}"
            )));
        }
    }
    for (gate_id, entry) in &registry.gates {
        if gate_id.is_empty() {
            return Err(VerifyError::Registry("gate id must be nonempty".into()));
        }
        check_closure_pin(entry)?;
        let card = confined_existing(&root, Path::new(&entry.card), "card")?;
        confined_existing(&root, Path::new(&entry.script), "script")?;
        confined_existing(&root, Path::new(&entry.run_artifact), "run artifact")?;
        check_inventory(&card)?;
        validate_script_shape(entry)?;
    }
    Ok(registry)
}

pub fn gate_for_requirement<'a>(
    registry: &'a GateRegistry,
    requirement: &str,
) -> Result<(&'a str, &'a GateRegistryEntry), VerifyError> {
    let id = registry
        .requirements
        .get(requirement)
        .ok_or_else(|| VerifyError::Registry(format!("unmapped requirement {requirement}")))?;
    let entry = registry.gates.get(id).ok_or_else(|| {
        VerifyError::Registry(format!(
            "requirement {requirement} points to missing gate {id}"
        ))
    })?;
    Ok((id.as_str(), entry))
}

pub fn check_closure_pin(entry: &GateRegistryEntry) -> Result<(), VerifyError> {
    let actual = closure_digest(&entry.closure)?;
    if entry.closure_digest.len() != 64
        || !entry
            .closure_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || entry.closure_digest != actual
    {
        return Err(VerifyError::Registry("closure digest drift".into()));
    }
    Ok(())
}

pub fn check_inventory(card_path: &Path) -> Result<Vec<(String, FailCategory)>, VerifyError> {
    let text = fs::read_to_string(card_path).map_err(registry_error)?;
    let mut in_frontmatter = false;
    let mut in_checks = false;
    let mut inventory = Vec::new();
    for line in text.lines() {
        if line.trim() == "---" {
            if in_frontmatter {
                break;
            }
            in_frontmatter = true;
            continue;
        }
        if !in_frontmatter {
            continue;
        }
        if line.trim() == "checks:" {
            in_checks = true;
            continue;
        }
        if !in_checks {
            continue;
        }
        let trimmed = line.trim();
        if !trimmed.starts_with("- {") {
            if !trimmed.is_empty() && !line.starts_with(' ') {
                break;
            }
            continue;
        }
        let body = trimmed.trim_start_matches("- {").trim_end_matches('}');
        let mut id = None;
        let mut category = None;
        for field in body.split(',') {
            let Some((key, value)) = field.split_once(':') else {
                continue;
            };
            match key.trim() {
                "id" => id = Some(value.trim()),
                "category" => category = parse_category(value.trim()),
                _ => {}
            }
        }
        let id = id
            .filter(|v| valid_id(v))
            .ok_or_else(|| VerifyError::Registry("malformed card check id".into()))?;
        let category = category
            .ok_or_else(|| VerifyError::Registry("malformed card check category".into()))?;
        if inventory.iter().any(|(known, _)| known == id) {
            return Err(VerifyError::Registry(format!(
                "duplicate card check id {id}"
            )));
        }
        inventory.push((id.to_owned(), category));
    }
    if inventory.is_empty() {
        return Err(VerifyError::Registry(
            "card check inventory is empty or malformed".into(),
        ));
    }
    Ok(inventory)
}

fn validate_script_shape(entry: &GateRegistryEntry) -> Result<(), VerifyError> {
    let argv = &entry.closure.argv;
    let direct = argv.first().is_some_and(|arg| arg == &entry.script);
    let interpreted = argv.get(1).is_some_and(|arg| arg == &entry.script)
        && argv.first().is_some_and(|program| {
            entry
                .closure
                .wrapped_tools
                .iter()
                .any(|tool| &tool.name == program && !tool.version.is_empty())
        });
    if !direct && !interpreted {
        return Err(VerifyError::Registry(
            "script must be direct argv[0] or argv[1] after a pinned interpreter".into(),
        ));
    }
    Ok(())
}

fn confined_existing(root: &Path, relative: &Path, label: &str) -> Result<PathBuf, VerifyError> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(VerifyError::Registry(format!(
            "{label} path is not a confined relative path"
        )));
    }
    let canonical = root
        .join(relative)
        .canonicalize()
        .map_err(|_| VerifyError::Registry(format!("{label} path is missing")))?;
    if !canonical.starts_with(root) {
        return Err(VerifyError::Registry(format!(
            "{label} path escapes repository"
        )));
    }
    Ok(canonical)
}

fn parse_category(value: &str) -> Option<FailCategory> {
    match value {
        "structure" => Some(FailCategory::Structure),
        "value" => Some(FailCategory::Value),
        "relation" => Some(FailCategory::Relation),
        "grounding" => Some(FailCategory::Grounding),
        "execution" => Some(FailCategory::Execution),
        "security" => Some(FailCategory::Security),
        _ => None,
    }
}
fn valid_id(id: &str) -> bool {
    let Some((prefix, digits)) = id.split_once('-') else {
        return false;
    };
    (2..=4).contains(&prefix.len())
        && prefix.bytes().all(|b| b.is_ascii_uppercase())
        && digits.len() == 2
        && digits.bytes().all(|b| b.is_ascii_digit())
}
fn registry_error(error: impl std::fmt::Display) -> VerifyError {
    VerifyError::Registry(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn closure() -> GateClosure {
        GateClosure {
            argv: vec!["node".into(), "gates/demo/gate.cjs".into()],
            env: BTreeMap::from([
                ("z\u{301}".into(), "e\u{301}".into()),
                ("A".into(), "one".into()),
            ]),
            cwd_policy: CwdPolicy::RepoRoot,
            wrapped_tools: vec![ToolPin {
                name: "node".into(),
                version: "20".into(),
            }],
        }
    }

    #[test]
    fn closure_digest_is_canonical() {
        let expected = "{\"argv\":[\"node\",\"gates/demo/gate.cjs\"],\"cwd_policy\":\"repo-root\",\"env\":{\"A\":\"one\",\"ź\":\"é\"},\"wrapped_tools\":[{\"name\":\"node\",\"version\":\"20\"}]}";
        assert_eq!(
            canonical_json(&serde_json::to_value(closure()).unwrap()).unwrap(),
            expected.as_bytes()
        );
        assert_eq!(
            closure_digest(&closure()).unwrap(),
            "c6c3c5de47c926581860db14cca0f6c57d74598337ed2fc7977ef8d81ec3c19b"
        );
        assert!(canonical_json(&serde_json::json!({"nested":{"v":1.5}})).is_err());
    }

    #[test]
    fn registry_rejects_unknown_fields() {
        let samples = [
            r#"{"schema":1,"gates":{},"requirements":{},"extra":1}"#,
            r#"{"schema":1,"gates":{"x":{"card":"c","script":"s","closure":{"argv":[],"env":{},"cwd_policy":"repo-root","wrapped_tools":[]},"closure_digest":"d","run_artifact":"r","extra":1}},"requirements":{}}"#,
            r#"{"schema":1,"gates":{"x":{"card":"c","script":"s","closure":{"argv":[],"env":{},"cwd_policy":"repo-root","wrapped_tools":[],"extra":1},"closure_digest":"d","run_artifact":"r"}},"requirements":{}}"#,
            r#"{"schema":1,"gates":{"x":{"card":"c","script":"s","closure":{"argv":[],"env":{},"cwd_policy":"repo-root","wrapped_tools":[{"name":"n","version":"1","extra":1}]},"closure_digest":"d","run_artifact":"r"}},"requirements":{}}"#,
        ];
        for sample in samples {
            assert!(serde_json::from_str::<GateRegistry>(sample).is_err());
        }
    }

    fn write_repo(root: &Path, argv: &[&str]) {
        fs::create_dir_all(root.join("gates/demo")).unwrap();
        fs::create_dir_all(root.join("artifacts/run")).unwrap();
        fs::write(root.join("gates/demo/gate.cjs"), "").unwrap();
        fs::write(root.join("gates/demo/card.md"), "---\nchecks:\n  - { id: TG-01, category: structure, desc: x }\n  - { id: TG-02, category: security, desc: y }\n---\n").unwrap();
        let mut c = closure();
        c.argv = argv.iter().map(|v| (*v).into()).collect();
        let doc = serde_json::json!({"schema":1,"gates":{"demo":{"card":"gates/demo/card.md","script":"gates/demo/gate.cjs","closure":c,"closure_digest":closure_digest(&c).unwrap(),"run_artifact":"artifacts/run"}},"requirements":{"REQ-1":"demo"}});
        fs::create_dir_all(root.join("gates")).unwrap();
        fs::write(
            root.join("gates/registry.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn registry_loads_closures_requirements_and_rejects_drift() {
        let temp = tempfile::tempdir().unwrap();
        write_repo(temp.path(), &["node", "gates/demo/gate.cjs"]);
        let registry = load_registry(temp.path()).unwrap();
        let (id, entry) = gate_for_requirement(&registry, "REQ-1").unwrap();
        assert_eq!(id, "demo");
        check_closure_pin(entry).unwrap();
        assert_eq!(
            check_inventory(&temp.path().join(&entry.card))
                .unwrap()
                .len(),
            2
        );
        let path = temp.path().join("gates/registry.json");
        let raw = fs::read_to_string(&path).unwrap();
        for bad in [
            raw.replacen(&entry.closure_digest, &"0".repeat(64), 1),
            raw.replace("\"REQ-1\":\"demo\"", "\"REQ-1\":\"missing\""),
            raw.replace("artifacts/run", "../escape"),
            raw.replace("artifacts/run", "artifacts/missing"),
            raw.replace(
                "[\"node\",\"gates/demo/gate.cjs\"]",
                "[\"node\",\"x\",\"gates/demo/gate.cjs\"]",
            ),
        ] {
            fs::write(&path, bad).unwrap();
            assert!(load_registry(temp.path()).is_err());
        }
        write_repo(temp.path(), &["gates/demo/gate.cjs"]);
        assert!(load_registry(temp.path()).is_ok());
    }
}
