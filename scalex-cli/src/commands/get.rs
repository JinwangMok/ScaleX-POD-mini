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
        GetResource::ConfigFiles => get_config_files(),
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

#[derive(Tabled)]
struct ConfigFileRow {
    #[tabled(rename = "File")]
    path: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Description")]
    description: String,
}

fn get_config_files() -> anyhow::Result<()> {
    let checks: Vec<(&str, &str)> = vec![
        (
            "credentials/.baremetal-init.yaml",
            "Bare-metal node SSH access config",
        ),
        (
            "credentials/.env",
            "Environment variables (SSH passwords/keys)",
        ),
        ("credentials/secrets.yaml", "Keycloak/ArgoCD/CF secrets"),
        (
            "credentials/cloudflare-tunnel.json",
            "Cloudflare tunnel credentials",
        ),
        ("config/sdi-specs.yaml", "SDI VM pool specifications"),
        (
            "config/k8s-clusters.yaml",
            "Multi-cluster Kubernetes config",
        ),
        ("_generated/facts/", "Hardware facts (from `scalex facts`)"),
        (
            "_generated/sdi/",
            "SDI OpenTofu state (from `scalex sdi init`)",
        ),
        (
            "_generated/clusters/",
            "Cluster configs (from `scalex cluster init`)",
        ),
    ];

    let mut rows: Vec<ConfigFileRow> = Vec::new();
    for (path, desc) in &checks {
        let p = std::path::Path::new(path);
        let status = if p.exists() {
            if p.is_dir() {
                let count = std::fs::read_dir(p).map(|d| d.count()).unwrap_or(0);
                if count > 0 {
                    format!("OK ({} items)", count)
                } else {
                    "EMPTY".to_string()
                }
            } else {
                // Validate YAML files
                if path.ends_with(".yaml") || path.ends_with(".yml") {
                    match std::fs::read_to_string(p) {
                        Ok(content) => {
                            if serde_yaml::from_str::<serde_yaml::Value>(&content).is_ok() {
                                "OK (valid YAML)".to_string()
                            } else {
                                "INVALID YAML".to_string()
                            }
                        }
                        Err(_) => "READ ERROR".to_string(),
                    }
                } else {
                    "OK".to_string()
                }
            }
        } else {
            "MISSING".to_string()
        };

        rows.push(ConfigFileRow {
            path: path.to_string(),
            status,
            description: desc.to_string(),
        });
    }

    let table = Table::new(&rows).to_string();
    println!("{}", table);
    Ok(())
}

