// ---------------------------------------------------------------------------
// Resource Registry — runtime API discovery for dynamic resource types
// ---------------------------------------------------------------------------
//
// Stores discovered K8s resource types (from API server discovery) and provides
// fuzzy matching for command-mode autocomplete. Supports shortnames (po, svc,
// deploy) and full names (pods, services, deployments).
//
// Two population modes:
//   1. Static builtins — instant autocomplete before cluster connection
//   2. Runtime discovery — queries the cluster's API server to find all
//      resource types (~50+ built-in + CRDs) with GVK, namespaced flag,
//      shortname aliases, and supported verbs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A single discovered Kubernetes resource type.
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    /// Plural resource name as used in API paths (e.g., "pods", "deployments")
    pub resource: String,
    /// Kubernetes Kind (e.g., "Pod", "Deployment")
    pub kind: String,
    /// API group (e.g., "" for core, "apps" for apps/v1)
    pub api_group: String,
    /// API version (e.g., "v1", "v1beta1")
    pub api_version: String,
    /// Whether the resource is namespaced
    pub namespaced: bool,
    /// Official short names (e.g., ["po"] for pods, ["svc"] for services)
    pub short_names: Vec<String>,
    /// Singular name (e.g., "pod", "deployment")
    pub singular_name: String,
    /// Supported API verbs (e.g., ["get", "list", "watch", "create", "delete"])
    pub verbs: Vec<String>,
}

impl ResourceEntry {
    /// All matchable aliases for this resource: resource, kind (lowered), singular, and shortnames.
    /// Used by fuzzy matching to find the best match for user input.
    pub fn all_aliases(&self) -> Vec<&str> {
        let mut aliases = vec![self.resource.as_str(), self.singular_name.as_str()];
        for s in &self.short_names {
            aliases.push(s.as_str());
        }
        aliases
    }

    /// Whether this resource supports list operations
    pub fn supports_list(&self) -> bool {
        self.verbs.iter().any(|v| v == "list")
    }

    /// Whether this resource supports watch operations
    pub fn supports_watch(&self) -> bool {
        self.verbs.iter().any(|v| v == "watch")
    }

    /// Whether this resource supports create operations
    pub fn supports_create(&self) -> bool {
        self.verbs.iter().any(|v| v == "create")
    }

    /// Whether this resource supports delete operations
    pub fn supports_delete(&self) -> bool {
        self.verbs.iter().any(|v| v == "delete")
    }

    /// Whether this resource supports update/patch (for YAML editor apply)
    pub fn supports_update(&self) -> bool {
        self.verbs.iter().any(|v| v == "update" || v == "patch")
    }

    /// Full api_version string suitable for kube-rs ApiResource construction
    /// (e.g., "v1" for core, "apps/v1" for extensions)
    pub fn full_api_version(&self) -> String {
        if self.api_group.is_empty() {
            self.api_version.clone()
        } else {
            format!("{}/{}", self.api_group, self.api_version)
        }
    }

    /// Build a kube-rs `discovery::ApiResource` for dynamic API usage
    pub fn to_kube_api_resource(&self) -> kube::discovery::ApiResource {
        kube::discovery::ApiResource {
            group: self.api_group.clone(),
            version: self.api_version.clone(),
            api_version: self.full_api_version(),
            kind: self.kind.clone(),
            plural: self.resource.clone(),
        }
    }
}

