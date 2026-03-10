use crate::models::baremetal::NodeFacts;
use crate::models::sdi::SdiPoolState;
use clap::{Args, Subcommand};
use std::path::PathBuf;
use tabled::{Table, Tabled};

#[derive(Args)]
pub struct GetArgs {
    #[command(subcommand)]
    resource: GetResource,
}

#[derive(Subcommand)]
pub enum GetResource {
    /// Show bare-metal node facts
    Baremetals {
        /// Directory containing facts JSON files
        #[arg(long, default_value = "_generated/facts")]
        facts_dir: PathBuf,
    },
    /// Show SDI VM pools
    SdiPools {
        /// SDI state directory
        #[arg(long, default_value = "_generated/sdi")]
        sdi_dir: PathBuf,
    },
    /// Show Kubernetes clusters
    Clusters {
        /// Clusters output directory
        #[arg(long, default_value = "_generated/clusters")]
        clusters_dir: PathBuf,
    },
    /// Show configuration files and validation status
    ConfigFiles,
}

#[derive(Tabled)]
struct BaremetalRow {
    #[tabled(rename = "Node")]
    name: String,
    #[tabled(rename = "CPU")]
    cpu: String,
    #[tabled(rename = "Cores")]
    cores: u32,
    #[tabled(rename = "RAM (GB)")]
    ram_gb: u64,
    #[tabled(rename = "Disks")]
    disks: String,
    #[tabled(rename = "NICs")]
    nics: String,
    #[tabled(rename = "GPUs")]
    gpus: u32,
    #[tabled(rename = "IOMMU")]
    iommu: String,
    #[tabled(rename = "Kernel")]
    kernel: String,
}

pub fn run(args: GetArgs) -> anyhow::Result<()> {
    match args.resource {
        GetResource::Baremetals { facts_dir } => get_baremetals(&facts_dir),
        GetResource::SdiPools { sdi_dir } => get_sdi_pools(&sdi_dir),
        GetResource::Clusters { clusters_dir } => get_clusters(&clusters_dir),
        GetResource::ConfigFiles => {
            println!("Config files: not yet implemented (Phase 5)");
            Ok(())
        }
    }
}

fn get_baremetals(facts_dir: &PathBuf) -> anyhow::Result<()> {
    if !facts_dir.exists() {
        anyhow::bail!(
            "Facts directory '{}' not found. Run `scalex facts` first.",
            facts_dir.display()
        );
    }

    let mut rows: Vec<BaremetalRow> = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(facts_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let content = std::fs::read_to_string(entry.path())?;
        let facts: NodeFacts = serde_json::from_str(&content)?;
        rows.push(facts_to_row(&facts));
    }

    if rows.is_empty() {
        println!("No facts found. Run `scalex facts` first.");
        return Ok(());
    }

    let table = Table::new(&rows).to_string();
    println!("{}", table);
    Ok(())
}

#[derive(Tabled)]
struct SdiPoolRow {
    #[tabled(rename = "Pool")]
    pool: String,
    #[tabled(rename = "Node")]
    node: String,
    #[tabled(rename = "IP")]
    ip: String,
    #[tabled(rename = "Host")]
    host: String,
    #[tabled(rename = "CPU")]
    cpu: u32,
    #[tabled(rename = "RAM (GB)")]
    mem_gb: u32,
    #[tabled(rename = "Disk (GB)")]
    disk_gb: u32,
    #[tabled(rename = "GPU")]
    gpu: String,
    #[tabled(rename = "Status")]
    status: String,
}

#[derive(Tabled)]
struct ClusterRow {
    #[tabled(rename = "Cluster")]
    name: String,
    #[tabled(rename = "Role")]
    role: String,
    #[tabled(rename = "Nodes")]
    nodes: u32,
    #[tabled(rename = "Kubeconfig")]
    kubeconfig: String,
}

