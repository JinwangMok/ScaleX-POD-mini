use crate::core::resource_planner::{
    estimate_cluster_resources, format_plan_summary, place_vms, select_tier, to_sdi_spec,
    PlacementTier,
};
use crate::core::resource_pool::generate_resource_pool_summary;
use crate::models::baremetal::NodeFacts;
use crate::models::cluster::K8sClustersConfig;
use crate::models::sdi::{CloudInitConfig, NetworkConfig, OsImageConfig};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct PlanArgs {
    /// Path to k8s-clusters.yaml
    #[arg(default_value = "config/k8s-clusters.yaml")]
    clusters_file: PathBuf,

    /// Facts directory
    #[arg(long, default_value = "_generated/facts")]
    facts_dir: PathBuf,

    /// Output sdi-specs.yaml path
    #[arg(long, short, default_value = "config/sdi-specs.yaml")]
    output: PathBuf,

    /// Force a specific tier (override auto-selection)
    #[arg(long, value_parser = ["minimal", "standard", "ha"])]
    tier: Option<String>,

    /// Management CIDR
    #[arg(long, default_value = "192.168.88.0/24")]
    management_cidr: String,

    /// Gateway IP
    #[arg(long, default_value = "192.168.88.1")]
    gateway: String,

    /// Bridge name
    #[arg(long, default_value = "br0")]
    bridge: String,

    /// Base IP last octet for VM addressing
    #[arg(long, default_value_t = 100)]
    base_ip: u8,

    /// Dry run — print plan without writing
    #[arg(long)]
    dry_run: bool,
}

pub fn run(args: PlanArgs) -> anyhow::Result<()> {
    // 1. Load facts
    let facts = load_facts(&args.facts_dir)?;
    if facts.is_empty() {
        anyhow::bail!(
            "No facts found in {}. Run `scalex facts --all` first.",
            args.facts_dir.display()
        );
    }
    println!("[plan] Loaded facts for {} bare-metal nodes", facts.len());

    // 2. Load cluster config
    let k8s_content = std::fs::read_to_string(&args.clusters_file).map_err(|e| {
        anyhow::anyhow!(
            "Failed to read {}: {}. Run `scalex plan <path>` with correct path.",
            args.clusters_file.display(),
            e
        )
    })?;
    let k8s_config: K8sClustersConfig = serde_yaml::from_str(&k8s_content)?;
    let sdi_clusters: Vec<_> = k8s_config
        .config
        .clusters
        .iter()
        .filter(|c| c.cluster_mode == crate::models::cluster::ClusterMode::Sdi)
        .collect();
    println!("[plan] Found {} SDI clusters", sdi_clusters.len());

    // 3. Estimate resources per cluster
    let estimates: Vec<_> = sdi_clusters
        .iter()
        .map(|c| estimate_cluster_resources(c))
        .collect();

    println!("\n── Resource Estimates ──");
    for est in &estimates {
        println!(
            "  {} ({}): {} vCPU, {} MB RAM, {} MB disk",
            est.cluster_name,
            est.cluster_role,
            est.total.cpu_millicores.div_ceil(1000),
            est.total.memory_mb,
            est.total.disk_mb
        );
        for (comp, budget) in &est.breakdown {
            println!(
                "    {:<25} {:>5}mc  {:>6}MB  {:>6}MB",
                comp, budget.cpu_millicores, budget.memory_mb, budget.disk_mb
            );
        }
    }

    // 4. Build host summary
    let pool_summary = generate_resource_pool_summary(&facts);

    // 5. Select tier
    let tier = if let Some(ref t) = args.tier {
        match t.as_str() {
            "minimal" => PlacementTier::Minimal,
            "standard" => PlacementTier::Standard,
            "ha" => PlacementTier::Ha,
            _ => unreachable!(),
        }
    } else {
        let (auto_tier, warnings) = select_tier(&estimates, &pool_summary.nodes);
        for w in &warnings {
            eprintln!("[plan] WARNING: {}", w);
        }
        println!("\n── Auto-selected tier: {} ──", auto_tier);
        auto_tier
    };

    if args.tier.is_some() {
        println!("\n── Forced tier: {} ──", tier);
    }

    // 6. Place VMs
    let plan = place_vms(&estimates, &pool_summary.nodes, &tier, args.base_ip);
    println!("\n{}", format_plan_summary(&plan));

    // 7. Generate SdiSpec
    let network = NetworkConfig {
        management_bridge: args.bridge,
        management_cidr: args.management_cidr,
        gateway: args.gateway,
        nameservers: vec!["8.8.8.8".to_string(), "8.8.4.4".to_string()],
    };
    let os_image = OsImageConfig {
        source: "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img"
            .to_string(),
        format: "qcow2".to_string(),
    };
    let cloud_init = CloudInitConfig {
        ssh_authorized_keys_file: "~/.ssh/id_ed25519.pub".to_string(),
        packages: vec![
            "curl".to_string(),
            "apt-transport-https".to_string(),
            "nfs-common".to_string(),
            "open-iscsi".to_string(),
        ],
    };

    let sdi_spec = to_sdi_spec(&plan, &network, &os_image, &cloud_init);

    if args.dry_run {
        println!("── Generated sdi-specs.yaml (dry-run) ──\n");
        println!("{}", serde_yaml::to_string(&sdi_spec)?);
    } else {
        let yaml = serde_yaml::to_string(&sdi_spec)?;
        std::fs::write(&args.output, &yaml)?;
        println!(
            "[plan] Written to {} ({} bytes)",
            args.output.display(),
            yaml.len()
        );
    }

    Ok(())
}

/// Load all NodeFacts JSON files from the facts directory.
fn load_facts(facts_dir: &std::path::Path) -> anyhow::Result<Vec<NodeFacts>> {
    let mut facts = Vec::new();
    if !facts_dir.exists() {
        return Ok(facts);
    }
    for entry in std::fs::read_dir(facts_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let content = std::fs::read_to_string(&path)?;
            let node_facts: NodeFacts = serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))?;
            facts.push(node_facts);
        }
    }
    Ok(facts)
}
