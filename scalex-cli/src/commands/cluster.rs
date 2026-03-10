use crate::core::{gitops, kubespray, validation};
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

    // Step 2.5: Cross-config validation (pure functions)
    let id_errors = validation::validate_unique_cluster_ids(&k8s_config);
    if !id_errors.is_empty() {
        eprintln!("[cluster] Cluster ID validation errors:");
        for err in &id_errors {
            eprintln!("  - {}", err);
        }
        anyhow::bail!(
            "Fix {} cluster ID error(s) before proceeding",
            id_errors.len()
        );
    }

    if let Some(ref spec) = sdi_spec {
        let pool_errors = validation::validate_cluster_sdi_pool_mapping(&k8s_config, spec);
        if !pool_errors.is_empty() {
            eprintln!("[cluster] SDI pool mapping errors:");
            for err in &pool_errors {
                eprintln!("  - {}", err);
            }
            anyhow::bail!(
                "Fix {} pool mapping error(s) before proceeding",
                pool_errors.len()
            );
        }

        let spec_errors = validation::validate_sdi_spec(spec);
        if !spec_errors.is_empty() {
            eprintln!("[cluster] SDI spec validation errors:");
            for err in &spec_errors {
                eprintln!("  - {}", err);
            }
            anyhow::bail!(
                "Fix {} SDI spec error(s) before proceeding",
                spec_errors.len()
            );
        }
    }

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

    // Step 4: Update GitOps Cilium values with correct control-plane IPs
    for cluster in &k8s_config.config.clusters {
        let cp_ip = find_control_plane_ip(cluster, sdi_spec.as_ref());
        if let Some(ip) = cp_ip {
            update_gitops_cilium_values(
                &cluster.cluster_name,
                &ip,
                &k8s_config.config.common.cilium_version,
                dry_run,
            )?;
        }
    }

    // Step 5: Update GitOps sandbox server URLs from collected kubeconfigs
    for cluster in &k8s_config.config.clusters {
        if cluster.cluster_role != "management" {
            let kc_path = output_dir
                .join(&cluster.cluster_name)
                .join("kubeconfig.yaml");
            if kc_path.exists() {
                if let Ok(kc_content) = std::fs::read_to_string(&kc_path) {
                    if let Some(server_url) = gitops::extract_server_from_kubeconfig(&kc_content) {
                        println!(
                            "[cluster] Updating gitops sandbox URLs with: {}",
                            server_url
                        );
                        update_gitops_sandbox_urls(&server_url, dry_run)?;
                    }
                }
            }
        }
    }

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

fn update_gitops_sandbox_urls(server_url: &str, dry_run: bool) -> anyhow::Result<()> {
    let gitops_dir = std::path::Path::new("gitops");
    for rel_path in gitops::gitops_files_needing_replacement() {
        let full_path = gitops_dir.join(rel_path);
        if full_path.exists() {
            let content = std::fs::read_to_string(&full_path)?;
            if gitops::has_sandbox_placeholder(&content) {
                let updated = gitops::replace_sandbox_server_url(&content, server_url);
                if dry_run {
                    println!("[dry-run] Would update {} with server URL", rel_path);
                } else {
                    std::fs::write(&full_path, &updated)?;
                    println!("[cluster] Updated {} with sandbox server URL", rel_path);
                }
            }
        }
    }
    Ok(())
}

/// Update gitops Cilium values.yaml for a cluster with the correct control-plane IP.
/// I/O function: reads and writes gitops/{cluster}/cilium/values.yaml.
fn update_gitops_cilium_values(
    cluster_name: &str,
    control_plane_ip: &str,
    cilium_version: &str,
    dry_run: bool,
) -> anyhow::Result<()> {
    let gitops_dir = std::path::Path::new("gitops");
    let values_path = gitops_dir.join(gitops::cilium_values_path(cluster_name));
    let kust_path = gitops_dir.join(gitops::cilium_kustomization_path(cluster_name));

    let values_content = gitops::generate_cilium_values(control_plane_ip, 6443);
    let kust_content = gitops::generate_cilium_kustomization(cilium_version);

    if dry_run {
        println!(
            "[dry-run] Would write Cilium values for {} (CP: {})",
            cluster_name, control_plane_ip
        );
    } else {
        if let Some(parent) = values_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&values_path, &values_content)?;
        std::fs::write(&kust_path, &kust_content)?;
        println!(
            "[cluster] Updated gitops/{}/cilium/ with CP IP {}",
            cluster_name, control_plane_ip
        );
    }
    Ok(())
}

