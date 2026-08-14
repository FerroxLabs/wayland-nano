//! P5 §5/S1 — the static tool-capability catalog (fail-closed), the
//! exact-mirror discipline of [`crate::vision_catalog`] (P2a §6.3) applied
//! to tool-use.
//!
//! Rules (same shape as D6):
//! - keyed by EXACT `provider:leaf` — no wildcard/prefix matching (prefix
//!   rules are how catalogs silently go stale);
//! - absent key ⇒ `tool_use: false`;
//! - every `tool_use: true` entry MUST name its proof artifact under
//!   `shared/fixtures/flux/tools/` — enforced at parse time, so an
//!   unproven `true` can never load (AGENTS.md "evidence before claims");
//! - the `flux-auto` alias MAY be blessed for tools (unlike vision): Flux
//!   routes internally and alias tool-calls are live-proven (fixtures under
//!   `shared/fixtures/flux/tools/`; earlier alias evidence under
//!   `shared/fixtures/flux/tool-calls/`). Capability admission is NOT leaf
//!   identity: §6 metering of alias candidates stays provenance-only;
//! - overrides are TIGHTENING-ONLY: a config override may turn a proven
//!   entry OFF; the false→true (positive) override is a typed config
//!   error, never silently applied.

use std::collections::BTreeMap;

use serde::Deserialize;

const VENDORED_JSON: &str = include_str!("../data/toolCapability.vendored.json");

/// The path prefix every proof artifact reference must carry (the S1 tool
/// probe recordings live under `shared/fixtures/flux/tools/`).
pub const PROOF_ARTIFACT_PREFIX: &str = "shared/fixtures/flux/tools/";

/// One catalog row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCapabilityEntry {
    pub tool_use: bool,
    /// The proof-artifact reference for `tool_use: true` entries (None
    /// while the entry is unproven).
    pub proven: Option<String>,
}

