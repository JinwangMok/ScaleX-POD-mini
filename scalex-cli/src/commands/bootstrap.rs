use crate::core::validation::validate_bootstrap_prerequisites;
use crate::models::cluster::K8sClustersConfig;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct BootstrapArgs {
    /// Path to k8s-clusters config file
    #[arg(long, default_value = "config/k8s-clusters.yaml")]
    config: PathBuf,

    /// Path to SDI specs file (for control-plane IP resolution)
    #[arg(long)]
    sdi_spec: Option<String>,

    /// SDI state directory (alternative to --sdi-spec)
    #[arg(long, default_value = "_generated/sdi")]
    sdi_dir: PathBuf,

    /// Clusters output directory (for kubeconfig paths)
    #[arg(long, default_value = "_generated/clusters")]
    clusters_dir: PathBuf,

    /// ArgoCD Helm chart version
    #[arg(long, default_value = "8.1.1")]
    argocd_version: String,

    /// Dry run — show what would be done
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

pub fn run(args: BootstrapArgs) -> anyhow::Result<()> {
    // Pre-flight: verify Helm is installed and accessible (AC 9a)
    match verify_helm_installed() {
        Ok(version) => println!("[bootstrap] Helm {} is installed", version),
        Err(e) => anyhow::bail!("[bootstrap] Helm pre-flight check failed: {}", e),
    }

    println!(
        "[bootstrap] Loading cluster config from {}...",
        args.config.display()
    );

    let raw = std::fs::read_to_string(&args.config)?;
    let k8s_config: K8sClustersConfig = serde_yaml::from_str(&raw)?;

    // Pre-bootstrap validation: check CF credentials
    let cf_creds_path = "credentials/cloudflare-tunnel.json";
    let cf_exists = std::path::Path::new(cf_creds_path).exists();
    let warnings = validate_bootstrap_prerequisites(
        &args.clusters_dir.display().to_string(),
        cf_creds_path,
        cf_exists,
    );
    for w in &warnings {
        eprintln!("[bootstrap] WARNING: {}", w);
    }

    let tower = k8s_config
        .config
        .clusters
        .iter()
        .find(|c| c.cluster_role == "management")
        .ok_or_else(|| anyhow::anyhow!("No management cluster found in config"))?;

    let tower_kubeconfig = args
        .clusters_dir
        .join(&tower.cluster_name)
        .join("kubeconfig.yaml");

    let cilium_version = &k8s_config.config.common.cilium_version;

    // Step 0: Fix /opt/cni/bin permissions on all nodes (kubespray sets kube:root ownership)
    // Cilium init container needs write access to copy cilium-mount binary
    if !args.dry_run {
        println!("[bootstrap] Phase 0: Fixing /opt/cni/bin permissions on all nodes...");
        for cluster in &k8s_config.config.clusters {
            let inventory_path = args
                .clusters_dir
                .join(&cluster.cluster_name)
                .join("inventory.ini");
            if let Ok(content) = std::fs::read_to_string(&inventory_path) {
                for line in content.lines() {
                    if let Some(host_part) = line
                        .split_whitespace()
                        .find(|s| s.starts_with("ansible_host="))
                    {
                        if let Some(ip) = host_part.strip_prefix("ansible_host=") {
                            let ssh_args = content
                                .lines()
                                .find(|l| l.contains(ip))
                                .and_then(|l| l.split("ProxyJump=").nth(1))
                                .map(|pj| pj.trim_end_matches('\'').to_string());
                            let user = content
                                .lines()
                                .find(|l| l.contains(ip))
                                .and_then(|l| {
                                    l.split_whitespace()
                                        .find(|s| s.starts_with("ansible_user="))
                                })
                                .and_then(|s| s.strip_prefix("ansible_user="))
                                .unwrap_or("ubuntu");

                            let mut cmd_args = vec![
                                "-o".to_string(),
                                "StrictHostKeyChecking=no".to_string(),
                                "-o".to_string(),
                                "BatchMode=yes".to_string(),
                            ];
                            if let Some(pj) = &ssh_args {
                                cmd_args.push("-o".to_string());
                                cmd_args.push(format!("ProxyJump={}", pj));
                            }
                            cmd_args.push(format!("{}@{}", user, ip));
                            cmd_args.push("sudo chmod 777 /opt/cni/bin".to_string());

                            let _ = std::process::Command::new("ssh").args(&cmd_args).output();
                        }
                    }
                }
            }
        }
        println!("[bootstrap] /opt/cni/bin permissions fixed");
    }

    // Step 1: Pre-install Cilium CNI on ALL clusters (required before ArgoCD — pods need CNI)
    for cluster in &k8s_config.config.clusters {
        let kubeconfig = args
            .clusters_dir
            .join(&cluster.cluster_name)
            .join("kubeconfig.yaml");
        let values_path = format!("gitops/{}/cilium/values.yaml", cluster.cluster_name);

        println!(
            "[bootstrap] Phase 1: Installing Cilium {} on '{}'...",
            cilium_version, cluster.cluster_name
        );

        let cilium_args = generate_cilium_helm_install_args(
            &kubeconfig.display().to_string(),
            cilium_version,
            &values_path,
        );

        if args.dry_run {
            println!("[dry-run] helm {}", cilium_args.join(" "));
            println!(
                "[dry-run] helm list -n kube-system (verify cilium deployed) on '{}'",
                cluster.cluster_name
            );
        } else {
            run_helm_install(&cilium_args)?;
            // AC 9b: Verify Cilium reached "deployed" state before proceeding.
            verify_helm_release_deployed(
                "cilium",
                "kube-system",
                &kubeconfig.display().to_string(),
                5,
                10,
            )?;
            println!("[bootstrap] Cilium installed on '{}'", cluster.cluster_name);
        }
    }

    // Step 2: Install ArgoCD on tower cluster (with values for kustomize.buildOptions)
    let argocd_values_path = "gitops/tower/argocd/values.yaml";
    println!(
        "[bootstrap] Phase 2: Installing ArgoCD on '{}'...",
        tower.cluster_name
    );

    let helm_args = generate_argocd_helm_install_args(
        &tower_kubeconfig.display().to_string(),
        &args.argocd_version,
        argocd_values_path,
    );

    if args.dry_run {
        println!("[dry-run] helm {}", helm_args.join(" "));
        println!("[dry-run] helm list -n argocd (verify argocd deployed) on tower");
    } else {
        run_helm_install(&helm_args)?;
        // AC 9b: Verify ArgoCD reached "deployed" state before proceeding.
        verify_helm_release_deployed(
            "argocd",
            "argocd",
            &tower_kubeconfig.display().to_string(),
            5,
            10,
        )?;
        println!("[bootstrap] ArgoCD installed on '{}'", tower.cluster_name);
    }

    // Step 3: Register non-management clusters in ArgoCD via cluster Secret
    let managed_clusters: Vec<_> = k8s_config
        .config
        .clusters
        .iter()
        .filter(|c| c.cluster_role != "management")
        .collect();

    for cluster in &managed_clusters {
        println!(
            "[bootstrap] Phase 3: Registering '{}' as remote cluster in ArgoCD...",
            cluster.cluster_name
        );

        let cluster_kubeconfig = args
            .clusters_dir
            .join(&cluster.cluster_name)
            .join("kubeconfig.yaml");

        if args.dry_run {
            println!(
                "[dry-run] kubectl apply cluster-secret for {}",
                cluster.cluster_name
            );
        } else {
            register_cluster_via_secret(
                &tower_kubeconfig.display().to_string(),
                &cluster.cluster_name,
                &cluster_kubeconfig.display().to_string(),
            )?;
            println!(
                "[bootstrap] Registered '{}' in ArgoCD",
                cluster.cluster_name
            );
        }
    }

    // Step 4: Apply spread.yaml
    println!("[bootstrap] Phase 4: Applying GitOps bootstrap (spread.yaml)...");
    let spread_args = generate_kubectl_apply_args(
        &tower_kubeconfig.display().to_string(),
        "gitops/bootstrap/spread.yaml",
    );

    if args.dry_run {
        println!("[dry-run] kubectl {}", spread_args.join(" "));
    } else {
        run_kubectl_apply(&spread_args)?;
        println!("[bootstrap] GitOps bootstrap applied");
    }

    println!("\n[bootstrap] Done. ArgoCD will now sync all applications via spread.yaml.");
    println!(
        "[bootstrap] Monitor progress: kubectl --kubeconfig {} -n argocd get applications -w",
        tower_kubeconfig.display()
    );

    Ok(())
}

/// Generate Helm install arguments for Cilium CNI.
/// Pure function — no I/O, no side effects.
/// Must be installed BEFORE ArgoCD so pods can schedule (CNI required for node Ready).
pub fn generate_cilium_helm_install_args(
    kubeconfig: &str,
    cilium_version: &str,
    values_path: &str,
) -> Vec<String> {
    vec![
        "upgrade".to_string(),
        "--install".to_string(),
        "cilium".to_string(),
        "cilium".to_string(),
        "--repo".to_string(),
        "https://helm.cilium.io/".to_string(),
        "--version".to_string(),
        cilium_version.to_string(),
        "--namespace".to_string(),
        "kube-system".to_string(),
        "--kubeconfig".to_string(),
        kubeconfig.to_string(),
        "--values".to_string(),
        values_path.to_string(),
        "--wait".to_string(),
        "--timeout".to_string(),
        "600s".to_string(),
    ]
}

/// Generate Helm install arguments for ArgoCD.
/// Pure function — no I/O, no side effects.
/// Includes values file for kustomize.buildOptions: --enable-helm.
pub fn generate_argocd_helm_install_args(
    kubeconfig: &str,
    chart_version: &str,
    values_path: &str,
) -> Vec<String> {
    vec![
        "upgrade".to_string(),
        "--install".to_string(),
        "argocd".to_string(),
        "argo-cd".to_string(),
        "--repo".to_string(),
        "https://argoproj.github.io/argo-helm".to_string(),
        "--version".to_string(),
        chart_version.to_string(),
        "--namespace".to_string(),
        "argocd".to_string(),
        "--create-namespace".to_string(),
        "--kubeconfig".to_string(),
        kubeconfig.to_string(),
        "--values".to_string(),
        values_path.to_string(),
        "--wait".to_string(),
        "--timeout".to_string(),
        "600s".to_string(),
    ]
}

/// Register a remote cluster in ArgoCD via a cluster Secret.
/// Reads the target cluster's kubeconfig and creates an ArgoCD cluster Secret
/// on the tower cluster. This avoids needing the argocd CLI or server access.
fn register_cluster_via_secret(
    tower_kubeconfig: &str,
    cluster_name: &str,
    cluster_kubeconfig_path: &str,
) -> anyhow::Result<()> {
    let kc_content = std::fs::read_to_string(cluster_kubeconfig_path)?;
    let kc: serde_yaml::Value = serde_yaml::from_str(&kc_content)?;

    let cluster = &kc["clusters"][0]["cluster"];
    let user = &kc["users"][0]["user"];
    let server = cluster["server"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No server in kubeconfig"))?;
    let ca_data = cluster["certificate-authority-data"].as_str().unwrap_or("");
    let cert_data = user["client-certificate-data"].as_str().unwrap_or("");
    let key_data = user["client-key-data"].as_str().unwrap_or("");

    let config_json = format!(
        r#"{{"tlsClientConfig":{{"caData":"{}","certData":"{}","keyData":"{}"}}}}"#,
        ca_data, cert_data, key_data
    );

    let secret_yaml = format!(
        r#"apiVersion: v1
kind: Secret
metadata:
  name: {cluster_name}-cluster
  namespace: argocd
  labels:
    argocd.argoproj.io/secret-type: cluster
type: Opaque
stringData:
  name: "{cluster_name}"
  server: "{server}"
  config: '{config_json}'
"#
    );

    // Use --server-side apply to avoid last-applied-configuration annotation conflicts
    // with ArgoCD drift detection. --force-conflicts ensures idempotency when field
    // ownership conflicts arise on re-runs.
    let output = std::process::Command::new("kubectl")
        .args([
            "apply",
            "--server-side",
            "--force-conflicts",
            "-f",
            "-",
            "--kubeconfig",
            tower_kubeconfig,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(secret_yaml.as_bytes())?;
            }
            child.wait_with_output()
        });

    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("kubectl apply cluster secret failed: {}", stderr);
        }
        Err(e) => anyhow::bail!("Failed to run kubectl: {}", e),
    }
}

