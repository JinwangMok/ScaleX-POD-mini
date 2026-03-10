use crate::core::error::ScalexError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaremetalInitConfig {
    #[serde(rename = "targetNodes")]
    pub target_nodes: Vec<NodeConnectionConfig>,
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
