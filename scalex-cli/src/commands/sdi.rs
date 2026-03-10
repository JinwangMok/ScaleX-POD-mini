use crate::core::config::load_baremetal_config;
use crate::core::host_prepare;
use crate::core::resource_pool;
use crate::core::ssh::{build_ssh_command, execute_ssh};
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

    // Step 2: Load baremetal config and facts
    let bm_config = load_baremetal_config(&config_path, &env_path)?;
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
                            let script = host_prepare::generate_bridge_setup_script(
                                primary_nic,
                                &node.node_ip,
                                "192.168.88.1", // TODO: from spec
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
            let host_inputs: Vec<tofu::HostInfraInput> = bm_config
                .target_nodes
                .iter()
                .map(|n| tofu::HostInfraInput {
                    name: n.name.clone(),
                    ip: n.node_ip.clone(),
                })
                .collect();

            // Use first node's network info as management network defaults
            let mgmt_bridge = "br0";
            let mgmt_cidr = "192.168.88.0/24";
            let mgmt_gateway = "192.168.88.1";

            let host_hcl =
                tofu::generate_tofu_host_infra(&host_inputs, mgmt_bridge, mgmt_cidr, mgmt_gateway);

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
    if hard && !confirm {
        anyhow::bail!("--hard requires --yes-i-really-want-to flag");
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
    let desired_nodes: std::collections::HashSet<String> = bm_config
        .target_nodes
        .iter()
        .map(|n| n.name.clone())
        .collect();

    // Step 2: Load current state from facts directory
    let current_facts = load_all_facts(&facts_dir)?;
    let current_nodes: std::collections::HashSet<String> =
        current_facts.iter().map(|f| f.node_name.clone()).collect();

    // Step 3: Compute diff
    let to_add: Vec<&str> = desired_nodes
        .iter()
        .filter(|n| !current_nodes.contains(n.as_str()))
        .map(|n| n.as_str())
        .collect();
    let to_remove: Vec<&str> = current_nodes
        .iter()
        .filter(|n| !desired_nodes.contains(n.as_str()))
        .map(|n| n.as_str())
        .collect();
    let unchanged: Vec<&str> = desired_nodes
        .iter()
        .filter(|n| current_nodes.contains(n.as_str()))
        .map(|n| n.as_str())
        .collect();

    // Step 4: Report sync plan
    println!("[sdi] Sync plan:");
    println!(
        "  Unchanged nodes ({}): {}",
        unchanged.len(),
        unchanged.join(", ")
    );
    if to_add.is_empty() && to_remove.is_empty() {
        println!("[sdi] Already in sync. Nothing to do.");
        return Ok(());
    }
    if !to_add.is_empty() {
        println!("  + Add nodes ({}): {}", to_add.len(), to_add.join(", "));
    }
    if !to_remove.is_empty() {
        println!(
            "  - Remove nodes ({}): {}",
            to_remove.len(),
            to_remove.join(", ")
        );
    }

    // Step 5: Check for side effects on removal
    if !to_remove.is_empty() {
        // Check if removed nodes host any SDI VMs
        let sdi_state_path = output_dir.join("sdi-state.json");
        if sdi_state_path.exists() {
            let state_raw = std::fs::read_to_string(&sdi_state_path)?;
            let pools: Vec<SdiPoolState> = serde_json::from_str(&state_raw)?;
            let mut affected_vms = Vec::new();
            for pool in &pools {
                for node in &pool.nodes {
                    if to_remove.contains(&node.host.as_str()) {
                        affected_vms.push(format!(
                            "  {} (pool: {}, host: {})",
                            node.node_name, pool.pool_name, node.host
                        ));
                    }
                }
            }
            if !affected_vms.is_empty() {
                println!("\n[sdi] WARNING: Removing these nodes will affect hosted VMs:");
                for vm in &affected_vms {
                    println!("{}", vm);
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
    if !to_add.is_empty() {
        println!("[sdi] Gathering facts for new nodes...");
        std::fs::create_dir_all(&facts_dir)?;
        for node_name in &to_add {
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
        for node_name in &to_add {
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
                            let script = host_prepare::generate_bridge_setup_script(
                                primary_nic,
                                &node.node_ip,
                                "192.168.88.1",
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
    if !to_remove.is_empty() {
        println!("[sdi] Removing facts for decommissioned nodes...");
        for node_name in &to_remove {
            let facts_path = facts_dir.join(format!("{}.json", node_name));
            if facts_path.exists() {
                std::fs::remove_file(&facts_path)?;
                println!("[sdi] Removed {}", facts_path.display());
            }
        }
    }

    // Step 8: Regenerate OpenTofu if sdi-state exists (needs re-plan with new hosts)
    let sdi_state_path = output_dir.join("sdi-state.json");
    if sdi_state_path.exists() && !to_add.is_empty() {
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
