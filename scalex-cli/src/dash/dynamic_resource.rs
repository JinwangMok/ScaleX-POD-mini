//! Dynamic resource view — fetches any Kubernetes resource type via kube-rs DynamicObject API.
//!
//! This module supports the k9s-style command mode where users type `:deployments`, `:crd`,
//! `:configmaps.v1`, etc. and the TUI dynamically fetches and renders the resource table
//! without hardcoded resource type knowledge.
//!
//! Key design decisions:
//! - Lazy fetch: only fetches when user navigates to a resource type (no eager discovery)
//! - Uses kube-rs `DynamicObject` + `ApiResource` for generic list/watch
//! - Column extraction is heuristic-based (name, namespace, age + resource-specific columns)
//! - Reuses existing kube-rs `Client` instances (bearer token, kubeconfig, SSH tunnel)

use anyhow::Result;
use chrono::Utc;
use kube::api::{ApiResource, DynamicObject, ListParams};
use kube::discovery::{self, Scope};
use kube::{Api, Client};
use std::time::Duration;

/// Per-API-call timeout (matches data.rs)
const API_CALL_TIMEOUT: Duration = Duration::from_millis(2000);

/// A resolved resource type from API discovery, ready to fetch.
#[derive(Debug, Clone)]
pub struct ResolvedResource {
    /// The kube-rs ApiResource descriptor for building dynamic APIs
    pub api_resource: ApiResource,
    /// Whether this resource is namespaced or cluster-scoped
    pub namespaced: bool,
    /// Human-readable display name (e.g., "Deployments", "CustomResourceDefinitions")
    pub display_name: String,
    /// Short alias used in command mode (e.g., "deploy", "crd")
    pub command_alias: String,
    /// Column definitions for table rendering
    pub columns: Vec<DynColumn>,
}

/// A column definition for dynamic resource table rendering.
#[derive(Debug, Clone)]
pub struct DynColumn {
    /// Column header text
    pub header: String,
    /// JSON path-like key to extract value from the DynamicObject
    pub extractor: ColumnExtractor,
    /// Suggested width constraint (percentage of table width)
    pub width_percent: u16,
}

/// How to extract a column value from a DynamicObject.
#[derive(Debug, Clone)]
pub enum ColumnExtractor {
    /// metadata.name
    Name,
    /// metadata.namespace
    Namespace,
    /// Computed age from metadata.creationTimestamp
    Age,
    /// Extract from .status.phase or similar status field
    StatusPhase,
    /// Extract a specific field from the JSON data at a dot-separated path
    JsonPath(String),
    /// spec.replicas / status.readyReplicas style "ready/desired"
    ReadyReplicas,
    /// Extract from labels
    Labels,
    /// spec.type (for services)
    SpecType,
    /// spec.clusterIP (for services)
    ClusterIP,
    /// status.conditions — ready condition
    ReadyCondition,
    /// Restarts count (for pods)
    Restarts,
    /// Container ready count (for pods)
    ContainerReady,
    /// Cluster name (for cross-cluster mode, from scalex.io/cluster label)
    ClusterLabel,
}

/// Holds the fetched data for a dynamic resource view.
#[derive(Debug, Clone)]
pub struct DynamicResourceData {
    /// The resolved resource definition
    pub resource: ResolvedResource,
    /// Raw fetched objects
    pub objects: Vec<DynamicObject>,
    /// Pre-extracted row data for rendering (each row = vec of column strings)
    pub rows: Vec<Vec<String>>,
    /// Whether data has been fetched at least once
    pub fetched: bool,
}

impl DynamicResourceData {
    pub fn new(resource: ResolvedResource) -> Self {
        Self {
            resource,
            objects: Vec::new(),
            rows: Vec::new(),
            fetched: false,
        }
    }

    /// Extract display rows from the raw objects.
    pub fn extract_rows(&mut self) {
        self.rows = self
            .objects
            .iter()
            .map(|obj| {
                self.resource
                    .columns
                    .iter()
                    .map(|col| extract_column_value(obj, &col.extractor))
                    .collect()
            })
            .collect();
        // Sort by name (first column is always name)
        self.rows.sort_by(|a, b| {
            let a_name = a.first().map(|s| s.as_str()).unwrap_or("");
            let b_name = b.first().map(|s| s.as_str()).unwrap_or("");
            a_name.cmp(b_name)
        });
    }

    /// Number of rows (for cursor clamping)
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Filter rows by search query, returning matching rows with original indices.
    pub fn filtered_rows<'a>(
        &'a self,
        search_lower: Option<&str>,
    ) -> Vec<(usize, &'a Vec<String>)> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                match search_lower {
                    None => true,
                    Some("") => true,
                    Some(needle) => {
                        // Search across all columns
                        row.iter().any(|val| {
                            val.as_bytes().windows(needle.len()).any(|window| {
                                window
                                    .iter()
                                    .zip(needle.as_bytes())
                                    .all(|(h, n)| h.to_ascii_lowercase() == *n)
                            })
                        })
                    }
                }
            })
            .collect()
    }

    /// Count of filtered rows (for UI display).
    pub fn filtered_count(&self, search_lower: Option<&str>) -> usize {
        match search_lower {
            None | Some("") => self.rows.len(),
            Some(needle) => self
                .rows
                .iter()
                .filter(|row| {
                    row.iter().any(|val| {
                        val.as_bytes().windows(needle.len()).any(|window| {
                            window
                                .iter()
                                .zip(needle.as_bytes())
                                .all(|(h, n)| h.to_ascii_lowercase() == *n)
                        })
                    })
                })
                .count(),
        }
    }
}

