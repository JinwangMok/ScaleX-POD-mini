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

/// Generate Cilium Helm values.yaml for a specific cluster.
/// Pure function: merges base config with cluster-specific k8sServiceHost.
pub fn generate_cilium_values(control_plane_ip: &str, service_port: u16) -> String {
    format!(
        "k8sServiceHost: \"{control_plane_ip}\"\nk8sServicePort: {service_port}\n{CILIUM_BASE_VALUES}"
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
        assert!(!values.contains("192.168.88.100"), "must not contain tower IP");
    }

    #[test]
    fn test_generate_cilium_values_different_clusters_differ() {
        let tower = generate_cilium_values("192.168.88.100", 6443);
        let sandbox = generate_cilium_values("192.168.88.110", 6443);
        assert_ne!(tower, sandbox, "different clusters must produce different values");
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
}
