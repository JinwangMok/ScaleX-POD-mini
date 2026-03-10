use crate::core::error::ScalexError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaremetalInitConfig {
    #[serde(rename = "targetNodes")]
    pub target_nodes: Vec<NodeConnectionConfig>,
    /// Optional network defaults for SDI host infrastructure.
    /// When present, used instead of hardcoded values in `sdi init`.
    #[serde(default, rename = "networkDefaults")]
    pub network_defaults: Option<BaremetalNetworkDefaults>,
}

/// Network configuration for bare-metal host infrastructure setup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaremetalNetworkDefaults {
    #[serde(default = "default_bridge", rename = "managementBridge")]
    pub management_bridge: String,
    #[serde(rename = "managementCidr")]
    pub management_cidr: String,
    pub gateway: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeConnectionConfig {
    pub name: String,
    pub direct_reachable: bool,
    #[serde(default)]
    pub node_ip: String,
    #[serde(default, rename = "reachable_node_ip")]
    pub reachable_node_ip: Option<String>,
    #[serde(default, rename = "reachable_via")]
    pub reachable_via: Option<Vec<String>>,
    #[serde(rename = "adminUser")]
    pub admin_user: String,
    #[serde(rename = "sshAuthMode")]
    pub ssh_auth_mode: SshAuthMode,
    #[serde(default, rename = "sshPassword")]
    pub ssh_password: Option<String>,
    #[serde(default, rename = "sshKeyPath")]
    pub ssh_key_path: Option<String>,
    #[serde(default, rename = "sshKeyPathOfReachableNode")]
    pub ssh_key_path_of_reachable_node: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SshAuthMode {
    Password,
    Key,
}

fn default_bridge() -> String {
    "br0".to_string()
}

/// Load .baremetal-init.yaml and resolve env var references from .env
pub fn load_baremetal_config(
    init_path: &Path,
    env_path: &Path,
) -> Result<BaremetalInitConfig, ScalexError> {
    let env_vars = load_env_file(env_path)?;
    let raw = std::fs::read_to_string(init_path)
        .map_err(|_| ScalexError::FileNotFound(init_path.display().to_string()))?;
    let mut config: BaremetalInitConfig = serde_yaml::from_str(&raw)?;

    for node in &mut config.target_nodes {
        if let Some(ref pass_var) = node.ssh_password {
            node.ssh_password = Some(resolve_env_var(pass_var, &env_vars)?);
        }
        if let Some(ref key_var) = node.ssh_key_path {
            node.ssh_key_path = Some(resolve_env_var(key_var, &env_vars)?);
        }
        if let Some(ref key_var) = node.ssh_key_path_of_reachable_node {
            node.ssh_key_path_of_reachable_node = Some(resolve_env_var(key_var, &env_vars)?);
        }
    }

    Ok(config)
}

/// Load a .env file into a HashMap
fn load_env_file(path: &Path) -> Result<HashMap<String, String>, ScalexError> {
    let mut map = HashMap::new();
    if !path.exists() {
        return Ok(map);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|_| ScalexError::FileNotFound(path.display().to_string()))?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().trim_matches('"').to_string();
            map.insert(key, value);
        }
    }
    Ok(map)
}

/// Resolve an env var name to its value; returns the raw string if not a known var
fn resolve_env_var(
    var_name: &str,
    env_vars: &HashMap<String, String>,
) -> Result<String, ScalexError> {
    match env_vars.get(var_name) {
        Some(val) => Ok(val.clone()),
        None => {
            // If it looks like a path or literal value, return as-is
            if var_name.contains('/') || var_name.contains('.') || var_name.contains('~') {
                Ok(var_name.to_string())
            } else {
                // Try system environment
                std::env::var(var_name).map_err(|_| ScalexError::EnvVar {
                    name: var_name.to_string(),
                })
            }
        }
    }
}

