use serde::{Deserialize, Serialize};

/// A single known-acceptable-degradation entry.
///
/// Each entry explicitly declares a Kubernetes resource condition that has been
/// reviewed and approved. Only resources matched by a `KnownDegradation` entry may
/// be suppressed during automated health re-verification; everything else is a live
/// failure. Entries **must** be structured data here — narrative prose in runbooks
/// is not sufficient and will not be consulted at evaluation time.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct KnownDegradation {
    /// Kubernetes namespace of the affected resource (exact match).
    pub namespace: String,

    /// Resource kind (e.g. `"Pod"`, `"Deployment"`, `"Node"`).
    /// Case-insensitive comparison at lookup time.
    pub resource_kind: String,

    /// Resource name or name pattern.
    /// A trailing or leading `*` acts as a glob wildcard
    /// (e.g. `"coredns-*"` matches `"coredns-5d78c9869d-xqbrf"`).
    pub name: String,

    /// Condition type or state string as reported by Kubernetes
    /// (e.g. `"CrashLoopBackOff"`, `"ContainerNotReady"`, `"NodeNotReady"`).
    /// Case-insensitive comparison at lookup time.
    pub condition: String,

    /// Human-readable justification for why this condition is acceptable.
    /// Required; must be non-empty.
    pub reason: String,

    /// Identity of the person who reviewed and approved this entry.
    #[serde(default)]
    pub acknowledged_by: String,

    /// Linked ticket, issue, or design document reference. Defaults to `"N/A"`.
    #[serde(default = "default_ticket")]
    pub ticket: String,
}

fn default_ticket() -> String {
    "N/A".to_string()
}

/// Top-level wrapper matching the `known_degradations.yaml` schema.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct KnownDegradationsConfig {
    /// The list of explicitly approved degradations.
    pub known_degradations: Vec<KnownDegradation>,
}

impl KnownDegradationsConfig {
    /// Load and parse `known_degradations.yaml` from `path`.
    ///
    /// Returns an **empty** config (zero entries) if the file does not exist so that
    /// callers can treat "no file" as "no known degradations" without an error.
    /// Returns `Err` only if the file exists but cannot be read or parsed.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {}", path.display(), e))?;
        let config: KnownDegradationsConfig = serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("parse error in {}: {}", path.display(), e))?;
        Ok(config)
    }

    /// Return `true` when the given `(namespace, resource_kind, name, condition)` tuple
    /// is listed in the known-acceptable-degradation inventory.
    ///
    /// Both `resource_kind` and `condition` comparisons are case-insensitive.
    /// `name` supports `*` glob wildcards (see [`glob_match`]).
    pub fn is_known(
        &self,
        namespace: &str,
        resource_kind: &str,
        name: &str,
        condition: &str,
    ) -> bool {
        self.find_match(namespace, resource_kind, name, condition)
            .is_some()
    }

    /// Return the first matching `KnownDegradation` entry, or `None` if no entry
    /// covers the given `(namespace, resource_kind, name, condition)` tuple.
    pub fn find_match(
        &self,
        namespace: &str,
        resource_kind: &str,
        name: &str,
        condition: &str,
    ) -> Option<&KnownDegradation> {
        self.known_degradations.iter().find(|d| {
            d.namespace == namespace
                && d.resource_kind.eq_ignore_ascii_case(resource_kind)
                && glob_match(&d.name, name)
                && d.condition.eq_ignore_ascii_case(condition)
        })
    }
}

