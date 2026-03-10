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
    /// Initialize SDI: virtualize bare-metal into resource pool
    Init {
        /// SDI specs file (optional — without it, prepares hosts only)
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
        /// Hard clean: remove everything except SSH access
        #[arg(long)]
        hard: bool,
        /// Confirmation flag
        #[arg(long = "yes-i-really-want-to")]
        confirm: bool,

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
            output_dir,
            dry_run,
        } => run_clean(hard, confirm, output_dir, dry_run),
        SdiCommand::Sync {
            config,
            env_file,
            facts_dir,
            output_dir,
            dry_run,
        } => run_sync(config, env_file, facts_dir, output_dir, dry_run),
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
                            // Find primary NIC from facts
                            let primary_nic = facts
                                .nics
                                .first()
                                .map(|n| n.name.as_str())
                                .unwrap_or("eno1");
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
                            let script = host_prepare::generate_bridge_setup_script(
                                primary_nic,
                                &node.node_ip,
                                &net.gateway,
                                24,
                            );
                            let ssh_cmd =
                                build_ssh_command(node, &script, &bm_config.target_nodes)?;
                            match execute_ssh(&ssh_cmd) {
                                Ok(out) => println!("{}", out),
                                Err(e) => eprintln!("[sdi] ERROR on {}: {}", node.name, e),
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

    // Step 4: If spec file provided, generate and apply OpenTofu
    if let Some(ref spec_path) = spec_file {
        println!("[sdi] Phase 2: Generating OpenTofu from spec...");
        let spec = load_sdi_spec(spec_path)?;

        std::fs::create_dir_all(&output_dir)?;

        // Generate main.tf
        let hcl = tofu::generate_tofu_main(&spec);
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

        // Run tofu init + apply
        if !dry_run {
            println!("[sdi] Running OpenTofu init...");
            run_tofu_command(&output_dir, &["init"])?;
            println!("[sdi] Running OpenTofu apply...");
            run_tofu_command(&output_dir, &["apply", "-auto-approve"])?;
        } else {
            println!("[dry-run] Would run: tofu init && tofu apply -auto-approve");
        }

        println!("[sdi] SDI initialization complete.");
    } else {
        // No spec file: set up host-level libvirt infrastructure via OpenTofu
        println!("[sdi] Phase 2: Setting up host-level libvirt infrastructure via OpenTofu...");
        let all_facts = load_all_facts(&facts_dir)?;
        if all_facts.is_empty() {
            println!("[sdi] No facts available. Run `scalex facts --all` first.");
        } else {
            // Generate resource pool summary
            let summary = resource_pool::generate_resource_pool_summary(&all_facts);
            let table = resource_pool::format_resource_pool_table(&summary);
            println!("{}", table);

            // Save summary JSON
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

fn run_clean(hard: bool, confirm: bool, output_dir: PathBuf, dry_run: bool) -> anyhow::Result<()> {
    if let Some(err) = validate_clean_args(hard, confirm) {
        anyhow::bail!(err);
    }

    if !output_dir.exists() {
        println!("[sdi] No SDI state found at {}", output_dir.display());
        return Ok(());
    }

    let main_tf = output_dir.join("main.tf");
    if main_tf.exists() {
        if dry_run {
            println!("[dry-run] Would run: tofu destroy -auto-approve");
        } else {
            println!("[sdi] Destroying OpenTofu resources...");
            run_tofu_command(&output_dir, &["destroy", "-auto-approve"])?;
        }
    }

    if hard {
        // SSH into each node and run full cleanup (K8s, KVM, bridge removal)
        let config_path = PathBuf::from("credentials/.baremetal-init.yaml");
        let env_path = PathBuf::from("credentials/.env");
        if config_path.exists() && env_path.exists() {
            let bm_config = load_baremetal_config(&config_path, &env_path)?;
            let cleanup_script = host_prepare::generate_node_cleanup_script();

            println!(
                "[sdi] Running full node cleanup on {} nodes...",
                bm_config.target_nodes.len()
            );
            for node in &bm_config.target_nodes {
                println!("[sdi] {} — cleaning K8s, KVM, bridge...", node.name);
                if !dry_run {
                    let ssh_cmd =
                        build_ssh_command(node, &cleanup_script, &bm_config.target_nodes)?;
                    match execute_ssh(&ssh_cmd) {
                        Ok(out) => println!("{}", out),
                        Err(e) => eprintln!("[sdi] ERROR on {}: {}", node.name, e),
                    }
                }
            }
        } else {
            println!("[sdi] No baremetal config found, skipping node cleanup (only removing local state)");
        }

        if dry_run {
            println!("[dry-run] Would remove {}", output_dir.display());
        } else {
            std::fs::remove_dir_all(&output_dir)?;
            println!("[sdi] Removed {}", output_dir.display());
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
        if sdi_state_path.exists() {
            let state_raw = std::fs::read_to_string(&sdi_state_path)?;
            let pools: Vec<SdiPoolState> = serde_json::from_str(&state_raw)?;
            let conflicts = sync::detect_vm_conflicts(&pools, &diff.to_remove);
            if !conflicts.is_empty() {
                println!("\n[sdi] WARNING: Removing these nodes will affect hosted VMs:");
                for c in &conflicts {
                    println!("  {} (pool: {}, host: {})", c.vm_name, c.pool_name, c.host);
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
                            let primary_nic = facts
                                .nics
                                .first()
                                .map(|n| n.name.as_str())
                                .unwrap_or("eno1");
                            let sync_net =
                                resolve_network_config(None, None).unwrap_or(NetworkDefaults {
                                    bridge: "br0".to_string(),
                                    cidr: "192.168.88.0/24".to_string(),
                                    gateway: "192.168.88.1".to_string(),
                                });
                            let script = host_prepare::generate_bridge_setup_script(
                                primary_nic,
                                &node.node_ip,
                                &sync_net.gateway,
                                24,
                            );
                            let ssh_cmd =
                                build_ssh_command(node, &script, &bm_config.target_nodes)?;
                            if let Err(e) = execute_ssh(&ssh_cmd) {
                                eprintln!("[sdi] ERROR on {}: {}", node_name, e);
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

fn check_gpu_passthrough_needed(_host_name: &str, spec_path: Option<&str>) -> bool {
    let Some(path) = spec_path else {
        return false;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(spec) = serde_yaml::from_str::<SdiSpec>(&raw) else {
        return false;
    };
    spec.spec.sdi_pools.iter().any(|p| {
        p.node_specs.iter().any(|n| {
            let on_host = n
                .host
                .as_deref()
                .or(p.placement.hosts.first().map(|s| s.as_str()));
            on_host == Some(_host_name) && n.devices.as_ref().is_some_and(|d| d.gpu_passthrough)
        })
    })
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
    bm_config_node_count: Option<usize>,
) -> Vec<CleanOperation> {
    let mut ops = Vec::new();

    if !output_dir_exists {
        ops.push(CleanOperation::NoState);
        return ops;
    }

    if main_tf_exists {
        ops.push(CleanOperation::TofuDestroy);
    }

    if hard {
        match bm_config_node_count {
            Some(count) => ops.push(CleanOperation::NodeCleanup { node_count: count }),
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
        let ops = plan_clean_operations(false, false, false, None);
        assert_eq!(ops, vec![CleanOperation::NoState]);
    }

    #[test]
    fn test_plan_clean_soft_with_main_tf() {
        let ops = plan_clean_operations(false, true, true, Some(4));
        assert_eq!(ops, vec![CleanOperation::TofuDestroy]);
    }

    #[test]
    fn test_plan_clean_soft_without_main_tf() {
        let ops = plan_clean_operations(false, true, false, Some(4));
        assert!(ops.is_empty(), "soft clean with no main.tf should be no-op");
    }

    #[test]
    fn test_plan_clean_hard_with_main_tf_and_bm_config() {
        let ops = plan_clean_operations(true, true, true, Some(4));
        assert_eq!(
            ops,
            vec![
                CleanOperation::TofuDestroy,
                CleanOperation::NodeCleanup { node_count: 4 },
                CleanOperation::RemoveStateDir,
            ]
        );
    }

    #[test]
    fn test_plan_clean_hard_without_bm_config() {
        let ops = plan_clean_operations(true, true, true, None);
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
        let ops = plan_clean_operations(true, true, false, Some(2));
        assert_eq!(
            ops,
            vec![
                CleanOperation::NodeCleanup { node_count: 2 },
                CleanOperation::RemoveStateDir,
            ]
        );
    }

    #[test]
    fn test_plan_clean_hard_no_state_dir_short_circuits() {
        // Even with hard mode, no state dir means nothing to do
        let ops = plan_clean_operations(true, false, false, Some(4));
        assert_eq!(ops, vec![CleanOperation::NoState]);
    }
}
