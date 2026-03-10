use crate::core::config::load_baremetal_config;
use crate::core::host_prepare;
use crate::core::ssh::{build_ssh_command, execute_ssh};
use crate::core::tofu;
use crate::models::baremetal::NodeFacts;
use crate::models::sdi::{SdiPoolState, SdiSpec};
use clap::{Args, Subcommand};
use std::path::PathBuf;

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
    Sync,
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
        SdiCommand::Sync => {
            println!("[sdi] Sync: not yet implemented (Phase 5)");
            Ok(())
        }
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
        println!("[sdi] Host preparation complete. Provide a spec file to create VM pools.");
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

fn dir_is_empty(path: &PathBuf) -> bool {
    std::fs::read_dir(path)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true)
}

fn load_all_facts(facts_dir: &PathBuf) -> anyhow::Result<Vec<NodeFacts>> {
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

fn run_tofu_command(work_dir: &PathBuf, args: &[&str]) -> anyhow::Result<()> {
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
