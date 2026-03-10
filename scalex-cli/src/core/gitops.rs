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