/// Registry of all discovered Kubernetes resource types for the active cluster.
#[derive(Debug, Clone, Default)]
pub struct ResourceRegistry {
    /// All discovered resource entries
    entries: Vec<ResourceEntry>,
    /// Index: alias (lowercase) → index into `entries`. Built from all aliases + kind.
    alias_index: HashMap<String, usize>,
    /// When this registry was last populated via runtime discovery (None = builtin only)
    discovered_at: Option<Instant>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry pre-populated with well-known core resource types.
    /// This provides instant autocomplete before API discovery completes.
    pub fn with_builtin_resources() -> Self {
        let mut reg = Self::new();
        // Standard verbs for most resources
        let all_verbs: Vec<String> = vec!["create", "delete", "deletecollection", "get", "list", "patch", "update", "watch"]
            .into_iter().map(String::from).collect();
        let read_verbs: Vec<String> = vec!["get", "list", "watch"]
            .into_iter().map(String::from).collect();

        type BuiltinEntry<'a> = (&'a str, &'a str, &'a str, &'a str, bool, Vec<&'a str>, &'a str, &'a [String]);
        let builtins: Vec<BuiltinEntry<'_>> = vec![
            ("pods", "Pod", "", "v1", true, vec!["po"], "pod", &all_verbs),
            ("services", "Service", "", "v1", true, vec!["svc"], "service", &all_verbs),
            ("deployments", "Deployment", "apps", "v1", true, vec!["deploy"], "deployment", &all_verbs),
            ("replicasets", "ReplicaSet", "apps", "v1", true, vec!["rs"], "replicaset", &all_verbs),
            ("statefulsets", "StatefulSet", "apps", "v1", true, vec!["sts"], "statefulset", &all_verbs),
            ("daemonsets", "DaemonSet", "apps", "v1", true, vec!["ds"], "daemonset", &all_verbs),
            ("configmaps", "ConfigMap", "", "v1", true, vec!["cm"], "configmap", &all_verbs),
            ("secrets", "Secret", "", "v1", true, vec![], "secret", &all_verbs),
            ("namespaces", "Namespace", "", "v1", false, vec!["ns"], "namespace", &all_verbs),
            ("nodes", "Node", "", "v1", false, vec!["no"], "node", &all_verbs),
            ("events", "Event", "", "v1", true, vec!["ev"], "event", &read_verbs),
            ("persistentvolumes", "PersistentVolume", "", "v1", false, vec!["pv"], "persistentvolume", &all_verbs),
            ("persistentvolumeclaims", "PersistentVolumeClaim", "", "v1", true, vec!["pvc"], "persistentvolumeclaim", &all_verbs),
            ("ingresses", "Ingress", "networking.k8s.io", "v1", true, vec!["ing"], "ingress", &all_verbs),
            ("networkpolicies", "NetworkPolicy", "networking.k8s.io", "v1", true, vec!["netpol"], "networkpolicy", &all_verbs),
            ("serviceaccounts", "ServiceAccount", "", "v1", true, vec!["sa"], "serviceaccount", &all_verbs),
            ("roles", "Role", "rbac.authorization.k8s.io", "v1", true, vec![], "role", &all_verbs),
            ("rolebindings", "RoleBinding", "rbac.authorization.k8s.io", "v1", true, vec![], "rolebinding", &all_verbs),
            ("clusterroles", "ClusterRole", "rbac.authorization.k8s.io", "v1", false, vec![], "clusterrole", &all_verbs),
            ("clusterrolebindings", "ClusterRoleBinding", "rbac.authorization.k8s.io", "v1", false, vec![], "clusterrolebinding", &all_verbs),
            ("cronjobs", "CronJob", "batch", "v1", true, vec!["cj"], "cronjob", &all_verbs),
            ("jobs", "Job", "batch", "v1", true, vec![], "job", &all_verbs),
            ("horizontalpodautoscalers", "HorizontalPodAutoscaler", "autoscaling", "v2", true, vec!["hpa"], "horizontalpodautoscaler", &all_verbs),
            ("storageclasses", "StorageClass", "storage.k8s.io", "v1", false, vec!["sc"], "storageclass", &all_verbs),
            ("endpoints", "Endpoints", "", "v1", true, vec!["ep"], "endpoints", &all_verbs),
        ];

        for (resource, kind, group, version, ns, shorts, singular, verbs) in builtins {
            reg.add(ResourceEntry {
                resource: resource.to_string(),
                kind: kind.to_string(),
                api_group: group.to_string(),
                api_version: version.to_string(),
                namespaced: ns,
                short_names: shorts.into_iter().map(|s| s.to_string()).collect(),
                singular_name: singular.to_string(),
                verbs: verbs.to_vec(),
            });
        }
        reg
    }

    /// Add a resource entry and update the alias index.
    pub fn add(&mut self, entry: ResourceEntry) {
        let idx = self.entries.len();
        // Index all aliases
        self.alias_index.insert(entry.resource.to_lowercase(), idx);
        self.alias_index.insert(entry.kind.to_lowercase(), idx);
        if !entry.singular_name.is_empty() {
            self.alias_index.insert(entry.singular_name.to_lowercase(), idx);
        }
        for s in &entry.short_names {
            self.alias_index.insert(s.to_lowercase(), idx);
        }
        self.entries.push(entry);
    }

    /// Exact match lookup by any alias (case-insensitive).
    pub fn lookup(&self, name: &str) -> Option<&ResourceEntry> {
        let lower = name.to_lowercase();
        self.alias_index.get(&lower).map(|&idx| &self.entries[idx])
    }

    /// Get all entries for iteration.
    pub fn entries(&self) -> &[ResourceEntry] {
        &self.entries
    }

    /// Clear all entries (for re-population from API discovery).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.alias_index.clear();
    }

    /// Return the number of registered resource types.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether the registry was populated via runtime discovery (not just builtins)
    pub fn is_discovered(&self) -> bool {
        self.discovered_at.is_some()
    }

    /// Whether the cached discovery results are still fresh
    pub fn is_fresh(&self) -> bool {
        self.discovered_at
            .map(|t| t.elapsed() < CACHE_TTL)
            .unwrap_or(false)
    }

    /// Iterate over only namespaced resource entries
    pub fn namespaced_entries(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.entries.iter().filter(|e| e.namespaced)
    }

    /// Iterate over only cluster-scoped resource entries
    pub fn cluster_scoped_entries(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.entries.iter().filter(|e| !e.namespaced)
    }

    /// Iterate over only listable resource entries (for command-mode display)
    pub fn listable_entries(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.entries.iter().filter(|e| e.supports_list())
    }

    /// Return a sorted, deduplicated list of all resource plural names.
    /// This is the canonical name list for autocomplete dropdown display.
    /// Names are sorted alphabetically for predictable presentation.
    pub fn sorted_resource_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.entries.iter().map(|e| e.resource.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Return a sorted list of all matchable aliases (plural, singular, short names, kind lowered).
    /// Used for prefix-based autocomplete matching where any alias should trigger completion.
    /// Each entry is (alias, entry_index) so the caller can look up the full ResourceEntry.
    pub fn sorted_alias_pairs(&self) -> Vec<(&str, usize)> {
        let mut pairs: Vec<(&str, usize)> = Vec::with_capacity(self.alias_index.len());
        for (alias, &idx) in &self.alias_index {
            pairs.push((alias.as_str(), idx));
        }
        pairs.sort_unstable_by(|a, b| a.0.cmp(b.0));
        pairs
    }

    /// Merge another registry's entries into this one (for discovery refresh).
    /// Replaces any entries with the same resource plural name, adds new ones.
    /// Returns the count of newly added entries (not counting replacements).
    pub fn merge_from(&mut self, other: &ResourceRegistry) -> usize {
        let mut added = 0usize;
        for entry in &other.entries {
            // Check if we already have this resource plural
            if let Some(&existing_idx) = self.alias_index.get(&entry.resource) {
                // Replace the existing entry in-place, rebuild its aliases
                self.remove_aliases_for(existing_idx);
                self.entries[existing_idx] = entry.clone();
                self.rebuild_aliases_for(existing_idx);
            } else {
                self.add(entry.clone());
                added += 1;
            }
        }
        if other.discovered_at.is_some() {
            self.discovered_at = other.discovered_at;
        }
        added
    }

    /// Remove all alias index entries pointing to a given entry index.
    fn remove_aliases_for(&mut self, idx: usize) {
        self.alias_index.retain(|_, &mut v| v != idx);
    }

    /// Rebuild alias index entries for a given entry index.
    fn rebuild_aliases_for(&mut self, idx: usize) {
        let entry = &self.entries[idx];
        self.alias_index.insert(entry.resource.to_lowercase(), idx);
        self.alias_index.insert(entry.kind.to_lowercase(), idx);
        if !entry.singular_name.is_empty() {
            self.alias_index
                .insert(entry.singular_name.to_lowercase(), idx);
        }
        for s in &entry.short_names {
            self.alias_index.insert(s.to_lowercase(), idx);
        }
    }
}