/// Validate config file presence and type. Pure function.
#[allow(dead_code)]
fn classify_config_status(
    path: &str,
    exists: bool,
    is_dir: bool,
    dir_count: usize,
    yaml_valid: Option<bool>,
) -> String {
    if !exists {
        return "MISSING".to_string();
    }
    if is_dir {
        if dir_count > 0 {
            format!("OK ({} items)", dir_count)
        } else {
            "EMPTY".to_string()
        }
    } else if path.ends_with(".yaml") || path.ends_with(".yml") {
        match yaml_valid {
            Some(true) => "OK (valid YAML)".to_string(),
            Some(false) => "INVALID YAML".to_string(),
            None => "READ ERROR".to_string(),
        }
    } else {
        "OK".to_string()
    }
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

/// Convert SdiPoolState list to flat row list for display. Pure function.
#[allow(dead_code)]
fn sdi_pools_to_rows(pools: &[SdiPoolState]) -> Vec<SdiPoolRow> {
    pools
        .iter()
        .flat_map(|pool| {
            pool.nodes.iter().map(move |node| SdiPoolRow {
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
            })
        })
        .collect()
}

/// Count nodes from inventory.ini content. Pure function.
pub fn count_nodes_from_inventory(content: &str) -> u32 {
    content
        .lines()
        .filter(|l| l.contains("ansible_host="))
        .count() as u32
}

/// Extract cluster name from cluster-vars.yml content. Pure function.
#[allow(dead_code)]
fn extract_cluster_name_from_vars(content: &str) -> Option<String> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::baremetal::*;
    use crate::models::sdi::{SdiNodeState, SdiPoolState};

    fn make_test_facts() -> NodeFacts {
        NodeFacts {
            node_name: "playbox-0".to_string(),
            timestamp: "2026-03-10T00:00:00Z".to_string(),
            cpu: CpuInfo {
                model: "Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz".to_string(),
                cores: 6,
                threads: 12,
                architecture: "x86_64".to_string(),
            },
            memory: MemoryInfo {
                total_mb: 32000,
                available_mb: 28000,
            },
            disks: vec![
                DiskInfo {
                    name: "sda".to_string(),
                    size_gb: 465,
                    disk_type: "disk".to_string(),
                    model: "Samsung_SSD_870".to_string(),
                },
                DiskInfo {
                    name: "nvme0n1".to_string(),
                    size_gb: 931,
                    disk_type: "disk".to_string(),
                    model: "WD_BLACK_SN770".to_string(),
                },
            ],
            nics: vec![
                NicInfo {
                    name: "eno1".to_string(),
                    mac: String::new(),
                    speed: "1G".to_string(),
                    driver: "e1000e".to_string(),
                    state: "up".to_string(),
                },
                NicInfo {
                    name: "ens2f0".to_string(),
                    mac: String::new(),
                    speed: "10G".to_string(),
                    driver: "mlx5_core".to_string(),
                    state: "up".to_string(),
                },
            ],
            gpus: vec![GpuInfo {
                pci_id: "01:00.0".to_string(),
                model: "NVIDIA GeForce RTX 3060".to_string(),
                vendor: "nvidia".to_string(),
                driver: String::new(),
            }],
            iommu_groups: vec![
                IommuGroup {
                    id: 1,
                    devices: vec!["0000:01:00.0".to_string()],
                },
                IommuGroup {
                    id: 2,
                    devices: vec!["0000:00:1f.0".to_string()],
                },
            ],
            kernel: KernelInfo {
                version: "6.8.0-45-generic".to_string(),
                params: std::collections::HashMap::new(),
            },
            bridges: vec!["br0".to_string()],
            bonds: vec![],
            pcie: vec![],
        }
    }

    #[test]
    fn test_facts_to_row() {
        let facts = make_test_facts();
        let row = facts_to_row(&facts);

        assert_eq!(row.name, "playbox-0");
        assert_eq!(row.cores, 6);
        assert_eq!(row.ram_gb, 31); // 32000 / 1024 = 31
        assert_eq!(row.gpus, 1);
        assert_eq!(row.iommu, "2 groups");
        assert_eq!(row.kernel, "6.8.0-45-generic");
        assert!(row.disks.contains("sda(465G)"));
        assert!(row.disks.contains("nvme0n1(931G)"));
        assert!(row.nics.contains("eno1(1G)"));
        assert!(row.nics.contains("ens2f0(10G)"));
    }

    #[test]
    fn test_facts_to_row_no_gpu() {
        let mut facts = make_test_facts();
        facts.gpus.clear();
        facts.iommu_groups.clear();

        let row = facts_to_row(&facts);
        assert_eq!(row.gpus, 0);
        assert_eq!(row.iommu, "none");
    }

    #[test]
    fn test_facts_to_row_long_cpu_model_truncated() {
        let mut facts = make_test_facts();
        facts.cpu.model =
            "A very long CPU model name that exceeds thirty characters limit".to_string();

        let row = facts_to_row(&facts);
        assert_eq!(row.cpu.len(), 30);
    }

    #[test]
    fn test_classify_config_status_missing() {
        assert_eq!(
            classify_config_status("test.yaml", false, false, 0, None),
            "MISSING"
        );
    }

    #[test]
    fn test_classify_config_status_valid_yaml() {
        assert_eq!(
            classify_config_status("test.yaml", true, false, 0, Some(true)),
            "OK (valid YAML)"
        );
    }

    #[test]
    fn test_classify_config_status_invalid_yaml() {
        assert_eq!(
            classify_config_status("test.yml", true, false, 0, Some(false)),
            "INVALID YAML"
        );
    }

    #[test]
    fn test_classify_config_status_dir_with_items() {
        assert_eq!(
            classify_config_status("facts/", true, true, 3, None),
            "OK (3 items)"
        );
    }

    #[test]
    fn test_classify_config_status_empty_dir() {
        assert_eq!(
            classify_config_status("facts/", true, true, 0, None),
            "EMPTY"
        );
    }

    #[test]
    fn test_classify_config_status_non_yaml_file() {
        assert_eq!(
            classify_config_status("tunnel.json", true, false, 0, None),
            "OK"
        );
    }

    // ── Unit 4: SDI pools and clusters pure function tests ──

    #[test]
    fn test_sdi_pools_to_rows_basic() {
        let pools = vec![SdiPoolState {
            pool_name: "tower".to_string(),
            purpose: "management".to_string(),
            nodes: vec![SdiNodeState {
                node_name: "tower-cp-0".to_string(),
                ip: "192.168.88.100".to_string(),
                host: "playbox-0".to_string(),
                cpu: 2,
                mem_gb: 3,
                disk_gb: 30,
                gpu_passthrough: false,
                status: "running".to_string(),
            }],
        }];
        let rows = sdi_pools_to_rows(&pools);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pool, "tower");
        assert_eq!(rows[0].node, "tower-cp-0");
        assert_eq!(rows[0].gpu, "-");
        assert_eq!(rows[0].status, "running");
    }

    #[test]
    fn test_sdi_pools_to_rows_multi_pool_multi_node() {
        let pools = vec![
            SdiPoolState {
                pool_name: "tower".to_string(),
                purpose: "management".to_string(),
                nodes: vec![SdiNodeState {
                    node_name: "tower-cp-0".to_string(),
                    ip: "192.168.88.100".to_string(),
                    host: "playbox-0".to_string(),
                    cpu: 2,
                    mem_gb: 3,
                    disk_gb: 30,
                    gpu_passthrough: false,
                    status: "running".to_string(),
                }],
            },
            SdiPoolState {
                pool_name: "sandbox".to_string(),
                purpose: "workload".to_string(),
                nodes: vec![
                    SdiNodeState {
                        node_name: "sandbox-cp-0".to_string(),
                        ip: "192.168.88.110".to_string(),
                        host: "playbox-0".to_string(),
                        cpu: 4,
                        mem_gb: 8,
                        disk_gb: 60,
                        gpu_passthrough: false,
                        status: "running".to_string(),
                    },
                    SdiNodeState {
                        node_name: "sandbox-w-0".to_string(),
                        ip: "192.168.88.120".to_string(),
                        host: "playbox-1".to_string(),
                        cpu: 8,
                        mem_gb: 16,
                        disk_gb: 100,
                        gpu_passthrough: true,
                        status: "running".to_string(),
                    },
                ],
            },
        ];
        let rows = sdi_pools_to_rows(&pools);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].pool, "tower");
        assert_eq!(rows[1].pool, "sandbox");
        assert_eq!(rows[2].pool, "sandbox");
        assert_eq!(rows[2].gpu, "VFIO");
    }

    #[test]
    fn test_sdi_pools_to_rows_empty() {
        let rows = sdi_pools_to_rows(&[]);
        assert!(rows.is_empty());
    }

    #[test]
    fn test_count_nodes_from_inventory() {
        let content = r#"[all]
tower-cp-0 ansible_host=192.168.88.100 ip=192.168.88.100
sandbox-cp-0 ansible_host=192.168.88.110 ip=192.168.88.110
sandbox-w-0 ansible_host=192.168.88.120 ip=192.168.88.120

[kube_control_plane]
tower-cp-0
"#;
        assert_eq!(count_nodes_from_inventory(content), 3);
    }

    #[test]
    fn test_count_nodes_from_inventory_empty() {
        assert_eq!(count_nodes_from_inventory(""), 0);
        assert_eq!(count_nodes_from_inventory("[all]\n[kube_node]\n"), 0);
    }

    #[test]
    fn test_extract_cluster_name_from_vars() {
        let content = r#"kube_version: "1.33.1"
cluster_name: "tower"
dns_domain: "tower.local"
"#;
        assert_eq!(
            extract_cluster_name_from_vars(content),
            Some("tower".to_string())
        );
    }

    #[test]
    fn test_extract_cluster_name_from_vars_missing() {
        let content = "kube_version: \"1.33.1\"\n";
        assert_eq!(extract_cluster_name_from_vars(content), None);
    }
}
