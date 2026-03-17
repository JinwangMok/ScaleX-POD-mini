//! Container port auto-detection for port-forward feature.
//!
//! Extracts containerPort entries from pod specs and targetPort/port
//! entries from service specs via kube-rs typed API.

use anyhow::Result;
use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::ListParams;
use kube::{Api, Client};
use std::time::Duration;

/// Timeout for port detection API calls (same as data.rs API_CALL_TIMEOUT).
const PORT_DETECT_TIMEOUT: Duration = Duration::from_millis(500);

/// A detected container port from a pod spec.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContainerPort {
    /// Pod name
    pub pod_name: String,
    /// Pod namespace
    pub namespace: String,
    /// Container name within the pod
    pub container_name: String,
    /// The container port number
    pub container_port: u16,
    /// Protocol (TCP, UDP, SCTP)
    pub protocol: String,
    /// Optional port name (e.g., "http", "metrics")
    pub port_name: Option<String>,
}

/// A detected service port from a service spec.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServicePort {
    /// Service name
    pub service_name: String,
    /// Service namespace
    pub namespace: String,
    /// The service port number (what clients connect to)
    pub port: u16,
    /// The target port (what the service routes to on pods).
    /// Can be a number or a named port reference.
    pub target_port: TargetPort,
    /// Protocol (TCP, UDP, SCTP)
    pub protocol: String,
    /// Optional port name
    pub port_name: Option<String>,
    /// NodePort if service type is NodePort/LoadBalancer
    pub node_port: Option<u16>,
}

/// Target port: either a numeric port or a named port reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetPort {
    Number(u16),
    Name(String),
}

impl std::fmt::Display for TargetPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetPort::Number(n) => write!(f, "{}", n),
            TargetPort::Name(s) => write!(f, "{}", s),
        }
    }
}

/// Extract all containerPort entries from a single pod's spec.
pub fn extract_pod_ports(pod: &Pod) -> Vec<ContainerPort> {
    let meta = &pod.metadata;
    let pod_name = meta.name.clone().unwrap_or_default();
    let namespace = meta.namespace.clone().unwrap_or_default();

    let spec = match pod.spec.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };

    let mut ports = Vec::new();

    // Extract from init containers
    if let Some(init_containers) = &spec.init_containers {
        for container in init_containers {
            if let Some(container_ports) = &container.ports {
                for cp in container_ports {
                    if let Some(port_num) = u16::try_from(cp.container_port).ok() {
                        ports.push(ContainerPort {
                            pod_name: pod_name.clone(),
                            namespace: namespace.clone(),
                            container_name: container.name.clone(),
                            container_port: port_num,
                            protocol: cp.protocol.clone().unwrap_or_else(|| "TCP".into()),
                            port_name: cp.name.clone(),
                        });
                    }
                }
            }
        }
    }

    // Extract from regular containers
    for container in &spec.containers {
        if let Some(container_ports) = &container.ports {
            for cp in container_ports {
                if let Some(port_num) = u16::try_from(cp.container_port).ok() {
                    ports.push(ContainerPort {
                        pod_name: pod_name.clone(),
                        namespace: namespace.clone(),
                        container_name: container.name.clone(),
                        container_port: port_num,
                        protocol: cp.protocol.clone().unwrap_or_else(|| "TCP".into()),
                        port_name: cp.name.clone(),
                    });
                }
            }
        }
    }

    ports
}

