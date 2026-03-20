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
/// 5s accommodates CF Tunnel connections (DNS + Cloudflare edge + TLS handshake)
/// which routinely exceed 500ms on cold start. Healthy direct-IP clusters still
/// complete in <200ms, so this only affects worst-case latency.
pub const API_CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-API-call timeout for headless mode.
/// Remote clusters (CF Tunnel, SSH tunnel) may have higher latency than 500ms.
/// 30s handles slow tunneled connections (SSH → remote bastion → K8s API) without
/// blocking CI scripts indefinitely. Each of the 7 parallel API calls has this
/// individual budget; total wall time is bounded by the slowest single call.
pub const HEADLESS_API_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// InternalIP address from node.status.addresses (for VM-to-Node mapping)
    pub internal_ip: String,
    /// Per-node CPU usage as percentage of capacity. None = no metrics.
    pub cpu_usage_percent: Option<f64>,
    /// Per-node MEM usage as percentage of capacity. None = no metrics.
    pub mem_usage_percent: Option<f64>,
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
    /// Container names in the pod (for log viewer container selector).
    pub containers: Vec<String>,
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
    /// Aggregate pod CPU utilization as % of total node allocatable CPU.
    /// -1.0 when pod metrics are unavailable (sentinel → renders as N/A).
    pub pod_cpu_percent: f64,
    /// Aggregate pod MEM utilization as % of total node allocatable memory.
    /// -1.0 when pod metrics are unavailable (sentinel → renders as N/A).
    pub pod_mem_percent: f64,
}

// ---------------------------------------------------------------------------
// Fetch functions (async, pure-ish — only side effect is network I/O)
// ---------------------------------------------------------------------------

pub async fn fetch_namespaces(client: &Client, timeout: Duration) -> Result<Vec<String>> {
    let api: Api<Namespace> = Api::all(client.clone());
    let ns_list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
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

pub async fn fetch_pods(
    client: &Client,
    namespace: Option<&str>,
    timeout: Duration,
) -> Result<Vec<PodInfo>> {
    let api: Api<Pod> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let pod_list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
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

            // Extract container names for log viewer container selector
            let containers: Vec<String> = spec
                .map(|s| {
                    let mut names: Vec<String> = Vec::new();
                    if let Some(init_cs) = &s.init_containers {
                        for ic in init_cs {
                            names.push(format!("init:{}", ic.name));
                        }
                    }
                    for c in &s.containers {
                        names.push(c.name.clone());
                    }
                    names
                })
                .unwrap_or_default();

            PodInfo {
                name: meta.name.clone().unwrap_or_default(),
                namespace: meta.namespace.clone().unwrap_or_default(),
                status: effective_status,
                ready: format!("{}/{}", ready_count, total_count),
                restarts_display: restarts.to_string(),
                restarts,
                age,
                node: spec.and_then(|s| s.node_name.clone()).unwrap_or_default(),
                containers,
            }
        })
        .collect())
}

pub async fn fetch_nodes(client: &Client, timeout: Duration) -> Result<Vec<NodeInfo>> {
    let api: Api<Node> = Api::all(client.clone());
    let node_list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
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
            let mem_display = format!("{}/{}", mem_allocatable_display, mem_capacity_display);

            let kubelet_version = status
                .and_then(|s| s.node_info.as_ref())
                .map(|ni| ni.kubelet_version.clone())
                .unwrap_or_default();

            let top_display = format!(
                "  {}  CPU: {}  MEM: {}",
                kubelet_version, cpu_display, mem_display
            );

            let internal_ip = status
                .and_then(|s| s.addresses.as_ref())
                .and_then(|addrs| addrs.iter().find(|a| a.type_ == "InternalIP"))
                .map(|a| a.address.clone())
                .unwrap_or_default();

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
                internal_ip,
                cpu_usage_percent: None,
                mem_usage_percent: None,
            }
        })
        .collect())
}

