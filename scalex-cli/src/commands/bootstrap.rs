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

    // Step 1: Install ArgoCD on tower cluster
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
    println!(
        "[bootstrap] Phase 1: Installing ArgoCD on '{}'...",
        tower.cluster_name
    );

    let helm_args = generate_argocd_helm_install_args(
        &tower_kubeconfig.display().to_string(),
        &args.argocd_version,
    );

    if args.dry_run {
        println!("[dry-run] helm {}", helm_args.join(" "));
    } else {
        run_helm_install(&helm_args)?;
        println!("[bootstrap] ArgoCD installed on '{}'", tower.cluster_name);
    }

    // Step 2: Register non-management clusters
    let managed_clusters: Vec<_> = k8s_config
        .config
        .clusters
        .iter()
        .filter(|c| c.cluster_role != "management")
        .collect();

    for cluster in &managed_clusters {
        println!(
            "[bootstrap] Phase 2: Registering '{}' as remote cluster in ArgoCD...",
            cluster.cluster_name
        );

        let cluster_kubeconfig = args
            .clusters_dir
            .join(&cluster.cluster_name)
            .join("kubeconfig.yaml");

        let register_args = generate_argocd_cluster_add_args(
            &cluster.cluster_name,
            &tower_kubeconfig.display().to_string(),
            &cluster_kubeconfig.display().to_string(),
        );

        if args.dry_run {
            println!("[dry-run] argocd {}", register_args.join(" "));
        } else {
            run_argocd_cluster_add(&register_args)?;
            println!(
                "[bootstrap] Registered '{}' in ArgoCD",
                cluster.cluster_name
            );
        }
    }

    // Step 3: Apply spread.yaml
    println!("[bootstrap] Phase 3: Applying GitOps bootstrap (spread.yaml)...");
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

/// Generate Helm install arguments for ArgoCD.
/// Pure function — no I/O, no side effects.
pub fn generate_argocd_helm_install_args(kubeconfig: &str, chart_version: &str) -> Vec<String> {
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
        "--wait".to_string(),
        "--timeout".to_string(),
        "300s".to_string(),
    ]
}

/// Generate argocd cluster add arguments.
/// Pure function — no I/O, no side effects.
pub fn generate_argocd_cluster_add_args(
    cluster_name: &str,
    tower_kubeconfig: &str,
    cluster_kubeconfig: &str,
) -> Vec<String> {
    vec![
        "cluster".to_string(),
        "add".to_string(),
        cluster_name.to_string(),
        "--kubeconfig".to_string(),
        cluster_kubeconfig.to_string(),
        "--core".to_string(),
        "--name".to_string(),
        cluster_name.to_string(),
        "-y".to_string(),
    ]
}