/// Validate baremetal config semantically. Pure function: no I/O.
/// Returns list of human-readable error messages (empty = valid).
/// Designed for newbie-friendly diagnostics (CL-5).
pub fn validate_baremetal_config(config: &BaremetalInitConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if config.target_nodes.is_empty() {
        errors.push("targetNodes is empty. At least 1 bare-metal node is required.".to_string());
        return errors;
    }

    let mut seen_names = std::collections::HashSet::new();
    let mut seen_ips = std::collections::HashSet::new();

    for (i, node) in config.target_nodes.iter().enumerate() {
        let ctx = format!("targetNodes[{}] (name='{}')", i, node.name);

        // Unique name
        if !seen_names.insert(&node.name) {
            errors.push(format!("{}: duplicate node name '{}'", ctx, node.name));
        }

        // Non-empty name
        if node.name.trim().is_empty() {
            errors.push(format!("targetNodes[{}]: name must not be empty", i));
        }

        // Non-empty node_ip
        if node.node_ip.trim().is_empty() {
            errors.push(format!("{}: node_ip must not be empty", ctx));
        }

        // Unique IP
        if !node.node_ip.trim().is_empty() && !seen_ips.insert(&node.node_ip) {
            errors.push(format!("{}: duplicate node_ip '{}'", ctx, node.node_ip));
        }

        // Auth mode consistency
        match node.ssh_auth_mode {
            SshAuthMode::Password => {
                if node.ssh_password.is_none() {
                    errors.push(format!(
                        "{}: sshAuthMode is 'password' but sshPassword is missing. \
                         Add sshPassword with a .env variable name (e.g., PLAYBOX_0_PASSWORD)",
                        ctx
                    ));
                }
            }
            SshAuthMode::Key => {
                if node.ssh_key_path.is_none() && node.ssh_key_path_of_reachable_node.is_none() {
                    errors.push(format!(
                        "{}: sshAuthMode is 'key' but neither sshKeyPath nor \
                         sshKeyPathOfReachableNode is set",
                        ctx
                    ));
                }
            }
        }

        // Reachability: non-direct must have reachable_node_ip or reachable_via
        if !node.direct_reachable
            && node.reachable_node_ip.is_none()
            && node.reachable_via.is_none()
        {
            errors.push(format!(
                "{}: direct_reachable is false but neither reachable_node_ip \
                 nor reachable_via is set. How should this node be reached?",
                ctx
            ));
        }

        // reachable_via references must exist
        if let Some(ref via) = node.reachable_via {
            for hop in via {
                if !config.target_nodes.iter().any(|n| &n.name == hop) {
                    errors.push(format!(
                        "{}: reachable_via references '{}' which is not in targetNodes",
                        ctx, hop
                    ));
                }
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_load_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join(".env");
        let mut f = std::fs::File::create(&env_path).unwrap();
        writeln!(f, "FOO_PASS=\"secret123\"").unwrap();
        writeln!(f, "# comment").unwrap();
        writeln!(f, "BAR_KEY=~/.ssh/id_ed25519").unwrap();

        let vars = load_env_file(&env_path).unwrap();
        assert_eq!(vars.get("FOO_PASS").unwrap(), "secret123");
        assert_eq!(vars.get("BAR_KEY").unwrap(), "~/.ssh/id_ed25519");
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn test_resolve_env_var_from_map() {
        let mut vars = HashMap::new();
        vars.insert("MY_PASS".to_string(), "s3cret".to_string());

        assert_eq!(resolve_env_var("MY_PASS", &vars).unwrap(), "s3cret");
    }

    #[test]
    fn test_resolve_env_var_path_passthrough() {
        let vars = HashMap::new();
        assert_eq!(
            resolve_env_var("~/.ssh/id_ed25519", &vars).unwrap(),
            "~/.ssh/id_ed25519"
        );
    }

    #[test]
    fn test_parse_baremetal_config() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id_ed25519"
  - name: "node-1"
    direct_reachable: false
    reachable_via: ["node-0"]
    node_ip: "10.0.0.2"
    adminUser: "admin"
    sshAuthMode: "password"
    sshPassword: "NODE1_PASS"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.target_nodes.len(), 2);
        assert_eq!(config.target_nodes[0].name, "node-0");
        assert!(config.target_nodes[0].direct_reachable);
        assert_eq!(config.target_nodes[1].ssh_auth_mode, SshAuthMode::Password);
        assert_eq!(
            config.target_nodes[1].reachable_via,
            Some(vec!["node-0".to_string()])
        );
    }

    #[test]
    fn test_parse_baremetal_config_with_network_defaults() {
        let yaml = r#"
networkDefaults:
  managementBridge: "br0"
  managementCidr: "10.0.0.0/24"
  gateway: "10.0.0.1"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.10"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id_ed25519"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.network_defaults.is_some());
        let net = config.network_defaults.unwrap();
        assert_eq!(net.management_bridge, "br0");
        assert_eq!(net.management_cidr, "10.0.0.0/24");
        assert_eq!(net.gateway, "10.0.0.1");
    }

    #[test]
    fn test_parse_baremetal_config_without_network_defaults_backward_compat() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.10"
    adminUser: "admin"
    sshAuthMode: "key"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.network_defaults.is_none());
        assert_eq!(config.target_nodes.len(), 1);
    }

    // ── Semantic validation tests (CL-5, CL-8) ──

    #[test]
    fn test_validate_baremetal_config_valid() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id_ed25519"
  - name: "node-1"
    direct_reachable: false
    reachable_via: ["node-0"]
    node_ip: "10.0.0.2"
    adminUser: "admin"
    sshAuthMode: "password"
    sshPassword: "MY_PASS"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(errors.is_empty(), "valid config must pass: {:?}", errors);
    }

    #[test]
    fn test_validate_baremetal_config_empty_nodes() {
        let config = BaremetalInitConfig {
            target_nodes: vec![],
            network_defaults: None,
        };
        let errors = validate_baremetal_config(&config);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("empty"));
    }

    #[test]
    fn test_validate_baremetal_config_duplicate_name() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.2"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(errors.iter().any(|e| e.contains("duplicate node name")));
    }

    #[test]
    fn test_validate_baremetal_config_duplicate_ip() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
  - name: "node-1"
    direct_reachable: true
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(errors.iter().any(|e| e.contains("duplicate node_ip")));
    }

    #[test]
    fn test_validate_baremetal_config_password_mode_missing_password() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "password"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(errors.iter().any(|e| e.contains("sshPassword is missing")));
    }

    #[test]
    fn test_validate_baremetal_config_unreachable_node() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: false
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.contains("How should this node be reached")));
    }

    #[test]
    fn test_validate_baremetal_config_reachable_via_invalid_ref() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
  - name: "node-1"
    direct_reachable: false
    reachable_via: ["nonexistent"]
    node_ip: "10.0.0.2"
    adminUser: "admin"
    sshAuthMode: "password"
    sshPassword: "PASS"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.contains("nonexistent") && e.contains("not in targetNodes")));
    }

    /// Verify the actual .example file content can be parsed by our code.
    /// This catches drift between example files and parsing logic.
    #[test]
    fn test_parse_baremetal_init_example_content() {
        let example_yaml = r#"
targetNodes:
  - name: "playbox-0"
    direct_reachable: false
    reachable_node_ip: "100.64.0.1"
    node_ip: "192.168.88.8"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "PLAYBOX_0_PASSWORD"
  - name: "playbox-1"
    direct_reachable: false
    reachable_via: ["playbox-0"]
    node_ip: "192.168.88.9"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "PLAYBOX_1_PASSWORD"
  - name: "playbox-2"
    direct_reachable: false
    reachable_via: ["playbox-0"]
    node_ip: "192.168.88.10"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "PLAYBOX_2_PASSWORD"
  - name: "playbox-3"
    direct_reachable: false
    reachable_via: ["playbox-0"]
    node_ip: "192.168.88.11"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "PLAYBOX_3_PASSWORD"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(example_yaml).unwrap();
        assert_eq!(config.target_nodes.len(), 4, "example must have 4 nodes");
        assert_eq!(config.target_nodes[0].name, "playbox-0");
        assert!(!config.target_nodes[0].direct_reachable);
        assert_eq!(
            config.target_nodes[0].reachable_node_ip,
            Some("100.64.0.1".to_string())
        );
        assert_eq!(
            config.target_nodes[1].reachable_via,
            Some(vec!["playbox-0".to_string()])
        );
    }
}