/// Extract a single column value from a DynamicObject.
fn extract_column_value(obj: &DynamicObject, extractor: &ColumnExtractor) -> String {
    match extractor {
        ColumnExtractor::Name => obj.metadata.name.clone().unwrap_or_default(),
        ColumnExtractor::Namespace => obj.metadata.namespace.clone().unwrap_or_default(),
        ColumnExtractor::Age => obj
            .metadata
            .creation_timestamp
            .as_ref()
            .map(|ts| format_age(Utc::now(), ts.0))
            .unwrap_or_else(|| "<unknown>".into()),
        ColumnExtractor::StatusPhase => obj
            .data
            .get("status")
            .and_then(|s| s.get("phase"))
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string(),
        ColumnExtractor::JsonPath(path) => extract_json_path(&obj.data, path),
        ColumnExtractor::ReadyReplicas => {
            let desired = obj
                .data
                .get("spec")
                .and_then(|s| s.get("replicas"))
                .and_then(|r| r.as_i64())
                .unwrap_or(0);
            let ready = obj
                .data
                .get("status")
                .and_then(|s| s.get("readyReplicas"))
                .and_then(|r| r.as_i64())
                .unwrap_or(0);
            format!("{}/{}", ready, desired)
        }
        ColumnExtractor::Labels => {
            obj.metadata
                .labels
                .as_ref()
                .map(|labels| {
                    labels
                        .iter()
                        .take(3) // Show first 3 labels max
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default()
        }
        ColumnExtractor::SpecType => obj
            .data
            .get("spec")
            .and_then(|s| s.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        ColumnExtractor::ClusterIP => obj
            .data
            .get("spec")
            .and_then(|s| s.get("clusterIP"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        ColumnExtractor::ReadyCondition => obj
            .data
            .get("status")
            .and_then(|s| s.get("conditions"))
            .and_then(|c| c.as_array())
            .and_then(|conditions| {
                conditions
                    .iter()
                    .find(|c| c.get("type").and_then(|t| t.as_str()) == Some("Ready"))
            })
            .and_then(|c| c.get("status"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        ColumnExtractor::Restarts => obj
            .data
            .get("status")
            .and_then(|s| s.get("containerStatuses"))
            .and_then(|c| c.as_array())
            .map(|statuses| {
                let total: i64 = statuses
                    .iter()
                    .filter_map(|s| s.get("restartCount").and_then(|r| r.as_i64()))
                    .sum();
                total.to_string()
            })
            .unwrap_or_else(|| "0".into()),
        ColumnExtractor::ContainerReady => obj
            .data
            .get("status")
            .and_then(|s| s.get("containerStatuses"))
            .and_then(|c| c.as_array())
            .map(|statuses| {
                let ready = statuses
                    .iter()
                    .filter(|s| s.get("ready").and_then(|r| r.as_bool()).unwrap_or(false))
                    .count();
                format!("{}/{}", ready, statuses.len())
            })
            .unwrap_or_else(|| "0/0".into()),
        ColumnExtractor::ClusterLabel => obj
            .metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get("scalex.io/cluster"))
            .cloned()
            .unwrap_or_default(),
    }
}

/// Extract a value from nested JSON using a dot-separated path.
fn extract_json_path(data: &serde_json::Value, path: &str) -> String {
    let mut current = data;
    for key in path.split('.') {
        match current.get(key) {
            Some(val) => current = val,
            None => return String::new(),
        }
    }
    match current {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Format age (mirrors data.rs format_age)
fn format_age(now: chrono::DateTime<Utc>, created: chrono::DateTime<Utc>) -> String {
    let duration = now.signed_duration_since(created);
    let secs = duration.num_seconds();
    if secs < 0 {
        return "0s".to_string();
    }
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        let days = secs / 86400;
        if days >= 365 {
            format!("{}y", days / 365)
        } else if days >= 7 {
            format!("{}w", days / 7)
        } else {
            format!("{}d", days)
        }
    }
}

// ---------------------------------------------------------------------------
// Resource resolution — maps user input to a GVR via API discovery
// ---------------------------------------------------------------------------

/// Built-in resource aliases (k9s-style shortcuts).
/// Returns (group, version, plural, kind, namespaced) for known aliases.
/// This avoids an API discovery call for the most common resource types.
fn builtin_alias(
    input: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str, bool)> {
    match input {
        // Core resources
        "po" | "pod" | "pods" => Some(("", "v1", "pods", "Pod", true)),
        "svc" | "service" | "services" => Some(("", "v1", "services", "Service", true)),
        "no" | "node" | "nodes" => Some(("", "v1", "nodes", "Node", false)),
        "ns" | "namespace" | "namespaces" => Some(("", "v1", "namespaces", "Namespace", false)),
        "cm" | "configmap" | "configmaps" => Some(("", "v1", "configmaps", "ConfigMap", true)),
        "secret" | "secrets" => Some(("", "v1", "secrets", "Secret", true)),
        "sa" | "serviceaccount" | "serviceaccounts" => {
            Some(("", "v1", "serviceaccounts", "ServiceAccount", true))
        }
        "ev" | "event" | "events" => Some(("", "v1", "events", "Event", true)),
        "pv" | "persistentvolume" | "persistentvolumes" => {
            Some(("", "v1", "persistentvolumes", "PersistentVolume", false))
        }
        "pvc" | "persistentvolumeclaim" | "persistentvolumeclaims" => Some((
            "",
            "v1",
            "persistentvolumeclaims",
            "PersistentVolumeClaim",
            true,
        )),
        "ep" | "endpoint" | "endpoints" => Some(("", "v1", "endpoints", "Endpoints", true)),
        "rc" | "replicationcontroller" | "replicationcontrollers" => Some((
            "",
            "v1",
            "replicationcontrollers",
            "ReplicationController",
            true,
        )),
        // Apps group
        "dp" | "dep" | "deploy" | "deployment" | "deployments" => {
            Some(("apps", "v1", "deployments", "Deployment", true))
        }
        "ds" | "daemonset" | "daemonsets" => Some(("apps", "v1", "daemonsets", "DaemonSet", true)),
        "sts" | "statefulset" | "statefulsets" => {
            Some(("apps", "v1", "statefulsets", "StatefulSet", true))
        }
        "rs" | "replicaset" | "replicasets" => {
            Some(("apps", "v1", "replicasets", "ReplicaSet", true))
        }
        // Batch group
        "job" | "jobs" => Some(("batch", "v1", "jobs", "Job", true)),
        "cj" | "cronjob" | "cronjobs" => Some(("batch", "v1", "cronjobs", "CronJob", true)),
        // Networking
        "ing" | "ingress" | "ingresses" => {
            Some(("networking.k8s.io", "v1", "ingresses", "Ingress", true))
        }
        "netpol" | "networkpolicy" | "networkpolicies" => Some((
            "networking.k8s.io",
            "v1",
            "networkpolicies",
            "NetworkPolicy",
            true,
        )),
        // RBAC
        "clusterrole" | "clusterroles" | "cr" => Some((
            "rbac.authorization.k8s.io",
            "v1",
            "clusterroles",
            "ClusterRole",
            false,
        )),
        "clusterrolebinding" | "clusterrolebindings" | "crb" => Some((
            "rbac.authorization.k8s.io",
            "v1",
            "clusterrolebindings",
            "ClusterRoleBinding",
            false,
        )),
        "role" | "roles" => Some(("rbac.authorization.k8s.io", "v1", "roles", "Role", true)),
        "rolebinding" | "rolebindings" | "rb" => Some((
            "rbac.authorization.k8s.io",
            "v1",
            "rolebindings",
            "RoleBinding",
            true,
        )),
        // Storage
        "sc" | "storageclass" | "storageclasses" => Some((
            "storage.k8s.io",
            "v1",
            "storageclasses",
            "StorageClass",
            false,
        )),
        // API extensions
        "crd" | "customresourcedefinition" | "customresourcedefinitions" => Some((
            "apiextensions.k8s.io",
            "v1",
            "customresourcedefinitions",
            "CustomResourceDefinition",
            false,
        )),
        _ => None,
    }
}

/// Build column definitions based on resource kind.
pub fn columns_for_resource(kind: &str, namespaced: bool) -> Vec<DynColumn> {
    let mut cols = vec![DynColumn {
        header: "NAME".into(),
        extractor: ColumnExtractor::Name,
        width_percent: 30,
    }];

    if namespaced {
        cols.push(DynColumn {
            header: "NAMESPACE".into(),
            extractor: ColumnExtractor::Namespace,
            width_percent: 15,
        });
    }

    // Add resource-specific columns
    match kind {
        "Pod" => {
            cols.push(DynColumn {
                header: "READY".into(),
                extractor: ColumnExtractor::ContainerReady,
                width_percent: 8,
            });
            cols.push(DynColumn {
                header: "STATUS".into(),
                extractor: ColumnExtractor::StatusPhase,
                width_percent: 15,
            });
            cols.push(DynColumn {
                header: "RESTARTS".into(),
                extractor: ColumnExtractor::Restarts,
                width_percent: 8,
            });
        }
        "Deployment" | "StatefulSet" | "ReplicaSet" => {
            cols.push(DynColumn {
                header: "READY".into(),
                extractor: ColumnExtractor::ReadyReplicas,
                width_percent: 10,
            });
        }
        "Service" => {
            cols.push(DynColumn {
                header: "TYPE".into(),
                extractor: ColumnExtractor::SpecType,
                width_percent: 12,
            });
            cols.push(DynColumn {
                header: "CLUSTER-IP".into(),
                extractor: ColumnExtractor::ClusterIP,
                width_percent: 15,
            });
        }
        "Node" => {
            cols.push(DynColumn {
                header: "STATUS".into(),
                extractor: ColumnExtractor::ReadyCondition,
                width_percent: 10,
            });
        }
        "Namespace" => {
            cols.push(DynColumn {
                header: "STATUS".into(),
                extractor: ColumnExtractor::StatusPhase,
                width_percent: 10,
            });
        }
        "Job" | "CronJob" => {
            cols.push(DynColumn {
                header: "STATUS".into(),
                extractor: ColumnExtractor::JsonPath("status.active".into()),
                width_percent: 10,
            });
        }
        "Ingress" => {
            cols.push(DynColumn {
                header: "CLASS".into(),
                extractor: ColumnExtractor::JsonPath("spec.ingressClassName".into()),
                width_percent: 12,
            });
        }
        "DaemonSet" => {
            cols.push(DynColumn {
                header: "DESIRED".into(),
                extractor: ColumnExtractor::JsonPath("status.desiredNumberScheduled".into()),
                width_percent: 8,
            });
            cols.push(DynColumn {
                header: "READY".into(),
                extractor: ColumnExtractor::JsonPath("status.numberReady".into()),
                width_percent: 8,
            });
        }
        "ConfigMap" | "Secret" => {
            cols.push(DynColumn {
                header: "DATA".into(),
                extractor: ColumnExtractor::JsonPath("data".into()),
                width_percent: 10,
            });
        }
        "PersistentVolume" => {
            cols.push(DynColumn {
                header: "CAPACITY".into(),
                extractor: ColumnExtractor::JsonPath("spec.capacity.storage".into()),
                width_percent: 10,
            });
            cols.push(DynColumn {
                header: "STATUS".into(),
                extractor: ColumnExtractor::StatusPhase,
                width_percent: 10,
            });
        }
        "PersistentVolumeClaim" => {
            cols.push(DynColumn {
                header: "STATUS".into(),
                extractor: ColumnExtractor::StatusPhase,
                width_percent: 10,
            });
            cols.push(DynColumn {
                header: "VOLUME".into(),
                extractor: ColumnExtractor::JsonPath("spec.volumeName".into()),
                width_percent: 15,
            });
        }
        "StorageClass" => {
            cols.push(DynColumn {
                header: "PROVISIONER".into(),
                extractor: ColumnExtractor::JsonPath("provisioner".into()),
                width_percent: 20,
            });
        }
        "CustomResourceDefinition" => {
            cols.push(DynColumn {
                header: "GROUP".into(),
                extractor: ColumnExtractor::JsonPath("spec.group".into()),
                width_percent: 20,
            });
            cols.push(DynColumn {
                header: "SCOPE".into(),
                extractor: ColumnExtractor::JsonPath("spec.scope".into()),
                width_percent: 10,
            });
        }
        _ => {
            // Generic: just show labels as a catch-all extra column
            // (all resources get NAME + NAMESPACE + AGE at minimum)
        }
    }

    cols.push(DynColumn {
        header: "AGE".into(),
        extractor: ColumnExtractor::Age,
        width_percent: 8,
    });

    cols
}

/// Resolve a command input string to a ResolvedResource.
/// First tries builtin aliases, then falls back to kube-rs API discovery.
pub async fn resolve_resource(client: &Client, input: &str) -> Result<ResolvedResource> {
    let input_lower = input.to_ascii_lowercase();
    let input_lower = input_lower.trim();

    // Try builtin alias first (no API call needed)
    if let Some((group, version, plural, kind, namespaced)) = builtin_alias(input_lower) {
        let api_version = if group.is_empty() {
            version.to_string()
        } else {
            format!("{}/{}", group, version)
        };
        let ar = ApiResource {
            group: group.into(),
            version: version.into(),
            api_version,
            kind: kind.into(),
            plural: plural.into(),
        };
        let display_name = format!("{}s", kind); // Simple pluralization for display
        return Ok(ResolvedResource {
            columns: columns_for_resource(kind, namespaced),
            api_resource: ar,
            namespaced,
            display_name,
            command_alias: input_lower.to_string(),
        });
    }

    // Fall back to API discovery for CRDs and unknown resources
    resolve_via_discovery(client, input_lower).await
}

/// Resolve a resource type via kube-rs API discovery (for CRDs and unknown types).
async fn resolve_via_discovery(client: &Client, input: &str) -> Result<ResolvedResource> {
    // Use kube-rs discovery to find the resource
    let discovery = tokio::time::timeout(
        Duration::from_secs(5),
        discovery::Discovery::new(client.clone()).run(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("API discovery timeout"))??;

    // Search through all discovered API groups for a matching resource
    for group in discovery.groups() {
        for (ar, caps) in group.resources_by_stability() {
            // Match by plural name, kind (case-insensitive), or short names
            let plural_lower = ar.plural.to_ascii_lowercase();
            let kind_lower = ar.kind.to_ascii_lowercase();

            if plural_lower == input || kind_lower == input || kind_lower.starts_with(input) {
                let namespaced = caps.scope == Scope::Namespaced;
                let display_name = ar.kind.clone();
                return Ok(ResolvedResource {
                    columns: columns_for_resource(&ar.kind, namespaced),
                    api_resource: ar,
                    namespaced,
                    display_name,
                    command_alias: input.to_string(),
                });
            }
        }
    }

    anyhow::bail!("Unknown resource type: {}", input)
}

/// Fetch resources dynamically using a resolved resource type.
pub async fn fetch_dynamic_resources(
    client: &Client,
    resource: &ResolvedResource,
    namespace: Option<&str>,
) -> Result<Vec<DynamicObject>> {
    let api: Api<DynamicObject> = if resource.namespaced {
        match namespace {
            Some(ns) => Api::namespaced_with(client.clone(), ns, &resource.api_resource),
            None => Api::all_with(client.clone(), &resource.api_resource),
        }
    } else {
        Api::all_with(client.clone(), &resource.api_resource)
    };

    let list = tokio::time::timeout(API_CALL_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("{} list timeout", resource.display_name))??;

    Ok(list.items)
}

// ---------------------------------------------------------------------------
// YAML describe / fetch for selected resource
// ---------------------------------------------------------------------------

/// Timeout for single-object GET requests (describe/YAML fetch).
const DESCRIBE_TIMEOUT: Duration = Duration::from_millis(3000);

/// Fetch the full YAML representation of a single resource by name.
///
/// Returns the serialized YAML string. Uses kube-rs `Api::get()` to fetch the
/// object with all fields (spec, status, metadata) — equivalent to `kubectl get -o yaml`.
///
/// # Arguments
/// * `client` - kube-rs Client (reuses existing connection: bearer token, kubeconfig, or tunnel)
/// * `resource` - Resolved resource type (GVR + namespaced flag)
/// * `name` - Resource name (e.g., "nginx-deploy-abc123")
/// * `namespace` - Namespace for namespaced resources (ignored for cluster-scoped)
pub async fn fetch_resource_yaml(
    client: &Client,
    resource: &ResolvedResource,
    name: &str,
    namespace: Option<&str>,
) -> Result<String> {
    let api: Api<DynamicObject> = if resource.namespaced {
        match namespace {
            Some(ns) => Api::namespaced_with(client.clone(), ns, &resource.api_resource),
            None => {
                anyhow::bail!(
                    "Namespace required for namespaced resource '{}'",
                    resource.display_name
                )
            }
        }
    } else {
        Api::all_with(client.clone(), &resource.api_resource)
    };

    let obj = tokio::time::timeout(DESCRIBE_TIMEOUT, api.get(name))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout fetching {} '{}'", resource.display_name, name))??;

    serde_yaml::to_string(&obj).map_err(|e| anyhow::anyhow!("YAML serialization failed: {}", e))
}

/// Generate a kubectl-describe-style text summary of a resource.
///
/// Fetches the resource via kube-rs `Api::get()` and formats key fields into a
/// human-readable describe output. This is a pure kube-rs implementation —
/// no kubectl dependency required.
///
/// The output format mimics `kubectl describe` with hierarchical sections:
/// - Metadata (name, namespace, labels, annotations, creation timestamp)
/// - Spec (resource-specific fields rendered as indented YAML)
/// - Status (resource-specific fields rendered as indented YAML)
///
/// # Arguments
/// * `client` - kube-rs Client
/// * `resource` - Resolved resource type
/// * `name` - Resource name
/// * `namespace` - Namespace for namespaced resources
pub async fn describe_resource(
    client: &Client,
    resource: &ResolvedResource,
    name: &str,
    namespace: Option<&str>,
) -> Result<String> {
    let api: Api<DynamicObject> = if resource.namespaced {
        match namespace {
            Some(ns) => Api::namespaced_with(client.clone(), ns, &resource.api_resource),
            None => {
                anyhow::bail!(
                    "Namespace required for namespaced resource '{}'",
                    resource.display_name
                )
            }
        }
    } else {
        Api::all_with(client.clone(), &resource.api_resource)
    };

    let obj = tokio::time::timeout(DESCRIBE_TIMEOUT, api.get(name))
        .await
        .map_err(|_| {
            anyhow::anyhow!("Timeout describing {} '{}'", resource.display_name, name)
        })??;

    Ok(format_describe(&obj, &resource.api_resource.kind))
}

/// Describe a resource by kind/name using well-known API mappings.
/// Returns formatted describe output. Used by the YAML modal for static resource views.
pub async fn describe_resource_yaml(
    client: &Client,
    kind: &str,
    name: &str,
    namespace: Option<&str>,
) -> Result<String> {
    let (group, version, plural, namespaced) = match kind {
        "Pod" => ("", "v1", "pods", true),
        "Deployment" => ("apps", "v1", "deployments", true),
        "Service" => ("", "v1", "services", true),
        "ConfigMap" => ("", "v1", "configmaps", true),
        "Node" => ("", "v1", "nodes", false),
        "Event" => ("", "v1", "events", true),
        "Namespace" => ("", "v1", "namespaces", false),
        "StatefulSet" => ("apps", "v1", "statefulsets", true),
        "DaemonSet" => ("apps", "v1", "daemonsets", true),
        "ReplicaSet" => ("apps", "v1", "replicasets", true),
        "Job" => ("batch", "v1", "jobs", true),
        "CronJob" => ("batch", "v1", "cronjobs", true),
        "Ingress" => ("networking.k8s.io", "v1", "ingresses", true),
        _ => return Err(anyhow::anyhow!("Unknown resource kind: {}", kind)),
    };
    let api_version = if group.is_empty() {
        version.to_string()
    } else {
        format!("{}/{}", group, version)
    };
    let ar = kube::api::ApiResource {
        group: group.into(),
        version: version.into(),
        kind: kind.into(),
        api_version,
        plural: plural.into(),
    };
    let api: Api<DynamicObject> = if namespaced {
        match namespace {
            Some(ns) => Api::namespaced_with(client.clone(), ns, &ar),
            None => Api::default_namespaced_with(client.clone(), &ar),
        }
    } else {
        Api::all_with(client.clone(), &ar)
    };
    let obj = tokio::time::timeout(DESCRIBE_TIMEOUT, api.get(name))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout describing {} '{}'", kind, name))??;
    Ok(format_describe(&obj, kind))
}

/// Format a DynamicObject into a kubectl-describe-style text output.
///
/// Produces a hierarchical human-readable summary with sections for metadata,
/// spec, and status. This is used by the TUI describe overlay.
fn format_describe(obj: &DynamicObject, kind: &str) -> String {
    let mut out = String::with_capacity(4096);

    // Header
    let name = obj.metadata.name.as_deref().unwrap_or("<unknown>");
    let ns = obj.metadata.namespace.as_deref().unwrap_or("");

    out.push_str(&format!("Name:         {}\n", name));
    if !ns.is_empty() {
        out.push_str(&format!("Namespace:    {}\n", ns));
    }
    out.push_str(&format!("Kind:         {}\n", kind));

    // Creation timestamp
    if let Some(ts) = &obj.metadata.creation_timestamp {
        out.push_str(&format!(
            "Created:      {}\n",
            ts.0.format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }

    // UID
    if let Some(uid) = &obj.metadata.uid {
        out.push_str(&format!("UID:          {}\n", uid));
    }

    // Resource version
    if let Some(rv) = &obj.metadata.resource_version {
        out.push_str(&format!("Version:      {}\n", rv));
    }

    // Labels
    if let Some(labels) = &obj.metadata.labels {
        if !labels.is_empty() {
            out.push_str("Labels:\n");
            let mut sorted_labels: Vec<_> = labels.iter().collect();
            sorted_labels.sort_by_key(|(k, _)| *k);
            for (k, v) in &sorted_labels {
                out.push_str(&format!("  {}={}\n", k, v));
            }
        } else {
            out.push_str("Labels:       <none>\n");
        }
    } else {
        out.push_str("Labels:       <none>\n");
    }

    // Annotations
    if let Some(annotations) = &obj.metadata.annotations {
        if !annotations.is_empty() {
            out.push_str("Annotations:\n");
            let mut sorted_annos: Vec<_> = annotations.iter().collect();
            sorted_annos.sort_by_key(|(k, _)| *k);
            for (k, v) in &sorted_annos {
                // Truncate long annotation values for readability
                let display_v = if v.len() > 120 {
                    format!("{}...", &v[..117])
                } else {
                    v.to_string()
                };
                out.push_str(&format!("  {}={}\n", k, display_v));
            }
        } else {
            out.push_str("Annotations:  <none>\n");
        }
    } else {
        out.push_str("Annotations:  <none>\n");
    }

    // Owner references
    if let Some(owners) = &obj.metadata.owner_references {
        if !owners.is_empty() {
            out.push_str("Controlled By:\n");
            for owner in owners {
                out.push_str(&format!("  {} / {}\n", owner.kind, owner.name));
            }
        }
    }

    // Finalizers
    if let Some(finalizers) = &obj.metadata.finalizers {
        if !finalizers.is_empty() {
            out.push_str("Finalizers:\n");
            for f in finalizers {
                out.push_str(&format!("  {}\n", f));
            }
        }
    }

    out.push('\n');

    // Spec section
    if let Some(spec) = obj.data.get("spec") {
        out.push_str("Spec:\n");
        format_json_section(&mut out, spec, 2);
        out.push('\n');
    }

    // Status section
    if let Some(status) = obj.data.get("status") {
        out.push_str("Status:\n");
        format_json_section(&mut out, status, 2);
        out.push('\n');
    }

    // Data section (for ConfigMaps/Secrets)
    if let Some(data) = obj.data.get("data") {
        out.push_str("Data:\n");
        if let Some(map) = data.as_object() {
            if map.is_empty() {
                out.push_str("  <empty>\n");
            } else {
                for (k, v) in map {
                    out.push_str(&format!("  {}:\n", k));
                    match v.as_str() {
                        Some(s) => {
                            // Indent multi-line values
                            for line in s.lines() {
                                out.push_str(&format!("    {}\n", line));
                            }
                        }
                        None => {
                            out.push_str(&format!("    {}\n", v));
                        }
                    }
                }
            }
        } else {
            format_json_section(&mut out, data, 2);
        }
        out.push('\n');
    }

    // Events would require a separate API call — note this for the user
    out.push_str("Events:       <use :events to view cluster events>\n");

    out
}

/// Format a JSON value as indented key-value pairs for the describe output.
/// Handles nested objects with increasing indentation.
fn format_json_section(out: &mut String, value: &serde_json::Value, indent: usize) {
    let prefix = " ".repeat(indent);
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                match v {
                    serde_json::Value::Object(_) => {
                        out.push_str(&format!("{}{}:\n", prefix, k));
                        format_json_section(out, v, indent + 2);
                    }
                    serde_json::Value::Array(arr) => {
                        out.push_str(&format!("{}{}:\n", prefix, k));
                        for (i, item) in arr.iter().enumerate() {
                            if item.is_object() {
                                out.push_str(&format!("{}- [{}]\n", prefix, i));
                                format_json_section(out, item, indent + 4);
                            } else {
                                out.push_str(&format!("{}- {}\n", prefix, format_scalar(item)));
                            }
                        }
                    }
                    _ => {
                        out.push_str(&format!("{}{}:  {}\n", prefix, k, format_scalar(v)));
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                if item.is_object() {
                    out.push_str(&format!("{}- [{}]\n", prefix, i));
                    format_json_section(out, item, indent + 4);
                } else {
                    out.push_str(&format!("{}- {}\n", prefix, format_scalar(item)));
                }
            }
        }
        _ => {
            out.push_str(&format!("{}{}\n", prefix, format_scalar(value)));
        }
    }
}

/// Format a scalar JSON value for display.
fn format_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "<none>".to_string(),
        other => other.to_string(),
    }
}

/// Fetch YAML for a resource directly from the DynamicResourceData's cached objects.
///
/// This avoids an extra API call when the object is already in memory from the
/// list/watch data. Falls back to `fetch_resource_yaml` if not found in cache.
///
/// Returns the YAML string if found in cache, or None if an API fetch is needed.
pub fn yaml_from_cache(
    data: &DynamicResourceData,
    name: &str,
    namespace: Option<&str>,
) -> Option<String> {
    let obj = data.objects.iter().find(|o| {
        let name_match = o.metadata.name.as_deref() == Some(name);
        let ns_match = match namespace {
            Some(ns) => o.metadata.namespace.as_deref() == Some(ns),
            None => true, // cluster-scoped or "all namespaces" — match by name only
        };
        name_match && ns_match
    })?;

    serde_yaml::to_string(obj).ok()
}

/// Generate describe output from cached DynamicResourceData objects.
///
/// Returns None if the resource is not found in cache (caller should fall back to API fetch).
pub fn describe_from_cache(
    data: &DynamicResourceData,
    name: &str,
    namespace: Option<&str>,
) -> Option<String> {
    let obj = data.objects.iter().find(|o| {
        let name_match = o.metadata.name.as_deref() == Some(name);
        let ns_match = match namespace {
            Some(ns) => o.metadata.namespace.as_deref() == Some(ns),
            None => true,
        };
        name_match && ns_match
    })?;

    Some(format_describe(obj, &data.resource.api_resource.kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_alias_pods() {
        assert!(builtin_alias("po").is_some());
        assert!(builtin_alias("pod").is_some());
        assert!(builtin_alias("pods").is_some());
        let (group, version, plural, kind, namespaced) = builtin_alias("pods").unwrap();
        assert_eq!(group, "");
        assert_eq!(version, "v1");
        assert_eq!(plural, "pods");
        assert_eq!(kind, "Pod");
        assert!(namespaced);
    }

    #[test]
    fn builtin_alias_deployments() {
        for alias in &["dp", "dep", "deploy", "deployment", "deployments"] {
            let result = builtin_alias(alias);
            assert!(result.is_some(), "alias '{}' should resolve", alias);
            let (group, _, plural, kind, namespaced) = result.unwrap();
            assert_eq!(group, "apps");
            assert_eq!(plural, "deployments");
            assert_eq!(kind, "Deployment");
            assert!(namespaced);
        }
    }

    #[test]
    fn builtin_alias_cluster_scoped() {
        let (_, _, _, _, namespaced) = builtin_alias("nodes").unwrap();
        assert!(!namespaced);

        let (_, _, _, _, namespaced) = builtin_alias("crd").unwrap();
        assert!(!namespaced);
    }

    #[test]
    fn builtin_alias_unknown_returns_none() {
        assert!(builtin_alias("foobar").is_none());
        assert!(builtin_alias("xyz123").is_none());
    }

    #[test]
    fn columns_for_pods_have_expected_headers() {
        let cols = columns_for_resource("Pod", true);
        let headers: Vec<&str> = cols.iter().map(|c| c.header.as_str()).collect();
        assert!(headers.contains(&"NAME"));
        assert!(headers.contains(&"NAMESPACE"));
        assert!(headers.contains(&"STATUS"));
        assert!(headers.contains(&"READY"));
        assert!(headers.contains(&"RESTARTS"));
        assert!(headers.contains(&"AGE"));
    }

    #[test]
    fn columns_for_cluster_scoped_no_namespace() {
        let cols = columns_for_resource("Node", false);
        let headers: Vec<&str> = cols.iter().map(|c| c.header.as_str()).collect();
        assert!(headers.contains(&"NAME"));
        assert!(!headers.contains(&"NAMESPACE"));
        assert!(headers.contains(&"AGE"));
    }

    #[test]
    fn extract_json_path_nested() {
        let data = serde_json::json!({
            "spec": {
                "replicas": 3,
                "type": "ClusterIP"
            },
            "status": {
                "phase": "Running"
            }
        });
        assert_eq!(extract_json_path(&data, "spec.replicas"), "3");
        assert_eq!(extract_json_path(&data, "spec.type"), "ClusterIP");
        assert_eq!(extract_json_path(&data, "status.phase"), "Running");
        assert_eq!(extract_json_path(&data, "nonexistent.path"), "");
    }

    #[test]
    fn format_age_values() {
        let now = Utc::now();
        assert_eq!(format_age(now, now - chrono::Duration::seconds(30)), "30s");
        assert_eq!(format_age(now, now - chrono::Duration::minutes(5)), "5m");
        assert_eq!(format_age(now, now - chrono::Duration::hours(2)), "2h");
        assert_eq!(format_age(now, now - chrono::Duration::days(3)), "3d");
    }

    #[test]
    fn dynamic_resource_data_filtered_count() {
        let resource = ResolvedResource {
            api_resource: ApiResource {
                group: "".into(),
                version: "v1".into(),
                api_version: "v1".into(),
                kind: "ConfigMap".into(),
                plural: "configmaps".into(),
            },
            namespaced: true,
            display_name: "ConfigMaps".into(),
            command_alias: "cm".into(),
            columns: columns_for_resource("ConfigMap", true),
        };
        let mut data = DynamicResourceData::new(resource);
        data.rows = vec![
            vec![
                "kube-root-ca.crt".into(),
                "default".into(),
                "3".into(),
                "1d".into(),
            ],
            vec![
                "my-config".into(),
                "default".into(),
                "5".into(),
                "2h".into(),
            ],
            vec![
                "other-config".into(),
                "kube-system".into(),
                "1".into(),
                "3d".into(),
            ],
        ];

        assert_eq!(data.filtered_count(None), 3);
        assert_eq!(data.filtered_count(Some("my")), 1);
        assert_eq!(data.filtered_count(Some("config")), 2);
        assert_eq!(data.filtered_count(Some("nonexistent")), 0);
    }

    // --- Tests for YAML describe / fetch functions ---

    fn make_test_obj(name: &str, ns: Option<&str>) -> DynamicObject {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        DynamicObject {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: ns.map(|s| s.to_string()),
                labels: Some(std::collections::BTreeMap::from([
                    ("app".to_string(), "test".to_string()),
                    ("env".to_string(), "dev".to_string()),
                ])),
                annotations: Some(std::collections::BTreeMap::from([(
                    "note".to_string(),
                    "test annotation".to_string(),
                )])),
                uid: Some("abc-123".to_string()),
                resource_version: Some("42".to_string()),
                ..Default::default()
            },
            types: None,
            data: serde_json::json!({
                "spec": {
                    "replicas": 3,
                    "selector": {
                        "matchLabels": { "app": "test" }
                    }
                },
                "status": {
                    "readyReplicas": 2,
                    "availableReplicas": 2
                }
            }),
        }
    }

    fn make_test_resource_data() -> DynamicResourceData {
        let resource = ResolvedResource {
            api_resource: ApiResource {
                group: "apps".into(),
                version: "v1".into(),
                api_version: "apps/v1".into(),
                kind: "Deployment".into(),
                plural: "deployments".into(),
            },
            namespaced: true,
            display_name: "Deployments".into(),
            command_alias: "deploy".into(),
            columns: columns_for_resource("Deployment", true),
        };
        let mut data = DynamicResourceData::new(resource);
        data.objects = vec![
            make_test_obj("nginx", Some("default")),
            make_test_obj("redis", Some("kube-system")),
        ];
        data
    }

    #[test]
    fn format_describe_includes_metadata() {
        let obj = make_test_obj("my-deploy", Some("production"));
        let output = format_describe(&obj, "Deployment");

        assert!(
            output.contains("Name:         my-deploy"),
            "should contain name"
        );
        assert!(
            output.contains("Namespace:    production"),
            "should contain namespace"
        );
        assert!(
            output.contains("Kind:         Deployment"),
            "should contain kind"
        );
        assert!(
            output.contains("UID:          abc-123"),
            "should contain UID"
        );
        assert!(
            output.contains("Version:      42"),
            "should contain resource version"
        );
    }

    #[test]
    fn format_describe_includes_labels() {
        let obj = make_test_obj("my-deploy", Some("default"));
        let output = format_describe(&obj, "Deployment");

        assert!(output.contains("Labels:"), "should have labels section");
        assert!(output.contains("app=test"), "should contain app label");
        assert!(output.contains("env=dev"), "should contain env label");
    }

    #[test]
    fn format_describe_includes_annotations() {
        let obj = make_test_obj("my-deploy", Some("default"));
        let output = format_describe(&obj, "Deployment");

        assert!(
            output.contains("Annotations:"),
            "should have annotations section"
        );
        assert!(
            output.contains("note=test annotation"),
            "should contain annotation"
        );
    }

    #[test]
    fn format_describe_includes_spec_and_status() {
        let obj = make_test_obj("my-deploy", Some("default"));
        let output = format_describe(&obj, "Deployment");

        assert!(output.contains("Spec:"), "should have spec section");
        assert!(output.contains("replicas"), "should contain replicas field");
        assert!(output.contains("Status:"), "should have status section");
        assert!(
            output.contains("readyReplicas"),
            "should contain readyReplicas"
        );
    }

    #[test]
    fn format_describe_no_namespace_for_cluster_scoped() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        let obj = DynamicObject {
            metadata: ObjectMeta {
                name: Some("my-node".into()),
                ..Default::default()
            },
            types: None,
            data: serde_json::json!({}),
        };
        let output = format_describe(&obj, "Node");
        assert!(output.contains("Name:         my-node"));
        assert!(
            !output.contains("Namespace:"),
            "cluster-scoped should not show namespace"
        );
    }

    #[test]
    fn yaml_from_cache_finds_by_name_and_namespace() {
        let data = make_test_resource_data();

        let yaml = yaml_from_cache(&data, "nginx", Some("default"));
        assert!(yaml.is_some(), "should find nginx in default");
        let yaml_str = yaml.unwrap();
        assert!(
            yaml_str.contains("nginx"),
            "YAML should contain resource name"
        );

        let yaml = yaml_from_cache(&data, "redis", Some("kube-system"));
        assert!(yaml.is_some(), "should find redis in kube-system");

        let yaml = yaml_from_cache(&data, "nonexistent", Some("default"));
        assert!(yaml.is_none(), "should not find nonexistent");

        let yaml = yaml_from_cache(&data, "nginx", Some("wrong-ns"));
        assert!(yaml.is_none(), "should not find nginx in wrong namespace");
    }

    #[test]
    fn yaml_from_cache_cluster_scoped_matches_by_name_only() {
        let data = make_test_resource_data();

        // With namespace=None, should match by name only
        let yaml = yaml_from_cache(&data, "nginx", None);
        assert!(yaml.is_some(), "cluster-scoped lookup should match by name");
    }

    #[test]
    fn describe_from_cache_returns_formatted_output() {
        let data = make_test_resource_data();

        let desc = describe_from_cache(&data, "nginx", Some("default"));
        assert!(desc.is_some());
        let text = desc.unwrap();
        assert!(text.contains("Name:         nginx"));
        assert!(text.contains("Kind:         Deployment"));
        assert!(text.contains("Namespace:    default"));
    }

    #[test]
    fn describe_from_cache_returns_none_for_missing() {
        let data = make_test_resource_data();
        assert!(describe_from_cache(&data, "missing", Some("default")).is_none());
    }

    #[test]
    fn format_json_section_handles_nested_objects() {
        let data = serde_json::json!({
            "containers": [
                {
                    "name": "nginx",
                    "image": "nginx:latest"
                }
            ],
            "replicas": 3
        });
        let mut out = String::new();
        format_json_section(&mut out, &data, 2);
        assert!(out.contains("containers:"));
        assert!(out.contains("nginx"));
        assert!(out.contains("replicas:  3"));
    }

    #[test]
    fn format_scalar_values() {
        assert_eq!(format_scalar(&serde_json::json!("hello")), "hello");
        assert_eq!(format_scalar(&serde_json::json!(42)), "42");
        assert_eq!(format_scalar(&serde_json::json!(true)), "true");
        assert_eq!(format_scalar(&serde_json::json!(null)), "<none>");
    }

    #[test]
    fn format_describe_truncates_long_annotations() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        let long_value = "x".repeat(200);
        let obj = DynamicObject {
            metadata: ObjectMeta {
                name: Some("test".into()),
                annotations: Some(std::collections::BTreeMap::from([(
                    "long-annotation".to_string(),
                    long_value,
                )])),
                ..Default::default()
            },
            types: None,
            data: serde_json::json!({}),
        };
        let output = format_describe(&obj, "Pod");
        // Should be truncated with "..."
        assert!(
            output.contains("..."),
            "long annotations should be truncated"
        );
        // Should not contain the full 200 chars
        assert!(!output.contains(&"x".repeat(200)));
    }

    #[test]
    fn format_describe_configmap_data_section() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        let obj = DynamicObject {
            metadata: ObjectMeta {
                name: Some("my-config".into()),
                ..Default::default()
            },
            types: None,
            data: serde_json::json!({
                "data": {
                    "config.yaml": "key: value\nother: stuff",
                    "simple": "one-liner"
                }
            }),
        };
        let output = format_describe(&obj, "ConfigMap");
        assert!(
            output.contains("Data:"),
            "should have Data section for ConfigMaps"
        );
        assert!(output.contains("config.yaml:"), "should show data key");
        assert!(
            output.contains("key: value"),
            "should show multi-line data content"
        );
    }
}

// --- Test helper for other modules ---

#[cfg(test)]
impl DynamicResourceData {
    /// Create a DynamicResourceData with test data in "fetched" state.
    /// Used by app.rs tests to verify cluster-change refresh clears stale data.
    pub fn new_test_fetched() -> Self {
        use kube::api::ApiResource;
        let resource = ResolvedResource {
            api_resource: ApiResource {
                group: "".into(),
                version: "v1".into(),
                api_version: "v1".into(),
                kind: "Pod".into(),
                plural: "pods".into(),
            },
            namespaced: true,
            display_name: "Pods".into(),
            command_alias: "po".into(),
            columns: columns_for_resource("Pod", true),
        };
        Self {
            resource,
            objects: Vec::new(),
            rows: vec![
                vec![
                    "nginx".into(),
                    "default".into(),
                    "Running".into(),
                    "1/1".into(),
                    "0".into(),
                    "1d".into(),
                ],
                vec![
                    "redis".into(),
                    "default".into(),
                    "Running".into(),
                    "1/1".into(),
                    "0".into(),
                    "2h".into(),
                ],
            ],
            fetched: true,
        }
    }
}