/// Generate kubectl apply arguments for spread.yaml.
/// Pure function — no I/O, no side effects.
pub fn generate_kubectl_apply_args(kubeconfig: &str, manifest_path: &str) -> Vec<String> {
    vec![
        "apply".to_string(),
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

/// Execute argocd cluster add. I/O function.
fn run_argocd_cluster_add(args: &[String]) -> anyhow::Result<()> {
    let output = std::process::Command::new("argocd").args(args).output();
    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("argocd cluster add failed: {}", stderr);
        }
        Err(e) => anyhow::bail!("Failed to run argocd CLI: {}. Is argocd CLI installed?", e),
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
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1");
        let joined = args.join(" ");
        assert!(
            joined.contains("--namespace argocd"),
            "Must install to argocd namespace — got: {}",
            joined
        );
    }

    #[test]
    fn test_helm_args_contain_create_namespace() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1");
        assert!(
            args.contains(&"--create-namespace".to_string()),
            "Must create namespace if not exists"
        );
    }

    #[test]
    fn test_helm_args_use_provided_kubeconfig() {
        let args = generate_argocd_helm_install_args("/my/kubeconfig.yaml", "8.1.1");
        let kc_idx = args.iter().position(|a| a == "--kubeconfig").unwrap();
        assert_eq!(args[kc_idx + 1], "/my/kubeconfig.yaml");
    }

    #[test]
    fn test_helm_args_use_correct_chart_version() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1");
        let ver_idx = args.iter().position(|a| a == "--version").unwrap();
        assert_eq!(args[ver_idx + 1], "8.1.1");
    }

    #[test]
    fn test_helm_args_use_upgrade_install_for_idempotency() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1");
        assert_eq!(
            args[0], "upgrade",
            "Must use 'upgrade --install' for idempotency"
        );
        assert_eq!(args[1], "--install");
    }

    #[test]
    fn test_helm_args_use_official_argo_helm_repo() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1");
        let repo_idx = args.iter().position(|a| a == "--repo").unwrap();
        assert_eq!(args[repo_idx + 1], "https://argoproj.github.io/argo-helm");
    }

    #[test]
    fn test_helm_args_wait_for_readiness() {
        let args = generate_argocd_helm_install_args("/tmp/kube.yaml", "8.1.1");
        assert!(
            args.contains(&"--wait".to_string()),
            "Must wait for ArgoCD to be ready before proceeding"
        );
    }

    // --- generate_argocd_cluster_add_args ---

    #[test]
    fn test_cluster_add_args_contain_cluster_name() {
        let args =
            generate_argocd_cluster_add_args("sandbox", "/tower/kube.yaml", "/sandbox/kube.yaml");
        assert!(
            args.contains(&"sandbox".to_string()),
            "Must reference cluster name"
        );
    }

    #[test]
    fn test_cluster_add_args_use_correct_kubeconfigs() {
        let args =
            generate_argocd_cluster_add_args("sandbox", "/tower/kube.yaml", "/sandbox/kube.yaml");
        let joined = args.join(" ");
        assert!(
            joined.contains("--kubeconfig /sandbox/kube.yaml"),
            "Must use sandbox kubeconfig for the cluster being added — got: {}",
            joined
        );
        assert!(
            joined.contains("--core"),
            "Must use tower kubeconfig for ArgoCD server — got: {}",
            joined
        );
    }

    #[test]
    fn test_cluster_add_args_auto_confirm() {
        let args = generate_argocd_cluster_add_args("sandbox", "/t.yaml", "/s.yaml");
        assert!(
            args.contains(&"-y".to_string()),
            "Must auto-confirm to avoid interactive prompt"
        );
    }

    // --- generate_kubectl_apply_args ---

    #[test]
    fn test_kubectl_apply_args_structure() {
        let args = generate_kubectl_apply_args("/tower/kube.yaml", "gitops/bootstrap/spread.yaml");
        assert_eq!(args[0], "apply");
        assert_eq!(args[1], "-f");
        assert_eq!(args[2], "gitops/bootstrap/spread.yaml");
        let kc_idx = args.iter().position(|a| a == "--kubeconfig").unwrap();
        assert_eq!(args[kc_idx + 1], "/tower/kube.yaml");
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

    // --- Bootstrap pipeline integration ---

    #[test]
    fn test_bootstrap_pipeline_order_helm_then_register_then_apply() {
        // Verify the 3-phase pipeline produces correct commands in order
        let helm = generate_argocd_helm_install_args("/tower/kube.yaml", "8.1.1");
        let register =
            generate_argocd_cluster_add_args("sandbox", "/tower/kube.yaml", "/sandbox/kube.yaml");
        let apply = generate_kubectl_apply_args("/tower/kube.yaml", "gitops/bootstrap/spread.yaml");

        // Phase 1: Helm installs ArgoCD
        assert!(helm[0] == "upgrade" && helm[1] == "--install");
        assert!(helm.contains(&"argocd".to_string()));

        // Phase 2: Register sandbox
        assert!(register[0] == "cluster" && register[1] == "add");
        assert!(register.contains(&"sandbox".to_string()));

        // Phase 3: Apply spread.yaml
        assert!(apply[0] == "apply");
        assert!(apply.contains(&"gitops/bootstrap/spread.yaml".to_string()));
    }

    #[test]
    fn test_bootstrap_all_commands_target_tower_kubeconfig() {
        let tower_kc = "/clusters/tower/kubeconfig.yaml";
        let helm = generate_argocd_helm_install_args(tower_kc, "8.1.1");
        let register = generate_argocd_cluster_add_args("sandbox", tower_kc, "/sandbox/kube.yaml");
        let apply = generate_kubectl_apply_args(tower_kc, "spread.yaml");

        // All three phases must reference tower kubeconfig
        assert!(
            helm.contains(&tower_kc.to_string()),
            "Helm must target tower"
        );
        assert!(
            register.contains(&"--core".to_string()),
            "Cluster add must use --core mode"
        );
        assert!(
            apply.contains(&tower_kc.to_string()),
            "kubectl apply must target tower"
        );
    }
}
