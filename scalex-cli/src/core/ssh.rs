use crate::core::config::{NodeConnectionConfig, SshAuthMode};
use crate::core::error::ScalexError;
use std::process::Command;

/// Build an SSH command for a given node configuration.
/// Pure function: returns the command parts without executing.
pub fn build_ssh_command(
    node: &NodeConnectionConfig,
    remote_command: &str,
    all_nodes: &[NodeConnectionConfig],
) -> Result<SshCommand, ScalexError> {
    let _target_ip = resolve_target_ip(node, all_nodes)?;
    let mut args: Vec<String> = vec![
        "-o".to_string(),
        "StrictHostKeyChecking=no".to_string(),
        "-o".to_string(),
        "ConnectTimeout=10".to_string(),
    ];

    // Handle auth mode
    let use_sshpass = match node.ssh_auth_mode {
        SshAuthMode::Password => true,
        SshAuthMode::Key => {
            if let Some(ref key_path) = node.ssh_key_path {
                args.push("-i".to_string());
                args.push(key_path.clone());
            }
            false
        }
    };

    // Handle ProxyJump for non-direct nodes
    if !node.direct_reachable {
        if let Some(ref proxy_ip) = node.reachable_node_ip {
            // Via external IP (e.g., Tailscale) — connect directly to that IP
            args.push(format!("{}@{}", node.admin_user, proxy_ip));
            // Then SSH from there to the actual node
            let inner_cmd = format!(
                "ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 {}@{} '{}'",
                node.admin_user,
                node.node_ip,
                remote_command.replace('\'', "'\\''")
            );
            args.push(inner_cmd);
        } else if let Some(ref via_nodes) = node.reachable_via {
            // Via ProxyJump through another node
            if let Some(proxy_name) = via_nodes.first() {
                let proxy_node = all_nodes
                    .iter()
                    .find(|n| n.name == *proxy_name)
                    .ok_or_else(|| {
                        ScalexError::Config(format!(
                            "ProxyJump node '{}' not found in config",
                            proxy_name
                        ))
                    })?;
                let proxy_ip = resolve_target_ip(proxy_node, all_nodes)?;
                args.push("-o".to_string());
                args.push(format!("ProxyJump={}@{}", proxy_node.admin_user, proxy_ip));
                args.push(format!("{}@{}", node.admin_user, node.node_ip));
                args.push(remote_command.to_string());
            }
        }
    } else {
        args.push(format!("{}@{}", node.admin_user, node.node_ip));
        args.push(remote_command.to_string());
    }

    Ok(SshCommand {
        use_sshpass,
        password: node.ssh_password.clone(),
        args,
    })
}

/// Resolve the IP to connect to for a given node
fn resolve_target_ip(
    node: &NodeConnectionConfig,
    _all_nodes: &[NodeConnectionConfig],
) -> Result<String, ScalexError> {
    if node.direct_reachable {
        Ok(node.node_ip.clone())
    } else if let Some(ref ip) = node.reachable_node_ip {
        Ok(ip.clone())
    } else {
        // For ProxyJump nodes, the SSH command connects to node_ip via proxy
        Ok(node.node_ip.clone())
    }
}

#[derive(Clone, Debug)]
pub struct SshCommand {
    pub use_sshpass: bool,
    pub password: Option<String>,
    pub args: Vec<String>,
}

/// Execute an SSH command and return stdout. This is the IO boundary.
pub fn execute_ssh(cmd: &SshCommand) -> Result<String, ScalexError> {
    let output = if cmd.use_sshpass {
        let password = cmd.password.as_deref().unwrap_or("");
        Command::new("sshpass")
            .arg("-p")
            .arg(password)
            .arg("ssh")
            .args(&cmd.args)
            .output()
            .map_err(|e| ScalexError::Ssh {
                host: "unknown".to_string(),
                detail: format!("sshpass not found or failed: {}", e),
            })?
    } else {
        Command::new("ssh")
            .args(&cmd.args)
            .output()
            .map_err(|e| ScalexError::Ssh {
                host: "unknown".to_string(),
                detail: format!("ssh failed: {}", e),
            })?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ScalexError::Ssh {
            host: cmd.args.last().unwrap_or(&String::new()).clone(),
            detail: stderr.to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::SshAuthMode;

    fn make_node(name: &str, direct: bool, ip: &str) -> NodeConnectionConfig {
        NodeConnectionConfig {
            name: name.to_string(),
            direct_reachable: direct,
            node_ip: ip.to_string(),
            reachable_node_ip: None,
            reachable_via: None,
            admin_user: "admin".to_string(),
            ssh_auth_mode: SshAuthMode::Key,
            ssh_password: None,
            ssh_key_path: Some("~/.ssh/id_ed25519".to_string()),
            ssh_key_path_of_reachable_node: None,
        }
    }

    #[test]
    fn test_build_ssh_direct() {
        let node = make_node("n0", true, "10.0.0.1");
        let cmd = build_ssh_command(&node, "hostname", &[node.clone()]).unwrap();
        assert!(!cmd.use_sshpass);
        assert!(cmd.args.contains(&"admin@10.0.0.1".to_string()));
        assert!(cmd.args.contains(&"hostname".to_string()));
    }

    #[test]
    fn test_build_ssh_proxy_jump() {
        let bastion = make_node("bastion", true, "10.0.0.1");
        let mut worker = make_node("worker", false, "10.0.0.2");
        worker.reachable_via = Some(vec!["bastion".to_string()]);

        let nodes = vec![bastion, worker.clone()];
        let cmd = build_ssh_command(&worker, "uname -r", &nodes).unwrap();
        assert!(cmd.args.iter().any(|a| a.contains("ProxyJump")));
    }
}
