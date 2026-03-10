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
    /// Disable firewalld on all nodes (default: false = disabled)
    #[serde(default)]
    pub firewalld_enabled: bool,
    /// Disable kube-vip (default: false = disabled)
    #[serde(default)]
    pub kube_vip_enabled: bool,
    /// Enable Gateway API CRDs
    #[serde(default)]
    pub gateway_api_enabled: bool,
    /// Gateway API version (e.g. "1.3.0")
    #[serde(default)]
    pub gateway_api_version: String,
    /// Enable kubelet graceful node shutdown
    #[serde(default)]
    pub graceful_node_shutdown: bool,
    /// Graceful shutdown timeout in seconds
    #[serde(default = "default_graceful_shutdown_sec")]
    pub graceful_node_shutdown_sec: u32,
    /// Custom kubelet flags
    #[serde(default)]
    pub kubelet_custom_flags: Vec<String>,
    /// Copy kubeconfig to Ansible host (default: true for production)
    #[serde(default = "default_true")]
    pub kubeconfig_localhost: bool,
    /// Download kubectl to Ansible host (default: true for production)
    #[serde(default = "default_true")]
    pub kubectl_localhost: bool,
    /// Enable nodelocal DNS cache (default: true for production)
    #[serde(default = "default_true")]
    pub enable_nodelocaldns: bool,
    /// Pod network node prefix length (default: 24)
    #[serde(default = "default_node_prefix")]
    pub kube_network_node_prefix: u32,
    /// Enable NTP synchronization (default: true)
    #[serde(default = "default_true")]
    pub ntp_enabled: bool,
    /// Etcd deployment type: "host" (recommended for production) or "docker"/"kubeadm"
    #[serde(default = "default_etcd_deployment_type")]
    pub etcd_deployment_type: String,
    /// DNS mode: "coredns" (default, production standard)
    #[serde(default = "default_dns_mode")]
    pub dns_mode: String,
}

fn default_graceful_shutdown_sec() -> u32 {
    120
}

fn default_true() -> bool {
    true
}

fn default_node_prefix() -> u32 {
    24
}

fn default_etcd_deployment_type() -> String {
    "host".to_string()
}

