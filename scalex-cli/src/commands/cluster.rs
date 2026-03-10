use crate::core::kubespray;
use crate::models::cluster::{ClusterMode, K8sClustersConfig};
use crate::models::sdi::SdiSpec;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args)]
pub struct ClusterArgs {
    #[command(subcommand)]
    command: ClusterCommand,
}

#[derive(Subcommand)]
enum ClusterCommand {
    /// Initialize Kubernetes clusters from SDI pools via Kubespray
    Init {
        /// Path to k8s-clusters config file
        config_file: String,

        /// Path to SDI specs file (for inventory generation)
        #[arg(long)]
        sdi_spec: Option<String>,

        /// SDI state directory (alternative to --sdi-spec)
        #[arg(long, default_value = "_generated/sdi")]
        sdi_dir: PathBuf,

        /// Output directory for generated kubespray configs
        #[arg(long, default_value = "_generated/clusters")]
        output_dir: PathBuf,

        /// Dry run — generate configs without running kubespray
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

pub fn run(args: ClusterArgs) -> anyhow::Result<()> {
    match args.command {
        ClusterCommand::Init {
            config_file,
            sdi_spec,
            sdi_dir,
            output_dir,
            dry_run,
        } => run_init(config_file, sdi_spec, sdi_dir, output_dir, dry_run),
    }
}

fn run_init(
    config_file: String,
    sdi_spec_path: Option<String>,
    sdi_dir: PathBuf,
    output_dir: PathBuf,
    dry_run: bool,
) -> anyhow::Result<()> {
    // Step 1: Load k8s-clusters config
    println!("[cluster] Loading cluster config from {}...", config_file);
    let raw = std::fs::read_to_string(&config_file)?;
    let k8s_config: K8sClustersConfig = serde_yaml::from_str(&raw)?;

    // Step 2: Load SDI spec (only needed if any cluster uses SDI mode)
    let has_sdi_clusters = k8s_config
        .config
        .clusters
        .iter()
        .any(|c| c.cluster_mode == ClusterMode::Sdi);
    let sdi_spec = if has_sdi_clusters {
        Some(load_sdi_spec_from_options(sdi_spec_path, &sdi_dir)?)
    } else {
        None
    };

    // Step 3: For each cluster, generate inventory + vars + run kubespray
    for cluster in &k8s_config.config.clusters {
        let mode_label = match cluster.cluster_mode {
            ClusterMode::Sdi => format!("sdi:{}", cluster.cluster_sdi_resource_pool),
            ClusterMode::Baremetal => "baremetal".to_string(),
        };
        println!(
            "\n[cluster] === {} (mode: {}, role: {}) ===",
            cluster.cluster_name, mode_label, cluster.cluster_role
        );

        let cluster_dir = output_dir.join(&cluster.cluster_name);
        std::fs::create_dir_all(&cluster_dir)?;

        // Generate inventory based on mode
        let inventory = match cluster.cluster_mode {
            ClusterMode::Sdi => {
                let spec = sdi_spec.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "SDI spec required for cluster '{}' (mode=sdi)",
                        cluster.cluster_name
                    )
                })?;
                kubespray::generate_inventory(cluster, spec).map_err(|e| anyhow::anyhow!(e))?
            }
            ClusterMode::Baremetal => {
                kubespray::generate_inventory_baremetal(cluster).map_err(|e| anyhow::anyhow!(e))?
            }
        };
        let inventory_path = cluster_dir.join("inventory.ini");
        if dry_run {
            println!("[dry-run] inventory.ini:\n{}", inventory);
        } else {
            std::fs::write(&inventory_path, &inventory)?;
            println!("[cluster] Generated {}", inventory_path.display());
        }

        // Generate cluster vars
        let vars = kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);
        let vars_path = cluster_dir.join("cluster-vars.yml");
        if dry_run {
            println!("[dry-run] cluster-vars.yml:\n{}", vars);
        } else {
            std::fs::write(&vars_path, &vars)?;
            println!("[cluster] Generated {}", vars_path.display());
        }

        // Run kubespray
        if !dry_run {
            println!(
                "[cluster] Running kubespray for {}...",
                cluster.cluster_name
            );
            run_kubespray(&cluster_dir, &cluster.cluster_name)?;

            // Collect kubeconfig
            println!(
                "[cluster] Collecting kubeconfig for {}...",
                cluster.cluster_name
            );
            collect_kubeconfig(&cluster_dir, &cluster.cluster_name, &sdi_spec, cluster)?;
        } else {
            println!(
                "[dry-run] Would run kubespray and collect kubeconfig for {}",
                cluster.cluster_name
            );
        }
    }

    println!("\n[cluster] All clusters initialized.");

    // Summary
    println!("\n[cluster] Kubeconfig files:");
    for cluster in &k8s_config.config.clusters {
        let kc_path = output_dir
            .join(&cluster.cluster_name)
            .join("kubeconfig.yaml");
        if kc_path.exists() {
            println!("  {} -> {}", cluster.cluster_name, kc_path.display());
        } else if !dry_run {
            println!("  {} -> (not yet available)", cluster.cluster_name);
        }
    }

    Ok(())
}

