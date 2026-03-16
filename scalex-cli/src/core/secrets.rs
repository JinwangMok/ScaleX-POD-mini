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
    pub tunnel_token: String,
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

/// Check if a credentials_file value looks like a file path (not inline JSON content).
/// Pure function.
pub fn is_credentials_file_path(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let trimmed = value.trim();
    // Inline JSON starts with '{', file paths don't
    !trimmed.starts_with('{')
}

/// Generate a Kubernetes Secret YAML manifest from a spec.
/// Pure function: no I/O. Uses block scalar (|) for values containing
/// quotes or newlines to prevent YAML corruption.
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
        if value.contains('"') || value.contains('\n') || value.contains('\\') {
            // Use YAML block scalar (|) for values with special characters
            yaml.push_str(&format!("  {key}: |\n"));
            for line in value.lines() {
                yaml.push_str(&format!("    {line}\n"));
            }
        } else {
            yaml.push_str(&format!("  {key}: \"{value}\"\n"));
        }
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
                    data: vec![
                        ("username".to_string(), "keycloak".to_string()),
                        ("password".to_string(), secrets.keycloak.db_password.clone()),
                    ],
                },
            ];
            // Cloudflare tunnel token (token-based auth)
            if !secrets.cloudflare.tunnel_token.is_empty() {
                specs.push(K8sSecretSpec {
                    name: "cloudflared-tunnel-token".to_string(),
                    namespace: "kube-tunnel".to_string(),
                    data: vec![(
                        "token".to_string(),
                        secrets.cloudflare.tunnel_token.clone(),
                    )],
                });
            }
            specs
        }
        _ => vec![], // Workload clusters don't need pre-bootstrap secrets
    }
}

