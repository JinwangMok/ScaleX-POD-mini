use anyhow::Result;
use chrono::Utc;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Node, Pod, Service};
use kube::api::ListParams;
use kube::{Api, Client};
use serde::Serialize;
use std::time::Duration;

/// Which resource type the TUI is currently displaying.
/// Used to selectively fetch only what's needed (reduces 7 API calls → 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveResource {
    Pods,
    Deployments,
    Services,
    ConfigMaps,
    Nodes,
}

/// Per-API-call timeout to prevent slow calls from blocking the entire fetch.
const API_CALL_TIMEOUT: Duration = Duration::from_secs(3);

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
    pub resource_usage: ResourceUsage,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Green,
    Yellow,
    Red,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub name: String,
    pub status: String,
    pub roles: Vec<String>,
    pub cpu_capacity: String,
    pub mem_capacity: String,
    pub cpu_allocatable: String,
    pub mem_allocatable: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PodInfo {
    pub name: String,
    pub namespace: String,
    pub status: String,
    pub ready: String,
    pub restarts: i32,
    pub age: String,
    pub node: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeploymentInfo {
    pub name: String,
    pub namespace: String,
    pub ready: String,
    pub up_to_date: i32,
    pub available: i32,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub name: String,
    pub namespace: String,
    pub svc_type: String,
    pub cluster_ip: String,
    pub ports: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigMapInfo {
    pub name: String,
    pub namespace: String,
    pub data_keys_count: usize,
    pub age: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ResourceUsage {
    pub cpu_percent: f64,
    pub mem_percent: f64,
    pub total_pods: usize,
    pub running_pods: usize,
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

            let container_statuses = status
                .map(|s| s.container_statuses.clone().unwrap_or_default())
                .unwrap_or_default();

            // Derive effective status: check container waiting reasons
            // (e.g., CrashLoopBackOff shows phase=Running but container is waiting)
            let effective_status = derive_effective_status(&phase, &container_statuses);

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
            let node_status = conditions
                .iter()
                .find(|c| c.type_ == "Ready")
                .map(|c| {
                    if c.status == "True" {
                        "Ready"
                    } else {
                        "NotReady"
                    }
                })
                .unwrap_or("Unknown")
                .to_string();

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

            let _ = spec; // suppress unused warning

            NodeInfo {
                name: meta.name.clone().unwrap_or_default(),
                status: node_status,
                roles,
                cpu_capacity: capacity
                    .and_then(|c| c.get("cpu"))
                    .map(|v| v.0.clone())
                    .unwrap_or_default(),
                mem_capacity: capacity
                    .and_then(|c| c.get("memory"))
                    .map(|v| v.0.clone())
                    .unwrap_or_default(),
                cpu_allocatable: allocatable
                    .and_then(|a| a.get("cpu"))
                    .map(|v| v.0.clone())
                    .unwrap_or_default(),
                mem_allocatable: allocatable
                    .and_then(|a| a.get("memory"))
                    .map(|v| v.0.clone())
                    .unwrap_or_default(),
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
                up_to_date,
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
                                    format!("{}/{}", p.port, proto)
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

            ServiceInfo {
                name: meta.name.clone().unwrap_or_default(),
                namespace: meta.namespace.clone().unwrap_or_default(),
                svc_type,
                cluster_ip,
                ports,
                age,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Cluster snapshot (aggregate)
// ---------------------------------------------------------------------------

/// Fetch cluster snapshot with optional selective resource filtering.
///
/// - `active_resource: None` → fetch ALL resources (used by headless mode)
/// - `active_resource: Some(r)` → fetch only namespaces + nodes + the active resource
///   (reduces 7 API calls → 3, cutting latency from ~6s to <1s)
///
/// Metrics fetch is removed (metrics_server_enabled hardcoded false).
pub async fn fetch_cluster_snapshot(
    client: &Client,
    cluster_name: &str,
    namespace: Option<&str>,
    active_resource: Option<ActiveResource>,
) -> Result<ClusterSnapshot> {
    // Always fetch namespaces + nodes (required for sidebar tree + health/status bar)
    let (namespaces, nodes) = tokio::join!(
        async { fetch_namespaces(client).await.unwrap_or_default() },
        async { fetch_nodes(client).await.unwrap_or_default() },
    );

    // Selectively fetch only the active resource type
    let (pods, deployments, services, configmaps) = match active_resource {
        None => {
            // Headless / full fetch: get everything in parallel
            let (p, d, s, c) = tokio::join!(
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
            );
            (Some(p), Some(d), Some(s), Some(c))
        }
        Some(ActiveResource::Pods) => {
            let p = fetch_pods(client, namespace).await.unwrap_or_default();
            (Some(p), None, None, None)
        }
        Some(ActiveResource::Deployments) => {
            let d = fetch_deployments(client, namespace)
                .await
                .unwrap_or_default();
            (None, Some(d), None, None)
        }
        Some(ActiveResource::Services) => {
            let s = fetch_services(client, namespace).await.unwrap_or_default();
            (None, None, Some(s), None)
        }
        Some(ActiveResource::ConfigMaps) => {
            let c = fetch_configmaps(client, namespace)
                .await
                .unwrap_or_default();
            (None, None, None, Some(c))
        }
        Some(ActiveResource::Nodes) => {
            // Nodes already fetched above, no extra API call needed
            (None, None, None, None)
        }
    };

    // For health computation, use pods if we have them, otherwise empty
    // (health will be recomputed on next full fetch)
    let pods_for_health = pods.as_deref().unwrap_or(&[]);
    let health = compute_health(&nodes, pods_for_health);
    let resource_usage = compute_resource_usage(&nodes, pods_for_health, None);

    Ok(ClusterSnapshot {
        name: cluster_name.to_string(),
        health,
        namespaces,
        nodes,
        pods: pods.unwrap_or_default(),
        deployments: deployments.unwrap_or_default(),
        services: services.unwrap_or_default(),
        configmaps: configmaps.unwrap_or_default(),
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
    let not_ready_nodes = nodes.iter().filter(|n| n.status != "Ready").count();
    let failed_pods = pods
        .iter()
        .filter(|p| matches!(p.status.as_str(), "Failed" | "CrashLoopBackOff" | "Error"))
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
    let ready_nodes = nodes.iter().filter(|n| n.status == "Ready").count();
    let total_pods = pods.len();
    let running_pods = pods.iter().filter(|p| p.status == "Running").count();
    let failed_pods = pods
        .iter()
        .filter(|p| matches!(p.status.as_str(), "Failed" | "CrashLoopBackOff" | "Error"))
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
        .unwrap_or((0.0, 0.0));

    ResourceUsage {
        cpu_percent,
        mem_percent,
        total_pods,
        running_pods,
        failed_pods,
        total_nodes,
        ready_nodes,
    }
}

/// Derive effective pod status by checking container waiting reasons.
/// K8s reports phase=Running even when containers are in CrashLoopBackOff.
fn derive_effective_status(
    phase: &str,
    container_statuses: &[k8s_openapi::api::core::v1::ContainerStatus],
) -> String {
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

fn format_age(now: chrono::DateTime<Utc>, created: chrono::DateTime<Utc>) -> String {
    let duration = now.signed_duration_since(created);
    let secs = duration.num_seconds();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
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
        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
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
        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Failed".into(),
            ready: "0/1".into(),
            restarts: 0,
            age: "1h".into(),
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
    fn derive_status_returns_phase_when_no_waiting() {
        let statuses = vec![];
        assert_eq!(derive_effective_status("Running", &statuses), "Running");
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
            derive_effective_status("Running", &statuses),
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
    fn configmap_info_serialization() {
        let cm = ConfigMapInfo {
            name: "test-cm".into(),
            namespace: "default".into(),
            data_keys_count: 3,
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
        }];
        let pods = vec![PodInfo {
            name: "p1".into(),
            namespace: "default".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
            age: "1h".into(),
            node: "n1".into(),
        }];
        let usage = compute_resource_usage(&nodes, &pods, None);
        assert!((usage.cpu_percent - 0.0).abs() < 1e-9);
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
        }];
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
        assert_eq!(derive_effective_status("Running", &statuses), "OOMKilled");
    }
}
