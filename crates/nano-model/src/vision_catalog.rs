//! P2a §6.3 — the static vision-capability catalog (fail-closed).
//!
//! `GET /v1/models` carries id + token limits ONLY, so per-leaf vision
//! gating comes from this STATIC LOCAL catalog, patterned on kimi's
//! UNKNOWN_CAPABILITY all-false default (`kosong/src/capability.ts`).
//!
//! Rules (D6):
//! - keyed by EXACT model id — no wildcard/prefix matching (prefix rules
//!   are how catalogs silently go stale);
//! - absent id ⇒ `image_in: false`;
//! - every `image_in: true` entry MUST name its §13 leg-6 proof artifact
//!   under `shared/fixtures/flux/vision/` — enforced at parse time, so an
//!   unproven `true` can never load (AGENTS.md "evidence before claims");
//! - aliases (`flux-auto`/`flux-standard`/`flux-fast`/`flux-reasoning`):
//!   the P2a v1 rule was NEVER-blessed (a single probe proved an alias
//!   routed to vision once, not routing stability). F-P2B-1 (2026-08-14)
//!   supersedes it for these four ids: the owner Flux media contract
//!   (shared/reviews/stable-wave/flux-media-contract-2026-08-14.md) forbids
//!   `/v1/models` capability gating ("until we publish modalities: assume
//!   vision works") and owner-measured 12/12 correct image probes on all
//!   four aliases on BOTH API shapes; the local capture
//!   (shared/fixtures/flux/vision/flux-openai-wire/20260814_probe_capture.json)
//!   proves genuine ingestion for `flux-auto` on both wires;
//! - overrides are TIGHTENING-ONLY (r2 codex-F9): a config override may
//!   turn a proven entry OFF; the false→true (positive) override is a
//!   typed config error, never silently applied.

use std::collections::BTreeMap;

use serde::Deserialize;

const VENDORED_JSON: &str = include_str!("../data/visionCatalog.vendored.json");

/// The path prefix every proof artifact reference must carry (§13 leg 6
/// recordings live under `shared/fixtures/flux/vision/`).
pub const PROOF_ARTIFACT_PREFIX: &str = "shared/fixtures/flux/vision/";

/// One catalog row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionCatalogEntry {
    pub image_in: bool,
    /// The proof-artifact reference for `image_in: true` entries (None
    /// while the entry is unproven).
    pub proven: Option<String>,
}

/// Catalog load/validation failures — fail-closed, never a silent default.
#[derive(Debug, thiserror::Error)]
pub enum VisionCatalogError {
    #[error("vision catalog JSON is invalid: {0}")]
    Json(String),
    #[error("vision catalog version {0} is not supported (expected 1)")]
    Version(u32),
    /// The honesty gate: an `image_in: true` entry without a recorded
    /// leg-6 proof artifact.
    #[error(
        "vision catalog entry {0} claims image_in without a proof artifact under {PROOF_ARTIFACT_PREFIX}"
    )]
    UnprovenTrue(String),
    /// The tightening-only rule: a false→true override is a typed config
    /// error, never silently applied.
    #[error("positive vision override for {0} is not supported — overrides are tightening-only")]
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
    image_in: bool,
    #[serde(default)]
    proven: Option<String>,
}

/// The exact-id keyed catalog.
#[derive(Debug, Clone)]
pub struct VisionCatalog {
    entries: BTreeMap<String, VisionCatalogEntry>,
}

impl VisionCatalog {
    /// The vendored catalog embedded at build time
    /// (`data/visionCatalog.vendored.json`).
    pub fn vendored() -> Result<Self, VisionCatalogError> {
        Self::from_json_str(VENDORED_JSON)
    }

    /// Parse and validate a catalog document. `image_in: true` without a
    /// `shared/fixtures/flux/vision/` proof reference fails CLOSED.
    pub fn from_json_str(text: &str) -> Result<Self, VisionCatalogError> {
        let file: CatalogFile =
            serde_json::from_str(text).map_err(|e| VisionCatalogError::Json(e.to_string()))?;
        if file.version != 1 {
            return Err(VisionCatalogError::Version(file.version));
        }
        let mut entries = BTreeMap::new();
        for (id, entry) in file.entries {
            if entry.image_in {
                let proof_ok = entry
                    .proven
                    .as_deref()
                    .is_some_and(|p| p.starts_with(PROOF_ARTIFACT_PREFIX));
                if !proof_ok {
                    return Err(VisionCatalogError::UnprovenTrue(id));
                }
            }
            entries.insert(
                id,
                VisionCatalogEntry {
                    image_in: entry.image_in,
                    proven: entry.proven,
                },
            );
        }
        Ok(Self { entries })
    }

    /// The gate lookup: EXACT id only, absent ⇒ false.
    pub fn image_in(&self, model_id: &str) -> bool {
        self.entries.get(model_id).is_some_and(|e| e.image_in)
    }