/// Orchestrate: parse secrets YAML → generate all K8s Secret manifests for a cluster role.
/// Returns multi-document YAML string (separated by ---). Pure function.
pub fn generate_all_secrets_manifests(
    yaml_content: &str,
    cluster_role: &str,
) -> Result<String, String> {
    let config = parse_secrets_config(yaml_content)?;
    let specs = secrets_for_cluster(cluster_role, &config);
    if specs.is_empty() {
        return Ok(String::new());
    }
    let manifests: Vec<String> = specs.iter().map(generate_k8s_secret_yaml).collect();
    Ok(manifests.join("---\n"))
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
                credentials_file: String::new(),
                tunnel_token: "cf-tunnel-token-abc123".to_string(),
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

        // cloudflared-tunnel-token
        assert_eq!(specs[2].name, "cloudflared-tunnel-token");
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
            .all(|s| s.name != "cloudflared-tunnel-token"));
    }

    #[test]
    fn test_generate_all_secrets_manifests_management() {
        let yaml = r#"
keycloak:
  admin_password: "admin-pass"
  db_password: "db-pass"
argocd:
  repo_pat: ""
cloudflare:
  credentials_file: ""
"#;
        let result = generate_all_secrets_manifests(yaml, "management").unwrap();
        // management without cloudflare = 2 secrets separated by ---
        assert!(result.contains("keycloak-admin"));
        assert!(result.contains("keycloak-db"));
        assert!(result.contains("admin-pass"));
        assert!(result.contains("db-pass"));
        assert!(result.contains("---"));
        // Should be valid multi-doc YAML
        let docs: Vec<&str> = result
            .split("---")
            .filter(|s: &&str| !s.trim().is_empty())
            .collect();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn test_generate_all_secrets_manifests_workload() {
        let yaml = r#"
keycloak:
  admin_password: "pass"
  db_password: "db"
"#;
        let result = generate_all_secrets_manifests(yaml, "workload").unwrap();
        assert!(result.is_empty(), "workload clusters need no secrets");
    }

    #[test]
    fn test_generate_all_secrets_manifests_invalid_yaml() {
        let result = generate_all_secrets_manifests("not: valid: [", "management");
        assert!(result.is_err());
    }

    // --- Sprint 40: cloudflare credentials file resolution + YAML escaping ---

    #[test]
    fn test_is_credentials_file_path_detects_path() {
        assert!(is_credentials_file_path(
            "credentials/cloudflare-tunnel.json"
        ));
        assert!(is_credentials_file_path("/path/to/creds.json"));
        assert!(is_credentials_file_path("./creds.json"));
    }

    #[test]
    fn test_is_credentials_file_path_detects_inline_json() {
        assert!(!is_credentials_file_path(
            r#"{"AccountTag":"abc","TunnelSecret":"xyz","TunnelID":"123"}"#
        ));
        assert!(!is_credentials_file_path(""));
    }

    #[test]
    fn test_generate_k8s_secret_yaml_json_value_uses_block_scalar() {
        let spec = K8sSecretSpec {
            name: "cf-creds".to_string(),
            namespace: "kube-tunnel".to_string(),
            data: vec![(
                "credentials.json".to_string(),
                r#"{"AccountTag":"abc","TunnelSecret":"s3c","TunnelID":"123"}"#.to_string(),
            )],
        };
        let yaml = generate_k8s_secret_yaml(&spec);
        // Must produce valid YAML even with JSON containing quotes
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap_or_else(|e| {
            panic!("generated secret with JSON value is not valid YAML: {e}\n{yaml}")
        });
        assert!(parsed.is_mapping());
        // The JSON content must be preserved intact
        let sd = parsed["stringData"]["credentials.json"].as_str().unwrap();
        assert!(sd.contains("AccountTag"));
        assert!(sd.contains("abc"));
    }

    #[test]
    fn test_generate_k8s_secret_yaml_value_with_quotes() {
        let spec = K8sSecretSpec {
            name: "test".to_string(),
            namespace: "default".to_string(),
            data: vec![(
                "key".to_string(),
                r#"value with "quotes" inside"#.to_string(),
            )],
        };
        let yaml = generate_k8s_secret_yaml(&spec);
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml)
            .unwrap_or_else(|e| panic!("YAML with quotes should be valid: {e}\n{yaml}"));
        let val = parsed["stringData"]["key"].as_str().unwrap();
        assert!(
            val.contains("\"quotes\""),
            "quotes must be preserved in value"
        );
    }

    #[test]
    fn test_generate_k8s_secret_yaml_value_with_newlines() {
        let spec = K8sSecretSpec {
            name: "test".to_string(),
            namespace: "default".to_string(),
            data: vec![("key".to_string(), "line1\nline2\nline3".to_string())],
        };
        let yaml = generate_k8s_secret_yaml(&spec);
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml)
            .unwrap_or_else(|e| panic!("YAML with newlines should be valid: {e}\n{yaml}"));
        let val = parsed["stringData"]["key"].as_str().unwrap();
        assert!(val.contains("line1"), "multiline content must be preserved");
        assert!(val.contains("line2"), "multiline content must be preserved");
    }

    // --- Sprint 32c: secrets edge case tests ---

    #[test]
    fn test_secrets_for_unknown_role_returns_empty() {
        let secrets = make_secrets();
        assert!(secrets_for_cluster("staging", &secrets).is_empty());
        assert!(secrets_for_cluster("", &secrets).is_empty());
        assert!(secrets_for_cluster("MANAGEMENT", &secrets).is_empty()); // case-sensitive
    }

    #[test]
    fn test_generate_k8s_secret_yaml_empty_data() {
        let spec = K8sSecretSpec {
            name: "empty-secret".to_string(),
            namespace: "default".to_string(),
            data: vec![],
        };
        let yaml = generate_k8s_secret_yaml(&spec);
        assert!(yaml.contains("stringData:"));
        // Valid YAML even with no data entries
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert!(parsed.is_mapping());
    }

    #[test]
    fn test_generate_k8s_secret_yaml_json_value() {
        // Cloudflare credentials are JSON strings stored as secret values
        let spec = K8sSecretSpec {
            name: "cf-creds".to_string(),
            namespace: "kube-tunnel".to_string(),
            data: vec![(
                "credentials.json".to_string(),
                r#"{"AccountTag":"abc","TunnelSecret":"s3c","TunnelID":"123"}"#.to_string(),
            )],
        };
        let yaml = generate_k8s_secret_yaml(&spec);
        assert!(yaml.contains("credentials.json:"));
        assert!(yaml.contains("AccountTag"));
    }

    #[test]
    fn test_parse_secrets_config_extra_fields_ignored() {
        // Forward compatibility: extra unknown fields should not break parsing
        let yaml = r#"
keycloak:
  admin_password: "pass"
  db_password: "db"
  extra_field: "ignored"
argocd:
  repo_pat: "pat"
  unknown: true
cloudflare:
  credentials_file: "creds"
  cert_file: ""
  future_field: 42
"#;
        // serde_yaml with default derives will fail on unknown fields
        // unless deny_unknown_fields is NOT set — verify our structs handle this
        let result = parse_secrets_config(yaml);
        // This tests the actual behavior — if it fails, we need #[serde(deny_unknown_fields)]
        // to be removed (which is the default, so it should pass)
        assert!(result.is_ok(), "extra fields should be silently ignored");
    }

    #[test]
    fn test_generate_all_manifests_management_with_cloudflare() {
        let yaml = r#"
keycloak:
  admin_password: "admin"
  db_password: "db"
cloudflare:
  tunnel_token: "my-cf-tunnel-token"
"#;
        let result = generate_all_secrets_manifests(yaml, "management").unwrap();
        let docs: Vec<&str> = result
            .split("---")
            .filter(|s| !s.trim().is_empty())
            .collect();
        assert_eq!(docs.len(), 3, "management + cloudflare = 3 secrets");
        assert!(result.contains("cloudflared-tunnel-token"));
    }

    #[test]
    fn test_parse_secrets_config_missing_required_field() {
        // keycloak.admin_password is required
        let yaml = r#"
keycloak:
  db_password: "db"
"#;
        let result = parse_secrets_config(yaml);
        assert!(result.is_err(), "missing admin_password should fail");
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
  tunnel_token: ""
  credentials_file: "credentials/cloudflare-tunnel.json"
  cert_file: ""
"#;
        let config = parse_secrets_config(example).unwrap();
        assert_eq!(config.keycloak.admin_password, "<CHANGE_ME>");
        assert_eq!(
            config.cloudflare.credentials_file,
            "credentials/cloudflare-tunnel.json"
        );
        assert!(config.cloudflare.tunnel_token.is_empty());
    }
}
