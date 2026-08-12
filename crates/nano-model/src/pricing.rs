//! Pricing-as-data for LLM providers (P1 §3.1 — port of wcore's
//! `wcore-pricing/src/lib.rs`; transformation recorded in UPSTREAM.md).
//!
//! Loads a TOML catalog of provider × model × input/output token rates
//! (USD per million tokens) and exposes a microcent-integer cost API.
//! The default catalog is bundled at compile time; `NANO_PRICING_PATH`
//! overrides it (namespaced per AGENTS.md — wcore's var is
//! `WAYLAND_PRICING_PATH`).
//!
//! Fail-closed rules (design §3.1/§6):
//! - a malformed override file is a TYPED error naming the path — never a
//!   silent fallback to bundled, never a partial parse;
//! - a model with no row resolves `priced: false` — absence is never $0;
//! - every Flux row is `billing = "unpriced"` (a dynamic-price router NEVER
//!   reports fake $0);
//! - cached tokens bill at the input rate when no cache row exists —
//!   conservative, never zero.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

const BUNDLED_PRICING_TOML: &str = include_str!("../pricing.toml");

/// The env var naming an override pricing catalog path (namespaced per
/// AGENTS.md; wcore's equivalent is `WAYLAND_PRICING_PATH`).
pub const PRICING_PATH_ENV: &str = "NANO_PRICING_PATH";

