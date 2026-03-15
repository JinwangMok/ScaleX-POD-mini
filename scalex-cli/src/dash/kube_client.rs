use anyhow::{Context, Result};
use kube::Client;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct ClusterClient {
    pub name: String,
    #[allow(dead_code)]
    pub kubeconfig_path: PathBuf,
    pub client: Client,
    /// SSH tunnel process ID (if auto-tunneled)
    pub tunnel_pid: Option<u32>,
}

/// Discover kubeconfig files from the given directory.
/// Expects structure: `{dir}/{cluster_name}/kubeconfig.yaml`
///
/// For each kubeconfig, checks if the K8s API server is reachable.
/// If not, automatically sets up an SSH tunnel through the bastion node
/// (from credentials/.baremetal-init.yaml) so the bastion can access
/// remote cluster APIs without manual tunnel setup.
pub async fn discover_clusters(dir: &Path) -> Result<Vec<ClusterClient>> {
    let mut clusters = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(clusters),
        Err(e) => return Err(e).context(format!("Reading kubeconfig dir: {}", dir.display())),
    };

    // Resolve bastion info once (for auto-tunneling)
    let repo_root = dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(Path::new("."));
    let bastion = resolve_bastion(repo_root);
    let mut next_local_port: u16 = 16443;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let kubeconfig_path = path.join("kubeconfig.yaml");
        if !kubeconfig_path.exists() {
            continue;
        }

        let cluster_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Try direct connection first
        match build_client(&kubeconfig_path).await {
            Ok(client) => {
                // Verify connectivity with a quick API call
                if kube::api::Api::<k8s_openapi::api::core::v1::Namespace>::all(client.clone())
                    .list(&kube::api::ListParams::default().limit(1))
                    .await
                    .is_ok()
                {
                    clusters.push(ClusterClient {
                        name: cluster_name,
                        kubeconfig_path,
                        client,
                        tunnel_pid: None,
                    });
                    continue;
                }
                // Direct connection built but API unreachable — fall through to tunnel
            }
            Err(_) => {
                // Kubeconfig load/client build failed — fall through to tunnel
            }
        }

        // Auto-tunnel: set up SSH port forward through bastion
        if let Some(ref bastion_host) = bastion {
            match setup_auto_tunnel(
                &kubeconfig_path,
                &cluster_name,
                bastion_host,
                next_local_port,
            )
            .await
            {
                Ok((client, pid)) => {
                    eprintln!(
                        "Auto-tunnel: {} → localhost:{} via {}",
                        cluster_name, next_local_port, bastion_host
                    );
                    clusters.push(ClusterClient {
                        name: cluster_name,
                        kubeconfig_path,
                        client,
                        tunnel_pid: Some(pid),
                    });
                    next_local_port += 1;
                    continue;
                }
                Err(e) => {
                    eprintln!("Warning: Auto-tunnel failed for {}: {}", cluster_name, e);
                }
            }
        } else {
            eprintln!(
                "Warning: Cannot reach {} and no bastion available for auto-tunnel",
                cluster_name
            );
        }
    }

    clusters.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(clusters)
}

/// Build a kube Client from a kubeconfig file.
async fn build_client(kubeconfig_path: &Path) -> Result<Client> {
    let kubeconfig = kube::config::Kubeconfig::read_from(kubeconfig_path)
        .context(format!("Reading {}", kubeconfig_path.display()))?;

    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default())
        .await
        .context("Building kube config")?;

    Client::try_from(config).context("Creating kube client")
}

/// Build a kube Client from kubeconfig content string.
async fn build_client_from_content(content: &str) -> Result<Client> {
    let kubeconfig: kube::config::Kubeconfig =
        serde_yaml::from_str(content).context("Parsing modified kubeconfig")?;

    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default())
        .await
        .context("Building kube config from modified content")?;

    Client::try_from(config).context("Creating kube client")
}