/// Generate kubectl apply arguments for spread.yaml.
/// Pure function — no I/O, no side effects.
/// Uses --server-side apply to avoid last-applied-configuration annotation conflicts
/// with ArgoCD drift detection. --force-conflicts resolves field ownership disputes
/// during re-runs (e.g., ArgoCD previously owned a field, now scalex-bootstrap re-applies).
pub fn generate_kubectl_apply_args(kubeconfig: &str, manifest_path: &str) -> Vec<String> {
    vec![
        "apply".to_string(),
        "--server-side".to_string(),
        "--force-conflicts".to_string(),
        "-f".to_string(),
        manifest_path.to_string(),
        "--kubeconfig".to_string(),
        kubeconfig.to_string(),
    ]
}

// ---------------------------------------------------------------------------
// AC 9b — Helm release status verification
// ---------------------------------------------------------------------------

/// A parsed Helm release status entry from `helm list` output.
/// Pure data type — no I/O, no side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelmReleaseStatus {
    /// Release name (e.g. "cilium", "argocd")
    pub name: String,
    /// Kubernetes namespace the release was installed into
    pub namespace: String,
    /// Status reported by Helm (e.g. "deployed", "failed", "pending-install")
    pub status: String,
    /// Chart name + version (e.g. "cilium-1.17.5")
    pub chart: String,
}