pub async fn fetch_deployments(
    client: &Client,
    namespace: Option<&str>,
    timeout: Duration,
) -> Result<Vec<DeploymentInfo>> {
    let api: Api<Deployment> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let dep_list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
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
    timeout: Duration,
) -> Result<Vec<ConfigMapInfo>> {
    let api: Api<ConfigMap> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let cm_list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
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

pub async fn fetch_services(
    client: &Client,
    namespace: Option<&str>,
    timeout: Duration,
) -> Result<Vec<ServiceInfo>> {
    let api: Api<Service> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let svc_list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
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
                    ingress
                        .first()
                        .and_then(|i| i.ip.clone().or_else(|| i.hostname.clone()))
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

pub async fn fetch_events(
    client: &Client,
    namespace: Option<&str>,
    timeout: Duration,
) -> Result<Vec<EventInfo>> {
    let api: Api<Event> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let event_list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
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
        "Failed"
        | "Error"
        | "OOMKilled"
        | "CrashLoopBackOff"
        | "ImagePullBackOff"
        | "ErrImagePull"
        | "CreateContainerConfigError"
        | "InvalidImageName"
        | "Evicted"
        | "NodeLost"
        | "Shutdown" => 0,
        // Init errors
        s if s.starts_with("Init:") && (s.contains("Error") || s.contains("CrashLoopBackOff")) => 1,
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
/// - `api_timeout` — per-call timeout; use `API_CALL_TIMEOUT` for TUI (500ms) and
///   `HEADLESS_API_TIMEOUT` for headless mode (30s) to handle remote cluster latency.
///
/// Node metrics are fetched from metrics.k8s.io/v1beta1 (metrics-server) when available.
/// When metrics-server is not installed, metrics fetch fails gracefully and cpu_percent/mem_percent
/// are set to the -1.0 sentinel, causing the UI to display "N/A" bars instead of percentages.
pub async fn fetch_cluster_snapshot(
    client: &Client,
    cluster_name: &str,
    namespace: Option<&str>,
    active_resource: Option<ActiveResource>,
    api_timeout: Duration,
) -> Result<ClusterSnapshot> {
    // Single parallel join for ALL API calls — eliminates sequential latency
    // between the namespaces+nodes group and the resources group.
    let (namespaces, nodes, pods, deployments, services, configmaps, events) = match active_resource
    {
        None => {
            // Full fetch: all 7 API calls in one parallel join
            let (ns, n, p, d, s, c, ev) = tokio::join!(
                async {
                    fetch_namespaces(client, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async { fetch_nodes(client, api_timeout).await.unwrap_or_default() },
                async {
                    fetch_pods(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async {
                    fetch_deployments(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async {
                    fetch_services(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async {
                    fetch_configmaps(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async {
                    fetch_events(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
            );
            (ns, n, Some(p), Some(d), Some(s), Some(c), Some(ev))
        }
        Some(ActiveResource::Pods) => {
            let (ns, n, p) = tokio::join!(
                async {
                    fetch_namespaces(client, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async { fetch_nodes(client, api_timeout).await.unwrap_or_default() },
                async {
                    fetch_pods(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
            );
            (ns, n, Some(p), None, None, None, None)
        }
        Some(ActiveResource::Deployments) => {
            let (ns, n, d) = tokio::join!(
                async {
                    fetch_namespaces(client, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async { fetch_nodes(client, api_timeout).await.unwrap_or_default() },
                async {
                    fetch_deployments(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
            );
            (ns, n, None, Some(d), None, None, None)
        }
        Some(ActiveResource::Services) => {
            let (ns, n, s) = tokio::join!(
                async {
                    fetch_namespaces(client, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async { fetch_nodes(client, api_timeout).await.unwrap_or_default() },
                async {
                    fetch_services(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
            );
            (ns, n, None, None, Some(s), None, None)
        }
        Some(ActiveResource::ConfigMaps) => {
            let (ns, n, c) = tokio::join!(
                async {
                    fetch_namespaces(client, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async { fetch_nodes(client, api_timeout).await.unwrap_or_default() },
                async {
                    fetch_configmaps(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
            );
            (ns, n, None, None, None, Some(c), None)
        }
        Some(ActiveResource::Events) => {
            let (ns, n, ev) = tokio::join!(
                async {
                    fetch_namespaces(client, api_timeout)
                        .await
                        .unwrap_or_default()
                },
                async { fetch_nodes(client, api_timeout).await.unwrap_or_default() },
                async {
                    fetch_events(client, namespace, api_timeout)
                        .await
                        .unwrap_or_default()
                },
            );
            (ns, n, None, None, None, None, Some(ev))
        }
        Some(ActiveResource::Nodes) => {
            // Nodes-only fetch for non-selected clusters: skip namespace API call
            // (namespaces change rarely, preserved by merge logic in run_tui)
            let n = fetch_nodes(client, api_timeout).await.unwrap_or_default();
            (Vec::new(), n, None, None, None, None, None)
        }
    };

    // Namespace fallback: if fetch_namespaces failed/timed-out (returned empty),
    // derive namespace list from the actually-fetched resources so headless mode
    // always returns non-empty namespace lists for each connected cluster.
    //
    // Three-tier fallback strategy:
    //   1. Use fetch_namespaces result if non-empty (normal path, requires cluster RBAC)
    //   2. If a namespace filter was specified, return at least [requested_namespace]
    //   3. Otherwise, collect unique namespace names from all successfully-fetched resources
    let namespaces = if namespaces.is_empty() {
        if let Some(explicit_ns) = namespace {
            // Namespace filter was specified — always include the requested namespace
            vec![explicit_ns.to_string()]
        } else {
            // Derive unique namespace names from all fetched namespaced resources
            let mut ns_set = std::collections::BTreeSet::new();
            if let Some(ref p) = pods {
                for item in p {
                    if !item.namespace.is_empty() {
                        ns_set.insert(item.namespace.clone());
                    }
                }
            }
            if let Some(ref d) = deployments {
                for item in d {
                    if !item.namespace.is_empty() {
                        ns_set.insert(item.namespace.clone());
                    }
                }
            }
            if let Some(ref s) = services {
                for item in s {
                    if !item.namespace.is_empty() {
                        ns_set.insert(item.namespace.clone());
                    }
                }
            }
            if let Some(ref c) = configmaps {
                for item in c {
                    if !item.namespace.is_empty() {
                        ns_set.insert(item.namespace.clone());
                    }
                }
            }
            if let Some(ref ev) = events {
                for item in ev {
                    if !item.namespace.is_empty() {
                        ns_set.insert(item.namespace.clone());
                    }
                }
            }
            ns_set.into_iter().collect()
        }
    } else {
        namespaces
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

    // Try to fetch node metrics from metrics.k8s.io (requires metrics-server).
    // Fails gracefully when metrics-server is not installed (returns Err → treated as None).
    // Uses the same api_timeout as other API calls.
    let node_metrics = fetch_node_metrics(client, api_timeout).await.ok();
    let pod_metrics = fetch_pod_metrics(client, api_timeout).await.ok();
    let resource_usage = compute_resource_usage(
        &nodes,
        &pods_vec,
        node_metrics.as_deref(),
        pod_metrics.as_deref(),
    );

    // Enrich nodes with per-node metrics for infra view VM-to-Node mapping
    if let Some(ref metrics) = node_metrics {
        for node in &mut nodes {
            if let Some(nm) = metrics.iter().find(|m| m.name == node.name) {
                let cpu_cap = parse_k8s_quantity(&node.cpu_capacity).unwrap_or(0.0);
                let mem_cap = parse_k8s_quantity(&node.mem_capacity).unwrap_or(0.0);
                node.cpu_usage_percent = Some(if cpu_cap > 0.0 {
                    (nm.cpu_usage / cpu_cap) * 100.0
                } else {
                    0.0
                });
                node.mem_usage_percent = Some(if mem_cap > 0.0 {
                    (nm.mem_usage / mem_cap) * 100.0
                } else {
                    0.0
                });
            }
        }
    }

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
                "namespaces" => {
                    // Return namespace list for each connected cluster
                    obj["namespaces"] = serde_json::to_value(&s.namespaces).unwrap_or_default();
                }
                // "checks" is handled separately in run_e2e_checks — should not reach here
                "checks" => {
                    obj["error"] = serde_json::json!("use run_e2e_checks() for checks mode");
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
// Headless checks — 5 E2E health checks for `scalex dash --headless`
// ---------------------------------------------------------------------------

/// Result of a single E2E health check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    /// Per-cluster detail (optional, included when check spans multiple clusters)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<CheckDetail>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckDetail {
    pub cluster: String,
    pub passed: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
    Skip,
    /// Failure that is explicitly listed in the known-degradation inventory.
    /// Treated as acceptable — does NOT drive overall Fail — but rendered
    /// with a distinct visual style so it is never confused with Pass.
    KnownDegraded,
}

/// Full report from all 7 E2E checks.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub overall: CheckStatus,
    pub passed: usize,
    pub failed: usize,
    pub known_degraded: usize,
    pub total: usize,
    pub checks: Vec<CheckResult>,
}

/// Run all 7 E2E health checks against the fetched cluster snapshots.
///
/// The 7 checks are:
/// 1. cluster_api_reachable  — both clusters discovered and API connectable
/// 2. all_nodes_ready        — every node in every cluster reports Ready
/// 3. namespaces_listed      — namespace listing non-empty on both clusters
/// 4. argocd_synced          — ArgoCD server deployment available in tower
/// 5. cf_tunnel_running      — cloudflared pod Running in tower cluster
/// 6. cilium_healthy         — hubble-relay pod Running on all clusters (resolves DNS timeout)
/// 7. kyverno_healthy        — kyverno admission controller pods Running on all clusters
///
/// `known_degradations` is the parsed inventory from `config/known_degradations.yaml`.
/// Any check that would produce `Fail` and is listed in `suppresses_check` of at least
/// one inventory entry is downgraded to `KnownDegraded` instead.
pub fn run_e2e_checks(
    snapshots: &[ClusterSnapshot],
    expected_clusters: &[&str],
    known_degradations: &super::degradation::KnownDegradationsConfig,
) -> CheckReport {
    let mut checks = vec![
        check_cluster_api_reachable(snapshots, expected_clusters),
        check_all_nodes_ready(snapshots),
        check_namespaces_listed(snapshots),
        check_argocd_synced(snapshots),
        check_cf_tunnel_running(snapshots),
        check_cilium_healthy(snapshots),
        check_kyverno_healthy(snapshots),
    ];

    // Apply known-degradation suppression: Fail → KnownDegraded when explicitly listed.
    for check in &mut checks {
        if check.status == CheckStatus::Fail
            && super::degradation::is_suppressed(known_degradations, &check.name)
        {
            let suppressors =
                super::degradation::suppressors_for_check(known_degradations, &check.name);
            let ack = suppressors
                .iter()
                .map(|e| format!("ack:{} ticket:{}", e.acknowledged_by, e.ticket))
                .collect::<Vec<_>>()
                .join("; ");
            check.status = CheckStatus::KnownDegraded;
            check.message = format!("{} [known-degraded: {}]", check.message, ack);
        }
    }

    let passed = checks.iter().filter(|c| c.status == CheckStatus::Pass).count();
    let failed = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let known_degraded = checks
        .iter()
        .filter(|c| c.status == CheckStatus::KnownDegraded)
        .count();
    let total = checks.len();
    let overall = if failed > 0 {
        CheckStatus::Fail
    } else if checks.iter().any(|c| c.status == CheckStatus::Warn) {
        CheckStatus::Warn
    } else if known_degraded > 0 {
        // All actual failures are known-acceptable → surface as Warn so callers
        // can distinguish "clean" from "known-degraded but acceptable".
        CheckStatus::Warn
    } else {
        CheckStatus::Pass
    };
    CheckReport {
        overall,
        passed,
        failed,
        known_degraded,
        total,
        checks,
    }
}

/// Check 1: All expected clusters were discovered and API is reachable.
fn check_cluster_api_reachable(
    snapshots: &[ClusterSnapshot],
    expected_clusters: &[&str],
) -> CheckResult {
    let discovered: Vec<&str> = snapshots.iter().map(|s| s.name.as_str()).collect();
    let mut details = Vec::new();
    let mut all_ok = true;
    for &expected in expected_clusters {
        let found = discovered.contains(&expected);
        if !found {
            all_ok = false;
        }
        details.push(CheckDetail {
            cluster: expected.to_string(),
            passed: found,
            message: if found {
                "API reachable".to_string()
            } else {
                "cluster not discovered — API unreachable".to_string()
            },
        });
    }
    CheckResult {
        name: "cluster_api_reachable".to_string(),
        status: if all_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        message: if all_ok {
            format!(
                "All {} cluster(s) API reachable",
                expected_clusters.len()
            )
        } else {
            let missing: Vec<&&str> = expected_clusters
                .iter()
                .filter(|&&c| !discovered.contains(&c))
                .collect();
            format!("Cluster(s) unreachable: {}", missing.iter().map(|c| **c).collect::<Vec<_>>().join(", "))
        },
        details,
    }
}

/// Check 2: All nodes in all clusters report Ready status.
fn check_all_nodes_ready(snapshots: &[ClusterSnapshot]) -> CheckResult {
    let mut details = Vec::new();
    let mut all_ok = true;
    for snap in snapshots {
        let not_ready: Vec<&str> = snap
            .nodes
            .iter()
            .filter(|n| !n.status.starts_with("Ready"))
            .map(|n| n.name.as_str())
            .collect();
        let passed = not_ready.is_empty() && !snap.nodes.is_empty();
        if !passed {
            all_ok = false;
        }
        details.push(CheckDetail {
            cluster: snap.name.clone(),
            passed,
            message: if snap.nodes.is_empty() {
                "no nodes found".to_string()
            } else if not_ready.is_empty() {
                format!("{}/{} nodes Ready", snap.nodes.len(), snap.nodes.len())
            } else {
                format!(
                    "{}/{} nodes not Ready: {}",
                    not_ready.len(),
                    snap.nodes.len(),
                    not_ready.join(", ")
                )
            },
        });
    }
    CheckResult {
        name: "all_nodes_ready".to_string(),
        status: if all_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        message: if all_ok {
            let total: usize = snapshots.iter().map(|s| s.nodes.len()).sum();
            format!("All {} nodes Ready across {} cluster(s)", total, snapshots.len())
        } else {
            "Some nodes not Ready".to_string()
        },
        details,
    }
}

/// Check 3: Namespace listing succeeds (non-empty) on all clusters.
fn check_namespaces_listed(snapshots: &[ClusterSnapshot]) -> CheckResult {
    let mut details = Vec::new();
    let mut all_ok = true;
    for snap in snapshots {
        let passed = !snap.namespaces.is_empty();
        if !passed {
            all_ok = false;
        }
        details.push(CheckDetail {
            cluster: snap.name.clone(),
            passed,
            message: if passed {
                format!("{} namespaces listed", snap.namespaces.len())
            } else {
                "namespace listing empty".to_string()
            },
        });
    }
    CheckResult {
        name: "namespaces_listed".to_string(),
        status: if all_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        message: if all_ok {
            format!(
                "Namespace listing OK on {} cluster(s)",
                snapshots.len()
            )
        } else {
            "Namespace listing failed on some cluster(s)".to_string()
        },
        details,
    }
}

/// Check 4: ArgoCD server deployment is available in tower cluster.
/// We verify by looking for deployments matching "argocd" in the argocd namespace.
fn check_argocd_synced(snapshots: &[ClusterSnapshot]) -> CheckResult {
    // Find the tower (management) cluster — ArgoCD runs there
    let tower = snapshots.iter().find(|s| s.name == "tower");
    match tower {
        None => CheckResult {
            name: "argocd_synced".to_string(),
            status: CheckStatus::Skip,
            message: "tower cluster not available — cannot check ArgoCD".to_string(),
            details: vec![],
        },
        Some(snap) => {
            // Look for argocd-server deployment in argocd namespace
            let argocd_deploys: Vec<&DeploymentInfo> = snap
                .deployments
                .iter()
                .filter(|d| d.namespace == "argocd")
                .collect();
            let argocd_server = argocd_deploys
                .iter()
                .find(|d| d.name.contains("argocd-server"));

            // Check that argocd-server exists and has ready replicas
            let passed = match argocd_server {
                Some(d) => {
                    // Parse "N/M" ready format to check availability
                    d.ready.split('/').next().and_then(|n| n.parse::<i32>().ok()).unwrap_or(0) > 0
                }
                None => false,
            };

            // Also check overall ArgoCD deployment health
            let all_argocd_healthy = !argocd_deploys.is_empty()
                && argocd_deploys.iter().all(|d| {
                    d.ready
                        .split('/')
                        .next()
                        .and_then(|n| n.parse::<i32>().ok())
                        .unwrap_or(0)
                        > 0
                });

            let status = if passed && all_argocd_healthy {
                CheckStatus::Pass
            } else if passed {
                CheckStatus::Warn
            } else {
                CheckStatus::Fail
            };

            let detail_msg = if argocd_deploys.is_empty() {
                "no ArgoCD deployments found in argocd namespace".to_string()
            } else {
                let summary: Vec<String> = argocd_deploys
                    .iter()
                    .map(|d| format!("{}={}", d.name, d.ready))
                    .collect();
                format!("ArgoCD deployments: {}", summary.join(", "))
            };

            CheckResult {
                name: "argocd_synced".to_string(),
                status,
                message: if status == CheckStatus::Pass {
                    format!(
                        "ArgoCD healthy — {} deployments in argocd namespace",
                        argocd_deploys.len()
                    )
                } else {
                    "ArgoCD not fully healthy".to_string()
                },
                details: vec![CheckDetail {
                    cluster: "tower".to_string(),
                    passed: status == CheckStatus::Pass,
                    message: detail_msg,
                }],
            }
        }
    }
}

/// Check 5: Cloudflare Tunnel pod is Running in tower cluster.
/// Looks for pods with "cloudflared" in their name in any namespace.
fn check_cf_tunnel_running(snapshots: &[ClusterSnapshot]) -> CheckResult {
    let tower = snapshots.iter().find(|s| s.name == "tower");
    match tower {
        None => CheckResult {
            name: "cf_tunnel_running".to_string(),
            status: CheckStatus::Skip,
            message: "tower cluster not available — cannot check CF Tunnel".to_string(),
            details: vec![],
        },
        Some(snap) => {
            let cf_pods: Vec<&PodInfo> = snap
                .pods
                .iter()
                .filter(|p| {
                    p.name.contains("cloudflared")
                        || p.name.contains("cf-tunnel")
                        || p.namespace == "kube-tunnel"
                })
                .collect();

            let running = cf_pods
                .iter()
                .filter(|p| p.status == "Running")
                .count();
            let total = cf_pods.len();

            let passed = running > 0;
            let status = if passed {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            };

            let detail_msg = if cf_pods.is_empty() {
                "no cloudflared pods found".to_string()
            } else {
                let summary: Vec<String> = cf_pods
                    .iter()
                    .map(|p| format!("{}={} ({})", p.name, p.status, p.namespace))
                    .collect();
                format!("{}/{} Running: {}", running, total, summary.join(", "))
            };

            CheckResult {
                name: "cf_tunnel_running".to_string(),
                status,
                message: if passed {
                    format!("CF Tunnel running — {}/{} pod(s) healthy", running, total)
                } else if cf_pods.is_empty() {
                    "CF Tunnel pod not found in tower cluster".to_string()
                } else {
                    format!("CF Tunnel not running — {}/{} pod(s) healthy", running, total)
                },
                details: vec![CheckDetail {
                    cluster: "tower".to_string(),
                    passed,
                    message: detail_msg,
                }],
            }
        }
    }
}

/// Check 6: Cilium CNI is healthy — hubble-relay pods are Running on every cluster.
///
/// This check specifically catches the hubble-relay DNS timeout regression where
/// hubble-relay enters CrashLoopBackOff when `peerService.clusterDomain` does not
/// match the cluster's actual DNS domain (set by Kubespray `dns_domain` variable).
/// Skips clusters where hubble-relay is not deployed (Hubble may be disabled).
fn check_cilium_healthy(snapshots: &[ClusterSnapshot]) -> CheckResult {
    let mut details = Vec::new();
    let mut any_deployed = false;
    let mut all_ok = true;

    for snap in snapshots {
        let hubble_pods: Vec<&PodInfo> = snap
            .pods
            .iter()
            .filter(|p| p.name.contains("hubble-relay"))
            .collect();

        if hubble_pods.is_empty() {
            // hubble-relay not deployed on this cluster — skip (Hubble may be disabled)
            details.push(CheckDetail {
                cluster: snap.name.clone(),
                passed: true,
                message: "hubble-relay not deployed (Hubble may be disabled)".to_string(),
            });
        } else {
            any_deployed = true;
            let running = hubble_pods.iter().filter(|p| p.status == "Running").count();
            let total = hubble_pods.len();
            let passed = running == total;
            if !passed {
                all_ok = false;
            }
            let detail_msg = if passed {
                format!("{}/{} hubble-relay pod(s) Running", running, total)
            } else {
                let not_running: Vec<String> = hubble_pods
                    .iter()
                    .filter(|p| p.status != "Running")
                    .map(|p| format!("{}={}", p.name, p.status))
                    .collect();
                format!(
                    "{}/{} Running; degraded: {}",
                    running,
                    total,
                    not_running.join(", ")
                )
            };
            details.push(CheckDetail {
                cluster: snap.name.clone(),
                passed,
                message: detail_msg,
            });
        }
    }

    // If hubble-relay is not deployed on any cluster, skip the check
    if !any_deployed {
        return CheckResult {
            name: "cilium_healthy".to_string(),
            status: CheckStatus::Skip,
            message: "hubble-relay not deployed on any cluster — skipping Cilium check"
                .to_string(),
            details,
        };
    }

    CheckResult {
        name: "cilium_healthy".to_string(),
        status: if all_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        message: if all_ok {
            format!(
                "Cilium/hubble-relay healthy across {} cluster(s)",
                snapshots.len()
            )
        } else {
            "hubble-relay not Running on one or more clusters (DNS timeout regression?)".to_string()
        },
        details,
    }
}

/// Check 7: Kyverno admission controller is healthy — kyverno pods are Running on all clusters.
///
/// This check reflects the resolved kyverno-policies ArgoCD OutOfSync state.
/// When kyverno policy YAML is out of sync (e.g., Kyverno injects `background: true`
/// defaults not present in git), kyverno webhooks may still function but ArgoCD
/// will flag OutOfSync; after the fix the kyverno pods should remain Running.
/// Skips clusters where kyverno is not deployed.
fn check_kyverno_healthy(snapshots: &[ClusterSnapshot]) -> CheckResult {
    let mut details = Vec::new();
    let mut any_deployed = false;
    let mut all_ok = true;

    for snap in snapshots {
        // Match kyverno admission controller pods in the kyverno namespace.
        // Exclude kyverno-test / kyverno-cleanup pods (short-lived jobs).
        let kyverno_pods: Vec<&PodInfo> = snap
            .pods
            .iter()
            .filter(|p| {
                p.namespace == "kyverno"
                    && (p.name.starts_with("kyverno-admission")
                        || p.name.starts_with("kyverno-background")
                        || p.name.starts_with("kyverno-reports")
                        || p.name == "kyverno"
                        || (p.name.starts_with("kyverno-") && !p.name.contains("cleanup") && !p.name.contains("test")))
            })
            .collect();

        if kyverno_pods.is_empty() {
            details.push(CheckDetail {
                cluster: snap.name.clone(),
                passed: true,
                message: "kyverno not deployed (policies may be disabled)".to_string(),
            });
        } else {
            any_deployed = true;
            let running = kyverno_pods.iter().filter(|p| p.status == "Running").count();
            let total = kyverno_pods.len();
            // Require at least one Running kyverno pod (admission controller)
            let passed = running > 0;
            if !passed {
                all_ok = false;
            }
            let detail_msg = if passed {
                format!("{}/{} kyverno pod(s) Running", running, total)
            } else {
                let not_running: Vec<String> = kyverno_pods
                    .iter()
                    .filter(|p| p.status != "Running")
                    .map(|p| format!("{}={}", p.name, p.status))
                    .collect();
                format!(
                    "{}/{} Running; degraded: {}",
                    running,
                    total,
                    not_running.join(", ")
                )
            };
            details.push(CheckDetail {
                cluster: snap.name.clone(),
                passed,
                message: detail_msg,
            });
        }
    }

    // If kyverno is not deployed on any cluster, skip the check
    if !any_deployed {
        return CheckResult {
            name: "kyverno_healthy".to_string(),
            status: CheckStatus::Skip,
            message: "kyverno not deployed on any cluster — skipping Kyverno check".to_string(),
            details,
        };
    }

    CheckResult {
        name: "kyverno_healthy".to_string(),
        status: if all_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        message: if all_ok {
            format!(
                "Kyverno admission controller healthy across {} cluster(s)",
                snapshots.len()
            )
        } else {
            "kyverno not Running on one or more clusters".to_string()
        },
        details,
    }
}

// ---------------------------------------------------------------------------
// Pure helper functions
// ---------------------------------------------------------------------------

pub fn compute_health(nodes: &[NodeInfo], pods: &[PodInfo]) -> HealthStatus {
    let not_ready_nodes = nodes
        .iter()
        .filter(|n| !n.status.starts_with("Ready"))
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
/// Returns empty vec if metrics API is unavailable or times out.
/// Gracefully fails when metrics-server is not installed — caller uses `.ok()` or `.unwrap_or_default()`.
pub async fn fetch_node_metrics(client: &Client, timeout: Duration) -> Result<Vec<NodeMetrics>> {
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
    let list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("node metrics list timeout"))??;

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

/// Per-pod metrics fetched from metrics.k8s.io/v1beta1 PodMetrics.
/// Aggregates all containers in the pod into a single CPU/MEM value.
#[derive(Debug, Clone, Serialize)]
pub struct PodMetrics {
    pub name: String,
    pub namespace: String,
    pub cpu_usage: f64, // cores (sum of all containers)
    pub mem_usage: f64, // bytes (sum of all containers)
}

/// Fetch pod metrics from metrics.k8s.io/v1beta1 (requires metrics-server).
/// Returns empty vec if metrics API is unavailable or times out.
/// Aggregates per-container CPU/MEM into per-pod totals.
pub async fn fetch_pod_metrics(client: &Client, timeout: Duration) -> Result<Vec<PodMetrics>> {
    use kube::api::ApiResource;
    use kube::api::DynamicObject;

    let ar = ApiResource {
        group: "metrics.k8s.io".into(),
        version: "v1beta1".into(),
        api_version: "metrics.k8s.io/v1beta1".into(),
        kind: "PodMetrics".into(),
        plural: "pods".into(),
    };

    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
    let list = tokio::time::timeout(timeout, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("pod metrics list timeout"))??;

    Ok(list
        .items
        .iter()
        .filter_map(|obj| {
            let name = obj.metadata.name.clone().unwrap_or_default();
            let namespace = obj.metadata.namespace.clone().unwrap_or_default();
            let containers = obj.data.get("containers")?.as_array()?;
            let mut total_cpu = 0.0f64;
            let mut total_mem = 0.0f64;
            for container in containers {
                let usage = container.get("usage")?;
                if let Some(cpu_str) = usage.get("cpu").and_then(|v| v.as_str()) {
                    total_cpu += parse_k8s_quantity(cpu_str).unwrap_or(0.0);
                }
                if let Some(mem_str) = usage.get("memory").and_then(|v| v.as_str()) {
                    total_mem += parse_k8s_quantity(mem_str).unwrap_or(0.0);
                }
            }
            Some(PodMetrics {
                name,
                namespace,
                cpu_usage: total_cpu,
                mem_usage: total_mem,
            })
        })
        .collect())
}

pub fn compute_resource_usage(
    nodes: &[NodeInfo],
    pods: &[PodInfo],
    node_metrics: Option<&[NodeMetrics]>,
    pod_metrics: Option<&[PodMetrics]>,
) -> ResourceUsage {
    let total_nodes = nodes.len();
    let ready_nodes = nodes
        .iter()
        .filter(|n| n.status.starts_with("Ready"))
        .count();
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

    // Compute aggregate pod CPU/MEM percentages vs node allocatable capacity.
    let (pod_cpu_percent, pod_mem_percent) = pod_metrics
        .filter(|m| !m.is_empty())
        .map(|metrics| {
            let total_pod_cpu: f64 = metrics.iter().map(|m| m.cpu_usage).sum();
            let total_pod_mem: f64 = metrics.iter().map(|m| m.mem_usage).sum();
            let total_cpu_allocatable: f64 = nodes
                .iter()
                .filter_map(|n| parse_k8s_quantity(&n.cpu_allocatable))
                .sum();
            let total_mem_allocatable: f64 = nodes
                .iter()
                .filter_map(|n| parse_k8s_quantity(&n.mem_allocatable))
                .sum();
            let pod_cpu_pct = if total_cpu_allocatable > 0.0 {
                (total_pod_cpu / total_cpu_allocatable) * 100.0
            } else {
                0.0
            };
            let pod_mem_pct = if total_mem_allocatable > 0.0 {
                (total_pod_mem / total_mem_allocatable) * 100.0
            } else {
                0.0
            };
            (pod_cpu_pct, pod_mem_pct)
        })
        .unwrap_or((-1.0, -1.0)); // sentinel: no pod metrics → renders as N/A

    ResourceUsage {
        cpu_percent,
        mem_percent,
        total_pods,
        running_pods,
        succeeded_pods,
        failed_pods,
        total_nodes,
        ready_nodes,
        pod_cpu_percent,
        pod_mem_percent,
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
            ..Default::default()
        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
            containers: vec![],
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
            ..Default::default()
        }];
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
            ..Default::default()
        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Failed".into(),
            ready: "0/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
            containers: vec![],
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
            ..Default::default()
        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "OOMKilled".into(),
            ready: "0/1".into(),
            restarts: 1,
            restarts_display: "1".into(),
            age: "1h".into(),
            node: "n1".into(),
            containers: vec![],
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
            ..Default::default()
        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "ImagePullBackOff".into(),
            ready: "0/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "5m".into(),
            node: "n1".into(),
            containers: vec![],
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
        assert_eq!(
            derive_effective_status("Running", &statuses, &[], None),
            "Running"
        );
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
            ..Default::default()
        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
            containers: vec![],
        }];
        let usage = compute_resource_usage(&nodes, &pods, None, None);
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
            ..Default::default()
        }];
        let metrics = vec![NodeMetrics {
            name: "n1".into(),
            cpu_usage: 2.0,
            mem_usage: 4.0 * 1024.0 * 1024.0 * 1024.0,
        }];
        let usage = compute_resource_usage(&nodes, &[], Some(&metrics), None);
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
        assert_eq!(
            derive_effective_status("Running", &statuses, &[], None),
            "OOMKilled"
        );
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
        use k8s_openapi::api::core::v1::{ContainerState, ContainerStateWaiting, ContainerStatus};

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
            ..Default::default()
        }];
        let pods = vec![PodInfo {
            name: "evicted-pod".into(),
            namespace: "default".into(),
            status: "Evicted".into(),
            ready: "0/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "n1".into(),
            containers: vec![],
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
            containers: vec![],
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
                "crash-1",
                "error-1",
                "oom-1",
                "init-err",
                "pending-1",
                "init-ok",
                "running-1",
                "running-2",
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
            containers: vec![],
        };
        let mut pods = vec![make_pod("b-pod"), make_pod("a-pod"), make_pod("c-pod")];
        sort_pods_by_severity(&mut pods);
        // All same severity → original order preserved (stable sort)
        let names: Vec<&str> = pods.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b-pod", "a-pod", "c-pod"]);
    }

    // ── Sub-AC 7.3: Cross-cluster namespace-scoped resource browsing tests ──

    /// Helper: create a ClusterSnapshot with pods, deployments, and services
    /// spread across multiple namespaces for testing namespace-scoped browsing.
    fn make_multi_ns_snapshot(cluster_name: &str) -> ClusterSnapshot {
        let make_pod = |name: &str, ns: &str, status: &str| PodInfo {
            name: name.into(),
            namespace: ns.into(),
            status: status.into(),
            ready: "1/1".into(),
            restarts: 0,
            restarts_display: "0".into(),
            age: "1h".into(),
            node: "node-0".into(),
            containers: vec!["app".into()],
        };
        let make_deploy = |name: &str, ns: &str| DeploymentInfo {
            name: name.into(),
            namespace: ns.into(),
            ready: "1/1".into(),
            ready_count: 1,
            desired_count: 1,
            up_to_date: 1,
            up_to_date_display: "1".into(),
            available: 1,
            available_display: "1".into(),
            age: "2h".into(),
        };
        let make_svc = |name: &str, ns: &str, stype: &str| ServiceInfo {
            name: name.into(),
            namespace: ns.into(),
            svc_type: stype.into(),
            cluster_ip: "10.96.0.1".into(),
            external_ip: "<none>".into(),
            ports: "80/TCP".into(),
            age: "3h".into(),
        };

        ClusterSnapshot {
            name: cluster_name.into(),
            health: HealthStatus::Green,
            namespaces: vec![
                "default".into(),
                "kube-system".into(),
                "monitoring".into(),
            ],
            nodes: vec![NodeInfo {
                name: "node-0".into(),
                status: "Ready".into(),
                roles: vec!["control-plane".into()],
                cpu_capacity: "8".into(),
                mem_capacity: "16Gi".into(),
                cpu_allocatable: "8".into(),
                mem_allocatable: "16Gi".into(),
                age: "7d".into(),
                ..Default::default()
            }],
            pods: vec![
                make_pod("nginx-abc", "default", "Running"),
                make_pod("coredns-xyz", "kube-system", "Running"),
                make_pod("prometheus-0", "monitoring", "Running"),
            ],
            deployments: vec![
                make_deploy("nginx", "default"),
                make_deploy("coredns", "kube-system"),
                make_deploy("prometheus", "monitoring"),
            ],
            services: vec![
                make_svc("kubernetes", "default", "ClusterIP"),
                make_svc("kube-dns", "kube-system", "ClusterIP"),
                make_svc("prometheus-svc", "monitoring", "ClusterIP"),
            ],
            configmaps: vec![],
            events: vec![],
            resource_usage: ResourceUsage::default(),
        }
    }

    #[test]
    fn filter_snapshot_pods_returns_all_namespaces() {
        let tower = make_multi_ns_snapshot("tower");
        let sandbox = make_multi_ns_snapshot("sandbox");
        let snapshots = vec![tower, sandbox];

        let result = filter_snapshot_by_resource(&snapshots, "pods");
        let clusters = result["clusters"].as_array().unwrap();
        assert_eq!(clusters.len(), 2, "should contain both clusters");

        // Each cluster should have 3 pods (one per namespace)
        for cluster in clusters {
            let pods = cluster["pods"].as_array().unwrap();
            assert_eq!(pods.len(), 3, "each cluster has 3 pods across namespaces");
        }
    }

    #[test]
    fn filter_snapshot_deployments_cross_cluster() {
        let tower = make_multi_ns_snapshot("tower");
        let sandbox = make_multi_ns_snapshot("sandbox");
        let snapshots = vec![tower, sandbox];

        let result = filter_snapshot_by_resource(&snapshots, "deployments");
        let clusters = result["clusters"].as_array().unwrap();

        // Verify cluster names
        let names: Vec<&str> = clusters
            .iter()
            .map(|c| c["cluster"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"tower"), "tower cluster present");
        assert!(names.contains(&"sandbox"), "sandbox cluster present");

        // Verify deployments are present in both clusters
        for cluster in clusters {
            let deploys = cluster["deployments"].as_array().unwrap();
            assert_eq!(deploys.len(), 3);
            let deploy_names: Vec<&str> = deploys
                .iter()
                .map(|d| d["name"].as_str().unwrap())
                .collect();
            assert!(deploy_names.contains(&"nginx"));
            assert!(deploy_names.contains(&"coredns"));
            assert!(deploy_names.contains(&"prometheus"));
        }
    }

    #[test]
    fn filter_snapshot_services_cross_cluster() {
        let tower = make_multi_ns_snapshot("tower");
        let sandbox = make_multi_ns_snapshot("sandbox");
        let snapshots = vec![tower, sandbox];

        let result = filter_snapshot_by_resource(&snapshots, "services");
        let clusters = result["clusters"].as_array().unwrap();
        assert_eq!(clusters.len(), 2);

        for cluster in clusters {
            let svcs = cluster["services"].as_array().unwrap();
            assert_eq!(svcs.len(), 3);
            let svc_namespaces: Vec<&str> = svcs
                .iter()
                .map(|s| s["namespace"].as_str().unwrap())
                .collect();
            assert!(svc_namespaces.contains(&"default"));
            assert!(svc_namespaces.contains(&"kube-system"));
            assert!(svc_namespaces.contains(&"monitoring"));
        }
    }

    #[test]
    fn filter_snapshot_namespaces_cross_cluster() {
        let tower = make_multi_ns_snapshot("tower");
        let sandbox = make_multi_ns_snapshot("sandbox");
        let snapshots = vec![tower, sandbox];

        let result = filter_snapshot_by_resource(&snapshots, "namespaces");
        let clusters = result["clusters"].as_array().unwrap();

        for cluster in clusters {
            let ns = cluster["namespaces"].as_array().unwrap();
            assert_eq!(ns.len(), 3);
            let ns_names: Vec<&str> = ns.iter().map(|n| n.as_str().unwrap()).collect();
            assert!(ns_names.contains(&"default"));
            assert!(ns_names.contains(&"kube-system"));
            assert!(ns_names.contains(&"monitoring"));
        }
    }

    #[test]
    fn filter_snapshot_unknown_resource_returns_error() {
        let snap = make_multi_ns_snapshot("tower");
        let result = filter_snapshot_by_resource(&[snap], "foobar");
        let clusters = result["clusters"].as_array().unwrap();
        assert_eq!(clusters.len(), 1);
        assert!(
            clusters[0]["error"].as_str().unwrap().contains("Unknown"),
            "unknown resource type should return error"
        );
    }

    #[test]
    fn filter_snapshot_health_included_with_resources() {
        let tower = make_multi_ns_snapshot("tower");
        let result = filter_snapshot_by_resource(&[tower], "pods");
        let cluster = &result["clusters"].as_array().unwrap()[0];
        assert_eq!(cluster["health"].as_str().unwrap(), "green");
        assert_eq!(cluster["cluster"].as_str().unwrap(), "tower");
    }

    #[test]
    fn snapshot_pods_have_namespace_field() {
        let snap = make_multi_ns_snapshot("tower");
        // Verify each pod carries its namespace — required for TUI namespace filtering
        for pod in &snap.pods {
            assert!(
                !pod.namespace.is_empty(),
                "pod {} must have namespace field set",
                pod.name
            );
        }
        // Verify namespace diversity
        let unique_ns: std::collections::HashSet<&str> =
            snap.pods.iter().map(|p| p.namespace.as_str()).collect();
        assert!(
            unique_ns.len() >= 2,
            "pods should span multiple namespaces for cross-ns browsing"
        );
    }

    #[test]
    fn snapshot_deployments_have_namespace_field() {
        let snap = make_multi_ns_snapshot("tower");
        for deploy in &snap.deployments {
            assert!(
                !deploy.namespace.is_empty(),
                "deployment {} must have namespace field",
                deploy.name
            );
        }
    }

    #[test]
    fn snapshot_services_have_namespace_field() {
        let snap = make_multi_ns_snapshot("tower");
        for svc in &snap.services {
            assert!(
                !svc.namespace.is_empty(),
                "service {} must have namespace field",
                svc.name
            );
        }
    }

    // --- E2E checks tests ---

    fn make_check_snapshot(
        name: &str,
        nodes_ready: bool,
        with_argocd: bool,
        with_cf_tunnel: bool,
        with_hubble_relay: bool,
        with_kyverno: bool,
    ) -> ClusterSnapshot {
        let nodes = vec![
            NodeInfo {
                name: format!("{}-cp-0", name),
                status: if nodes_ready { "Ready".into() } else { "NotReady".into() },
                roles: vec!["control-plane".into()],
                ..Default::default()
            },
            NodeInfo {
                name: format!("{}-cp-1", name),
                status: "Ready".into(),
                roles: vec!["control-plane".into()],
                ..Default::default()
            },
        ];

        let mut deployments = vec![];
        if with_argocd {
            deployments.push(DeploymentInfo {
                name: "argocd-server".into(),
                namespace: "argocd".into(),
                ready: "1/1".into(),
                ready_count: 1,
                desired_count: 1,
                up_to_date: 1,
                up_to_date_display: "1".into(),
                available: 1,
                available_display: "1".into(),
                age: "1h".into(),
            });
            deployments.push(DeploymentInfo {
                name: "argocd-repo-server".into(),
                namespace: "argocd".into(),
                ready: "1/1".into(),
                ready_count: 1,
                desired_count: 1,
                up_to_date: 1,
                up_to_date_display: "1".into(),
                available: 1,
                available_display: "1".into(),
                age: "1h".into(),
            });
        }

        let mut pods = vec![
            PodInfo {
                name: "coredns-abc".into(),
                namespace: "kube-system".into(),
                status: "Running".into(),
                ready: "1/1".into(),
                restarts: 0,
                restarts_display: "0".into(),
                age: "1d".into(),
                node: format!("{}-cp-0", name),
                containers: vec!["coredns".into()],
            },
        ];
        if with_cf_tunnel {
            pods.push(PodInfo {
                name: "cloudflared-tunnel-xyz".into(),
                namespace: "kube-tunnel".into(),
                status: "Running".into(),
                ready: "1/1".into(),
                restarts: 0,
                restarts_display: "0".into(),
                age: "1h".into(),
                node: format!("{}-cp-0", name),
                containers: vec!["cloudflared".into()],
            });
        }
        if with_hubble_relay {
            pods.push(PodInfo {
                name: format!("hubble-relay-{}-abc", name),
                namespace: "kube-system".into(),
                status: "Running".into(),
                ready: "1/1".into(),
                restarts: 0,
                restarts_display: "0".into(),
                age: "1h".into(),
                node: format!("{}-cp-0", name),
                containers: vec!["hubble-relay".into()],
            });
        }
        if with_kyverno {
            pods.push(PodInfo {
                name: "kyverno-admission-controller-xyz".into(),
                namespace: "kyverno".into(),
                status: "Running".into(),
                ready: "1/1".into(),
                restarts: 0,
                restarts_display: "0".into(),
                age: "1h".into(),
                node: format!("{}-cp-0", name),
                containers: vec!["kyverno".into()],
            });
        }

        ClusterSnapshot {
            name: name.into(),
            health: if nodes_ready { HealthStatus::Green } else { HealthStatus::Red },
            namespaces: vec!["default".into(), "kube-system".into(), "argocd".into()],
            nodes,
            pods,
            deployments,
            services: vec![],
            configmaps: vec![],
            events: vec![],
            resource_usage: ResourceUsage::default(),
        }
    }

    #[test]
    fn e2e_checks_all_pass() {
        // All 7 checks: both clusters have hubble-relay and kyverno deployed and Running
        let tower = make_check_snapshot("tower", true, true, true, true, true);
        let sandbox = make_check_snapshot("sandbox", true, false, false, true, true);
        let snapshots = vec![tower, sandbox];
        let report = run_e2e_checks(&snapshots, &["tower", "sandbox"], &Default::default());
        assert_eq!(report.overall, CheckStatus::Pass);
        assert_eq!(report.passed, 7);
        assert_eq!(report.failed, 0);
        assert_eq!(report.total, 7);
        for check in &report.checks {
            assert_eq!(
                check.status, CheckStatus::Pass,
                "check '{}' should pass: {}",
                check.name, check.message
            );
        }
    }

    #[test]
    fn e2e_checks_cluster_unreachable() {
        // Only tower available, sandbox expected but missing
        let tower = make_check_snapshot("tower", true, true, true, true, true);
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower", "sandbox"], &Default::default());
        assert_eq!(report.overall, CheckStatus::Fail);
        let api_check = &report.checks[0];
        assert_eq!(api_check.name, "cluster_api_reachable");
        assert_eq!(api_check.status, CheckStatus::Fail);
    }

    #[test]
    fn e2e_checks_nodes_not_ready() {
        let tower = make_check_snapshot("tower", false, true, true, true, true);
        let sandbox = make_check_snapshot("sandbox", true, false, false, true, true);
        let snapshots = vec![tower, sandbox];
        let report = run_e2e_checks(&snapshots, &["tower", "sandbox"], &Default::default());
        let node_check = &report.checks[1];
        assert_eq!(node_check.name, "all_nodes_ready");
        assert_eq!(node_check.status, CheckStatus::Fail);
    }

    #[test]
    fn e2e_checks_no_argocd() {
        let tower = make_check_snapshot("tower", true, false, true, true, true);
        let sandbox = make_check_snapshot("sandbox", true, false, false, true, true);
        let snapshots = vec![tower, sandbox];
        let report = run_e2e_checks(&snapshots, &["tower", "sandbox"], &Default::default());
        let argocd_check = &report.checks[3];
        assert_eq!(argocd_check.name, "argocd_synced");
        assert_eq!(argocd_check.status, CheckStatus::Fail);
    }

    #[test]
    fn e2e_checks_no_cf_tunnel() {
        let tower = make_check_snapshot("tower", true, true, false, true, true);
        let sandbox = make_check_snapshot("sandbox", true, false, false, true, true);
        let snapshots = vec![tower, sandbox];
        let report = run_e2e_checks(&snapshots, &["tower", "sandbox"], &Default::default());
        let cf_check = &report.checks[4];
        assert_eq!(cf_check.name, "cf_tunnel_running");
        assert_eq!(cf_check.status, CheckStatus::Fail);
    }

    #[test]
    fn e2e_checks_serializes_to_json() {
        let tower = make_check_snapshot("tower", true, true, true, true, true);
        let sandbox = make_check_snapshot("sandbox", true, false, false, true, true);
        let snapshots = vec![tower, sandbox];
        let report = run_e2e_checks(&snapshots, &["tower", "sandbox"], &Default::default());
        let json = serde_json::to_string_pretty(&report).unwrap();
        // Verify it's valid JSON with expected fields
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["overall"].as_str().unwrap(), "pass");
        assert_eq!(parsed["passed"].as_u64().unwrap(), 7);
        assert_eq!(parsed["total"].as_u64().unwrap(), 7);
        let checks = parsed["checks"].as_array().unwrap();
        assert_eq!(checks.len(), 7);
        // Verify check names in order
        let names: Vec<&str> = checks.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "cluster_api_reachable",
                "all_nodes_ready",
                "namespaces_listed",
                "argocd_synced",
                "cf_tunnel_running",
                "cilium_healthy",
                "kyverno_healthy",
            ]
        );
    }

    #[test]
    fn e2e_checks_empty_namespaces_fails() {
        let mut tower = make_check_snapshot("tower", true, true, true, true, true);
        tower.namespaces.clear();
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower"], &Default::default());
        let ns_check = &report.checks[2];
        assert_eq!(ns_check.name, "namespaces_listed");
        assert_eq!(ns_check.status, CheckStatus::Fail);
    }

    #[test]
    fn e2e_checks_tower_missing_skips_argocd_and_cf() {
        // Only sandbox available — ArgoCD and CF Tunnel checks should skip (tower-only)
        let sandbox = make_check_snapshot("sandbox", true, false, false, true, true);
        let snapshots = vec![sandbox];
        let report = run_e2e_checks(&snapshots, &["sandbox"], &Default::default());
        let argocd_check = &report.checks[3];
        assert_eq!(argocd_check.status, CheckStatus::Skip);
        let cf_check = &report.checks[4];
        assert_eq!(cf_check.status, CheckStatus::Skip);
    }

    #[test]
    fn e2e_checks_cilium_healthy_passes_when_hubble_relay_running() {
        let tower = make_check_snapshot("tower", true, false, false, true, false);
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower"], &Default::default());
        let cilium_check = &report.checks[5];
        assert_eq!(cilium_check.name, "cilium_healthy");
        assert_eq!(cilium_check.status, CheckStatus::Pass);
        assert!(cilium_check.message.contains("healthy"));
    }

    #[test]
    fn e2e_checks_cilium_healthy_fails_when_hubble_relay_crashloop() {
        let mut tower = make_check_snapshot("tower", true, false, false, false, false);
        // Add a CrashLoopBackOff hubble-relay pod (DNS timeout regression)
        tower.pods.push(PodInfo {
            name: "hubble-relay-crashloop".into(),
            namespace: "kube-system".into(),
            status: "CrashLoopBackOff".into(),
            ready: "0/1".into(),
            restarts: 5,
            restarts_display: "5".into(),
            age: "10m".into(),
            node: "tower-cp-0".into(),
            containers: vec!["hubble-relay".into()],
        });
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower"], &Default::default());
        let cilium_check = &report.checks[5];
        assert_eq!(cilium_check.name, "cilium_healthy");
        assert_eq!(cilium_check.status, CheckStatus::Fail);
        assert!(cilium_check.message.contains("DNS timeout") || cilium_check.message.contains("not Running"));
    }

    #[test]
    fn e2e_checks_cilium_skips_when_hubble_not_deployed() {
        // No hubble-relay pods → check should Skip, not Fail
        let tower = make_check_snapshot("tower", true, false, false, false, false);
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower"], &Default::default());
        let cilium_check = &report.checks[5];
        assert_eq!(cilium_check.name, "cilium_healthy");
        assert_eq!(cilium_check.status, CheckStatus::Skip);
    }

    #[test]
    fn e2e_checks_kyverno_healthy_passes_when_admission_controller_running() {
        let tower = make_check_snapshot("tower", true, false, false, false, true);
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower"], &Default::default());
        let kyverno_check = &report.checks[6];
        assert_eq!(kyverno_check.name, "kyverno_healthy");
        assert_eq!(kyverno_check.status, CheckStatus::Pass);
        assert!(kyverno_check.message.contains("healthy"));
    }

    #[test]
    fn e2e_checks_kyverno_healthy_fails_when_pods_not_running() {
        let mut tower = make_check_snapshot("tower", true, false, false, false, false);
        // Add a non-running kyverno pod
        tower.pods.push(PodInfo {
            name: "kyverno-admission-controller-xyz".into(),
            namespace: "kyverno".into(),
            status: "CrashLoopBackOff".into(),
            ready: "0/1".into(),
            restarts: 3,
            restarts_display: "3".into(),
            age: "5m".into(),
            node: "tower-cp-0".into(),
            containers: vec!["kyverno".into()],
        });
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower"], &Default::default());
        let kyverno_check = &report.checks[6];
        assert_eq!(kyverno_check.name, "kyverno_healthy");
        assert_eq!(kyverno_check.status, CheckStatus::Fail);
    }

    #[test]
    fn e2e_checks_kyverno_skips_when_not_deployed() {
        // No kyverno pods → check should Skip, not Fail
        let tower = make_check_snapshot("tower", true, false, false, false, false);
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower"], &Default::default());
        let kyverno_check = &report.checks[6];
        assert_eq!(kyverno_check.name, "kyverno_healthy");
        assert_eq!(kyverno_check.status, CheckStatus::Skip);
    }

    // ---------------------------------------------------------------------------
    // Known-degradation suppression tests
    // ---------------------------------------------------------------------------

    #[test]
    fn e2e_checks_known_degraded_suppresses_fail() {
        use crate::models::degradation::{KnownDegradation, KnownDegradationsConfig};

        // Create a cluster where argocd_synced would normally Fail
        // (no argocd deployments in the snapshot), but cf_tunnel=true so only argocd fails.
        let tower = make_check_snapshot("tower", true, false, true, true, true);
        let snapshots = vec![tower];

        // Inventory entry that suppresses argocd_synced
        let inv = KnownDegradationsConfig {
            known_degradations: vec![KnownDegradation {
                namespace: "argocd".to_string(),
                resource_kind: "Pod".to_string(),
                name: "argocd-dex-server-*".to_string(),
                condition: "CrashLoopBackOff".to_string(),
                cause_kind: "architectural-assumption".to_string(),
                reason: "OIDC not wired in this environment.".to_string(),
                acknowledged_by: "jinwang".to_string(),
                ticket: "N/A".to_string(),
                suppresses_check: vec!["argocd_synced".to_string()],
            }],
        };

        let report = run_e2e_checks(&snapshots, &["tower"], &inv);
        let argocd_check = report.checks.iter().find(|c| c.name == "argocd_synced").unwrap();

        // Status must be KnownDegraded, not Fail
        assert_eq!(
            argocd_check.status,
            CheckStatus::KnownDegraded,
            "argocd_synced should be KnownDegraded when suppressed by inventory"
        );
        assert!(
            argocd_check.message.contains("[known-degraded:"),
            "message must include [known-degraded:] marker; got: {}",
            argocd_check.message
        );

        // No real failures → overall should not be Fail
        assert_ne!(
            report.overall,
            CheckStatus::Fail,
            "overall should not be Fail when only known-degraded items remain"
        );
        assert_eq!(report.known_degraded, 1);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn e2e_checks_without_inventory_still_fails() {
        // Same scenario without inventory → should remain Fail
        let tower = make_check_snapshot("tower", true, false, true, true, true);
        let snapshots = vec![tower];
        let report = run_e2e_checks(&snapshots, &["tower"], &Default::default());
        let argocd_check = report.checks.iter().find(|c| c.name == "argocd_synced").unwrap();
        assert_eq!(
            argocd_check.status,
            CheckStatus::Fail,
            "argocd_synced should Fail when no inventory suppresses it"
        );
        assert_eq!(report.failed, 1);
        assert_eq!(report.known_degraded, 0);
    }

    // ---------------------------------------------------------------------------
    // Metrics parsing and compute_resource_usage integration tests
    // ---------------------------------------------------------------------------

    #[test]
    fn node_metrics_cpu_nanocores_parse() {
        // metrics-server reports CPU in nanocores (e.g., "250000000n" = 0.25 cores)
        let cpu = parse_k8s_quantity("250000000n");
        assert!(cpu.is_some());
        assert!((cpu.unwrap() - 0.25).abs() < 1e-6, "250000000n should parse to 0.25 cores");
    }

    #[test]
    fn node_metrics_cpu_millicores_parse() {
        // metrics-server also reports CPU in millicores (e.g., "125m" = 0.125 cores)
        let cpu = parse_k8s_quantity("125m");
        assert!(cpu.is_some());
        assert!((cpu.unwrap() - 0.125).abs() < 1e-9, "125m should parse to 0.125 cores");
    }

    #[test]
    fn node_metrics_memory_ki_parse() {
        // metrics-server reports memory in Ki (e.g., "1048576Ki" = 1Gi)
        let mem = parse_k8s_quantity("1048576Ki");
        assert!(mem.is_some());
        let gib = 1024.0 * 1024.0 * 1024.0;
        assert!((mem.unwrap() - gib).abs() < 1.0, "1048576Ki should parse to 1GiB");
    }

    #[test]
    fn compute_resource_usage_with_node_metrics_shows_real_percent() {
        // When node metrics are available, cpu_percent and mem_percent must be ≥ 0.0
        // (not the -1.0 sentinel which triggers "N/A" display in the status bar)
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            cpu_capacity: "4".into(),         // 4 cores
            mem_capacity: "8589934592".into(), // 8GiB in bytes
            cpu_allocatable: "4".into(),
            mem_allocatable: "8589934592".into(),
            age: "1d".into(),
            ..Default::default()
        }];
        let metrics = vec![NodeMetrics {
            name: "n1".into(),
            cpu_usage: 1.0,                       // 1 core = 25%
            mem_usage: 2.0 * 1024.0 * 1024.0 * 1024.0, // 2GiB = 25%
        }];
        let usage = compute_resource_usage(&nodes, &[], Some(&metrics), None);
        assert!(
            usage.cpu_percent >= 0.0,
            "cpu_percent must not be -1.0 sentinel when metrics are available"
        );
        assert!(
            (usage.cpu_percent - 25.0).abs() < 1e-3,
            "cpu_percent should be ~25% (1/4 cores)"
        );
        assert!(
            usage.mem_percent >= 0.0,
            "mem_percent must not be -1.0 sentinel when metrics are available"
        );
        assert!(
            (usage.mem_percent - 25.0).abs() < 1e-3,
            "mem_percent should be ~25% (2/8 GiB)"
        );
    }

    #[test]
    fn compute_resource_usage_no_metrics_shows_sentinel() {
        // When no node metrics, cpu_percent/mem_percent must be -1.0 (sentinel → "N/A" in UI)
        let nodes = vec![NodeInfo {
            name: "n1".into(),
            status: "Ready".into(),
            cpu_capacity: "4".into(),
            mem_capacity: "8Gi".into(),
            cpu_allocatable: "4".into(),
            mem_allocatable: "8Gi".into(),
            age: "1d".into(),
            ..Default::default()
        }];
        let usage = compute_resource_usage(&nodes, &[], None, None);
        assert!(
            (usage.cpu_percent - (-1.0)).abs() < 1e-9,
            "cpu_percent must be -1.0 sentinel when no metrics"
        );
        assert!(
            (usage.mem_percent - (-1.0)).abs() < 1e-9,
            "mem_percent must be -1.0 sentinel when no metrics"
        );
    }

    #[test]
    fn pod_metrics_parse_k8s_quantities_no_panic() {
        // Verify parse_k8s_quantity handles all quantity formats that metrics-server may return
        // without panicking or returning garbage (ensures "no parse errors" AC)
        let test_cases = [
            // nanocores (most common from metrics-server)
            ("100000000n", Some(0.1f64)),
            ("500000000n", Some(0.5f64)),
            ("1000000000n", Some(1.0f64)),
            // millicores
            ("250m", Some(0.25f64)),
            ("1500m", Some(1.5f64)),
            // whole cores
            ("2", Some(2.0f64)),
            // memory Ki
            ("524288Ki", Some(536870912.0f64)), // 512Mi
            // memory Mi
            ("512Mi", Some(536870912.0f64)),
            // memory Gi
            ("4Gi", Some(4294967296.0f64)),
            // empty → None (not a panic)
            ("", None),
            // unparseable → None (not a panic)
            ("invalid", None),
        ];
        for (input, expected) in &test_cases {
            let result = parse_k8s_quantity(input);
            match expected {
                Some(exp) => {
                    assert!(
                        result.is_some(),
                        "parse_k8s_quantity({:?}) returned None, expected Some({})",
                        input, exp
                    );
                    assert!(
                        (result.unwrap() - exp).abs() < exp.abs() * 1e-4 + 1.0,
                        "parse_k8s_quantity({:?}) = {:?}, expected ~{}",
                        input,
                        result,
                        exp
                    );
                }
                None => {
                    assert!(
                        result.is_none(),
                        "parse_k8s_quantity({:?}) returned {:?}, expected None",
                        input,
                        result
                    );
                }
            }
        }
    }

    #[test]
    fn format_k8s_memory_no_parse_errors_on_metrics_output() {
        // Verify format_k8s_memory handles typical metrics-server outputs without producing
        // garbled output or panicking (ensures "no parse errors" AC for memory display)
        let cases = [
            ("7816040Ki", "7.5Gi"),     // Node memory (Ki → Gi)
            ("8Gi", "8Gi"),             // Already in Gi
            ("512Mi", "512Mi"),         // Already in Mi
            ("1073741824", "1Gi"),      // Plain bytes → Gi
            ("0Ki", "0B"),              // Zero bytes
            ("", "N/A"),                // Empty → explicit N/A (not blank)
        ];
        for (input, expected) in &cases {
            let result = format_k8s_memory(input);
            assert_eq!(
                result, *expected,
                "format_k8s_memory({:?}) = {:?}, expected {:?}",
                input, result, expected
            );
        }
    }
}
