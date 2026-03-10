//! Pure functions for GitOps YAML manipulation.
//! No I/O — returns transformed strings.
#![allow(dead_code)]

/// Default placeholder used in generator/project YAML for sandbox cluster server URL.
#[allow(dead_code)]
const SANDBOX_SERVER_PLACEHOLDER: &str = "https://sandbox-api:6443";

/// Replace sandbox server URL placeholder in a YAML string.
/// Pure function: takes content string, returns transformed string.
pub fn replace_sandbox_server_url(content: &str, actual_url: &str) -> String {
    content.replace(SANDBOX_SERVER_PLACEHOLDER, actual_url)
}

/// Check if a YAML string still contains the sandbox placeholder.
/// Pure function.
pub fn has_sandbox_placeholder(content: &str) -> bool {
    content.contains(SANDBOX_SERVER_PLACEHOLDER)
}

/// Given a list of (path, content) tuples, return only those that contain
/// the sandbox placeholder, with the placeholder replaced.
/// Pure function.
pub fn replace_all_sandbox_urls(
    files: &[(String, String)],
    actual_url: &str,
) -> Vec<(String, String)> {
    files
        .iter()
        .filter(|(_, content)| has_sandbox_placeholder(content))
        .map(|(path, content)| {
            (
                path.clone(),
                replace_sandbox_server_url(content, actual_url),
            )
        })
        .collect()
}

/// Extract the server URL from a kubeconfig YAML string.
/// Pure function: parses the first cluster's server field.
pub fn extract_server_from_kubeconfig(kubeconfig_content: &str) -> Option<String> {
    let value: serde_yaml::Value = serde_yaml::from_str(kubeconfig_content).ok()?;
    value
        .get("clusters")?
        .as_sequence()?
        .first()?
        .get("cluster")?
        .get("server")?
        .as_str()
        .map(|s| s.to_string())
}

/// Collect gitops YAML file paths that need sandbox URL replacement.
/// Returns paths relative to the gitops directory.
pub fn gitops_files_needing_replacement() -> Vec<&'static str> {
    vec![
        "generators/sandbox/common-generator.yaml",
        "generators/sandbox/sandbox-generator.yaml",
        "projects/sandbox-project.yaml",
    ]
}

/// Generate a cluster-config ConfigMap manifest for a specific cluster.
/// Pure function: returns YAML string with correct cluster metadata.
pub fn generate_cluster_config_manifest(
    cluster_name: &str,
    cluster_domain: &str,
    cluster_role: &str,
) -> String {
    let cluster_type = match cluster_role {
        "management" => "management",
        _ => "workload",
    };
    format!(
        r#"apiVersion: v1
kind: ConfigMap
metadata:
  name: cluster-info
  namespace: kube-system
data:
  cluster.name: "{cluster_name}"
  cluster.domain: "{cluster_domain}"
  cluster.type: "{cluster_type}"
  managed-by: "argocd"
"#
    )
}

/// Return the gitops-relative path for a cluster's cluster-config manifest.
/// Pure function.
pub fn cluster_config_path(cluster_name: &str) -> String {
    format!("{cluster_name}/cluster-config/manifest.yaml")
}

/// Base Cilium Helm values that are common across all clusters.
/// k8sServiceHost is intentionally absent — it must be set per-cluster.
const CILIUM_BASE_VALUES: &str = r#"kubeProxyReplacement: true
operator:
  replicas: 1
ipam:
  mode: kubernetes
hubble:
  enabled: true
  relay:
    enabled: true
  ui:
    enabled: false
l2announcements:
  enabled: true
gatewayAPI:
  enabled: true
"#;

/// Return the gitops-relative path for a cluster's Cilium values.yaml.
/// Pure function.
pub fn cilium_values_path(cluster_name: &str) -> String {
    format!("{cluster_name}/cilium/values.yaml")
}