impl HelmReleaseStatus {
    /// Returns true when the release has successfully reached `deployed` state.
    /// `superseded` is intentionally excluded — it indicates an old revision,
    /// not the current one, and should be treated as needing investigation.
    pub fn is_deployed(&self) -> bool {
        self.status == "deployed"
    }
}

/// Generate `helm list` arguments to query all releases in a namespace.
/// The caller can filter by release name after parsing.
///
/// Pure function — no I/O, no side effects.
pub fn generate_helm_list_args(namespace: &str, kubeconfig: &str) -> Vec<String> {
    vec![
        "list".to_string(),
        "--namespace".to_string(),
        namespace.to_string(),
        "--kubeconfig".to_string(),
        kubeconfig.to_string(),
        "--output".to_string(),
        "table".to_string(),
    ]
}

/// Parse the table output of `helm list` into a list of `HelmReleaseStatus` entries.
///
/// Expected format (tab/space-separated columns):
/// ```
/// NAME    NAMESPACE     REVISION  UPDATED                                  STATUS    CHART          APP VERSION
/// cilium  kube-system   1         2025-03-22 10:00:00.000000000 +0000 UTC  deployed  cilium-1.17.5  1.17.5
/// argocd  argocd        1         2025-03-22 10:05:00.000000000 +0000 UTC  deployed  argo-cd-8.1.1  v2.12.0
/// ```
///
/// Pure function — no I/O, no side effects. Skips the header row and blank lines.
pub fn parse_helm_list_output(output: &str) -> Vec<HelmReleaseStatus> {
    let mut releases = Vec::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Header row starts with "NAME"; skip it and any short/empty lines.
        // Minimum 6 columns: NAME NAMESPACE REVISION UPDATED STATUS CHART
        // (UPDATED spans multiple tokens — STATUS is at a variable index).
        // Strategy: find the STATUS token from the known status vocabulary.
        if cols.len() < 6 || cols[0] == "NAME" {
            continue;
        }
        let name = cols[0].to_string();
        let namespace = cols[1].to_string();
        // Helm status strings are well-known; scan for them to handle
        // multi-token UPDATED column (e.g. "2025-03-22 10:00:00.000 +0000 UTC").
        let known_statuses = [
            "deployed",
            "failed",
            "pending-install",
            "pending-upgrade",
            "pending-rollback",
            "superseded",
            "uninstalling",
        ];
        let status_idx = cols
            .iter()
            .position(|c| known_statuses.contains(c))
            .unwrap_or(0);
        if status_idx == 0 {
            // Could not identify status token — skip malformed line.
            continue;
        }
        let status = cols[status_idx].to_string();
        // CHART follows STATUS; APP VERSION may follow CHART.
        let chart = cols
            .get(status_idx + 1)
            .map(|s| s.to_string())
            .unwrap_or_default();
        releases.push(HelmReleaseStatus {
            name,
            namespace,
            status,
            chart,
        });
    }
    releases
}

/// Find the status of a specific release by name in the parsed output.
/// Returns `None` when the release is not found (not yet installed).
///
/// Pure function — no I/O, no side effects.
pub fn find_release_status<'a>(
    release_name: &str,
    releases: &'a [HelmReleaseStatus],
) -> Option<&'a HelmReleaseStatus> {
    releases.iter().find(|r| r.name == release_name)
}

