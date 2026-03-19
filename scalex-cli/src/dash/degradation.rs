/// Known-degradation inventory — parses `config/known_degradations.yaml` and provides
/// matching logic so `run_e2e_checks` can downgrade Fail → KnownDegraded for explicitly
/// approved items.
///
/// # Schema
///
/// Each entry in the YAML must have:
///   - `namespace`       — Kubernetes namespace (exact match)
///   - `resource_kind`   — "Pod", "Node", etc. (case-insensitive)
///   - `name`            — resource name; trailing '*' is a prefix glob
///   - `condition`       — condition/reason string (case-insensitive)
///   - `reason`          — human-readable explanation (informational only)
///   - `acknowledged_by` — who approved this known degradation
///   - `ticket`          — tracking ticket or "N/A"
///   - `suppresses_check`— optional list of E2E check names this entry suppresses
///
/// # Purpose
///
/// This is the ONLY place known-acceptable degradations are recorded.
/// Narrative prose in runbooks is NOT sufficient — every known-acceptable
/// degradation must have a structured entry here so automated health
/// re-verification can suppress false-positive alerts without silently
/// hiding real failures.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct KnownDegradation {
    pub namespace: String,
    pub resource_kind: String,
    pub name: String,
    pub condition: String,
    pub reason: String,
    pub acknowledged_by: String,
    #[serde(default = "default_ticket")]
    pub ticket: String,
    /// E2E check names (from `run_e2e_checks`) that this entry suppresses.
    /// If empty, the entry is informational only and suppresses no checks.
    #[serde(default)]
    pub suppresses_check: Vec<String>,
}

fn default_ticket() -> String {
    "N/A".to_string()
}

#[derive(Debug, Clone, Deserialize)]
struct KnownDegradationFile {
    known_degradations: Vec<KnownDegradation>,
}

/// Load known degradations from a YAML file.
///
/// Returns an empty `Vec` if the file is missing — callers treat this as
/// "no known degradations", which is the safe default.
pub fn load(path: &Path) -> Vec<KnownDegradation> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    match serde_yaml::from_str::<KnownDegradationFile>(&content) {
        Ok(f) => f.known_degradations,
        Err(e) => {
            eprintln!(
                "warning: failed to parse {}: {} — ignoring known-degradation file",
                path.display(),
                e
            );
            Vec::new()
        }
    }
}

/// Return `true` if `name` matches `pattern` (trailing `*` is a prefix glob).
pub fn name_matches(pattern: &str, name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        pattern == name
    }
}

/// Return all known-degradation entries that suppress a specific E2E check name.
pub fn suppressors_for_check<'a>(
    inventory: &'a [KnownDegradation],
    check_name: &str,
) -> Vec<&'a KnownDegradation> {
    inventory
        .iter()
        .filter(|e| {
            e.suppresses_check
                .iter()
                .any(|c| c.as_str() == check_name)
        })
        .collect()
}

/// Return `true` if `check_name` is suppressed by at least one entry in `inventory`.
pub fn is_suppressed(inventory: &[KnownDegradation], check_name: &str) -> bool {
    !suppressors_for_check(inventory, check_name).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_prefix() {
        assert!(name_matches("coredns-*", "coredns-5d78c9869d-xqbrf"));
        assert!(!name_matches("coredns-*", "coredns"));
        assert!(name_matches("argocd-dex-server-*", "argocd-dex-server-abc123"));
    }

    #[test]
    fn exact_match() {
        assert!(name_matches("kube-vip", "kube-vip"));
        assert!(!name_matches("kube-vip", "kube-vip-extra"));
    }

    #[test]
    fn suppressor_lookup() {
        let entry = KnownDegradation {
            namespace: "argocd".to_string(),
            resource_kind: "Pod".to_string(),
            name: "argocd-dex-server-*".to_string(),
            condition: "CrashLoopBackOff".to_string(),
            reason: "OIDC not wired".to_string(),
            acknowledged_by: "jinwang".to_string(),
            ticket: "N/A".to_string(),
            suppresses_check: vec!["argocd_synced".to_string()],
        };
        let inventory = vec![entry];
        assert!(is_suppressed(&inventory, "argocd_synced"));
        assert!(!is_suppressed(&inventory, "all_nodes_ready"));
    }
}