/// Catalog load/validation failures — fail-closed, never a silent default.
#[derive(Debug, thiserror::Error)]
pub enum ToolCapabilityCatalogError {
    #[error("tool capability catalog JSON is invalid: {0}")]
    Json(String),
    #[error("tool capability catalog version {0} is not supported (expected 1)")]
    Version(u32),
    /// The honesty gate: a `tool_use: true` entry without a recorded proof
    /// artifact under the mandated fixture tree.
    #[error(
        "tool capability entry {0} claims tool_use without a proof artifact under {PROOF_ARTIFACT_PREFIX}"
    )]
    UnprovenTrue(String),
    /// The tightening-only rule: a false→true override is a typed config
    /// error, never silently applied.
    #[error(
        "positive tool-capability override for {0} is not supported — overrides are tightening-only"
    )]
    PositiveOverride(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    version: u32,
    /// Free-form review note; carries no semantics.
    #[serde(default)]
    #[allow(dead_code)]
    notes: Option<String>,
    entries: BTreeMap<String, EntryFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryFile {
    tool_use: bool,
    #[serde(default)]
    proven: Option<String>,
}

/// The exact-key catalog (`provider:leaf`).
#[derive(Debug, Clone)]
pub struct ToolCapabilityCatalog {
    entries: BTreeMap<String, ToolCapabilityEntry>,
}

impl ToolCapabilityCatalog {
    /// The vendored catalog embedded at build time
    /// (`data/toolCapability.vendored.json`).
    pub fn vendored() -> Result<Self, ToolCapabilityCatalogError> {
        Self::from_json_str(VENDORED_JSON)
    }

    /// The catalog key for one provider/leaf pair (exact match only).
    pub fn key(provider: &str, leaf: &str) -> String {
        format!("{provider}:{leaf}")
    }

    /// Parse and validate a catalog document. `tool_use: true` without a
    /// `shared/fixtures/flux/tools/` proof reference fails CLOSED.
    pub fn from_json_str(text: &str) -> Result<Self, ToolCapabilityCatalogError> {
        let file: CatalogFile = serde_json::from_str(text)
            .map_err(|e| ToolCapabilityCatalogError::Json(e.to_string()))?;
        if file.version != 1 {
            return Err(ToolCapabilityCatalogError::Version(file.version));
        }
        let mut entries = BTreeMap::new();
        for (id, entry) in file.entries {
            if entry.tool_use {
                let proof_ok = entry
                    .proven
                    .as_deref()
                    .is_some_and(|p| p.starts_with(PROOF_ARTIFACT_PREFIX));
                if !proof_ok {
                    return Err(ToolCapabilityCatalogError::UnprovenTrue(id));
                }
            }
            entries.insert(
                id,
                ToolCapabilityEntry {
                    tool_use: entry.tool_use,
                    proven: entry.proven,
                },
            );
        }
        Ok(Self { entries })
    }

    /// The gate lookup: EXACT provider/leaf only, absent ⇒ false.
    pub fn tool_use(&self, provider: &str, leaf: &str) -> bool {
        self.entries
            .get(&Self::key(provider, leaf))
            .is_some_and(|e| e.tool_use)
    }

    /// The proof-artifact reference for an entry, when present.
    pub fn proven(&self, provider: &str, leaf: &str) -> Option<&str> {
        self.entries
            .get(&Self::key(provider, leaf))
            .and_then(|e| e.proven.as_deref())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Apply config overrides — TIGHTENING-ONLY (the vision-catalog rule):
    /// `false` turns a proven entry OFF (honored and reported in the
    /// returned applied list so the host can log it); `true` is a typed
    /// [`ToolCapabilityCatalogError::PositiveOverride`] config error, never
    /// silently applied. The receiver is unchanged on error.
    pub fn with_tightening_overrides(
        &self,
        overrides: &BTreeMap<String, bool>,
    ) -> Result<(Self, Vec<String>), ToolCapabilityCatalogError> {
        let mut tightened = self.clone();
        let mut applied = Vec::new();
        for (id, value) in overrides {
            if *value {
                return Err(ToolCapabilityCatalogError::PositiveOverride(id.clone()));
            }
            let was = tightened.entries.get(id).is_some_and(|e| e.tool_use);
            tightened.entries.insert(
                id.clone(),
                ToolCapabilityEntry {
                    tool_use: false,
                    proven: self
                        .entries
                        .get(id)
                        .and_then(|e| e.proven.as_deref().map(str::to_owned)),
                },
            );
            applied.push(id.clone());
            let _ = was; // tightening an already-false/absent id is a harmless no-op
        }
        Ok((tightened, applied))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROOF: &str = "shared/fixtures/flux/tools/probe.json";

    fn catalog_with_true() -> ToolCapabilityCatalog {
        let json = format!(
            r#"{{"version": 1, "entries": {{
                "flux-router:flux-auto": {{ "tool_use": true, "proven": "{PROOF}" }},
                "flux-router:flux-pinned-codestral": {{ "tool_use": false, "proven": null }}
            }}}}"#
        );
        ToolCapabilityCatalog::from_json_str(&json).unwrap()
    }

    #[test]
    fn vendored_catalog_parses() {
        let catalog = ToolCapabilityCatalog::vendored().expect("vendored catalog parses");
        assert!(!catalog.is_empty());
        // Aliases are listed explicitly for reviewability; their blessed
        // state is whatever the vendored file records (probe-evidenced).
        for alias in ["flux-auto", "flux-standard", "flux-fast", "flux-reasoning"] {
            assert!(
                catalog.proven("flux-router", alias).is_none()
                    || catalog.tool_use("flux-router", alias)
            );
        }
    }

    #[test]
    fn lookup_is_exact_key_only_and_absent_is_false() {
        let catalog = catalog_with_true();
        // The proven pair is true…
        assert!(catalog.tool_use("flux-router", "flux-auto"));
        assert_eq!(catalog.proven("flux-router", "flux-auto"), Some(PROOF));
        // …but it must NOT bless a suffix/prefix neighbor or another
        // provider's same-named leaf (no wildcard/prefix matching).
        assert!(!catalog.tool_use("flux-router", "flux-auto-unknown-suffix"));
        assert!(!catalog.tool_use("openai", "flux-auto"));
        // Absent key ⇒ false.
        assert!(!catalog.tool_use("flux-router", "flux-pinned-mistral-large-9"));
        assert_eq!(catalog.proven("absent", "leaf"), None);
    }

    #[test]
    fn true_without_a_proof_artifact_fails_closed() {
        let json = r#"{"version": 1, "entries": {"p:x": { "tool_use": true, "proven": null }}}"#;
        assert!(matches!(
            ToolCapabilityCatalog::from_json_str(json),
            Err(ToolCapabilityCatalogError::UnprovenTrue(id)) if id == "p:x"
        ));
        // A proof reference outside the mandated fixture tree is not proof.
        let json = r#"{"version": 1, "entries": {"p:x": { "tool_use": true, "proven": "docs/claim.md" }}}"#;
        assert!(matches!(
            ToolCapabilityCatalog::from_json_str(json),
            Err(ToolCapabilityCatalogError::UnprovenTrue(_))
        ));
    }

    #[test]
    fn overrides_are_tightening_only() {
        let catalog = catalog_with_true();
        // true→false is honored and reported for logging.
        let mut overrides = BTreeMap::new();
        overrides.insert("flux-router:flux-auto".to_string(), false);
        let (tightened, applied) = catalog.with_tightening_overrides(&overrides).unwrap();
        assert!(!tightened.tool_use("flux-router", "flux-auto"));
        assert_eq!(applied, ["flux-router:flux-auto"]);
        // The receiver is unchanged.
        assert!(catalog.tool_use("flux-router", "flux-auto"));

        // false→true is a typed config error, never silently applied.
        let mut positive = BTreeMap::new();
        positive.insert("flux-router:flux-pinned-codestral".to_string(), true);
        let err = catalog.with_tightening_overrides(&positive).unwrap_err();
        assert!(matches!(
            err,
            ToolCapabilityCatalogError::PositiveOverride(id) if id == "flux-router:flux-pinned-codestral"
        ));
        // …including on keys the catalog does not know.
        let mut unknown = BTreeMap::new();
        unknown.insert("totally-unknown:leaf".to_string(), true);
        assert!(matches!(
            catalog.with_tightening_overrides(&unknown),
            Err(ToolCapabilityCatalogError::PositiveOverride(_))
        ));
        // Tightening an absent/already-false key is a harmless no-op.
        let mut noop = BTreeMap::new();
        noop.insert("totally-unknown:leaf".to_string(), false);
        let (tightened, _) = catalog.with_tightening_overrides(&noop).unwrap();
        assert!(!tightened.tool_use("totally-unknown", "leaf"));
        assert!(tightened.tool_use("flux-router", "flux-auto"));
    }
}