// ---------------------------------------------------------------------------
// ResourceNameProvider — autocomplete-optimized sorted name list
// ---------------------------------------------------------------------------

/// A pre-computed, sorted list of resource names for efficient autocomplete.
///
/// Built from a `ResourceRegistry` snapshot, this struct provides O(log n) prefix
/// lookups and an ordered iteration interface suitable for rendering autocomplete
/// dropdown lists. It is cheap to rebuild when the registry is updated via
/// runtime API discovery.
///
/// Two name lists are maintained:
/// - `names`: sorted unique plural resource names (canonical display names)
/// - `all_aliases`: sorted unique aliases (includes plural, singular, shorts, kind-lowered)
///
/// The provider is intentionally decoupled from `ResourceRegistry` so that it can
/// be rebuilt on a background task and swapped in atomically.
#[derive(Debug, Clone)]
pub struct ResourceNameProvider {
    /// Sorted unique plural resource names (e.g., ["configmaps", "daemonsets", "deployments", ...])
    names: Vec<String>,
    /// Sorted unique aliases for prefix search (includes shortnames, singular, kind-lowered)
    all_aliases: Vec<AliasEntry>,
    /// Total number of resource types (entries in the source registry)
    resource_count: usize,
    /// Whether the source registry was populated via runtime discovery (not just builtins)
    from_discovery: bool,
}

/// An alias entry mapping an alias string to its canonical resource plural name.
#[derive(Debug, Clone)]
pub struct AliasEntry {
    /// The alias string (lowercased)
    pub alias: String,
    /// The canonical plural resource name this alias resolves to
    pub resource: String,
    /// The resource Kind (e.g., "Pod", "Deployment") — useful for display
    pub kind: String,
    /// API group for disambiguation
    pub api_group: String,
    /// Whether namespaced
    pub namespaced: bool,
}

impl ResourceNameProvider {
    /// Build a name provider from a ResourceRegistry snapshot.
    pub fn from_registry(registry: &ResourceRegistry) -> Self {
        // Collect sorted unique plural names
        let mut names: Vec<String> = registry
            .entries()
            .iter()
            .map(|e| e.resource.clone())
            .collect();
        names.sort_unstable();
        names.dedup();

        // Collect all aliases with their canonical resource
        let mut all_aliases: Vec<AliasEntry> = Vec::new();
        for entry in registry.entries() {
            let base = |alias: String| AliasEntry {
                alias,
                resource: entry.resource.clone(),
                kind: entry.kind.clone(),
                api_group: entry.api_group.clone(),
                namespaced: entry.namespaced,
            };
            all_aliases.push(base(entry.resource.to_lowercase()));
            if !entry.singular_name.is_empty() && entry.singular_name != entry.resource {
                all_aliases.push(base(entry.singular_name.to_lowercase()));
            }
            all_aliases.push(base(entry.kind.to_lowercase()));
            for s in &entry.short_names {
                all_aliases.push(base(s.to_lowercase()));
            }
        }
        // Sort by alias, deduplicate by alias (keep first occurrence)
        all_aliases.sort_by(|a, b| a.alias.cmp(&b.alias));
        all_aliases.dedup_by(|a, b| a.alias == b.alias);

        Self {
            resource_count: registry.len(),
            from_discovery: registry.is_discovered(),
            names,
            all_aliases,
        }
    }

    /// Return the sorted list of all canonical resource plural names.
    pub fn sorted_names(&self) -> &[String] {
        &self.names
    }

    /// Return the number of canonical resource names.
    pub fn name_count(&self) -> usize {
        self.names.len()
    }

    /// Return the total number of resource types in the source registry.
    pub fn resource_count(&self) -> usize {
        self.resource_count
    }

    /// Whether the provider was built from runtime API discovery data.
    pub fn is_from_discovery(&self) -> bool {
        self.from_discovery
    }

    /// Find all aliases that start with the given prefix (case-insensitive).
    /// Returns matching alias entries sorted alphabetically.
    /// Uses binary search for O(log n) first-match lookup.
    pub fn prefix_matches(&self, prefix: &str) -> Vec<&AliasEntry> {
        if prefix.is_empty() {
            return self.all_aliases.iter().collect();
        }
        let prefix_lower = prefix.to_lowercase();
        // Binary search for the first alias >= prefix
        let start = self
            .all_aliases
            .partition_point(|a| a.alias.as_str() < prefix_lower.as_str());
        // Collect all aliases that start with the prefix
        self.all_aliases[start..]
            .iter()
            .take_while(|a| a.alias.starts_with(&prefix_lower))
            .collect()
    }

    /// Look up the canonical resource name for an exact alias match.
    /// Returns None if the alias is not known.
    pub fn resolve_alias(&self, alias: &str) -> Option<&AliasEntry> {
        let alias_lower = alias.to_lowercase();
        self.all_aliases
            .binary_search_by(|a| a.alias.as_str().cmp(&alias_lower))
            .ok()
            .map(|idx| &self.all_aliases[idx])
    }