/// Determine which clusters require an SDI spec for inventory generation.
/// Pure function: returns list of cluster names that use SDI mode.
#[cfg(test)]
pub fn clusters_requiring_sdi(config: &K8sClustersConfig) -> Vec<String> {
    config
        .config
        .clusters
        .iter()
        .filter(|c| c.cluster_mode == ClusterMode::Sdi)
        .map(|c| c.cluster_name.clone())
        .collect()
}

/// Find the control-plane node IP for a given cluster.
/// Pure function: searches SDI spec or baremetal nodes for control-plane role.
pub fn find_control_plane_ip(
    cluster: &crate::models::cluster::ClusterDef,
    sdi_spec: Option<&SdiSpec>,
) -> Option<String> {
    match cluster.cluster_mode {
        ClusterMode::Baremetal => cluster
            .baremetal_nodes
            .iter()
            .find(|n| n.roles.iter().any(|r| r == "control-plane"))
            .map(|n| n.ip.clone()),
        ClusterMode::Sdi => sdi_spec.and_then(|spec| {
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
    }
}

/// Determine which clusters need GitOps sandbox URL updates.
/// Non-management clusters that have kubeconfigs need their URLs replaced.
/// Pure function: returns cluster names needing URL updates.
#[cfg(test)]
pub fn clusters_needing_gitops_update(config: &K8sClustersConfig) -> Vec<String> {
    config
        .config
        .clusters
        .iter()
        .filter(|c| c.cluster_role != "management")
        .map(|c| c.cluster_name.clone())
        .collect()
}

fn collect_kubeconfig(
    cluster_dir: &std::path::Path,
    cluster_name: &str,
    sdi_spec: &Option<SdiSpec>,
    cluster: &crate::models::cluster::ClusterDef,
) -> anyhow::Result<()> {
    let cp_ip = find_control_plane_ip(cluster, sdi_spec.as_ref());

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::cluster::*;
    use crate::models::sdi::*;

    fn make_common() -> CommonConfig {
        serde_yaml::from_str(
            r#"
kubernetes_version: "v1.32.0"
kubespray_version: "v2.30.0"
"#,
        )
        .unwrap()
    }

    fn make_sdi_cluster(name: &str, pool: &str) -> ClusterDef {
        ClusterDef {
            cluster_name: name.to_string(),
            cluster_mode: ClusterMode::Sdi,
            cluster_sdi_resource_pool: pool.to_string(),
            baremetal_nodes: vec![],
            cluster_role: "management".to_string(),
            network: ClusterNetwork {
                pod_cidr: "10.233.0.0/18".to_string(),
                service_cidr: "10.233.64.0/18".to_string(),
                dns_domain: "cluster.local".to_string(),
                native_routing_cidr: None,
            },
            cilium: None,
            oidc: None,
            kubespray_extra_vars: None,
        }
    }

    fn make_baremetal_cluster(name: &str, role: &str) -> ClusterDef {
        ClusterDef {
            cluster_name: name.to_string(),
            cluster_mode: ClusterMode::Baremetal,
            cluster_sdi_resource_pool: String::new(),
            baremetal_nodes: vec![BaremetalNode {
                node_name: "bm-cp-0".to_string(),
                ip: "10.0.0.50".to_string(),
                roles: vec!["control-plane".to_string(), "etcd".to_string()],
            }],
            cluster_role: role.to_string(),
            network: ClusterNetwork {
                pod_cidr: "10.234.0.0/18".to_string(),
                service_cidr: "10.234.64.0/18".to_string(),
                dns_domain: "cluster.local".to_string(),
                native_routing_cidr: None,
            },
            cilium: None,
            oidc: None,
            kubespray_extra_vars: None,
        }
    }

    // --- clusters_requiring_sdi ---

    #[test]
    fn test_clusters_requiring_sdi_mixed() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: make_common(),
                clusters: vec![
                    make_sdi_cluster("tower", "tower-pool"),
                    make_baremetal_cluster("edge", "workload"),
                    make_sdi_cluster("sandbox", "sandbox-pool"),
                ],
                argocd: None,
                domains: None,
            },
        };

        let sdi_clusters = clusters_requiring_sdi(&config);
        assert_eq!(sdi_clusters, vec!["tower", "sandbox"]);
    }

    #[test]
    fn test_clusters_requiring_sdi_all_baremetal() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: make_common(),
                clusters: vec![make_baremetal_cluster("edge", "workload")],
                argocd: None,
                domains: None,
            },
        };

        let sdi_clusters = clusters_requiring_sdi(&config);
        assert!(sdi_clusters.is_empty());
    }

    // --- find_control_plane_ip ---

    #[test]
    fn test_find_cp_ip_baremetal() {
        let cluster = make_baremetal_cluster("edge", "workload");
        let ip = find_control_plane_ip(&cluster, None);
        assert_eq!(ip, Some("10.0.0.50".to_string()));
    }

    #[test]
    fn test_find_cp_ip_sdi() {
        let cluster = make_sdi_cluster("tower", "tower-pool");
        let spec = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "pool".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec![],
                },
            },
            os_image: OsImageConfig {
                source: "img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "keys".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![SdiPool {
                    pool_name: "tower-pool".to_string(),
                    purpose: "management".to_string(),
                    placement: PlacementConfig {
                        hosts: vec!["playbox-0".to_string()],
                        spread: false,
                    },
                    node_specs: vec![NodeSpec {
                        node_name: "tower-cp-0".to_string(),
                        ip: "192.168.88.100".to_string(),
                        cpu: 4,
                        mem_gb: 8,
                        disk_gb: 50,
                        host: None,
                        roles: vec!["control-plane".to_string()],
                        devices: None,
                    }],
                }],
            },
        };

        let ip = find_control_plane_ip(&cluster, Some(&spec));
        assert_eq!(ip, Some("192.168.88.100".to_string()));
    }

    #[test]
    fn test_find_cp_ip_sdi_no_matching_pool() {
        let cluster = make_sdi_cluster("tower", "nonexistent-pool");
        let spec = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "pool".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "10.0.0.0/24".to_string(),
                    gateway: "10.0.0.1".to_string(),
                    nameservers: vec![],
                },
            },
            os_image: OsImageConfig {
                source: "img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "keys".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec { sdi_pools: vec![] },
        };

        let ip = find_control_plane_ip(&cluster, Some(&spec));
        assert_eq!(ip, None);
    }

    // --- clusters_needing_gitops_update ---

    #[test]
    fn test_gitops_update_targets() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: make_common(),
                clusters: vec![
                    make_sdi_cluster("tower", "tower-pool"), // role = management
                    {
                        let mut c = make_sdi_cluster("sandbox", "sandbox-pool");
                        c.cluster_role = "workload".to_string();
                        c
                    },
                ],
                argocd: None,
                domains: None,
            },
        };

        let targets = clusters_needing_gitops_update(&config);
        assert_eq!(targets, vec!["sandbox"]);
    }

    #[test]
    fn test_gitops_update_no_targets_all_management() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: make_common(),
                clusters: vec![make_sdi_cluster("tower", "tower-pool")],
                argocd: None,
                domains: None,
            },
        };

        let targets = clusters_needing_gitops_update(&config);
        assert!(targets.is_empty());
    }
}