    /// The proof-artifact reference for an entry, when present.
    pub fn proven(&self, model_id: &str) -> Option<&str> {
        self.entries.get(model_id).and_then(|e| e.proven.as_deref())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Apply config `[model_capabilities]` overrides — TIGHTENING-ONLY
    /// (r2 codex-F9): `false` turns a proven entry OFF (honored and
    /// reported in the returned applied list so the host can log it);
    /// `true` is a typed [`VisionCatalogError::PositiveOverride`] config
    /// error, never silently applied. The receiver is unchanged on error.
    pub fn with_tightening_overrides(
        &self,
        overrides: &BTreeMap<String, bool>,
    ) -> Result<(Self, Vec<String>), VisionCatalogError> {
        let mut tightened = self.clone();
        let mut applied = Vec::new();
        for (id, value) in overrides {
            if *value {
                return Err(VisionCatalogError::PositiveOverride(id.clone()));
            }
            let was = tightened.image_in(id);
            tightened.entries.insert(
                id.clone(),
                VisionCatalogEntry {
                    image_in: false,
                    proven: self.proven(id).map(str::to_owned),
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

    const PROOF: &str = "shared/fixtures/flux/vision/probe.json";

    fn catalog_with_true() -> VisionCatalog {
        let json = format!(
            r#"{{"version": 1, "entries": {{
                "flux-pinned-gpt-5": {{ "image_in": true, "proven": "{PROOF}" }},
                "flux-pinned-codestral": {{ "image_in": false, "proven": null }}
            }}}}"#
        );
        VisionCatalog::from_json_str(&json).unwrap()
    }

    #[test]
    fn vendored_catalog_parses_and_aliases_are_probe_blessed() {
        let catalog = VisionCatalog::vendored().expect("vendored catalog parses");
        assert!(!catalog.is_empty());
        // F-P2B-1 (2026-08-14): the four routing aliases are blessed on the
        // owner Flux media contract ("assume vision works", 12/12 on all
        // four, both wires) plus the local flux-openai-wire capture —
        // superseding the P2a D6 aliases-never-blessed posture. Every
        // blessed alias cites the fixture-tree proof artifact.
        for alias in ["flux-auto", "flux-standard", "flux-fast", "flux-reasoning"] {
            assert!(
                catalog.image_in(alias),
                "alias {alias} is blessed (F-P2B-1)"
            );
            let proven = catalog
                .proven(alias)
                .expect("blessed alias names its proof");
            assert!(
                proven.starts_with(PROOF_ARTIFACT_PREFIX),
                "proof artifact under the fixture tree: {proven}"
            );
        }
        // Excluded families stay fail-closed.
        assert!(!catalog.image_in("flux-pinned-codestral"));
        assert!(!catalog.image_in("flux-image"));
    }

    #[test]
    fn lookup_is_exact_id_only_and_absent_is_false() {
        let catalog = catalog_with_true();
        // The proven id is true…
        assert!(catalog.image_in("flux-pinned-gpt-5"));
        assert_eq!(catalog.proven("flux-pinned-gpt-5"), Some(PROOF));
        // …but it must NOT bless a suffix/prefix neighbor (no wildcard or
        // prefix matching).
        assert!(!catalog.image_in("flux-pinned-gpt-5-unknown-suffix"));
        assert!(!catalog.image_in("flux-pinned-gpt"));
        // Absent id ⇒ false.
        assert!(!catalog.image_in("flux-pinned-mistral-large-9"));
        assert_eq!(catalog.proven("absent"), None);
    }

    #[test]
    fn true_without_a_proof_artifact_fails_closed() {
        let json = r#"{"version": 1, "entries": {"x": { "image_in": true, "proven": null }}}"#;
        assert!(matches!(
            VisionCatalog::from_json_str(json),
            Err(VisionCatalogError::UnprovenTrue(id)) if id == "x"
        ));
        // A proof reference outside the mandated fixture tree is not proof.
        let json =
            r#"{"version": 1, "entries": {"x": { "image_in": true, "proven": "docs/claim.md" }}}"#;
        assert!(matches!(
            VisionCatalog::from_json_str(json),
            Err(VisionCatalogError::UnprovenTrue(_))
        ));
    }

    #[test]
    fn overrides_are_tightening_only() {
        let catalog = catalog_with_true();
        // true→false is honored and reported for logging.
        let mut overrides = BTreeMap::new();
        overrides.insert("flux-pinned-gpt-5".to_string(), false);
        let (tightened, applied) = catalog.with_tightening_overrides(&overrides).unwrap();
        assert!(!tightened.image_in("flux-pinned-gpt-5"));
        assert_eq!(applied, ["flux-pinned-gpt-5"]);
        // The receiver is unchanged.
        assert!(catalog.image_in("flux-pinned-gpt-5"));

        // false→true is a typed config error, never silently applied.
        let mut positive = BTreeMap::new();
        positive.insert("flux-pinned-codestral".to_string(), true);
        let err = catalog.with_tightening_overrides(&positive).unwrap_err();
        assert!(
            matches!(err, VisionCatalogError::PositiveOverride(id) if id == "flux-pinned-codestral")
        );
        // …including on ids the catalog does not know.
        let mut unknown = BTreeMap::new();
        unknown.insert("totally-unknown".to_string(), true);
        assert!(matches!(
            catalog.with_tightening_overrides(&unknown),
            Err(VisionCatalogError::PositiveOverride(_))
        ));
        // Tightening an absent/already-false id is a harmless no-op.
        let mut noop = BTreeMap::new();
        noop.insert("totally-unknown".to_string(), false);
        let (tightened, _) = catalog.with_tightening_overrides(&noop).unwrap();
        assert!(!tightened.image_in("totally-unknown"));
        assert!(tightened.image_in("flux-pinned-gpt-5"));
    }
}
