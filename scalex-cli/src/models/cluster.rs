use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level k8s-clusters.yaml structure
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct K8sClustersConfig {
    pub config: K8sConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct K8sConfig {
    pub common: CommonConfig,
    pub clusters: Vec<ClusterDef>,
    #[serde(default)]
    pub argocd: Option<ArgoCdConfig>,
    #[serde(default)]
    pub domains: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommonConfig {
    pub kubernetes_version: String,
    pub kubespray_version: String,
    #[serde(default = "default_container_runtime")]
    pub container_runtime: String,
    #[serde(default = "default_cni")]
    pub cni: String,
    #[serde(default)]
    pub cilium_version: String,
    #[serde(default)]
    pub kube_proxy_remove: bool,
    #[serde(default = "default_cgroup_driver")]
    pub cgroup_driver: String,
    #[serde(default)]
    pub helm_enabled: bool,
    #[serde(default)]
    pub kube_apiserver_admission_plugins: Vec<String>,
}

fn default_container_runtime() -> String {
    "containerd".to_string()
}
fn default_cni() -> String {
    "cilium".to_string()
}
fn default_cgroup_driver() -> String {
    "systemd".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterDef {
    pub cluster_name: String,
    pub cluster_sdi_resource_pool: String,
    #[serde(default)]
    pub cluster_role: String,
    pub network: ClusterNetwork,
    #[serde(default)]
    pub cilium: Option<CiliumConfig>,
    #[serde(default)]
    pub kubespray_extra_vars: Option<serde_yaml::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterNetwork {
    pub pod_cidr: String,
    pub service_cidr: String,
    #[serde(default = "default_dns_domain")]
    pub dns_domain: String,
    #[serde(default)]
    pub native_routing_cidr: Option<String>,
}

fn default_dns_domain() -> String {
    "cluster.local".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CiliumConfig {
    pub cluster_id: u32,
    pub cluster_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArgoCdConfig {
    #[serde(default = "default_argocd_ns")]
    pub namespace: String,
    #[serde(default)]
    pub repo_url: String,
    #[serde(default)]
    pub repo_branch: String,
    #[serde(default)]
    pub tower_manages: Vec<String>,
}

fn default_argocd_ns() -> String {
    "argocd".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_k8s_clusters_config() {
        let yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
    cni: "cilium"
    cilium_version: "1.17.5"
    kube_proxy_remove: true
    helm_enabled: true
    kube_apiserver_admission_plugins:
      - NodeRestriction
  clusters:
    - cluster_name: "tower"
      cluster_sdi_resource_pool: "tower"
      cluster_role: "management"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
        dns_domain: "tower.local"
      cilium:
        cluster_id: 1
        cluster_name: "tower"
    - cluster_name: "sandbox"
      cluster_sdi_resource_pool: "sandbox"
      cluster_role: "workload"
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
        dns_domain: "sandbox.local"
      cilium:
        cluster_id: 2
        cluster_name: "sandbox"
  argocd:
    namespace: "argocd"
    repo_url: "https://github.com/example/repo.git"
    tower_manages: ["sandbox"]
"#;
        let config: K8sClustersConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.config.common.kubernetes_version, "1.33.1");
        assert!(config.config.common.kube_proxy_remove);
        assert_eq!(config.config.clusters.len(), 2);
        assert_eq!(config.config.clusters[0].cluster_name, "tower");
        assert_eq!(config.config.clusters[0].network.pod_cidr, "10.244.0.0/20");
        assert_eq!(
            config.config.clusters[1]
                .cilium
                .as_ref()
                .unwrap()
                .cluster_id,
            2
        );
        assert_eq!(
            config.config.argocd.as_ref().unwrap().tower_manages,
            vec!["sandbox"]
        );
    }
}