/// Minimal `*`-glob matching used for resource name patterns.
///
/// Rules:
/// - `"*"` matches everything.
/// - Patterns without `*` require an exact string match.
/// - `"prefix-*"` matches any string starting with `"prefix-"`.
/// - `"*-suffix"` matches any string ending with `"-suffix"`.
/// - Multiple `*` tokens are matched left-to-right, consuming the minimum required
///   substring at each step.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == value;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    let mut remaining = value;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            // consecutive `*` or leading/trailing `*` — skip
            continue;
        }
        if i == 0 {
            // pattern must start with this literal prefix
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if i == parts.len() - 1 {
            // pattern must end with this literal suffix
            return remaining.ends_with(part);
        } else {
            // interior part — consume up to and including the first occurrence
            match remaining.find(part) {
                Some(pos) => remaining = &remaining[pos + part.len()..],
                None => return false,
            }
        }
    }
    true
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── glob_match ──

    #[test]
    fn glob_star_matches_everything() {
        assert!(glob_match("*", "anything-at-all"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn glob_exact_match() {
        assert!(glob_match("coredns", "coredns"));
        assert!(!glob_match("coredns", "coredns-abc"));
    }

    #[test]
    fn glob_trailing_star() {
        assert!(glob_match("coredns-*", "coredns-5d78c9869d-xqbrf"));
        assert!(glob_match("coredns-*", "coredns-"));
        assert!(!glob_match("coredns-*", "notcoredns-abc"));
    }

    #[test]
    fn glob_leading_star() {
        assert!(glob_match("*-worker", "node-1-worker"));
        assert!(!glob_match("*-worker", "node-1-master"));
    }

    #[test]
    fn glob_both_ends() {
        assert!(glob_match("*dex*", "argocd-dex-server-7f8b9c-abc"));
        assert!(!glob_match("*dex*", "argocd-server-abc"));
    }

    // ── KnownDegradationsConfig::load ──

    #[test]
    fn load_nonexistent_path_returns_empty() {
        let tmp = std::path::Path::new("/nonexistent/path/known_degradations.yaml");
        let cfg = KnownDegradationsConfig::load(tmp).unwrap();
        assert!(cfg.known_degradations.is_empty());
    }

    #[test]
    fn load_and_parse_yaml() {
        let yaml = r#"
known_degradations:
  - namespace: "kube-system"
    resource_kind: "Pod"
    name: "coredns-*"
    condition: "ContainerNotReady"
    reason: "CoreDNS startup artefact on bootstrap."
    acknowledged_by: "alice"
    ticket: "N/A"
  - namespace: "argocd"
    resource_kind: "Pod"
    name: "argocd-dex-server-*"
    condition: "CrashLoopBackOff"
    reason: "OIDC not configured."
    acknowledged_by: "bob"
"#;
        let cfg: KnownDegradationsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.known_degradations.len(), 2);

        let first = &cfg.known_degradations[0];
        assert_eq!(first.namespace, "kube-system");
        assert_eq!(first.resource_kind, "Pod");
        assert_eq!(first.name, "coredns-*");
        assert_eq!(first.condition, "ContainerNotReady");
        assert!(!first.reason.is_empty());
        assert_eq!(first.acknowledged_by, "alice");
        assert_eq!(first.ticket, "N/A");

        // Default ticket
        let second = &cfg.known_degradations[1];
        assert_eq!(second.ticket, "N/A");
    }

    // ── is_known / find_match ──

    fn make_cfg() -> KnownDegradationsConfig {
        let yaml = r#"
known_degradations:
  - namespace: "kube-system"
    resource_kind: "Pod"
    name: "coredns-*"
    condition: "ContainerNotReady"
    reason: "Startup artefact."
    acknowledged_by: "jinwang"
  - namespace: "argocd"
    resource_kind: "Pod"
    name: "argocd-dex-server-*"
    condition: "CrashLoopBackOff"
    reason: "OIDC not configured."
    acknowledged_by: "jinwang"
  - namespace: "kube-system"
    resource_kind: "Pod"
    name: "kube-vip-*"
    condition: "NodeNotReady"
    reason: "VIP ARP propagation."
    acknowledged_by: "jinwang"
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn is_known_exact_glob_hit() {
        let cfg = make_cfg();
        assert!(cfg.is_known(
            "kube-system",
            "Pod",
            "coredns-5d78c9869d-xqbrf",
            "ContainerNotReady"
        ));
    }

    #[test]
    fn is_known_case_insensitive_kind() {
        let cfg = make_cfg();
        assert!(cfg.is_known(
            "kube-system",
            "pod", // lowercase
            "coredns-abc",
            "containernotready" // lowercase
        ));
    }

    #[test]
    fn is_known_wrong_namespace_returns_false() {
        let cfg = make_cfg();
        assert!(!cfg.is_known(
            "default", // wrong namespace
            "Pod",
            "coredns-abc",
            "ContainerNotReady"
        ));
    }

    #[test]
    fn is_known_wrong_condition_returns_false() {
        let cfg = make_cfg();
        assert!(!cfg.is_known(
            "kube-system",
            "Pod",
            "coredns-abc",
            "CrashLoopBackOff" // different condition
        ));
    }

    #[test]
    fn is_known_name_not_matching_glob_returns_false() {
        let cfg = make_cfg();
        assert!(!cfg.is_known(
            "kube-system",
            "Pod",
            "nginx-abc", // not coredns-*
            "ContainerNotReady"
        ));
    }

    #[test]
    fn find_match_returns_correct_entry() {
        let cfg = make_cfg();
        let entry = cfg
            .find_match("argocd", "Pod", "argocd-dex-server-7f8b9c-abc", "CrashLoopBackOff")
            .unwrap();
        assert_eq!(entry.namespace, "argocd");
        assert_eq!(entry.condition, "CrashLoopBackOff");
        assert!(!entry.reason.is_empty());
    }

    #[test]
    fn find_match_returns_none_when_no_entry() {
        let cfg = make_cfg();
        let result = cfg.find_match("default", "Pod", "nginx-abc", "Pending");
        assert!(result.is_none());
    }

    // ── Round-trip serialization ──

    #[test]
    fn round_trip_serialize_deserialize() {
        let cfg = make_cfg();
        let serialized = serde_yaml::to_string(&cfg).unwrap();
        let restored: KnownDegradationsConfig = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(cfg.known_degradations.len(), restored.known_degradations.len());
        for (orig, rt) in cfg
            .known_degradations
            .iter()
            .zip(restored.known_degradations.iter())
        {
            assert_eq!(orig, rt);
        }
    }

    // ── Load from real temp file ──

    #[test]
    fn load_from_temp_file() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmp,
            r#"known_degradations:
  - namespace: "test-ns"
    resource_kind: "Pod"
    name: "test-pod-*"
    condition: "Pending"
    reason: "Test entry."
    acknowledged_by: "tester"
"#
        )
        .unwrap();
        let cfg = KnownDegradationsConfig::load(tmp.path()).unwrap();
        assert_eq!(cfg.known_degradations.len(), 1);
        assert_eq!(cfg.known_degradations[0].namespace, "test-ns");
    }
}
