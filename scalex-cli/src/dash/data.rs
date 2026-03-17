use anyhow::Result;
use chrono::Utc;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Event, Namespace, Node, Pod, Service};
use kube::api::ListParams;
use kube::{Api, Client};
use serde::Serialize;
use std::time::Duration;

/// Which resource type the TUI is currently displaying.
/// Used to selectively fetch only what's needed (reduces 7 API calls → 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActiveResource {
    Pods,
    Deployments,
    Services,
    ConfigMaps,
    Nodes,
    Events,
}

/// Per-API-call timeout to prevent slow calls from blocking the entire fetch.
/// Reduced from 1s to 500ms — K8s API calls on a healthy cluster complete in <200ms;
/// 500ms is generous while halving worst-case fetch latency.
const API_CALL_TIMEOUT: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ClusterSnapshot {
    pub name: String,
    pub health: HealthStatus,
    pub namespaces: Vec<String>,
    pub nodes: Vec<NodeInfo>,
    pub pods: Vec<PodInfo>,
    pub deployments: Vec<DeploymentInfo>,
    pub services: Vec<ServiceInfo>,
    pub configmaps: Vec<ConfigMapInfo>,
    pub events: Vec<EventInfo>,
    pub resource_usage: ResourceUsage,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // Unknown variant used by UI display but not constructed in data layer
pub enum HealthStatus {
    Green,
    Yellow,
    Red,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct NodeInfo {
    pub name: String,
    pub status: String,
    pub roles: Vec<String>,
    pub cpu_capacity: String,
    pub mem_capacity: String,
    pub cpu_allocatable: String,
    pub mem_allocatable: String,
    pub age: String,
    /// Pre-computed display strings to avoid per-frame allocations in render path
    pub roles_display: String,
    pub mem_capacity_display: String,
    pub mem_allocatable_display: String,
    /// Pre-computed "allocatable/capacity" columns to avoid per-frame format!()
    pub cpu_display: String,
    pub mem_display: String,
    /// Kubelet version (e.g., "v1.33.1") from node.status.nodeInfo
    pub kubelet_version: String,
    /// Pre-computed display string for Top tab: "  v1.33.1  CPU: 8/8  MEM: 7.5Gi/7.8Gi"
    pub top_display: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PodInfo {
    pub name: String,
    pub namespace: String,
    pub status: String,
    pub ready: String,
    pub restarts: i32,
    /// Pre-computed restarts.to_string() to avoid per-frame allocation in render path
    pub restarts_display: String,
    pub age: String,
    pub node: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeploymentInfo {
    pub name: String,
    pub namespace: String,
    pub ready: String,
    /// Ready replica count for render-path color coding (avoids parsing `ready` string per frame)
    pub ready_count: i32,
    /// Desired replica count for render-path color coding
    pub desired_count: i32,
    pub up_to_date: i32,
    /// Pre-computed up_to_date.to_string() to avoid per-frame allocation
    pub up_to_date_display: String,
    pub available: i32,
    /// Pre-computed available.to_string() to avoid per-frame allocation
    pub available_display: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub name: String,
    pub namespace: String,
    pub svc_type: String,
    pub cluster_ip: String,
    /// External IP from LoadBalancer ingress, or "<none>" for non-LB services
    pub external_ip: String,
    pub ports: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigMapInfo {
    pub name: String,
    pub namespace: String,
    pub data_keys_count: usize,
    /// Pre-computed data_keys_count.to_string() to avoid per-frame allocation
    pub data_keys_display: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventInfo {
    pub namespace: String,
    pub name: String,
    /// Event type: "Normal" or "Warning"
    pub event_type: String,
    /// Reason string (e.g., "Pulled", "Scheduled", "FailedScheduling")
    pub reason: String,
    /// Involved object kind + name (e.g., "Pod/nginx-abc123")
    pub object: String,
    /// Human-readable event message
    pub message: String,
    /// Event count (how many times this event occurred)
    pub count: i32,
    /// Pre-computed count.to_string()
    pub count_display: String,
    /// Formatted age since last occurrence
    pub last_seen: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ResourceUsage {
    pub cpu_percent: f64,
    pub mem_percent: f64,
    pub total_pods: usize,
    pub running_pods: usize,
    pub succeeded_pods: usize,
    pub failed_pods: usize,
    pub total_nodes: usize,
    pub ready_nodes: usize,
}

// ---------------------------------------------------------------------------
// Fetch functions (async, pure-ish — only side effect is network I/O)
// ---------------------------------------------------------------------------

pub async fn fetch_namespaces(client: &Client) -> Result<Vec<String>> {
    let api: Api<Namespace> = Api::all(client.clone());
    let ns_list = tokio::time::timeout(API_CALL_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("namespace list timeout"))??;
    let mut names: Vec<String> = ns_list
        .items
        .iter()
        .filter_map(|ns| ns.metadata.name.clone())
        .collect();
    names.sort();
    Ok(names)
}

pub async fn fetch_pods(client: &Client, namespace: Option<&str>) -> Result<Vec<PodInfo>> {
    let api: Api<Pod> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let pod_list = tokio::time::timeout(API_CALL_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("pod list timeout"))??;
    let now = Utc::now();

    Ok(pod_list
        .items
        .iter()
        .map(|pod| {
            let meta = &pod.metadata;
            let status = pod.status.as_ref();
            let spec = pod.spec.as_ref();

            let phase = status
                .and_then(|s| s.phase.clone())
                .unwrap_or_else(|| "Unknown".into());

            let init_container_statuses = status
                .map(|s| s.init_container_statuses.clone().unwrap_or_default())
                .unwrap_or_default();
            let container_statuses = status
                .map(|s| s.container_statuses.clone().unwrap_or_default())
                .unwrap_or_default();

            // Derive effective status: check container waiting reasons
            // (e.g., CrashLoopBackOff shows phase=Running but container is waiting)
            let has_deletion_timestamp = meta.deletion_timestamp.is_some();
            let effective_status = if has_deletion_timestamp {
                "Terminating".to_string()
            } else {
                derive_effective_status(
                    &phase,
                    &container_statuses,
                    &init_container_statuses,
                    status.and_then(|s| s.reason.as_deref()),
                )
            };

            let ready_count = container_statuses.iter().filter(|c| c.ready).count();
            let total_count = container_statuses.len();
            let restarts: i32 = container_statuses.iter().map(|c| c.restart_count).sum();

            let age = meta
                .creation_timestamp
                .as_ref()
                .map(|ts| format_age(now, ts.0))
                .unwrap_or_else(|| "<unknown>".into());

            PodInfo {
                name: meta.name.clone().unwrap_or_default(),
                namespace: meta.namespace.clone().unwrap_or_default(),
                status: effective_status,
                ready: format!("{}/{}", ready_count, total_count),
                restarts_display: restarts.to_string(),
                restarts,
                age,
                node: spec.and_then(|s| s.node_name.clone()).unwrap_or_default(),
            }
        })
        .collect())
}

pub async fn fetch_nodes(client: &Client) -> Result<Vec<NodeInfo>> {
    let api: Api<Node> = Api::all(client.clone());
    let node_list = tokio::time::timeout(API_CALL_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("node list timeout"))??;
    let now = Utc::now();

    Ok(node_list
        .items
        .iter()
        .map(|node| {
            let meta = &node.metadata;
            let status = node.status.as_ref();
            let spec = node.spec.as_ref();

            let conditions = status
                .map(|s| s.conditions.clone().unwrap_or_default())
                .unwrap_or_default();
            let ready_str = conditions
                .iter()
                .find(|c| c.type_ == "Ready")
                .map(|c| {
                    if c.status == "True" {
                        "Ready"
                    } else {
                        "NotReady"
                    }
                })
                .unwrap_or("Unknown");
            // Append SchedulingDisabled for cordoned nodes (spec.unschedulable=true)
            let node_status = if spec.and_then(|s| s.unschedulable).unwrap_or(false) {
                format!("{},SchedulingDisabled", ready_str)
            } else {
                ready_str.to_string()
            };

            let labels = meta.labels.as_ref();
            let roles: Vec<String> = labels
                .map(|l| {
                    l.keys()
                        .filter(|k| k.starts_with("node-role.kubernetes.io/"))
                        .map(|k| k.trim_start_matches("node-role.kubernetes.io/").to_string())
                        .collect()
                })
                .unwrap_or_default();

            let capacity = status.and_then(|s| s.capacity.as_ref());
            let allocatable = status.and_then(|s| s.allocatable.as_ref());

            let age = meta
                .creation_timestamp
                .as_ref()
                .map(|ts| format_age(now, ts.0))
                .unwrap_or_else(|| "<unknown>".into());

            let cpu_cap = capacity
                .and_then(|c| c.get("cpu"))
                .map(|v| v.0.clone())
                .unwrap_or_default();
            let mem_cap = capacity
                .and_then(|c| c.get("memory"))
                .map(|v| v.0.clone())
                .unwrap_or_default();
            let cpu_alloc = allocatable
                .and_then(|a| a.get("cpu"))
                .map(|v| v.0.clone())
                .unwrap_or_default();
            let mem_alloc = allocatable
                .and_then(|a| a.get("memory"))
                .map(|v| v.0.clone())
                .unwrap_or_default();

            let roles_display = if roles.is_empty() {
                "<none>".to_string()
            } else {
                roles.join(",")
            };
            let mem_capacity_display = format_k8s_memory(&mem_cap);
            let mem_allocatable_display = format_k8s_memory(&mem_alloc);

            let cpu_display = format!("{}/{}", cpu_alloc, cpu_cap);
            let mem_display = format!(
                "{}/{}",
                mem_allocatable_display, mem_capacity_display
            );

            let kubelet_version = status
                .and_then(|s| s.node_info.as_ref())
                .map(|ni| ni.kubelet_version.clone())
                .unwrap_or_default();

            let top_display = format!(
                "  {}  CPU: {}  MEM: {}",
                kubelet_version, cpu_display, mem_display
            );

            NodeInfo {
                name: meta.name.clone().unwrap_or_default(),
                status: node_status,
                roles,
                cpu_capacity: cpu_cap,
                mem_capacity: mem_cap,
                cpu_allocatable: cpu_alloc,
                mem_allocatable: mem_alloc,
                age,
                roles_display,
                mem_capacity_display,
                mem_allocatable_display,
                cpu_display,
                mem_display,
                kubelet_version,
                top_display,
            }
        })
        .collect())
}

pub async fn fetch_deployments(
    client: &Client,
    namespace: Option<&str>,
) -> Result<Vec<DeploymentInfo>> {
    let api: Api<Deployment> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let dep_list = tokio::time::timeout(API_CALL_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("deployment list timeout"))??;
    let now = Utc::now();

    Ok(dep_list
        .items
        .iter()
        .map(|dep| {
            let meta = &dep.metadata;
            let status = dep.status.as_ref();

            let replicas = status.and_then(|s| s.replicas).unwrap_or(0);
            let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
            let up_to_date = status.and_then(|s| s.updated_replicas).unwrap_or(0);
            let available = status.and_then(|s| s.available_replicas).unwrap_or(0);

            let age = meta
                .creation_timestamp
                .as_ref()
                .map(|ts| format_age(now, ts.0))
                .unwrap_or_else(|| "<unknown>".into());

            DeploymentInfo {
                name: meta.name.clone().unwrap_or_default(),
                namespace: meta.namespace.clone().unwrap_or_default(),
                ready: format!("{}/{}", ready, replicas),
                ready_count: ready,
                desired_count: replicas,
                up_to_date_display: up_to_date.to_string(),
                up_to_date,
                available_display: available.to_string(),
                available,
                age,
            }
        })
        .collect())
}

pub async fn fetch_configmaps(
    client: &Client,
    namespace: Option<&str>,
) -> Result<Vec<ConfigMapInfo>> {
    let api: Api<ConfigMap> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let cm_list = tokio::time::timeout(API_CALL_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("configmap list timeout"))??;
    let now = Utc::now();

    Ok(cm_list
        .items
        .iter()
        .map(|cm| {
            let meta = &cm.metadata;
            let data_keys_count = cm.data.as_ref().map(|d| d.len()).unwrap_or(0)
                + cm.binary_data.as_ref().map(|d| d.len()).unwrap_or(0);

            let age = meta
                .creation_timestamp
                .as_ref()
                .map(|ts| format_age(now, ts.0))
                .unwrap_or_else(|| "<unknown>".into());

            ConfigMapInfo {
                name: meta.name.clone().unwrap_or_default(),
                namespace: meta.namespace.clone().unwrap_or_default(),
                data_keys_display: data_keys_count.to_string(),
                data_keys_count,
                age,
            }
        })
        .collect())
}

pub async fn fetch_services(client: &Client, namespace: Option<&str>) -> Result<Vec<ServiceInfo>> {
    let api: Api<Service> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let svc_list = tokio::time::timeout(API_CALL_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("service list timeout"))??;
    let now = Utc::now();

    Ok(svc_list
        .items
        .iter()
        .map(|svc| {
            let meta = &svc.metadata;
            let spec = svc.spec.as_ref();

            let svc_type = spec
                .and_then(|s| s.type_.clone())
                .unwrap_or_else(|| "ClusterIP".into());

            let cluster_ip = spec
                .and_then(|s| s.cluster_ip.clone())
                .unwrap_or_else(|| "None".into());

            let ports = spec
                .map(|s| {
                    s.ports
                        .as_ref()
                        .map(|ps| {
                            ps.iter()
                                .map(|p| {
                                    let proto = p.protocol.as_deref().unwrap_or("TCP");
                                    match p.node_port {
                                        Some(np) => format!("{}:{}/{}", p.port, np, proto),
                                        None => format!("{}/{}", p.port, proto),
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();

            let age = meta
                .creation_timestamp
                .as_ref()
                .map(|ts| format_age(now, ts.0))
                .unwrap_or_else(|| "<unknown>".into());

            // External IP from LoadBalancer ingress status
            let external_ip = svc
                .status
                .as_ref()
                .and_then(|s| s.load_balancer.as_ref())
                .and_then(|lb| lb.ingress.as_ref())
                .and_then(|ingress| {
                    ingress.first().and_then(|i| {
                        i.ip.clone().or_else(|| i.hostname.clone())
                    })
                })
                .unwrap_or_else(|| "<none>".into());

            ServiceInfo {
                name: meta.name.clone().unwrap_or_default(),
                namespace: meta.namespace.clone().unwrap_or_default(),
                svc_type,
                cluster_ip,
                external_ip,
                ports,
                age,
            }
        })
        .collect())
}

pub async fn fetch_events(client: &Client, namespace: Option<&str>) -> Result<Vec<EventInfo>> {
    let api: Api<Event> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let event_list = tokio::time::timeout(API_CALL_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("event list timeout"))??;
    let now = Utc::now();

    let mut events: Vec<EventInfo> = event_list
        .items
        .iter()
        .map(|evt| {
            let meta = &evt.metadata;

            let event_type = evt.type_.clone().unwrap_or_else(|| "Normal".into());
            let reason = evt.reason.clone().unwrap_or_default();
            let message = evt.message.clone().unwrap_or_default();
            let count = evt.count.unwrap_or(1);

            // Involved object: "Kind/name"
            let object = {
                let obj = &evt.involved_object;
                let kind = obj.kind.as_deref().unwrap_or("");
                let name = obj.name.as_deref().unwrap_or("");
                if kind.is_empty() {
                    name.to_string()
                } else {
                    format!("{}/{}", kind, name)
                }
            };

            // Last seen: use last_timestamp, fall back to event_time, then creation_timestamp
            let last_ts = evt
                .last_timestamp
                .as_ref()
                .map(|t| t.0)
                .or_else(|| evt.event_time.as_ref().map(|t| t.0))
                .or_else(|| meta.creation_timestamp.as_ref().map(|t| t.0));

            let last_seen = last_ts
                .map(|ts| format_age(now, ts))
                .unwrap_or_else(|| "<unknown>".into());

            let age = meta
                .creation_timestamp
                .as_ref()
                .map(|ts| format_age(now, ts.0))
                .unwrap_or_else(|| "<unknown>".into());

            EventInfo {
                namespace: meta.namespace.clone().unwrap_or_default(),
                name: meta.name.clone().unwrap_or_default(),
                event_type,
                reason,
                object,
                message,
                count,
                count_display: count.to_string(),
                last_seen,
                age,
            }
        })
        .collect();

    // Sort by last_seen ascending so most recent events appear first
    // (last_seen is a formatted age string — shorter = more recent)
    // Use reverse sort on the raw timestamp instead for correctness
    events.sort_by(|a, b| a.last_seen.cmp(&b.last_seen));

    Ok(events)
}

// ---------------------------------------------------------------------------
// k9s-style pod sorting by status severity (errors first)
// ---------------------------------------------------------------------------

/// Status severity rank for k9s-style pod sorting.
/// Lower number = higher severity (shown first in table).
fn pod_status_severity(status: &str) -> u8 {
    match status {
        // Critical errors: shown first
        "Failed" | "Error" | "OOMKilled" | "CrashLoopBackOff" | "ImagePullBackOff"
        | "ErrImagePull" | "CreateContainerConfigError" | "InvalidImageName" | "Evicted"
        | "NodeLost" | "Shutdown" => 0,
        // Init errors
        s if s.starts_with("Init:") && (s.contains("Error") || s.contains("CrashLoopBackOff")) => {
            1
        }
        // Pending/Initializing states
        "Pending" | "ContainerCreating" | "PodInitializing" | "Terminating" => 2,
        // Init in progress
        s if s.starts_with("Init:") => 3,
        // Normal running
        "Running" => 4,
        // Completed successfully
        "Succeeded" | "Completed" => 5,
        // Unknown
        _ => 3,
    }
}

/// Sort pods by status severity (k9s-style: errors first, then pending, then running).
/// Stable sort preserves original order within each severity group.
pub fn sort_pods_by_severity(pods: &mut [PodInfo]) {
    pods.sort_by_key(|p| pod_status_severity(&p.status));
}

// ---------------------------------------------------------------------------
// Cluster snapshot (aggregate)
// ---------------------------------------------------------------------------

/// Fetch cluster snapshot with optional selective resource filtering.
///
/// - `active_resource: None` → fetch ALL resources (used for selected cluster / headless)
/// - `active_resource: Some(r)` → fetch only nodes + the active resource
///   (reduces API calls, cutting latency)
///
/// Metrics fetch is removed (metrics_server_enabled hardcoded false).
pub async fn fetch_cluster_snapshot(
    client: &Client,
    cluster_name: &str,
    namespace: Option<&str>,
    active_resource: Option<ActiveResource>,
) -> Result<ClusterSnapshot> {
    // Single parallel join for ALL API calls — eliminates sequential latency
    // between the namespaces+nodes group and the resources group.
    let (namespaces, nodes, pods, deployments, services, configmaps, events) = match active_resource {
        None => {
            // Full fetch: all 7 API calls in one parallel join
            let (ns, n, p, d, s, c, ev) = tokio::join!(
                async { fetch_namespaces(client).await.unwrap_or_default() },
                async { fetch_nodes(client).await.unwrap_or_default() },
                async { fetch_pods(client, namespace).await.unwrap_or_default() },
                async {
                    fetch_deployments(client, namespace)
                        .await
                        .unwrap_or_default()
                },
                async { fetch_services(client, namespace).await.unwrap_or_default() },
                async {
                    fetch_configmaps(client, namespace)
                        .await
                        .unwrap_or_default()
                },
                async { fetch_events(client, namespace).await.unwrap_or_default() },
            );
            (ns, n, Some(p), Some(d), Some(s), Some(c), Some(ev))
        }
        Some(ActiveResource::Pods) => {
            let (ns, n, p) = tokio::join!(
                async { fetch_namespaces(client).await.unwrap_or_default() },
                async { fetch_nodes(client).await.unwrap_or_default() },
                async { fetch_pods(client, namespace).await.unwrap_or_default() },
            );
            (ns, n, Some(p), None, None, None, None)
        }
        Some(ActiveResource::Deployments) => {
            let (ns, n, d) = tokio::join!(
                async { fetch_namespaces(client).await.unwrap_or_default() },
                async { fetch_nodes(client).await.unwrap_or_default() },
                async {
                    fetch_deployments(client, namespace)
                        .await
                        .unwrap_or_default()
                },
            );
            (ns, n, None, Some(d), None, None, None)
        }
        Some(ActiveResource::Services) => {
            let (ns, n, s) = tokio::join!(
                async { fetch_namespaces(client).await.unwrap_or_default() },
                async { fetch_nodes(client).await.unwrap_or_default() },
                async { fetch_services(client, namespace).await.unwrap_or_default() },
            );
            (ns, n, None, None, Some(s), None, None)
        }
        Some(ActiveResource::ConfigMaps) => {
            let (ns, n, c) = tokio::join!(
                async { fetch_namespaces(client).await.unwrap_or_default() },
                async { fetch_nodes(client).await.unwrap_or_default() },
                async {
                    fetch_configmaps(client, namespace)
                        .await
                        .unwrap_or_default()
                },
            );
            (ns, n, None, None, None, Some(c), None)
        }
        Some(ActiveResource::Events) => {
            let (ns, n, ev) = tokio::join!(
                async { fetch_namespaces(client).await.unwrap_or_default() },
                async { fetch_nodes(client).await.unwrap_or_default() },
                async { fetch_events(client, namespace).await.unwrap_or_default() },
            );
            (ns, n, None, None, None, None, Some(ev))
        }
        Some(ActiveResource::Nodes) => {
            // Nodes-only fetch for non-selected clusters: skip namespace API call
            // (namespaces change rarely, preserved by merge logic in run_tui)
            let n = fetch_nodes(client).await.unwrap_or_default();
            (Vec::new(), n, None, None, None, None, None)
        }
    };

    // Sort pods by status severity (k9s-style: errors first)
    let mut pods_vec = pods.unwrap_or_default();
    sort_pods_by_severity(&mut pods_vec);

    // Sort other resources alphabetically by name (k9s parity)
    let mut nodes = nodes;
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    let mut deployments_vec = deployments.unwrap_or_default();
    deployments_vec.sort_by(|a, b| a.name.cmp(&b.name));
    let mut services_vec = services.unwrap_or_default();
    services_vec.sort_by(|a, b| a.name.cmp(&b.name));
    let mut configmaps_vec = configmaps.unwrap_or_default();
    configmaps_vec.sort_by(|a, b| a.name.cmp(&b.name));
    let events_vec = events.unwrap_or_default();

    // For health computation, use pods if we have them, otherwise empty
    // (health will be recomputed on next full fetch)
    let health = compute_health(&nodes, &pods_vec);
    let resource_usage = compute_resource_usage(&nodes, &pods_vec, None);

    Ok(ClusterSnapshot {
        name: cluster_name.to_string(),
        health,
        namespaces,
        nodes,
        pods: pods_vec,
        deployments: deployments_vec,
        services: services_vec,
        configmaps: configmaps_vec,
        events: events_vec,
        resource_usage,
    })
}

pub fn filter_snapshot_by_resource(
    snapshots: &[ClusterSnapshot],
    resource: &str,
) -> serde_json::Value {
    let filtered: Vec<serde_json::Value> = snapshots
        .iter()
        .map(|s| {
            let mut obj = serde_json::json!({ "cluster": s.name, "health": s.health });
            match resource {
                "pods" => {
                    obj["pods"] = serde_json::to_value(&s.pods).unwrap_or_default();
                }
                "deployments" => {
                    obj["deployments"] = serde_json::to_value(&s.deployments).unwrap_or_default();
                }
                "services" => {
                    obj["services"] = serde_json::to_value(&s.services).unwrap_or_default();
                }
                "nodes" => {
                    obj["nodes"] = serde_json::to_value(&s.nodes).unwrap_or_default();
                }
                "configmaps" => {
                    obj["configmaps"] = serde_json::to_value(&s.configmaps).unwrap_or_default();
                }
                "events" => {
                    obj["events"] = serde_json::to_value(&s.events).unwrap_or_default();
                }
                _ => {
                    obj["error"] =
                        serde_json::json!(format!("Unknown resource type: {}", resource));
                }
            }
            obj
        })
        .collect();
    serde_json::json!({ "clusters": filtered })
}

// ---------------------------------------------------------------------------
// Pure helper functions
// ---------------------------------------------------------------------------

pub fn compute_health(nodes: &[NodeInfo], pods: &[PodInfo]) -> HealthStatus {
    let not_ready_nodes = nodes.iter().filter(|n| !n.status.starts_with("Ready")).count();
    let failed_pods = pods
        .iter()
        .filter(|p| {
            matches!(
                p.status.as_str(),
                "Failed"
                    | "CrashLoopBackOff"
                    | "Error"
                    | "OOMKilled"
                    | "ImagePullBackOff"
                    | "ErrImagePull"
                    | "Evicted"
            )
        })
        .count();

    if not_ready_nodes > 0 || failed_pods > 5 {
        HealthStatus::Red
    } else if failed_pods > 0 {
        HealthStatus::Yellow
    } else {
        HealthStatus::Green
    }
}

/// Parse a Kubernetes resource quantity string to a base value (cores for CPU, bytes for memory).
/// CPU: "100m" -> 0.1, "250000000n" -> 0.25, "2" -> 2.0
/// Memory: "1Gi" -> 1073741824.0, "512Mi" -> 536870912.0, "1024Ki" -> 1048576.0, "1000" -> 1000.0
pub fn parse_k8s_quantity(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(v) = s.strip_suffix("Ti") {
        v.parse::<f64>()
            .ok()
            .map(|n| n * 1024.0 * 1024.0 * 1024.0 * 1024.0)
    } else if let Some(v) = s.strip_suffix("Gi") {
        v.parse::<f64>().ok().map(|n| n * 1024.0 * 1024.0 * 1024.0)
    } else if let Some(v) = s.strip_suffix("Mi") {
        v.parse::<f64>().ok().map(|n| n * 1024.0 * 1024.0)
    } else if let Some(v) = s.strip_suffix("Ki") {
        v.parse::<f64>().ok().map(|n| n * 1024.0)
    } else if let Some(v) = s.strip_suffix('n') {
        v.parse::<f64>().ok().map(|n| n / 1_000_000_000.0)
    } else if let Some(v) = s.strip_suffix('m') {
        v.parse::<f64>().ok().map(|n| n / 1000.0)
    } else {
        s.parse::<f64>().ok()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeMetrics {
    pub name: String,
    pub cpu_usage: f64, // cores
    pub mem_usage: f64, // bytes
}

/// Fetch node metrics from metrics.k8s.io/v1beta1 (requires metrics-server).
/// Returns empty vec if metrics API is unavailable.
/// Currently unused: metrics_server_enabled is hardcoded false in kubespray.rs.
#[allow(dead_code)]
pub async fn fetch_node_metrics(client: &Client) -> Result<Vec<NodeMetrics>> {
    use kube::api::ApiResource;
    use kube::api::DynamicObject;

    let ar = ApiResource {
        group: "metrics.k8s.io".into(),
        version: "v1beta1".into(),
        api_version: "metrics.k8s.io/v1beta1".into(),
        kind: "NodeMetrics".into(),
        plural: "nodes".into(),
    };

    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
    let list = api.list(&ListParams::default()).await?;

    Ok(list
        .items
        .iter()
        .filter_map(|obj| {
            let name = obj.metadata.name.clone().unwrap_or_default();
            let usage = obj.data.get("usage")?;
            let cpu_str = usage.get("cpu")?.as_str()?;
            let mem_str = usage.get("memory")?.as_str()?;
            Some(NodeMetrics {
                name,
                cpu_usage: parse_k8s_quantity(cpu_str)?,
                mem_usage: parse_k8s_quantity(mem_str)?,
            })
        })
        .collect())
}

pub fn compute_resource_usage(
    nodes: &[NodeInfo],
    pods: &[PodInfo],
    node_metrics: Option<&[NodeMetrics]>,
) -> ResourceUsage {
    let total_nodes = nodes.len();
    let ready_nodes = nodes.iter().filter(|n| n.status.starts_with("Ready")).count();
    let total_pods = pods.len();
    let running_pods = pods.iter().filter(|p| p.status == "Running").count();
    let succeeded_pods = pods
        .iter()
        .filter(|p| matches!(p.status.as_str(), "Succeeded" | "Completed"))
        .count();
    let failed_pods = pods
        .iter()
        .filter(|p| {
            matches!(
                p.status.as_str(),
                "Failed"
                    | "CrashLoopBackOff"
                    | "Error"
                    | "OOMKilled"
                    | "ImagePullBackOff"
                    | "ErrImagePull"
                    | "Evicted"
            )
        })
        .count();

    // Compute CPU/Mem percentages from metrics-server data if available
    let (cpu_percent, mem_percent) = node_metrics
        .filter(|m| !m.is_empty())
        .map(|metrics| {
            let total_cpu_usage: f64 = metrics.iter().map(|m| m.cpu_usage).sum();
            let total_mem_usage: f64 = metrics.iter().map(|m| m.mem_usage).sum();
            let total_cpu_capacity: f64 = nodes
                .iter()
                .filter_map(|n| parse_k8s_quantity(&n.cpu_capacity))
                .sum();
            let total_mem_capacity: f64 = nodes
                .iter()
                .filter_map(|n| parse_k8s_quantity(&n.mem_capacity))
                .sum();
            let cpu_pct = if total_cpu_capacity > 0.0 {
                (total_cpu_usage / total_cpu_capacity) * 100.0
            } else {
                0.0
            };
            let mem_pct = if total_mem_capacity > 0.0 {
                (total_mem_usage / total_mem_capacity) * 100.0
            } else {
                0.0
            };
            (cpu_pct, mem_pct)
        })
        .unwrap_or((-1.0, -1.0)); // sentinel: no metrics data → render_usage_bar shows N/A

    ResourceUsage {
        cpu_percent,
        mem_percent,
        total_pods,
        running_pods,
        succeeded_pods,
        failed_pods,
        total_nodes,
        ready_nodes,
    }
}

/// Derive effective pod status by checking container waiting reasons.
/// K8s reports phase=Running even when containers are in CrashLoopBackOff.
/// Also handles init containers (Init:N/M) and pod-level reasons (Evicted).
fn derive_effective_status(
    phase: &str,
    container_statuses: &[k8s_openapi::api::core::v1::ContainerStatus],
    init_container_statuses: &[k8s_openapi::api::core::v1::ContainerStatus],
    pod_reason: Option<&str>,
) -> String {
    // Pod-level reason overrides phase (e.g., Evicted)
    if let Some(reason) = pod_reason {
        if reason == "Evicted" || reason == "NodeLost" || reason == "Shutdown" {
            return reason.to_string();
        }
    }

    // Check init containers first: if any init container is not completed,
    // show Init:completed/total (mirrors kubectl behavior)
    if !init_container_statuses.is_empty() {
        let total = init_container_statuses.len();
        let completed = init_container_statuses
            .iter()
            .filter(|cs| {
                cs.state
                    .as_ref()
                    .and_then(|s| s.terminated.as_ref())
                    .is_some_and(|t| t.exit_code == 0)
            })
            .count();
        if completed < total {
            // Check for error in init containers
            for cs in init_container_statuses {
                if let Some(state) = &cs.state {
                    if let Some(waiting) = &state.waiting {
                        if let Some(reason) = &waiting.reason {
                            match reason.as_str() {
                                "CrashLoopBackOff" | "ImagePullBackOff" | "ErrImagePull" => {
                                    return format!("Init:{} ({})", reason, cs.name);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            return format!("Init:{}/{}", completed, total);
        }
    }

    // Check for waiting containers with error reasons
    for cs in container_statuses {
        if let Some(state) = &cs.state {
            if let Some(waiting) = &state.waiting {
                if let Some(reason) = &waiting.reason {
                    match reason.as_str() {
                        "CrashLoopBackOff"
                        | "ImagePullBackOff"
                        | "ErrImagePull"
                        | "CreateContainerConfigError"
                        | "InvalidImageName" => {
                            return reason.clone();
                        }
                        _ => {}
                    }
                }
            }
            // Check for terminated containers with error
            if let Some(terminated) = &state.terminated {
                if let Some(reason) = &terminated.reason {
                    if reason == "Error" || reason == "OOMKilled" {
                        return reason.clone();
                    }
                }
            }
        }
    }
    phase.to_string()
}

/// Format a K8s resource quantity string to human-readable form.
/// Memory: "7816040Ki" → "7.5Gi", "512Mi" → "512Mi", "1073741824" → "1.0Gi"
/// CPU: "4" → "4", "500m" → "500m" (already readable, returned as-is)
pub fn format_k8s_memory(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "N/A".to_string();
    }
    // Parse to bytes first, then pick best unit
    let bytes = match parse_k8s_quantity(s) {
        Some(b) => b,
        None => return s.to_string(), // unparseable, return raw
    };
    // If the original string has memory suffixes (Ki/Mi/Gi/Ti) or is a plain number >= 1024,
    // format as human-readable. CPU values (m, n, plain small numbers) pass through.
    let is_memory = s.ends_with("Ki")
        || s.ends_with("Mi")
        || s.ends_with("Gi")
        || s.ends_with("Ti")
        || (s.parse::<f64>().is_ok() && bytes >= 1024.0);

    if !is_memory {
        return s.to_string();
    }

    const TI: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;
    const GI: f64 = 1024.0 * 1024.0 * 1024.0;
    const MI: f64 = 1024.0 * 1024.0;
    const KI: f64 = 1024.0;

    if bytes >= TI {
        let val = bytes / TI;
        if (val - val.round()).abs() < 0.05 {
            format!("{:.0}Ti", val)
        } else {
            format!("{:.1}Ti", val)
        }
    } else if bytes >= GI {
        let val = bytes / GI;
        if (val - val.round()).abs() < 0.05 {
            format!("{:.0}Gi", val)
        } else {
            format!("{:.1}Gi", val)
        }
    } else if bytes >= MI {
        let val = bytes / MI;
        if (val - val.round()).abs() < 0.05 {
            format!("{:.0}Mi", val)
        } else {
            format!("{:.1}Mi", val)
        }
    } else if bytes >= KI {
        let val = bytes / KI;
        if (val - val.round()).abs() < 0.05 {
            format!("{:.0}Ki", val)
        } else {
            format!("{:.1}Ki", val)
        }
    } else {
        format!("{:.0}B", bytes)
    }
}

fn format_age(now: chrono::DateTime<Utc>, created: chrono::DateTime<Utc>) -> String {
    let duration = now.signed_duration_since(created);
    let secs = duration.num_seconds();
    // Guard: clock skew — created in the future (US-402)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_green_when_all_ok() {
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            roles: vec![],
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
        }];
        assert_eq!(compute_health(&nodes, &pods), HealthStatus::Green);
    }

    #[test]
    fn health_red_when_node_not_ready() {
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "NotReady".into(),
            roles: vec![],
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()        }];
        assert_eq!(compute_health(&nodes, &[]), HealthStatus::Red);
    }

    #[test]
    fn health_yellow_when_few_failed_pods() {
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            roles: vec![],
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Failed".into(),
            ready: "0/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
        }];
        assert_eq!(compute_health(&nodes, &pods), HealthStatus::Yellow);
    }

    #[test]
    fn health_yellow_when_oomkilled_pod() {
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            roles: vec![],
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "OOMKilled".into(),
            ready: "0/1".into(),
            restarts: 1,
            restarts_display: "1".into(),
            age: "1h".into(),
            node: "n1".into(),
        }];
        assert_eq!(compute_health(&nodes, &pods), HealthStatus::Yellow);
    }

    #[test]
    fn health_yellow_when_imagepullbackoff_pod() {
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            roles: vec![],
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "ImagePullBackOff".into(),
            ready: "0/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "5m".into(),
            node: "n1".into(),
        }];
        assert_eq!(compute_health(&nodes, &pods), HealthStatus::Yellow);
    }

    #[test]
    fn format_age_seconds() {
        let now = Utc::now();
        let created = now - chrono::Duration::seconds(30);
        assert_eq!(format_age(now, created), "30s");
    }

    #[test]
    fn format_age_days() {
        let now = Utc::now();
        let created = now - chrono::Duration::days(3);
        assert_eq!(format_age(now, created), "3d");
    }

    #[test]
    fn format_age_weeks() {
        let now = Utc::now();
        let created = now - chrono::Duration::days(14);
        assert_eq!(format_age(now, created), "2w");
    }

    #[test]
    fn format_age_years() {
        let now = Utc::now();
        let created = now - chrono::Duration::days(400);
        assert_eq!(format_age(now, created), "1y");
    }

    #[test]
    fn format_age_clock_skew_returns_zero() {
        let now = Utc::now();
        let future = now + chrono::Duration::seconds(30);
        assert_eq!(format_age(now, future), "0s");
    }

    #[test]
    fn derive_status_returns_phase_when_no_waiting() {
        let statuses = vec![];
        assert_eq!(derive_effective_status("Running", &statuses, &[], None), "Running");
    }

    #[test]
    fn derive_status_detects_crashloopbackoff() {
        use k8s_openapi::api::core::v1::{ContainerState, ContainerStateWaiting, ContainerStatus};

        let statuses = vec![ContainerStatus {
            name: "app".into(),
            ready: false,
            restart_count: 5,
            image: "test:latest".into(),
            image_id: "".into(),
            state: Some(ContainerState {
                waiting: Some(ContainerStateWaiting {
                    reason: Some("CrashLoopBackOff".into()),
                    message: None,
                }),
                running: None,
                terminated: None,
            }),
            ..Default::default()
        }];
        assert_eq!(
            derive_effective_status("Running", &statuses, &[], None),
            "CrashLoopBackOff"
        );
    }

    #[test]
    fn parse_k8s_quantity_cpu_millicores() {
        assert!((parse_k8s_quantity("100m").unwrap() - 0.1).abs() < 1e-9);
        assert!((parse_k8s_quantity("250m").unwrap() - 0.25).abs() < 1e-9);
        assert!((parse_k8s_quantity("1000m").unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn parse_k8s_quantity_cpu_nanocores() {
        assert!((parse_k8s_quantity("250000000n").unwrap() - 0.25).abs() < 1e-6);
        assert!((parse_k8s_quantity("1000000000n").unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parse_k8s_quantity_cpu_cores() {
        assert!((parse_k8s_quantity("2").unwrap() - 2.0).abs() < 1e-9);
        assert!((parse_k8s_quantity("4").unwrap() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn parse_k8s_quantity_memory() {
        assert!((parse_k8s_quantity("1Gi").unwrap() - 1_073_741_824.0).abs() < 1.0);
        assert!((parse_k8s_quantity("512Mi").unwrap() - 536_870_912.0).abs() < 1.0);
        assert!((parse_k8s_quantity("1024Ki").unwrap() - 1_048_576.0).abs() < 1.0);
        assert!((parse_k8s_quantity("1000").unwrap() - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn parse_k8s_quantity_empty() {
        assert!(parse_k8s_quantity("").is_none());
        assert!(parse_k8s_quantity("  ").is_none());
    }

    #[test]
    fn format_k8s_memory_ki_to_gi() {
        // 7816040Ki ≈ 7.5Gi
        assert_eq!(format_k8s_memory("7816040Ki"), "7.5Gi");
    }

    #[test]
    fn format_k8s_memory_exact_gi() {
        assert_eq!(format_k8s_memory("8Gi"), "8Gi");
    }

    #[test]
    fn format_k8s_memory_mi() {
        assert_eq!(format_k8s_memory("512Mi"), "512Mi");
    }

    #[test]
    fn format_k8s_memory_plain_bytes_large() {
        // 1073741824 bytes = 1Gi
        assert_eq!(format_k8s_memory("1073741824"), "1Gi");
    }

    #[test]
    fn format_k8s_memory_cpu_passthrough() {
        // CPU values should not be reformatted
        assert_eq!(format_k8s_memory("4"), "4");
        assert_eq!(format_k8s_memory("500m"), "500m");
    }

    #[test]
    fn format_k8s_memory_empty() {
        assert_eq!(format_k8s_memory(""), "N/A");
    }

    #[test]
    fn configmap_info_serialization() {
        let cm = ConfigMapInfo {
            name: "test-cm".into(),
            namespace: "default".into(),
            data_keys_count: 3,
            data_keys_display: "3".into(),
            age: "1h".into(),
        };
        let json = serde_json::to_value(&cm).unwrap();
        assert_eq!(json["name"], "test-cm");
        assert_eq!(json["data_keys_count"], 3);
    }

    #[test]
    fn compute_resource_usage_without_metrics() {
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            roles: vec![],
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
        }];
        let usage = compute_resource_usage(&nodes, &pods, None);
        assert!((usage.cpu_percent - (-1.0)).abs() < 1e-9); // sentinel: no metrics → -1.0
        assert!((usage.mem_percent - (-1.0)).abs() < 1e-9);
        assert_eq!(usage.running_pods, 1);
    }

    #[test]
    fn compute_resource_usage_with_metrics() {
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            roles: vec![],
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()        }];
        let metrics = vec![NodeMetrics {
            name: "n1".into(),
            cpu_usage: 2.0,
            mem_usage: 4.0 * 1024.0 * 1024.0 * 1024.0,
        }];
        let usage = compute_resource_usage(&nodes, &[], Some(&metrics));
        assert!((usage.cpu_percent - 50.0).abs() < 1e-6);
        assert!((usage.mem_percent - 50.0).abs() < 1e-6);
    }

    #[test]
    fn derive_status_detects_oomkilled() {
        use k8s_openapi::api::core::v1::{
            ContainerState, ContainerStateTerminated, ContainerStatus,
        };

        let statuses = vec![ContainerStatus {
            name: "app".into(),
            ready: false,
            restart_count: 1,
            image: "test:latest".into(),
            image_id: "".into(),
            state: Some(ContainerState {
                waiting: None,
                running: None,
                terminated: Some(ContainerStateTerminated {
                    reason: Some("OOMKilled".into()),
                    exit_code: 137,
                    ..Default::default()
                }),
            }),
            ..Default::default()
        }];
        assert_eq!(derive_effective_status("Running", &statuses, &[], None), "OOMKilled");
    }

    #[test]
    fn derive_status_evicted_pod() {
        assert_eq!(
            derive_effective_status("Failed", &[], &[], Some("Evicted")),
            "Evicted"
        );
    }

    #[test]
    fn derive_status_init_containers_pending() {
        use k8s_openapi::api::core::v1::{
            ContainerState, ContainerStateWaiting, ContainerStatus,
        };

        let init_statuses = vec![
            ContainerStatus {
                name: "init-1".into(),
                ready: false,
                restart_count: 0,
                image: "busybox:latest".into(),
                image_id: "".into(),
                state: Some(ContainerState {
                    waiting: Some(ContainerStateWaiting {
                        reason: Some("PodInitializing".into()),
                        message: None,
                    }),
                    running: None,
                    terminated: None,
                }),
                ..Default::default()
            },
            ContainerStatus {
                name: "init-2".into(),
                ready: false,
                restart_count: 0,
                image: "busybox:latest".into(),
                image_id: "".into(),
                state: Some(ContainerState {
                    waiting: Some(ContainerStateWaiting {
                        reason: Some("PodInitializing".into()),
                        message: None,
                    }),
                    running: None,
                    terminated: None,
                }),
                ..Default::default()
            },
        ];
        assert_eq!(
            derive_effective_status("Pending", &[], &init_statuses, None),
            "Init:0/2"
        );
    }

    #[test]
    fn derive_status_init_containers_partial() {
        use k8s_openapi::api::core::v1::{
            ContainerState, ContainerStateTerminated, ContainerStateWaiting, ContainerStatus,
        };

        let init_statuses = vec![
            ContainerStatus {
                name: "init-1".into(),
                ready: false,
                restart_count: 0,
                image: "busybox:latest".into(),
                image_id: "".into(),
                state: Some(ContainerState {
                    waiting: None,
                    running: None,
                    terminated: Some(ContainerStateTerminated {
                        exit_code: 0,
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
            ContainerStatus {
                name: "init-2".into(),
                ready: false,
                restart_count: 0,
                image: "busybox:latest".into(),
                image_id: "".into(),
                state: Some(ContainerState {
                    waiting: Some(ContainerStateWaiting {
                        reason: Some("PodInitializing".into()),
                        message: None,
                    }),
                    running: None,
                    terminated: None,
                }),
                ..Default::default()
            },
        ];
        assert_eq!(
            derive_effective_status("Pending", &[], &init_statuses, None),
            "Init:1/2"
        );
    }

    #[test]
    fn health_counts_evicted_as_failed() {
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            roles: vec![],
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()        }];
        let pods = vec![PodInfo {
            name: "evicted-pod".into(),
            namespace: "default".into(),
            status: "Evicted".into(),
            ready: "0/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
        }];
        assert_eq!(compute_health(&nodes, &pods), HealthStatus::Yellow);
    }

    // --- US-004: k9s-style pod sorting by status severity ---

    #[test]
    fn pod_sort_order_errors_first() {
        let make_pod = |name: &str, status: &str| PodInfo {
            name: name.into(),
            namespace: "default".into(),
            status: status.into(),
            ready: "1/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
        };
        let mut pods = vec![
            make_pod("running-1", "Running"),
            make_pod("crash-1", "CrashLoopBackOff"),
            make_pod("completed-1", "Succeeded"),
            make_pod("pending-1", "Pending"),
            make_pod("error-1", "Error"),
            make_pod("running-2", "Running"),
            make_pod("oom-1", "OOMKilled"),
            make_pod("init-err", "Init:CrashLoopBackOff (sidecar)"),
            make_pod("init-ok", "Init:1/2"),
        ];
        sort_pods_by_severity(&mut pods);
        let names: Vec<&str> = pods.iter().map(|p| p.name.as_str()).collect();
        // Errors first (severity 0): crash-1, error-1, oom-1
        // Init errors (severity 1): init-err
        // Pending (severity 2): pending-1
        // Init in progress (severity 3): init-ok
        // Running (severity 4): running-1, running-2
        // Succeeded (severity 5): completed-1
        assert_eq!(
            names,
            vec![
                "crash-1", "error-1", "oom-1",
                "init-err",
                "pending-1",
                "init-ok",
                "running-1", "running-2",
                "completed-1",
            ]
        );
    }

    #[test]
    fn pod_sort_stable_within_group() {
        let make_pod = |name: &str| PodInfo {
            name: name.into(),
            namespace: "default".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
        };
        let mut pods = vec![make_pod("b-pod"), make_pod("a-pod"), make_pod("c-pod")];
        sort_pods_by_severity(&mut pods);
        // All same severity → original order preserved (stable sort)
        let names: Vec<&str> = pods.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b-pod", "a-pod", "c-pod"]);
    }
}