fn get_clusters(clusters_dir: &std::path::Path) -> anyhow::Result<()> {
    if !clusters_dir.exists() {
        anyhow::bail!(
            "Clusters directory '{}' not found. Run `scalex cluster init` first.",
            clusters_dir.display()
        );
    }

    let mut rows: Vec<ClusterRow> = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(clusters_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let cluster_dir = entry.path();

        // Count nodes from inventory
        let inventory_path = cluster_dir.join("inventory.ini");
        let node_count = if inventory_path.exists() {
            let content = std::fs::read_to_string(&inventory_path).unwrap_or_default();
            content
                .lines()
                .filter(|l| l.contains("ansible_host="))
                .count() as u32
        } else {
            0
        };

        // Check kubeconfig
        let kc_path = cluster_dir.join("kubeconfig.yaml");
        let kc_status = if kc_path.exists() {
            kc_path.display().to_string()
        } else {
            "not available".to_string()
        };

        // Try to read role from cluster-vars
        let vars_path = cluster_dir.join("cluster-vars.yml");
        let role = if vars_path.exists() {
            let content = std::fs::read_to_string(&vars_path).unwrap_or_default();
            content
                .lines()
                .find(|l| l.starts_with("cluster_name:"))
                .map(|l| {
                    l.split(':')
                        .nth(1)
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .to_string()
                })
                .unwrap_or_else(|| name.clone())
        } else {
            name.clone()
        };

        rows.push(ClusterRow {
            name,
            role,
            nodes: node_count,
            kubeconfig: kc_status,
        });
    }

    if rows.is_empty() {
        println!("No clusters found. Run `scalex cluster init` first.");
        return Ok(());
    }

    let table = Table::new(&rows).to_string();
    println!("{}", table);
    Ok(())
}

fn get_sdi_pools(sdi_dir: &std::path::Path) -> anyhow::Result<()> {
    let state_path = sdi_dir.join("sdi-state.json");
    if !state_path.exists() {
        anyhow::bail!(
            "SDI state not found at '{}'. Run `scalex sdi init <spec>` first.",
            state_path.display()
        );
    }

    let content = std::fs::read_to_string(&state_path)?;
    let pools: Vec<SdiPoolState> = serde_json::from_str(&content)?;

    let mut rows: Vec<SdiPoolRow> = Vec::new();
    for pool in &pools {
        for node in &pool.nodes {
            rows.push(SdiPoolRow {
                pool: pool.pool_name.clone(),
                node: node.node_name.clone(),
                ip: node.ip.clone(),
                host: node.host.clone(),
                cpu: node.cpu,
                mem_gb: node.mem_gb,
                disk_gb: node.disk_gb,
                gpu: if node.gpu_passthrough {
                    "VFIO".to_string()
                } else {
                    "-".to_string()
                },
                status: node.status.clone(),
            });
        }
    }

    if rows.is_empty() {
        println!("No SDI pools found.");
        return Ok(());
    }

    let table = Table::new(&rows).to_string();
    println!("{}", table);
    Ok(())
}

/// Convert NodeFacts to a table row. Pure function.
fn facts_to_row(facts: &NodeFacts) -> BaremetalRow {
    let disk_summary: String = facts
        .disks
        .iter()
        .map(|d| format!("{}({}G)", d.name, d.size_gb))
        .collect::<Vec<_>>()
        .join(",");

    let nic_summary: String = facts
        .nics
        .iter()
        .map(|n| format!("{}({})", n.name, n.speed))
        .collect::<Vec<_>>()
        .join(",");

    let iommu_str = if facts.iommu_groups.is_empty() {
        "none".to_string()
    } else {
        format!("{} groups", facts.iommu_groups.len())
    };

    BaremetalRow {
        name: facts.node_name.clone(),
        cpu: facts.cpu.model.chars().take(30).collect(),
        cores: facts.cpu.cores,
        ram_gb: facts.memory.total_mb / 1024,
        disks: disk_summary,
        nics: nic_summary,
        gpus: facts.gpus.len() as u32,
        iommu: iommu_str,
        kernel: facts.kernel.version.clone(),
    }
}
