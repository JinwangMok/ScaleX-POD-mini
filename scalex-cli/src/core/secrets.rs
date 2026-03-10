use serde::{Deserialize, Serialize};

/// Parsed representation of credentials/secrets.yaml
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecretsConfig {
    pub keycloak: KeycloakSecrets,
    #[serde(default)]
    pub argocd: ArgocdSecrets,
    #[serde(default)]
    pub cloudflare: CloudflareSecrets,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeycloakSecrets {
    pub admin_password: String,
    pub db_password: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArgocdSecrets {
    #[serde(default)]
    pub repo_pat: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CloudflareSecrets {
    #[serde(default)]
    pub credentials_file: String,
    #[serde(default)]
    pub cert_file: String,
}

/// A K8s Secret to be created before GitOps bootstrap.
#[derive(Clone, Debug, PartialEq)]
pub struct K8sSecretSpec {
    pub name: String,
    pub namespace: String,
    pub data: Vec<(String, String)>,
}

/// Generate a Kubernetes Secret YAML manifest from a spec.
/// Pure function: no I/O.
pub fn generate_k8s_secret_yaml(spec: &K8sSecretSpec) -> String {
    let mut yaml = String::new();
    yaml.push_str("apiVersion: v1\n");
    yaml.push_str("kind: Secret\n");
    yaml.push_str("metadata:\n");
    yaml.push_str(&format!("  name: \"{}\"\n", spec.name));
    yaml.push_str(&format!("  namespace: \"{}\"\n", spec.namespace));
    yaml.push_str("type: Opaque\n");
    yaml.push_str("stringData:\n");
    for (key, value) in &spec.data {
        yaml.push_str(&format!("  {key}: \"{value}\"\n"));
    }
    yaml
}

/// Determine which K8s secrets are needed for a given cluster role.
/// Pure function.
pub fn secrets_for_cluster(cluster_role: &str, secrets: &SecretsConfig) -> Vec<K8sSecretSpec> {
    match cluster_role {
        "management" => {
            let mut specs = vec![
                K8sSecretSpec {
                    name: "keycloak-admin".to_string(),
                    namespace: "keycloak".to_string(),
                    data: vec![(
                        "admin-password".to_string(),
                        secrets.keycloak.admin_password.clone(),
                    )],
                },
                K8sSecretSpec {
                    name: "keycloak-db".to_string(),
                    namespace: "keycloak".to_string(),
                    data: vec![("password".to_string(), secrets.keycloak.db_password.clone())],
                },
            ];
            // Cloudflare tunnel credentials (only if credentials_file is set)
            if !secrets.cloudflare.credentials_file.is_empty() {
                specs.push(K8sSecretSpec {
                    name: "cloudflared-tunnel-credentials".to_string(),
                    namespace: "kube-tunnel".to_string(),
                    data: vec![(
                        "credentials.json".to_string(),
                        secrets.cloudflare.credentials_file.clone(),
                    )],
                });
            }
            specs
        }
        _ => vec![], // Workload clusters don't need pre-bootstrap secrets
    }
}

/// Parse secrets.yaml content into SecretsConfig.
/// Pure function (takes string, not file path).
pub fn parse_secrets_config(yaml_content: &str) -> Result<SecretsConfig, String> {
    serde_yaml::from_str(yaml_content).map_err(|e| format!("Failed to parse secrets.yaml: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_secrets() -> SecretsConfig {
        SecretsConfig {
            keycloak: KeycloakSecrets {
                admin_password: "admin123".to_string(),
                db_password: "dbpass456".to_string(),
            },
            argocd: ArgocdSecrets {
                repo_pat: "ghp_xxx".to_string(),
            },
            cloudflare: CloudflareSecrets {
                credentials_file:
                    "{\"AccountTag\":\"abc\",\"TunnelSecret\":\"xyz\",\"TunnelID\":\"123\"}"
                        .to_string(),
                cert_file: String::new(),
            },
        }
    }

    #[test]
    fn test_generate_k8s_secret_yaml_keycloak_admin() {
        let spec = K8sSecretSpec {
            name: "keycloak-admin".to_string(),
            namespace: "keycloak".to_string(),
            data: vec![("admin-password".to_string(), "s3cret".to_string())],
        };
        let yaml = generate_k8s_secret_yaml(&spec);

        assert!(yaml.contains("kind: Secret"));
        assert!(yaml.contains("name: \"keycloak-admin\""));
        assert!(yaml.contains("namespace: \"keycloak\""));
        assert!(yaml.contains("admin-password: \"s3cret\""));
        assert!(yaml.contains("stringData:"));

        // Must be valid YAML
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml)
            .unwrap_or_else(|e| panic!("generated secret is not valid YAML: {e}\n{yaml}"));
        assert!(parsed.is_mapping());
    }

    #[test]
    fn test_generate_k8s_secret_yaml_multiple_keys() {
        let spec = K8sSecretSpec {
            name: "keycloak-db".to_string(),
            namespace: "keycloak".to_string(),
            data: vec![
                ("password".to_string(), "dbpass".to_string()),
                ("username".to_string(), "keycloak".to_string()),
            ],
        };
        let yaml = generate_k8s_secret_yaml(&spec);
        assert!(yaml.contains("password: \"dbpass\""));
        assert!(yaml.contains("username: \"keycloak\""));
    }

    #[test]
    fn test_parse_secrets_config() {
        let yaml = r#"
keycloak:
  admin_password: "test-pass"
  db_password: "db-pass"
argocd:
  repo_pat: "ghp_test"
cloudflare:
  credentials_file: "/path/to/creds.json"
"#;
        let config = parse_secrets_config(yaml).unwrap();
        assert_eq!(config.keycloak.admin_password, "test-pass");
        assert_eq!(config.keycloak.db_password, "db-pass");
        assert_eq!(config.argocd.repo_pat, "ghp_test");
        assert_eq!(config.cloudflare.credentials_file, "/path/to/creds.json");
    }

    #[test]
    fn test_parse_secrets_config_minimal() {
        // Only keycloak is required; others have defaults
        let yaml = r#"
keycloak:
  admin_password: "pass"
  db_password: "dbpass"
"#;
        let config = parse_secrets_config(yaml).unwrap();
        assert_eq!(config.keycloak.admin_password, "pass");
        assert!(config.argocd.repo_pat.is_empty());
        assert!(config.cloudflare.credentials_file.is_empty());
    }

    #[test]
    fn test_parse_secrets_config_invalid() {
        let result = parse_secrets_config("not: valid: yaml: [");
        assert!(result.is_err());
    }

    #[test]
    fn test_secrets_for_management_cluster() {
        let secrets = make_secrets();
        let specs = secrets_for_cluster("management", &secrets);

        assert_eq!(specs.len(), 3, "management cluster needs 3 secrets");

        // keycloak-admin
        assert_eq!(specs[0].name, "keycloak-admin");
        assert_eq!(specs[0].namespace, "keycloak");
        assert_eq!(specs[0].data[0].0, "admin-password");

        // keycloak-db
        assert_eq!(specs[1].name, "keycloak-db");
        assert_eq!(specs[1].namespace, "keycloak");

        // cloudflared-tunnel-credentials
        assert_eq!(specs[2].name, "cloudflared-tunnel-credentials");
        assert_eq!(specs[2].namespace, "kube-tunnel");
    }

    #[test]
    fn test_secrets_for_workload_cluster() {
        let secrets = make_secrets();
        let specs = secrets_for_cluster("workload", &secrets);
        assert!(
            specs.is_empty(),
            "workload clusters need no pre-bootstrap secrets"
        );
    }

    #[test]
    fn test_secrets_for_management_no_cloudflare() {
        let secrets = SecretsConfig {
            keycloak: KeycloakSecrets {
                admin_password: "pass".to_string(),
                db_password: "db".to_string(),
            },
            argocd: ArgocdSecrets::default(),
            cloudflare: CloudflareSecrets::default(),
        };
        let specs = secrets_for_cluster("management", &secrets);
        assert_eq!(specs.len(), 2, "without cloudflare, only 2 secrets needed");
        assert!(specs
            .iter()
            .all(|s| s.name != "cloudflared-tunnel-credentials"));
    }

    /// Verify the example file content can be parsed
    #[test]
    fn test_parse_secrets_example_content() {
        let example = r#"
keycloak:
  admin_password: "<CHANGE_ME>"
  db_password: "<CHANGE_ME>"
argocd:
  repo_pat: ""
cloudflare:
  credentials_file: "credentials/cloudflare-tunnel.json"
  cert_file: ""
"#;
        let config = parse_secrets_config(example).unwrap();
        assert_eq!(config.keycloak.admin_password, "<CHANGE_ME>");
        assert_eq!(
            config.cloudflare.credentials_file,
            "credentials/cloudflare-tunnel.json"
        );
    }
}
