//! Fail-closed v1 marketplace plugin substrate.

pub mod error;
pub mod fetch;
pub mod manifest;
pub mod plan;
pub mod source;
pub mod store;

pub use error::PluginError;
pub use manifest::{MarketplaceManifest, PluginKind, PluginManifest};
pub use plan::{InstallPlan, SpawnPreview};
pub use source::RegistrySource;
pub use store::{InstalledPlugin, PluginStore, RegistryRecord};
