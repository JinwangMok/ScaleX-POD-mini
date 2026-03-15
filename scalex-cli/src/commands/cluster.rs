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
    let name_errors = validation::validate_unique_cluster_names(&k8s_config);
    if !name_errors.is_empty() {
        eprintln!("[cluster] Cluster name validation errors:");
        for err in &name_errors {
            eprintln!("  - {}", err);
        }
        anyhow::bail!(
            "Fix {} cluster name error(s) before proceeding",
            name_errors.len()
        );
    }

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

    let net_errors = validation::validate_cluster_network_overlap(&k8s_config);
    if !net_errors.is_empty() {
        eprintln!("[cluster] Network overlap errors:");
        for err in &net_errors {
            eprintln!("  - {}", err);
        }
        anyhow::bail!(
            "Fix {} network overlap error(s) before proceeding",
            net_errors.len()
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
            // Wait for cloud-init to finish on all nodes before kubespray
            wait_for_cloud_init(&cluster_dir, &cluster.cluster_name)?;

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

/// Wait for cloud-init to finish on all nodes in a cluster.
/// Parses inventory.ini to get node IPs, then SSH-polls until cloud-init reports done.
fn wait_for_cloud_init(cluster_dir: &std::path::Path, cluster_name: &str) -> anyhow::Result<()> {
    let inventory_path = cluster_dir.join("inventory.ini");
    if !inventory_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&inventory_path)?;
    let mut ips: Vec<String> = Vec::new();
    for line in content.lines() {
        // Parse lines like: tower-cp-0 ansible_host=192.168.88.100 ...
        if let Some(ip_part) = line.split_whitespace().find(|s| s.starts_with("ansible_host=")) {
            if let Some(ip) = ip_part.strip_prefix("ansible_host=") {
                ips.push(ip.to_string());
            }
        }
    }

    if ips.is_empty() {
        return Ok(());
    }

    println!(
        "[cluster] Waiting for cloud-init on {} ({} nodes)...",
        cluster_name,
        ips.len()
    );

    let max_attempts = 60; // 5 minutes (60 * 5s)
    for ip in &ips {
        for attempt in 1..=max_attempts {
            let result = std::process::Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=5",
                    "-o",
                    "StrictHostKeyChecking=no",
                    "-o",
                    "BatchMode=yes",
                    &format!("jinwang@{}", ip),
                    "cloud-init status --wait 2>/dev/null || test -f /var/lib/cloud/instance/boot-finished",
                ])
                .output();

            match result {
                Ok(out) if out.status.success() => {
                    println!("[cluster] cloud-init done on {}", ip);
                    break;
                }
                _ => {
                    if attempt == max_attempts {
                        eprintln!(
                            "[cluster] WARNING: cloud-init timeout on {} — proceeding anyway",
                            ip
                        );
                    } else if attempt % 12 == 0 {
                        println!(
                            "[cluster] Still waiting for cloud-init on {} ({}/{})",
                            ip, attempt, max_attempts
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            }
        }
    }

    Ok(())
}

fn run_kubespray(cluster_dir: &std::path::Path, cluster_name: &str) -> anyhow::Result<()> {
    // Use absolute paths since current_dir changes to kubespray dir
    let inventory_path = std::fs::canonicalize(cluster_dir.join("inventory.ini"))
        .unwrap_or_else(|_| cluster_dir.join("inventory.ini"));
    let vars_path = std::fs::canonicalize(cluster_dir.join("cluster-vars.yml"))
        .unwrap_or_else(|_| cluster_dir.join("cluster-vars.yml"));

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

/// Candidate paths to search for kubespray's cluster.yml.
/// Pure function: returns the list of candidate directories to check.
pub fn kubespray_candidate_paths() -> Vec<&'static str> {
    vec![
        "kubespray/kubespray", // Git submodule (primary)
        "kubespray",           // Direct clone
        "../kubespray",        // Sibling directory
        "/opt/kubespray",      // System-wide install
    ]
}

fn find_kubespray_dir() -> anyhow::Result<String> {
    let candidates = kubespray_candidate_paths();
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

/// Build SCP arguments for kubeconfig collection.
/// Pure function: no I/O, no side effects.
/// Uses the provided admin_user instead of hardcoded root for security.
pub fn build_kubeconfig_scp_args(admin_user: &str, ip: &str, local_path: &str) -> Vec<String> {
    vec![
        "-o".to_string(),
        "StrictHostKeyChecking=no".to_string(),
        format!("{}@{}:/etc/kubernetes/admin.conf", admin_user, ip),
        local_path.to_string(),
    ]
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

    // SCP kubeconfig from control plane using ssh_user from cluster config
    let ssh_user = cluster.ssh_user.as_deref().unwrap_or("root");
    let local_path = cluster_dir.join("kubeconfig.yaml");
    let scp_args = build_kubeconfig_scp_args(ssh_user, &ip, &local_path.display().to_string());

    let output = std::process::Command::new("scp").args(&scp_args).output();

    match output {
        Ok(out) if out.status.success() => {
            // Replace 127.0.0.1 with the control plane IP so kubeconfig works from bastion
            if let Ok(content) = std::fs::read_to_string(&local_path) {
                let fixed = content.replace("https://127.0.0.1:", &format!("https://{}:", ip));
                if fixed != content {
                    let _ = std::fs::write(&local_path, &fixed);
                    println!(
                        "[cluster] kubeconfig for {} -> {} (server: {})",
                        cluster_name,
                        local_path.display(),
                        ip
                    );
                } else {
                    println!(
                        "[cluster] kubeconfig for {} -> {}",
                        cluster_name,
                        local_path.display()
                    );
                }
            }
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
            ssh_user: None,
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
            ssh_user: None,
        }
    }

    // --- build_kubeconfig_scp_args ---

    #[test]
    fn test_kubeconfig_scp_uses_admin_user_not_root() {
        let args = build_kubeconfig_scp_args("jinwang", "192.168.88.100", "/tmp/kube.yaml");
        // Must NOT contain root@
        let joined = args.join(" ");
        assert!(
            !joined.contains("root@"),
            "kubeconfig SCP must use admin_user, not root — found: {}",
            joined
        );
        assert!(
            joined.contains("jinwang@192.168.88.100"),
            "kubeconfig SCP must use admin_user@ip — found: {}",
            joined
        );
    }

    #[test]
    fn test_kubeconfig_scp_args_structure() {
        let args = build_kubeconfig_scp_args("admin", "10.0.0.1", "/out/kubeconfig.yaml");
        assert!(args.iter().any(|a| a == "StrictHostKeyChecking=no"));
        assert!(args
            .iter()
            .any(|a| a.contains("/etc/kubernetes/admin.conf")));
        assert!(args.iter().any(|a| a == "/out/kubeconfig.yaml"));
    }

    /// CL-4: Verify ssh_user from cluster config is used in SCP args (not hardcoded root).
    #[test]
    fn test_kubeconfig_scp_uses_cluster_ssh_user() {
        // When ssh_user is set, it should be used
        let args = build_kubeconfig_scp_args("jinwang", "10.0.0.1", "/tmp/kube.yaml");
        let scp_target = args.iter().find(|a| a.contains('@')).unwrap();
        assert!(
            scp_target.starts_with("jinwang@"),
            "must use cluster ssh_user, got: {}",
            scp_target
        );
        assert!(
            !scp_target.starts_with("root@"),
            "must NOT use root, got: {}",
            scp_target
        );
    }

    /// CL-8: Verify k8s-clusters.yaml.example parses with ssh_user field.
    #[test]
    fn test_example_config_ssh_user_field() {
        let content = include_str!("../../../config/k8s-clusters.yaml.example");
        let config: K8sClustersConfig = serde_yaml::from_str(content).unwrap();
        // Tower cluster should have ssh_user set
        let tower = config
            .config
            .clusters
            .iter()
            .find(|c| c.cluster_name == "tower")
            .unwrap();
        assert_eq!(
            tower.ssh_user,
            Some("jinwang".to_string()),
            "tower cluster must have ssh_user set in example config"
        );
        // Sandbox cluster should not have ssh_user set (defaults to None → root)
        let sandbox = config
            .config
            .clusters
            .iter()
            .find(|c| c.cluster_name == "sandbox")
            .unwrap();
        assert_eq!(
            sandbox.ssh_user, None,
            "sandbox must default to None (root fallback)"
        );
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

    // --- kubespray_candidate_paths (Sprint 15a: C-3 fix) ---

    #[test]
    fn test_kubespray_candidates_includes_submodule_path() {
        let candidates = super::kubespray_candidate_paths();
        assert!(
            candidates.contains(&"kubespray/kubespray"),
            "Must include git submodule path 'kubespray/kubespray' — got: {:?}",
            candidates
        );
    }

    #[test]
    fn test_kubespray_submodule_path_is_first_candidate() {
        let candidates = super::kubespray_candidate_paths();
        assert_eq!(
            candidates[0], "kubespray/kubespray",
            "Submodule path must be first candidate (highest priority) — got: {:?}",
            candidates
        );
    }

    #[test]
    fn test_kubespray_candidates_no_legacy_only_paths() {
        let candidates = super::kubespray_candidate_paths();
        // Ensure the old bug (missing submodule path) cannot regress
        let has_submodule = candidates.iter().any(|c| c.contains("kubespray/kubespray"));
        assert!(
            has_submodule,
            "Regression: kubespray submodule path missing from candidates"
        );
    }

    // --- Sprint 31d: find_control_plane_ip edge cases ---

    #[test]
    fn test_find_cp_ip_sdi_no_cp_role_in_pool() {
        // Pool exists but has only worker nodes — no control-plane
        let cluster = make_sdi_cluster("sandbox", "sandbox-pool");
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
            spec: SdiPoolsSpec {
                sdi_pools: vec![SdiPool {
                    pool_name: "sandbox-pool".to_string(),
                    purpose: "workload".to_string(),
                    placement: PlacementConfig {
                        hosts: vec!["h0".to_string()],
                        spread: false,
                    },
                    node_specs: vec![NodeSpec {
                        node_name: "worker-0".to_string(),
                        ip: "10.0.0.50".to_string(),
                        cpu: 4,
                        mem_gb: 8,
                        disk_gb: 50,
                        host: None,
                        roles: vec!["worker".to_string()], // no control-plane
                        devices: None,
                    }],
                }],
            },
        };

        let ip = find_control_plane_ip(&cluster, Some(&spec));
        assert_eq!(
            ip, None,
            "must return None when pool has no control-plane nodes"
        );
    }

    #[test]
    fn test_find_cp_ip_baremetal_no_cp_role() {
        let mut cluster = make_baremetal_cluster("edge", "workload");
        // Override the node to only have worker role
        cluster.baremetal_nodes = vec![BaremetalNode {
            node_name: "bm-w-0".to_string(),
            ip: "10.0.0.50".to_string(),
            roles: vec!["worker".to_string()],
        }];

        let ip = find_control_plane_ip(&cluster, None);
        assert_eq!(
            ip, None,
            "must return None when baremetal has no control-plane"
        );
    }

    #[test]
    fn test_find_cp_ip_sdi_without_spec_returns_none() {
        // SDI mode but no spec provided (edge case during partial workflow)
        let cluster = make_sdi_cluster("tower", "tower-pool");
        let ip = find_control_plane_ip(&cluster, None);
        assert_eq!(ip, None, "SDI mode without spec must return None");
    }

    // --- Sprint 31d: clusters_needing_gitops_update with 3+ clusters ---

    #[test]
    fn test_gitops_update_targets_three_clusters() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: make_common(),
                clusters: vec![
                    make_sdi_cluster("tower", "tower-pool"), // management
                    {
                        let mut c = make_sdi_cluster("sandbox", "sandbox-pool");
                        c.cluster_role = "workload".to_string();
                        c
                    },
                    {
                        let mut c = make_baremetal_cluster("prod", "workload");
                        c.cluster_role = "workload".to_string();
                        c
                    },
                ],
                argocd: None,
                domains: None,
            },
        };

        let targets = clusters_needing_gitops_update(&config);
        assert_eq!(targets.len(), 2, "both non-management clusters need update");
        assert!(targets.contains(&"sandbox".to_string()));
        assert!(targets.contains(&"prod".to_string()));
    }

    // --- Sprint 31d: build_kubeconfig_scp_args defaults ---

    #[test]
    fn test_kubeconfig_scp_default_root_user() {
        // When ssh_user is None, collect_kubeconfig falls back to "root"
        let args = build_kubeconfig_scp_args("root", "192.168.88.100", "/out/kube.yaml");
        let scp_target = args.iter().find(|a| a.contains('@')).unwrap();
        assert!(scp_target.starts_with("root@"), "default must be root@");
    }

    // ===== Sprint 33c: Cluster Pipeline Integration Tests =====

    #[test]
    fn test_sprint33c_example_configs_inventory_generation_e2e() {
        // cluster init: sdi-specs + k8s-clusters → generate_inventory for each cluster
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s_config.config.clusters {
            let inventory = crate::core::kubespray::generate_inventory(cluster, &sdi_spec)
                .unwrap_or_else(|e| {
                    panic!(
                        "generate_inventory must succeed for cluster '{}': {}",
                        cluster.cluster_name, e
                    )
                });

            // Must contain [all], [kube_control_plane], [etcd], [kube_node], [k8s_cluster:children]
            assert!(
                inventory.contains("[all]"),
                "{}: missing [all]",
                cluster.cluster_name
            );
            assert!(
                inventory.contains("[kube_control_plane]"),
                "{}: missing [kube_control_plane]",
                cluster.cluster_name
            );
            assert!(
                inventory.contains("[etcd]"),
                "{}: missing [etcd]",
                cluster.cluster_name
            );
            assert!(
                inventory.contains("[kube_node]"),
                "{}: missing [kube_node]",
                cluster.cluster_name
            );
            assert!(
                inventory.contains("[k8s_cluster:children]"),
                "{}: missing [k8s_cluster:children]",
                cluster.cluster_name
            );
        }
    }

    #[test]
    fn test_sprint33c_two_cluster_inventories_no_ip_overlap() {
        // tower and sandbox inventories must not share any IPs
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let mut all_ips: Vec<(String, String)> = Vec::new(); // (cluster, ip)

        for cluster in &k8s_config.config.clusters {
            let inventory = crate::core::kubespray::generate_inventory(cluster, &sdi_spec).unwrap();
            // Extract IPs from ansible_host=<ip> entries
            for line in inventory.lines() {
                if let Some(ip_part) = line.split("ansible_host=").nth(1) {
                    let ip = ip_part.split_whitespace().next().unwrap_or("");
                    all_ips.push((cluster.cluster_name.clone(), ip.to_string()));
                }
            }
        }

        // Check for duplicates across clusters
        for i in 0..all_ips.len() {
            for j in (i + 1)..all_ips.len() {
                if all_ips[i].0 != all_ips[j].0 && all_ips[i].1 == all_ips[j].1 {
                    panic!(
                        "IP overlap between clusters: {} ({}) and {} ({})",
                        all_ips[i].0, all_ips[i].1, all_ips[j].0, all_ips[j].1
                    );
                }
            }
        }

        assert!(all_ips.len() >= 2, "Must have IPs from at least 2 clusters");
    }

    #[test]
    fn test_sprint33c_control_plane_ip_to_cilium_values_pipeline() {
        // find_control_plane_ip → generate_cilium_values: data flows correctly
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s_config.config.clusters {
            let cp_ip = find_control_plane_ip(cluster, Some(&sdi_spec));
            assert!(
                cp_ip.is_some(),
                "Cluster '{}' must have a control-plane IP",
                cluster.cluster_name
            );

            let ip = cp_ip.unwrap();
            let values = crate::core::gitops::generate_cilium_values(&ip, 6443);

            // values.yaml must contain the exact CP IP
            assert!(
                values.contains(&ip),
                "Cilium values for '{}' must contain CP IP '{}' — got:\n{}",
                cluster.cluster_name,
                ip,
                values
            );
            assert!(
                values.contains("k8sServiceHost"),
                "Cilium values must contain k8sServiceHost"
            );
            assert!(
                values.contains("6443"),
                "Cilium values must contain port 6443"
            );
        }
    }

    #[test]
    fn test_sprint33c_cluster_vars_generation_e2e() {
        // generate_cluster_vars must produce valid YAML-like output for each cluster
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s_config.config.clusters {
            let vars =
                crate::core::kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);

            // Must contain critical Kubespray variables
            assert!(
                vars.contains("kube_version"),
                "{}: must contain kube_version",
                cluster.cluster_name
            );
            assert!(
                vars.contains("container_manager:") && vars.contains("containerd"),
                "{}: must use containerd runtime",
                cluster.cluster_name
            );
            assert!(
                vars.contains(&cluster.network.pod_cidr),
                "{}: must contain pod_cidr {}",
                cluster.cluster_name,
                cluster.network.pod_cidr
            );
            assert!(
                vars.contains(&cluster.network.service_cidr),
                "{}: must contain service_cidr {}",
                cluster.cluster_name,
                cluster.network.service_cidr
            );
            assert!(
                vars.contains(&cluster.network.dns_domain),
                "{}: must contain dns_domain {}",
                cluster.cluster_name,
                cluster.network.dns_domain
            );
        }
    }

    // ── Sprint 37: Dry-run E2E pipeline tests (CL-7) ──

    /// Full pipeline: load both example configs → validate → generate inventory+vars
    /// for every cluster. This simulates what `scalex cluster init` does minus I/O.
    #[test]
    fn test_full_dryrun_pipeline_both_configs() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        // Step 1: Validate cross-references (every SDI cluster must have matching pool)
        let mapping_errors =
            crate::core::validation::validate_cluster_sdi_pool_mapping(&k8s_config, &sdi_spec);
        assert!(
            mapping_errors.is_empty(),
            "pool mapping errors: {:?}",
            mapping_errors
        );

        // Step 2: Validate SDI spec itself
        let sdi_errors = crate::core::validation::validate_sdi_spec(&sdi_spec);
        assert!(sdi_errors.is_empty(), "SDI spec errors: {:?}", sdi_errors);

        // Step 3: Validate unique cluster names and IDs
        let name_errors = crate::core::validation::validate_unique_cluster_names(&k8s_config);
        assert!(
            name_errors.is_empty(),
            "duplicate cluster names: {:?}",
            name_errors
        );

        let id_errors = crate::core::validation::validate_unique_cluster_ids(&k8s_config);
        assert!(
            id_errors.is_empty(),
            "duplicate cluster IDs: {:?}",
            id_errors
        );

        // Step 4: Validate no network overlap
        let net_errors = crate::core::validation::validate_cluster_network_overlap(&k8s_config);
        assert!(net_errors.is_empty(), "network overlap: {:?}", net_errors);

        // Step 5: Generate inventory + vars for each cluster
        for cluster in &k8s_config.config.clusters {
            let ini = crate::core::kubespray::generate_inventory(cluster, &sdi_spec)
                .unwrap_or_else(|e| {
                    panic!(
                        "inventory generation failed for {}: {}",
                        cluster.cluster_name, e
                    )
                });

            // Inventory must have all required sections
            for section in &[
                "[all]",
                "[kube_control_plane]",
                "[etcd]",
                "[kube_node]",
                "[k8s_cluster:children]",
            ] {
                assert!(
                    ini.contains(section),
                    "{}: missing section {}",
                    cluster.cluster_name,
                    section
                );
            }

            let vars =
                crate::core::kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);

            // Vars must contain essential Kubespray settings
            assert!(
                vars.contains("kube_version:"),
                "{}: missing kube_version",
                cluster.cluster_name
            );
            assert!(
                vars.contains("container_manager:"),
                "{}: missing container_manager",
                cluster.cluster_name
            );
            assert!(
                vars.contains("kube_pods_subnet:"),
                "{}: missing kube_pods_subnet",
                cluster.cluster_name
            );
            assert!(
                vars.contains("kube_service_addresses:"),
                "{}: missing kube_service_addresses",
                cluster.cluster_name
            );
        }
    }

    /// Verify that the pipeline produces distinct, non-overlapping inventories
    /// for tower vs sandbox clusters.
    #[test]
    fn test_dryrun_pipeline_inventories_are_distinct() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let inventories: Vec<(String, String)> = k8s_config
            .config
            .clusters
            .iter()
            .map(|c| {
                let ini = crate::core::kubespray::generate_inventory(c, &sdi_spec).unwrap();
                (c.cluster_name.clone(), ini)
            })
            .collect();

        assert!(
            inventories.len() >= 2,
            "example config must define at least 2 clusters"
        );

        // Collect all IPs from each cluster inventory
        let extract_ips = |ini: &str| -> Vec<String> {
            ini.lines()
                .filter_map(|line| {
                    line.split("ansible_host=")
                        .nth(1)
                        .and_then(|s| s.split_whitespace().next())
                        .map(|s| s.to_string())
                })
                .collect()
        };

        let tower_ips = extract_ips(&inventories[0].1);
        let sandbox_ips = extract_ips(&inventories[1].1);

        // No IP should appear in both inventories
        for ip in &tower_ips {
            assert!(
                !sandbox_ips.contains(ip),
                "IP {} appears in both tower and sandbox inventories — clusters must be distinct",
                ip
            );
        }

        // Each cluster must have at least one node
        assert!(!tower_ips.is_empty(), "tower inventory has no nodes");
        assert!(!sandbox_ips.is_empty(), "sandbox inventory has no nodes");
    }

    /// Verify that cluster vars for different clusters have non-overlapping CIDRs
    /// and distinct cluster names/IDs.
    #[test]
    fn test_dryrun_pipeline_cluster_vars_distinct() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let all_vars: Vec<(String, String)> = k8s_config
            .config
            .clusters
            .iter()
            .map(|c| {
                let vars =
                    crate::core::kubespray::generate_cluster_vars(c, &k8s_config.config.common);
                (c.cluster_name.clone(), vars)
            })
            .collect();

        assert!(all_vars.len() >= 2);

        // Tower and sandbox must have different pod CIDRs
        let tower_vars = &all_vars[0].1;
        let sandbox_vars = &all_vars[1].1;

        assert_ne!(
            k8s_config.config.clusters[0].network.pod_cidr,
            k8s_config.config.clusters[1].network.pod_cidr,
            "clusters must have distinct pod CIDRs"
        );

        // Each must reference its own cluster_name
        assert!(
            tower_vars.contains("cluster_name: \"tower\""),
            "tower vars must contain tower cluster_name"
        );
        assert!(
            sandbox_vars.contains("cluster_name: \"sandbox\""),
            "sandbox vars must contain sandbox cluster_name"
        );

        // Cilium cluster IDs must differ
        if let (Some(t_cilium), Some(s_cilium)) = (
            &k8s_config.config.clusters[0].cilium,
            &k8s_config.config.clusters[1].cilium,
        ) {
            assert_ne!(
                t_cilium.cluster_id, s_cilium.cluster_id,
                "Cilium cluster IDs must be unique across clusters"
            );
        }
    }

    /// Single-node pipeline: 1 bare-metal → 1 SDI pool → 1 cluster → full validation.
    /// This tests the minimal viable deployment scenario (CL-1 philosophy).
    #[test]
    fn test_dryrun_single_node_pipeline_end_to_end() {
        let sdi_yaml = r#"
resource_pool:
  name: "single-pool"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.1.0/24"
    gateway: "192.168.1.1"
    nameservers: ["1.1.1.1"]
os_image:
  source: "https://example.com/img.qcow2"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id.pub"
spec:
  sdi_pools:
    - pool_name: "aio"
      purpose: "all-in-one"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "node-0"
          ip: "192.168.1.100"
          cpu: 4
          mem_gb: 8
          disk_gb: 50
          roles: [control-plane, worker]
"#;

        let k8s_yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
    cni: "cilium"
    cilium_version: "1.17.5"
    kube_proxy_remove: true
    helm_enabled: true
  clusters:
    - cluster_name: "mini"
      cluster_sdi_resource_pool: "aio"
      cluster_role: "management"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
        dns_domain: "mini.local"
      cilium:
        cluster_id: 1
        cluster_name: "mini"
"#;

        let sdi_spec: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_yaml).unwrap();

        // Validation
        let mapping_errors =
            crate::core::validation::validate_cluster_sdi_pool_mapping(&k8s_config, &sdi_spec);
        assert!(
            mapping_errors.is_empty(),
            "mapping errors: {:?}",
            mapping_errors
        );

        let sdi_errors = crate::core::validation::validate_sdi_spec(&sdi_spec);
        assert!(sdi_errors.is_empty(), "SDI errors: {:?}", sdi_errors);

        // Generate
        let cluster = &k8s_config.config.clusters[0];
        let ini = crate::core::kubespray::generate_inventory(cluster, &sdi_spec).unwrap();
        let vars =
            crate::core::kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);

        // Single node appears in all required sections
        assert!(ini.contains("node-0 ansible_host=192.168.1.100"));
        assert!(ini.contains("[kube_control_plane]\nnode-0"));
        assert!(ini.contains("[kube_node]\nnode-0"));

        // Vars contain essential settings
        assert!(vars.contains("kube_version: \"1.33.1\""));
        assert!(vars.contains("kube_pods_subnet: \"10.244.0.0/20\""));
        assert!(vars.contains("cilium_cluster_id: 1"));
    }

    /// Verify that generated HCL (OpenTofu) matches the SDI spec.
    /// Tests the SDI layer of the pipeline: sdi-specs → generate_tofu_main.
    #[test]
    fn test_dryrun_sdi_to_hcl_pipeline() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let hcl = crate::core::tofu::generate_tofu_main(&sdi_spec, "scalex");

        // Every node from every pool must appear in HCL
        for pool in &sdi_spec.spec.sdi_pools {
            for node in &pool.node_specs {
                assert!(
                    hcl.contains(&node.node_name),
                    "HCL must contain node '{}' from pool '{}'",
                    node.node_name,
                    pool.pool_name
                );
                assert!(
                    hcl.contains(&node.ip),
                    "HCL must contain IP '{}' for node '{}'",
                    node.ip,
                    node.node_name
                );
            }
        }

        // HCL must reference base volume by name (pre-created via virsh)
        assert!(
            hcl.contains("base_volume_name"),
            "HCL must reference base volume by name"
        );

        // HCL must reference the network bridge
        assert!(
            hcl.contains(&sdi_spec.resource_pool.network.management_bridge),
            "HCL must reference management bridge"
        );
    }

    /// Sprint 45: Cross-module chain test — facts JSON roundtrip.
    /// Verifies that NodeFacts can be serialized to JSON and deserialized back,
    /// preserving all fields needed by sdi init.
    #[test]
    fn test_chain_facts_json_roundtrip_for_sdi_consumption() {
        use crate::models::baremetal::NodeFacts;

        // Simulate facts output using the actual marker/section format
        let raw = r#"some preamble
---SCALEX_FACTS_START---
cpu_model=Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz
cpu_cores=6
cpu_threads=12
cpu_arch=x86_64
mem_total_kb=32768000
mem_avail_kb=28000000
kernel_version=6.8.0-45-generic
---DISKS---
nvme0n1 1000204886016 disk WD_BLACK_SN770
---NICS---
[]
---NIC_SPEEDS---
eno1|1000|e1000e|up
enp2s0|10000|mlx5_core|up
---GPUS---
---PCIE---
---IOMMU---
---BRIDGES---
br0
---BONDS---
---KERNEL_PARAMS---
net.ipv4.ip_forward = 1
---SCALEX_FACTS_END---"#;

        let facts = crate::commands::facts::parse_facts_output_public("playbox-0", raw).unwrap();

        // Serialize to JSON (what `scalex facts` writes to _generated/facts/)
        let json = serde_json::to_string_pretty(&facts).unwrap();

        // Deserialize back (what `sdi init` reads from _generated/facts/)
        let restored: NodeFacts = serde_json::from_str(&json).unwrap();

        // Critical fields must survive the roundtrip
        assert_eq!(restored.node_name, "playbox-0");
        assert_eq!(restored.cpu.cores, 6);
        assert_eq!(restored.cpu.threads, 12);
        assert_eq!(restored.memory.total_mb, 32000); // 32768000 KB → ~32000 MB
        assert_eq!(restored.kernel.version, "6.8.0-45-generic");
        assert_eq!(restored.disks.len(), 1);
        assert_eq!(restored.nics.len(), 2);
        assert_eq!(restored.bridges, vec!["br0"]);

        // sdi init uses node_name to map to baremetal config hosts
        assert!(
            !restored.node_name.is_empty(),
            "node_name must survive roundtrip for sdi host mapping"
        );
    }

    /// Sprint 45: Cross-module chain test — SDI HCL VM IPs flow into inventory ansible_host.
    /// Verifies the critical data linkage: sdi-specs VM IPs → cluster inventory ansible_host.
    #[test]
    fn test_chain_sdi_vm_ips_flow_into_inventory_ansible_host() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s_config.config.clusters {
            // Find the matching SDI pool
            let pool = sdi_spec
                .spec
                .sdi_pools
                .iter()
                .find(|p| p.pool_name == cluster.cluster_sdi_resource_pool)
                .unwrap_or_else(|| {
                    panic!(
                        "cluster '{}' references pool '{}' which must exist in sdi-specs",
                        cluster.cluster_name, cluster.cluster_sdi_resource_pool
                    )
                });

            // Generate inventory from this cluster + sdi spec
            let inventory = crate::core::kubespray::generate_inventory(cluster, &sdi_spec).unwrap();

            // Every VM IP in the SDI pool must appear as ansible_host in inventory
            for node in &pool.node_specs {
                let expected_host = format!("ansible_host={}", node.ip);
                assert!(
                    inventory.contains(&expected_host),
                    "cluster '{}': SDI VM IP '{}' (node '{}') must appear as ansible_host in inventory.\n\
                     This verifies the critical SDI→Kubespray data chain.\n\
                     Inventory excerpt:\n{}",
                    cluster.cluster_name,
                    node.ip,
                    node.node_name,
                    inventory.lines().take(20).collect::<Vec<_>>().join("\n")
                );
            }

            // Also verify node_name appears in inventory
            for node in &pool.node_specs {
                assert!(
                    inventory.contains(&node.node_name),
                    "cluster '{}': SDI node_name '{}' must appear in inventory",
                    cluster.cluster_name,
                    node.node_name
                );
            }
        }
    }

    /// Sprint 45: Cross-module chain test — bootstrap args reference tower kubeconfig path.
    /// Verifies bootstrap generates helm/kubectl commands targeting the correct cluster.
    #[test]
    fn test_chain_bootstrap_args_reference_generated_kubeconfig_path() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        // Find the tower cluster (role = management)
        let tower = k8s_config
            .config
            .clusters
            .iter()
            .find(|c| c.cluster_role == "management")
            .expect("example config must have a management cluster");

        // The expected kubeconfig path pattern
        let expected_kubeconfig_pattern =
            format!("_generated/clusters/{}/kubeconfig", tower.cluster_name);

        // Bootstrap helm args must reference this kubeconfig (version is a CLI arg, default 7.8.13)
        let helm_args = crate::commands::bootstrap::generate_argocd_helm_install_args(
            &expected_kubeconfig_pattern,
            "7.8.13",
        );

        assert!(
            helm_args
                .iter()
                .any(|a| a.contains(&expected_kubeconfig_pattern)),
            "helm args must reference tower kubeconfig at '{}'. Args: {:?}",
            expected_kubeconfig_pattern,
            helm_args
        );
    }
}