fn load_sdi_spec_from_options(
    sdi_spec_path: Option<String>,
    sdi_dir: &std::path::Path,
) -> anyhow::Result<SdiSpec> {
    // Try explicit spec file first
    if let Some(ref path) = sdi_spec_path {
        let raw = std::fs::read_to_string(path)?;
        return Ok(serde_yaml::from_str(&raw)?);
    }

    // Try to reconstruct from sdi-state.json (minimal approach)
    let state_path = sdi_dir.join("sdi-state.json");
    if state_path.exists() {
        // We need the actual spec, not just state. Look for cached spec.
        let spec_cache = sdi_dir.join("sdi-spec-cache.yaml");
        if spec_cache.exists() {
            let raw = std::fs::read_to_string(&spec_cache)?;
            return Ok(serde_yaml::from_str(&raw)?);
        }
    }

    anyhow::bail!(
        "SDI spec required. Provide --sdi-spec <path> or run `scalex sdi init <spec>` first."
    );
}

fn run_kubespray(cluster_dir: &std::path::Path, cluster_name: &str) -> anyhow::Result<()> {
    let inventory_path = cluster_dir.join("inventory.ini");
    let vars_path = cluster_dir.join("cluster-vars.yml");

    let output = std::process::Command::new("ansible-playbook")
        .args([
            "-i",
            &inventory_path.display().to_string(),
            "cluster.yml",
            "-e",
            &format!("@{}", vars_path.display()),
            "--become",
        ])
        .current_dir(find_kubespray_dir()?)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stdout.is_empty() {
                println!("{}", stdout);
            }
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("[cluster] kubespray error for {}: {}", cluster_name, stderr);
                anyhow::bail!("kubespray failed for cluster '{}'", cluster_name);
            }
            Ok(())
        }
        Err(e) => {
            anyhow::bail!(
                "Failed to run ansible-playbook: {}. Is kubespray installed?",
                e
            );
        }
    }
}

fn find_kubespray_dir() -> anyhow::Result<String> {
    // Check common locations
    let candidates = [
        "kubespray",
        "../kubespray",
        "/opt/kubespray",
        ".legacy-datax-kubespray",
    ];
    for dir in &candidates {
        if std::path::Path::new(dir).join("cluster.yml").exists() {
            return Ok(dir.to_string());
        }
    }
    anyhow::bail!(
        "kubespray directory not found. Expected cluster.yml in one of: {:?}",
        candidates
    );
}

fn collect_kubeconfig(
    cluster_dir: &std::path::Path,
    cluster_name: &str,
    sdi_spec: &Option<SdiSpec>,
    cluster: &crate::models::cluster::ClusterDef,
) -> anyhow::Result<()> {
    // Find the first control-plane node IP based on cluster mode
    let cp_ip: Option<String> = match cluster.cluster_mode {
        ClusterMode::Baremetal => cluster
            .baremetal_nodes
            .iter()
            .find(|n| n.roles.iter().any(|r| r == "control-plane"))
            .map(|n| n.ip.clone()),
        ClusterMode::Sdi => sdi_spec.as_ref().and_then(|spec| {
            spec.spec
                .sdi_pools
                .iter()
                .find(|p| p.pool_name == cluster.cluster_sdi_resource_pool)
                .and_then(|p| {
                    p.node_specs
                        .iter()
                        .find(|n| n.roles.iter().any(|r| r == "control-plane"))
                        .map(|n| n.ip.clone())
                })
        }),
    };

    let Some(ip) = cp_ip else {
        eprintln!("[cluster] No control-plane node found for {}", cluster_name);
        return Ok(());
    };

    // SCP kubeconfig from control plane
    let remote_path = "/etc/kubernetes/admin.conf";
    let local_path = cluster_dir.join("kubeconfig.yaml");

    let output = std::process::Command::new("scp")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            &format!("root@{ip}:{remote_path}"),
            &local_path.display().to_string(),
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            println!(
                "[cluster] kubeconfig for {} -> {}",
                cluster_name,
                local_path.display()
            );
        }
        _ => {
            eprintln!(
                "[cluster] Failed to collect kubeconfig from {} for {}",
                ip, cluster_name
            );
        }
    }

    Ok(())
}