/// Resolve bastion hostname from credentials/.baremetal-init.yaml.
/// Returns the first node name (which should match ~/.ssh/config).
fn resolve_bastion(repo_root: &Path) -> Option<String> {
    let bm_yaml = repo_root.join("credentials/.baremetal-init.yaml");
    let content = std::fs::read_to_string(&bm_yaml).ok()?;
    // Extract first node name from YAML
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- name:") || trimmed.starts_with("name:") {
            let name = trimmed
                .split(':')
                .nth(1)?
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Extract the server URL from a kubeconfig file.
fn extract_server_url(kubeconfig_path: &Path) -> Option<(String, String, u16)> {
    let content = std::fs::read_to_string(kubeconfig_path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("server:") {
            let url = trimmed
                .split_whitespace()
                .nth(1)?
                .trim_matches('"')
                .to_string();
            // Parse: https://IP:PORT
            let without_scheme = url.strip_prefix("https://")?;
            let parts: Vec<&str> = without_scheme.splitn(2, ':').collect();
            let ip = parts[0].to_string();
            let port: u16 = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(6443);
            return Some((url, ip, port));
        }
    }
    None
}

/// Set up an SSH tunnel and return a Client connected through it.
async fn setup_auto_tunnel(
    kubeconfig_path: &Path,
    cluster_name: &str,
    bastion_host: &str,
    local_port: u16,
) -> Result<(Client, u32)> {
    let (server_url, server_ip, server_port) =
        extract_server_url(kubeconfig_path).context("Cannot parse server URL from kubeconfig")?;

    // Determine the target IP: if 127.0.0.1, look up from SDI state
    let target_ip = if server_ip == "127.0.0.1" || server_ip == "localhost" {
        lookup_cp_ip(kubeconfig_path, cluster_name).unwrap_or(server_ip)
    } else {
        server_ip
    };

    // Start SSH tunnel: localhost:local_port → target_ip:server_port via bastion
    let child = std::process::Command::new("ssh")
        .args([
            "-N",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=30",
            "-L",
            &format!("{}:{}:{}", local_port, target_ip, server_port),
            bastion_host,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context(format!(
            "SSH tunnel to {} via {}",
            cluster_name, bastion_host
        ))?;

    let pid = child.id();

    // Wait for tunnel to establish
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    // Verify tunnel process is alive
    if !is_process_alive(pid) {
        anyhow::bail!("SSH tunnel process died immediately (PID {})", pid);
    }

    // Create modified kubeconfig with localhost:local_port
    let content = std::fs::read_to_string(kubeconfig_path)
        .context("Reading kubeconfig for tunnel rewrite")?;
    let modified = content.replace(&server_url, &format!("https://127.0.0.1:{}", local_port));

    let client = build_client_from_content(&modified).await?;

    Ok((client, pid))
}

/// Look up the control plane IP from SDI state for a given cluster.
fn lookup_cp_ip(kubeconfig_path: &Path, cluster_name: &str) -> Option<String> {
    let sdi_state_path = kubeconfig_path
        .parent()?
        .parent()?
        .parent()?
        .join("_generated/sdi/sdi-state.json");

    let content = std::fs::read_to_string(&sdi_state_path).ok()?;
    let pools: Vec<serde_json::Value> = serde_json::from_str(&content)
        .or_else(|_| serde_json::from_str::<serde_json::Value>(&content).map(|v| vec![v]))
        .ok()?;

    for pool in &pools {
        for node in pool.get("nodes")?.as_array()? {
            let node_name = node.get("node_name")?.as_str()?;
            if node_name.starts_with(&format!("{}-cp", cluster_name)) {
                return node.get("ip")?.as_str().map(String::from);
            }
        }
    }
    None
}

/// Check if a process is still alive.
fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Clean up SSH tunnel processes when the app exits.
pub fn cleanup_tunnels(clusters: &[ClusterClient]) {
    for cluster in clusters {
        if let Some(pid) = cluster.tunnel_pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_empty_for_missing_dir() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(discover_clusters(Path::new("/nonexistent/path")));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn discover_returns_empty_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(discover_clusters(dir.path()));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn resolve_bastion_from_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let creds_dir = dir.path().join("credentials");
        std::fs::create_dir_all(&creds_dir).unwrap();
        std::fs::write(
            creds_dir.join(".baremetal-init.yaml"),
            "targetNodes:\n  - name: \"playbox-0\"\n    node_ip: \"192.168.88.8\"\n",
        )
        .unwrap();
        let result = resolve_bastion(dir.path());
        assert_eq!(result, Some("playbox-0".to_string()));
    }

    #[test]
    fn extract_server_url_parses_kubeconfig() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("kubeconfig.yaml");
        std::fs::write(
            &kc,
            "clusters:\n- cluster:\n    server: https://192.168.88.100:6443\n",
        )
        .unwrap();
        let result = extract_server_url(&kc);
        assert!(result.is_some());
        let (url, ip, port) = result.unwrap();
        assert_eq!(url, "https://192.168.88.100:6443");
        assert_eq!(ip, "192.168.88.100");
        assert_eq!(port, 6443);
    }

    #[test]
    fn lookup_cp_ip_returns_none_for_missing_state() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("kubeconfig.yaml");
        std::fs::write(&kc, "").unwrap();
        assert!(lookup_cp_ip(&kc, "tower").is_none());
    }
}
