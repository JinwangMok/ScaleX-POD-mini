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
        std::fs::write(&inventory_path, &inventory)?;
        if dry_run {
            println!("[dry-run] inventory.ini:\n{}", inventory);
        } else {
            println!("[cluster] Generated {}", inventory_path.display());
        }

        // Generate cluster vars
        let vars = kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);
        let vars_path = cluster_dir.join("cluster-vars.yml");
        std::fs::write(&vars_path, &vars)?;
        if dry_run {
            println!("[dry-run] cluster-vars.yml:\n{}", vars);
        } else {
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

            // Verify control plane pods are Running (AC 8.1)
            println!(
                "[cluster] Verifying control plane pods for {}...",
                cluster.cluster_name
            );
            verify_control_plane_pods(&cluster_dir, &cluster.cluster_name)?;

            // Verify all worker nodes are Ready (AC 8.2)
            println!(
                "[cluster] Verifying worker nodes for {}...",
                cluster.cluster_name
            );
            verify_worker_nodes_ready(&cluster_dir, &cluster.cluster_name)?;

            // Collect kubeconfig
            println!(
                "[cluster] Collecting kubeconfig for {}...",
                cluster.cluster_name
            );
            collect_kubeconfig(&cluster_dir, &cluster.cluster_name, &sdi_spec, cluster)?;

            // Apply kubectl manifests (CRDs, namespaces, RBAC, workloads) — AC 9c
            println!(
                "[cluster] Applying gitops manifests for {}...",
                cluster.cluster_name
            );
            // Gitops apply is best-effort during cluster init — the full bootstrap
            // (ArgoCD + ApplicationSets) is handled by `scalex-pod bootstrap`. Failure here
            // must NOT block subsequent clusters from being provisioned.
            if let Err(e) = apply_gitops_manifests(
                &cluster_dir,
                &cluster.cluster_name,
                std::path::Path::new("gitops"),
            ) {
                eprintln!(
                    "[cluster] WARNING: gitops apply failed for {} ({}). Continuing — run `scalex-pod bootstrap` later.",
                    cluster.cluster_name, e
                );
            }
        } else {
            println!(
                "[dry-run] Files written to {}. Kubespray NOT executed for {}",
                cluster_dir.display(),
                cluster.cluster_name
            );
        }
    }

    println!("\n[cluster] All clusters initialized.");

    // Step 4: Update GitOps Cilium values with correct API server IP.
    // Prefer kube-vip VIP (HA) when kube_vip_enabled=true; fall back to first CP node IP.
    for cluster in &k8s_config.config.clusters {
        let api_ip = get_kube_vip_address(cluster)
            .or_else(|| find_control_plane_ip(cluster, sdi_spec.as_ref()));
        if let Some(ip) = api_ip {
            let cilium_cluster_name = cluster
                .cilium
                .as_ref()
                .map(|c| c.cluster_name.clone())
                .unwrap_or_else(|| cluster.cluster_name.clone());
            let cilium_cluster_id = cluster.cilium.as_ref().map(|c| c.cluster_id).unwrap_or(0);
            update_gitops_cilium_values(
                &cluster.cluster_name,
                &ip,
                &k8s_config.config.common.cilium_version,
                &cluster.network.dns_domain,
                &cilium_cluster_name,
                cilium_cluster_id,
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
        "SDI spec required. Provide --sdi-spec <path> or run `scalex-pod sdi init <spec>` first."
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
        if let Some(ip_part) = line
            .split_whitespace()
            .find(|s| s.starts_with("ansible_host="))
        {
            if let Some(ip) = ip_part.strip_prefix("ansible_host=") {
                ips.push(ip.to_string());
            }
        }
    }

    if ips.is_empty() {
        return Ok(());
    }

    // Parse ansible_user and ssh args from inventory for ProxyJump support
    let default_user = "ubuntu".to_string();
    let mut ssh_user = default_user.clone();
    let mut proxy_jump = String::new();
    for line in content.lines() {
        if let Some(u) = line
            .split_whitespace()
            .find(|s| s.starts_with("ansible_user="))
        {
            if let Some(user) = u.strip_prefix("ansible_user=") {
                ssh_user = user.to_string();
            }
        }
        if line.contains("ProxyJump=") {
            if let Some(pj) = line.split("ProxyJump=").nth(1) {
                proxy_jump = pj.trim_end_matches('\'').to_string();
            }
        }
    }

    println!(
        "[cluster] Waiting for cloud-init on {} ({} nodes)...",
        cluster_name,
        ips.len()
    );

    let max_attempts = 60; // 5 minutes (60 * 5s)
    for ip in &ips {
        for attempt in 1..=max_attempts {
            let mut ssh_args = vec![
                "-o".to_string(),
                "ConnectTimeout=5".to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=no".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
            ];
            if !proxy_jump.is_empty() {
                ssh_args.push("-o".to_string());
                ssh_args.push(format!("ProxyJump={}", proxy_jump));
            }
            ssh_args.push(format!("{}@{}", ssh_user, ip));
            ssh_args.push("cloud-init status --wait 2>/dev/null || test -f /var/lib/cloud/instance/boot-finished".to_string());

            let result = std::process::Command::new("ssh").args(&ssh_args).output();

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

/// Required control-plane component pod name prefixes to verify after kubeadm init.
/// Each must appear exactly once per control-plane node in kube-system in Running phase.
pub const REQUIRED_CP_COMPONENTS: &[&str] = &[
    "kube-apiserver-",
    "kube-controller-manager-",
    "kube-scheduler-",
];

/// Parse the `kubectl get pods -n kube-system` output and return which of the
/// required control-plane components are Running.
///
/// Pure function — takes the raw text output and returns a Vec of component
/// prefixes that were found in Running state.  Suitable for unit testing.
pub fn parse_control_plane_pod_status(kubectl_output: &str) -> Vec<String> {
    let mut running = Vec::new();
    for line in kubectl_output.lines() {
        // Lines look like:
        //   kube-apiserver-tower-cp-0   1/1   Running   17   43h
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 {
            continue;
        }
        let pod_name = cols[0];
        let status = cols[2];
        if status != "Running" {
            continue;
        }
        for &prefix in REQUIRED_CP_COMPONENTS {
            if pod_name.starts_with(prefix) && !running.contains(&prefix.to_string()) {
                running.push(prefix.to_string());
            }
        }
    }
    running
}

/// Verify that the three required control-plane pods (kube-apiserver,
/// kube-controller-manager, kube-scheduler) are in Running state on the
/// first control-plane node of a cluster.
///
/// Reads the inventory.ini generated by `generate_inventory()` to locate the
/// first control-plane node and its ProxyJump (if any), then SSHes to it and
/// runs `kubectl get pods -n kube-system`.  Retries for up to 10 minutes.
fn verify_control_plane_pods(
    cluster_dir: &std::path::Path,
    cluster_name: &str,
) -> anyhow::Result<()> {
    let inventory_path = cluster_dir.join("inventory.ini");
    if !inventory_path.exists() {
        anyhow::bail!(
            "[cluster] inventory.ini not found for {}: {}",
            cluster_name,
            inventory_path.display()
        );
    }

    let content = std::fs::read_to_string(&inventory_path)?;

    // Parse first control-plane node IP from [kube_control_plane] section
    let mut in_cp_section = false;
    let mut first_cp_name: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[kube_control_plane]" {
            in_cp_section = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_cp_section = false;
            continue;
        }
        if in_cp_section && !trimmed.is_empty() {
            first_cp_name = Some(trimmed.to_string());
            break;
        }
    }

    let cp_name = first_cp_name.ok_or_else(|| {
        anyhow::anyhow!(
            "No control-plane node found in [kube_control_plane] section for {}",
            cluster_name
        )
    })?;

    // Resolve the IP and SSH args for this node from the [all] section
    let mut cp_ip = String::new();
    let mut ssh_user = "ubuntu".to_string();
    let mut proxy_jump = String::new();
    for line in content.lines() {
        // Lines look like: tower-cp-0 ansible_host=192.168.88.100 ip=... ansible_user=ubuntu ansible_ssh_common_args='...'
        if !line.starts_with(&cp_name) {
            continue;
        }
        for token in line.split_whitespace() {
            if let Some(ip) = token.strip_prefix("ansible_host=") {
                cp_ip = ip.to_string();
            }
            if let Some(u) = token.strip_prefix("ansible_user=") {
                ssh_user = u.to_string();
            }
        }
        // ProxyJump sits inside the quoted ansible_ssh_common_args
        if line.contains("ProxyJump=") {
            if let Some(pj) = line.split("ProxyJump=").nth(1) {
                proxy_jump = pj
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('\'')
                    .to_string();
            }
        }
        break;
    }

    if cp_ip.is_empty() {
        anyhow::bail!(
            "Could not resolve IP for control-plane node '{}' in inventory for {}",
            cp_name,
            cluster_name
        );
    }

    let kubectl_cmd = "sudo kubectl --kubeconfig=/etc/kubernetes/admin.conf get pods -n kube-system 2>/dev/null";

    let max_attempts = 60; // 60 × 10s = 10 minutes
    for attempt in 1..=max_attempts {
        let mut ssh_args = vec![
            "-o".to_string(),
            "ConnectTimeout=10".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=no".to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
        ];
        if !proxy_jump.is_empty() {
            ssh_args.push("-o".to_string());
            ssh_args.push(format!("ProxyJump={}", proxy_jump));
        }
        ssh_args.push(format!("{}@{}", ssh_user, cp_ip));
        ssh_args.push(kubectl_cmd.to_string());

        match std::process::Command::new("ssh").args(&ssh_args).output() {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let running = parse_control_plane_pod_status(&stdout);

                let missing: Vec<&str> = REQUIRED_CP_COMPONENTS
                    .iter()
                    .filter(|&&req| !running.iter().any(|r| r == req))
                    .copied()
                    .collect();

                if missing.is_empty() {
                    println!(
                        "[cluster] ✓ Control plane pods Running for {}: apiserver, scheduler, controller-manager",
                        cluster_name
                    );
                    return Ok(());
                }

                if attempt % 6 == 0 || attempt == 1 {
                    eprintln!(
                        "[cluster] Waiting for control plane pods on {} ({}/{}): missing {:?}",
                        cluster_name, attempt, max_attempts, missing
                    );
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if attempt % 6 == 0 || attempt == 1 {
                    eprintln!(
                        "[cluster] kubectl not ready on {} ({}/{}): {}",
                        cluster_name, attempt, max_attempts, stderr.trim()
                    );
                }
            }
            Err(e) => {
                if attempt % 6 == 0 || attempt == 1 {
                    eprintln!(
                        "[cluster] SSH to {} failed ({}/{}): {}",
                        cp_ip, attempt, max_attempts, e
                    );
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(10));
    }

    anyhow::bail!(
        "Control plane pods did not reach Running state for {} within 10 minutes. \
         Check: ssh {}@{} 'sudo kubectl --kubeconfig=/etc/kubernetes/admin.conf get pods -n kube-system'",
        cluster_name,
        ssh_user,
        cp_ip
    )
}

// ---------------------------------------------------------------------------
// AC 8.2 — Worker node join verification
// ---------------------------------------------------------------------------

/// Parse `kubectl get nodes` output and return a list of (node_name, is_ready) pairs.
///
/// Each row in the output looks like:
///   tower-cp-0     Ready    control-plane   43h   v1.32.0
///   tower-worker-0 Ready    <none>          43h   v1.32.0
///   tower-worker-1 NotReady <none>          2m    v1.32.0
///
/// Pure function — no I/O, no side effects.  Returns an entry for every data
/// row (skips the header and blank lines).
pub fn parse_kubectl_get_nodes_output(kubectl_output: &str) -> Vec<(String, bool)> {
    let mut nodes = Vec::new();
    for line in kubectl_output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Header row starts with "NAME"; skip it and any short/empty lines
        if cols.len() < 2 || cols[0] == "NAME" {
            continue;
        }
        let node_name = cols[0].to_string();
        let is_ready = cols[1] == "Ready";
        nodes.push((node_name, is_ready));
    }
    nodes
}

/// Verify that all worker nodes listed in the inventory are present **and**
/// report `Ready` status in `kubectl get nodes` on the first control-plane node.
///
/// * Reads `inventory.ini` from `cluster_dir` to discover:
///   - The worker node names listed under `[kube_node]`
///   - The first control-plane node from `[kube_control_plane]` (SSH target)
///   - The SSH user and ProxyJump settings
/// * Retries for up to 10 minutes (60 × 10 s) — same policy as
///   `verify_control_plane_pods`.
fn verify_worker_nodes_ready(
    cluster_dir: &std::path::Path,
    cluster_name: &str,
) -> anyhow::Result<()> {
    let inventory_path = cluster_dir.join("inventory.ini");
    if !inventory_path.exists() {
        anyhow::bail!(
            "[cluster] inventory.ini not found for {}: {}",
            cluster_name,
            inventory_path.display()
        );
    }

    let content = std::fs::read_to_string(&inventory_path)?;

    // ── Parse worker node names from [kube_node] ──────────────────────────
    let mut worker_names: Vec<String> = Vec::new();
    let mut in_node_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[kube_node]" {
            in_node_section = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_node_section = false;
            continue;
        }
        if in_node_section && !trimmed.is_empty() {
            worker_names.push(trimmed.to_string());
        }
    }

    if worker_names.is_empty() {
        // No worker nodes in inventory — nothing to verify.
        println!(
            "[cluster] No worker nodes in [kube_node] for {} — skipping worker verification",
            cluster_name
        );
        return Ok(());
    }

    println!(
        "[cluster] Waiting for {} worker node(s) to be Ready in {}: {:?}",
        worker_names.len(),
        cluster_name,
        worker_names
    );

    // ── Parse first control-plane node IP and SSH settings ────────────────
    let mut in_cp_section = false;
    let mut first_cp_name: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[kube_control_plane]" {
            in_cp_section = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_cp_section = false;
            continue;
        }
        if in_cp_section && !trimmed.is_empty() {
            first_cp_name = Some(trimmed.to_string());
            break;
        }
    }

    let cp_name = first_cp_name.ok_or_else(|| {
        anyhow::anyhow!(
            "No control-plane node found in [kube_control_plane] for {}",
            cluster_name
        )
    })?;

    let mut cp_ip = String::new();
    let mut ssh_user = "ubuntu".to_string();
    let mut proxy_jump = String::new();
    for line in content.lines() {
        if !line.starts_with(&cp_name) {
            continue;
        }
        for token in line.split_whitespace() {
            if let Some(ip) = token.strip_prefix("ansible_host=") {
                cp_ip = ip.to_string();
            }
            if let Some(u) = token.strip_prefix("ansible_user=") {
                ssh_user = u.to_string();
            }
        }
        if line.contains("ProxyJump=") {
            if let Some(pj) = line.split("ProxyJump=").nth(1) {
                proxy_jump = pj
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('\'')
                    .to_string();
            }
        }
        break;
    }

    if cp_ip.is_empty() {
        anyhow::bail!(
            "Could not resolve IP for control-plane node '{}' in inventory for {}",
            cp_name,
            cluster_name
        );
    }

    let kubectl_cmd =
        "sudo kubectl --kubeconfig=/etc/kubernetes/admin.conf get nodes 2>/dev/null";

    let max_attempts = 60; // 60 × 10s = 10 minutes
    for attempt in 1..=max_attempts {
        let mut ssh_args = vec![
            "-o".to_string(),
            "ConnectTimeout=10".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=no".to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
        ];
        if !proxy_jump.is_empty() {
            ssh_args.push("-o".to_string());
            ssh_args.push(format!("ProxyJump={}", proxy_jump));
        }
        ssh_args.push(format!("{}@{}", ssh_user, cp_ip));
        ssh_args.push(kubectl_cmd.to_string());

        match std::process::Command::new("ssh").args(&ssh_args).output() {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let node_statuses = parse_kubectl_get_nodes_output(&stdout);

                // Check every expected worker is present and Ready
                let not_ready: Vec<&str> = worker_names
                    .iter()
                    .filter(|wn| {
                        !node_statuses
                            .iter()
                            .any(|(name, ready)| name == *wn && *ready)
                    })
                    .map(|s| s.as_str())
                    .collect();

                if not_ready.is_empty() {
                    println!(
                        "[cluster] ✓ All {} worker node(s) Ready in {}: {:?}",
                        worker_names.len(),
                        cluster_name,
                        worker_names
                    );
                    return Ok(());
                }

                if attempt % 6 == 0 || attempt == 1 {
                    eprintln!(
                        "[cluster] Worker nodes not yet Ready in {} ({}/{}): {:?}",
                        cluster_name, attempt, max_attempts, not_ready
                    );
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if attempt % 6 == 0 || attempt == 1 {
                    eprintln!(
                        "[cluster] kubectl get nodes not ready on {} ({}/{}): {}",
                        cluster_name,
                        attempt,
                        max_attempts,
                        stderr.trim()
                    );
                }
            }
            Err(e) => {
                if attempt % 6 == 0 || attempt == 1 {
                    eprintln!(
                        "[cluster] SSH to {} failed ({}/{}): {}",
                        cp_ip, attempt, max_attempts, e
                    );
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(10));
    }

    anyhow::bail!(
        "Worker nodes did not become Ready in {} within 10 minutes. \
         Expected: {:?}. Check: ssh {}@{} 'sudo kubectl --kubeconfig=/etc/kubernetes/admin.conf get nodes'",
        cluster_name,
        worker_names,
        ssh_user,
        cp_ip
    )
}

fn run_kubespray(cluster_dir: &std::path::Path, cluster_name: &str) -> anyhow::Result<()> {
    // Use absolute paths since current_dir changes to kubespray dir
    let inventory_path = std::fs::canonicalize(cluster_dir.join("inventory.ini"))
        .unwrap_or_else(|_| cluster_dir.join("inventory.ini"));
    let vars_path = std::fs::canonicalize(cluster_dir.join("cluster-vars.yml"))
        .unwrap_or_else(|_| cluster_dir.join("cluster-vars.yml"));

    let kubespray_dir = find_kubespray_dir()?;

    // Limit forks to 3 to reduce SSH load through bastion (playbox-0).
    // All Kubespray connections ProxyJump through playbox-0; default forks=5
    // can OOM the bastion when it also hosts VMs.
    let output = std::process::Command::new("ansible-playbook")
        .args([
            "-i",
            &inventory_path.display().to_string(),
            "cluster.yml",
            "-e",
            &format!("@{}", vars_path.display()),
            "--become",
            "-f",
            "3",
        ])
        .current_dir(&kubespray_dir)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stdout.is_empty() {
                println!("{}", stdout);
            }
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                // Check if this is a Cilium CNI permission issue (common with Kubespray's
                // containernetworking-plugins setting /opt/cni/bin to 755 kube:root).
                // Fix permissions and let Cilium self-heal via CrashLoopBackOff retry.
                if stdout.contains("cni plugin not initialized")
                    || stdout.contains("NetworkPluginNotReady")
                {
                    eprintln!(
                        "[cluster] {} — Cilium CNI not ready (likely /opt/cni/bin permission issue). Fixing...",
                        cluster_name
                    );
                    // Fix /opt/cni/bin permissions via ansible ad-hoc
                    let _ = std::process::Command::new("ansible")
                        .args([
                            "-i",
                            &inventory_path.display().to_string(),
                            "all",
                            "--become",
                            "-m",
                            "file",
                            "-a",
                            "path=/opt/cni/bin state=directory mode=0777",
                        ])
                        .current_dir(&kubespray_dir)
                        .output();
                    eprintln!("[cluster] {} — /opt/cni/bin permissions fixed, waiting for Cilium recovery (up to 120s)...", cluster_name);

                    // Wait up to 120s for nodes to become Ready (Cilium self-heals on next retry).
                    // Use ansible to run kubectl on the first control-plane node (since the local
                    // machine may not have direct access to the K8s API behind NAT).
                    let mut recovered = false;
                    let first_cp = format!("{}[0]", cluster_name);  // e.g., "tower[0]" selects first host in group
                    for attempt in 0..24 {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        if let Ok(check) = std::process::Command::new("ansible")
                            .args([
                                "-i",
                                &inventory_path.display().to_string(),
                                &first_cp,
                                "--become",
                                "-m",
                                "command",
                                "-a",
                                "kubectl --kubeconfig=/etc/kubernetes/admin.conf get nodes -o jsonpath={.items[*].status.conditions[?(@.type==\"Ready\")].status}",
                            ])
                            .current_dir(&kubespray_dir)
                            .output()
                        {
                            let out = String::from_utf8_lossy(&check.stdout);
                            // Ansible output contains the command result after ">>"; extract the jsonpath line
                            let statuses = out.lines()
                                .find(|l| l.contains("True") || l.contains("False"))
                                .unwrap_or("");
                            if !statuses.is_empty() && statuses.split_whitespace().all(|s| s == "True") {
                                eprintln!("[cluster] {} — all nodes Ready after {}s", cluster_name, (attempt + 1) * 5);
                                recovered = true;
                                break;
                            }
                        }
                    }
                    if recovered {
                        return Ok(());
                    }
                }
                eprintln!("[cluster] kubespray error for {}: {}", cluster_name, stderr);
                anyhow::bail!("kubespray failed for cluster '{}'", cluster_name);
            }

            // Kubespray succeeded. Proactively fix /opt/cni/bin ownership before
            // worker-ready checks. Kubespray's containernetworking-plugins role sets
            // the directory to kube:root, which blocks Cilium's mount-cgroup init
            // container from writing cilium-mount. This is a known issue with
            // Kubespray + Cilium and must be fixed before nodes can become Ready.
            println!(
                "[cluster] {} — fixing /opt/cni/bin ownership (kube:root → root:root) on all nodes...",
                cluster_name
            );
            let _ = std::process::Command::new("ansible")
                .args([
                    "-i",
                    &inventory_path.display().to_string(),
                    "all",
                    "--become",
                    "-m",
                    "file",
                    "-a",
                    "path=/opt/cni/bin state=directory owner=root group=root mode=0755",
                    "-f",
                    "3",
                ])
                .current_dir(&kubespray_dir)
                .output();
            // Also restart Cilium pods so they pick up the fixed permissions immediately
            // instead of waiting for CrashLoopBackOff retry (which can take minutes).
            let first_cp = format!("{}[0]", cluster_name);
            let _ = std::process::Command::new("ansible")
                .args([
                    "-i",
                    &inventory_path.display().to_string(),
                    &first_cp,
                    "--become",
                    "-m",
                    "command",
                    "-a",
                    "kubectl --kubeconfig=/etc/kubernetes/admin.conf delete pod -n kube-system -l k8s-app=cilium",
                ])
                .current_dir(&kubespray_dir)
                .output();
            println!(
                "[cluster] {} — Cilium pods restarted with fixed permissions",
                cluster_name
            );

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
    dns_domain: &str,
    cilium_cluster_name: &str,
    cilium_cluster_id: u32,
    dry_run: bool,
) -> anyhow::Result<()> {
    let gitops_dir = std::path::Path::new("gitops");
    let values_path = gitops_dir.join(gitops::cilium_values_path(cluster_name));
    let kust_path = gitops_dir.join(gitops::cilium_kustomization_path(cluster_name));

    let values_content = gitops::generate_cilium_values(
        control_plane_ip,
        6443,
        dns_domain,
        cilium_cluster_name,
        cilium_cluster_id,
    );
    let kust_content = gitops::generate_cilium_kustomization(cilium_version);

    if let Some(parent) = values_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&values_path, &values_content)?;
    std::fs::write(&kust_path, &kust_content)?;

    if dry_run {
        println!(
            "[dry-run] Wrote Cilium values for {} (k8sServiceHost: {})",
            cluster_name, control_plane_ip
        );
    } else {
        println!(
            "[cluster] Updated gitops/{}/cilium/ with k8sServiceHost={}",
            cluster_name, control_plane_ip
        );
    }
    Ok(())
}
// ---------------------------------------------------------------------------
// AC 8.3 — CNI application helpers
// ---------------------------------------------------------------------------

