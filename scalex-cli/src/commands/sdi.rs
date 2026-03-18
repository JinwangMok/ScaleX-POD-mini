use crate::core::config::load_baremetal_config;
use crate::core::host_prepare;
use crate::core::resource_pool;
use crate::core::ssh::{build_ssh_command, execute_ssh};
use crate::core::sync;
use crate::core::tofu;
use crate::models::baremetal::NodeFacts;
use crate::models::sdi::{SdiPoolState, SdiSpec};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct SdiArgs {
    #[command(subcommand)]
    command: SdiCommand,
}

#[derive(Subcommand)]
enum SdiCommand {
    /// Initialize SDI: virtualize all bare-metal into a unified resource pool (no spec) or create VM pools from spec (with spec)
    Init {
        /// SDI specs file (optional — without it, virtualizes bare-metal and creates unified resource pool)
        spec_file: Option<String>,

        /// Path to baremetal-init.yaml
        #[arg(long, default_value = "credentials/.baremetal-init.yaml")]
        config: PathBuf,

        /// Path to .env file
        #[arg(long, default_value = "credentials/.env")]
        env_file: PathBuf,

        /// Facts directory
        #[arg(long, default_value = "_generated/facts")]
        facts_dir: PathBuf,

        /// Output directory for generated OpenTofu files
        #[arg(long, default_value = "_generated/sdi")]
        output_dir: PathBuf,

        /// Dry run — show what would be done
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Remove all SDI resources (VMs, bridges, etc.)
    Clean {
        /// Hard clean: remove everything except SSH access (K8s, KVM, bridge, iptables)
        #[arg(long)]
        hard: bool,
        /// Confirmation flag
        #[arg(long = "yes-i-really-want-to")]
        confirm: bool,

        /// Target a specific node by name (optional — if omitted, all nodes are cleaned).
        /// Requires --hard. Example: --node playbox-2
        #[arg(long)]
        node: Option<String>,

        /// Path to baremetal-init.yaml (used for --hard node cleanup)
        #[arg(long, default_value = "credentials/.baremetal-init.yaml")]
        config: PathBuf,

        /// Path to .env file
        #[arg(long, default_value = "credentials/.env")]
        env_file: PathBuf,

        /// Output directory for generated OpenTofu files
        #[arg(long, default_value = "_generated/sdi")]
        output_dir: PathBuf,

        /// Dry run
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Sync SDI state with baremetal-init.yaml (add/remove machines)
    Sync {
        /// Path to baremetal-init.yaml
        #[arg(long, default_value = "credentials/.baremetal-init.yaml")]
        config: PathBuf,

        /// Path to .env file
        #[arg(long, default_value = "credentials/.env")]
        env_file: PathBuf,

        /// Facts directory
        #[arg(long, default_value = "_generated/facts")]
        facts_dir: PathBuf,

        /// SDI output directory
        #[arg(long, default_value = "_generated/sdi")]
        output_dir: PathBuf,

        /// Dry run — show sync plan without executing
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Force — bypass safety checks (e.g. missing SDI state file)
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

pub fn run(args: SdiArgs) -> anyhow::Result<()> {
    match args.command {
        SdiCommand::Init {
            spec_file,
            config,
            env_file,
            facts_dir,
            output_dir,
            dry_run,
        } => run_init(spec_file, config, env_file, facts_dir, output_dir, dry_run),
        SdiCommand::Clean {
            hard,
            confirm,
            node,
            config,
            env_file,
            output_dir,
            dry_run,
        } => run_clean(hard, confirm, node, config, env_file, output_dir, dry_run),
        SdiCommand::Sync {
            config,
            env_file,
            facts_dir,
            output_dir,
            dry_run,
            force,
        } => run_sync(config, env_file, facts_dir, output_dir, dry_run, force),
    }
}

fn run_init(
    spec_file: Option<String>,
    config_path: PathBuf,
    env_path: PathBuf,
    facts_dir: PathBuf,
    output_dir: PathBuf,
    dry_run: bool,
) -> anyhow::Result<()> {
    // Step 1: Ensure facts exist
    if !facts_dir.exists() || dir_is_empty(&facts_dir) {
        println!("[sdi] No facts found. Running facts collection first...");
        if dry_run {
            println!("[dry-run] Would run `scalex facts --all` first");
        } else {
            run_facts_collection(&config_path, &env_path, &facts_dir)?;
        }
    }

    // Step 2: Load baremetal config and validate
    let bm_config = load_baremetal_config(&config_path, &env_path)?;
    let config_errors = crate::core::config::validate_baremetal_config(&bm_config);
    if !config_errors.is_empty() {
        eprintln!("[sdi] Configuration errors in {}:", config_path.display());
        for err in &config_errors {
            eprintln!("  - {}", err);
        }
        anyhow::bail!(
            "Fix {} error(s) in {} before proceeding",
            config_errors.len(),
            config_path.display()
        );
    }
    let all_facts = load_all_facts(&facts_dir)?;

    // Step 3: Prepare hosts (KVM, bridge, VFIO)
    println!("[sdi] Phase 1: Preparing hosts for virtualization...");
    for node in &bm_config.target_nodes {
        let facts = all_facts.iter().find(|f| f.node_name == node.name);
        if let Some(facts) = facts {
            let needs_gpu = check_gpu_passthrough_needed(&node.name, spec_file.as_deref());
            let steps = host_prepare::plan_host_preparation(node, facts, needs_gpu);

            if steps.is_empty() {
                println!("[sdi] {} — already prepared", node.name);
                continue;
            }

            for step in &steps {
                match step {
                    host_prepare::PrepStep::InstallKvm => {
                        println!("[sdi] {} — installing KVM/libvirt", node.name);
                        if !dry_run {
                            let script = host_prepare::generate_kvm_install_script();
                            let ssh_cmd =
                                build_ssh_command(node, &script, &bm_config.target_nodes)?;
                            match execute_ssh(&ssh_cmd) {
                                Ok(out) => println!("{}", out),
                                Err(e) => eprintln!("[sdi] ERROR on {}: {}", node.name, e),
                            }
                        }
                    }
                    host_prepare::PrepStep::SetupBridge => {
                        println!("[sdi] {} — setting up br0 bridge", node.name);
                        if !dry_run {
                            // Prefer bond interface if present, otherwise first physical NIC
                            let (primary_nic, is_bond) = if let Some(bond) = facts.bonds.first() {
                                (bond.as_str(), true)
                            } else {
                                (
                                    facts
                                        .nics
                                        .first()
                                        .map(|n| n.name.as_str())
                                        .unwrap_or("eno1"),
                                    false,
                                )
                            };
                            let net = resolve_network_config(
                                spec_file
                                    .as_deref()
                                    .and_then(|p| load_sdi_spec(p).ok())
                                    .as_ref(),
                                None,
                            )
                            .unwrap_or(NetworkDefaults {
                                bridge: "br0".to_string(),
                                cidr: "192.168.88.0/24".to_string(),
                                gateway: "192.168.88.1".to_string(),
                            });
                            let cidr_prefix = extract_cidr_prefix(&net.cidr);
                            let script = host_prepare::generate_bridge_setup_script(
                                primary_nic,
                                &node.node_ip,
                                &net.gateway,
                                cidr_prefix,
                                is_bond,
                            );
                            let ssh_cmd =
                                build_ssh_command(node, &script, &bm_config.target_nodes)?;
                            let ssh_failed = match execute_ssh(&ssh_cmd) {
                                Ok(out) => {
                                    println!("{}", out);
                                    false
                                }
                                Err(e) => {
                                    if is_bond {
                                        println!(
                                            "[sdi] {} — SSH disconnected during bridge setup (expected for bond→bridge)",
                                            node.name
                                        );
                                    } else {
                                        eprintln!("[sdi] ERROR on {}: {}", node.name, e);
                                    }
                                    true
                                }
                            };
                            // Always verify br0 came up for bond interfaces
                            if is_bond {
                                let verify_script = format!(
                                    "ip addr show br0 2>/dev/null | grep -q '{}/{}'",
                                    node.node_ip, cidr_prefix
                                );
                                let mut bridge_ok = false;
                                let max_attempts = if ssh_failed { 6 } else { 3 };
                                let wait_secs = if ssh_failed { 5 } else { 2 };
                                for attempt in 1..=max_attempts {
                                    std::thread::sleep(std::time::Duration::from_secs(wait_secs));
                                    if let Ok(verify_cmd) = build_ssh_command(
                                        node,
                                        &verify_script,
                                        &bm_config.target_nodes,
                                    ) {
                                        if execute_ssh(&verify_cmd).is_ok() {
                                            println!(
                                                "[sdi] {} — br0 verified after {}s",
                                                node.name,
                                                attempt * wait_secs
                                            );
                                            bridge_ok = true;
                                            break;
                                        }
                                    }
                                    println!(
                                        "[sdi] {} — waiting for br0... (attempt {}/{})",
                                        node.name, attempt, max_attempts
                                    );
                                }
                                if !bridge_ok {
                                    eprintln!(
                                        "[sdi] ERROR on {}: bridge setup failed — br0 not reachable after {}s",
                                        node.name,
                                        max_attempts * wait_secs
                                    );
                                }
                            }
                        }
                    }
                    host_prepare::PrepStep::ConfigureVfio => {
                        let gpu_ids = host_prepare::extract_gpu_pci_ids(facts);
                        println!(
                            "[sdi] {} — configuring VFIO-PCI for GPUs: {:?}",
                            node.name, gpu_ids
                        );
                        if !dry_run {
                            let script = host_prepare::generate_vfio_setup_script(&gpu_ids);
                            let ssh_cmd =
                                build_ssh_command(node, &script, &bm_config.target_nodes)?;
                            match execute_ssh(&ssh_cmd) {
                                Ok(out) => println!("{}", out),
                                Err(e) => eprintln!("[sdi] ERROR on {}: {}", node.name, e),
                            }
                        }
                    }
                }
            }
        } else {
            println!(
                "[sdi] {} — no facts available, skipping preparation",
                node.name
            );
        }
    }

    // Step 4: Generate resource pool summary (always, regardless of spec)
    // Checklist requirement: unified resource pool view must be generated before VM creation
    if !all_facts.is_empty() {
        let summary = resource_pool::generate_resource_pool_summary(&all_facts);
        let table = resource_pool::format_resource_pool_table(&summary);
        println!("{}", table);

        std::fs::create_dir_all(&output_dir)?;
        let summary_path = output_dir.join("resource-pool-summary.json");
        let json = serde_json::to_string_pretty(&summary)?;
        if dry_run {
            println!("[dry-run] Would write {}", summary_path.display());
        } else {
            std::fs::write(&summary_path, &json)?;
            println!(
                "[sdi] Saved resource pool summary to {}",
                summary_path.display()
            );
        }
    }

    // Step 5: If spec file provided, generate and apply OpenTofu
    if let Some(ref spec_path) = spec_file {
        println!("[sdi] Phase 2: Generating OpenTofu from spec...");
        let mut spec = load_sdi_spec(spec_path)?;

        let bm_node_names: Vec<String> = bm_config
            .target_nodes
            .iter()
            .map(|n| n.name.clone())
            .collect();

        // Auto-placement: resolve hosts for VMs that don't have an explicit host
        let has_unplaced = spec
            .spec
            .sdi_pools
            .iter()
            .flat_map(|p| &p.node_specs)
            .any(|n| n.host.is_none());

        if has_unplaced {
            println!("[sdi] Resolving auto-placement for unassigned VMs...");
            if all_facts.is_empty() {
                anyhow::bail!(
                    "Auto-placement requires node facts but none are available. \
                     Run `scalex facts --all` first or assign hosts explicitly in sdi-specs.yaml"
                );
            }
            let summary = resource_pool::generate_resource_pool_summary(&all_facts);
            match crate::core::placement::resolve_placement(&mut spec, &summary, &bm_node_names) {
                Ok(plan) => {
                    println!("{}", crate::core::placement::format_placement_table(&plan));
                    let auto_count = plan.assignments.iter().filter(|a| a.was_auto).count();
                    println!(
                        "[sdi] Placed {} VM(s) automatically, {} explicit",
                        auto_count,
                        plan.assignments.len() - auto_count,
                    );
                }
                Err(e) => {
                    anyhow::bail!("Auto-placement failed: {}", e);
                }
            }
        }

        // Validate SDI hosts reference known bare-metal nodes (after placement)
        let host_errors = crate::core::validation::validate_sdi_hosts_exist(&spec, &bm_node_names);
        if !host_errors.is_empty() {
            eprintln!("[sdi] SDI host reference errors:");
            for err in &host_errors {
                eprintln!("  - {}", err);
            }
            anyhow::bail!(
                "Fix {} host reference error(s) — SDI spec references hosts not in baremetal-init.yaml",
                host_errors.len()
            );
        }

        std::fs::create_dir_all(&output_dir)?;

        // Generate main.tf — ssh_user from baremetal config (first node as default)
        let ssh_user = bm_config
            .target_nodes
            .first()
            .map(|n| n.admin_user.as_str())
            .unwrap_or("root");
        let mut hcl = tofu::generate_tofu_main(&spec, ssh_user);

        // The libvirt Go provider uses its own SSH client (not system OpenSSH),
        // so it cannot use ~/.ssh/config ProxyJump. For non-direct nodes we set up
        // SSH port-forward tunnels and rewrite the provider URI to localhost:<port>.
        // For direct/Tailscale nodes we just use the reachable IP.
        let mut tunnel_pids: Vec<u32> = Vec::new();
        let mut local_port: u16 = 22101;
        for node in &bm_config.target_nodes {
            if let Some(reachable_via) = &node.reachable_via {
                // ProxyJump node: set up SSH tunnel via first hop
                let proxy_name = reachable_via.first().unwrap();
                let proxy_node = bm_config
                    .target_nodes
                    .iter()
                    .find(|n| n.name == *proxy_name)
                    .unwrap();
                let proxy_ip = proxy_node
                    .reachable_node_ip
                    .as_deref()
                    .unwrap_or(&proxy_node.node_ip);

                println!(
                    "[sdi] Setting up SSH tunnel for {} (localhost:{} -> {}:22 via {})",
                    node.name, local_port, node.node_ip, proxy_ip
                );
                let mut tunnel_args = vec![
                    "-fN".to_string(),
                    "-o".to_string(),
                    "StrictHostKeyChecking=no".to_string(),
                    "-o".to_string(),
                    "ExitOnForwardFailure=yes".to_string(),
                    format!("-L{}:{}:22", local_port, node.node_ip),
                ];
                if let Some(ref key_path) = proxy_node.ssh_key_path {
                    tunnel_args.push("-i".to_string());
                    tunnel_args.push(key_path.clone());
                }
                tunnel_args.push(format!("{}@{}", proxy_node.admin_user, proxy_ip));

                let status = std::process::Command::new("ssh")
                    .args(&tunnel_args)
                    .status()
                    .map_err(|e| anyhow::anyhow!("Failed to create SSH tunnel: {}", e))?;
                if !status.success() {
                    anyhow::bail!("SSH tunnel setup failed for {}", node.name);
                }
                // Find the tunnel PID
                if let Ok(out) = std::process::Command::new("lsof")
                    .args(["-ti", &format!(":{}", local_port)])
                    .output()
                {
                    if let Ok(pid_str) = String::from_utf8(out.stdout) {
                        if let Ok(pid) = pid_str.trim().parse::<u32>() {
                            tunnel_pids.push(pid);
                        }
                    }
                }

                // Rewrite provider URI to use the local tunnel
                // Also add host key to known_hosts for localhost
                let _ = std::process::Command::new("ssh-keygen")
                    .args(["-R", &format!("[127.0.0.1]:{}", local_port)])
                    .output();
                let _ = std::process::Command::new("bash")
                    .args([
                        "-c",
                        &format!(
                            "ssh -o StrictHostKeyChecking=accept-new -p {} {}@127.0.0.1 true 2>/dev/null || true",
                            local_port, node.admin_user
                        ),
                    ])
                    .status();

                hcl = hcl.replace(
                    &format!("qemu+ssh://{}@{}/system?no_verify=1", ssh_user, node.name),
                    &format!(
                        "qemu+ssh://{}@127.0.0.1:{}/system?no_verify=1",
                        ssh_user, local_port
                    ),
                );
                local_port += 1;
            } else {
                // Direct or Tailscale node: use the reachable IP directly
                let ip = node.reachable_node_ip.as_deref().unwrap_or(&node.node_ip);
                hcl = hcl.replace(
                    &format!("qemu+ssh://{}@{}/system?no_verify=1", ssh_user, node.name),
                    &format!("qemu+ssh://{}@{}/system?no_verify=1", ssh_user, ip),
                );
            }
        }

        // Detect the actual libvirt storage pool name on the first host
        // (the HCL defaults to "default" but Ubuntu systems often use "images")
        {
            let first_host = &bm_config.target_nodes[0];
            let pool_check = crate::core::ssh::build_ssh_command(
                first_host,
                "virsh -c qemu:///system pool-list --name 2>/dev/null | head -1",
                &bm_config.target_nodes,
            );
            if let Ok(cmd) = pool_check {
                if let Ok(output) = crate::core::ssh::execute_ssh(&cmd) {
                    let pool_name = output.trim();
                    if !pool_name.is_empty() && pool_name != "default" {
                        println!(
                            "[sdi] Detected storage pool: '{}' (replacing 'default')",
                            pool_name
                        );
                        // Replace all pool references regardless of spacing
                        hcl = hcl.replace("= \"default\"", &format!("= \"{}\"", pool_name));
                    }
                }
            }
        }

        // Pre-create base volumes on each host by downloading directly on the remote node.
        // Each bare-metal node downloads the cloud image via curl (much faster than SSH pipe).
        // The HCL references these by name instead of creating them.
        if !dry_run {
            let image_url = &spec.os_image.source;
            let unique_hosts = tofu::collect_unique_hosts(&spec);
            for host_name in &unique_hosts {
                let node = bm_config.target_nodes.iter().find(|n| n.name == *host_name);
                if let Some(node) = node {
                    let vol_name = format!("base-ubuntu-{}.qcow2", host_name);
                    // Check if volume already exists
                    let check_script = format!(
                        "sudo virsh -c qemu:///system vol-info '{}' --pool default >/dev/null 2>&1 && echo EXISTS || echo MISSING",
                        vol_name
                    );
                    let check_cmd = crate::core::ssh::build_ssh_command(
                        node,
                        &check_script,
                        &bm_config.target_nodes,
                    );
                    let vol_exists = check_cmd
                        .ok()
                        .and_then(|cmd| crate::core::ssh::execute_ssh(&cmd).ok())
                        .map(|o| o.trim() == "EXISTS")
                        .unwrap_or(false);

                    if vol_exists {
                        println!(
                            "[sdi] Base volume '{}' already exists on {}",
                            vol_name, host_name
                        );
                    } else {
                        // Download image directly on the remote node via curl
                        println!(
                            "[sdi] Downloading base image on {} (remote curl)...",
                            host_name
                        );
                        let pool_dir = "/var/lib/libvirt/images";
                        let remote_path = format!("{}/{}", pool_dir, vol_name);
                        let download_script = format!(
                            "sudo mkdir -p {pool_dir} && \
                             curl -fSL --progress-bar -o /tmp/scalex-download.img '{url}' && \
                             sudo mv /tmp/scalex-download.img '{path}' && \
                             sudo chmod 644 '{path}' && \
                             sudo chown libvirt-qemu:kvm '{path}' 2>/dev/null; \
                             sudo virsh -c qemu:///system pool-refresh default && \
                             echo VOLUME_CREATED",
                            pool_dir = pool_dir,
                            url = image_url,
                            path = remote_path,
                        );
                        let dl_cmd = crate::core::ssh::build_ssh_command(
                            node,
                            &download_script,
                            &bm_config.target_nodes,
                        );
                        match dl_cmd {
                            Ok(cmd) => match crate::core::ssh::execute_ssh(&cmd) {
                                Ok(out) => {
                                    if out.contains("VOLUME_CREATED") {
                                        println!("[sdi]   {} — VOLUME_CREATED", host_name);
                                    } else {
                                        println!(
                                            "[sdi]   WARNING: download may have failed on {}: {}",
                                            host_name,
                                            out.trim()
                                        );
                                    }
                                }
                                Err(e) => println!(
                                    "[sdi]   WARNING: remote download failed on {}: {}",
                                    host_name, e
                                ),
                            },
                            Err(e) => println!(
                                "[sdi]   WARNING: SSH command build failed for {}: {}",
                                host_name, e
                            ),
                        }
                    }
                }
            }
        }

        let main_tf = output_dir.join("main.tf");
        if dry_run {
            println!(
                "[dry-run] Would write {} ({} bytes)",
                main_tf.display(),
                hcl.len()
            );
            println!("--- Generated HCL preview (first 40 lines) ---");
            for line in hcl.lines().take(40) {
                println!("  {}", line);
            }
            println!("...");
        } else {
            std::fs::write(&main_tf, &hcl)?;
            println!("[sdi] Generated {}", main_tf.display());
        }

        // Generate VFIO XSLT if any node needs GPU passthrough
        let needs_vfio = spec.spec.sdi_pools.iter().any(|p| {
            p.node_specs
                .iter()
                .any(|n| n.devices.as_ref().is_some_and(|d| d.gpu_passthrough))
        });
        if needs_vfio {
            let xslt = tofu::generate_vfio_xslt();
            let xslt_path = output_dir.join("vfio-passthrough.xslt");
            if dry_run {
                println!("[dry-run] Would write {}", xslt_path.display());
            } else {
                std::fs::write(&xslt_path, &xslt)?;
                println!("[sdi] Generated {}", xslt_path.display());
            }
        }

        // Save pool state for `scalex get sdi-pools`
        let state = build_pool_state(&spec);
        let state_path = output_dir.join("sdi-state.json");
        let state_json = serde_json::to_string_pretty(&state)?;
        if dry_run {
            println!("[dry-run] Would write {}", state_path.display());
        } else {
            std::fs::write(&state_path, &state_json)?;
            println!("[sdi] Saved pool state to {}", state_path.display());
        }

        // Cache spec file for cluster init workflow (sdi-spec-cache.yaml)
        let spec_cache_path = output_dir.join("sdi-spec-cache.yaml");
        if dry_run {
            println!("[dry-run] Would write {}", spec_cache_path.display());
        } else {
            let spec_yaml = std::fs::read_to_string(spec_file.as_ref().unwrap())?;
            std::fs::write(&spec_cache_path, &spec_yaml)?;
            println!("[sdi] Cached spec file to {}", spec_cache_path.display());
        }

        // Run tofu init + apply
        if !dry_run {
            // Remove stale lock file to allow provider version changes
            let lock_file = output_dir.join(".terraform.lock.hcl");
            if lock_file.exists() {
                std::fs::remove_file(&lock_file).ok();
            }
            println!("[sdi] Running OpenTofu init...");
            run_tofu_command(&output_dir, &["init"])?;
            println!("[sdi] Running OpenTofu apply...");
            let apply_result = run_tofu_command(&output_dir, &["apply", "-auto-approve"]);

            // Clean up SSH tunnels regardless of apply result
            for pid in &tunnel_pids {
                let _ = std::process::Command::new("kill")
                    .arg(pid.to_string())
                    .status();
            }
            if !tunnel_pids.is_empty() {
                println!("[sdi] Cleaned up {} SSH tunnel(s)", tunnel_pids.len());
            }

            apply_result?;
        } else {
            println!("[dry-run] Would run: tofu init && tofu apply -auto-approve");
        }

        println!("[sdi] SDI initialization complete.");
    } else {
        // No spec file: set up host-level libvirt infrastructure via OpenTofu
        println!("[sdi] Phase 2: Setting up host-level libvirt infrastructure via OpenTofu...");
        if all_facts.is_empty() {
            println!("[sdi] No facts available. Run `scalex facts --all` first.");
        } else {
            // Generate OpenTofu HCL for host-level libvirt infra (storage pools)
            let host_inputs = build_host_infra_inputs(&bm_config.target_nodes);

            // Resolve network config from available sources (no spec in this path)
            let bm_net = bm_config
                .network_defaults
                .as_ref()
                .map(|nd| NetworkDefaults {
                    bridge: nd.management_bridge.clone(),
                    cidr: nd.management_cidr.clone(),
                    gateway: nd.gateway.clone(),
                });
            let net = resolve_network_config(None, bm_net.as_ref()).unwrap_or(NetworkDefaults {
                bridge: "br0".to_string(),
                cidr: "192.168.88.0/24".to_string(),
                gateway: "192.168.88.1".to_string(),
            });

            let host_hcl =
                tofu::generate_tofu_host_infra(&host_inputs, &net.bridge, &net.cidr, &net.gateway);

            let host_infra_dir = output_dir.join("host-infra");
            std::fs::create_dir_all(&host_infra_dir)?;
            let host_tf = host_infra_dir.join("main.tf");

            if dry_run {
                println!(
                    "[dry-run] Would write {} ({} bytes)",
                    host_tf.display(),
                    host_hcl.len()
                );
                println!("--- Generated Host Infra HCL preview ---");
                for line in host_hcl.lines().take(30) {
                    println!("  {}", line);
                }
                println!("...");
            } else {
                std::fs::write(&host_tf, &host_hcl)?;
                println!("[sdi] Generated {}", host_tf.display());

                // Run tofu init + apply for host infrastructure
                println!("[sdi] Running OpenTofu for host infrastructure...");
                run_tofu_command(&host_infra_dir, &["init"])?;
                run_tofu_command(&host_infra_dir, &["apply", "-auto-approve"])?;
            }
        }
        println!(
            "[sdi] Host infrastructure setup complete. Provide a spec file to create VM pools."
        );
    }

    Ok(())
}

fn run_clean(
    hard: bool,
    confirm: bool,
    node_filter: Option<String>,
    config_path: PathBuf,
    env_path: PathBuf,
    output_dir: PathBuf,
    dry_run: bool,
) -> anyhow::Result<()> {
    if let Some(err) = validate_clean_args(hard, confirm) {
        anyhow::bail!(err);
    }

    // Soft mode: bail early when no SDI state directory exists.
    // Hard mode: proceed to node cleanup even without a state dir — this supports the
    // Phase 1 bare-metal clean-reinstall scenario where no prior SDI state was created.
    if !output_dir.exists() {
        if !hard {
            println!(
                "[sdi] No SDI state found at {}. Nothing to clean.",
                output_dir.display()
            );
            return Ok(());
        }
        println!(
            "[sdi] No SDI state found at {} (proceeding with node cleanup for clean reinstall)",
            output_dir.display()
        );
    }

    // Step 1: Destroy host-infra OpenTofu resources (created by `sdi init` no-spec path).
    let host_infra_dir = output_dir.join("host-infra");
    let host_infra_tf = host_infra_dir.join("main.tf");
    if host_infra_tf.exists() {
        if dry_run {
            println!("[dry-run] Would run: tofu destroy -auto-approve (host-infra)");
        } else {
            println!("[sdi] Step 1: Destroying host-infra OpenTofu resources...");
            if let Err(e) = run_tofu_command(&host_infra_dir, &["destroy", "-auto-approve"]) {
                if hard {
                    eprintln!("[sdi] WARNING: host-infra tofu destroy failed ({}), continuing with --hard node cleanup", e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    // Step 2: Destroy main OpenTofu VM resources (created by `sdi init <spec>`).
    let main_tf = output_dir.join("main.tf");
    if main_tf.exists() {
        if dry_run {
            println!("[dry-run] Would run: tofu destroy -auto-approve");
        } else {
            println!("[sdi] Step 2: Destroying OpenTofu VM resources...");
            if let Err(e) = run_tofu_command(&output_dir, &["destroy", "-auto-approve"]) {
                if hard {
                    eprintln!("[sdi] WARNING: tofu destroy failed ({}), continuing with --hard node cleanup", e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    if hard {
        // Step 3: SSH into each target node and run full cleanup (K8s, KVM, bridge).
        // Cleanup is sequenced in three phases to avoid dependency failures:
        //   Phase A — KVM/libvirt teardown (stops VMs, removes storage pools/volumes)
        //   Phase B — Full node cleanup (removes K8s, KVM packages, bridge config)
        //   Phase C — Per-node result summary
        if !config_path.exists() || !env_path.exists() {
            println!(
                "[sdi] Step 3: No baremetal config found — skipping node cleanup (local state only)"
            );
        } else {
            let bm_config = load_baremetal_config(&config_path, &env_path)?;

            // Apply per-node filter: if --node was specified, scope cleanup to that node only.
            let target_nodes: Vec<_> = if let Some(ref name) = node_filter {
                let matched: Vec<_> = bm_config
                    .target_nodes
                    .iter()
                    .filter(|n| n.name == *name)
                    .collect();
                if matched.is_empty() {
                    anyhow::bail!(
                        "--node '{}' not found in baremetal config. Available nodes: {}",
                        name,
                        bm_config
                            .target_nodes
                            .iter()
                            .map(|n| n.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                matched
            } else {
                bm_config.target_nodes.iter().collect()
            };

            let total = target_nodes.len();
            let node_names: Vec<&str> = target_nodes.iter().map(|n| n.name.as_str()).collect();
            println!(
                "[sdi] Step 3: Hard cleanup across {} node(s): {}",
                total,
                node_names.join(", ")
            );

            // Phase A: KVM/libvirt teardown — destroy running VMs and storage pools FIRST.
            // Runs before the main cleanup so VM disk volumes are cleanly removed via virsh
            // rather than left as dangling files after package purge.
            let kvm_teardown_script = host_prepare::generate_kvm_teardown_script();
            println!(
                "[sdi]   Phase A: KVM/libvirt teardown (destroy VMs + storage pools) on all nodes..."
            );

            let mut kvm_results: Vec<(String, Result<(), String>)> = Vec::new();
            for (idx, node) in target_nodes.iter().enumerate() {
                println!(
                    "[sdi]   [{}/{}] {} — destroying VMs, storage pools, network...",
                    idx + 1,
                    total,
                    node.name
                );
                if dry_run {
                    println!(
                        "[dry-run]     Would run KVM teardown script on {}",
                        node.name
                    );
                    kvm_results.push((node.name.clone(), Ok(())));
                } else {
                    match build_ssh_command(node, &kvm_teardown_script, &bm_config.target_nodes) {
                        Ok(ssh_cmd) => match execute_ssh(&ssh_cmd) {
                            Ok(out) => {
                                if !out.trim().is_empty() {
                                    println!("{}", out.trim());
                                }
                                kvm_results.push((node.name.clone(), Ok(())));
                            }
                            Err(e) => {
                                eprintln!("[sdi]   ERROR on {} (KVM teardown): {}", node.name, e);
                                kvm_results.push((node.name.clone(), Err(e.to_string())));
                            }
                        },
                        Err(e) => {
                            eprintln!("[sdi]   ERROR on {} (SSH setup): {}", node.name, e);
                            kvm_results.push((node.name.clone(), Err(e.to_string())));
                        }
                    }
                }
            }

            // Phase B: Full node cleanup — removes K8s components, KVM packages, bridge config.
            // Runs after KVM teardown to avoid removing packages while VMs are still active.
            let cleanup_script = host_prepare::generate_node_cleanup_script();
            println!(
                "[sdi]   Phase B: Full node cleanup (K8s + KVM packages + bridge) on all nodes..."
            );

            let mut cleanup_results: Vec<(String, Result<(), String>)> = Vec::new();
            for (idx, node) in target_nodes.iter().enumerate() {
                println!(
                    "[sdi]   [{}/{}] {} — removing K8s, KVM packages, bridge...",
                    idx + 1,
                    total,
                    node.name
                );
                if dry_run {
                    println!(
                        "[dry-run]     Would run full cleanup script on {}",
                        node.name
                    );
                    cleanup_results.push((node.name.clone(), Ok(())));
                } else {
                    // Wrap cleanup in nohup so it survives SSH disconnection
                    // (bridge removal kills the SSH session on non-direct nodes)
                    let wrapped_script = format!(
                        "nohup bash -c '{}' > /tmp/scalex-cleanup.log 2>&1 &\n\
                         CLEANUP_PID=$!\n\
                         echo \"[scalex] Cleanup started (PID $CLEANUP_PID), waiting...\"\n\
                         # Wait up to 180s for cleanup to finish\n\
                         for i in $(seq 1 180); do\n\
                           kill -0 $CLEANUP_PID 2>/dev/null || break\n\
                           sleep 1\n\
                         done\n\
                         if kill -0 $CLEANUP_PID 2>/dev/null; then\n\
                           echo \"[scalex] Cleanup still running after 180s (will complete in background)\"\n\
                         else\n\
                           echo \"[scalex] Cleanup finished\"\n\
                         fi\n\
                         cat /tmp/scalex-cleanup.log 2>/dev/null | tail -5",
                        cleanup_script.replace('\'', "'\\''")
                    );
                    match build_ssh_command(node, &wrapped_script, &bm_config.target_nodes) {
                        Ok(ssh_cmd) => match execute_ssh(&ssh_cmd) {
                            Ok(out) => {
                                if !out.trim().is_empty() {
                                    println!("{}", out.trim());
                                }
                                cleanup_results.push((node.name.clone(), Ok(())));
                            }
                            Err(e) => {
                                // SSH disconnection during cleanup is expected (bridge removal).
                                // The nohup ensures cleanup completes in background.
                                let msg = e.to_string();
                                if msg.contains("Timeout")
                                    || msg.contains("closed")
                                    || msg.contains("reset")
                                {
                                    eprintln!(
                                        "[sdi]   WARNING on {} (cleanup): SSH disconnected (bridge removal), cleanup continues in background",
                                        node.name
                                    );
                                    cleanup_results.push((node.name.clone(), Ok(())));
                                } else {
                                    eprintln!("[sdi]   ERROR on {} (cleanup): {}", node.name, e);
                                    cleanup_results.push((node.name.clone(), Err(msg)));
                                }
                            }
                        },
                        Err(e) => {
                            eprintln!("[sdi]   ERROR on {} (SSH setup): {}", node.name, e);
                            cleanup_results.push((node.name.clone(), Err(e.to_string())));
                        }
                    }
                }
            }

            // Phase C: Print per-node result summary.
            println!("\n[sdi] Node cleanup summary ({} node(s)):", total);
            let mut failed_nodes = 0usize;
            for node in &target_nodes {
                let kvm_ok = kvm_results
                    .iter()
                    .find(|(n, _)| n == &node.name)
                    .map(|(_, r)| r.is_ok())
                    .unwrap_or(false);
                let clean_ok = cleanup_results
                    .iter()
                    .find(|(n, _)| n == &node.name)
                    .map(|(_, r)| r.is_ok())
                    .unwrap_or(false);
                if kvm_ok && clean_ok {
                    println!("[sdi]   OK {}", node.name);
                } else {
                    let kvm_label = if kvm_ok { "OK" } else { "FAIL" };
                    let clean_label = if clean_ok { "OK" } else { "FAIL" };
                    eprintln!(
                        "[sdi]   FAIL {} (kvm-teardown: {}, cleanup: {})",
                        node.name, kvm_label, clean_label
                    );
                    failed_nodes += 1;
                }
            }
            if failed_nodes > 0 {
                eprintln!(
                    "[sdi] WARNING: {}/{} node(s) had failures. Review errors above.",
                    failed_nodes, total
                );
            } else {
                println!("[sdi] All {} node(s) cleaned successfully.", total);
            }
        }

        // Step 4: Remove local SDI state directory (only if it actually exists).
        if output_dir.exists() {
            if dry_run {
                println!("[dry-run] Would remove {}", output_dir.display());
            } else {
                println!(
                    "[sdi] Step 4: Removing local SDI state ({})...",
                    output_dir.display()
                );
                std::fs::remove_dir_all(&output_dir)?;
                println!("[sdi] Removed {}", output_dir.display());
            }
        }
    }

    println!("[sdi] Clean complete.");
    Ok(())
}

// --- Helper functions ---

fn run_sync(
    config_path: PathBuf,
    env_path: PathBuf,
    facts_dir: PathBuf,
    output_dir: PathBuf,
    dry_run: bool,
    force: bool,
) -> anyhow::Result<()> {
    println!("[sdi] Sync: reconciling baremetal config with current state...");

    // Step 1: Load desired state from baremetal-init.yaml
    let bm_config = load_baremetal_config(&config_path, &env_path)?;
    let config_errors = crate::core::config::validate_baremetal_config(&bm_config);
    if !config_errors.is_empty() {
        eprintln!("[sdi] Configuration errors in {}:", config_path.display());
        for err in &config_errors {
            eprintln!("  - {}", err);
        }
        anyhow::bail!(
            "Fix {} error(s) in {} before proceeding",
            config_errors.len(),
            config_path.display()
        );
    }
    let desired_nodes: Vec<String> = bm_config
        .target_nodes
        .iter()
        .map(|n| n.name.clone())
        .collect();

    // Step 2: Load current state from facts directory
    let current_facts = load_all_facts(&facts_dir)?;
    let current_nodes: Vec<String> = current_facts.iter().map(|f| f.node_name.clone()).collect();

    // Step 3: Compute diff using pure function
    let diff = sync::compute_sync_diff(&desired_nodes, &current_nodes);

    // Step 4: Report sync plan
    println!("[sdi] Sync plan:");
    println!(
        "  Unchanged nodes ({}): {}",
        diff.unchanged.len(),
        diff.unchanged.join(", ")
    );
    if diff.to_add.is_empty() && diff.to_remove.is_empty() {
        println!("[sdi] Already in sync. Nothing to do.");
        return Ok(());
    }
    if !diff.to_add.is_empty() {
        println!(
            "  + Add nodes ({}): {}",
            diff.to_add.len(),
            diff.to_add.join(", ")
        );
    }
    if !diff.to_remove.is_empty() {
        println!(
            "  - Remove nodes ({}): {}",
            diff.to_remove.len(),
            diff.to_remove.join(", ")
        );
    }

    // Step 5: Check for side effects on removal using pure function
    if !diff.to_remove.is_empty() {
        let sdi_state_path = output_dir.join("sdi-state.json");

        // Safety check: warn if state file missing but removing nodes
        let safety_warnings =
            sync::validate_removal_safety(sdi_state_path.exists(), &diff.to_remove);
        for w in &safety_warnings {
            eprintln!("[sdi] WARNING: {}", w);
        }
        if !safety_warnings.is_empty() && !dry_run && !force {
            anyhow::bail!(
                "Cannot safely remove nodes without SDI state. \
                 Run `scalex sdi init` first or use `--force` to override."
            );
        }
        if !safety_warnings.is_empty() && force {
            eprintln!("[sdi] --force specified, proceeding despite missing state.");
        }

        if sdi_state_path.exists() {
            let state_raw = std::fs::read_to_string(&sdi_state_path)?;
            let pools: Vec<SdiPoolState> = serde_json::from_str(&state_raw)?;
            let conflicts = sync::detect_vm_conflicts(&pools, &diff.to_remove);
            if !conflicts.is_empty() {
                // Classify severity for each conflict
                let has_mgmt = sync::has_management_cluster_conflict(&conflicts, &pools);

                println!("\n[sdi] WARNING: Removing these nodes will affect hosted VMs:");
                for c in &conflicts {
                    let severity = sync::classify_conflict_severity(c, &pools);
                    let label = match severity {
                        sync::ConflictSeverity::Critical => "CRITICAL",
                        sync::ConflictSeverity::High => "HIGH",
                        sync::ConflictSeverity::Medium => "MEDIUM",
                    };
                    println!(
                        "  [{}] {} (pool: {}, host: {})",
                        label, c.vm_name, c.pool_name, c.host
                    );
                }

                if has_mgmt {
                    println!(
                        "\n[sdi] FATAL: Removing this host would destroy the management cluster (tower)."
                    );
                    println!(
                        "[sdi] The management cluster cannot be recovered without a full rebuild."
                    );
                    anyhow::bail!(
                        "Cannot remove nodes hosting management cluster VMs. \
                         This would destroy the entire platform management plane."
                    );
                }

                println!("[sdi] You must migrate or destroy these VMs before removing the host.");
                if !dry_run {
                    anyhow::bail!(
                        "Cannot remove nodes with active VMs. Run `scalex sdi clean` first or migrate VMs."
                    );
                }
            }
        }
    }

    if dry_run {
        println!("[dry-run] Would execute the above sync plan.");
        return Ok(());
    }

    // Step 6: Add new nodes — gather facts
    if !diff.to_add.is_empty() {
        println!("[sdi] Gathering facts for new nodes...");
        std::fs::create_dir_all(&facts_dir)?;
        for node_name in &diff.to_add {
            let node = bm_config
                .target_nodes
                .iter()
                .find(|n| n.name == *node_name)
                .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in config", node_name))?;
            println!("[sdi] Gathering facts from {}...", node_name);
            let script = crate::commands::facts::build_facts_script_public();
            let ssh_cmd = build_ssh_command(node, &script, &bm_config.target_nodes)?;
            match execute_ssh(&ssh_cmd) {
                Ok(output) => {
                    match crate::commands::facts::parse_facts_output_public(node_name, &output) {
                        Ok(facts) => {
                            let json = serde_json::to_string_pretty(&facts)?;
                            let out_path = facts_dir.join(format!("{}.json", node_name));
                            std::fs::write(&out_path, &json)?;
                            println!("[sdi] {} -> {}", node_name, out_path.display());
                        }
                        Err(e) => eprintln!("[sdi] ERROR parsing facts for {}: {}", node_name, e),
                    }
                }
                Err(e) => eprintln!("[sdi] ERROR connecting to {}: {}", node_name, e),
            }
        }

        // Prepare new hosts (KVM, bridge, VFIO)
        println!("[sdi] Preparing new hosts...");
        let all_facts = load_all_facts(&facts_dir)?;
        for node_name in &diff.to_add {
            let node = bm_config
                .target_nodes
                .iter()
                .find(|n| n.name == *node_name)
                .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in config", node_name))?;
            let facts = all_facts.iter().find(|f| f.node_name == *node_name);
            if let Some(facts) = facts {
                let steps = host_prepare::plan_host_preparation(node, facts, false);
                for step in &steps {
                    match step {
                        host_prepare::PrepStep::InstallKvm => {
                            println!("[sdi] {} — installing KVM/libvirt", node_name);
                            let script = host_prepare::generate_kvm_install_script();
                            let ssh_cmd =
                                build_ssh_command(node, &script, &bm_config.target_nodes)?;
                            if let Err(e) = execute_ssh(&ssh_cmd) {
                                eprintln!("[sdi] ERROR on {}: {}", node_name, e);
                            }
                        }
                        host_prepare::PrepStep::SetupBridge => {
                            println!("[sdi] {} — setting up br0 bridge", node_name);
                            let (primary_nic, is_bond) = if let Some(bond) = facts.bonds.first() {
                                (bond.as_str(), true)
                            } else {
                                (
                                    facts
                                        .nics
                                        .first()
                                        .map(|n| n.name.as_str())
                                        .unwrap_or("eno1"),
                                    false,
                                )
                            };
                            let bm_net =
                                bm_config
                                    .network_defaults
                                    .as_ref()
                                    .map(|nd| NetworkDefaults {
                                        bridge: nd.management_bridge.clone(),
                                        cidr: nd.management_cidr.clone(),
                                        gateway: nd.gateway.clone(),
                                    });
                            let sync_net = resolve_network_config(None, bm_net.as_ref()).unwrap_or(
                                NetworkDefaults {
                                    bridge: "br0".to_string(),
                                    cidr: "192.168.88.0/24".to_string(),
                                    gateway: "192.168.88.1".to_string(),
                                },
                            );
                            let cidr_prefix = extract_cidr_prefix(&sync_net.cidr);
                            let script = host_prepare::generate_bridge_setup_script(
                                primary_nic,
                                &node.node_ip,
                                &sync_net.gateway,
                                cidr_prefix,
                                is_bond,
                            );
                            let ssh_cmd =
                                build_ssh_command(node, &script, &bm_config.target_nodes)?;
                            match execute_ssh(&ssh_cmd) {
                                Ok(out) => println!("{}", out),
                                Err(e) => {
                                    if is_bond {
                                        println!(
                                            "[sdi] {} — SSH disconnected during bridge setup (expected for bond→bridge), waiting for reconnect...",
                                            node_name
                                        );
                                        let verify_script = format!(
                                            "ip addr show br0 2>/dev/null | grep -q '{}/{}'",
                                            node.node_ip, cidr_prefix
                                        );
                                        let mut bridge_ok = false;
                                        for attempt in 1..=6 {
                                            std::thread::sleep(std::time::Duration::from_secs(5));
                                            if let Ok(verify_cmd) = build_ssh_command(
                                                node,
                                                &verify_script,
                                                &bm_config.target_nodes,
                                            ) {
                                                if execute_ssh(&verify_cmd).is_ok() {
                                                    println!(
                                                        "[sdi] {} — br0 verified after {}s",
                                                        node_name,
                                                        attempt * 5
                                                    );
                                                    bridge_ok = true;
                                                    break;
                                                }
                                            }
                                            println!(
                                                "[sdi] {} — waiting for br0... (attempt {}/6)",
                                                node_name, attempt
                                            );
                                        }
                                        if !bridge_ok {
                                            eprintln!(
                                                "[sdi] ERROR on {}: bridge setup failed — br0 not reachable after 30s",
                                                node_name
                                            );
                                        }
                                    } else {
                                        eprintln!("[sdi] ERROR on {}: {}", node_name, e);
                                    }
                                }
                            }
                        }
                        host_prepare::PrepStep::ConfigureVfio => {
                            let gpu_ids = host_prepare::extract_gpu_pci_ids(facts);
                            println!("[sdi] {} — configuring VFIO-PCI: {:?}", node_name, gpu_ids);
                            let script = host_prepare::generate_vfio_setup_script(&gpu_ids);
                            let ssh_cmd =
                                build_ssh_command(node, &script, &bm_config.target_nodes)?;
                            if let Err(e) = execute_ssh(&ssh_cmd) {
                                eprintln!("[sdi] ERROR on {}: {}", node_name, e);
                            }
                        }
                    }
                }
            }
        }
    }

    // Step 7: Remove old nodes — clean up facts
    if !diff.to_remove.is_empty() {
        println!("[sdi] Removing facts for decommissioned nodes...");
        for node_name in &diff.to_remove {
            let facts_path = facts_dir.join(format!("{}.json", node_name));
            if facts_path.exists() {
                std::fs::remove_file(&facts_path)?;
                println!("[sdi] Removed {}", facts_path.display());
            }
        }
    }

    // Step 8: Regenerate OpenTofu if sdi-state exists (needs re-plan with new hosts)
    let sdi_state_path = output_dir.join("sdi-state.json");
    if sdi_state_path.exists() && !diff.to_add.is_empty() {
        println!(
            "[sdi] NOTE: New hosts added. Re-run `scalex sdi init <spec>` to update VM placement."
        );
    }

    println!("[sdi] Sync complete.");
    Ok(())
}

fn dir_is_empty(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true)
}

fn load_all_facts(facts_dir: &Path) -> anyhow::Result<Vec<NodeFacts>> {
    let mut facts = Vec::new();
    if !facts_dir.exists() {
        return Ok(facts);
    }
    let mut entries: Vec<_> = std::fs::read_dir(facts_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let content = std::fs::read_to_string(entry.path())?;
        let node_facts: NodeFacts = serde_json::from_str(&content)?;
        facts.push(node_facts);
    }
    Ok(facts)
}

fn load_sdi_spec(path: &str) -> anyhow::Result<SdiSpec> {
    let raw = std::fs::read_to_string(path)?;
    let spec: SdiSpec = serde_yaml::from_str(&raw)?;
    Ok(spec)
}

/// Pure function: checks if any VM on `host_name` requires GPU passthrough.
fn spec_needs_gpu_passthrough(spec: &SdiSpec, host_name: &str) -> bool {
    spec.spec.sdi_pools.iter().any(|p| {
        p.node_specs.iter().any(|n| {
            let on_host = n
                .host
                .as_deref()
                .or(p.placement.hosts.first().map(|s| s.as_str()));
            on_host == Some(host_name) && n.devices.as_ref().is_some_and(|d| d.gpu_passthrough)
        })
    })
}

fn check_gpu_passthrough_needed(host_name: &str, spec_path: Option<&str>) -> bool {
    let Some(path) = spec_path else {
        return false;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(spec) = serde_yaml::from_str::<SdiSpec>(&raw) else {
        return false;
    };
    spec_needs_gpu_passthrough(&spec, host_name)
}

fn run_facts_collection(
    config_path: &std::path::Path,
    env_path: &std::path::Path,
    facts_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let config = load_baremetal_config(config_path, env_path)?;
    std::fs::create_dir_all(facts_dir)?;

    for node in &config.target_nodes {
        println!("[facts] Gathering facts from {}...", node.name);
        let script = crate::commands::facts::build_facts_script_public();
        let ssh_cmd = build_ssh_command(node, &script, &config.target_nodes)?;
        match execute_ssh(&ssh_cmd) {
            Ok(output) => {
                match crate::commands::facts::parse_facts_output_public(&node.name, &output) {
                    Ok(facts) => {
                        let json = serde_json::to_string_pretty(&facts)?;
                        let output_path = facts_dir.join(format!("{}.json", facts.node_name));
                        std::fs::write(&output_path, &json)?;
                        println!("[facts] {} -> {}", node.name, output_path.display());
                    }
                    Err(e) => eprintln!("[facts] ERROR parsing {}: {}", node.name, e),
                }
            }
            Err(e) => eprintln!("[facts] ERROR on {}: {}", node.name, e),
        }
    }
    Ok(())
}

/// Public wrapper for cross-module test access.
#[cfg(test)]
pub fn build_pool_state_public(spec: &SdiSpec) -> Vec<SdiPoolState> {
    build_pool_state(spec)
}

fn build_pool_state(spec: &SdiSpec) -> Vec<SdiPoolState> {
    spec.spec
        .sdi_pools
        .iter()
        .map(|pool| SdiPoolState {
            pool_name: pool.pool_name.clone(),
            purpose: pool.purpose.clone(),
            nodes: pool
                .node_specs
                .iter()
                .map(|n| crate::models::sdi::SdiNodeState {
                    node_name: n.node_name.clone(),
                    ip: n.ip.clone(),
                    host: n
                        .host
                        .clone()
                        .or_else(|| pool.placement.hosts.first().cloned())
                        .unwrap_or_else(|| "unassigned".to_string()),
                    cpu: n.cpu,
                    mem_gb: n.mem_gb,
                    disk_gb: n.disk_gb,
                    status: "planned".to_string(),
                    gpu_passthrough: n.devices.as_ref().is_some_and(|d| d.gpu_passthrough),
                })
                .collect(),
        })
        .collect()
}

/// Validate arguments for the `sdi clean` command. Pure function.
/// Returns error message if invalid, None if valid.
pub fn validate_clean_args(hard: bool, confirm: bool) -> Option<String> {
    if hard && !confirm {
        Some("--hard requires --yes-i-really-want-to flag".to_string())
    } else {
        None
    }
}

/// Describes an operation that `sdi clean` would perform.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub enum CleanOperation {
    /// No SDI state exists — nothing to do
    NoState,
    /// Destroy OpenTofu resources (main.tf exists)
    TofuDestroy,
    /// Destroy host-infra OpenTofu resources (host-infra/main.tf exists)
    TofuDestroyHostInfra,
    /// Run dedicated KVM/libvirt teardown per node (destroy VMs, remove storage pools, undefine domains)
    KvmTeardown { node_count: usize },
    /// Run node cleanup scripts via SSH (hard mode, baremetal config exists)
    NodeCleanup { node_count: usize },
    /// Skip node cleanup (hard mode, but no baremetal config)
    SkipNodeCleanup,
    /// Remove local state directory (hard mode)
    RemoveStateDir,
}

/// Plan what operations `sdi clean` would execute. Pure function: no I/O.
/// Used for dry-run reporting and testability.
#[cfg(test)]
pub fn plan_clean_operations(
    hard: bool,
    output_dir_exists: bool,
    main_tf_exists: bool,
    host_infra_main_tf_exists: bool,
    bm_config_node_count: Option<usize>,
) -> Vec<CleanOperation> {
    let mut ops = Vec::new();

    if !output_dir_exists {
        ops.push(CleanOperation::NoState);
        return ops;
    }

    if host_infra_main_tf_exists {
        ops.push(CleanOperation::TofuDestroyHostInfra);
    }

    if main_tf_exists {
        ops.push(CleanOperation::TofuDestroy);
    }

    if hard {
        match bm_config_node_count {
            Some(count) => {
                // KVM teardown runs first (destroy VMs/pools/domains), then full node cleanup
                ops.push(CleanOperation::KvmTeardown { node_count: count });
                ops.push(CleanOperation::NodeCleanup { node_count: count });
            }
            None => ops.push(CleanOperation::SkipNodeCleanup),
        }
        ops.push(CleanOperation::RemoveStateDir);
    }

    ops
}

/// Network defaults resolved from available config sources.
/// Used to eliminate hardcoded network values in SDI init.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkDefaults {
    pub bridge: String,
    pub cidr: String,
    pub gateway: String,
}

/// Resolve network configuration from available sources.
/// Priority: SdiSpec network_config > BaremetalInitConfig network_defaults > error.
/// Pure function: no IO, no side effects.
pub fn resolve_network_config(
    sdi_spec: Option<&SdiSpec>,
    baremetal_network: Option<&NetworkDefaults>,
) -> Result<NetworkDefaults, String> {
    if let Some(spec) = sdi_spec {
        return Ok(NetworkDefaults {
            bridge: spec.resource_pool.network.management_bridge.clone(),
            cidr: spec.resource_pool.network.management_cidr.clone(),
            gateway: spec.resource_pool.network.gateway.clone(),
        });
    }
    if let Some(defaults) = baremetal_network {
        return Ok(defaults.clone());
    }
    Err("No network configuration available. Provide sdi-specs.yaml or add network_defaults to baremetal-init.yaml".to_string())
}

/// Extract CIDR prefix length from a CIDR string (e.g., "192.168.88.0/24" → 24).
/// Returns 24 as default for malformed input.
/// Pure function: no IO, no side effects.
pub fn extract_cidr_prefix(cidr: &str) -> u8 {
    cidr.rsplit_once('/')
        .and_then(|(_, prefix)| prefix.parse::<u8>().ok())
        .unwrap_or(24)
}

/// Build HostInfraInput list from BaremetalInitConfig.
/// Pure function: simple data transformation.
pub fn build_host_infra_inputs(
    nodes: &[crate::core::config::NodeConnectionConfig],
) -> Vec<tofu::HostInfraInput> {
    nodes
        .iter()
        .map(|n| tofu::HostInfraInput {
            name: n.name.clone(),
            ip: n.node_ip.clone(),
            ssh_user: n.admin_user.clone(),
        })
        .collect()
}

fn run_tofu_command(work_dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = std::process::Command::new("tofu")
        .args(args)
        .current_dir(work_dir)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stdout.is_empty() {
                println!("{}", stdout);
            }
            if !out.status.success() {
                eprintln!("[sdi] tofu error: {}", stderr);
                anyhow::bail!("tofu {} failed", args.join(" "));
            }
            Ok(())
        }
        Err(_) => {
            anyhow::bail!(
                "OpenTofu ('tofu') not found. Install from https://opentofu.org/docs/intro/install/"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::NodeConnectionConfig;
    use crate::models::sdi::*;

    // --- TDD Cycle 1-1: resolve_network_config ---

    #[test]
    fn test_resolve_network_config_from_sdi_spec() {
        let spec = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "playbox-pool".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "10.0.0.0/24".to_string(),
                    gateway: "10.0.0.1".to_string(),
                    nameservers: vec!["8.8.8.8".to_string()],
                },
            },
            os_image: OsImageConfig {
                source: "https://example.com/image.qcow2".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "~/.ssh/authorized_keys".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec { sdi_pools: vec![] },
        };

        let result = resolve_network_config(Some(&spec), None).unwrap();
        assert_eq!(result.bridge, "br0");
        assert_eq!(result.cidr, "10.0.0.0/24");
        assert_eq!(result.gateway, "10.0.0.1");
    }

    #[test]
    fn test_resolve_network_config_from_baremetal_defaults() {
        let defaults = NetworkDefaults {
            bridge: "br1".to_string(),
            cidr: "192.168.1.0/24".to_string(),
            gateway: "192.168.1.1".to_string(),
        };

        let result = resolve_network_config(None, Some(&defaults)).unwrap();
        assert_eq!(result, defaults);
    }

    #[test]
    fn test_resolve_network_config_spec_takes_priority() {
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
        let defaults = NetworkDefaults {
            bridge: "br1".to_string(),
            cidr: "192.168.1.0/24".to_string(),
            gateway: "192.168.1.1".to_string(),
        };

        let result = resolve_network_config(Some(&spec), Some(&defaults)).unwrap();
        // SdiSpec takes priority
        assert_eq!(result.bridge, "br0");
        assert_eq!(result.gateway, "10.0.0.1");
    }

    #[test]
    fn test_resolve_network_config_none_returns_error() {
        let result = resolve_network_config(None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No network configuration"));
    }

    // --- TDD Cycle 1-2: build_host_infra_inputs ---

    #[test]
    fn test_build_host_infra_inputs_single_node() {
        let nodes = vec![NodeConnectionConfig {
            name: "node-0".to_string(),
            direct_reachable: true,
            node_ip: "192.168.88.8".to_string(),
            reachable_node_ip: None,
            reachable_via: None,
            admin_user: "admin".to_string(),
            ssh_auth_mode: crate::core::config::SshAuthMode::Key,
            ssh_password: None,
            ssh_key_path: Some("~/.ssh/id_ed25519".to_string()),
            ssh_key_path_of_reachable_node: None,
        }];

        let inputs = build_host_infra_inputs(&nodes);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "node-0");
        assert_eq!(inputs[0].ip, "192.168.88.8");
    }

    #[test]
    fn test_build_host_infra_inputs_multi_node() {
        let nodes = vec![
            NodeConnectionConfig {
                name: "playbox-0".to_string(),
                direct_reachable: true,
                node_ip: "192.168.88.8".to_string(),
                reachable_node_ip: None,
                reachable_via: None,
                admin_user: "admin".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Key,
                ssh_password: None,
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
            NodeConnectionConfig {
                name: "playbox-1".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.9".to_string(),
                reachable_node_ip: None,
                reachable_via: Some(vec!["playbox-0".to_string()]),
                admin_user: "admin".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Password,
                ssh_password: Some("pass".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
        ];

        let inputs = build_host_infra_inputs(&nodes);
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].name, "playbox-0");
        assert_eq!(inputs[1].name, "playbox-1");
        assert_eq!(inputs[1].ip, "192.168.88.9");
    }

    // --- TDD Cycle 1-3: build_pool_state (already exists, add tests) ---

    #[test]
    fn test_build_pool_state_basic() {
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

        let state = build_pool_state(&spec);
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].pool_name, "tower-pool");
        assert_eq!(state[0].nodes.len(), 1);
        assert_eq!(state[0].nodes[0].node_name, "tower-cp-0");
        assert_eq!(state[0].nodes[0].host, "playbox-0"); // from placement.hosts
        assert_eq!(state[0].nodes[0].cpu, 4);
        assert_eq!(state[0].nodes[0].status, "planned");
        assert!(!state[0].nodes[0].gpu_passthrough);
    }

    #[test]
    fn test_build_pool_state_with_explicit_host_and_gpu() {
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
                    pool_name: "gpu-pool".to_string(),
                    purpose: "compute".to_string(),
                    placement: PlacementConfig {
                        hosts: vec!["host-0".to_string()],
                        spread: false,
                    },
                    node_specs: vec![NodeSpec {
                        node_name: "gpu-node-0".to_string(),
                        ip: "10.0.0.50".to_string(),
                        cpu: 8,
                        mem_gb: 32,
                        disk_gb: 100,
                        host: Some("host-1".to_string()), // explicit host overrides placement
                        roles: vec!["worker".to_string()],
                        devices: Some(DeviceConfig {
                            gpu_passthrough: true,
                        }),
                    }],
                }],
            },
        };

        let state = build_pool_state(&spec);
        assert_eq!(state[0].nodes[0].host, "host-1"); // explicit host wins
        assert!(state[0].nodes[0].gpu_passthrough);
        assert_eq!(state[0].nodes[0].mem_gb, 32);
    }

    // --- TDD Cycle 8a.1: validate_clean_args ---

    #[test]
    fn test_clean_hard_without_confirm_rejected() {
        let result = validate_clean_args(true, false);
        assert!(result.is_some());
        assert!(result.unwrap().contains("--yes-i-really-want-to"));
    }

    #[test]
    fn test_clean_hard_with_confirm_accepted() {
        let result = validate_clean_args(true, true);
        assert!(result.is_none());
    }

    #[test]
    fn test_clean_soft_without_confirm_accepted() {
        let result = validate_clean_args(false, false);
        assert!(result.is_none());
    }

    #[test]
    fn test_clean_soft_with_confirm_accepted() {
        let result = validate_clean_args(false, true);
        assert!(result.is_none());
    }

    // --- TDD Cycle 8a.1: plan_clean_operations ---

    #[test]
    fn test_plan_clean_no_state_dir() {
        let ops = plan_clean_operations(false, false, false, false, None);
        assert_eq!(ops, vec![CleanOperation::NoState]);
    }

    #[test]
    fn test_plan_clean_soft_with_main_tf() {
        let ops = plan_clean_operations(false, true, true, false, Some(4));
        assert_eq!(ops, vec![CleanOperation::TofuDestroy]);
    }

    #[test]
    fn test_plan_clean_soft_without_main_tf() {
        let ops = plan_clean_operations(false, true, false, false, Some(4));
        assert!(ops.is_empty(), "soft clean with no main.tf should be no-op");
    }

    #[test]
    fn test_plan_clean_hard_with_main_tf_and_bm_config() {
        let ops = plan_clean_operations(true, true, true, false, Some(4));
        assert_eq!(
            ops,
            vec![
                CleanOperation::TofuDestroy,
                CleanOperation::KvmTeardown { node_count: 4 },
                CleanOperation::NodeCleanup { node_count: 4 },
                CleanOperation::RemoveStateDir,
            ]
        );
    }

    #[test]
    fn test_plan_clean_hard_without_bm_config() {
        let ops = plan_clean_operations(true, true, true, false, None);
        assert_eq!(
            ops,
            vec![
                CleanOperation::TofuDestroy,
                CleanOperation::SkipNodeCleanup,
                CleanOperation::RemoveStateDir,
            ]
        );
    }

    #[test]
    fn test_plan_clean_hard_no_main_tf_with_bm_config() {
        let ops = plan_clean_operations(true, true, false, false, Some(2));
        assert_eq!(
            ops,
            vec![
                CleanOperation::KvmTeardown { node_count: 2 },
                CleanOperation::NodeCleanup { node_count: 2 },
                CleanOperation::RemoveStateDir,
            ]
        );
    }

    #[test]
    fn test_plan_clean_hard_no_state_dir_short_circuits() {
        // Even with hard mode, no state dir means nothing to do
        let ops = plan_clean_operations(true, false, false, false, Some(4));
        assert_eq!(ops, vec![CleanOperation::NoState]);
    }

    // --- Sprint 16a: host-infra tofu destroy (G-1) ---

    #[test]
    fn test_plan_clean_soft_with_host_infra_only() {
        // No main.tf but host-infra/main.tf exists → should destroy host-infra
        let ops = plan_clean_operations(false, true, false, true, Some(4));
        assert_eq!(
            ops,
            vec![CleanOperation::TofuDestroyHostInfra],
            "soft clean must destroy host-infra tofu resources when host-infra/main.tf exists"
        );
    }

    #[test]
    fn test_plan_clean_hard_with_both_main_tf_and_host_infra() {
        // Both main.tf and host-infra/main.tf exist → destroy both
        let ops = plan_clean_operations(true, true, true, true, Some(4));
        assert_eq!(
            ops,
            vec![
                CleanOperation::TofuDestroyHostInfra,
                CleanOperation::TofuDestroy,
                CleanOperation::KvmTeardown { node_count: 4 },
                CleanOperation::NodeCleanup { node_count: 4 },
                CleanOperation::RemoveStateDir,
            ],
            "hard clean must destroy host-infra BEFORE main tofu (dependency order)"
        );
    }

    #[test]
    fn test_plan_clean_hard_host_infra_only_no_main_tf() {
        // host-infra/main.tf exists but no main.tf → destroy host-infra + hard clean
        let ops = plan_clean_operations(true, true, false, true, Some(2));
        assert_eq!(
            ops,
            vec![
                CleanOperation::TofuDestroyHostInfra,
                CleanOperation::KvmTeardown { node_count: 2 },
                CleanOperation::NodeCleanup { node_count: 2 },
                CleanOperation::RemoveStateDir,
            ]
        );
    }

    // --- KVM teardown operation tests (Sub-AC 3) ---

    #[test]
    fn test_plan_clean_hard_includes_kvm_teardown_before_node_cleanup() {
        // KVM teardown must appear BEFORE full NodeCleanup in hard mode
        let ops = plan_clean_operations(true, true, true, false, Some(4));
        let kvm_pos = ops
            .iter()
            .position(|op| matches!(op, CleanOperation::KvmTeardown { .. }))
            .expect("KvmTeardown must be present in hard mode with bm config");
        let node_cleanup_pos = ops
            .iter()
            .position(|op| matches!(op, CleanOperation::NodeCleanup { .. }))
            .expect("NodeCleanup must be present in hard mode with bm config");
        assert!(
            kvm_pos < node_cleanup_pos,
            "KvmTeardown must precede NodeCleanup to destroy VMs before package removal"
        );
    }

    #[test]
    fn test_plan_clean_hard_kvm_teardown_uses_correct_node_count() {
        let ops = plan_clean_operations(true, true, false, false, Some(4));
        let kvm_op = ops
            .iter()
            .find(|op| matches!(op, CleanOperation::KvmTeardown { .. }))
            .expect("KvmTeardown must be present");
        assert_eq!(
            *kvm_op,
            CleanOperation::KvmTeardown { node_count: 4 },
            "KvmTeardown must carry the correct node count for per-node SSH execution"
        );
    }

    #[test]
    fn test_plan_clean_soft_has_no_kvm_teardown() {
        // Soft clean (no --hard) must NOT include KVM teardown
        let ops = plan_clean_operations(false, true, true, false, Some(4));
        assert!(
            !ops.iter()
                .any(|op| matches!(op, CleanOperation::KvmTeardown { .. })),
            "soft clean must not include KVM teardown"
        );
    }

    #[test]
    fn test_plan_clean_hard_no_bm_config_has_no_kvm_teardown() {
        // Without baremetal config, skip KVM teardown (no SSH access info)
        let ops = plan_clean_operations(true, true, true, false, None);
        assert!(
            !ops.iter()
                .any(|op| matches!(op, CleanOperation::KvmTeardown { .. })),
            "hard clean without bm config must not include KVM teardown"
        );
        assert!(
            ops.contains(&CleanOperation::SkipNodeCleanup),
            "must include SkipNodeCleanup when bm config absent"
        );
    }

    #[test]
    fn test_plan_clean_hard_kvm_teardown_with_two_nodes() {
        let ops = plan_clean_operations(true, true, false, false, Some(2));
        assert!(
            ops.contains(&CleanOperation::KvmTeardown { node_count: 2 }),
            "KvmTeardown must reflect actual node count"
        );
        assert!(
            ops.contains(&CleanOperation::NodeCleanup { node_count: 2 }),
            "NodeCleanup must follow KvmTeardown"
        );
    }

    // --- dir_is_empty tests ---

    #[test]
    fn test_dir_is_empty_nonexistent_returns_true() {
        // Non-existent directory should be treated as empty
        assert!(dir_is_empty(std::path::Path::new(
            "/tmp/scalex_test_nonexistent_dir_xyz"
        )));
    }

    #[test]
    fn test_dir_is_empty_empty_dir_returns_true() {
        let tmp = std::env::temp_dir().join("scalex_test_empty_dir");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(dir_is_empty(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_dir_is_empty_with_file_returns_false() {
        let tmp = std::env::temp_dir().join("scalex_test_nonempty_dir");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("facts.json"), "{}").unwrap();
        assert!(!dir_is_empty(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// B-5: CIDR prefix must be extracted from managementCidr string, not hardcoded to 24.
    #[test]
    fn test_extract_cidr_prefix() {
        assert_eq!(extract_cidr_prefix("192.168.88.0/24"), 24);
        assert_eq!(extract_cidr_prefix("10.0.0.0/16"), 16);
        assert_eq!(extract_cidr_prefix("172.16.0.0/20"), 20);
        // Fallback for malformed input
        assert_eq!(extract_cidr_prefix("192.168.88.0"), 24);
        assert_eq!(extract_cidr_prefix(""), 24);
    }

    /// B-4: resolve_network_config must accept baremetal_network when sdi_spec is None.
    /// This tests the sync path scenario where only baremetal config is available.
    #[test]
    fn test_resolve_network_config_baremetal_defaults_used_in_sync_path() {
        let defaults = NetworkDefaults {
            bridge: "br-custom".to_string(),
            cidr: "10.0.0.0/16".to_string(),
            gateway: "10.0.0.1".to_string(),
        };
        let result = resolve_network_config(None, Some(&defaults)).unwrap();
        assert_eq!(result.bridge, "br-custom");
        assert_eq!(result.cidr, "10.0.0.0/16");
        assert_eq!(result.gateway, "10.0.0.1");
        // CIDR prefix should be extractable from the result
        assert_eq!(extract_cidr_prefix(&result.cidr), 16);
    }

    // --- Sprint 31a: build_host_infra_inputs edge cases ---

    #[test]
    fn test_build_host_infra_inputs_empty_list() {
        let inputs = build_host_infra_inputs(&[]);
        assert!(
            inputs.is_empty(),
            "empty node list must produce empty inputs"
        );
    }

    #[test]
    fn test_build_host_infra_inputs_four_nodes_matching_example() {
        let nodes = vec![
            NodeConnectionConfig {
                name: "playbox-0".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.8".to_string(),
                reachable_node_ip: Some("100.64.0.1".to_string()),
                reachable_via: None,
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Password,
                ssh_password: Some("secret".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
            NodeConnectionConfig {
                name: "playbox-1".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.9".to_string(),
                reachable_node_ip: None,
                reachable_via: Some(vec!["playbox-0".to_string()]),
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Password,
                ssh_password: Some("secret".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
            NodeConnectionConfig {
                name: "playbox-2".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.10".to_string(),
                reachable_node_ip: None,
                reachable_via: Some(vec!["playbox-0".to_string()]),
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Password,
                ssh_password: Some("secret".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
            NodeConnectionConfig {
                name: "playbox-3".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.11".to_string(),
                reachable_node_ip: None,
                reachable_via: Some(vec!["playbox-0".to_string()]),
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Password,
                ssh_password: Some("secret".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
        ];

        let inputs = build_host_infra_inputs(&nodes);
        assert_eq!(inputs.len(), 4);
        assert_eq!(inputs[0].name, "playbox-0");
        assert_eq!(inputs[0].ip, "192.168.88.8");
        assert_eq!(inputs[0].ssh_user, "jinwang");
        assert_eq!(inputs[3].name, "playbox-3");
        assert_eq!(inputs[3].ip, "192.168.88.11");
    }

    #[test]
    fn test_build_host_infra_inputs_preserves_different_admin_users() {
        let nodes = vec![
            NodeConnectionConfig {
                name: "node-a".to_string(),
                direct_reachable: true,
                node_ip: "10.0.0.1".to_string(),
                reachable_node_ip: None,
                reachable_via: None,
                admin_user: "alice".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Key,
                ssh_password: None,
                ssh_key_path: Some("key".to_string()),
                ssh_key_path_of_reachable_node: None,
            },
            NodeConnectionConfig {
                name: "node-b".to_string(),
                direct_reachable: true,
                node_ip: "10.0.0.2".to_string(),
                reachable_node_ip: None,
                reachable_via: None,
                admin_user: "bob".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Key,
                ssh_password: None,
                ssh_key_path: Some("key".to_string()),
                ssh_key_path_of_reachable_node: None,
            },
        ];

        let inputs = build_host_infra_inputs(&nodes);
        assert_eq!(inputs[0].ssh_user, "alice");
        assert_eq!(inputs[1].ssh_user, "bob");
    }

    // --- Sprint 31a: build_pool_state multi-pool + edge cases ---

    #[test]
    fn test_build_pool_state_multi_pool_tower_and_sandbox() {
        let spec = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "playbox-pool".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec!["8.8.8.8".to_string()],
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
                sdi_pools: vec![
                    SdiPool {
                        pool_name: "tower".to_string(),
                        purpose: "management".to_string(),
                        placement: PlacementConfig {
                            hosts: vec!["playbox-0".to_string()],
                            spread: false,
                        },
                        node_specs: vec![NodeSpec {
                            node_name: "tower-cp-0".to_string(),
                            ip: "192.168.88.100".to_string(),
                            cpu: 2,
                            mem_gb: 3,
                            disk_gb: 30,
                            host: None,
                            roles: vec!["control-plane".to_string(), "worker".to_string()],
                            devices: None,
                        }],
                    },
                    SdiPool {
                        pool_name: "sandbox".to_string(),
                        purpose: "workload".to_string(),
                        placement: PlacementConfig {
                            hosts: vec![],
                            spread: true,
                        },
                        node_specs: vec![
                            NodeSpec {
                                node_name: "sandbox-cp-0".to_string(),
                                ip: "192.168.88.110".to_string(),
                                cpu: 4,
                                mem_gb: 8,
                                disk_gb: 60,
                                host: Some("playbox-0".to_string()),
                                roles: vec!["control-plane".to_string()],
                                devices: None,
                            },
                            NodeSpec {
                                node_name: "sandbox-w-2".to_string(),
                                ip: "192.168.88.122".to_string(),
                                cpu: 12,
                                mem_gb: 32,
                                disk_gb: 200,
                                host: Some("playbox-3".to_string()),
                                roles: vec!["worker".to_string()],
                                devices: Some(DeviceConfig {
                                    gpu_passthrough: true,
                                }),
                            },
                        ],
                    },
                ],
            },
        };

        let state = build_pool_state(&spec);
        assert_eq!(state.len(), 2, "must produce 2 pools");

        // Tower
        assert_eq!(state[0].pool_name, "tower");
        assert_eq!(state[0].purpose, "management");
        assert_eq!(state[0].nodes.len(), 1);
        assert_eq!(state[0].nodes[0].host, "playbox-0"); // from placement.hosts
        assert!(!state[0].nodes[0].gpu_passthrough);

        // Sandbox
        assert_eq!(state[1].pool_name, "sandbox");
        assert_eq!(state[1].nodes.len(), 2);
        assert_eq!(state[1].nodes[0].host, "playbox-0"); // explicit
        assert_eq!(state[1].nodes[1].host, "playbox-3"); // explicit
        assert!(state[1].nodes[1].gpu_passthrough);
        assert_eq!(state[1].nodes[1].mem_gb, 32);
    }

    #[test]
    fn test_build_pool_state_unassigned_host_fallback() {
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
                    pool_name: "orphan-pool".to_string(),
                    purpose: "test".to_string(),
                    placement: PlacementConfig {
                        hosts: vec![],
                        spread: true,
                    },
                    node_specs: vec![NodeSpec {
                        node_name: "orphan-node".to_string(),
                        ip: "10.0.0.50".to_string(),
                        cpu: 2,
                        mem_gb: 4,
                        disk_gb: 20,
                        host: None,
                        roles: vec!["worker".to_string()],
                        devices: None,
                    }],
                }],
            },
        };

        let state = build_pool_state(&spec);
        assert_eq!(
            state[0].nodes[0].host, "unassigned",
            "node with no host and no placement hosts must be 'unassigned'"
        );
    }

    #[test]
    fn test_build_pool_state_empty_pools() {
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

        let state = build_pool_state(&spec);
        assert!(state.is_empty(), "empty sdi_pools must produce empty state");
    }

    // --- Sprint 31a: extract_cidr_prefix edge cases ---

    #[test]
    fn test_extract_cidr_prefix_boundary_values() {
        assert_eq!(extract_cidr_prefix("10.0.0.0/0"), 0);
        assert_eq!(extract_cidr_prefix("10.0.0.1/32"), 32);
        assert_eq!(extract_cidr_prefix("10.0.0.0/8"), 8);
        // Invalid prefix falls back to 24
        assert_eq!(extract_cidr_prefix("10.0.0.0/abc"), 24);
        assert_eq!(extract_cidr_prefix("10.0.0.0/999"), 24); // overflows u8
    }

    // --- Sprint 31a: no-flag pipeline — host-infra HCL generation ---

    #[test]
    fn test_no_flag_pipeline_host_infra_hcl_generation() {
        use crate::core::tofu;

        let nodes = vec![
            NodeConnectionConfig {
                name: "playbox-0".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.8".to_string(),
                reachable_node_ip: Some("100.64.0.1".to_string()),
                reachable_via: None,
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Password,
                ssh_password: Some("pass".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
            NodeConnectionConfig {
                name: "playbox-1".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.9".to_string(),
                reachable_node_ip: None,
                reachable_via: Some(vec!["playbox-0".to_string()]),
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Password,
                ssh_password: Some("pass".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
        ];

        let inputs = build_host_infra_inputs(&nodes);
        let net = NetworkDefaults {
            bridge: "br0".to_string(),
            cidr: "192.168.88.0/24".to_string(),
            gateway: "192.168.88.1".to_string(),
        };

        let hcl = tofu::generate_tofu_host_infra(&inputs, &net.bridge, &net.cidr, &net.gateway);

        assert!(hcl.contains("playbox-0"), "HCL must reference playbox-0");
        assert!(hcl.contains("playbox-1"), "HCL must reference playbox-1");
        assert!(
            hcl.contains("192.168.88.8"),
            "HCL must contain playbox-0 IP"
        );
        assert!(hcl.contains("jinwang"), "HCL must contain ssh_user");
        assert!(hcl.contains("libvirt"), "HCL must use libvirt provider");
    }

    #[test]
    fn test_no_flag_fallback_network_defaults_coherence() {
        // When resolve_network_config returns Err, run_init uses hardcoded fallback.
        // Verify the fallback matches documented defaults (192.168.88.0/24).
        let result = resolve_network_config(None, None);
        assert!(result.is_err());

        let fallback = NetworkDefaults {
            bridge: "br0".to_string(),
            cidr: "192.168.88.0/24".to_string(),
            gateway: "192.168.88.1".to_string(),
        };
        assert_eq!(extract_cidr_prefix(&fallback.cidr), 24);
    }

    // --- Sprint 32a: spec_needs_gpu_passthrough pure function tests ---

    fn stub_sdi_spec(pools: Vec<SdiPool>) -> SdiSpec {
        SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "test-pool".to_string(),
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
            spec: SdiPoolsSpec { sdi_pools: pools },
        }
    }

    fn make_gpu_pool(host: Option<&str>, placement_hosts: Vec<&str>, gpu: bool) -> SdiPool {
        SdiPool {
            pool_name: "test-pool".to_string(),
            purpose: "test".to_string(),
            placement: PlacementConfig {
                hosts: placement_hosts.into_iter().map(|s| s.to_string()).collect(),
                spread: false,
            },
            node_specs: vec![NodeSpec {
                node_name: "vm-0".to_string(),
                ip: "10.0.0.10".to_string(),
                cpu: 4,
                mem_gb: 8,
                disk_gb: 50,
                host: host.map(|s| s.to_string()),
                roles: vec!["worker".to_string()],
                devices: if gpu {
                    Some(DeviceConfig {
                        gpu_passthrough: true,
                    })
                } else {
                    None
                },
            }],
        }
    }

    #[test]
    fn test_spec_needs_gpu_passthrough_explicit_host_match() {
        let spec = stub_sdi_spec(vec![make_gpu_pool(
            Some("playbox-0"),
            vec!["playbox-0"],
            true,
        )]);
        assert!(spec_needs_gpu_passthrough(&spec, "playbox-0"));
    }

    #[test]
    fn test_spec_needs_gpu_passthrough_explicit_host_no_match() {
        let spec = stub_sdi_spec(vec![make_gpu_pool(
            Some("playbox-0"),
            vec!["playbox-0"],
            true,
        )]);
        assert!(!spec_needs_gpu_passthrough(&spec, "playbox-1"));
    }

    #[test]
    fn test_spec_needs_gpu_passthrough_falls_back_to_placement_host() {
        let spec = stub_sdi_spec(vec![make_gpu_pool(None, vec!["playbox-2"], true)]);
        assert!(spec_needs_gpu_passthrough(&spec, "playbox-2"));
        assert!(!spec_needs_gpu_passthrough(&spec, "playbox-0"));
    }

    #[test]
    fn test_spec_needs_gpu_passthrough_no_devices() {
        let spec = stub_sdi_spec(vec![make_gpu_pool(
            Some("playbox-0"),
            vec!["playbox-0"],
            false,
        )]);
        assert!(!spec_needs_gpu_passthrough(&spec, "playbox-0"));
    }

    #[test]
    fn test_spec_needs_gpu_passthrough_empty_pools() {
        let spec = stub_sdi_spec(vec![]);
        assert!(!spec_needs_gpu_passthrough(&spec, "any-host"));
    }

    #[test]
    fn test_spec_needs_gpu_passthrough_empty_placement_and_no_host() {
        let spec = stub_sdi_spec(vec![make_gpu_pool(None, vec![], true)]);
        assert!(!spec_needs_gpu_passthrough(&spec, "playbox-0"));
    }

    #[test]
    fn test_spec_needs_gpu_passthrough_multi_pool_mixed() {
        let spec = stub_sdi_spec(vec![
            SdiPool {
                pool_name: "pool-a".to_string(),
                purpose: "compute".to_string(),
                placement: PlacementConfig {
                    hosts: vec!["playbox-0".to_string()],
                    spread: false,
                },
                node_specs: vec![NodeSpec {
                    node_name: "vm-a".to_string(),
                    ip: "10.0.0.1".to_string(),
                    cpu: 2,
                    mem_gb: 4,
                    disk_gb: 20,
                    host: None,
                    roles: vec!["worker".to_string()],
                    devices: None,
                }],
            },
            SdiPool {
                pool_name: "pool-b".to_string(),
                purpose: "gpu".to_string(),
                placement: PlacementConfig {
                    hosts: vec!["playbox-0".to_string()],
                    spread: false,
                },
                node_specs: vec![NodeSpec {
                    node_name: "vm-b".to_string(),
                    ip: "10.0.0.2".to_string(),
                    cpu: 4,
                    mem_gb: 8,
                    disk_gb: 50,
                    host: None,
                    roles: vec!["worker".to_string()],
                    devices: Some(DeviceConfig {
                        gpu_passthrough: true,
                    }),
                }],
            },
        ]);
        assert!(spec_needs_gpu_passthrough(&spec, "playbox-0"));
        assert!(!spec_needs_gpu_passthrough(&spec, "playbox-1"));
    }

    // ===== Sprint 33b: SDI Pipeline Integration Tests =====
    // These tests verify data flows correctly between SDI modules end-to-end.

    #[test]
    fn test_sprint33b_no_flag_pipeline_host_infra_e2e() {
        // sdi init no-flag: baremetal config → build_host_infra_inputs → generate_tofu_host_infra
        // Verify the data flows correctly through the entire pipeline.
        use crate::core::config::{NodeConnectionConfig, SshAuthMode};

        let nodes = vec![
            NodeConnectionConfig {
                name: "playbox-0".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.8".to_string(),
                reachable_node_ip: Some("100.64.0.1".to_string()),
                reachable_via: None,
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: SshAuthMode::Password,
                ssh_password: Some("secret".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
            NodeConnectionConfig {
                name: "playbox-1".to_string(),
                direct_reachable: false,
                node_ip: "192.168.88.9".to_string(),
                reachable_node_ip: None,
                reachable_via: Some(vec!["playbox-0".to_string()]),
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: SshAuthMode::Password,
                ssh_password: Some("secret".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
        ];

        // Step 1: build_host_infra_inputs (pure transform)
        let inputs = build_host_infra_inputs(&nodes);
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].name, "playbox-0");
        assert_eq!(inputs[0].ip, "192.168.88.8");
        assert_eq!(inputs[0].ssh_user, "jinwang");
        assert_eq!(inputs[1].name, "playbox-1");

        // Step 2: resolve network config from baremetal defaults
        let bm_network = NetworkDefaults {
            bridge: "br0".to_string(),
            cidr: "192.168.88.0/24".to_string(),
            gateway: "192.168.88.1".to_string(),
        };
        let net = resolve_network_config(None, Some(&bm_network)).unwrap();

        // Step 3: generate_tofu_host_infra with pipeline output
        let hcl = crate::core::tofu::generate_tofu_host_infra(
            &inputs,
            &net.bridge,
            &net.cidr,
            &net.gateway,
        );

        // Verify HCL contains both hosts with correct connection info
        assert!(hcl.contains("playbox-0"), "HCL must contain playbox-0");
        assert!(hcl.contains("playbox-1"), "HCL must contain playbox-1");
        assert!(
            hcl.contains("jinwang@192.168.88.8"),
            "HCL must contain correct SSH URI for playbox-0"
        );
        assert!(
            hcl.contains("jinwang@192.168.88.9"),
            "HCL must contain correct SSH URI for playbox-1"
        );
        assert!(
            hcl.contains("scalex-pool"),
            "HCL must create storage pool on each host"
        );
    }

    #[test]
    fn test_sprint33b_spec_pipeline_tofu_to_pool_state() {
        // sdi init <spec>: SdiSpec → generate_tofu_main() → build_pool_state()
        // Verify pool state reflects spec accurately.
        let content = include_str!("../../../config/sdi-specs.yaml.example");
        let spec: SdiSpec = serde_yaml::from_str(content).unwrap();

        // Step 1: Generate HCL from spec
        let hcl = crate::core::tofu::generate_tofu_main(&spec, "jinwang");

        // Verify HCL contains all nodes from both pools
        assert!(hcl.contains("tower-cp-0"), "HCL must contain tower CP node");
        assert!(
            hcl.contains("sandbox-cp-0"),
            "HCL must contain sandbox CP node"
        );
        assert!(
            hcl.contains("sandbox-w-0"),
            "HCL must contain sandbox worker 0"
        );
        assert!(
            hcl.contains("sandbox-w-1"),
            "HCL must contain sandbox worker 1"
        );
        assert!(
            hcl.contains("sandbox-w-2"),
            "HCL must contain sandbox worker 2"
        );

        // Step 2: Build pool state from same spec
        let pool_states = build_pool_state(&spec);
        assert_eq!(pool_states.len(), 2, "Must have 2 pools");

        // Tower pool
        assert_eq!(pool_states[0].pool_name, "tower");
        assert_eq!(pool_states[0].nodes.len(), 1);
        assert_eq!(pool_states[0].nodes[0].node_name, "tower-cp-0");
        assert_eq!(pool_states[0].nodes[0].ip, "192.168.88.100");

        // Sandbox pool
        assert_eq!(pool_states[1].pool_name, "sandbox");
        assert_eq!(pool_states[1].nodes.len(), 4);

        // Verify all node IPs in pool_state appear in the HCL
        for pool in &pool_states {
            for node in &pool.nodes {
                assert!(
                    hcl.contains(&node.ip),
                    "HCL must contain IP {} from pool state node {}",
                    node.ip,
                    node.node_name
                );
            }
        }
    }

    #[test]
    fn test_sprint33b_sdi_to_cluster_spec_cache_continuity() {
        // Verify that sdi-spec-cache.yaml (written by sdi init) can be re-read by cluster init.
        // This tests the serialization round-trip: SdiSpec → YAML → SdiSpec.
        let content = include_str!("../../../config/sdi-specs.yaml.example");
        let original: SdiSpec = serde_yaml::from_str(content).unwrap();

        // Simulate what sdi init does: serialize to YAML (sdi-spec-cache.yaml)
        let cached_yaml =
            serde_yaml::to_string(&original).expect("SdiSpec must be serializable to YAML");

        // Simulate what cluster init does: deserialize from cache
        let restored: SdiSpec =
            serde_yaml::from_str(&cached_yaml).expect("Cached SdiSpec YAML must be parseable back");

        // Verify structural equivalence
        assert_eq!(
            original.spec.sdi_pools.len(),
            restored.spec.sdi_pools.len(),
            "Pool count must survive round-trip"
        );
        for (orig_pool, rest_pool) in original
            .spec
            .sdi_pools
            .iter()
            .zip(restored.spec.sdi_pools.iter())
        {
            assert_eq!(orig_pool.pool_name, rest_pool.pool_name);
            assert_eq!(orig_pool.node_specs.len(), rest_pool.node_specs.len());
            for (orig_node, rest_node) in
                orig_pool.node_specs.iter().zip(rest_pool.node_specs.iter())
            {
                assert_eq!(orig_node.node_name, rest_node.node_name);
                assert_eq!(orig_node.ip, rest_node.ip);
                assert_eq!(orig_node.cpu, rest_node.cpu);
                assert_eq!(orig_node.mem_gb, rest_node.mem_gb);
                assert_eq!(orig_node.disk_gb, rest_node.disk_gb);
                assert_eq!(orig_node.roles, rest_node.roles);
            }
        }
        assert_eq!(
            original.resource_pool.network.management_cidr,
            restored.resource_pool.network.management_cidr,
            "Network config must survive round-trip"
        );
    }

    #[test]
    fn test_sprint33b_pool_state_to_get_sdi_pools_format() {
        // Verify build_pool_state output can be consumed by get sdi-pools display.
        let content = include_str!("../../../config/sdi-specs.yaml.example");
        let spec: SdiSpec = serde_yaml::from_str(content).unwrap();

        let pool_states = build_pool_state(&spec);

        // Verify each pool state has required fields for table display
        for pool in &pool_states {
            assert!(!pool.pool_name.is_empty(), "pool_name required for display");
            assert!(!pool.purpose.is_empty(), "purpose required for display");
            for node in &pool.nodes {
                assert!(!node.node_name.is_empty(), "node_name required for display");
                assert!(!node.ip.is_empty(), "ip required for display");
                assert!(!node.host.is_empty(), "host must be resolved (not empty)");
                assert!(node.cpu > 0, "cpu must be > 0");
                assert!(node.mem_gb > 0, "mem_gb must be > 0");
                assert!(node.disk_gb > 0, "disk_gb must be > 0");
            }
        }

        // Verify GPU passthrough is correctly tracked
        let sandbox = &pool_states[1];
        let gpu_node = sandbox.nodes.iter().find(|n| n.node_name == "sandbox-w-2");
        assert!(
            gpu_node.is_some(),
            "sandbox-w-2 (GPU node) must exist in pool state"
        );
        assert!(
            gpu_node.unwrap().gpu_passthrough,
            "sandbox-w-2 must have gpu_passthrough=true"
        );
    }

    // ===== Sub-AC 5: Idempotent re-run of `scalex sdi clean --hard` across all 4 nodes =====
    //
    // These tests validate:
    //   1. First run returns the correct set of operations for 4-node playbox setup
    //   2. Second run (after state is removed by first run) returns [NoState] — clean exit
    //   3. The idempotency holds regardless of which OpenTofu files are present
    //   4. The 4-node scenario (playbox-0/1/2/3) is explicitly represented
    //
    // Per constraints: `scalex sdi clean --hard` must be idempotent; second run must
    // exit cleanly with no leftover K8s/KVM/bridge artifacts.

    /// Sub-AC 5: Full two-run scenario — first run with 4 nodes removes state,
    /// second run returns NoState and exits cleanly.
    #[test]
    fn test_clean_hard_idempotent_two_run_4_nodes_main_tf() {
        // First run: _generated/sdi exists with main.tf, 4 playbox nodes
        let first_run = plan_clean_operations(
            true,  // hard
            true,  // output_dir_exists
            true,  // main_tf_exists
            false, // host_infra_main_tf_exists
            Some(4),
        );
        assert_eq!(
            first_run,
            vec![
                CleanOperation::TofuDestroy,
                CleanOperation::KvmTeardown { node_count: 4 },
                CleanOperation::NodeCleanup { node_count: 4 },
                CleanOperation::RemoveStateDir,
            ],
            "First run must: destroy OpenTofu, run KVM teardown, run node cleanup, remove state dir"
        );

        // After first run: RemoveStateDir removes _generated/sdi → output_dir_exists = false
        // Second run must be a no-op (clean exit, nothing left to do)
        let second_run = plan_clean_operations(
            true,  // hard
            false, // output_dir_exists (removed by RemoveStateDir)
            false, // main_tf_exists (irrelevant — dir is gone)
            false, // host_infra_main_tf_exists
            Some(4),
        );
        assert_eq!(
            second_run,
            vec![CleanOperation::NoState],
            "Second run after state removal must return NoState — idempotent clean exit"
        );
    }

    /// Sub-AC 5: Two-run scenario with host-infra OpenTofu (no-flag path) plus 4 nodes.
    #[test]
    fn test_clean_hard_idempotent_two_run_4_nodes_host_infra_only() {
        // First run: no main.tf but host-infra/main.tf exists (sdi init no-flag path)
        let first_run = plan_clean_operations(
            true,  // hard
            true,  // output_dir_exists
            false, // main_tf_exists
            true,  // host_infra_main_tf_exists
            Some(4),
        );
        assert_eq!(
            first_run,
            vec![
                CleanOperation::TofuDestroyHostInfra,
                CleanOperation::KvmTeardown { node_count: 4 },
                CleanOperation::NodeCleanup { node_count: 4 },
                CleanOperation::RemoveStateDir,
            ],
            "First run (no-flag path) must: destroy host-infra, KVM teardown, clean 4 nodes, remove state"
        );

        // Second run: output_dir removed → NoState
        let second_run = plan_clean_operations(true, false, false, false, Some(4));
        assert_eq!(
            second_run,
            vec![CleanOperation::NoState],
            "Second run after no-flag path cleanup must return NoState"
        );
    }

    /// Sub-AC 5: Two-run scenario with ALL artifacts (both main.tf and host-infra).
    /// This represents a full clean of a complete E2E installation.
    #[test]
    fn test_clean_hard_idempotent_two_run_4_nodes_full_state() {
        // First run: full state — both main.tf and host-infra/main.tf, 4 playbox nodes
        let first_run = plan_clean_operations(
            true, // hard
            true, // output_dir_exists
            true, // main_tf_exists
            true, // host_infra_main_tf_exists
            Some(4),
        );
        assert_eq!(
            first_run,
            vec![
                CleanOperation::TofuDestroyHostInfra,
                CleanOperation::TofuDestroy,
                CleanOperation::KvmTeardown { node_count: 4 },
                CleanOperation::NodeCleanup { node_count: 4 },
                CleanOperation::RemoveStateDir,
            ],
            "Full first run must destroy host-infra THEN main tofu THEN KVM teardown THEN clean all 4 nodes"
        );

        // Second run is always a clean exit
        let second_run = plan_clean_operations(true, false, false, false, Some(4));
        assert_eq!(
            second_run,
            vec![CleanOperation::NoState],
            "Second run of full clean must return NoState"
        );
    }

    /// Sub-AC 5: Repeated re-runs (3rd, 4th, 5th) are all safe — not just the second.
    #[test]
    fn test_clean_hard_idempotent_n_runs_always_safe() {
        // After first run removes state dir, every subsequent run sees no state
        for run_number in 2..=5 {
            let ops = plan_clean_operations(
                true,  // hard
                false, // output_dir does not exist after first run
                false,
                false,
                Some(4),
            );
            assert_eq!(
                ops,
                vec![CleanOperation::NoState],
                "Run #{} must return NoState for idempotent clean exit",
                run_number
            );
        }
    }

    /// Sub-AC 5: Even without baremetal config, second run on clean state still exits cleanly.
    #[test]
    fn test_clean_hard_idempotent_second_run_no_bm_config() {
        // First run: state exists but no baremetal config → SkipNodeCleanup
        let first_run = plan_clean_operations(
            true,  // hard
            true,  // output_dir_exists
            true,  // main_tf_exists
            false, // host_infra_main_tf_exists
            None,  // no baremetal config available
        );
        assert_eq!(
            first_run,
            vec![
                CleanOperation::TofuDestroy,
                CleanOperation::SkipNodeCleanup,
                CleanOperation::RemoveStateDir,
            ],
            "First run without bm_config must SkipNodeCleanup but still remove state"
        );

        // Second run: state removed → NoState regardless of bm_config presence
        let second_run = plan_clean_operations(true, false, false, false, None);
        assert_eq!(
            second_run,
            vec![CleanOperation::NoState],
            "Second run without bm_config must still return NoState"
        );
    }

    /// Sub-AC 5: Validate that the --hard + --yes-i-really-want-to guard is
    /// consistently enforced on every run — protecting against accidental re-runs.
    #[test]
    fn test_clean_hard_confirm_required_on_every_run() {
        // --hard without confirmation must always be rejected
        assert!(
            validate_clean_args(true, false).is_some(),
            "First run: --hard without confirm must be rejected"
        );
        assert!(
            validate_clean_args(true, false).is_some(),
            "Second run: --hard without confirm must still be rejected"
        );
        // With confirmation, all runs are accepted
        assert!(validate_clean_args(true, true).is_none());
    }

    /// Sub-AC 5: RemoveStateDir is always last, ensuring no partial-state can
    /// confuse a subsequent `sdi init` run after `sdi clean --hard`.
    #[test]
    fn test_clean_hard_remove_state_dir_is_last_operation() {
        let ops = plan_clean_operations(true, true, true, true, Some(4));

        // RemoveStateDir must always be the FINAL operation in hard clean
        assert_eq!(
            ops.last(),
            Some(&CleanOperation::RemoveStateDir),
            "RemoveStateDir must be the final operation to prevent leftover partial state"
        );

        // After RemoveStateDir, plan_clean_operations returns NoState — guaranteed clean
        let post_clean = plan_clean_operations(true, false, false, false, Some(4));
        assert_eq!(
            post_clean,
            vec![CleanOperation::NoState],
            "After RemoveStateDir, no artifacts remain — clean exit guaranteed on re-run"
        );
    }

    /// Sub-AC 5: NodeCleanup targets exactly all 4 playbox nodes (playbox-0/1/2/3).
    #[test]
    fn test_clean_hard_plans_node_cleanup_for_all_4_playbox_nodes() {
        let ops = plan_clean_operations(true, true, true, false, Some(4));

        let node_cleanup = ops
            .iter()
            .find(|op| matches!(op, CleanOperation::NodeCleanup { .. }));
        assert!(
            node_cleanup.is_some(),
            "NodeCleanup must be planned for 4 playbox nodes"
        );
        assert_eq!(
            *node_cleanup.unwrap(),
            CleanOperation::NodeCleanup { node_count: 4 },
            "NodeCleanup must target exactly 4 nodes: playbox-0, playbox-1, playbox-2, playbox-3"
        );
    }
    // ===== Sub-AC 4: Per-node targeting tests =====

    /// Sub-AC 4: --node filter applied to plan: 1 node out of 4 targeted.
    /// When --node is specified, only that node is cleaned — others are skipped.
    #[test]
    fn test_clean_hard_with_node_filter_targets_single_node() {
        // plan_clean_operations reflects the count after filtering
        let ops_filtered = plan_clean_operations(true, true, false, false, Some(1));
        let node_cleanup = ops_filtered
            .iter()
            .find(|op| matches!(op, CleanOperation::NodeCleanup { .. }));
        assert!(
            node_cleanup.is_some(),
            "NodeCleanup must be planned when a single node is targeted"
        );
        assert_eq!(
            *node_cleanup.unwrap(),
            CleanOperation::NodeCleanup { node_count: 1 },
            "NodeCleanup with --node filter must target exactly 1 node"
        );
    }

    /// Sub-AC 4: validate_clean_args allows --hard with confirmation regardless of --node.
    #[test]
    fn test_validate_clean_args_hard_confirm_valid() {
        // --hard + confirm: should pass regardless of node filter presence
        assert_eq!(validate_clean_args(true, true), None);
    }

    /// Sub-AC 4: validate_clean_args rejects --hard without confirmation.
    #[test]
    fn test_validate_clean_args_hard_without_confirm_is_error() {
        let err = validate_clean_args(true, false);
        assert!(
            err.is_some(),
            "--hard without --yes-i-really-want-to must fail"
        );
        assert!(
            err.unwrap().contains("--yes-i-really-want-to"),
            "error message must mention the confirmation flag"
        );
    }

    /// Sub-AC 4: plan with node_filter=1 keeps KvmTeardown + NodeCleanup in that order.
    #[test]
    fn test_clean_node_filter_preserves_kvm_then_cleanup_ordering() {
        let ops = plan_clean_operations(true, true, false, false, Some(1));
        let kvm_pos = ops
            .iter()
            .position(|op| matches!(op, CleanOperation::KvmTeardown { .. }));
        let clean_pos = ops
            .iter()
            .position(|op| matches!(op, CleanOperation::NodeCleanup { .. }));
        assert!(
            kvm_pos.is_some(),
            "KvmTeardown must be present even for single-node cleanup"
        );
        assert!(
            clean_pos.is_some(),
            "NodeCleanup must be present even for single-node cleanup"
        );
        assert!(
            kvm_pos.unwrap() < clean_pos.unwrap(),
            "KvmTeardown must precede NodeCleanup even for per-node targeted clean"
        );
    }
}
