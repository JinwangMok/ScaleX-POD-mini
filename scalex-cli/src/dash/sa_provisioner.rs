use anyhow::{Context, Result};
use std::path::Path;

const SA_MANIFEST: &str = r#"apiVersion: v1
kind: Namespace
metadata:
  name: scalex-system
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: scalex-dash
  namespace: scalex-system
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: scalex-dash-admin
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: cluster-admin
subjects:
  - kind: ServiceAccount
    name: scalex-dash
    namespace: scalex-system
---
apiVersion: v1
kind: Secret
metadata:
  name: scalex-dash-token
  namespace: scalex-system
  annotations:
    kubernetes.io/service-account.name: scalex-dash
type: kubernetes.io/service-account-token
"#;

/// Read cached SA token from `_generated/clusters/{name}/dash-token`.
pub fn read_cached_token(kubeconfig_path: &Path) -> Option<String> {
    let token_path = kubeconfig_path.parent()?.join("dash-token");
    std::fs::read_to_string(token_path)
        .ok()
        .map(|t| t.trim().to_string())
}

/// Cache SA token to `_generated/clusters/{name}/dash-token`.
pub fn cache_token(kubeconfig_path: &Path, token: &str) -> Result<()> {
    let token_path = kubeconfig_path
        .parent()
        .context("No parent dir for kubeconfig")?
        .join("dash-token");
    std::fs::write(&token_path, token)
        .context(format!("Writing dash-token to {}", token_path.display()))
}

/// Provision `scalex-dash` ServiceAccount on a cluster via SSH and return the bearer token.
/// Connects through bastion using ProxyJump to the control plane IP (extracted from kubeconfig).
pub async fn provision_dash_sa(
    kubeconfig_path: &Path,
    cluster_name: &str,
    bastion: &str,
) -> Result<String> {
    let cp_ip = extract_cp_ip(kubeconfig_path)?;

    // Step 1: Apply SA manifest via kubectl
    let apply_output = std::process::Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            &format!("ProxyJump={bastion}"),
            &format!("ubuntu@{cp_ip}"),
            "sudo kubectl apply -f -",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Spawning SSH for SA provisioning")?;

    // Write manifest to stdin
    use std::io::Write;
    let mut child = apply_output;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(SA_MANIFEST.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "kubectl apply failed for {}: {}",
            cluster_name,
            stderr.trim()
        );
    }

    // Step 2: Wait briefly for token controller to populate the secret
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Step 3: Extract token
    let token_output = std::process::Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", "BatchMode=yes",
            "-o", &format!("ProxyJump={bastion}"),
            &format!("ubuntu@{cp_ip}"),
            "sudo kubectl get secret scalex-dash-token -n scalex-system -o jsonpath='{.data.token}' | base64 -d",
        ])
        .output()
        .context("SSH to extract SA token")?;

    if !token_output.status.success() {
        let stderr = String::from_utf8_lossy(&token_output.stderr);
        anyhow::bail!(
            "Token extraction failed for {}: {}",
            cluster_name,
            stderr.trim()
        );
    }

    let token = String::from_utf8_lossy(&token_output.stdout)
        .trim()
        .to_string();
    if token.is_empty() {
        anyhow::bail!("Empty token returned for {}", cluster_name);
    }

    Ok(token)
}

/// Extract control plane IP from kubeconfig's server URL.
/// Falls back to .original kubeconfig if primary has a domain URL.
fn extract_cp_ip(kubeconfig_path: &Path) -> Result<String> {
    // Try .original first (has VM IP even after domain rewrite)
    let original = kubeconfig_path.with_extension("yaml.original");
    let path = if original.exists() {
        &original
    } else {
        kubeconfig_path
    };

    let content =
        std::fs::read_to_string(path).context(format!("Reading kubeconfig {}", path.display()))?;

    // Parse server: https://IP:PORT
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("server:") {
            let url = trimmed
                .trim_start_matches("server:")
                .trim()
                .trim_matches('"');
            let host = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
                .unwrap_or(url)
                .split(':')
                .next()
                .unwrap_or("");
            // Only return if it's an IP address, not a domain
            if !host.is_empty() && host.parse::<std::net::IpAddr>().is_ok() {
                return Ok(host.to_string());
            }
        }
    }

    anyhow::bail!("Cannot extract control plane IP from {}", path.display())
}
