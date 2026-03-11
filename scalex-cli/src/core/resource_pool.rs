use crate::models::baremetal::NodeFacts;
use serde::{Deserialize, Serialize};

/// Aggregated resource pool summary from all bare-metal node facts.
/// Pure function: takes facts, returns summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourcePoolSummary {
    pub total_nodes: usize,
    pub total_cpu_cores: u32,
    pub total_cpu_threads: u32,
    pub total_memory_mb: u64,
    pub total_gpu_count: usize,
    pub total_disk_count: usize,
    pub total_disk_gb: u64,
    pub nodes: Vec<NodeResourceSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeResourceSummary {
    pub node_name: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub cpu_threads: u32,
    pub memory_mb: u64,
    pub gpu_count: usize,
    pub gpu_models: Vec<String>,
    pub disk_count: usize,
    pub disk_gb: u64,
    pub nic_count: usize,
    pub kernel_version: String,
    pub has_bridge: bool,
}

/// Generate a resource pool summary from collected node facts.
/// Pure function: no I/O, no side effects.
pub fn generate_resource_pool_summary(facts: &[NodeFacts]) -> ResourcePoolSummary {
    let nodes: Vec<NodeResourceSummary> = facts
        .iter()
        .map(|f| NodeResourceSummary {
            node_name: f.node_name.clone(),
            cpu_model: f.cpu.model.clone(),
            cpu_cores: f.cpu.cores,
            cpu_threads: f.cpu.threads,
            memory_mb: f.memory.total_mb,
            gpu_count: f.gpus.len(),
            gpu_models: f.gpus.iter().map(|g| g.model.clone()).collect(),
            disk_count: f.disks.len(),
            disk_gb: f.disks.iter().map(|d| d.size_gb).sum(),
            nic_count: f.nics.len(),
            kernel_version: f.kernel.version.clone(),
            has_bridge: f.bridges.iter().any(|b| b == "br0"),
        })
        .collect();

    ResourcePoolSummary {
        total_nodes: nodes.len(),
        total_cpu_cores: nodes.iter().map(|n| n.cpu_cores).sum(),
        total_cpu_threads: nodes.iter().map(|n| n.cpu_threads).sum(),
        total_memory_mb: nodes.iter().map(|n| n.memory_mb).sum(),
        total_gpu_count: nodes.iter().map(|n| n.gpu_count).sum(),
        total_disk_count: nodes.iter().map(|n| n.disk_count).sum(),
        total_disk_gb: nodes.iter().map(|n| n.disk_gb).sum(),
        nodes,
    }
}