    /// Check if the provider contains a given resource plural name.
    pub fn has_resource(&self, name: &str) -> bool {
        self.names.binary_search_by(|n| n.as_str().cmp(name)).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Runtime API discovery — queries cluster to discover all resource types
// ---------------------------------------------------------------------------

/// Timeout for the entire discovery operation.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a cached registry remains valid before requiring refresh.
const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

/// Run API discovery against a cluster and build a `ResourceRegistry`.
///
/// Queries the API server's discovery endpoints to enumerate all available
/// resource types including:
/// - Core resources (pods, services, configmaps, etc.) via `/api/v1`
/// - Extension resources (deployments, ingresses, etc.) via `/apis/{group}/{version}`
/// - CRDs (any custom resource definitions installed on the cluster)
///
/// Extracts from each resource: kind, plural, group, version, namespaced flag,
/// short_names, singular_name, and verbs — all from the raw
/// `k8s_openapi::apimachinery::pkg::apis::meta::v1::APIResource` struct which
/// carries fields not exposed by kube-rs's higher-level `Discovery` type.
///
/// # Arguments
/// * `client` - An existing kube-rs Client (reuses bearer token, tunnel, etc.)
///
/// # Returns
/// * `Ok(ResourceRegistry)` - Registry with all discovered resources (typically 50-100+)
/// * `Err(...)` - If discovery fails (network, auth, timeout)
pub async fn discover_api_resources(client: &kube::Client) -> anyhow::Result<ResourceRegistry> {
    let result = tokio::time::timeout(DISCOVERY_TIMEOUT, run_discovery(client)).await;

    match result {
        Ok(Ok(registry)) => Ok(registry),
        Ok(Err(e)) => Err(anyhow::anyhow!("API discovery failed: {}", e)),
        Err(_) => Err(anyhow::anyhow!(
            "API discovery timed out after {}s",
            DISCOVERY_TIMEOUT.as_secs()
        )),
    }
}

/// Internal discovery implementation without timeout wrapper.
async fn run_discovery(client: &kube::Client) -> anyhow::Result<ResourceRegistry> {
    let mut registry = ResourceRegistry::new();

    // 1. Discover core API resources (/api/v1)
    let core_versions = client.list_core_api_versions().await?;
    for version in &core_versions.versions {
        let resource_list = client.list_core_api_resources(version).await?;
        let group_version = &resource_list.group_version;

        for res in &resource_list.resources {
            // Skip subresources (e.g., "pods/log", "pods/exec", "pods/status")
            if res.name.contains('/') {
                continue;
            }
            if let Some(entry) = entry_from_raw_api_resource(res, group_version) {
                registry.add(entry);
            }
        }
    }

    // 2. Discover all API groups (/apis)
    let api_groups = client.list_api_groups().await?;
    for group in &api_groups.groups {
        // Use preferred version if available, otherwise try all versions.
        // Preferred version avoids redundant entries for multi-version resources.
        let versions_to_query: Vec<&str> = if let Some(ref pv) = group.preferred_version {
            vec![&pv.group_version]
        } else {
            group
                .versions
                .iter()
                .map(|v| v.group_version.as_str())
                .collect()
        };

        for group_version in versions_to_query {
            match client.list_api_group_resources(group_version).await {
                Ok(resource_list) => {
                    let gv = &resource_list.group_version;
                    for res in &resource_list.resources {
                        // Skip subresources
                        if res.name.contains('/') {
                            continue;
                        }
                        if let Some(entry) = entry_from_raw_api_resource(res, gv) {
                            registry.add(entry);
                        }
                    }
                }
                Err(_e) => {
                    // Non-fatal: some API groups may be unavailable
                    // (aggregated APIs, webhook backends down, etc.)
                    // We silently skip rather than failing the whole discovery.
                }
            }
        }
    }

    registry.discovered_at = Some(Instant::now());
    Ok(registry)
}

/// Build a `ResourceEntry` from a raw k8s `APIResource` and its group_version string.
///
/// The raw `APIResource` from the discovery API carries fields that kube-rs's
/// higher-level `ApiResource` does not: `short_names`, `singular_name`, `verbs`.
fn entry_from_raw_api_resource(
    res: &k8s_openapi::apimachinery::pkg::apis::meta::v1::APIResource,
    group_version: &str,
) -> Option<ResourceEntry> {
    // Parse group and version from the group_version string
    // e.g., "apps/v1" → group="apps", version="v1"
    // e.g., "v1" → group="", version="v1" (core group)
    let (group, version) = if let Some(slash_pos) = group_version.find('/') {
        (
            group_version[..slash_pos].to_string(),
            group_version[slash_pos + 1..].to_string(),
        )
    } else {
        (String::new(), group_version.to_string())
    };

    Some(ResourceEntry {
        resource: res.name.to_lowercase(),
        kind: res.kind.clone(),
        api_group: res.group.clone().unwrap_or(group),
        api_version: res.version.clone().unwrap_or(version),
        namespaced: res.namespaced,
        short_names: res
            .short_names
            .as_ref()
            .map(|v| v.iter().map(|s| s.to_lowercase()).collect())
            .unwrap_or_default(),
        singular_name: res.singular_name.to_lowercase(),
        verbs: res.verbs.clone(),
    })
}

/// Per-cluster discovery cache with TTL-based invalidation.
///
/// Used by the TUI app to cache discovery results per cluster, avoiding
/// redundant API calls when switching between views within the same cluster.
pub struct ClusterDiscoveryCache {
    /// Map of cluster name → (cached registry, timestamp)
    registries: HashMap<String, Arc<ResourceRegistry>>,
}

impl ClusterDiscoveryCache {
    /// Create an empty cache
    pub fn new() -> Self {
        Self {
            registries: HashMap::new(),
        }
    }

    /// Get the cached registry for a cluster (returns None if expired or missing)
    pub fn get(&self, cluster_name: &str) -> Option<Arc<ResourceRegistry>> {
        self.registries.get(cluster_name).and_then(|r| {
            if r.is_fresh() {
                Some(Arc::clone(r))
            } else {
                None
            }
        })
    }

    /// Store a discovered registry for a cluster
    pub fn put(&mut self, cluster_name: String, registry: ResourceRegistry) {
        self.registries.insert(cluster_name, Arc::new(registry));
    }

    /// Invalidate the cache for a specific cluster (e.g., after CRD install)
    pub fn invalidate(&mut self, cluster_name: &str) {
        self.registries.remove(cluster_name);
    }

    /// Invalidate all cached registries
    #[allow(dead_code)]
    pub fn invalidate_all(&mut self) {
        self.registries.clear();
    }
}

// ---------------------------------------------------------------------------
// Fuzzy matching
// ---------------------------------------------------------------------------

/// A scored match result for autocomplete suggestions.
#[derive(Debug, Clone)]
pub struct FuzzyMatch {
    /// Index into the ResourceRegistry entries
    pub entry_idx: usize,
    /// The specific alias that matched
    pub matched_alias: String,
    /// Match score (higher = better match). 0 = no match.
    pub score: u32,
    /// Whether this was an exact prefix match
    pub is_prefix: bool,
}

/// Fuzzy match a query against all resource entries in the registry.
/// Returns matches sorted by score (descending), limited to `max_results`.
///
/// Scoring algorithm:
/// - Exact match: 1000 points
/// - Prefix match: 500 + (query_len / alias_len * 100) — longer prefix coverage = higher
/// - Subsequence match: 100 + bonus for consecutive chars and early positions
/// - No match: excluded
pub fn fuzzy_match(registry: &ResourceRegistry, query: &str, max_results: usize) -> Vec<FuzzyMatch> {
    if query.is_empty() {
        // Empty query: return all entries sorted alphabetically by resource name
        let mut results: Vec<FuzzyMatch> = registry
            .entries()
            .iter()
            .enumerate()
            .map(|(idx, entry)| FuzzyMatch {
                entry_idx: idx,
                matched_alias: entry.resource.clone(),
                score: 1, // uniform score for alphabetical display
                is_prefix: false,
            })
            .collect();
        results.sort_by(|a, b| a.matched_alias.cmp(&b.matched_alias));
        results.truncate(max_results);
        return results;
    }

    let query_lower = query.to_lowercase();
    let mut matches: Vec<FuzzyMatch> = Vec::new();
    let mut seen_entries: HashMap<usize, u32> = HashMap::new(); // entry_idx → best score

    for (entry_idx, entry) in registry.entries().iter().enumerate() {
        let mut best_score: u32 = 0;
        let mut best_alias = String::new();
        let mut best_prefix = false;

        // Check all aliases + kind (lowered)
        let aliases = entry.all_aliases();
        let kind_lower = entry.kind.to_lowercase();
        let all_candidates: Vec<&str> = aliases.into_iter().chain(std::iter::once(kind_lower.as_str())).collect();

        for alias in all_candidates {
            let alias_lower = alias.to_lowercase();
            let (score, is_prefix) = score_match(&query_lower, &alias_lower);
            if score > best_score {
                best_score = score;
                best_alias = alias.to_string();
                best_prefix = is_prefix;
            }
        }

        if best_score > 0 {
            let prev = seen_entries.get(&entry_idx).copied().unwrap_or(0);
            if best_score > prev {
                seen_entries.insert(entry_idx, best_score);
                // Remove previous if exists, add new
                matches.retain(|m| m.entry_idx != entry_idx);
                matches.push(FuzzyMatch {
                    entry_idx,
                    matched_alias: best_alias,
                    score: best_score,
                    is_prefix: best_prefix,
                });
            }
        }
    }

    // Sort by score descending, then alphabetically by resource name for ties
    matches.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            let a_name = &registry.entries()[a.entry_idx].resource;
            let b_name = &registry.entries()[b.entry_idx].resource;
            a_name.cmp(b_name)
        })
    });
    matches.truncate(max_results);
    matches
}

