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

#[cfg(test)]
mod tests {
    use super::*;

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
}
