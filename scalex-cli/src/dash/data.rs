use anyhow::Result;
use chrono::Utc;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Node, Pod, Service};
use kube::api::ListParams;
use kube::{Api, Client};
use serde::Serialize;

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
    let ns_list = api.list(&ListParams::default()).await?;
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
    let pod_list = api.list(&ListParams::default()).await?;
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

            let ready_count = container_statuses.iter().filter(|c| c.ready).count();
            let total_count = container_statuses.len();
            let restarts: i32 = container_statuses
                .iter()
                .map(|c| c.restart_count)
                .sum();

            let age = meta
                .creation_timestamp
                .as_ref()
                .map(|ts| format_age(now, ts.0))
                .unwrap_or_else(|| "<unknown>".into());

            PodInfo {
                name: meta.name.clone().unwrap_or_default(),
                namespace: meta.namespace.clone().unwrap_or_default(),
                status: phase,
                ready: format!("{}/{}", ready_count, total_count),
                restarts,
                age,
                node: spec
                    .and_then(|s| s.node_name.clone())
                    .unwrap_or_default(),
            }
        })
        .collect())
}

pub async fn fetch_nodes(client: &Client) -> Result<Vec<NodeInfo>> {
    let api: Api<Node> = Api::all(client.clone());
    let node_list = api.list(&ListParams::default()).await?;

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
    let dep_list = api.list(&ListParams::default()).await?;
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

pub async fn fetch_services(
    client: &Client,
    namespace: Option<&str>,
) -> Result<Vec<ServiceInfo>> {
    let api: Api<Service> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let svc_list = api.list(&ListParams::default()).await?;
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

pub async fn fetch_cluster_snapshot(
    client: &Client,
    cluster_name: &str,
    namespace: Option<&str>,
) -> Result<ClusterSnapshot> {
    let namespaces = fetch_namespaces(client).await.unwrap_or_default();
    let nodes = fetch_nodes(client).await.unwrap_or_default();
    let pods = fetch_pods(client, namespace).await.unwrap_or_default();
    let deployments = fetch_deployments(client, namespace).await.unwrap_or_default();
    let services = fetch_services(client, namespace).await.unwrap_or_default();

    let health = compute_health(&nodes, &pods);
    let resource_usage = compute_resource_usage(&nodes, &pods);

    Ok(ClusterSnapshot {
        name: cluster_name.to_string(),
        health,
        namespaces,
        nodes,
        pods,
        deployments,
        services,
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
                _ => {
                    obj["error"] = serde_json::json!(format!("Unknown resource type: {}", resource));
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

pub fn compute_resource_usage(nodes: &[NodeInfo], pods: &[PodInfo]) -> ResourceUsage {
    let total_nodes = nodes.len();
    let ready_nodes = nodes.iter().filter(|n| n.status == "Ready").count();
    let total_pods = pods.len();
    let running_pods = pods.iter().filter(|p| p.status == "Running").count();
    let failed_pods = pods
        .iter()
        .filter(|p| matches!(p.status.as_str(), "Failed" | "CrashLoopBackOff" | "Error"))
        .count();

    ResourceUsage {
        cpu_percent: 0.0, // metrics-server data when available
        mem_percent: 0.0,
        total_pods,
        running_pods,
        failed_pods,
        total_nodes,
        ready_nodes,
    }
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
}