/// Extract all port entries from a single service's spec.
pub fn extract_service_ports(svc: &Service) -> Vec<ServicePort> {
    let meta = &svc.metadata;
    let svc_name = meta.name.clone().unwrap_or_default();
    let namespace = meta.namespace.clone().unwrap_or_default();

    let spec = match svc.spec.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };

    let svc_ports = match &spec.ports {
        Some(p) => p,
        None => return Vec::new(),
    };

    svc_ports
        .iter()
        .filter_map(|sp| {
            let port = u16::try_from(sp.port).ok()?;
            let target_port = sp
                .target_port
                .as_ref()
                .map(|tp| match tp {
                    IntOrString::Int(n) => {
                        TargetPort::Number(u16::try_from(*n).unwrap_or(*n as u16))
                    }
                    IntOrString::String(s) => TargetPort::Name(s.clone()),
                })
                .unwrap_or(TargetPort::Number(port)); // default: targetPort = port
            let node_port = sp.node_port.and_then(|np| u16::try_from(np).ok());

            Some(ServicePort {
                service_name: svc_name.clone(),
                namespace: namespace.clone(),
                port,
                target_port,
                protocol: sp.protocol.clone().unwrap_or_else(|| "TCP".into()),
                port_name: sp.name.clone(),
                node_port,
            })
        })
        .collect()
}

/// Fetch all container ports for a specific pod by name.
pub async fn detect_pod_ports(
    client: &Client,
    namespace: &str,
    pod_name: &str,
) -> Result<Vec<ContainerPort>> {
    let api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let pod = tokio::time::timeout(PORT_DETECT_TIMEOUT, api.get(pod_name))
        .await
        .map_err(|_| anyhow::anyhow!("pod get timeout for port detection"))??;
    Ok(extract_pod_ports(&pod))
}

/// Fetch all ports for a specific service by name.
pub async fn detect_service_ports(
    client: &Client,
    namespace: &str,
    service_name: &str,
) -> Result<Vec<ServicePort>> {
    let api: Api<Service> = Api::namespaced(client.clone(), namespace);
    let svc = tokio::time::timeout(PORT_DETECT_TIMEOUT, api.get(service_name))
        .await
        .map_err(|_| anyhow::anyhow!("service get timeout for port detection"))??;
    Ok(extract_service_ports(&svc))
}

/// Fetch all container ports across pods in a namespace (or all namespaces).
pub async fn detect_all_pod_ports(
    client: &Client,
    namespace: Option<&str>,
) -> Result<Vec<ContainerPort>> {
    let api: Api<Pod> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let pod_list = tokio::time::timeout(PORT_DETECT_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("pod list timeout for port detection"))??;

    Ok(pod_list.items.iter().flat_map(extract_pod_ports).collect())
}

/// Fetch all service ports across services in a namespace (or all namespaces).
pub async fn detect_all_service_ports(
    client: &Client,
    namespace: Option<&str>,
) -> Result<Vec<ServicePort>> {
    let api: Api<Service> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let svc_list = tokio::time::timeout(PORT_DETECT_TIMEOUT, api.list(&ListParams::default()))
        .await
        .map_err(|_| anyhow::anyhow!("service list timeout for port detection"))??;

    Ok(svc_list
        .items
        .iter()
        .flat_map(extract_service_ports)
        .collect())
}

/// Resolve a named target port to a numeric port by looking up the port name
/// in the pod's container port definitions.
pub fn resolve_named_port(pod: &Pod, port_name: &str) -> Option<u16> {
    let spec = pod.spec.as_ref()?;
    for container in &spec.containers {
        if let Some(ports) = &container.ports {
            for cp in ports {
                if cp.name.as_deref() == Some(port_name) {
                    return u16::try_from(cp.container_port).ok();
                }
            }
        }
    }
    // Also check init containers
    if let Some(init_containers) = &spec.init_containers {
        for container in init_containers {
            if let Some(ports) = &container.ports {
                for cp in ports {
                    if cp.name.as_deref() == Some(port_name) {
                        return u16::try_from(cp.container_port).ok();
                    }
                }
            }
        }
    }
    None
}

/// Format a ContainerPort for display (e.g., "8080/TCP (http)").
impl std::fmt::Display for ContainerPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.port_name {
            Some(name) => write!(f, "{}/{} ({})", self.container_port, self.protocol, name),
            None => write!(f, "{}/{}", self.container_port, self.protocol),
        }
    }
}