/// Return the gitops-relative path for a cluster's Cilium kustomization.yaml.
/// Pure function.
pub fn cilium_kustomization_path(cluster_name: &str) -> String {
    format!("{cluster_name}/cilium/kustomization.yaml")
}

/// Generate Cilium Helm values.yaml for a specific cluster.
/// Pure function: merges base config with cluster-specific k8sServiceHost.
pub fn generate_cilium_values(control_plane_ip: &str, service_port: u16) -> String {
    format!(
        "k8sServiceHost: \"{control_plane_ip}\"\nk8sServicePort: {service_port}\n{CILIUM_BASE_VALUES}"
    )
}

/// ClusterMesh peer information for Cilium multi-cluster connectivity.
pub struct ClusterMeshPeer {
    pub name: String,
    pub id: u32,
    pub api_server_ip: String,
}

/// Generate Cilium Helm values with ClusterMesh enabled.
/// Pure function: adds cluster identity and mesh configuration.
pub fn generate_cilium_values_with_mesh(
    control_plane_ip: &str,
    service_port: u16,
    cluster_name: &str,
    cluster_id: u32,
) -> String {
    let base = generate_cilium_values(control_plane_ip, service_port);
    format!(
        "{base}cluster:\n  name: \"{cluster_name}\"\n  id: {cluster_id}\nclustermesh:\n  useAPIServer: true\n  apiserver:\n    service:\n      type: NodePort\n"
    )
}

/// Generate a Cilium ClusterMesh connect manifest (Secret) for peering.
/// Pure function: generates the connection config for a remote cluster.
pub fn generate_clustermesh_peer_secret(local_cluster: &str, peer: &ClusterMeshPeer) -> String {
    format!(
        r#"apiVersion: v1
kind: Secret
metadata:
  name: cilium-clustermesh-{peer_name}
  namespace: kube-system
  labels:
    app.kubernetes.io/part-of: cilium
    clustermesh.cilium.io/peer: "{peer_name}"
  annotations:
    scalex.io/local-cluster: "{local_cluster}"
    scalex.io/peer-cluster: "{peer_name}"
type: Opaque
stringData:
  config.yaml: |
    cluster:
      name: "{peer_name}"
      id: {peer_id}
    endpoints:
      - "https://{peer_ip}:2379"
"#,
        peer_name = peer.name,
        peer_id = peer.id,
        peer_ip = peer.api_server_ip,
        local_cluster = local_cluster,
    )
}