/// Format resource pool summary as a human-readable table string.
/// Pure function.
pub fn format_resource_pool_table(summary: &ResourcePoolSummary) -> String {
    let mut out = String::new();
    out.push_str("╔══════════════════════════════════════════════════════════════════╗\n");
    out.push_str("║                   SDI Resource Pool Summary                     ║\n");
    out.push_str("╠══════════════════════════════════════════════════════════════════╣\n");
    out.push_str(&format!(
        "║  Total Nodes: {}  │  CPU: {} cores / {} threads  │  RAM: {} GB  ║\n",
        summary.total_nodes,
        summary.total_cpu_cores,
        summary.total_cpu_threads,
        summary.total_memory_mb / 1024,
    ));
    out.push_str(&format!(
        "║  GPUs: {}  │  Disks: {} ({} GB)                                    ║\n",
        summary.total_gpu_count, summary.total_disk_count, summary.total_disk_gb,
    ));
    out.push_str("╠══════════════════════════════════════════════════════════════════╣\n");

    for node in &summary.nodes {
        out.push_str(&format!(
            "║  {:<14} │ {} cores │ {} GB │ {} GPU │ {} disks │ br0:{} ║\n",
            node.node_name,
            node.cpu_cores,
            node.memory_mb / 1024,
            node.gpu_count,
            node.disk_count,
            if node.has_bridge { "✓" } else { "✗" },
        ));
    }
    out.push_str("╚══════════════════════════════════════════════════════════════════╝\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::baremetal::*;
    use std::collections::HashMap;

    fn make_facts(name: &str, cores: u32, mem_mb: u64, gpus: usize, disks: usize) -> NodeFacts {
        NodeFacts {
            node_name: name.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            cpu: CpuInfo {
                model: "Intel Xeon".to_string(),
                cores,
                threads: cores * 2,
                architecture: "x86_64".to_string(),
            },
            memory: MemoryInfo {
                total_mb: mem_mb,
                available_mb: mem_mb - 1024,
            },
            disks: (0..disks)
                .map(|i| DiskInfo {
                    name: format!("sda{}", i),
                    size_gb: 500,
                    model: "Samsung SSD".to_string(),
                    disk_type: "ssd".to_string(),
                })
                .collect(),
            nics: vec![NicInfo {
                name: "eno1".to_string(),
                mac: "aa:bb:cc:dd:ee:ff".to_string(),
                speed: "1000Mb/s".to_string(),
                driver: String::new(),
                state: "UP".to_string(),
            }],
            gpus: (0..gpus)
                .map(|_| GpuInfo {
                    pci_id: "01:00.0".to_string(),
                    model: "NVIDIA RTX 3060 [10de:2544]".to_string(),
                    vendor: "nvidia".to_string(),
                    driver: String::new(),
                })
                .collect(),
            iommu_groups: vec![],
            kernel: KernelInfo {
                version: "6.8.0".to_string(),
                params: HashMap::new(),
            },
            bridges: vec!["br0".to_string()],
            bonds: vec![],
            pcie: vec![],
        }
    }

    #[test]
    fn test_generate_resource_pool_summary_single_node() {
        let facts = vec![make_facts("playbox-0", 8, 32768, 1, 2)];
        let summary = generate_resource_pool_summary(&facts);

        assert_eq!(summary.total_nodes, 1);
        assert_eq!(summary.total_cpu_cores, 8);
        assert_eq!(summary.total_cpu_threads, 16);
        assert_eq!(summary.total_memory_mb, 32768);
        assert_eq!(summary.total_gpu_count, 1);
        assert_eq!(summary.total_disk_count, 2);
        assert_eq!(summary.nodes[0].node_name, "playbox-0");
        assert!(summary.nodes[0].has_bridge);
    }

    #[test]
    fn test_generate_resource_pool_summary_multi_node() {
        let facts = vec![
            make_facts("playbox-0", 8, 32768, 0, 2),
            make_facts("playbox-1", 16, 65536, 2, 4),
            make_facts("playbox-2", 8, 32768, 1, 2),
            make_facts("playbox-3", 32, 131072, 4, 8),
        ];
        let summary = generate_resource_pool_summary(&facts);

        assert_eq!(summary.total_nodes, 4);
        assert_eq!(summary.total_cpu_cores, 8 + 16 + 8 + 32);
        assert_eq!(summary.total_cpu_threads, (8 + 16 + 8 + 32) * 2);
        assert_eq!(summary.total_memory_mb, 32768 + 65536 + 32768 + 131072);
        assert_eq!(summary.total_gpu_count, 0 + 2 + 1 + 4);
        assert_eq!(summary.total_disk_count, 2 + 4 + 2 + 8);
    }

    #[test]
    fn test_generate_resource_pool_summary_empty() {
        let summary = generate_resource_pool_summary(&[]);
        assert_eq!(summary.total_nodes, 0);
        assert_eq!(summary.total_cpu_cores, 0);
        assert_eq!(summary.total_memory_mb, 0);
    }

    #[test]
    fn test_format_resource_pool_table() {
        let facts = vec![
            make_facts("playbox-0", 8, 32768, 1, 2),
            make_facts("playbox-1", 16, 65536, 0, 4),
        ];
        let summary = generate_resource_pool_summary(&facts);
        let table = format_resource_pool_table(&summary);

        assert!(table.contains("SDI Resource Pool Summary"));
        assert!(table.contains("playbox-0"));
        assert!(table.contains("playbox-1"));
        assert!(table.contains("Total Nodes: 2"));
    }

    #[test]
    fn test_node_bridge_detection() {
        let mut facts = make_facts("no-bridge", 4, 16384, 0, 1);
        facts.bridges = vec![]; // no br0
        let summary = generate_resource_pool_summary(&[facts]);
        assert!(!summary.nodes[0].has_bridge);
    }

    /// C-4: Resource pool must include total disk capacity in GB (not just count)
    /// for meaningful capacity planning.
    #[test]
    fn test_resource_pool_includes_total_disk_gb() {
        let facts = vec![
            make_facts("playbox-0", 8, 32768, 0, 2), // 2 disks × 500 GB = 1000 GB
            make_facts("playbox-1", 8, 32768, 0, 3), // 3 disks × 500 GB = 1500 GB
        ];
        let summary = generate_resource_pool_summary(&facts);
        assert_eq!(
            summary.total_disk_gb, 2500,
            "total_disk_gb must aggregate actual disk capacity across all nodes"
        );
    }

    /// C-4: Per-node summary must include disk capacity in GB.
    #[test]
    fn test_node_summary_includes_disk_gb() {
        let facts = vec![make_facts("playbox-0", 8, 32768, 0, 2)]; // 2 × 500 GB
        let summary = generate_resource_pool_summary(&facts);
        assert_eq!(
            summary.nodes[0].disk_gb, 1000,
            "node disk_gb must be sum of all disk sizes"
        );
    }
}
