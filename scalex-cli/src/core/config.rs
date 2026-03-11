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

            // Self-reference check
            if via.contains(&node.name) {
                errors.push(format!(
                    "{}: reachable_via references itself ('{}') — a node cannot be its own proxy hop",
                    ctx, node.name
                ));
            }
        }

        // Conflicting: direct_reachable=true + reachable_via set
        if node.direct_reachable && node.reachable_via.is_some() {
            errors.push(format!(
                "{}: direct_reachable is true but reachable_via is also set — \
                 remove reachable_via or set direct_reachable to false",
                ctx
            ));
        }

        // IP format validation
        if !node.node_ip.trim().is_empty() && !is_valid_ip(&node.node_ip) {
            errors.push(format!(
                "{}: node_ip '{}' is not a valid IP address",
                ctx, node.node_ip
            ));
        }

        // reachable_node_ip format validation
        if let Some(ref rip) = node.reachable_node_ip {
            if !is_valid_ip(rip) {
                errors.push(format!(
                    "{}: reachable_node_ip '{}' is not a valid IP address",
                    ctx, rip
                ));
            }
        }
    }

    // Circular reachable_via detection: find if any non-direct node can reach a root
    let direct_nodes: std::collections::HashSet<&str> = config
        .target_nodes
        .iter()
        .filter(|n| n.direct_reachable || n.reachable_node_ip.is_some())
        .map(|n| n.name.as_str())
        .collect();

    for node in &config.target_nodes {
        if node.direct_reachable || node.reachable_node_ip.is_some() {
            continue; // Root nodes — always reachable
        }
        if node.reachable_via.is_none() {
            continue; // Already reported as unreachable above
        }
        // Walk the chain to see if we reach a root node
        let mut visited = std::collections::HashSet::new();
        let mut current = node.name.as_str();
        let mut found_root = false;
        while visited.insert(current) {
            if direct_nodes.contains(current) {
                found_root = true;
                break;
            }
            // Find the next hop
            if let Some(hop_node) = config.target_nodes.iter().find(|n| n.name == current) {
                if let Some(ref via) = hop_node.reachable_via {
                    if let Some(next) = via.first() {
                        current = next.as_str();
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        if !found_root {
            errors.push(format!(
                "targetNodes '{}': circular or broken reachable_via chain — \
                 no reachable root node found. Ensure the chain leads to a \
                 direct_reachable or reachable_node_ip node.",
                node.name
            ));
        }
    }

    errors
}

/// Check if a string is a valid IPv4 address. Pure function.
fn is_valid_ip(s: &str) -> bool {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|p| p.parse::<u8>().is_ok())
}

// ── Sprint 38: User-friendly config error helpers (pure functions) ──

/// Format a user-friendly error message when a config file is not found.
/// If an example file path is provided, suggests copying it.
/// Pure function: no I/O.
pub fn format_config_not_found(missing_path: &str, example_path: &str) -> String {
    format!(
        "Config file not found: {}\n\
         \n\
         To get started, copy the example template:\n\
         \n\
         cp {} {}\n\
         \n\
         Then edit the file with your actual values.",
        missing_path, example_path, missing_path
    )
}

/// Validate that a config file exists, returning a user-friendly error if not.
/// Pure function: only checks path existence.
pub fn validate_config_file_exists(path: &str, example_path: Option<&str>) -> Result<(), String> {
    if Path::new(path).exists() {
        Ok(())
    } else {
        match example_path {
            Some(example) => Err(format_config_not_found(path, example)),
            None => Err(format!("Config file not found: {}", path)),
        }
    }
}

/// Format a user-friendly YAML parse error with file context.
/// Pure function.
pub fn format_yaml_parse_error(file_path: &str, err: &serde_yaml::Error) -> String {
    format!(
        "Failed to parse YAML in '{}':\n\
         \n\
         {}\n\
         \n\
         Check indentation and field names against the .example template.",
        file_path, err
    )
}

/// Format multiple validation errors into a single user-friendly message.
/// Returns empty string if there are no errors.
/// Pure function.
pub fn format_validation_errors(context: &str, errors: &[String]) -> String {
    if errors.is_empty() {
        return String::new();
    }
    let mut msg = format!("{} validation found {} error(s):\n", context, errors.len());
    for err in errors {
        msg.push_str(&format!("  - {}\n", err));
    }
    msg
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

    /// Parse the EXACT YAML format from the user's Checklist (CL-8).
    /// All 3 access modes: direct, external IP (Tailscale), ProxyJump.
    /// Includes sshKeyPathOfReachableNode which is the rarest field.
    #[test]
    fn test_parse_checklist_yaml_all_three_access_modes() {
        let yaml = r#"
targetNodes:
  - name: "playbox-0"
    direct_reachable: true
    node_ip: "192.168.88.8"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "PLAYBOX_0_PASSWORD"
    sshKeyPath: "EXAMPLE_SSH_KEY_PATH"
  - name: "playbox-0-ts"
    direct_reachable: false
    reachable_node_ip: "100.64.0.1"
    node_ip: "192.168.88.8"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "PLAYBOX_0_PASSWORD"
    sshKeyPath: "EXAMPLE_SSH_KEY_PATH"
  - name: "playbox-1"
    direct_reachable: false
    reachable_via: ["playbox-0"]
    node_ip: "192.168.88.9"
    adminUser: "jinwang"
    sshAuthMode: "key"
    sshKeyPathOfReachableNode: "EXAMPLE_SSH_KEY_PATH"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.target_nodes.len(), 3);

        // Case 1: direct reachable
        let case1 = &config.target_nodes[0];
        assert!(case1.direct_reachable);
        assert_eq!(case1.admin_user, "jinwang");
        assert_eq!(case1.ssh_auth_mode, SshAuthMode::Password);
        assert_eq!(case1.ssh_password, Some("PLAYBOX_0_PASSWORD".to_string()));
        assert_eq!(case1.ssh_key_path, Some("EXAMPLE_SSH_KEY_PATH".to_string()));

        // Case 2: external IP (Tailscale)
        let case2 = &config.target_nodes[1];
        assert!(!case2.direct_reachable);
        assert_eq!(case2.reachable_node_ip, Some("100.64.0.1".to_string()));
        assert_eq!(case2.node_ip, "192.168.88.8");

        // Case 3: ProxyJump via another node
        let case3 = &config.target_nodes[2];
        assert!(!case3.direct_reachable);
        assert_eq!(case3.reachable_via, Some(vec!["playbox-0".to_string()]));
        assert_eq!(case3.ssh_auth_mode, SshAuthMode::Key);
        assert_eq!(
            case3.ssh_key_path_of_reachable_node,
            Some("EXAMPLE_SSH_KEY_PATH".to_string())
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

    // ── Sprint 38: Config error UX tests ──

    #[test]
    fn test_friendly_error_missing_config_file() {
        let msg = format_config_not_found(
            "config/k8s-clusters.yaml",
            "config/k8s-clusters.yaml.example",
        );
        assert!(
            msg.contains("config/k8s-clusters.yaml"),
            "must mention the missing file"
        );
        assert!(
            msg.contains(".example"),
            "must suggest copying from .example"
        );
        assert!(msg.contains("cp "), "must include cp command");
    }

    #[test]
    fn test_friendly_error_missing_credentials() {
        let msg = format_config_not_found(
            "credentials/.baremetal-init.yaml",
            "credentials/.baremetal-init.yaml.example",
        );
        assert!(msg.contains("credentials/.baremetal-init.yaml"));
        assert!(msg.contains(".example"));
    }

    #[test]
    fn test_friendly_error_missing_env() {
        let msg = format_config_not_found("credentials/.env", "credentials/.env.example");
        assert!(msg.contains("credentials/.env"));
        assert!(msg.contains(".example"));
    }

    #[test]
    fn test_validate_config_file_exists_ok() {
        // Existing file should return Ok
        let result = validate_config_file_exists("Cargo.toml", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_config_file_exists_missing_with_example() {
        let result = validate_config_file_exists(
            "nonexistent/config.yaml",
            Some("config/k8s-clusters.yaml.example"),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("nonexistent/config.yaml"),
            "error must name the missing file"
        );
        assert!(
            err.contains(".example"),
            "error must mention the example file"
        );
    }

    #[test]
    fn test_validate_config_file_exists_missing_no_example() {
        let result = validate_config_file_exists("nonexistent/file.yaml", None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("nonexistent/file.yaml"));
    }

    #[test]
    fn test_format_yaml_parse_error_hint() {
        let bad_yaml = "targetNodes:\n  - name: test\n  bad_indent: true";
        let err = serde_yaml::from_str::<BaremetalInitConfig>(bad_yaml).unwrap_err();
        let msg = format_yaml_parse_error("credentials/.baremetal-init.yaml", &err);
        assert!(
            msg.contains("credentials/.baremetal-init.yaml"),
            "must name the file"
        );
        assert!(msg.contains("YAML"), "must mention YAML");
    }

    #[test]
    fn test_format_validation_errors_empty() {
        let msg = format_validation_errors("SDI spec", &[]);
        assert!(msg.is_empty(), "no errors = no output");
    }

    // ── A-2: Enhanced baremetal validation tests (Sprint 41 TDD) ──

    #[test]
    fn test_validate_baremetal_config_reachable_via_self_reference() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: false
    reachable_via: ["node-0"]
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "password"
    sshPassword: "PASS"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(
            errors.iter().any(|e| e.contains("references itself")),
            "must detect self-reference in reachable_via: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_baremetal_config_reachable_via_circular() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: false
    reachable_via: ["node-1"]
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "password"
    sshPassword: "PASS"
  - name: "node-1"
    direct_reachable: false
    reachable_via: ["node-0"]
    node_ip: "10.0.0.2"
    adminUser: "admin"
    sshAuthMode: "password"
    sshPassword: "PASS"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("circular") || e.contains("no reachable root")),
            "must detect circular reachable_via chain: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_baremetal_config_invalid_ip_format() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    node_ip: "not-an-ip"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(
            errors.iter().any(|e| e.contains("valid IP")),
            "must detect invalid IP format: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_baremetal_config_invalid_reachable_node_ip() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: false
    reachable_node_ip: "bad-ip"
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "password"
    sshPassword: "PASS"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("valid IP") && e.contains("reachable_node_ip")),
            "must detect invalid reachable_node_ip: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_baremetal_config_direct_reachable_with_reachable_via_warns() {
        let yaml = r#"
targetNodes:
  - name: "node-0"
    direct_reachable: true
    reachable_via: ["some-node"]
    node_ip: "10.0.0.1"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
  - name: "some-node"
    direct_reachable: true
    node_ip: "10.0.0.2"
    adminUser: "admin"
    sshAuthMode: "key"
    sshKeyPath: "~/.ssh/id"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_baremetal_config(&config);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("direct_reachable is true") && e.contains("reachable_via")),
            "must warn about conflicting direct_reachable + reachable_via: {:?}",
            errors
        );
    }

    #[test]
    fn test_format_validation_errors_multiple() {
        let errors = vec![
            "Duplicate pool name 'tower'".to_string(),
            "Node 'cp-0' has 0 CPU cores".to_string(),
        ];
        let msg = format_validation_errors("SDI spec", &errors);
        assert!(msg.contains("SDI spec"), "must mention the context");
        assert!(
            msg.contains("Duplicate pool name"),
            "must include first error"
        );
        assert!(msg.contains("0 CPU cores"), "must include second error");
        assert!(msg.contains("2 error(s)"), "must show error count");
    }
}