/// Format a ServicePort for display (e.g., "80:8080/TCP (http)").
impl std::fmt::Display for ServicePort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let target = match &self.target_port {
            TargetPort::Number(n) if *n == self.port => String::new(),
            tp => format!("→{}", tp),
        };
        let node = match self.node_port {
            Some(np) => format!(":{}", np),
            None => String::new(),
        };
        match &self.port_name {
            Some(name) => write!(f, "{}{}{}/{} ({})", self.port, target, node, self.protocol, name),
            None => write!(f, "{}{}{}/{}", self.port, target, node, self.protocol),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{
        Container, ContainerPort as K8sContainerPort, Pod, PodSpec, Service, ServicePort as K8sServicePort,
        ServiceSpec,
    };
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    use kube::api::ObjectMeta;

    fn make_pod(name: &str, ns: &str, containers: Vec<Container>) -> Pod {
        Pod {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some(ns.into()),
                ..Default::default()
            },
            spec: Some(PodSpec {
                containers,
                ..Default::default()
            }),
            status: None,
        }
    }

    fn make_container(name: &str, ports: Vec<K8sContainerPort>) -> Container {
        Container {
            name: name.into(),
            ports: if ports.is_empty() {
                None
            } else {
                Some(ports)
            },
            ..Default::default()
        }
    }

    fn make_k8s_port(port: i32, name: Option<&str>, protocol: Option<&str>) -> K8sContainerPort {
        K8sContainerPort {
            container_port: port,
            name: name.map(String::from),
            protocol: protocol.map(String::from),
            ..Default::default()
        }
    }

    fn make_service(name: &str, ns: &str, ports: Vec<K8sServicePort>) -> Service {
        Service {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some(ns.into()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                ports: if ports.is_empty() {
                    None
                } else {
                    Some(ports)
                },
                ..Default::default()
            }),
            status: None,
        }
    }

    fn make_svc_port(
        port: i32,
        target_port: Option<IntOrString>,
        name: Option<&str>,
        node_port: Option<i32>,
    ) -> K8sServicePort {
        K8sServicePort {
            port,
            target_port,
            name: name.map(String::from),
            node_port,
            protocol: Some("TCP".into()),
            ..Default::default()
        }
    }

    // ── Pod port extraction tests ──

    #[test]
    fn extract_single_container_port() {
        let pod = make_pod(
            "nginx",
            "default",
            vec![make_container("nginx", vec![make_k8s_port(80, Some("http"), None)])],
        );
        let ports = extract_pod_ports(&pod);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].pod_name, "nginx");
        assert_eq!(ports[0].namespace, "default");
        assert_eq!(ports[0].container_name, "nginx");
        assert_eq!(ports[0].container_port, 80);
        assert_eq!(ports[0].protocol, "TCP");
        assert_eq!(ports[0].port_name, Some("http".into()));
    }

    #[test]
    fn extract_multiple_container_ports() {
        let pod = make_pod(
            "web",
            "prod",
            vec![make_container(
                "web",
                vec![
                    make_k8s_port(8080, Some("http"), Some("TCP")),
                    make_k8s_port(8443, Some("https"), Some("TCP")),
                    make_k8s_port(9090, Some("metrics"), Some("TCP")),
                ],
            )],
        );
        let ports = extract_pod_ports(&pod);
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0].container_port, 8080);
        assert_eq!(ports[1].container_port, 8443);
        assert_eq!(ports[2].container_port, 9090);
    }

    #[test]
    fn extract_multi_container_pod_ports() {
        let pod = make_pod(
            "sidecar-pod",
            "default",
            vec![
                make_container("app", vec![make_k8s_port(8080, None, None)]),
                make_container("envoy", vec![make_k8s_port(15001, None, None)]),
            ],
        );
        let ports = extract_pod_ports(&pod);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].container_name, "app");
        assert_eq!(ports[0].container_port, 8080);
        assert_eq!(ports[1].container_name, "envoy");
        assert_eq!(ports[1].container_port, 15001);
    }

    #[test]
    fn extract_pod_no_ports() {
        let pod = make_pod(
            "no-ports",
            "default",
            vec![make_container("app", vec![])],
        );
        let ports = extract_pod_ports(&pod);
        assert!(ports.is_empty());
    }

    #[test]
    fn extract_pod_no_spec() {
        let pod = Pod {
            metadata: ObjectMeta {
                name: Some("empty".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: None,
            status: None,
        };
        let ports = extract_pod_ports(&pod);
        assert!(ports.is_empty());
    }

    #[test]
    fn extract_pod_with_init_containers() {
        let mut pod = make_pod(
            "init-pod",
            "default",
            vec![make_container("app", vec![make_k8s_port(8080, None, None)])],
        );
        pod.spec.as_mut().unwrap().init_containers = Some(vec![Container {
            name: "init-db".into(),
            ports: Some(vec![K8sContainerPort {
                container_port: 5432,
                name: Some("postgres".into()),
                protocol: Some("TCP".into()),
                ..Default::default()
            }]),
            ..Default::default()
        }]);
        let ports = extract_pod_ports(&pod);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].container_name, "init-db");
        assert_eq!(ports[0].container_port, 5432);
        assert_eq!(ports[1].container_name, "app");
        assert_eq!(ports[1].container_port, 8080);
    }

    #[test]
    fn extract_pod_udp_protocol() {
        let pod = make_pod(
            "dns",
            "kube-system",
            vec![make_container(
                "coredns",
                vec![make_k8s_port(53, Some("dns"), Some("UDP"))],
            )],
        );
        let ports = extract_pod_ports(&pod);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].protocol, "UDP");
    }

    // ── Service port extraction tests ──

    #[test]
    fn extract_service_with_numeric_target_port() {
        let svc = make_service(
            "web-svc",
            "default",
            vec![make_svc_port(80, Some(IntOrString::Int(8080)), Some("http"), None)],
        );
        let ports = extract_service_ports(&svc);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].service_name, "web-svc");
        assert_eq!(ports[0].port, 80);
        assert_eq!(ports[0].target_port, TargetPort::Number(8080));
        assert_eq!(ports[0].port_name, Some("http".into()));
    }

    #[test]
    fn extract_service_with_named_target_port() {
        let svc = make_service(
            "api-svc",
            "prod",
            vec![make_svc_port(
                443,
                Some(IntOrString::String("https".into())),
                Some("https"),
                None,
            )],
        );
        let ports = extract_service_ports(&svc);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].target_port, TargetPort::Name("https".into()));
    }

    #[test]
    fn extract_service_default_target_port() {
        // When targetPort is not specified, it defaults to port
        let svc = make_service(
            "simple-svc",
            "default",
            vec![make_svc_port(3000, None, None, None)],
        );
        let ports = extract_service_ports(&svc);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 3000);
        assert_eq!(ports[0].target_port, TargetPort::Number(3000));
    }

    #[test]
    fn extract_service_with_node_port() {
        let svc = make_service(
            "nodeport-svc",
            "default",
            vec![make_svc_port(80, Some(IntOrString::Int(8080)), Some("http"), Some(30080))],
        );
        let ports = extract_service_ports(&svc);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].node_port, Some(30080));
    }

    #[test]
    fn extract_service_multiple_ports() {
        let svc = make_service(
            "multi-svc",
            "default",
            vec![
                make_svc_port(80, Some(IntOrString::Int(8080)), Some("http"), None),
                make_svc_port(443, Some(IntOrString::Int(8443)), Some("https"), None),
            ],
        );
        let ports = extract_service_ports(&svc);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].port, 80);
        assert_eq!(ports[1].port, 443);
    }

    #[test]
    fn extract_service_no_ports() {
        let svc = make_service("headless", "default", vec![]);
        let ports = extract_service_ports(&svc);
        assert!(ports.is_empty());
    }

    #[test]
    fn extract_service_no_spec() {
        let svc = Service {
            metadata: ObjectMeta {
                name: Some("empty".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            spec: None,
            status: None,
        };
        let ports = extract_service_ports(&svc);
        assert!(ports.is_empty());
    }

    // ── Named port resolution tests ──

    #[test]
    fn resolve_named_port_found() {
        let pod = make_pod(
            "web",
            "default",
            vec![make_container("web", vec![make_k8s_port(8080, Some("http"), None)])],
        );
        assert_eq!(resolve_named_port(&pod, "http"), Some(8080));
    }

    #[test]
    fn resolve_named_port_not_found() {
        let pod = make_pod(
            "web",
            "default",
            vec![make_container("web", vec![make_k8s_port(8080, Some("http"), None)])],
        );
        assert_eq!(resolve_named_port(&pod, "grpc"), None);
    }

    #[test]
    fn resolve_named_port_no_spec() {
        let pod = Pod {
            metadata: ObjectMeta::default(),
            spec: None,
            status: None,
        };
        assert_eq!(resolve_named_port(&pod, "http"), None);
    }

    #[test]
    fn resolve_named_port_in_init_container() {
        let mut pod = make_pod("pod", "default", vec![make_container("app", vec![])]);
        pod.spec.as_mut().unwrap().init_containers = Some(vec![Container {
            name: "init".into(),
            ports: Some(vec![K8sContainerPort {
                container_port: 5432,
                name: Some("pg".into()),
                ..Default::default()
            }]),
            ..Default::default()
        }]);
        assert_eq!(resolve_named_port(&pod, "pg"), Some(5432));
    }

    // ── Display format tests ──

    #[test]
    fn container_port_display_with_name() {
        let port = ContainerPort {
            pod_name: "p".into(),
            namespace: "ns".into(),
            container_name: "c".into(),
            container_port: 8080,
            protocol: "TCP".into(),
            port_name: Some("http".into()),
        };
        assert_eq!(format!("{}", port), "8080/TCP (http)");
    }

    #[test]
    fn container_port_display_without_name() {
        let port = ContainerPort {
            pod_name: "p".into(),
            namespace: "ns".into(),
            container_name: "c".into(),
            container_port: 9090,
            protocol: "TCP".into(),
            port_name: None,
        };
        assert_eq!(format!("{}", port), "9090/TCP");
    }

    #[test]
    fn service_port_display_same_target() {
        let port = ServicePort {
            service_name: "s".into(),
            namespace: "ns".into(),
            port: 80,
            target_port: TargetPort::Number(80),
            protocol: "TCP".into(),
            port_name: None,
            node_port: None,
        };
        // When target_port == port, no arrow shown
        assert_eq!(format!("{}", port), "80/TCP");
    }

    #[test]
    fn service_port_display_different_target() {
        let port = ServicePort {
            service_name: "s".into(),
            namespace: "ns".into(),
            port: 80,
            target_port: TargetPort::Number(8080),
            protocol: "TCP".into(),
            port_name: Some("http".into()),
            node_port: None,
        };
        assert_eq!(format!("{}", port), "80→8080/TCP (http)");
    }

    #[test]
    fn service_port_display_with_node_port() {
        let port = ServicePort {
            service_name: "s".into(),
            namespace: "ns".into(),
            port: 80,
            target_port: TargetPort::Number(8080),
            protocol: "TCP".into(),
            port_name: None,
            node_port: Some(30080),
        };
        assert_eq!(format!("{}", port), "80→8080:30080/TCP");
    }

    #[test]
    fn service_port_display_named_target() {
        let port = ServicePort {
            service_name: "s".into(),
            namespace: "ns".into(),
            port: 80,
            target_port: TargetPort::Name("http".into()),
            protocol: "TCP".into(),
            port_name: None,
            node_port: None,
        };
        assert_eq!(format!("{}", port), "80→http/TCP");
    }

    #[test]
    fn target_port_display() {
        assert_eq!(format!("{}", TargetPort::Number(8080)), "8080");
        assert_eq!(format!("{}", TargetPort::Name("http".into())), "http");
    }
}
