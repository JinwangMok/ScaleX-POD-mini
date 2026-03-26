use crate::core::kernel;
use clap::{Args, ValueEnum};

#[derive(Args)]
pub struct KernelTuneArgs {
    /// Node role for kernel tuning
    #[arg(long, default_value = "worker")]
    role: String,

    /// Output format
    #[arg(long, value_enum, default_value = "sysctl")]
    format: OutputFormat,

    /// Compare against a node's current kernel params (from facts)
    #[arg(long)]
    diff_node: Option<String>,

    /// Facts directory for diff comparison
    #[arg(long, default_value = "_generated/facts")]
    facts_dir: std::path::PathBuf,
}

#[derive(Clone, ValueEnum)]
pub enum OutputFormat {
    /// sysctl.conf format
    Sysctl,
    /// Ansible task YAML
    Ansible,
}

pub fn run(args: KernelTuneArgs) -> anyhow::Result<()> {
    let params = kernel::generate_k8s_sysctl_params(&args.role);

    if let Some(node_name) = &args.diff_node {
        let facts_path = args.facts_dir.join(format!("{}.json", node_name));
        if !facts_path.exists() {
            anyhow::bail!(
                "Facts file '{}' not found. Run `scalex-pod facts --host {}` first.",
                facts_path.display(),
                node_name
            );
        }
        let content = std::fs::read_to_string(&facts_path)?;
        let facts: crate::models::baremetal::NodeFacts = serde_json::from_str(&content)?;
        let diffs = kernel::diff_kernel_params(&facts.kernel.params, &params);

        if diffs.is_empty() {
            println!("Node '{}' kernel params are fully tuned.", node_name);
        } else {
            println!(
                "Node '{}': {} param(s) need tuning:\n",
                node_name,
                diffs.len()
            );
            for (key, current, recommended) in &diffs {
                let cur_str = current.as_deref().unwrap_or("(not set)");
                println!("  {} = {} -> {}", key, cur_str, recommended);
            }
        }
        return Ok(());
    }

    match args.format {
        OutputFormat::Sysctl => {
            let output = kernel::format_sysctl_conf(&params);
            print!("{}", output);
        }
        OutputFormat::Ansible => {
            let output = kernel::format_ansible_sysctl_tasks(&params);
            print!("{}", output);
        }
    }

    Ok(())
}