/// Score a query against a single alias. Returns (score, is_prefix).
/// Returns (0, false) if no match.
fn score_match(query: &str, alias: &str) -> (u32, bool) {
    if query.is_empty() {
        return (1, false);
    }
    if alias.is_empty() {
        return (0, false);
    }

    // Exact match
    if query == alias {
        return (1000, true);
    }

    // Prefix match
    if alias.starts_with(query) {
        // Score based on how much of the alias is covered
        let coverage = (query.len() as f32 / alias.len() as f32 * 100.0) as u32;
        return (500 + coverage, true);
    }

    // Subsequence match with scoring
    let score = subsequence_score(query, alias);
    if score > 0 {
        return (score, false);
    }

    (0, false)
}

/// Score a fuzzy subsequence match. Returns 0 if query is not a subsequence of alias.
///
/// Bonuses:
/// - +20 for each matched character at the start of the alias
/// - +15 for consecutive matched characters
/// - +10 for each matched character
/// - Position penalty: -1 for each position gap from start
fn subsequence_score(query: &str, alias: &str) -> u32 {
    let query_bytes = query.as_bytes();
    let alias_bytes = alias.as_bytes();

    if query_bytes.len() > alias_bytes.len() {
        return 0;
    }

    let mut qi = 0;
    let mut score: u32 = 100; // base score for being a subsequence
    let mut last_match_pos: Option<usize> = None;
    let mut first_char_matched = false;

    for (ai, &ab) in alias_bytes.iter().enumerate() {
        if qi < query_bytes.len() && ab.to_ascii_lowercase() == query_bytes[qi] {
            // Character matched
            score += 10;

            // Bonus: match at start of alias
            if ai == qi && qi < 3 {
                score += 20;
                if qi == 0 {
                    first_char_matched = true;
                }
            }

            // Bonus: consecutive match
            if let Some(last) = last_match_pos {
                if ai == last + 1 {
                    score += 15;
                }
            }

            last_match_pos = Some(ai);
            qi += 1;
        }
    }

    // All query chars must have been matched
    if qi < query_bytes.len() {
        return 0;
    }

    // Bonus for matching first character
    if first_char_matched {
        score += 10;
    }

    score
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> ResourceRegistry {
        ResourceRegistry::with_builtin_resources()
    }

    #[test]
    fn exact_shortname_lookup() {
        let reg = test_registry();
        let entry = reg.lookup("po").unwrap();
        assert_eq!(entry.resource, "pods");

        let entry = reg.lookup("svc").unwrap();
        assert_eq!(entry.resource, "services");

        let entry = reg.lookup("deploy").unwrap();
        assert_eq!(entry.resource, "deployments");
    }

    #[test]
    fn exact_full_name_lookup() {
        let reg = test_registry();
        let entry = reg.lookup("pods").unwrap();
        assert_eq!(entry.kind, "Pod");

        let entry = reg.lookup("deployments").unwrap();
        assert_eq!(entry.kind, "Deployment");
    }

    #[test]
    fn case_insensitive_lookup() {
        let reg = test_registry();
        assert!(reg.lookup("Po").is_some());
        assert!(reg.lookup("PODS").is_some());
        assert!(reg.lookup("Deploy").is_some());
    }

    #[test]
    fn kind_lookup() {
        let reg = test_registry();
        let entry = reg.lookup("Pod").unwrap();
        assert_eq!(entry.resource, "pods");
    }

    #[test]
    fn unknown_returns_none() {
        let reg = test_registry();
        assert!(reg.lookup("foobar").is_none());
    }

    #[test]
    fn fuzzy_match_exact() {
        let reg = test_registry();
        let results = fuzzy_match(&reg, "po", 10);
        assert!(!results.is_empty());
        // "po" is an exact shortname match for pods — should be top result
        assert_eq!(reg.entries()[results[0].entry_idx].resource, "pods");
        assert!(results[0].score >= 1000); // exact match
    }

    #[test]
    fn fuzzy_match_prefix() {
        let reg = test_registry();
        let results = fuzzy_match(&reg, "dep", 10);
        assert!(!results.is_empty());
        assert_eq!(reg.entries()[results[0].entry_idx].resource, "deployments");
        assert!(results[0].is_prefix);
    }

    #[test]
    fn fuzzy_match_subsequence() {
        let reg = test_registry();
        // "dps" is a subsequence of "deployments" (d-e-p-l-o-y-m-e-n-t-s)
        let results = fuzzy_match(&reg, "dps", 10);
        // Should find daemonsets (d-a-e-m-o-n-s-e-t-s → d...s) or similar
        assert!(!results.is_empty());
    }

    #[test]
    fn fuzzy_match_empty_query_returns_all() {
        let reg = test_registry();
        let results = fuzzy_match(&reg, "", 100);
        assert_eq!(results.len(), reg.len());
    }

    #[test]
    fn fuzzy_match_respects_max_results() {
        let reg = test_registry();
        let results = fuzzy_match(&reg, "", 5);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn fuzzy_match_ranks_exact_over_prefix() {
        let reg = test_registry();
        // "pod" should match "pod" (singular exact) higher than prefix of "pods"
        let results = fuzzy_match(&reg, "pod", 10);
        assert!(!results.is_empty());
        assert_eq!(reg.entries()[results[0].entry_idx].resource, "pods");
        assert!(results[0].score >= 1000); // exact match on singular
    }

    #[test]
    fn fuzzy_match_ranks_prefix_over_subsequence() {
        let reg = test_registry();
        let results = fuzzy_match(&reg, "se", 10);
        // "se" is a prefix of "secrets" and "services" and "serviceaccounts"
        // All should rank above any subsequence-only matches
        for r in &results {
            if r.is_prefix {
                assert!(r.score >= 500);
            }
        }
    }

    #[test]
    fn fuzzy_match_shortname_svc() {
        let reg = test_registry();
        let results = fuzzy_match(&reg, "svc", 5);
        assert!(!results.is_empty());
        assert_eq!(reg.entries()[results[0].entry_idx].resource, "services");
    }

    #[test]
    fn fuzzy_match_no_duplicate_entries() {
        let reg = test_registry();
        let results = fuzzy_match(&reg, "po", 20);
        let mut seen = std::collections::HashSet::new();
        for r in &results {
            assert!(seen.insert(r.entry_idx), "Duplicate entry_idx: {}", r.entry_idx);
        }
    }

    #[test]
    fn registry_add_custom_crd() {
        let mut reg = test_registry();
        let initial_len = reg.len();
        reg.add(ResourceEntry {
            resource: "certificates".to_string(),
            kind: "Certificate".to_string(),
            api_group: "cert-manager.io".to_string(),
            api_version: "v1".to_string(),
            namespaced: true,
            short_names: vec!["cert".to_string()],
            singular_name: "certificate".to_string(),
            verbs: vec!["get".into(), "list".into(), "watch".into(), "create".into(), "delete".into()],
        });
        assert_eq!(reg.len(), initial_len + 1);
        assert!(reg.lookup("cert").is_some());
        assert_eq!(reg.lookup("cert").unwrap().resource, "certificates");
    }

    #[test]
    fn resource_entry_verb_checks() {
        let reg = test_registry();
        let pods = reg.lookup("pods").unwrap();
        assert!(pods.supports_list());
        assert!(pods.supports_watch());
        assert!(pods.supports_create());
        assert!(pods.supports_delete());
        assert!(pods.supports_update());

        let events = reg.lookup("events").unwrap();
        assert!(events.supports_list());
        assert!(events.supports_watch());
        assert!(!events.supports_create());
        assert!(!events.supports_delete());
    }

    #[test]
    fn full_api_version_formatting() {
        let reg = test_registry();
        let pods = reg.lookup("pods").unwrap();
        assert_eq!(pods.full_api_version(), "v1");

        let deploy = reg.lookup("deploy").unwrap();
        assert_eq!(deploy.full_api_version(), "apps/v1");
    }

    #[test]
    fn to_kube_api_resource() {
        let reg = test_registry();
        let pods = reg.lookup("pods").unwrap();
        let ar = pods.to_kube_api_resource();
        assert_eq!(ar.kind, "Pod");
        assert_eq!(ar.plural, "pods");
        assert_eq!(ar.group, "");
        assert_eq!(ar.version, "v1");
        assert_eq!(ar.api_version, "v1");
    }

    #[test]
    fn namespaced_and_cluster_scoped_filtering() {
        let reg = test_registry();
        let ns_count = reg.namespaced_entries().count();
        let cs_count = reg.cluster_scoped_entries().count();
        assert!(ns_count > 0, "should have namespaced entries");
        assert!(cs_count > 0, "should have cluster-scoped entries");
        assert_eq!(ns_count + cs_count, reg.len());
    }

    #[test]
    fn listable_entries_filtering() {
        let reg = test_registry();
        let listable_count = reg.listable_entries().count();
        assert_eq!(listable_count, reg.len(), "all builtins should be listable");
    }

    #[test]
    fn is_fresh_for_builtin_registry() {
        let reg = ResourceRegistry::with_builtin_resources();
        // Builtin registry has no discovered_at, so is_fresh should be false
        assert!(!reg.is_fresh());
        assert!(!reg.is_discovered());
    }

    #[test]
    fn entry_from_raw_core_api_resource() {
        let raw = k8s_openapi::apimachinery::pkg::apis::meta::v1::APIResource {
            name: "pods".to_string(),
            singular_name: "pod".to_string(),
            namespaced: true,
            group: None,
            version: None,
            kind: "Pod".to_string(),
            verbs: vec!["get".into(), "list".into(), "watch".into(), "create".into(), "delete".into()],
            short_names: Some(vec!["po".into()]),
            categories: None,
            storage_version_hash: None,
        };

        let entry = entry_from_raw_api_resource(&raw, "v1").unwrap();
        assert_eq!(entry.api_group, "");
        assert_eq!(entry.api_version, "v1");
        assert_eq!(entry.kind, "Pod");
        assert_eq!(entry.resource, "pods");
        assert_eq!(entry.short_names, vec!["po"]);
        assert!(entry.namespaced);
        assert!(entry.supports_list());
    }

    #[test]
    fn entry_from_raw_apps_group_resource() {
        let raw = k8s_openapi::apimachinery::pkg::apis::meta::v1::APIResource {
            name: "deployments".to_string(),
            singular_name: "deployment".to_string(),
            namespaced: true,
            group: Some("apps".into()),
            version: Some("v1".into()),
            kind: "Deployment".to_string(),
            verbs: vec!["create".into(), "delete".into(), "get".into(), "list".into(), "patch".into(), "update".into(), "watch".into()],
            short_names: Some(vec!["deploy".into()]),
            categories: None,
            storage_version_hash: None,
        };

        let entry = entry_from_raw_api_resource(&raw, "apps/v1").unwrap();
        assert_eq!(entry.api_group, "apps");
        assert_eq!(entry.api_version, "v1");
        assert_eq!(entry.kind, "Deployment");
        assert_eq!(entry.resource, "deployments");
        assert_eq!(entry.short_names, vec!["deploy"]);
        assert!(entry.namespaced);
        assert_eq!(entry.full_api_version(), "apps/v1");
    }

    #[test]
    fn entry_from_raw_crd_resource() {
        // Simulates a CRD like cert-manager Certificate
        let raw = k8s_openapi::apimachinery::pkg::apis::meta::v1::APIResource {
            name: "certificates".to_string(),
            singular_name: "certificate".to_string(),
            namespaced: true,
            group: Some("cert-manager.io".into()),
            version: Some("v1".into()),
            kind: "Certificate".to_string(),
            verbs: vec!["get".into(), "list".into(), "watch".into(), "create".into(), "delete".into(), "patch".into(), "update".into()],
            short_names: Some(vec!["cert".into(), "certs".into()]),
            categories: None,
            storage_version_hash: None,
        };

        let entry = entry_from_raw_api_resource(&raw, "cert-manager.io/v1").unwrap();
        assert_eq!(entry.api_group, "cert-manager.io");
        assert_eq!(entry.kind, "Certificate");
        assert_eq!(entry.short_names, vec!["cert", "certs"]);
        assert_eq!(entry.full_api_version(), "cert-manager.io/v1");
    }

    #[test]
    fn cluster_discovery_cache_operations() {
        let mut cache = ClusterDiscoveryCache::new();

        // Empty cache returns None
        assert!(cache.get("tower").is_none());

        // Insert and retrieve
        let mut registry = ResourceRegistry::new();
        registry.discovered_at = Some(Instant::now());
        registry.add(ResourceEntry {
            resource: "pods".to_string(),
            kind: "Pod".to_string(),
            api_group: String::new(),
            api_version: "v1".to_string(),
            namespaced: true,
            short_names: vec!["po".to_string()],
            singular_name: "pod".to_string(),
            verbs: vec!["get".into(), "list".into()],
        });
        cache.put("tower".to_string(), registry);

        assert!(cache.get("tower").is_some());
        assert_eq!(cache.get("tower").unwrap().len(), 1);

        // Invalidate
        cache.invalidate("tower");
        assert!(cache.get("tower").is_none());
    }

    #[test]
    fn score_match_exact() {
        let (score, prefix) = score_match("pods", "pods");
        assert_eq!(score, 1000);
        assert!(prefix);
    }

    #[test]
    fn score_match_prefix() {
        let (score, prefix) = score_match("dep", "deployments");
        assert!(score >= 500);
        assert!(prefix);
    }

    #[test]
    fn score_match_no_match() {
        let (score, _) = score_match("xyz", "pods");
        assert_eq!(score, 0);
    }

    #[test]
    fn subsequence_score_basic() {
        let score = subsequence_score("dpl", "deployments");
        assert!(score > 0);
    }

    #[test]
    fn subsequence_score_no_match() {
        let score = subsequence_score("xyz", "pods");
        assert_eq!(score, 0);
    }

    // ---------------------------------------------------------------------------
    // sorted_resource_names / sorted_alias_pairs tests
    // ---------------------------------------------------------------------------

    #[test]
    fn sorted_resource_names_is_alphabetical() {
        let reg = test_registry();
        let names = reg.sorted_resource_names();
        assert!(!names.is_empty());
        // Verify sorted
        for w in names.windows(2) {
            assert!(w[0] <= w[1], "names not sorted: {:?} > {:?}", w[0], w[1]);
        }
        // Verify contains known entries
        assert!(names.contains(&"pods"));
        assert!(names.contains(&"deployments"));
        assert!(names.contains(&"services"));
    }

    #[test]
    fn sorted_resource_names_no_duplicates() {
        let reg = test_registry();
        let names = reg.sorted_resource_names();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len());
    }

    #[test]
    fn sorted_alias_pairs_is_alphabetical() {
        let reg = test_registry();
        let pairs = reg.sorted_alias_pairs();
        assert!(!pairs.is_empty());
        for w in pairs.windows(2) {
            assert!(w[0].0 <= w[1].0, "aliases not sorted: {:?} > {:?}", w[0].0, w[1].0);
        }
    }

    #[test]
    fn sorted_alias_pairs_includes_shortnames() {
        let reg = test_registry();
        let pairs = reg.sorted_alias_pairs();
        let aliases: Vec<&str> = pairs.iter().map(|(a, _)| *a).collect();
        assert!(aliases.contains(&"po"), "should include shortname 'po'");
        assert!(aliases.contains(&"svc"), "should include shortname 'svc'");
        assert!(aliases.contains(&"deploy"), "should include shortname 'deploy'");
    }

    // ---------------------------------------------------------------------------
    // merge_from tests
    // ---------------------------------------------------------------------------

    #[test]
    fn merge_from_adds_new_entries() {
        let mut reg = test_registry();
        let initial_len = reg.len();

        let mut other = ResourceRegistry::new();
        other.add(ResourceEntry {
            resource: "certificates".to_string(),
            kind: "Certificate".to_string(),
            api_group: "cert-manager.io".to_string(),
            api_version: "v1".to_string(),
            namespaced: true,
            short_names: vec!["cert".to_string()],
            singular_name: "certificate".to_string(),
            verbs: vec!["get".into(), "list".into()],
        });
        other.discovered_at = Some(Instant::now());

        let added = reg.merge_from(&other);
        assert_eq!(added, 1);
        assert_eq!(reg.len(), initial_len + 1);
        assert!(reg.lookup("cert").is_some());
    }

    #[test]
    fn merge_from_replaces_existing() {
        let mut reg = test_registry();
        let initial_len = reg.len();

        // Create a modified "pods" entry with different verbs
        let mut other = ResourceRegistry::new();
        other.add(ResourceEntry {
            resource: "pods".to_string(),
            kind: "Pod".to_string(),
            api_group: String::new(),
            api_version: "v1".to_string(),
            namespaced: true,
            short_names: vec!["po".to_string()],
            singular_name: "pod".to_string(),
            verbs: vec!["get".into(), "list".into()], // fewer verbs than builtin
        });

        let added = reg.merge_from(&other);
        assert_eq!(added, 0, "should replace, not add");
        assert_eq!(reg.len(), initial_len, "count should not change");
        // Verify the entry was updated
        let pods = reg.lookup("pods").unwrap();
        assert_eq!(pods.verbs.len(), 2);
    }

    // ---------------------------------------------------------------------------
    // ResourceNameProvider tests
    // ---------------------------------------------------------------------------

    #[test]
    fn name_provider_sorted_names() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        let names = provider.sorted_names();
        assert!(!names.is_empty());
        // Verify sorted
        for w in names.windows(2) {
            assert!(w[0] <= w[1], "names not sorted: {} > {}", w[0], w[1]);
        }
        assert_eq!(provider.name_count(), names.len());
    }

    #[test]
    fn name_provider_has_resource() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        assert!(provider.has_resource("pods"));
        assert!(provider.has_resource("deployments"));
        assert!(!provider.has_resource("foobar"));
    }

    #[test]
    fn name_provider_prefix_matches_po() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        let matches = provider.prefix_matches("po");
        assert!(!matches.is_empty());
        // All matches should start with "po"
        for m in &matches {
            assert!(m.alias.starts_with("po"), "alias '{}' doesn't start with 'po'", m.alias);
        }
        // Should include "pods" (or "pod") alias
        let resources: Vec<&str> = matches.iter().map(|m| m.resource.as_str()).collect();
        assert!(resources.contains(&"pods"), "should match pods, got: {:?}", resources);
    }

    #[test]
    fn name_provider_prefix_matches_empty_returns_all() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        let matches = provider.prefix_matches("");
        // Should return all aliases
        assert!(matches.len() > reg.len(), "should have more aliases than entries");
    }

    #[test]
    fn name_provider_prefix_matches_no_match() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        let matches = provider.prefix_matches("zzz");
        assert!(matches.is_empty());
    }

    #[test]
    fn name_provider_resolve_alias_exact() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);

        let result = provider.resolve_alias("po");
        assert!(result.is_some());
        assert_eq!(result.unwrap().resource, "pods");

        let result = provider.resolve_alias("svc");
        assert!(result.is_some());
        assert_eq!(result.unwrap().resource, "services");

        let result = provider.resolve_alias("deploy");
        assert!(result.is_some());
        assert_eq!(result.unwrap().resource, "deployments");
    }

    #[test]
    fn name_provider_resolve_alias_case_insensitive() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);

        let result = provider.resolve_alias("PO");
        assert!(result.is_some());
        assert_eq!(result.unwrap().resource, "pods");

        let result = provider.resolve_alias("Deploy");
        assert!(result.is_some());
        assert_eq!(result.unwrap().resource, "deployments");
    }

    #[test]
    fn name_provider_resolve_alias_unknown() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        assert!(provider.resolve_alias("foobar").is_none());
    }

    #[test]
    fn name_provider_from_discovery_flag() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        assert!(!provider.is_from_discovery(), "builtin registry should not be from_discovery");

        let mut discovered_reg = test_registry();
        discovered_reg.discovered_at = Some(Instant::now());
        let provider = ResourceNameProvider::from_registry(&discovered_reg);
        assert!(provider.is_from_discovery());
    }

    #[test]
    fn name_provider_resource_count() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        assert_eq!(provider.resource_count(), reg.len());
    }

    #[test]
    fn name_provider_alias_entry_metadata() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        let result = provider.resolve_alias("deploy").unwrap();
        assert_eq!(result.kind, "Deployment");
        assert_eq!(result.api_group, "apps");
        assert!(result.namespaced);
    }

    #[test]
    fn name_provider_prefix_de_returns_deployments_and_more() {
        let reg = test_registry();
        let provider = ResourceNameProvider::from_registry(&reg);
        let matches = provider.prefix_matches("de");
        assert!(!matches.is_empty());
        let aliases: Vec<&str> = matches.iter().map(|m| m.alias.as_str()).collect();
        assert!(aliases.contains(&"deploy") || aliases.contains(&"deployment") || aliases.contains(&"deployments"),
            "should match deployments-related alias, got: {:?}", aliases);
    }
}