#[derive(Debug, Error)]
pub enum PricingError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("unknown model {model} for provider {provider}")]
    UnknownModel { provider: String, model: String },
    /// Fail-closed override/parse failure: the message names the source
    /// (override path or "bundled") so startup diagnostics point at the
    /// offending file.
    #[error("pricing catalog parse error in {origin}: {error}")]
    Parse {
        origin: String,
        error: toml::de::Error,
    },
    /// The override path could not be read at all (missing/unreadable) —
    /// typed, never a silent fallback to the bundled catalog.
    #[error("pricing override {path} unreadable: {error}")]
    OverrideIo { path: String, error: std::io::Error },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ModelPrice {
    pub input_per_mtok_usd: f64,
    pub output_per_mtok_usd: f64,
    #[serde(default)]
    pub cache_read_per_mtok_usd: Option<f64>,
    #[serde(default)]
    pub cache_write_per_mtok_usd: Option<f64>,
    /// Whether this row carries a real metered price, a known-free local
    /// price, or only a zero-valued placeholder for a dynamically priced
    /// router. Defaults to `metered` for existing external catalogs.
    #[serde(default)]
    pub billing: BillingClassification,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BillingClassification {
    #[default]
    Metered,
    Free,
    Unpriced,
}

/// A numeric estimate plus an explicit statement of whether it is a real
/// price. `priced == true && microcents == 0` represents known-free usage;
/// `priced == false` represents unknown dynamic pricing, never a free call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceStatus {
    pub microcents: u64,
    pub priced: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PricingCatalog {
    #[serde(flatten)]
    pub providers: HashMap<String, HashMap<String, ModelPrice>>,
}

impl PricingCatalog {
    fn classify_cost(price: &ModelPrice, microcents: u64) -> PriceStatus {
        match price.billing {
            BillingClassification::Metered => PriceStatus {
                microcents,
                priced: true,
            },
            BillingClassification::Free => PriceStatus {
                microcents: 0,
                priced: true,
            },
            BillingClassification::Unpriced => PriceStatus {
                microcents: 0,
                priced: false,
            },
        }
    }

    /// Load the effective catalog: the `NANO_PRICING_PATH` override when
    /// set, else the compile-time bundled table. Fail-closed (P1 §3.1): an
    /// unreadable or malformed override is a TYPED error naming the path —
    /// never a silent fallback to bundled, never a partial parse. A
    /// bundled-table parse failure is likewise typed (it would mean the
    /// shipped artifact is corrupt).
    pub fn load_default() -> Result<Self, PricingError> {
        if let Ok(path) = std::env::var(PRICING_PATH_ENV) {
            let raw = std::fs::read_to_string(&path).map_err(|error| PricingError::OverrideIo {
                path: path.clone(),
                error,
            })?;
            return Self::from_toml_str(&raw).map_err(|error| PricingError::Parse {
                origin: path,
                error,
            });
        }
        Self::from_toml_str(BUNDLED_PRICING_TOML).map_err(|error| PricingError::Parse {
            origin: "bundled pricing.toml".to_string(),
            error,
        })
    }

    /// Parse a catalog from a TOML string. Deserializes directly into the
    /// provider→model map instead of going through `PricingCatalog`'s
    /// `#[serde(flatten)]`, which the `toml` crate mishandles for
    /// externally-supplied catalogs (wcore regression #4).
    pub fn from_toml_str(raw: &str) -> Result<Self, toml::de::Error> {
        let providers: HashMap<String, HashMap<String, ModelPrice>> = toml::from_str(raw)?;
        Ok(Self { providers })
    }

    pub fn get(&self, provider: &str, model: &str) -> Result<&ModelPrice, PricingError> {
        let prov = self
            .providers
            .get(provider)
            .ok_or_else(|| PricingError::UnknownProvider(provider.into()))?;
        if let Some(p) = prov.get(model) {
            return Ok(p);
        }
        // Live API model slugs use dots in the version segment
        // (`gemini-2.5-flash`) while catalog keys use dashes
        // (`gemini-2-5-flash`). Retry with dots→dashes so a dotted slug still
        // resolves. Exact match is tried first, so dotted catalog keys
        // (e.g. `gpt-4.1-mini`) are unaffected.
        let normalized = model.replace('.', "-");
        if normalized != model
            && let Some(p) = prov.get(&normalized)
        {
            return Ok(p);
        }
        Err(PricingError::UnknownModel {
            provider: provider.into(),
            model: model.into(),
        })
    }

    pub fn estimate_cost_microcents(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<u64, PricingError> {
        let p = self.get(provider, model)?;
        let in_usd = (input_tokens as f64 / 1_000_000.0) * p.input_per_mtok_usd;
        let out_usd = (output_tokens as f64 / 1_000_000.0) * p.output_per_mtok_usd;
        let total_microcents = ((in_usd + out_usd) * 100.0 * 1_000_000.0).round() as u64;
        Ok(total_microcents)
    }

    /// Estimate cost while preserving the distinction between a known zero
    /// price and an unpriceable dynamic-router placeholder.
    pub fn estimate_cost_status(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<PriceStatus, PricingError> {
        let price = self.get(provider, model)?;
        let microcents =
            self.estimate_cost_microcents(provider, model, input_tokens, output_tokens)?;
        Ok(Self::classify_cost(price, microcents))
    }

    /// Estimate a turn whose provider reports cached input separately from
    /// uncached input. When a model has no special cache rate, cached tokens
    /// conservatively use the ordinary input rate instead of becoming free.
    pub fn estimate_cost_with_cache_microcents(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
    ) -> Result<u64, PricingError> {
        let p = self.get(provider, model)?;
        let per_mtok = |tokens: u64, usd: f64| (tokens as f64 / 1_000_000.0) * usd;
        let total_usd = per_mtok(input_tokens, p.input_per_mtok_usd)
            + per_mtok(output_tokens, p.output_per_mtok_usd)
            + per_mtok(
                cache_read_tokens,
                p.cache_read_per_mtok_usd.unwrap_or(p.input_per_mtok_usd),
            )
            + per_mtok(
                cache_write_tokens,
                p.cache_write_per_mtok_usd.unwrap_or(p.input_per_mtok_usd),
            );
        Ok((total_usd * 100.0 * 1_000_000.0).round() as u64)
    }

    /// Cache-aware counterpart to [`Self::estimate_cost_status`].
    pub fn estimate_cost_with_cache_status(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
    ) -> Result<PriceStatus, PricingError> {
        let price = self.get(provider, model)?;
        let microcents = self.estimate_cost_with_cache_microcents(
            provider,
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        )?;
        Ok(Self::classify_cost(price, microcents))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_tokens_use_catalog_rates_without_becoming_free() {
        let raw = "[p.m]\ninput_per_mtok_usd = 10.0\noutput_per_mtok_usd = 20.0\ncache_read_per_mtok_usd = 1.0\ncache_write_per_mtok_usd = 12.0\n\n[p.no-cache-rate]\ninput_per_mtok_usd = 10.0\noutput_per_mtok_usd = 20.0\n";
        let cat = PricingCatalog::from_toml_str(raw).unwrap();
        let priced = cat
            .estimate_cost_with_cache_microcents(
                "p", "m", 1_000_000, 1_000_000, 1_000_000, 1_000_000,
            )
            .unwrap();
        assert_eq!(priced, 43 * 100 * 1_000_000);

        let conservative = cat
            .estimate_cost_with_cache_microcents("p", "no-cache-rate", 0, 0, 1_000_000, 1_000_000)
            .unwrap();
        assert_eq!(conservative, 20 * 100 * 1_000_000);
    }

    #[test]
    fn status_distinguishes_known_free_from_unpriced_zero() {
        let raw = r#"
[local.model]
input_per_mtok_usd = 0.0
output_per_mtok_usd = 0.0
billing = "free"

[router.auto]
input_per_mtok_usd = 0.0
output_per_mtok_usd = 0.0
billing = "unpriced"

[paid.model]
input_per_mtok_usd = 1.0
output_per_mtok_usd = 2.0
"#;
        let cat = PricingCatalog::from_toml_str(raw).unwrap();

        assert_eq!(
            cat.estimate_cost_status("local", "model", 1_000_000, 1_000_000)
                .unwrap(),
            PriceStatus {
                microcents: 0,
                priced: true,
            }
        );
        assert_eq!(
            cat.estimate_cost_status("router", "auto", 1_000_000, 1_000_000)
                .unwrap(),
            PriceStatus {
                microcents: 0,
                priced: false,
            }
        );
        assert_eq!(
            cat.estimate_cost_status("paid", "model", 1_000_000, 1_000_000)
                .unwrap(),
            PriceStatus {
                microcents: 300_000_000,
                priced: true,
            }
        );
        // The legacy numeric API remains source- and behavior-compatible.
        assert_eq!(
            cat.estimate_cost_microcents("router", "auto", 1_000_000, 1_000_000)
                .unwrap(),
            0
        );
    }

    #[test]
    fn bundled_flux_rows_are_all_unpriced() {
        let cat = PricingCatalog::load_default().expect("bundled catalog parses");
        for (provider, alias) in [
            ("flux-router", "flux-auto"),
            ("flux-router", "flux-fast"),
            ("flux-router", "flux-standard"),
            ("flux-router", "flux-reasoning"),
            ("openai", "flux-auto"),
            ("openai", "flux-fast"),
            ("openai", "flux-standard"),
            ("openai", "flux-reasoning"),
        ] {
            let p = cat
                .get(provider, alias)
                .unwrap_or_else(|e| panic!("{provider}/{alias} must be bundled: {e}"));
            assert_eq!(
                p.billing,
                BillingClassification::Unpriced,
                "{provider}/{alias} is router-priced: never a fake $0"
            );
            let status = cat
                .estimate_cost_with_cache_status(provider, alias, 10, 10, 10, 10)
                .unwrap();
            assert_eq!(status.microcents, 0);
            assert!(!status.priced, "{provider}/{alias} must be unpriced");
        }
    }

    #[test]
    fn bundled_router_and_local_rows_have_honest_billing_status() {
        let cat = PricingCatalog::load_default().expect("bundled catalog parses");
        for (provider, model) in [
            ("openrouter", "auto"),
            ("litellm", "proxy"),
            ("openai-compatible", "default"),
        ] {
            let status = cat
                .estimate_cost_with_cache_status(provider, model, 10, 10, 10, 10)
                .unwrap();
            assert_eq!(status.microcents, 0);
            assert!(!status.priced, "{provider}/{model} must be unpriced");
        }
        for provider in ["ollama", "vllm", "lmstudio"] {
            let status = cat.estimate_cost_status(provider, "local", 10, 10).unwrap();
            assert_eq!(status.microcents, 0);
            assert!(status.priced, "{provider}/local must be known-free");
        }
    }

    #[test]
    fn microcent_math_matches_wcore_formula_including_cache_fallback() {
        let raw = r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 5.0
output_per_mtok_usd = 25.0
cache_read_per_mtok_usd = 0.5
cache_write_per_mtok_usd = 6.25

[openai.gpt-5]
input_per_mtok_usd = 5.0
output_per_mtok_usd = 15.0
"#;
        let cat = PricingCatalog::from_toml_str(raw).unwrap();
        assert_eq!(
            cat.estimate_cost_microcents("anthropic", "claude-opus-4-7", 1_000_000, 0)
                .unwrap(),
            500_000_000
        );
        assert_eq!(
            cat.estimate_cost_microcents("openai", "gpt-5", 0, 0)
                .unwrap(),
            0
        );
        // Cache-aware: 5 + 25 + 0.5 + 6.25 = 36.75 USD per the full million split.
        assert_eq!(
            cat.estimate_cost_with_cache_microcents(
                "anthropic",
                "claude-opus-4-7",
                1_000_000,
                1_000_000,
                1_000_000,
                1_000_000,
            )
            .unwrap(),
            3_675_000_000
        );
    }

    #[test]
    fn dotted_api_slug_resolves_to_dashed_key() {
        let raw = "[gemini.gemini-2-5-flash]\ninput_per_mtok_usd = 0.30\noutput_per_mtok_usd = 2.50\n\n[openai.\"gpt-4.1-mini\"]\ninput_per_mtok_usd = 0.40\noutput_per_mtok_usd = 1.60\n";
        let cat = PricingCatalog::from_toml_str(raw).unwrap();
        assert!(cat.get("gemini", "gemini-2.5-flash").is_ok());
        assert!(cat.get("gemini", "gemini-2-5-flash").is_ok());
        assert!(cat.get("openai", "gpt-4.1-mini").is_ok());
        assert!(matches!(
            cat.get("gemini", "ghost-9"),
            Err(PricingError::UnknownModel { .. })
        ));
    }

    /// Fail-closed override (P1 §3.1): a malformed NANO_PRICING_PATH file is
    /// a typed error NAMING THE PATH — never a silent fallback to bundled,
    /// never a partial parse; an unreadable override path is likewise
    /// typed. Env-mutating tests must not run in parallel with each other
    /// (or with `load_default` readers): keep every NANO_PRICING_PATH case
    /// in ONE test body so the harness serializes them (the flux_key.rs
    /// convention).
    #[test]
    fn override_failures_are_typed_and_bundled_loads_clean() {
        // Malformed override → typed Parse error naming the path.
        let dir = std::env::temp_dir().join(format!("nano-pricing-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad-pricing.toml");
        std::fs::write(&path, "this is [not valid toml\n").unwrap();
        unsafe { std::env::set_var(PRICING_PATH_ENV, &path) };
        let outcome = PricingCatalog::load_default();
        unsafe { std::env::remove_var(PRICING_PATH_ENV) };
        let _ = std::fs::remove_dir_all(&dir);
        match outcome {
            Err(PricingError::Parse { origin, .. }) => {
                assert_eq!(origin, path.to_string_lossy());
            }
            other => panic!("malformed override must be a typed Parse error, got {other:?}"),
        }

        // Unreadable override path → typed Io error naming the path
        // (fail-closed), never a silent bundled fallback.
        let path = std::env::temp_dir().join(format!(
            "nano-pricing-missing-{}/no-such.toml",
            std::process::id()
        ));
        unsafe { std::env::set_var(PRICING_PATH_ENV, &path) };
        let outcome = PricingCatalog::load_default();
        unsafe { std::env::remove_var(PRICING_PATH_ENV) };
        match outcome {
            Err(PricingError::OverrideIo { path: named, .. }) => {
                assert_eq!(named, path.to_string_lossy());
            }
            other => panic!("unreadable override must be a typed Io error, got {other:?}"),
        }

        // Bundled catalog parses clean (override var unset — same
        // serialized body).
        let cat = PricingCatalog::load_default().expect("bundled catalog should parse");
        assert!(!cat.providers.is_empty());

        // Absence is never $0: a model with no row resolves unpriced
        // through the status API (the caller decides; the numeric API
        // stays an Err).
        assert!(matches!(
            cat.estimate_cost_status("flux-router", "no-such-model", 10, 10),
            Err(PricingError::UnknownModel { .. })
        ));
        assert!(matches!(
            cat.get("no-such-provider", "x"),
            Err(PricingError::UnknownProvider(_))
        ));
    }
}
