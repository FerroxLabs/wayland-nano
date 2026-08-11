//! Picks the newest vendored Flux models snapshot and exports it as
//! `FLUX_MODELS_FIXTURE` (file name only) so the crate can embed the catalog
//! body at compile time. Embedded is the shipped-binary fallback; the
//! on-disk fixture stays the primary source in dev (see flux_models.rs).
use std::path::Path;

fn main() {
    let dir = Path::new("fixtures-flux/models");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("vendored flux models fixture dir must exist")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with("_models.json"))
        .collect();
    files.sort();
    let newest = files
        .pop()
        .expect("at least one *_models.json fixture must exist");
    println!("cargo:rustc-env=FLUX_MODELS_FIXTURE={newest}");
}