/// Verify that a Helm release is in `deployed` state by running `helm list`.
/// Retries up to `max_attempts` times with `retry_secs` seconds between attempts
/// to handle transient delays after install.
///
/// I/O function — calls the system `helm` binary.
pub fn verify_helm_release_deployed(
    release_name: &str,
    namespace: &str,
    kubeconfig: &str,
    max_attempts: u32,
    retry_secs: u64,
) -> anyhow::Result<()> {
    let args = generate_helm_list_args(namespace, kubeconfig);
    for attempt in 1..=max_attempts {
        let output = std::process::Command::new("helm").args(&args).output();
        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let releases = parse_helm_list_output(&stdout);
                match find_release_status(release_name, &releases) {
                    Some(r) if r.is_deployed() => {
                        println!(
                            "[bootstrap] ✓ Release '{}' in namespace '{}' is deployed (chart: {})",
                            release_name, namespace, r.chart
                        );
                        return Ok(());
                    }
                    Some(r) => {
                        if attempt == max_attempts {
                            anyhow::bail!(
                                "Release '{}' in namespace '{}' did not reach 'deployed' state \
                                 after {} attempts. Last status: '{}'. \
                                 Check: helm list -n {} --kubeconfig {}",
                                release_name,
                                namespace,
                                max_attempts,
                                r.status,
                                namespace,
                                kubeconfig
                            );
                        }
                        eprintln!(
                            "[bootstrap] Release '{}' status='{}' ({}/{}), retrying in {}s...",
                            release_name, r.status, attempt, max_attempts, retry_secs
                        );
                    }
                    None => {
                        if attempt == max_attempts {
                            anyhow::bail!(
                                "Release '{}' not found in namespace '{}' after {} attempts. \
                                 Check: helm list -n {} --kubeconfig {}",
                                release_name,
                                namespace,
                                max_attempts,
                                namespace,
                                kubeconfig
                            );
                        }
                        eprintln!(
                            "[bootstrap] Release '{}' not yet visible ({}/{}), retrying in {}s...",
                            release_name, attempt, max_attempts, retry_secs
                        );
                    }
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if attempt == max_attempts {
                    anyhow::bail!("helm list failed: {}", stderr);
                }
            }
            Err(e) => {
                if attempt == max_attempts {
                    anyhow::bail!("Failed to run helm: {}", e);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(retry_secs));
    }
    Ok(())
}

/// A required Helm chart repository entry.
/// Used for pre-flight checks before any Helm install steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelmRepo {
    /// Human-readable name (for diagnostics)
    pub name: &'static str,
    /// OCI-or-HTTP URL used in `--repo` flag
    pub url: &'static str,
}

/// Return all Helm chart repositories required by the bootstrap pipeline.
/// Pure function — no I/O, no side effects.
///
/// Both repos are accessed via `helm upgrade --install ... --repo <url>` (OCI-like
/// single-command install, no persistent `helm repo add` needed).
pub fn helm_required_repos() -> Vec<HelmRepo> {
    vec![
        HelmRepo {
            name: "cilium",
            url: "https://helm.cilium.io/",
        },
        HelmRepo {
            name: "argo-helm",
            url: "https://argoproj.github.io/argo-helm",
        },
    ]
}

/// Parse the output of `helm version --short` and decide if Helm is installed.
/// Returns `Ok(version_string)` when the output looks like a valid Helm version line
/// (starts with "v3." or "v2."), otherwise `Err(diagnostic_message)`.
/// Pure function — no I/O, no side effects.
pub fn parse_helm_version_output(output: &str) -> Result<String, String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Err("helm version output is empty — helm may not be installed".to_string());
    }
    // helm version --short outputs e.g. "v3.17.3+g...commit..." or just "v3.17.3"
    if trimmed.starts_with("v3.") || trimmed.starts_with("v2.") {
        Ok(trimmed.to_string())
    } else {
        Err(format!(
            "Unexpected helm version output: '{}' — expected 'v3.x.y' or 'v2.x.y'",
            trimmed
        ))
    }
}

/// Verify that the `helm` binary is installed and accessible on the bootstrap node.
/// Runs `helm version --short` and validates the output.
/// I/O function — calls the system `helm` binary.
pub fn verify_helm_installed() -> anyhow::Result<String> {
    let output = std::process::Command::new("helm")
        .args(["version", "--short"])
        .output();
    match output {
        Err(e) => anyhow::bail!(
            "helm binary not found or not executable: {}. \
             Run install.sh to install Helm, or add ~/.local/bin to PATH",
            e
        ),
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("helm version --short failed: {}", stderr);
        }
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            parse_helm_version_output(&stdout).map_err(|e| anyhow::anyhow!("{}", e))
        }
    }
}

/// Execute Helm install. I/O function.
fn run_helm_install(args: &[String]) -> anyhow::Result<()> {
    let output = std::process::Command::new("helm").args(args).output();
    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("helm install failed: {}", stderr);
        }
        Err(e) => anyhow::bail!("Failed to run helm: {}. Is Helm installed?", e),
    }
}

