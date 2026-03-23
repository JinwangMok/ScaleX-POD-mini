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
        } else {
            run_helm_install(&cilium_args)?;
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
    } else {
        run_helm_install(&helm_args)?;
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
}