fn default_dns_mode() -> String {
    "coredns".to_string()
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ClusterMode {
    #[default]
    Sdi,
    Baremetal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaremetalNode {
    pub node_name: String,
    pub ip: String,
    pub roles: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterDef {
    pub cluster_name: String,
    /// "sdi" (default) or "baremetal"
    #[serde(default)]
    pub cluster_mode: ClusterMode,
    /// SDI pool name (required when mode=sdi)
    #[serde(default)]
    pub cluster_sdi_resource_pool: String,
    /// Direct baremetal nodes (required when mode=baremetal)
    #[serde(default)]
    pub baremetal_nodes: Vec<BaremetalNode>,
    #[serde(default)]
    pub cluster_role: String,
    pub network: ClusterNetwork,
    #[serde(default)]
    pub cilium: Option<CiliumConfig>,
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
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

/// OIDC authentication config for kube-apiserver (Keycloak integration)
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OidcConfig {
    /// Enable OIDC authentication on kube-apiserver
    #[serde(default)]
    pub enabled: bool,
    /// OIDC client ID (e.g., "kubernetes")
    #[serde(default)]
    pub client_id: String,
    /// OIDC issuer URL (e.g., "https://auth.jinwang.dev/realms/kubernetes")
    #[serde(default)]
    pub issuer_url: String,
    /// Token claim for username (default: "preferred_username")
    #[serde(default = "default_username_claim")]
    pub username_claim: String,
    /// Username prefix (default: "oidc:")
    #[serde(default = "default_oidc_prefix")]
    pub username_prefix: String,
    /// Token claim for groups (default: "groups")
    #[serde(default = "default_groups_claim")]
    pub groups_claim: String,
    /// Groups prefix (default: "oidc:")
    #[serde(default = "default_oidc_prefix")]
    pub groups_prefix: String,
}

fn default_username_claim() -> String {
    "preferred_username".to_string()
}
fn default_groups_claim() -> String {
    "groups".to_string()
}
fn default_oidc_prefix() -> String {
    "oidc:".to_string()
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
        // Default mode should be Sdi
        assert_eq!(config.config.clusters[0].cluster_mode, ClusterMode::Sdi);
    }

    #[test]
    fn test_parse_baremetal_cluster() {
        let yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
  clusters:
    - cluster_name: "prod"
      cluster_mode: "baremetal"
      cluster_role: "workload"
      baremetal_nodes:
        - node_name: "node-0"
          ip: "10.0.0.1"
          roles: [control-plane, etcd]
        - node_name: "node-1"
          ip: "10.0.0.2"
          roles: [worker]
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
"#;
        let config: K8sClustersConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.config.clusters[0].cluster_mode,
            ClusterMode::Baremetal
        );
        assert_eq!(config.config.clusters[0].baremetal_nodes.len(), 2);
        assert_eq!(
            config.config.clusters[0].baremetal_nodes[0].node_name,
            "node-0"
        );
        assert_eq!(config.config.clusters[0].baremetal_nodes[1].ip, "10.0.0.2");
    }

    #[test]
    fn test_parse_oidc_config() {
        let yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
  clusters:
    - cluster_name: "sandbox"
      cluster_sdi_resource_pool: "sandbox"
      cluster_role: "workload"
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
      oidc:
        enabled: true
        client_id: "kubernetes"
        issuer_url: "https://auth.jinwang.dev/realms/kubernetes"
"#;
        let config: K8sClustersConfig = serde_yaml::from_str(yaml).unwrap();
        let oidc = config.config.clusters[0].oidc.as_ref().unwrap();
        assert!(oidc.enabled);
        assert_eq!(oidc.client_id, "kubernetes");
        assert_eq!(
            oidc.issuer_url,
            "https://auth.jinwang.dev/realms/kubernetes"
        );
        // Defaults
        assert_eq!(oidc.username_claim, "preferred_username");
        assert_eq!(oidc.groups_claim, "groups");
        assert_eq!(oidc.username_prefix, "oidc:");
        assert_eq!(oidc.groups_prefix, "oidc:");
    }

    #[test]
    fn test_parse_cluster_without_oidc() {
        let yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
  clusters:
    - cluster_name: "tower"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
"#;
        let config: K8sClustersConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.config.clusters[0].oidc.is_none());
    }

    /// Verify the actual k8s-clusters.yaml.example content can be parsed.
    #[test]
    fn test_parse_k8s_clusters_example_content() {
        let yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
    container_runtime: "containerd"
    cni: "cilium"
    cilium_version: "1.17.5"
    kube_proxy_remove: true
    cgroup_driver: "systemd"
    helm_enabled: true
    kube_apiserver_admission_plugins:
      - NodeRestriction
      - PodTolerationRestriction
    firewalld_enabled: false
    kube_vip_enabled: false
    graceful_node_shutdown: true
    graceful_node_shutdown_sec: 120
    kubelet_custom_flags:
      - "--node-ip={{ ip }}"
    gateway_api_enabled: true
    gateway_api_version: "1.3.0"
    kubeconfig_localhost: true
    kubectl_localhost: true
    enable_nodelocaldns: true
    kube_network_node_prefix: 24
    ntp_enabled: true
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
      kubespray_extra_vars:
        kube_api_anonymous_auth: true
    - cluster_name: "sandbox"
      cluster_sdi_resource_pool: "sandbox"
      cluster_role: "workload"
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
        dns_domain: "sandbox.local"
        native_routing_cidr: "10.233.0.0/16"
      cilium:
        cluster_id: 2
        cluster_name: "sandbox"
      oidc:
        enabled: true
        client_id: "kubernetes"
        issuer_url: "https://auth.jinwang.dev/realms/kubernetes"
        username_claim: "preferred_username"
        username_prefix: "oidc:"
        groups_claim: "groups"
        groups_prefix: "oidc:"
      kubespray_extra_vars:
        kube_api_anonymous_auth: true
  argocd:
    namespace: "argocd"
    repo_url: "https://github.com/JinwangMok/ScaleX-POD-mini.git"
    repo_branch: "main"
    tower_manages: ["sandbox"]
  domains:
    auth: "auth.jinwang.dev"
    argocd: "cd.jinwang.dev"
"#;
        let config: K8sClustersConfig = serde_yaml::from_str(yaml).unwrap();

        // Common settings
        assert_eq!(config.config.common.kubernetes_version, "1.33.1");
        assert!(config.config.common.kube_proxy_remove);
        assert!(config.config.common.helm_enabled);
        assert!(config.config.common.ntp_enabled);
        assert!(config.config.common.kubeconfig_localhost);
        assert_eq!(
            config.config.common.kube_apiserver_admission_plugins,
            vec!["NodeRestriction", "PodTolerationRestriction"]
        );

        // Clusters
        assert_eq!(config.config.clusters.len(), 2);

        // Tower
        let tower = &config.config.clusters[0];
        assert_eq!(tower.cluster_name, "tower");
        assert_eq!(tower.cluster_role, "management");
        assert_eq!(tower.cluster_mode, ClusterMode::Sdi);
        assert!(tower.oidc.is_none());

        // Sandbox with OIDC
        let sandbox = &config.config.clusters[1];
        assert_eq!(sandbox.cluster_name, "sandbox");
        assert_eq!(sandbox.cluster_role, "workload");
        let oidc = sandbox.oidc.as_ref().unwrap();
        assert!(oidc.enabled);
        assert_eq!(oidc.client_id, "kubernetes");
        assert_eq!(
            oidc.issuer_url,
            "https://auth.jinwang.dev/realms/kubernetes"
        );
        assert_eq!(
            sandbox.network.native_routing_cidr,
            Some("10.233.0.0/16".to_string())
        );

        // ArgoCD
        let argocd = config.config.argocd.as_ref().unwrap();
        assert_eq!(argocd.tower_manages, vec!["sandbox"]);
    }
}