/// Generate a Kustomize kustomization.yaml for a Cilium Helm chart deployment.
/// Pure function.
pub fn generate_cilium_kustomization(cilium_version: &str) -> String {
    format!(
        r#"apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization

helmCharts:
  - name: cilium
    repo: https://helm.cilium.io/
    version: {cilium_version}
    releaseName: cilium
    namespace: kube-system
    valuesFile: values.yaml
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Unit 1: Cilium multi-cluster values generation ──

    #[test]
    fn test_generate_cilium_values_tower() {
        let values = generate_cilium_values("192.168.88.100", 6443);
        assert!(
            values.contains("k8sServiceHost: \"192.168.88.100\""),
            "tower cilium must have tower CP IP"
        );
        assert!(values.contains("k8sServicePort: 6443"));
        assert!(values.contains("kubeProxyReplacement: true"));
        assert!(values.contains("hubble:"));
        assert!(values.contains("l2announcements:"));
        assert!(values.contains("gatewayAPI:"));

        // Must be valid YAML
        let parsed: serde_yaml::Value = serde_yaml::from_str(&values)
            .unwrap_or_else(|e| panic!("cilium values not valid YAML: {e}\n{values}"));
        assert!(parsed.is_mapping());
    }

    #[test]
    fn test_generate_cilium_values_sandbox() {
        let values = generate_cilium_values("192.168.88.110", 6443);
        assert!(
            values.contains("k8sServiceHost: \"192.168.88.110\""),
            "sandbox cilium must have sandbox CP IP"
        );
        assert!(
            !values.contains("192.168.88.100"),
            "must not contain tower IP"
        );
    }

    #[test]
    fn test_generate_cilium_values_different_clusters_differ() {
        let tower = generate_cilium_values("192.168.88.100", 6443);
        let sandbox = generate_cilium_values("192.168.88.110", 6443);
        assert_ne!(
            tower, sandbox,
            "different clusters must produce different values"
        );
        // But both share the same base structure
        assert!(tower.contains("kubeProxyReplacement: true"));
        assert!(sandbox.contains("kubeProxyReplacement: true"));
    }

    #[test]
    fn test_generate_cilium_kustomization() {
        let kust = generate_cilium_kustomization("1.17.5");
        assert!(kust.contains("version: 1.17.5"));
        assert!(kust.contains("name: cilium"));
        assert!(kust.contains("repo: https://helm.cilium.io/"));
        assert!(kust.contains("valuesFile: values.yaml"));

        // Must be valid YAML
        let parsed: serde_yaml::Value = serde_yaml::from_str(&kust)
            .unwrap_or_else(|e| panic!("kustomization not valid YAML: {e}"));
        assert!(parsed.is_mapping());
    }

    // ── Sprint 5: Disk-based generator YAML correctness tests ──

    #[test]
    fn test_tower_common_generator_parses_as_valid_yaml() {
        let content = include_str!("../../../gitops/generators/tower/common-generator.yaml");
        let value: serde_yaml::Value =
            serde_yaml::from_str(content).expect("tower common-generator.yaml is invalid YAML");
        assert_eq!(
            value["kind"].as_str().unwrap(),
            "ApplicationSet",
            "tower common-generator must be an ApplicationSet"
        );
        assert_eq!(value["metadata"]["name"].as_str().unwrap(), "tower-common");
    }

    #[test]
    fn test_tower_generator_parses_as_valid_yaml() {
        let content = include_str!("../../../gitops/generators/tower/tower-generator.yaml");
        let value: serde_yaml::Value =
            serde_yaml::from_str(content).expect("tower-generator.yaml is invalid YAML");
        assert_eq!(value["kind"].as_str().unwrap(), "ApplicationSet");
        assert_eq!(value["metadata"]["name"].as_str().unwrap(), "tower-apps");
    }

    #[test]
    fn test_sandbox_common_generator_parses_as_valid_yaml() {
        let content = include_str!("../../../gitops/generators/sandbox/common-generator.yaml");
        let value: serde_yaml::Value =
            serde_yaml::from_str(content).expect("sandbox common-generator.yaml is invalid YAML");
        assert_eq!(value["kind"].as_str().unwrap(), "ApplicationSet");
        assert_eq!(
            value["metadata"]["name"].as_str().unwrap(),
            "sandbox-common"
        );
    }

    #[test]
    fn test_sandbox_generator_parses_as_valid_yaml() {
        let content = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");
        let value: serde_yaml::Value =
            serde_yaml::from_str(content).expect("sandbox-generator.yaml is invalid YAML");
        assert_eq!(value["kind"].as_str().unwrap(), "ApplicationSet");
        assert_eq!(value["metadata"]["name"].as_str().unwrap(), "sandbox-apps");
    }

    #[test]
    fn test_sandbox_generators_contain_placeholder_url() {
        let common = include_str!("../../../gitops/generators/sandbox/common-generator.yaml");
        let specific = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");
        assert!(
            has_sandbox_placeholder(common),
            "sandbox common-generator must contain placeholder URL"
        );
        assert!(
            has_sandbox_placeholder(specific),
            "sandbox-generator must contain placeholder URL"
        );
    }

    #[test]
    fn test_tower_generators_do_not_contain_placeholder_url() {
        let common = include_str!("../../../gitops/generators/tower/common-generator.yaml");
        let specific = include_str!("../../../gitops/generators/tower/tower-generator.yaml");
        assert!(
            !has_sandbox_placeholder(common),
            "tower common-generator must NOT contain sandbox placeholder"
        );
        assert!(
            !has_sandbox_placeholder(specific),
            "tower-generator must NOT contain sandbox placeholder"
        );
    }

    #[test]
    fn test_replace_sandbox_url_on_actual_generator_files() {
        let common = include_str!("../../../gitops/generators/sandbox/common-generator.yaml");
        let specific = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");
        let actual_url = "https://192.168.88.110:6443";

        let replaced_common = replace_sandbox_server_url(common, actual_url);
        let replaced_specific = replace_sandbox_server_url(specific, actual_url);

        // Placeholder gone
        assert!(!has_sandbox_placeholder(&replaced_common));
        assert!(!has_sandbox_placeholder(&replaced_specific));
        // Actual URL present
        assert!(replaced_common.contains(actual_url));
        assert!(replaced_specific.contains(actual_url));
        // Structure preserved
        let v: serde_yaml::Value = serde_yaml::from_str(&replaced_common)
            .expect("replaced common-generator must still be valid YAML");
        assert_eq!(v["kind"].as_str().unwrap(), "ApplicationSet");
    }

    #[test]
    fn test_tower_and_sandbox_common_generators_have_same_app_list() {
        let tower = include_str!("../../../gitops/generators/tower/common-generator.yaml");
        let sandbox = include_str!("../../../gitops/generators/sandbox/common-generator.yaml");

        let tv: serde_yaml::Value = serde_yaml::from_str(tower).unwrap();
        let sv: serde_yaml::Value = serde_yaml::from_str(sandbox).unwrap();

        let t_elements = tv["spec"]["generators"][0]["list"]["elements"]
            .as_sequence()
            .expect("tower common-generator must have elements");
        let s_elements = sv["spec"]["generators"][0]["list"]["elements"]
            .as_sequence()
            .expect("sandbox common-generator must have elements");

        let t_apps: Vec<&str> = t_elements
            .iter()
            .map(|e| e["appName"].as_str().unwrap())
            .collect();
        let s_apps: Vec<&str> = s_elements
            .iter()
            .map(|e| e["appName"].as_str().unwrap())
            .collect();

        assert_eq!(
            t_apps, s_apps,
            "Tower and Sandbox common-generators must deploy the same app list"
        );
    }

    #[test]
    fn test_all_generators_use_consistent_repo_url() {
        let files = vec![
            (
                "tower/common-generator",
                include_str!("../../../gitops/generators/tower/common-generator.yaml"),
            ),
            (
                "tower/tower-generator",
                include_str!("../../../gitops/generators/tower/tower-generator.yaml"),
            ),
            (
                "sandbox/common-generator",
                include_str!("../../../gitops/generators/sandbox/common-generator.yaml"),
            ),
            (
                "sandbox/sandbox-generator",
                include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml"),
            ),
        ];

        let mut repo_urls: Vec<(&str, String)> = Vec::new();
        for (name, content) in &files {
            let v: serde_yaml::Value = serde_yaml::from_str(content).unwrap();
            let url = v["spec"]["template"]["spec"]["source"]["repoURL"]
                .as_str()
                .unwrap_or_else(|| panic!("{} must have repoURL", name))
                .to_string();
            repo_urls.push((name, url));
        }

        let first_url = &repo_urls[0].1;
        for (name, url) in &repo_urls[1..] {
            assert_eq!(
                url, first_url,
                "repoURL mismatch: {} has '{}' but {} has '{}'",
                name, url, repo_urls[0].0, first_url
            );
        }
    }

    // ── Original unit tests ──

    #[test]
    fn test_replace_sandbox_server_url() {
        let yaml = r#"
      destination:
        server: "https://sandbox-api:6443"
        namespace: kube-system
"#;
        let result = replace_sandbox_server_url(yaml, "https://192.168.88.110:6443");
        assert!(result.contains("https://192.168.88.110:6443"));
        assert!(!result.contains("sandbox-api"));
    }

    #[test]
    fn test_has_sandbox_placeholder() {
        assert!(has_sandbox_placeholder(
            "server: \"https://sandbox-api:6443\""
        ));
        assert!(!has_sandbox_placeholder(
            "server: \"https://192.168.88.110:6443\""
        ));
    }

    #[test]
    fn test_replace_all_sandbox_urls() {
        let files = vec![
            (
                "generators/sandbox/common-generator.yaml".to_string(),
                "server: \"https://sandbox-api:6443\"".to_string(),
            ),
            (
                "generators/tower/tower-generator.yaml".to_string(),
                "server: \"https://kubernetes.default.svc\"".to_string(),
            ),
            (
                "projects/sandbox-project.yaml".to_string(),
                "server: \"https://sandbox-api:6443\"".to_string(),
            ),
        ];
        let result = replace_all_sandbox_urls(&files, "https://10.0.0.5:6443");
        // Only 2 files should be returned (those with placeholder)
        assert_eq!(result.len(), 2);
        assert!(result[0].1.contains("https://10.0.0.5:6443"));
        assert!(result[1].1.contains("https://10.0.0.5:6443"));
        // Tower generator should NOT be in result
        assert!(result.iter().all(|(p, _)| !p.contains("tower-generator")));
    }

    #[test]
    fn test_extract_server_from_kubeconfig() {
        let kubeconfig = r#"
apiVersion: v1
kind: Config
clusters:
  - cluster:
      certificate-authority-data: LS0t...
      server: https://192.168.88.110:6443
    name: sandbox
contexts:
  - context:
      cluster: sandbox
      user: admin
    name: admin@sandbox
"#;
        let server = extract_server_from_kubeconfig(kubeconfig);
        assert_eq!(server, Some("https://192.168.88.110:6443".to_string()));
    }

    #[test]
    fn test_extract_server_from_empty_kubeconfig() {
        assert_eq!(extract_server_from_kubeconfig("{}"), None);
        assert_eq!(extract_server_from_kubeconfig("invalid"), None);
    }

    #[test]
    fn test_replace_preserves_non_placeholder_content() {
        let yaml = r#"apiVersion: argoproj.io/v1alpha1
kind: ApplicationSet
metadata:
  name: sandbox-common
spec:
  template:
    spec:
      destination:
        server: "https://sandbox-api:6443"
        namespace: "{{.namespace}}"
      syncPolicy:
        automated:
          prune: true
"#;
        let result = replace_sandbox_server_url(yaml, "https://192.168.88.110:6443");
        assert!(result.contains("kind: ApplicationSet"));
        assert!(result.contains("sandbox-common"));
        assert!(result.contains("prune: true"));
        assert!(result.contains("https://192.168.88.110:6443"));
    }

    // ── Sprint 3 (new session): Placeholder replacement completeness ──

    /// Verify gitops_files_needing_replacement() lists exactly the files
    /// that actually contain the sandbox-api placeholder on disk.
    #[test]
    fn test_replacement_list_matches_actual_placeholder_files() {
        let all_generator_files = [
            (
                "generators/tower/common-generator.yaml",
                include_str!("../../../gitops/generators/tower/common-generator.yaml"),
            ),
            (
                "generators/tower/tower-generator.yaml",
                include_str!("../../../gitops/generators/tower/tower-generator.yaml"),
            ),
            (
                "generators/sandbox/common-generator.yaml",
                include_str!("../../../gitops/generators/sandbox/common-generator.yaml"),
            ),
            (
                "generators/sandbox/sandbox-generator.yaml",
                include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml"),
            ),
            (
                "projects/sandbox-project.yaml",
                include_str!("../../../gitops/projects/sandbox-project.yaml"),
            ),
            (
                "projects/tower-project.yaml",
                include_str!("../../../gitops/projects/tower-project.yaml"),
            ),
        ];

        let files_with_placeholder: Vec<&str> = all_generator_files
            .iter()
            .filter(|(_, content)| has_sandbox_placeholder(content))
            .map(|(path, _)| *path)
            .collect();

        let expected = gitops_files_needing_replacement();
        assert_eq!(
            files_with_placeholder, expected,
            "gitops_files_needing_replacement() must match actual files containing placeholder"
        );
    }

    /// Verify each placeholder file remains valid YAML after URL replacement.
    #[test]
    fn test_replacement_produces_valid_yaml_for_all_files() {
        let files = [
            include_str!("../../../gitops/generators/sandbox/common-generator.yaml"),
            include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml"),
            include_str!("../../../gitops/projects/sandbox-project.yaml"),
        ];
        let replacement_url = "https://10.0.0.100:6443";

        for (i, content) in files.iter().enumerate() {
            let replaced = replace_sandbox_server_url(content, replacement_url);
            let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(&replaced);
            assert!(
                parsed.is_ok(),
                "File index {} produced invalid YAML after replacement: {:?}",
                i,
                parsed.err()
            );
            assert!(
                !has_sandbox_placeholder(&replaced),
                "File index {} still has placeholder after replacement",
                i
            );
        }
    }

    /// Verify every app referenced in generators has a corresponding directory.
    /// Also verify no directory in common/ is dead code (unreferenced by any generator).
    #[test]
    fn test_generator_references_match_actual_directories() {
        // Parse all generator files to extract app names and source paths
        let generators: Vec<(&str, &str)> = vec![
            (
                "tower-common",
                include_str!("../../../gitops/generators/tower/common-generator.yaml"),
            ),
            (
                "tower-apps",
                include_str!("../../../gitops/generators/tower/tower-generator.yaml"),
            ),
            (
                "sandbox-common",
                include_str!("../../../gitops/generators/sandbox/common-generator.yaml"),
            ),
            (
                "sandbox-apps",
                include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml"),
            ),
        ];

        let mut all_referenced_paths: Vec<String> = Vec::new();

        for (gen_name, content) in &generators {
            let yaml: serde_yaml::Value = serde_yaml::from_str(content)
                .unwrap_or_else(|e| panic!("Failed to parse {}: {}", gen_name, e));

            // Extract path template from spec.template.spec.source.path
            let path_template = yaml["spec"]["template"]["spec"]["source"]["path"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: missing source.path", gen_name));

            // Extract app names from generator list elements
            let elements = yaml["spec"]["generators"][0]["list"]["elements"]
                .as_sequence()
                .unwrap_or_else(|| panic!("{}: missing elements", gen_name));

            for elem in elements {
                let app_name = elem["appName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{}: element missing appName", gen_name));
                let resolved = path_template.replace("{{.appName}}", app_name);
                all_referenced_paths.push(resolved);
            }
        }

        // Verify each referenced path exists as a directory
        for path in &all_referenced_paths {
            let full_path = format!(
                "{}/{}",
                env!("CARGO_MANIFEST_DIR").trim_end_matches("/scalex-cli"),
                path
            );
            // Use Path to check existence at compile time via a runtime check
            assert!(
                std::path::Path::new(&full_path).is_dir(),
                "Generator references '{}' but directory does not exist at '{}'",
                path,
                full_path
            );
        }

        // Verify common/ has no unreferenced directories (dead code check)
        let common_dir = format!(
            "{}/gitops/common",
            env!("CARGO_MANIFEST_DIR").trim_end_matches("/scalex-cli")
        );
        if let Ok(entries) = std::fs::read_dir(&common_dir) {
            let common_apps: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| format!("gitops/common/{}", e.file_name().to_string_lossy()))
                .collect();

            for app_dir in &common_apps {
                assert!(
                    all_referenced_paths.contains(app_dir),
                    "Directory '{}' exists but is not referenced by any generator (dead code)",
                    app_dir
                );
            }
        }
    }

    /// Verify tower/cilium and sandbox/cilium each have k8sServiceHost set.
    #[test]
    fn test_per_cluster_cilium_has_service_host() {
        let tower_values = include_str!("../../../gitops/tower/cilium/values.yaml");
        let sandbox_values = include_str!("../../../gitops/sandbox/cilium/values.yaml");

        assert!(
            tower_values.contains("k8sServiceHost:"),
            "tower/cilium/values.yaml must define k8sServiceHost"
        );
        assert!(
            sandbox_values.contains("k8sServiceHost:"),
            "sandbox/cilium/values.yaml must define k8sServiceHost"
        );

        // Tower should have a real IP, not a placeholder
        assert!(
            !tower_values.contains("PLACEHOLDER"),
            "tower/cilium should have a real IP, not PLACEHOLDER"
        );
    }

    // ── cluster-config ConfigMap generation ──

    #[test]
    fn test_generate_cluster_config_tower_is_management() {
        let manifest = generate_cluster_config_manifest("tower", "jinwang.dev", "management");
        assert!(manifest.contains("cluster.type: \"management\""));
        assert!(manifest.contains("cluster.name: \"tower\""));
        assert!(manifest.contains("cluster.domain: \"jinwang.dev\""));

        let parsed: serde_yaml::Value = serde_yaml::from_str(&manifest)
            .unwrap_or_else(|e| panic!("cluster-config not valid YAML: {e}"));
        assert_eq!(parsed["kind"].as_str().unwrap(), "ConfigMap");
    }

    #[test]
    fn test_generate_cluster_config_sandbox_is_workload() {
        let manifest = generate_cluster_config_manifest("sandbox", "jinwang.dev", "workload");
        assert!(manifest.contains("cluster.type: \"workload\""));
        assert!(manifest.contains("cluster.name: \"sandbox\""));
    }

    #[test]
    fn test_cluster_config_path_mapping() {
        assert_eq!(
            cluster_config_path("tower"),
            "tower/cluster-config/manifest.yaml"
        );
        assert_eq!(
            cluster_config_path("sandbox"),
            "sandbox/cluster-config/manifest.yaml"
        );
    }

    /// Verify cilium_values_path returns correct gitops-relative paths.
    #[test]
    fn test_cilium_values_path_mapping() {
        assert_eq!(cilium_values_path("tower"), "tower/cilium/values.yaml");
        assert_eq!(cilium_values_path("sandbox"), "sandbox/cilium/values.yaml");
    }

    /// Verify that generate_cilium_values with a known CP IP produces
    /// content that matches what the static gitops file should contain.
    #[test]
    fn test_generated_cilium_values_match_static_tower_structure() {
        let generated = generate_cilium_values("192.168.88.100", 6443);
        let static_content = include_str!("../../../gitops/tower/cilium/values.yaml");

        // Both must have the same k8sServiceHost
        let gen_parsed: serde_yaml::Value = serde_yaml::from_str(&generated).unwrap();
        let static_parsed: serde_yaml::Value = serde_yaml::from_str(static_content).unwrap();

        assert_eq!(
            gen_parsed["k8sServiceHost"], static_parsed["k8sServiceHost"],
            "generated Cilium values k8sServiceHost must match static tower file"
        );

        // Both must have kubeProxyReplacement
        assert_eq!(
            gen_parsed["kubeProxyReplacement"], static_parsed["kubeProxyReplacement"],
            "kubeProxyReplacement must match"
        );
    }

    /// Verify no placeholder URL remains in tower generator files (they should
    /// point to the local cluster, not sandbox).
    #[test]
    fn test_tower_files_never_contain_sandbox_placeholder() {
        let tower_files = [
            include_str!("../../../gitops/generators/tower/common-generator.yaml"),
            include_str!("../../../gitops/generators/tower/tower-generator.yaml"),
            include_str!("../../../gitops/projects/tower-project.yaml"),
        ];
        for content in &tower_files {
            assert!(
                !has_sandbox_placeholder(content),
                "Tower file unexpectedly contains sandbox placeholder"
            );
        }
    }

    // ── ClusterMesh ──

    #[test]
    fn test_generate_cilium_values_with_mesh_contains_cluster_identity() {
        let values = generate_cilium_values_with_mesh("192.168.88.100", 6443, "tower", 1);
        assert!(values.contains("cluster:\n  name: \"tower\""));
        assert!(values.contains("id: 1"));
        assert!(values.contains("clustermesh:"));
        assert!(values.contains("useAPIServer: true"));
    }

    #[test]
    fn test_generate_cilium_values_with_mesh_preserves_base() {
        let values = generate_cilium_values_with_mesh("192.168.88.100", 6443, "tower", 1);
        assert!(values.contains("k8sServiceHost: \"192.168.88.100\""));
        assert!(values.contains("kubeProxyReplacement: true"));
        assert!(values.contains("hubble:"));
        assert!(values.contains("gatewayAPI:"));
    }

    #[test]
    fn test_generate_cilium_values_with_mesh_valid_yaml() {
        let values = generate_cilium_values_with_mesh("192.168.88.110", 6443, "sandbox", 2);
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&values).expect("ClusterMesh values must be valid YAML");
        assert_eq!(parsed["cluster"]["name"].as_str().unwrap(), "sandbox");
        assert_eq!(parsed["cluster"]["id"].as_u64().unwrap(), 2);
        assert_eq!(
            parsed["clustermesh"]["useAPIServer"].as_bool().unwrap(),
            true
        );
    }

    #[test]
    fn test_generate_clustermesh_peer_secret_structure() {
        let peer = ClusterMeshPeer {
            name: "sandbox".to_string(),
            id: 2,
            api_server_ip: "192.168.88.110".to_string(),
        };
        let secret = generate_clustermesh_peer_secret("tower", &peer);
        assert!(secret.contains("kind: Secret"));
        assert!(secret.contains("cilium-clustermesh-sandbox"));
        assert!(secret.contains("namespace: kube-system"));
        assert!(secret.contains("peer: \"sandbox\""));
    }

    #[test]
    fn test_generate_clustermesh_peer_secret_valid_yaml() {
        let peer = ClusterMeshPeer {
            name: "sandbox".to_string(),
            id: 2,
            api_server_ip: "192.168.88.110".to_string(),
        };
        let secret = generate_clustermesh_peer_secret("tower", &peer);
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&secret).expect("ClusterMesh peer secret must be valid YAML");
        assert_eq!(parsed["kind"].as_str().unwrap(), "Secret");
        assert_eq!(
            parsed["metadata"]["name"].as_str().unwrap(),
            "cilium-clustermesh-sandbox"
        );
    }

    #[test]
    fn test_generate_clustermesh_peer_secret_contains_endpoint() {
        let peer = ClusterMeshPeer {
            name: "tower".to_string(),
            id: 1,
            api_server_ip: "192.168.88.100".to_string(),
        };
        let secret = generate_clustermesh_peer_secret("sandbox", &peer);
        assert!(secret.contains("https://192.168.88.100:2379"));
        assert!(secret.contains("name: \"tower\""));
        assert!(secret.contains("id: 1"));
        assert!(secret.contains("local-cluster: \"sandbox\""));
    }

    #[test]
    fn test_generate_clustermesh_peer_secret_bidirectional() {
        let tower_peer = ClusterMeshPeer {
            name: "tower".to_string(),
            id: 1,
            api_server_ip: "192.168.88.100".to_string(),
        };
        let sandbox_peer = ClusterMeshPeer {
            name: "sandbox".to_string(),
            id: 2,
            api_server_ip: "192.168.88.110".to_string(),
        };
        let secret_on_sandbox = generate_clustermesh_peer_secret("sandbox", &tower_peer);
        let secret_on_tower = generate_clustermesh_peer_secret("tower", &sandbox_peer);

        // Each secret references the OTHER cluster
        assert!(secret_on_sandbox.contains("cilium-clustermesh-tower"));
        assert!(secret_on_tower.contains("cilium-clustermesh-sandbox"));
    }
}