/// Generate `kubectl wait` arguments that block until all nodes report Ready.
///
/// After Kubespray installs the CNI plugin (Cilium), nodes transition from
/// `NotReady` to `Ready` once the CNI daemonset initialises the Pod network.
/// This command is used to confirm that transition has completed.
///
/// Pure function — no I/O, no side effects.
pub fn generate_kubectl_wait_nodes_ready_args(kubeconfig: &str, timeout: &str) -> Vec<String> {
    vec![
        "wait".to_string(),
        "--for=condition=Ready".to_string(),
        "nodes".to_string(),
        "--all".to_string(),
        "--timeout".to_string(),
        timeout.to_string(),
        "--kubeconfig".to_string(),
        kubeconfig.to_string(),
    ]
}

/// Count how many nodes in a parsed status list are in Ready state.
///
/// Operates on the output of `parse_kubectl_get_nodes_output`.
/// Pure function — no I/O, no side effects.
pub fn count_ready_nodes(node_statuses: &[(String, bool)]) -> usize {
    node_statuses.iter().filter(|(_, ready)| *ready).count()
}

/// Return the names of the expected nodes that are **not yet Ready**.
///
/// An empty return value means every named node is Ready — the CNI plugin
/// has successfully brought all nodes into service.
///
/// Pure function — no I/O, no side effects.
pub fn nodes_not_yet_ready(expected: &[&str], statuses: &[(String, bool)]) -> Vec<String> {
    expected
        .iter()
        .filter(|&&name| {
            !statuses
                .iter()
                .any(|(n, ready)| n == name && *ready)
        })
        .map(|s| s.to_string())
        .collect()
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

/// Extract the kube-vip VIP address from a cluster's kubespray_extra_vars.
/// Returns Some(vip_address) when kube_vip_enabled=true and kube_vip_address is set.
/// Returns None otherwise (e.g., single-node or non-HA clusters without kube-vip).
/// Pure function.
pub fn get_kube_vip_address(cluster: &crate::models::cluster::ClusterDef) -> Option<String> {
    let extra_vars = cluster.kubespray_extra_vars.as_ref()?;
    // Only return the VIP when kube-vip is explicitly enabled
    let enabled = extra_vars
        .get("kube_vip_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    extra_vars
        .get("kube_vip_address")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
            // Fallback 2: SSH + cat (works through ProxyJump where SCP may fail)
            println!(
                "[cluster] SCP failed, trying SSH cat fallback for {}...",
                cluster_name
            );
            let ssh_cat = std::process::Command::new("ssh")
                .args([
                    "-o",
                    "StrictHostKeyChecking=no",
                    "-o",
                    "ProxyJump=playbox-0",
                    &format!("{}@{}", ssh_user, ip),
                    "sudo cat /etc/kubernetes/admin.conf",
                ])
                .output();
            if let Ok(out) = ssh_cat {
                if out.status.success() && !out.stdout.is_empty() {
                    std::fs::write(&local_path, &out.stdout)?;
                    println!(
                        "[cluster] kubeconfig for {} -> {} (server: {}, from SSH cat)",
                        cluster_name,
                        local_path.display(),
                        ip
                    );
                    return Ok(());
                }
            }

            // Fallback 3: kubespray artifacts (kubeconfig_localhost: true)
            let artifacts_path = cluster_dir.join("artifacts").join("admin.conf");
            if artifacts_path.exists() {
                println!(
                    "[cluster] SCP failed, using kubespray artifacts for {}",
                    cluster_name
                );
                if let Ok(content) = std::fs::read_to_string(&artifacts_path) {
                    let fixed = content.replace("https://127.0.0.1:", &format!("https://{}:", ip));
                    let _ = std::fs::write(&local_path, &fixed);
                    println!(
                        "[cluster] kubeconfig for {} -> {} (server: {}, from artifacts)",
                        cluster_name,
                        local_path.display(),
                        ip
                    );
                }
            } else {
                eprintln!(
                    "[cluster] Failed to collect kubeconfig from {} for {}",
                    ip, cluster_name
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// AC 9c — kubectl manifest application and resource readiness verification
// ---------------------------------------------------------------------------

/// Manifest category determines the application order:
/// Config (cluster metadata, namespaces) → RBAC → Workloads.
///
/// `Namespace` is reserved for manifests that create only Namespace objects and
/// have no dependency on any other resource.  Currently the cluster-config step
/// covers namespace creation as part of the Config category.
#[derive(Debug, Clone, PartialEq)]
pub enum ManifestCategory {
    /// Pure namespace-creation manifests (no Deployments or RBAC objects).
    #[allow(dead_code)]
    Namespace,
    /// RBAC manifests: ClusterRole, ClusterRoleBinding, ServiceAccount.
    Rbac,
    /// Cluster metadata and configuration ConfigMaps.
    Config,
    /// Deployments, DaemonSets, StatefulSets and supporting ConfigMaps.
    Workload,
}

/// A single manifest application step: a category label and a path relative to the gitops root.
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestApplyStep {
    pub category: ManifestCategory,
    /// Path relative to the gitops root directory.
    pub relative_path: String,
    /// Human-readable label used in log output.
    pub description: String,
}

/// Return the ordered list of manifest paths to apply for a given cluster.
///
/// Application order is strictly:
///   1. Namespaces / cluster-config  (must exist before namespaced objects)
///   2. RBAC                         (service-accounts need the namespace)
///   3. Config / system workloads    (provisioner, test resources)
///
/// Pure function — no I/O.
pub fn manifest_apply_order(cluster_name: &str) -> Vec<ManifestApplyStep> {
    let mut steps = vec![
        // cluster-info ConfigMap in kube-system (required by scalex-dash)
        ManifestApplyStep {
            category: ManifestCategory::Config,
            relative_path: format!("{cluster_name}/cluster-config/manifest.yaml"),
            description: format!("{cluster_name} cluster-config"),
        },
        // scalex-dash RBAC (namespace + SA + ClusterRole/Binding + token Secret)
        ManifestApplyStep {
            category: ManifestCategory::Rbac,
            relative_path: "common/scalex-dash-rbac/manifest.yaml".to_string(),
            description: "scalex-dash RBAC".to_string(),
        },
    ];

    // Cluster-specific steps
    match cluster_name {
        "sandbox" => {
            steps.push(ManifestApplyStep {
                category: ManifestCategory::Rbac,
                relative_path: "sandbox/rbac/manifest.yaml".to_string(),
                description: "sandbox OIDC RBAC".to_string(),
            });
            steps.push(ManifestApplyStep {
                category: ManifestCategory::Workload,
                relative_path: "sandbox/local-path-provisioner/manifest.yaml".to_string(),
                description: "sandbox local-path-provisioner".to_string(),
            });
            steps.push(ManifestApplyStep {
                category: ManifestCategory::Workload,
                relative_path: "sandbox/test-resources/manifest.yaml".to_string(),
                description: "sandbox test-resources".to_string(),
            });
        }
        "tower" => {
            steps.push(ManifestApplyStep {
                category: ManifestCategory::Workload,
                relative_path: "tower/local-path-provisioner/manifest.yaml".to_string(),
                description: "tower local-path-provisioner".to_string(),
            });
        }
        _ => {
            // Generic: apply local-path-provisioner if it exists
            steps.push(ManifestApplyStep {
                category: ManifestCategory::Workload,
                relative_path: format!("{cluster_name}/local-path-provisioner/manifest.yaml"),
                description: format!("{cluster_name} local-path-provisioner"),
            });
        }
    }

    steps
}

/// Generate `kubectl apply -f` arguments for a manifest file.
///
/// Pure function — no I/O.
pub fn generate_manifest_apply_args(kubeconfig: &str, manifest_path: &str) -> Vec<String> {
    vec![
        "apply".to_string(),
        "--kubeconfig".to_string(),
        kubeconfig.to_string(),
        "-f".to_string(),
        manifest_path.to_string(),
    ]
}

/// Represents the readiness state of a single pod as parsed from `kubectl get pods -A`.
#[derive(Debug, Clone, PartialEq)]
pub struct PodStatus {
    pub namespace: String,
    pub name: String,
    /// Raw `READY` column, e.g. "1/1" or "0/1".
    pub ready: String,
    /// Raw `STATUS` column, e.g. "Running", "Pending", "CrashLoopBackOff".
    pub status: String,
}

/// Parse the tabular output of `kubectl get pods -A` into `PodStatus` records.
///
/// Expected header line: `NAMESPACE   NAME   READY   STATUS   RESTARTS   AGE`
/// Lines that do not match the expected column count are silently skipped.
///
/// Pure function — no I/O.
pub fn parse_kubectl_get_pods_output(output: &str) -> Vec<PodStatus> {
    let mut pods = Vec::new();
    for line in output.lines() {
        // Skip the header row
        if line.trim_start().starts_with("NAMESPACE") {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Minimum: NAMESPACE NAME READY STATUS (RESTARTS AGE optional for robustness)
        if cols.len() < 4 {
            continue;
        }
        pods.push(PodStatus {
            namespace: cols[0].to_string(),
            name: cols[1].to_string(),
            ready: cols[2].to_string(),
            status: cols[3].to_string(),
        });
    }
    pods
}

/// Terminal healthy statuses for a pod.  Anything else is considered unhealthy for
/// the purposes of the post-apply readiness gate.
///
/// Pure function — no I/O.
pub fn is_pod_healthy(status: &str) -> bool {
    matches!(
        status,
        "Running" | "Completed" | "Succeeded"
    )
}

/// Return the subset of pods that are **not** healthy (not Running/Completed/Succeeded).
///
/// Pure function — no I/O.
pub fn find_unhealthy_pods(pods: &[PodStatus]) -> Vec<PodStatus> {
    pods.iter()
        .filter(|p| !is_pod_healthy(&p.status))
        .cloned()
        .collect()
}

/// Return `true` if every pod in `pods` is healthy.
///
/// Pure function — no I/O.
pub fn all_pods_healthy(pods: &[PodStatus]) -> bool {
    pods.iter().all(|p| is_pod_healthy(&p.status))
}

/// Generate `kubectl wait --for=condition=Available` arguments for a Deployment.
///
/// Used to block until a workload Deployment has at least one available replica.
/// Pure function — no I/O.
pub fn generate_wait_deployment_args(
    kubeconfig: &str,
    namespace: &str,
    name: &str,
    timeout: &str,
) -> Vec<String> {
    vec![
        "wait".to_string(),
        "deployment".to_string(),
        name.to_string(),
        "--for=condition=Available".to_string(),
        format!("--namespace={namespace}"),
        format!("--timeout={timeout}"),
        "--kubeconfig".to_string(),
        kubeconfig.to_string(),
    ]
}

/// Apply all gitops manifests for a cluster in the correct order, then verify that
/// every pod in the affected namespaces reaches Running/Completed state.
///
/// I/O function: reads manifest files, runs kubectl, polls for readiness.
fn apply_gitops_manifests(
    cluster_dir: &std::path::Path,
    cluster_name: &str,
    gitops_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let kubeconfig = cluster_dir.join("kubeconfig.yaml");
    if !kubeconfig.exists() {
        eprintln!(
            "[cluster] No kubeconfig found at {} — skipping manifest apply for {}",
            kubeconfig.display(),
            cluster_name
        );
        return Ok(());
    }
    let kubeconfig_str = kubeconfig.display().to_string();

    let steps = manifest_apply_order(cluster_name);
    for step in &steps {
        let manifest_path = gitops_dir.join(&step.relative_path);
        if !manifest_path.exists() {
            println!(
                "[cluster]   skip {} (not found: {})",
                step.description,
                manifest_path.display()
            );
            continue;
        }

        println!("[cluster]   apply {} ...", step.description);
        let args = generate_manifest_apply_args(&kubeconfig_str, &manifest_path.display().to_string());
        let result = std::process::Command::new("kubectl").args(&args).output();
        match result {
            Ok(out) if out.status.success() => {
                println!("[cluster]   ✓ {}", step.description);
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                anyhow::bail!(
                    "kubectl apply failed for {} in cluster '{}': {}",
                    step.description,
                    cluster_name,
                    stderr.trim()
                );
            }
            Err(e) => {
                anyhow::bail!(
                    "Failed to run kubectl apply for {} in cluster '{}': {}",
                    step.description,
                    cluster_name,
                    e
                );
            }
        }
    }

    // Verify no unhealthy pods after applying all manifests
    println!(
        "[cluster] Verifying all pods healthy in {} after manifest apply...",
        cluster_name
    );
    verify_pods_healthy_after_apply(&kubeconfig_str, cluster_name)?;

    println!(
        "[cluster] ✓ All manifests applied and pods healthy in {}",
        cluster_name
    );
    Ok(())
}

/// Poll `kubectl get pods -A` until all pods are healthy or timeout expires.
/// Retries every 10 seconds for up to 5 minutes.
fn verify_pods_healthy_after_apply(kubeconfig: &str, cluster_name: &str) -> anyhow::Result<()> {
    let max_attempts = 30; // 30 × 10s = 5 minutes
    let interval = std::time::Duration::from_secs(10);

    for attempt in 1..=max_attempts {
        let result = std::process::Command::new("kubectl")
            .args([
                "get",
                "pods",
                "-A",
                "--kubeconfig",
                kubeconfig,
                "--no-headers",
            ])
            .output();

        match result {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let pods = parse_kubectl_get_pods_output(&stdout);
                let unhealthy = find_unhealthy_pods(&pods);

                if unhealthy.is_empty() {
                    return Ok(());
                }

                if attempt == max_attempts {
                    let names: Vec<String> = unhealthy
                        .iter()
                        .map(|p| format!("{}/{} ({})", p.namespace, p.name, p.status))
                        .collect();
                    anyhow::bail!(
                        "Pods still unhealthy in '{}' after {}s: {}",
                        cluster_name,
                        max_attempts * 10,
                        names.join(", ")
                    );
                }

                println!(
                    "[cluster] {} pod(s) not yet healthy in {} ({}/{}): {:?}",
                    unhealthy.len(),
                    cluster_name,
                    attempt,
                    max_attempts,
                    unhealthy
                        .iter()
                        .map(|p| p.status.as_str())
                        .collect::<Vec<_>>()
                );
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!(
                    "[cluster] kubectl get pods failed ({}/{}): {}",
                    attempt, max_attempts, stderr.trim()
                );
            }
            Err(e) => {
                eprintln!(
                    "[cluster] SSH/kubectl error ({}/{}): {}",
                    attempt, max_attempts, e
                );
            }
        }

        std::thread::sleep(interval);
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
            api_endpoint: None,
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
            api_endpoint: None,
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

    // --- get_kube_vip_address ---

    #[test]
    fn test_get_kube_vip_address_returns_vip_when_enabled() {
        let mut cluster = make_sdi_cluster("tower", "tower");
        cluster.kubespray_extra_vars = Some(serde_yaml::from_str(
            "kube_vip_enabled: true\nkube_vip_address: \"192.168.88.99\"",
        ).unwrap());
        let vip = get_kube_vip_address(&cluster);
        assert_eq!(vip, Some("192.168.88.99".to_string()), "must return VIP when kube-vip enabled");
    }

    #[test]
    fn test_get_kube_vip_address_returns_none_when_disabled() {
        let mut cluster = make_sdi_cluster("tower", "tower");
        cluster.kubespray_extra_vars = Some(serde_yaml::from_str(
            "kube_vip_enabled: false\nkube_vip_address: \"192.168.88.99\"",
        ).unwrap());
        let vip = get_kube_vip_address(&cluster);
        assert_eq!(vip, None, "must return None when kube-vip disabled");
    }

    #[test]
    fn test_get_kube_vip_address_returns_none_without_extra_vars() {
        let cluster = make_sdi_cluster("tower", "tower");
        // make_sdi_cluster does not set kubespray_extra_vars
        let vip = get_kube_vip_address(&cluster);
        assert_eq!(vip, None, "must return None when no extra_vars");
    }

    #[test]
    fn test_get_kube_vip_address_returns_none_without_address_field() {
        let mut cluster = make_sdi_cluster("tower", "tower");
        cluster.kubespray_extra_vars = Some(serde_yaml::from_str(
            "kube_vip_enabled: true",
        ).unwrap());
        let vip = get_kube_vip_address(&cluster);
        assert_eq!(vip, None, "must return None when enabled but kube_vip_address missing");
    }

    #[test]
    fn test_get_kube_vip_address_actual_config() {
        // Verify with actual k8s-clusters.yaml (has kube_vip_enabled: true for both clusters)
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let tower = k8s_config.config.clusters.iter().find(|c| c.cluster_name == "tower").unwrap();
        let tower_vip = get_kube_vip_address(tower);
        assert_eq!(tower_vip, Some("192.168.88.99".to_string()),
            "tower cluster must use kube-vip VIP 192.168.88.99");

        let sandbox = k8s_config.config.clusters.iter().find(|c| c.cluster_name == "sandbox").unwrap();
        let sandbox_vip = get_kube_vip_address(sandbox);
        assert_eq!(sandbox_vip, Some("192.168.88.109".to_string()),
            "sandbox cluster must use kube-vip VIP 192.168.88.109");
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
            let dns_domain = &cluster.network.dns_domain;
            let cilium_name = cluster
                .cilium
                .as_ref()
                .map(|c| c.cluster_name.as_str())
                .unwrap_or(cluster.cluster_name.as_str());
            let cilium_id = cluster.cilium.as_ref().map(|c| c.cluster_id).unwrap_or(0);
            let values = crate::core::gitops::generate_cilium_values(
                &ip,
                6443,
                dns_domain,
                cilium_name,
                cilium_id,
            );

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
    /// for every cluster. This simulates what `scalex-pod cluster init` does minus I/O.
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

        // Serialize to JSON (what `scalex-pod facts` writes to _generated/facts/)
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
            "gitops/tower/argocd/values.yaml",
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

    // ---------------------------------------------------------------------------
    // AC 8.1 — verify_control_plane_pods / parse_control_plane_pod_status
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_cp_pod_status_all_running() {
        let output = "\
NAME                                 READY   STATUS    RESTARTS   AGE
kube-apiserver-tower-cp-0            1/1     Running   17         43h
kube-controller-manager-tower-cp-0   1/1     Running   4          43h
kube-scheduler-tower-cp-0            1/1     Running   4          43h
etcd-tower-cp-0                      1/1     Running   2          43h
cilium-425lm                         1/1     Running   0          43h";

        let running = parse_control_plane_pod_status(output);

        // All three required components must be detected
        assert!(
            running.contains(&"kube-apiserver-".to_string()),
            "kube-apiserver must be Running"
        );
        assert!(
            running.contains(&"kube-controller-manager-".to_string()),
            "kube-controller-manager must be Running"
        );
        assert!(
            running.contains(&"kube-scheduler-".to_string()),
            "kube-scheduler must be Running"
        );
    }

    #[test]
    fn test_parse_cp_pod_status_missing_scheduler() {
        let output = "\
NAME                                 READY   STATUS    RESTARTS   AGE
kube-apiserver-tower-cp-0            1/1     Running   0          1h
kube-controller-manager-tower-cp-0   1/1     Running   0          1h
kube-scheduler-tower-cp-0            0/1     Pending   0          1h";

        let running = parse_control_plane_pod_status(output);

        assert!(
            running.contains(&"kube-apiserver-".to_string()),
            "apiserver must be detected as Running"
        );
        assert!(
            running.contains(&"kube-controller-manager-".to_string()),
            "controller-manager must be detected as Running"
        );
        assert!(
            !running.contains(&"kube-scheduler-".to_string()),
            "scheduler is Pending — must NOT be in running list"
        );
    }

    #[test]
    fn test_parse_cp_pod_status_empty_output() {
        let running = parse_control_plane_pod_status("");
        assert!(
            running.is_empty(),
            "empty output must produce empty running list"
        );
    }

    #[test]
    fn test_parse_cp_pod_status_only_header() {
        let output = "NAME   READY   STATUS   RESTARTS   AGE\n";
        let running = parse_control_plane_pod_status(output);
        assert!(
            running.is_empty(),
            "header-only output must produce empty running list"
        );
    }

    #[test]
    fn test_parse_cp_pod_status_non_cp_pods_ignored() {
        let output = "\
cilium-425lm   1/1   Running   0   43h
coredns-abc    1/1   Running   0   43h
etcd-cp-0      1/1   Running   0   43h";

        let running = parse_control_plane_pod_status(output);
        // etcd, cilium, coredns are not in REQUIRED_CP_COMPONENTS
        assert!(
            running.is_empty(),
            "non-CP pods must not appear in running list; got: {:?}",
            running
        );
    }

    #[test]
    fn test_parse_cp_pod_status_sandbox_cluster() {
        // Sandbox-style pod names include "sandbox-cp-0" suffix
        let output = "\
NAME                                     READY   STATUS    RESTARTS   AGE
kube-apiserver-sandbox-cp-0              1/1     Running   2          43h
kube-controller-manager-sandbox-cp-0     1/1     Running   3          43h
kube-scheduler-sandbox-cp-0             1/1     Running   3          43h";

        let running = parse_control_plane_pod_status(output);
        assert_eq!(
            running.len(),
            3,
            "All 3 sandbox control plane components must be detected as Running"
        );
    }

    #[test]
    fn test_parse_cp_pod_status_deduplicates_multi_node() {
        // HA cluster: 3 apiservers, 3 schedulers, 3 controller-managers
        let output = "\
kube-apiserver-tower-cp-0            1/1   Running   0   43h
kube-apiserver-tower-cp-1            1/1   Running   0   43h
kube-apiserver-tower-cp-2            1/1   Running   0   43h
kube-scheduler-tower-cp-0            1/1   Running   0   43h
kube-scheduler-tower-cp-1            1/1   Running   0   43h
kube-scheduler-tower-cp-2            1/1   Running   0   43h
kube-controller-manager-tower-cp-0   1/1   Running   0   43h
kube-controller-manager-tower-cp-1   1/1   Running   0   43h
kube-controller-manager-tower-cp-2   1/1   Running   0   43h";

        let running = parse_control_plane_pod_status(output);
        // Each component is deduplicated — only 3 unique entries, not 9
        assert_eq!(
            running.len(),
            3,
            "Must deduplicate HA multi-node pods: expected 3 unique components, got {} — {:?}",
            running.len(),
            running
        );
    }

    #[test]
    fn test_required_cp_components_contains_all_three() {
        let components: Vec<&str> = REQUIRED_CP_COMPONENTS.to_vec();
        assert!(
            components.contains(&"kube-apiserver-"),
            "REQUIRED_CP_COMPONENTS must include kube-apiserver-"
        );
        assert!(
            components.contains(&"kube-controller-manager-"),
            "REQUIRED_CP_COMPONENTS must include kube-controller-manager-"
        );
        assert!(
            components.contains(&"kube-scheduler-"),
            "REQUIRED_CP_COMPONENTS must include kube-scheduler-"
        );
    }

    #[test]
    fn test_parse_cp_pod_status_terminating_not_running() {
        // Terminating pods during rolling-restart must NOT be counted as Running
        let output = "\
kube-apiserver-tower-cp-0   1/1   Terminating   0   43h
kube-apiserver-tower-cp-1   1/1   Running       0   43h
kube-controller-manager-tower-cp-0   1/1   Running   0   43h
kube-scheduler-tower-cp-0   1/1   Running   0   43h";

        let running = parse_control_plane_pod_status(output);
        // apiserver detected because cp-1 is Running
        assert!(
            running.contains(&"kube-apiserver-".to_string()),
            "apiserver on cp-1 is Running, so it must be detected"
        );
        // All three should be present via cp-1/cp-0 entries
        assert_eq!(
            running.len(),
            3,
            "All 3 components must appear (some from cp-0 Running, apiserver from cp-1): {:?}",
            running
        );
    }

    // ---------------------------------------------------------------------------
    // AC 8.2 — parse_kubectl_get_nodes_output
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_get_nodes_all_ready() {
        let output = "\
NAME             STATUS   ROLES           AGE   VERSION
tower-cp-0       Ready    control-plane   43h   v1.32.0
tower-worker-0   Ready    <none>          43h   v1.32.0
tower-worker-1   Ready    <none>          43h   v1.32.0";

        let nodes = parse_kubectl_get_nodes_output(output);
        assert_eq!(nodes.len(), 3, "Must parse 3 nodes");
        assert!(
            nodes.iter().all(|(_, ready)| *ready),
            "All nodes must be Ready — got: {:?}",
            nodes
        );
        let names: Vec<&str> = nodes.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"tower-cp-0"));
        assert!(names.contains(&"tower-worker-0"));
        assert!(names.contains(&"tower-worker-1"));
    }

    #[test]
    fn test_parse_get_nodes_worker_not_ready() {
        let output = "\
NAME             STATUS    ROLES           AGE   VERSION
tower-cp-0       Ready     control-plane   43h   v1.32.0
tower-worker-0   Ready     <none>          43h   v1.32.0
tower-worker-1   NotReady  <none>          2m    v1.32.0";

        let nodes = parse_kubectl_get_nodes_output(output);
        assert_eq!(nodes.len(), 3, "Must parse 3 nodes including NotReady");

        let worker1 = nodes
            .iter()
            .find(|(n, _)| n == "tower-worker-1")
            .expect("tower-worker-1 must be present");
        assert!(
            !worker1.1,
            "tower-worker-1 is NotReady — must be false, got: {:?}",
            worker1
        );

        let worker0 = nodes
            .iter()
            .find(|(n, _)| n == "tower-worker-0")
            .expect("tower-worker-0 must be present");
        assert!(worker0.1, "tower-worker-0 is Ready — must be true");
    }

    #[test]
    fn test_parse_get_nodes_empty_output() {
        let nodes = parse_kubectl_get_nodes_output("");
        assert!(
            nodes.is_empty(),
            "empty output must return empty list, got: {:?}",
            nodes
        );
    }

    #[test]
    fn test_parse_get_nodes_header_only() {
        let output = "NAME   STATUS   ROLES   AGE   VERSION\n";
        let nodes = parse_kubectl_get_nodes_output(output);
        assert!(
            nodes.is_empty(),
            "header-only output must return empty list, got: {:?}",
            nodes
        );
    }

    #[test]
    fn test_parse_get_nodes_sandbox_topology() {
        // sandbox: 1 CP + 3 workers
        let output = "\
NAME               STATUS   ROLES           AGE   VERSION
sandbox-cp-0       Ready    control-plane   10h   v1.32.0
sandbox-worker-0   Ready    <none>          10h   v1.32.0
sandbox-worker-1   Ready    <none>          10h   v1.32.0
sandbox-worker-2   Ready    <none>          10h   v1.32.0";

        let nodes = parse_kubectl_get_nodes_output(output);
        assert_eq!(nodes.len(), 4, "sandbox has 1 CP + 3 workers = 4 nodes");

        let workers: Vec<&(String, bool)> = nodes
            .iter()
            .filter(|(n, _)| n.starts_with("sandbox-worker-"))
            .collect();
        assert_eq!(workers.len(), 3, "Must detect 3 sandbox workers");
        assert!(
            workers.iter().all(|(_, ready)| *ready),
            "All sandbox workers must be Ready: {:?}",
            workers
        );
    }

    #[test]
    fn test_parse_get_nodes_tower_ha_topology() {
        // tower: 3 CPs + 2 workers
        let output = "\
NAME             STATUS   ROLES           AGE   VERSION
tower-cp-0       Ready    control-plane   43h   v1.32.0
tower-cp-1       Ready    control-plane   43h   v1.32.0
tower-cp-2       Ready    control-plane   43h   v1.32.0
tower-worker-0   Ready    <none>          43h   v1.32.0
tower-worker-1   Ready    <none>          43h   v1.32.0";

        let nodes = parse_kubectl_get_nodes_output(output);
        assert_eq!(nodes.len(), 5, "tower has 3 CPs + 2 workers = 5 nodes");

        let workers: Vec<&(String, bool)> = nodes
            .iter()
            .filter(|(n, _)| n.starts_with("tower-worker-"))
            .collect();
        assert_eq!(workers.len(), 2, "Must detect 2 tower workers");
        assert!(
            workers.iter().all(|(_, ready)| *ready),
            "All tower workers must be Ready: {:?}",
            workers
        );
    }

    #[test]
    fn test_parse_get_nodes_mixed_ready_status() {
        // Some nodes ready, some not — mixed
        let output = "\
NAME             STATUS    ROLES           AGE   VERSION
tower-cp-0       Ready     control-plane   43h   v1.32.0
tower-worker-0   Ready     <none>          43h   v1.32.0
tower-worker-1   NotReady  <none>          30s   v1.32.0";

        let nodes = parse_kubectl_get_nodes_output(output);

        let ready_count = nodes.iter().filter(|(_, r)| *r).count();
        let not_ready_count = nodes.iter().filter(|(_, r)| !*r).count();

        assert_eq!(ready_count, 2, "2 nodes should be Ready");
        assert_eq!(not_ready_count, 1, "1 node should be NotReady");
    }

    #[test]
    fn test_parse_get_nodes_only_worker_names_checked() {
        // Verify that filtering by worker names in the real verify function
        // would only flag workers, not CPs, as missing.
        let output = "\
NAME             STATUS   ROLES           AGE   VERSION
tower-cp-0       Ready    control-plane   43h   v1.32.0
tower-worker-0   Ready    <none>          43h   v1.32.0";

        let nodes = parse_kubectl_get_nodes_output(output);
        let worker_names = vec!["tower-worker-0".to_string()];

        let not_ready: Vec<&str> = worker_names
            .iter()
            .filter(|wn| !nodes.iter().any(|(name, ready)| name == *wn && *ready))
            .map(|s| s.as_str())
            .collect();

        assert!(
            not_ready.is_empty(),
            "tower-worker-0 is Ready, not_ready must be empty — got: {:?}",
            not_ready
        );
    }

    #[test]
    fn test_parse_get_nodes_all_workers_missing() {
        // If output is empty, all workers are "missing" (not Ready)
        let nodes = parse_kubectl_get_nodes_output("");
        let worker_names = vec!["tower-worker-0".to_string(), "tower-worker-1".to_string()];

        let not_ready: Vec<&str> = worker_names
            .iter()
            .filter(|wn| !nodes.iter().any(|(name, ready)| name == *wn && *ready))
            .map(|s| s.as_str())
            .collect();

        assert_eq!(
            not_ready.len(),
            2,
            "Both workers must be not-ready when output is empty"
        );
    }

    // ---------------------------------------------------------------------------
    // AC 8.3 — CNI plugin application and NotReady → Ready transition
    // ---------------------------------------------------------------------------

    /// generate_kubectl_wait_nodes_ready_args: basic structure check.
    #[test]
    fn test_kubectl_wait_nodes_ready_args_structure() {
        let args = generate_kubectl_wait_nodes_ready_args("/tmp/kube.yaml", "600s");
        assert_eq!(args[0], "wait", "first arg must be 'wait'");
        assert!(
            args.contains(&"--for=condition=Ready".to_string()),
            "must specify --for=condition=Ready — got: {:?}",
            args
        );
        assert!(
            args.contains(&"nodes".to_string()),
            "resource kind must be 'nodes' — got: {:?}",
            args
        );
        assert!(
            args.contains(&"--all".to_string()),
            "must target all nodes with --all — got: {:?}",
            args
        );
    }

    /// generate_kubectl_wait_nodes_ready_args: timeout and kubeconfig are threaded through.
    #[test]
    fn test_kubectl_wait_nodes_ready_args_timeout_and_kubeconfig() {
        let args = generate_kubectl_wait_nodes_ready_args("/clusters/tower/kubeconfig.yaml", "300s");

        let timeout_idx = args
            .iter()
            .position(|a| a == "--timeout")
            .expect("--timeout must be present");
        assert_eq!(
            args[timeout_idx + 1], "300s",
            "timeout value must follow --timeout"
        );

        let kc_idx = args
            .iter()
            .position(|a| a == "--kubeconfig")
            .expect("--kubeconfig must be present");
        assert_eq!(
            args[kc_idx + 1], "/clusters/tower/kubeconfig.yaml",
            "kubeconfig path must follow --kubeconfig"
        );
    }

    /// generate_kubectl_wait_nodes_ready_args: default 600 s timeout is long enough for
    /// Cilium init containers to pull images and configure the pod network on bare-metal.
    #[test]
    fn test_kubectl_wait_nodes_ready_default_timeout_is_sufficient() {
        let args = generate_kubectl_wait_nodes_ready_args("/kube.yaml", "600s");
        let timeout_idx = args.iter().position(|a| a == "--timeout").unwrap();
        let timeout_val = &args[timeout_idx + 1];
        // Extract the numeric seconds value
        let seconds: u64 = timeout_val
            .trim_end_matches('s')
            .parse()
            .expect("timeout must be a numeric value ending in 's'");
        assert!(
            seconds >= 300,
            "default timeout ({seconds}s) must be ≥ 300s to allow Cilium init on slow nodes"
        );
    }

    /// count_ready_nodes: all ready.
    #[test]
    fn test_count_ready_nodes_all_ready() {
        let statuses = vec![
            ("tower-cp-0".to_string(), true),
            ("tower-worker-0".to_string(), true),
            ("tower-worker-1".to_string(), true),
        ];
        assert_eq!(
            count_ready_nodes(&statuses),
            3,
            "all 3 nodes are Ready — count must be 3"
        );
    }

    /// count_ready_nodes: mixed state.
    #[test]
    fn test_count_ready_nodes_mixed() {
        let statuses = vec![
            ("cp-0".to_string(), true),
            ("worker-0".to_string(), false),
            ("worker-1".to_string(), true),
        ];
        assert_eq!(
            count_ready_nodes(&statuses),
            2,
            "2 out of 3 nodes are Ready"
        );
    }

    /// count_ready_nodes: none ready (just joined, CNI not yet applied).
    #[test]
    fn test_count_ready_nodes_none_ready() {
        let statuses = vec![
            ("worker-0".to_string(), false),
            ("worker-1".to_string(), false),
        ];
        assert_eq!(
            count_ready_nodes(&statuses),
            0,
            "no nodes ready — count must be 0 (CNI not yet applied)"
        );
    }

    /// count_ready_nodes: empty status list.
    #[test]
    fn test_count_ready_nodes_empty() {
        let statuses: Vec<(String, bool)> = vec![];
        assert_eq!(count_ready_nodes(&statuses), 0);
    }

    /// nodes_not_yet_ready: happy path — all expected nodes are Ready.
    #[test]
    fn test_nodes_not_yet_ready_all_ready() {
        let statuses = vec![
            ("tower-worker-0".to_string(), true),
            ("tower-worker-1".to_string(), true),
        ];
        let expected = &["tower-worker-0", "tower-worker-1"];
        let not_ready = nodes_not_yet_ready(expected, &statuses);
        assert!(
            not_ready.is_empty(),
            "all nodes are Ready — not_ready must be empty, got: {:?}",
            not_ready
        );
    }

    /// nodes_not_yet_ready: one node still NotReady (waiting for CNI).
    #[test]
    fn test_nodes_not_yet_ready_one_missing() {
        let statuses = vec![
            ("tower-worker-0".to_string(), true),
            ("tower-worker-1".to_string(), false), // NotReady — CNI not yet applied
        ];
        let expected = &["tower-worker-0", "tower-worker-1"];
        let not_ready = nodes_not_yet_ready(expected, &statuses);
        assert_eq!(
            not_ready,
            vec!["tower-worker-1"],
            "tower-worker-1 is NotReady — must appear in not_ready list"
        );
    }

    /// nodes_not_yet_ready: node completely absent from output (not yet joined).
    #[test]
    fn test_nodes_not_yet_ready_absent_node() {
        let statuses = vec![("tower-worker-0".to_string(), true)];
        let expected = &["tower-worker-0", "tower-worker-1"];
        let not_ready = nodes_not_yet_ready(expected, &statuses);
        assert!(
            not_ready.contains(&"tower-worker-1".to_string()),
            "absent node must be reported as not-ready — got: {:?}",
            not_ready
        );
    }

    /// nodes_not_yet_ready: all 4 sandbox nodes Ready after CNI install.
    #[test]
    fn test_nodes_not_yet_ready_sandbox_all_ready() {
        let raw = "\
NAME               STATUS   ROLES           AGE   VERSION
sandbox-cp-0       Ready    control-plane   2h    v1.32.0
sandbox-worker-0   Ready    <none>          2h    v1.32.0
sandbox-worker-1   Ready    <none>          2h    v1.32.0
sandbox-worker-2   Ready    <none>          2h    v1.32.0";

        let statuses = parse_kubectl_get_nodes_output(raw);
        let expected = &[
            "sandbox-worker-0",
            "sandbox-worker-1",
            "sandbox-worker-2",
        ];
        let not_ready = nodes_not_yet_ready(expected, &statuses);
        assert!(
            not_ready.is_empty(),
            "all sandbox workers Ready after CNI install — got not_ready: {:?}",
            not_ready
        );
        assert_eq!(
            count_ready_nodes(&statuses),
            4,
            "all 4 sandbox nodes (1 CP + 3 workers) must be Ready"
        );
    }

    /// nodes_not_yet_ready: tower HA cluster — 5 nodes all Ready.
    #[test]
    fn test_nodes_not_yet_ready_tower_ha_all_ready() {
        let raw = "\
NAME             STATUS   ROLES           AGE   VERSION
tower-cp-0       Ready    control-plane   43h   v1.32.0
tower-cp-1       Ready    control-plane   43h   v1.32.0
tower-cp-2       Ready    control-plane   43h   v1.32.0
tower-worker-0   Ready    <none>          43h   v1.32.0
tower-worker-1   Ready    <none>          43h   v1.32.0";

        let statuses = parse_kubectl_get_nodes_output(raw);
        let expected = &["tower-worker-0", "tower-worker-1"];
        let not_ready = nodes_not_yet_ready(expected, &statuses);
        assert!(
            not_ready.is_empty(),
            "tower workers must all be Ready — got: {:?}",
            not_ready
        );
        assert_eq!(
            count_ready_nodes(&statuses),
            5,
            "all 5 tower nodes (3 CP + 2 workers) must be Ready"
        );
    }

    /// Verify that Cilium CNI is configured as the network plugin in kubespray vars.
    /// This is the key linkage: sdi-specs.yaml → k8s-clusters.yaml → kubespray vars.
    #[test]
    fn test_cni_kubespray_vars_include_cilium_network_plugin() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s_config.config.clusters {
            let vars =
                crate::core::kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);
            assert!(
                vars.contains("kube_network_plugin: cilium"),
                "cluster '{}' vars must set kube_network_plugin: cilium (required for nodes to become Ready) — got:\n{}",
                cluster.cluster_name,
                vars
            );
        }
    }

    /// Verify that kube_proxy_remove is set for Cilium strict mode.
    /// Cilium in kube-proxy replacement mode requires kube-proxy to be removed.
    #[test]
    fn test_cni_kubespray_vars_remove_kube_proxy_for_cilium() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        assert!(
            k8s_config.config.common.kube_proxy_remove,
            "kube_proxy_remove must be true when using Cilium CNI"
        );

        for cluster in &k8s_config.config.clusters {
            let vars =
                crate::core::kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);
            assert!(
                vars.contains("kube_proxy_remove: true"),
                "cluster '{}' must set kube_proxy_remove: true for Cilium strict mode",
                cluster.cluster_name
            );
        }
    }

    /// Integration: the full CNI readiness pipeline — parse nodes output,
    /// count ready nodes, and confirm the transition has completed.
    #[test]
    fn test_cni_readiness_pipeline_notready_to_ready_transition() {
        // Simulate the sequence: nodes initially NotReady (CNI not yet applied)
        let before_cni = "\
NAME             STATUS    ROLES           AGE   VERSION
tower-cp-0       NotReady  control-plane   5m    v1.32.0
tower-worker-0   NotReady  <none>          3m    v1.32.0
tower-worker-1   NotReady  <none>          3m    v1.32.0";

        let statuses_before = parse_kubectl_get_nodes_output(before_cni);
        let ready_before = count_ready_nodes(&statuses_before);
        assert_eq!(
            ready_before, 0,
            "before CNI: 0 nodes should be Ready (CNI not yet applied)"
        );

        // After Cilium CNI is applied, all nodes become Ready
        let after_cni = "\
NAME             STATUS   ROLES           AGE   VERSION
tower-cp-0       Ready    control-plane   5m    v1.32.0
tower-worker-0   Ready    <none>          3m    v1.32.0
tower-worker-1   Ready    <none>          3m    v1.32.0";

        let statuses_after = parse_kubectl_get_nodes_output(after_cni);
        let ready_after = count_ready_nodes(&statuses_after);
        assert_eq!(
            ready_after, 3,
            "after CNI: all 3 nodes must be Ready"
        );

        // Confirm zero not-yet-ready workers
        let expected_workers = &["tower-worker-0", "tower-worker-1"];
        let not_ready = nodes_not_yet_ready(expected_workers, &statuses_after);
        assert!(
            not_ready.is_empty(),
            "after CNI: no workers should be not-ready — got: {:?}",
            not_ready
        );
    }

    /// Verify that generate_kubectl_wait_nodes_ready_args targets the cluster kubeconfig,
    /// not a hardcoded path — essential for multi-cluster environments.
    #[test]
    fn test_kubectl_wait_nodes_ready_uses_cluster_kubeconfig() {
        let tower_kc = "_generated/clusters/tower/kubeconfig.yaml";
        let sandbox_kc = "_generated/clusters/sandbox/kubeconfig.yaml";

        let tower_args = generate_kubectl_wait_nodes_ready_args(tower_kc, "600s");
        let sandbox_args = generate_kubectl_wait_nodes_ready_args(sandbox_kc, "600s");

        assert!(
            tower_args.contains(&tower_kc.to_string()),
            "tower wait args must reference tower kubeconfig"
        );
        assert!(
            sandbox_args.contains(&sandbox_kc.to_string()),
            "sandbox wait args must reference sandbox kubeconfig"
        );
        // Must NOT cross-reference each other's kubeconfig
        assert!(
            !tower_args.contains(&sandbox_kc.to_string()),
            "tower wait args must NOT reference sandbox kubeconfig"
        );
    }

    // ---------------------------------------------------------------------------
    // AC 9c — manifest application and pod readiness verification
    // ---------------------------------------------------------------------------

    /// manifest_apply_order: sandbox cluster returns steps in namespace→rbac→workload order.
    #[test]
    fn test_manifest_apply_order_sandbox_contains_required_steps() {
        let steps = manifest_apply_order("sandbox");
        // Must include cluster-config (config), scalex-dash-rbac (rbac), sandbox rbac,
        // local-path-provisioner (workload), test-resources (workload).
        let paths: Vec<&str> = steps.iter().map(|s| s.relative_path.as_str()).collect();
        assert!(
            paths.contains(&"sandbox/cluster-config/manifest.yaml"),
            "sandbox steps must include cluster-config — got: {:?}",
            paths
        );
        assert!(
            paths.contains(&"common/scalex-dash-rbac/manifest.yaml"),
            "sandbox steps must include scalex-dash-rbac — got: {:?}",
            paths
        );
        assert!(
            paths.contains(&"sandbox/rbac/manifest.yaml"),
            "sandbox steps must include sandbox/rbac — got: {:?}",
            paths
        );
        assert!(
            paths.contains(&"sandbox/local-path-provisioner/manifest.yaml"),
            "sandbox steps must include local-path-provisioner — got: {:?}",
            paths
        );
        assert!(
            paths.contains(&"sandbox/test-resources/manifest.yaml"),
            "sandbox steps must include test-resources — got: {:?}",
            paths
        );
    }

    /// manifest_apply_order: tower cluster returns steps for tower-specific workloads.
    #[test]
    fn test_manifest_apply_order_tower_contains_required_steps() {
        let steps = manifest_apply_order("tower");
        let paths: Vec<&str> = steps.iter().map(|s| s.relative_path.as_str()).collect();
        assert!(
            paths.contains(&"tower/cluster-config/manifest.yaml"),
            "tower steps must include cluster-config — got: {:?}",
            paths
        );
        assert!(
            paths.contains(&"common/scalex-dash-rbac/manifest.yaml"),
            "tower steps must include scalex-dash-rbac — got: {:?}",
            paths
        );
        assert!(
            paths.contains(&"tower/local-path-provisioner/manifest.yaml"),
            "tower steps must include local-path-provisioner — got: {:?}",
            paths
        );
    }

    /// manifest_apply_order: Config steps appear before RBAC steps (namespace-first ordering).
    #[test]
    fn test_manifest_apply_order_config_before_rbac() {
        let steps = manifest_apply_order("sandbox");
        let config_pos = steps
            .iter()
            .position(|s| s.category == ManifestCategory::Config)
            .expect("must have a Config step");
        let rbac_pos = steps
            .iter()
            .position(|s| s.category == ManifestCategory::Rbac)
            .expect("must have at least one Rbac step");
        assert!(
            config_pos < rbac_pos,
            "Config steps (pos={config_pos}) must precede RBAC steps (pos={rbac_pos})"
        );
    }

    /// manifest_apply_order: RBAC steps appear before Workload steps.
    #[test]
    fn test_manifest_apply_order_rbac_before_workload() {
        let steps = manifest_apply_order("sandbox");
        let last_rbac = steps
            .iter()
            .rposition(|s| s.category == ManifestCategory::Rbac)
            .expect("must have at least one Rbac step");
        let first_workload = steps
            .iter()
            .position(|s| s.category == ManifestCategory::Workload)
            .expect("must have at least one Workload step");
        assert!(
            last_rbac < first_workload,
            "last RBAC step (pos={last_rbac}) must precede first Workload step (pos={first_workload})"
        );
    }

    /// manifest_apply_order: scalex-dash-rbac is present for both clusters
    /// (it is the token source for the scalex-dash TUI).
    #[test]
    fn test_manifest_apply_order_scalex_dash_rbac_in_both_clusters() {
        for cluster in &["sandbox", "tower"] {
            let steps = manifest_apply_order(cluster);
            let has_dash_rbac = steps
                .iter()
                .any(|s| s.relative_path == "common/scalex-dash-rbac/manifest.yaml");
            assert!(
                has_dash_rbac,
                "cluster '{}' must include scalex-dash-rbac (required for scalex dash TUI)",
                cluster
            );
        }
    }

    /// generate_manifest_apply_args: must produce `kubectl apply --kubeconfig <kc> -f <path>`.
    #[test]
    fn test_generate_manifest_apply_args_structure() {
        let args = generate_manifest_apply_args(
            "_generated/clusters/sandbox/kubeconfig.yaml",
            "gitops/sandbox/rbac/manifest.yaml",
        );
        assert!(
            args.contains(&"apply".to_string()),
            "args must contain 'apply' — got: {:?}",
            args
        );
        assert!(
            args.contains(&"--kubeconfig".to_string()),
            "args must contain '--kubeconfig' — got: {:?}",
            args
        );
        assert!(
            args.contains(&"_generated/clusters/sandbox/kubeconfig.yaml".to_string()),
            "args must contain kubeconfig path — got: {:?}",
            args
        );
        assert!(
            args.contains(&"-f".to_string()),
            "args must contain '-f' — got: {:?}",
            args
        );
        assert!(
            args.contains(&"gitops/sandbox/rbac/manifest.yaml".to_string()),
            "args must contain manifest path — got: {:?}",
            args
        );
    }

    /// generate_manifest_apply_args: kubeconfig and manifest paths are independent per cluster.
    #[test]
    fn test_generate_manifest_apply_args_cluster_isolation() {
        let sandbox_args = generate_manifest_apply_args(
            "_generated/clusters/sandbox/kubeconfig.yaml",
            "gitops/sandbox/rbac/manifest.yaml",
        );
        let tower_args = generate_manifest_apply_args(
            "_generated/clusters/tower/kubeconfig.yaml",
            "gitops/tower/cluster-config/manifest.yaml",
        );
        // Verify no cross-contamination
        assert!(
            !sandbox_args.contains(&"_generated/clusters/tower/kubeconfig.yaml".to_string()),
            "sandbox args must not reference tower kubeconfig"
        );
        assert!(
            !tower_args.contains(&"_generated/clusters/sandbox/kubeconfig.yaml".to_string()),
            "tower args must not reference sandbox kubeconfig"
        );
    }

    /// parse_kubectl_get_pods_output: happy path — all Running pods parsed correctly.
    #[test]
    fn test_parse_kubectl_get_pods_all_running() {
        let raw = "\
NAMESPACE       NAME                               READY   STATUS    RESTARTS   AGE
kube-system     coredns-5dd5756b68-abc12           1/1     Running   0          5m
kube-system     cilium-ds-node0                    1/1     Running   0          4m
local-path-storage  local-path-provisioner-xyz     1/1     Running   0          2m
scalex-system   scalex-dash-token-renewer           1/1     Running   0          1m";

        let pods = parse_kubectl_get_pods_output(raw);
        assert_eq!(pods.len(), 4, "should parse 4 pods — got: {:?}", pods);
        assert!(pods.iter().all(|p| p.status == "Running"));
    }

    /// parse_kubectl_get_pods_output: mixed healthy and unhealthy states.
    #[test]
    fn test_parse_kubectl_get_pods_mixed_states() {
        let raw = "\
NAMESPACE     NAME             READY   STATUS             RESTARTS   AGE
kube-system   coredns-abc      1/1     Running            0          5m
kube-system   bad-pod          0/1     CrashLoopBackOff   3          2m
kube-system   pending-pod      0/1     Pending            0          1m";

        let pods = parse_kubectl_get_pods_output(raw);
        assert_eq!(pods.len(), 3);
        let unhealthy = find_unhealthy_pods(&pods);
        assert_eq!(unhealthy.len(), 2, "two pods unhealthy — got: {:?}", unhealthy);
        let statuses: Vec<&str> = unhealthy.iter().map(|p| p.status.as_str()).collect();
        assert!(statuses.contains(&"CrashLoopBackOff"), "CrashLoopBackOff must be flagged");
        assert!(statuses.contains(&"Pending"), "Pending must be flagged");
    }

    /// parse_kubectl_get_pods_output: header line is skipped.
    #[test]
    fn test_parse_kubectl_get_pods_skips_header() {
        let raw = "NAMESPACE   NAME   READY   STATUS   RESTARTS   AGE\n\
                   default     my-pod 1/1     Running  0          5m";
        let pods = parse_kubectl_get_pods_output(raw);
        assert_eq!(pods.len(), 1, "header must not appear as a pod — got: {:?}", pods);
        assert_eq!(pods[0].name, "my-pod");
    }

    /// parse_kubectl_get_pods_output: empty output returns empty vec.
    #[test]
    fn test_parse_kubectl_get_pods_empty() {
        let pods = parse_kubectl_get_pods_output("");
        assert!(pods.is_empty());
    }

    /// is_pod_healthy: Running/Completed/Succeeded are healthy.
    #[test]
    fn test_is_pod_healthy_positive_cases() {
        assert!(is_pod_healthy("Running"), "Running must be healthy");
        assert!(is_pod_healthy("Completed"), "Completed must be healthy");
        assert!(is_pod_healthy("Succeeded"), "Succeeded must be healthy");
    }

    /// is_pod_healthy: Pending/Error/CrashLoopBackOff are unhealthy.
    #[test]
    fn test_is_pod_healthy_negative_cases() {
        assert!(!is_pod_healthy("Pending"), "Pending must be unhealthy");
        assert!(!is_pod_healthy("Error"), "Error must be unhealthy");
        assert!(!is_pod_healthy("CrashLoopBackOff"), "CrashLoopBackOff must be unhealthy");
        assert!(!is_pod_healthy("OOMKilled"), "OOMKilled must be unhealthy");
        assert!(!is_pod_healthy("ImagePullBackOff"), "ImagePullBackOff must be unhealthy");
        assert!(!is_pod_healthy("Terminating"), "Terminating must be unhealthy");
        assert!(!is_pod_healthy("Unknown"), "Unknown must be unhealthy");
    }

    /// all_pods_healthy: returns true when every pod is Running/Completed.
    #[test]
    fn test_all_pods_healthy_all_running() {
        let pods = vec![
            PodStatus { namespace: "kube-system".to_string(), name: "coredns".to_string(), ready: "1/1".to_string(), status: "Running".to_string() },
            PodStatus { namespace: "scalex-system".to_string(), name: "scalex-dash".to_string(), ready: "1/1".to_string(), status: "Running".to_string() },
        ];
        assert!(all_pods_healthy(&pods), "all Running pods must be healthy");
    }

    /// all_pods_healthy: returns false when one pod is Pending.
    #[test]
    fn test_all_pods_healthy_one_pending() {
        let pods = vec![
            PodStatus { namespace: "kube-system".to_string(), name: "coredns".to_string(), ready: "1/1".to_string(), status: "Running".to_string() },
            PodStatus { namespace: "default".to_string(), name: "slow-pod".to_string(), ready: "0/1".to_string(), status: "Pending".to_string() },
        ];
        assert!(!all_pods_healthy(&pods), "one Pending pod must make all_pods_healthy return false");
    }

    /// find_unhealthy_pods: returns only the unhealthy subset.
    #[test]
    fn test_find_unhealthy_pods_filters_correctly() {
        let pods = vec![
            PodStatus { namespace: "ns".to_string(), name: "ok".to_string(), ready: "1/1".to_string(), status: "Running".to_string() },
            PodStatus { namespace: "ns".to_string(), name: "bad".to_string(), ready: "0/1".to_string(), status: "Error".to_string() },
            PodStatus { namespace: "ns".to_string(), name: "done".to_string(), ready: "0/1".to_string(), status: "Completed".to_string() },
        ];
        let unhealthy = find_unhealthy_pods(&pods);
        assert_eq!(unhealthy.len(), 1, "only 'bad' must be unhealthy — got: {:?}", unhealthy);
        assert_eq!(unhealthy[0].name, "bad");
    }

    /// find_unhealthy_pods: returns empty vec when all pods are healthy.
    #[test]
    fn test_find_unhealthy_pods_all_healthy() {
        let pods = vec![
            PodStatus { namespace: "kube-system".to_string(), name: "apiserver".to_string(), ready: "1/1".to_string(), status: "Running".to_string() },
        ];
        let unhealthy = find_unhealthy_pods(&pods);
        assert!(unhealthy.is_empty(), "all healthy — unhealthy list must be empty");
    }

    /// generate_wait_deployment_args: structure is correct.
    #[test]
    fn test_generate_wait_deployment_args_structure() {
        let args = generate_wait_deployment_args(
            "_generated/clusters/sandbox/kubeconfig.yaml",
            "local-path-storage",
            "local-path-provisioner",
            "120s",
        );
        assert!(args.contains(&"wait".to_string()), "must contain 'wait'");
        assert!(args.contains(&"deployment".to_string()), "must contain 'deployment'");
        assert!(args.contains(&"local-path-provisioner".to_string()), "must contain deployment name");
        assert!(
            args.contains(&"--for=condition=Available".to_string()),
            "must wait for Available condition"
        );
        assert!(
            args.iter().any(|a| a.contains("local-path-storage")),
            "must specify namespace"
        );
        assert!(
            args.iter().any(|a| a.contains("120s")),
            "must specify timeout"
        );
        assert!(
            args.contains(&"--kubeconfig".to_string()),
            "must pass kubeconfig"
        );
    }

    /// Integration: manifest_apply_order for sandbox covers all 4 required categories.
    #[test]
    fn test_manifest_apply_order_sandbox_covers_all_categories() {
        let steps = manifest_apply_order("sandbox");
        let has_config = steps.iter().any(|s| s.category == ManifestCategory::Config);
        let has_rbac = steps.iter().any(|s| s.category == ManifestCategory::Rbac);
        let has_workload = steps.iter().any(|s| s.category == ManifestCategory::Workload);
        assert!(has_config, "sandbox must have at least one Config step");
        assert!(has_rbac, "sandbox must have at least one RBAC step");
        assert!(has_workload, "sandbox must have at least one Workload step");
    }

    /// Verify that gitops manifest files referenced in manifest_apply_order actually exist.
    #[test]
    fn test_manifest_apply_order_sandbox_files_exist() {
        let gitops_root = std::path::Path::new("../gitops");
        if !gitops_root.exists() {
            // Running from workspace root instead
            let gitops_root2 = std::path::Path::new("gitops");
            if !gitops_root2.exists() {
                // Skip if gitops dir not accessible from test runner CWD
                return;
            }
        }
        let gitops_root = if std::path::Path::new("gitops").exists() {
            std::path::Path::new("gitops")
        } else {
            std::path::Path::new("../gitops")
        };
        for step in manifest_apply_order("sandbox") {
            let path = gitops_root.join(&step.relative_path);
            assert!(
                path.exists(),
                "manifest file must exist on disk: {} (step: {})",
                path.display(),
                step.description
            );
        }
    }

    /// Verify that gitops manifest files for tower cluster exist.
    #[test]
    fn test_manifest_apply_order_tower_files_exist() {
        let gitops_root = if std::path::Path::new("gitops").exists() {
            std::path::Path::new("gitops")
        } else if std::path::Path::new("../gitops").exists() {
            std::path::Path::new("../gitops")
        } else {
            return; // skip if not accessible
        };
        for step in manifest_apply_order("tower") {
            let path = gitops_root.join(&step.relative_path);
            assert!(
                path.exists(),
                "tower manifest file must exist on disk: {} (step: {})",
                path.display(),
                step.description
            );
        }
    }

    /// parse_kubectl_get_pods_output: Completed pods are treated as healthy
    /// (used by Job-based setup pods e.g. cert-manager webhooks).
    #[test]
    fn test_parse_kubectl_get_pods_completed_is_healthy() {
        let raw = "\
NAMESPACE   NAME        READY   STATUS      RESTARTS   AGE
default     setup-job   0/1     Completed   0          30s";
        let pods = parse_kubectl_get_pods_output(raw);
        assert_eq!(pods.len(), 1);
        assert!(
            is_pod_healthy(&pods[0].status),
            "Completed pod must be treated as healthy"
        );
        assert!(all_pods_healthy(&pods), "single Completed pod must pass all_pods_healthy");
    }
}