/// Execute kubectl apply. I/O function.
fn run_kubectl_apply(args: &[String]) -> anyhow::Result<()> {
    let output = std::process::Command::new("kubectl").args(args).output();
    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("kubectl apply failed: {}", stderr);
        }
        Err(e) => anyhow::bail!("Failed to run kubectl: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- generate_argocd_helm_install_args ---

    #[test]
    fn test_helm_args_contain_namespace_argocd() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1", "values.yaml");
        let joined = args.join(" ");
        assert!(
            joined.contains("--namespace argocd"),
            "Must install to argocd namespace — got: {}",
            joined
        );
    }

    #[test]
    fn test_helm_args_contain_create_namespace() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1", "values.yaml");
        assert!(
            args.contains(&"--create-namespace".to_string()),
            "Must create namespace if not exists"
        );
    }

    #[test]
    fn test_helm_args_use_provided_kubeconfig() {
        let args = generate_argocd_helm_install_args("/my/kubeconfig.yaml", "8.1.1", "values.yaml");
        let kc_idx = args.iter().position(|a| a == "--kubeconfig").unwrap();
        assert_eq!(args[kc_idx + 1], "/my/kubeconfig.yaml");
    }

    #[test]
    fn test_helm_args_use_correct_chart_version() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1", "values.yaml");
        let ver_idx = args.iter().position(|a| a == "--version").unwrap();
        assert_eq!(args[ver_idx + 1], "8.1.1");
    }

    #[test]
    fn test_helm_args_use_upgrade_install_for_idempotency() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1", "values.yaml");
        assert_eq!(
            args[0], "upgrade",
            "Must use 'upgrade --install' for idempotency"
        );
        assert_eq!(args[1], "--install");
    }

    #[test]
    fn test_helm_args_use_official_argo_helm_repo() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1", "values.yaml");
        let repo_idx = args.iter().position(|a| a == "--repo").unwrap();
        assert_eq!(args[repo_idx + 1], "https://argoproj.github.io/argo-helm");
    }

    #[test]
    fn test_helm_args_wait_for_readiness() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1", "values.yaml");
        assert!(
            args.contains(&"--wait".to_string()),
            "Must wait for ArgoCD to be ready before proceeding"
        );
    }

    // --- generate_kubectl_apply_args ---

    #[test]
    fn test_kubectl_apply_args_structure() {
        let args = generate_kubectl_apply_args("/tower/kube.yaml", "gitops/bootstrap/spread.yaml");
        assert_eq!(args[0], "apply");
        // --server-side and --force-conflicts must precede -f
        assert!(
            args.contains(&"--server-side".to_string()),
            "Must use server-side apply to prevent ArgoCD drift annotation conflicts — got: {:?}",
            args
        );
        assert!(
            args.contains(&"--force-conflicts".to_string()),
            "Must include --force-conflicts for idempotent re-runs — got: {:?}",
            args
        );
        // -f and manifest path must still be present
        let f_idx = args.iter().position(|a| a == "-f").unwrap();
        assert_eq!(args[f_idx + 1], "gitops/bootstrap/spread.yaml");
        let kc_idx = args.iter().position(|a| a == "--kubeconfig").unwrap();
        assert_eq!(args[kc_idx + 1], "/tower/kube.yaml");
    }

    #[test]
    fn test_kubectl_apply_uses_server_side() {
        let args = generate_kubectl_apply_args("/tower/kube.yaml", "spread.yaml");
        assert!(
            args.contains(&"--server-side".to_string()),
            "Server-side apply prevents last-applied-configuration annotation conflicts with ArgoCD"
        );
        assert!(
            args.contains(&"--force-conflicts".to_string()),
            "--force-conflicts required for idempotency when field ownership changes between runs"
        );
    }

    #[test]
    fn test_kubectl_apply_uses_tower_kubeconfig() {
        let args = generate_kubectl_apply_args("/my/tower.yaml", "spread.yaml");
        let joined = args.join(" ");
        assert!(
            joined.contains("--kubeconfig /my/tower.yaml"),
            "spread.yaml must be applied to tower cluster — got: {}",
            joined
        );
    }

    // --- generate_cilium_helm_install_args ---

    #[test]
    fn test_cilium_args_use_cilium_helm_repo() {
        let args = generate_cilium_helm_install_args("/kube.yaml", "1.17.5", "values.yaml");
        let repo_idx = args.iter().position(|a| a == "--repo").unwrap();
        assert_eq!(args[repo_idx + 1], "https://helm.cilium.io/");
    }

    #[test]
    fn test_cilium_args_target_kube_system() {
        let args = generate_cilium_helm_install_args("/kube.yaml", "1.17.5", "values.yaml");
        let joined = args.join(" ");
        assert!(
            joined.contains("--namespace kube-system"),
            "Cilium must install to kube-system — got: {}",
            joined
        );
    }

    #[test]
    fn test_cilium_args_use_values_file() {
        let args = generate_cilium_helm_install_args(
            "/kube.yaml",
            "1.17.5",
            "gitops/tower/cilium/values.yaml",
        );
        let val_idx = args.iter().position(|a| a == "--values").unwrap();
        assert_eq!(args[val_idx + 1], "gitops/tower/cilium/values.yaml");
    }

    #[test]
    fn test_cilium_args_idempotent() {
        let args = generate_cilium_helm_install_args("/kube.yaml", "1.17.5", "v.yaml");
        assert_eq!(args[0], "upgrade");
        assert_eq!(args[1], "--install");
    }

    #[test]
    fn test_cilium_args_wait_for_readiness() {
        let args = generate_cilium_helm_install_args("/kube.yaml", "1.17.5", "v.yaml");
        assert!(
            args.contains(&"--wait".to_string()),
            "Must wait for Cilium to be ready before proceeding"
        );
    }

    // --- Bootstrap pipeline integration ---

    #[test]
    fn test_bootstrap_pipeline_order_cilium_then_argocd_then_apply() {
        // Verify the pipeline produces correct commands in order
        let cilium = generate_cilium_helm_install_args(
            "/tower/kube.yaml",
            "1.17.5",
            "gitops/tower/cilium/values.yaml",
        );
        let helm = generate_argocd_helm_install_args(
            "/tower/kube.yaml",
            "8.1.1",
            "gitops/tower/argocd/values.yaml",
        );
        let apply = generate_kubectl_apply_args("/tower/kube.yaml", "gitops/bootstrap/spread.yaml");

        // Phase 1: Cilium CNI (must be first — pods need CNI to schedule)
        assert!(cilium[0] == "upgrade" && cilium[1] == "--install");
        assert!(cilium.contains(&"cilium".to_string()));

        // Phase 2: Helm installs ArgoCD with values
        assert!(helm[0] == "upgrade" && helm[1] == "--install");
        assert!(helm.contains(&"argocd".to_string()));
        assert!(
            helm.contains(&"gitops/tower/argocd/values.yaml".to_string()),
            "ArgoCD must use values.yaml for kustomize.buildOptions"
        );

        // Phase 4: Apply spread.yaml
        assert!(apply[0] == "apply");
        assert!(apply.contains(&"gitops/bootstrap/spread.yaml".to_string()));
    }

    #[test]
    fn test_bootstrap_all_commands_target_tower_kubeconfig() {
        let tower_kc = "/clusters/tower/kubeconfig.yaml";
        let cilium = generate_cilium_helm_install_args(tower_kc, "1.17.5", "values.yaml");
        let helm = generate_argocd_helm_install_args(tower_kc, "8.1.1", "values.yaml");
        let apply = generate_kubectl_apply_args(tower_kc, "spread.yaml");

        // All phases must reference tower kubeconfig
        assert!(
            cilium.contains(&tower_kc.to_string()),
            "Cilium must target tower"
        );
        assert!(
            helm.contains(&tower_kc.to_string()),
            "Helm must target tower"
        );
        assert!(
            apply.contains(&tower_kc.to_string()),
            "kubectl apply must target tower"
        );
    }

    #[test]
    fn test_argocd_helm_includes_values_file() {
        let args = generate_argocd_helm_install_args(
            "/kube.yaml",
            "8.1.1",
            "gitops/tower/argocd/values.yaml",
        );
        let val_idx = args.iter().position(|a| a == "--values").unwrap();
        assert_eq!(args[val_idx + 1], "gitops/tower/argocd/values.yaml");
    }

    #[test]
    fn test_argocd_helm_timeout_600s() {
        let args = generate_argocd_helm_install_args("/kube.yaml", "8.1.1", "v.yaml");
        let t_idx = args.iter().position(|a| a == "--timeout").unwrap();
        assert_eq!(
            args[t_idx + 1],
            "600s",
            "ArgoCD needs longer timeout on resource-constrained nodes"
        );
    }

    // --- AC 9a: helm_required_repos ---

    #[test]
    fn test_helm_required_repos_contains_cilium() {
        let repos = helm_required_repos();
        assert!(
            repos.iter().any(|r| r.url == "https://helm.cilium.io/"),
            "Required repos must include Cilium Helm repo"
        );
    }

    #[test]
    fn test_helm_required_repos_contains_argo_helm() {
        let repos = helm_required_repos();
        assert!(
            repos.iter().any(|r| r.url == "https://argoproj.github.io/argo-helm"),
            "Required repos must include official argo-helm repo"
        );
    }

    #[test]
    fn test_helm_required_repos_matches_cilium_install_args() {
        // The repo URL in helm_required_repos() must match what generate_cilium_helm_install_args
        // actually uses, so pre-flight checks cover the real install step.
        let repos = helm_required_repos();
        let cilium_repo = repos.iter().find(|r| r.name == "cilium").unwrap();
        let cilium_args = generate_cilium_helm_install_args("/kube.yaml", "1.17.5", "v.yaml");
        let repo_idx = cilium_args.iter().position(|a| a == "--repo").unwrap();
        assert_eq!(
            cilium_args[repo_idx + 1],
            cilium_repo.url,
            "helm_required_repos cilium URL must match generate_cilium_helm_install_args --repo"
        );
    }

    #[test]
    fn test_helm_required_repos_matches_argocd_install_args() {
        // The repo URL in helm_required_repos() must match what generate_argocd_helm_install_args
        // actually uses, so pre-flight checks cover the real install step.
        let repos = helm_required_repos();
        let argo_repo = repos.iter().find(|r| r.name == "argo-helm").unwrap();
        let argocd_args = generate_argocd_helm_install_args("/kube.yaml", "8.1.1", "v.yaml");
        let repo_idx = argocd_args.iter().position(|a| a == "--repo").unwrap();
        assert_eq!(
            argocd_args[repo_idx + 1],
            argo_repo.url,
            "helm_required_repos argo-helm URL must match generate_argocd_helm_install_args --repo"
        );
    }

    #[test]
    fn test_helm_required_repos_count_matches_bootstrap_pipeline() {
        // Exactly 2 repos are used in bootstrap: Cilium + ArgoCD.
        // If the pipeline gains/loses a chart, this test forces an update to helm_required_repos().
        let repos = helm_required_repos();
        assert_eq!(
            repos.len(),
            2,
            "Bootstrap pipeline uses exactly 2 Helm repos (Cilium + ArgoCD). \
             Update helm_required_repos() if a new chart is added."
        );
    }

    // --- AC 9a: parse_helm_version_output ---

    #[test]
    fn test_parse_helm_version_output_valid_v3() {
        let result = parse_helm_version_output("v3.17.3+g68eae3b");
        assert!(result.is_ok(), "v3.x.y+commit should be valid");
        assert_eq!(result.unwrap(), "v3.17.3+g68eae3b");
    }

    #[test]
    fn test_parse_helm_version_output_valid_v3_plain() {
        let result = parse_helm_version_output("v3.10.0");
        assert!(result.is_ok(), "v3.x.y should be valid");
    }

    #[test]
    fn test_parse_helm_version_output_valid_with_newline() {
        // helm version --short adds a trailing newline; must be trimmed
        let result = parse_helm_version_output("v3.17.3\n");
        assert!(result.is_ok(), "trailing newline must be trimmed");
        assert_eq!(result.unwrap(), "v3.17.3");
    }

    #[test]
    fn test_parse_helm_version_output_empty_is_error() {
        let result = parse_helm_version_output("");
        assert!(result.is_err(), "empty output must return error");
        let err = result.unwrap_err();
        assert!(
            err.contains("empty"),
            "error must mention 'empty': got '{}'",
            err
        );
    }

    #[test]
    fn test_parse_helm_version_output_garbage_is_error() {
        let result = parse_helm_version_output("command not found");
        assert!(result.is_err(), "'command not found' must return error");
    }

    #[test]
    fn test_parse_helm_version_output_error_includes_actual_output() {
        let result = parse_helm_version_output("unexpected garbage");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("unexpected garbage"),
            "error must quote the actual output for diagnosability: '{}'",
            err
        );
    }

    #[test]
    fn test_helm_required_repos_all_use_https() {
        // All Helm repos must use HTTPS for security; HTTP would be a MITM risk.
        let repos = helm_required_repos();
        for repo in &repos {
            assert!(
                repo.url.starts_with("https://"),
                "Helm repo '{}' must use HTTPS, got: {}",
                repo.name,
                repo.url
            );
        }
    }

    #[test]
    fn test_helm_required_repos_no_duplicates() {
        let repos = helm_required_repos();
        let mut urls: Vec<&str> = repos.iter().map(|r| r.url).collect();
        let original_len = urls.len();
        urls.dedup();
        assert_eq!(
            urls.len(),
            original_len,
            "helm_required_repos must not have duplicate URLs"
        );
    }

    // ---------------------------------------------------------------------------
    // AC 9b — Helm release status verification
    // ---------------------------------------------------------------------------

    // --- parse_helm_list_output ---

    #[test]
    fn test_parse_helm_list_output_deployed() {
        let output = "\
NAME    NAMESPACE     REVISION  UPDATED                                  STATUS    CHART          APP VERSION\n\
cilium  kube-system   1         2025-03-22 10:00:00.000000000 +0000 UTC  deployed  cilium-1.17.5  1.17.5";

        let releases = parse_helm_list_output(output);
        assert_eq!(releases.len(), 1, "must parse 1 release");
        assert_eq!(releases[0].name, "cilium");
        assert_eq!(releases[0].namespace, "kube-system");
        assert_eq!(releases[0].status, "deployed");
        assert_eq!(releases[0].chart, "cilium-1.17.5");
    }

    #[test]
    fn test_parse_helm_list_output_multiple_releases() {
        let output = "\
NAME    NAMESPACE     REVISION  UPDATED                                  STATUS    CHART          APP VERSION\n\
cilium  kube-system   1         2025-03-22 10:00:00.000000000 +0000 UTC  deployed  cilium-1.17.5  1.17.5\n\
argocd  argocd        1         2025-03-22 10:05:00.000000000 +0000 UTC  deployed  argo-cd-8.1.1  v2.12.0";

        let releases = parse_helm_list_output(output);
        assert_eq!(releases.len(), 2, "must parse 2 releases");
        assert_eq!(releases[0].name, "cilium");
        assert_eq!(releases[1].name, "argocd");
    }

    #[test]
    fn test_parse_helm_list_output_failed_status() {
        let output = "\
NAME    NAMESPACE     REVISION  UPDATED                                  STATUS  CHART          APP VERSION\n\
cilium  kube-system   1         2025-03-22 10:00:00.000000000 +0000 UTC  failed  cilium-1.17.5  1.17.5";

        let releases = parse_helm_list_output(output);
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].status, "failed");
        assert!(
            !releases[0].is_deployed(),
            "failed release must not report is_deployed()"
        );
    }

    #[test]
    fn test_parse_helm_list_output_pending_install_status() {
        let output = "\
NAME    NAMESPACE     REVISION  UPDATED                                  STATUS           CHART          APP VERSION\n\
argocd  argocd        1         2025-03-22 10:05:00.000000000 +0000 UTC  pending-install  argo-cd-8.1.1  v2.12.0";

        let releases = parse_helm_list_output(output);
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].status, "pending-install");
        assert!(
            !releases[0].is_deployed(),
            "pending-install must not report is_deployed()"
        );
    }

    #[test]
    fn test_parse_helm_list_output_empty_input() {
        let releases = parse_helm_list_output("");
        assert!(
            releases.is_empty(),
            "empty input must produce no releases"
        );
    }

    #[test]
    fn test_parse_helm_list_output_header_only() {
        let output =
            "NAME    NAMESPACE  REVISION  UPDATED  STATUS  CHART  APP VERSION";
        let releases = parse_helm_list_output(output);
        assert!(
            releases.is_empty(),
            "header-only output must produce no releases"
        );
    }

    // --- HelmReleaseStatus::is_deployed ---

    #[test]
    fn test_release_status_is_deployed_true() {
        let r = HelmReleaseStatus {
            name: "cilium".to_string(),
            namespace: "kube-system".to_string(),
            status: "deployed".to_string(),
            chart: "cilium-1.17.5".to_string(),
        };
        assert!(r.is_deployed(), "status='deployed' must return is_deployed()=true");
    }

    #[test]
    fn test_release_status_is_deployed_false_for_failed() {
        let r = HelmReleaseStatus {
            name: "cilium".to_string(),
            namespace: "kube-system".to_string(),
            status: "failed".to_string(),
            chart: "cilium-1.17.5".to_string(),
        };
        assert!(!r.is_deployed(), "status='failed' must return is_deployed()=false");
    }

    #[test]
    fn test_release_status_is_deployed_false_for_superseded() {
        // superseded = an OLD revision; current release may be in different state
        let r = HelmReleaseStatus {
            name: "argocd".to_string(),
            namespace: "argocd".to_string(),
            status: "superseded".to_string(),
            chart: "argo-cd-7.0.0".to_string(),
        };
        assert!(
            !r.is_deployed(),
            "status='superseded' must NOT report is_deployed() — \
             it indicates an old revision, not the current healthy release"
        );
    }

    #[test]
    fn test_release_status_is_deployed_false_for_pending() {
        for status in &["pending-install", "pending-upgrade", "pending-rollback"] {
            let r = HelmReleaseStatus {
                name: "test".to_string(),
                namespace: "ns".to_string(),
                status: status.to_string(),
                chart: "chart-1.0.0".to_string(),
            };
            assert!(
                !r.is_deployed(),
                "status='{}' must NOT report is_deployed() — release not yet stable",
                status
            );
        }
    }

    // --- find_release_status ---

    #[test]
    fn test_find_release_status_found() {
        let releases = vec![
            HelmReleaseStatus {
                name: "cilium".to_string(),
                namespace: "kube-system".to_string(),
                status: "deployed".to_string(),
                chart: "cilium-1.17.5".to_string(),
            },
            HelmReleaseStatus {
                name: "argocd".to_string(),
                namespace: "argocd".to_string(),
                status: "deployed".to_string(),
                chart: "argo-cd-8.1.1".to_string(),
            },
        ];
        let found = find_release_status("cilium", &releases);
        assert!(found.is_some(), "must find 'cilium' release");
        assert_eq!(found.unwrap().namespace, "kube-system");
    }

    #[test]
    fn test_find_release_status_not_found() {
        let releases: Vec<HelmReleaseStatus> = vec![];
        let found = find_release_status("cilium", &releases);
        assert!(
            found.is_none(),
            "must return None when release not present in output"
        );
    }

    #[test]
    fn test_find_release_status_wrong_name() {
        let releases = vec![HelmReleaseStatus {
            name: "argocd".to_string(),
            namespace: "argocd".to_string(),
            status: "deployed".to_string(),
            chart: "argo-cd-8.1.1".to_string(),
        }];
        let found = find_release_status("cilium", &releases);
        assert!(
            found.is_none(),
            "must return None when only 'argocd' is present but 'cilium' is requested"
        );
    }

    // --- generate_helm_list_args ---

    #[test]
    fn test_generate_helm_list_args_structure() {
        let args = generate_helm_list_args("kube-system", "/clusters/tower/kubeconfig.yaml");
        assert_eq!(args[0], "list", "must start with 'list'");
        assert!(
            args.contains(&"--namespace".to_string()),
            "must include --namespace"
        );
        let ns_idx = args.iter().position(|a| a == "--namespace").unwrap();
        assert_eq!(args[ns_idx + 1], "kube-system");
        let kc_idx = args.iter().position(|a| a == "--kubeconfig").unwrap();
        assert_eq!(args[kc_idx + 1], "/clusters/tower/kubeconfig.yaml");
    }

    #[test]
    fn test_generate_helm_list_args_includes_output_table() {
        let args = generate_helm_list_args("argocd", "/tmp/kube.yaml");
        assert!(
            args.contains(&"--output".to_string()),
            "must include --output for deterministic parsing"
        );
        let out_idx = args.iter().position(|a| a == "--output").unwrap();
        assert_eq!(
            args[out_idx + 1], "table",
            "output format must be 'table' to match parse_helm_list_output expectations"
        );
    }

    #[test]
    fn test_generate_helm_list_args_distinct_namespaces() {
        // Cilium and ArgoCD are in different namespaces — each helm list call
        // must target the correct namespace to avoid cross-contamination.
        let cilium_args = generate_helm_list_args("kube-system", "/kube.yaml");
        let argocd_args = generate_helm_list_args("argocd", "/kube.yaml");

        let cilium_ns_idx = cilium_args.iter().position(|a| a == "--namespace").unwrap();
        let argocd_ns_idx = argocd_args.iter().position(|a| a == "--namespace").unwrap();

        assert_eq!(cilium_args[cilium_ns_idx + 1], "kube-system");
        assert_eq!(argocd_args[argocd_ns_idx + 1], "argocd");
        assert_ne!(
            cilium_args[cilium_ns_idx + 1],
            argocd_args[argocd_ns_idx + 1],
            "Cilium and ArgoCD must use different namespaces"
        );
    }

    // --- E2E pipeline: install → verify ---

    #[test]
    fn test_ac9b_bootstrap_pipeline_verifies_cilium_after_install() {
        // Simulate the full output that `helm list -n kube-system` returns
        // after Cilium is successfully installed.
        let helm_list_output = "\
NAME    NAMESPACE     REVISION  UPDATED                                  STATUS    CHART          APP VERSION\n\
cilium  kube-system   1         2025-03-22 10:00:00.000000000 +0000 UTC  deployed  cilium-1.17.5  1.17.5";

        let releases = parse_helm_list_output(helm_list_output);
        let cilium = find_release_status("cilium", &releases);

        assert!(cilium.is_some(), "cilium release must be found after install");
        assert!(
            cilium.unwrap().is_deployed(),
            "cilium must be in 'deployed' state after Helm install"
        );
        assert_eq!(
            cilium.unwrap().namespace, "kube-system",
            "cilium must be installed in kube-system"
        );
    }

    #[test]
    fn test_ac9b_bootstrap_pipeline_verifies_argocd_after_install() {
        // Simulate the full output that `helm list -n argocd` returns
        // after ArgoCD is successfully installed on the tower cluster.
        let helm_list_output = "\
NAME    NAMESPACE  REVISION  UPDATED                                  STATUS    CHART          APP VERSION\n\
argocd  argocd     1         2025-03-22 10:05:00.000000000 +0000 UTC  deployed  argo-cd-8.1.1  v2.12.0";

        let releases = parse_helm_list_output(helm_list_output);
        let argocd = find_release_status("argocd", &releases);

        assert!(argocd.is_some(), "argocd release must be found after install");
        assert!(
            argocd.unwrap().is_deployed(),
            "argocd must be in 'deployed' state after Helm install"
        );
        assert_eq!(
            argocd.unwrap().namespace, "argocd",
            "argocd must be installed in argocd namespace"
        );
    }

    #[test]
    fn test_ac9b_both_releases_deployed_multi_cluster() {
        // Simulate checking both core releases after bootstrap completes
        // on a tower+sandbox configuration (tower has both Cilium and ArgoCD;
        // sandbox has Cilium only).
        let tower_list = "\
NAME    NAMESPACE     REVISION  UPDATED                                  STATUS    CHART          APP VERSION\n\
cilium  kube-system   1         2025-03-22 10:00:00.000000000 +0000 UTC  deployed  cilium-1.17.5  1.17.5\n\
argocd  argocd        1         2025-03-22 10:05:00.000000000 +0000 UTC  deployed  argo-cd-8.1.1  v2.12.0";

        let sandbox_list = "\
NAME    NAMESPACE     REVISION  UPDATED                                  STATUS    CHART          APP VERSION\n\
cilium  kube-system   1         2025-03-22 10:02:00.000000000 +0000 UTC  deployed  cilium-1.17.5  1.17.5";

        // Tower cluster: both cilium AND argocd are deployed
        let tower_releases = parse_helm_list_output(tower_list);
        let tower_cilium = find_release_status("cilium", &tower_releases);
        let tower_argocd = find_release_status("argocd", &tower_releases);
        assert!(
            tower_cilium.map(|r| r.is_deployed()).unwrap_or(false),
            "tower: cilium must be deployed"
        );
        assert!(
            tower_argocd.map(|r| r.is_deployed()).unwrap_or(false),
            "tower: argocd must be deployed"
        );

        // Sandbox cluster: only cilium is deployed (argocd is tower-only)
        let sandbox_releases = parse_helm_list_output(sandbox_list);
        let sandbox_cilium = find_release_status("cilium", &sandbox_releases);
        let sandbox_argocd = find_release_status("argocd", &sandbox_releases);
        assert!(
            sandbox_cilium.map(|r| r.is_deployed()).unwrap_or(false),
            "sandbox: cilium must be deployed"
        );
        assert!(
            sandbox_argocd.is_none(),
            "sandbox: argocd must NOT appear — ArgoCD is tower-only"
        );
    }

    #[test]
    fn test_ac9b_failed_release_is_not_deployed() {
        // Verify that a failed release is correctly detected and not passed off as deployed.
        let helm_list_output = "\
NAME    NAMESPACE     REVISION  UPDATED                                  STATUS  CHART          APP VERSION\n\
cilium  kube-system   1         2025-03-22 10:00:00.000000000 +0000 UTC  failed  cilium-1.17.5  1.17.5";

        let releases = parse_helm_list_output(helm_list_output);
        let cilium = find_release_status("cilium", &releases);

        assert!(cilium.is_some(), "failed release must be found");
        assert!(
            !cilium.unwrap().is_deployed(),
            "failed release must NOT be treated as deployed — \
             verify_helm_release_deployed must retry/bail when status='failed'"
        );
    }
}
