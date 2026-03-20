/// Known-degradation integration for `scalex dash`.
///
/// This module bridges `crate::models::degradation::KnownDegradationsConfig`
/// into the E2E-check pipeline so that `run_e2e_checks` can downgrade
/// `Fail` → `KnownDegraded` for items that are explicitly listed in
/// `config/known_degradations.yaml` with a matching `suppresses_check` entry.
///
/// # Separation of concerns
/// - **Parsing** lives in `crate::models::degradation`.
/// - **Suppression logic** (keyed on E2E check names) lives here.
/// - **Rendering** (colored text for `--once`, TUI color) lives in `mod.rs` / `ui.rs`.
pub use crate::models::degradation::{KnownDegradation, KnownDegradationsConfig};

use std::path::Path;

/// Load the known-degradation inventory from `path`.
///
/// Returns an empty config (zero entries) when the file is absent — callers
/// treat this as "no known degradations", which is the safe default.
pub fn load(path: &Path) -> KnownDegradationsConfig {
    KnownDegradationsConfig::load(path).unwrap_or_default()
}

/// Return `true` if `check_name` is suppressed by at least one entry in `cfg`.
pub fn is_suppressed(cfg: &KnownDegradationsConfig, check_name: &str) -> bool {
    cfg.is_check_suppressed(check_name)
}

/// Return all entries that suppress `check_name`.
pub fn suppressors_for_check<'a>(
    cfg: &'a KnownDegradationsConfig,
    check_name: &str,
) -> Vec<&'a KnownDegradation> {
    cfg.suppressors_for(check_name)
}
