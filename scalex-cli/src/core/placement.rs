use crate::core::resource_pool::ResourcePoolSummary;
use crate::models::sdi::SdiSpec;
use std::collections::HashMap;

/// Result of auto-placement: maps VM node_name → assigned bare-metal host.
#[derive(Clone, Debug)]
pub struct PlacementPlan {
    pub assignments: Vec<PlacementAssignment>,
}

#[derive(Clone, Debug)]
pub struct PlacementAssignment {
    pub vm_name: String,
    pub pool_name: String,
    pub host: String,
    pub was_auto: bool,
}

/// Scoring weights: CPU:MEM:Disk = 1:2:1
const WEIGHT_CPU: f64 = 1.0;
const WEIGHT_MEM: f64 = 2.0;
const WEIGHT_DISK: f64 = 1.0;

/// Soft anti-affinity penalty multiplier (0.0-1.0).
/// A node already hosting a VM from the same pool gets its score multiplied by this.
const ANTI_AFFINITY_FACTOR: f64 = 0.3;

/// Available resources on a bare-metal node after cumulative deduction.
#[derive(Clone, Debug)]
struct NodeBudget {
    cpu: f64,
    mem_gb: f64,
    disk_gb: f64,
}

impl NodeBudget {
    fn score(&self) -> f64 {
        self.cpu * WEIGHT_CPU + self.mem_gb * WEIGHT_MEM + self.disk_gb * WEIGHT_DISK
    }
}

/// Resolve auto-placement for all VMs in the SDI spec that lack an explicit `host`.
///
/// Algorithm:
/// 1. Initialize per-node budgets from ResourcePoolSummary
/// 2. First pass: deduct resources for explicitly placed VMs
/// 3. Second pass: for each unplaced VM, score all candidate nodes
///    (resource fit + weighted composite score + spread anti-affinity)
///    and assign to the best candidate. Deduct resources cumulatively.
///
/// Mutates `spec` in-place: fills in `node.host = Some(chosen)` for unplaced VMs.
/// Returns a PlacementPlan describing all assignments.
pub fn resolve_placement(
    spec: &mut SdiSpec,
    pool_summary: &ResourcePoolSummary,
    baremetal_names: &[String],
) -> Result<PlacementPlan, String> {
    if baremetal_names.is_empty() {
        return Err("No bare-metal nodes available for placement".to_string());
    }

    // Initialize budgets from resource pool summary
    let mut budgets: HashMap<String, NodeBudget> = HashMap::new();
    for node in &pool_summary.nodes {
        if baremetal_names.contains(&node.node_name) {
            budgets.insert(
                node.node_name.clone(),
                NodeBudget {
                    cpu: node.cpu_cores as f64,
                    mem_gb: (node.memory_mb as f64) / 1024.0,
                    disk_gb: node.disk_gb as f64,
                },
            );
        }
    }

    // Ensure all baremetal names have a budget (fallback for nodes without facts)
    for name in baremetal_names {
        budgets.entry(name.clone()).or_insert(NodeBudget {
            cpu: 0.0,
            mem_gb: 0.0,
            disk_gb: 0.0,
        });
    }

    let mut assignments = Vec::new();

    // First pass: deduct resources for explicitly placed VMs and record them
    for pool in &spec.spec.sdi_pools {
        for node in &pool.node_specs {
            if let Some(ref host) = node.host {
                if let Some(budget) = budgets.get_mut(host) {
                    budget.cpu -= node.cpu as f64;
                    budget.mem_gb -= node.mem_gb as f64;
                    budget.disk_gb -= node.disk_gb as f64;
                }
                assignments.push(PlacementAssignment {
                    vm_name: node.node_name.clone(),
                    pool_name: pool.pool_name.clone(),
                    host: host.clone(),
                    was_auto: false,
                });
            }
        }
    }

    // Second pass: auto-place VMs without explicit host
    for pool in &mut spec.spec.sdi_pools {
        // Track which bare-metal hosts already have VMs from THIS pool (for anti-affinity)
        let mut pool_hosts: Vec<String> = pool
            .node_specs
            .iter()
            .filter_map(|n| n.host.clone())
            .collect();

        for node in &mut pool.node_specs {
            if node.host.is_some() {
                continue;
            }

            // Find candidates that fit the resource requirements
            let required_cpu = node.cpu as f64;
            let required_mem = node.mem_gb as f64;
            let required_disk = node.disk_gb as f64;

            let mut best_host: Option<String> = None;
            let mut best_score: f64 = f64::NEG_INFINITY;

            for name in baremetal_names {
                let budget = match budgets.get(name) {
                    Some(b) => b,
                    None => continue,
                };

                // Hard constraint: must have enough resources
                if budget.cpu < required_cpu
                    || budget.mem_gb < required_mem
                    || budget.disk_gb < required_disk
                {
                    continue;
                }

                // Compute remaining headroom score after placing this VM
                let remaining = NodeBudget {
                    cpu: budget.cpu - required_cpu,
                    mem_gb: budget.mem_gb - required_mem,
                    disk_gb: budget.disk_gb - required_disk,
                };
                let mut score = remaining.score();

                // Soft anti-affinity: penalize nodes already hosting VMs from this pool
                if pool_hosts.contains(name) {
                    score *= ANTI_AFFINITY_FACTOR;
                }

                if score > best_score {
                    best_score = score;
                    best_host = Some(name.clone());
                }
            }

            match best_host {
                Some(host) => {
                    // Deduct resources
                    if let Some(budget) = budgets.get_mut(&host) {
                        budget.cpu -= required_cpu;
                        budget.mem_gb -= required_mem;
                        budget.disk_gb -= required_disk;
                    }
                    pool_hosts.push(host.clone());
                    node.host = Some(host.clone());
                    assignments.push(PlacementAssignment {
                        vm_name: node.node_name.clone(),
                        pool_name: pool.pool_name.clone(),
                        host,
                        was_auto: true,
                    });
                }
                None => {
                    return Err(format!(
                        "Cannot place VM '{}' (pool '{}', requires {}cpu/{}GB mem/{}GB disk): \
                         no bare-metal node has sufficient remaining resources",
                        node.node_name, pool.pool_name, node.cpu, node.mem_gb, node.disk_gb,
                    ));
                }
            }
        }
    }

    Ok(PlacementPlan { assignments })
}

/// Format placement plan as a human-readable table.
pub fn format_placement_table(plan: &PlacementPlan) -> String {
    let mut out = String::new();
    out.push_str("╔══════════════════════════════════════════════════════════════╗\n");
    out.push_str("║                    VM Placement Plan                        ║\n");
    out.push_str("╠══════════════════════════════════════════════════════════════╣\n");
    for a in &plan.assignments {
        let mode = if a.was_auto { "auto" } else { "explicit" };
        out.push_str(&format!(
            "║  {:<18} → {:<14} (pool: {:<10} {:<8}) ║\n",
            a.vm_name, a.host, a.pool_name, mode,
        ));
    }
    out.push_str("╚══════════════════════════════════════════════════════════════╝\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::resource_pool::{NodeResourceSummary, ResourcePoolSummary};
    use crate::models::sdi::*;

    fn make_pool_summary(nodes: Vec<(&str, u32, u64, u64)>) -> ResourcePoolSummary {
        let node_summaries: Vec<NodeResourceSummary> = nodes
            .iter()
            .map(|(name, cpu, mem_mb, disk_gb)| NodeResourceSummary {
                node_name: name.to_string(),
                cpu_model: "test".to_string(),
                cpu_cores: *cpu,
                cpu_threads: cpu * 2,
                memory_mb: *mem_mb,
                gpu_count: 0,
                gpu_models: vec![],
                disk_count: 1,
                disk_gb: *disk_gb,
                nic_count: 1,
                kernel_version: "6.8.0".to_string(),
                has_bridge: true,
            })
            .collect();
        ResourcePoolSummary {
            total_nodes: node_summaries.len(),
            total_cpu_cores: node_summaries.iter().map(|n| n.cpu_cores).sum(),
            total_cpu_threads: node_summaries.iter().map(|n| n.cpu_threads).sum(),
            total_memory_mb: node_summaries.iter().map(|n| n.memory_mb).sum(),
            total_gpu_count: 0,
            total_disk_count: node_summaries.len(),
            total_disk_gb: node_summaries.iter().map(|n| n.disk_gb).sum(),
            nodes: node_summaries,
        }
    }

    fn make_sdi_spec(pools: Vec<SdiPool>) -> SdiSpec {
        SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "test".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec!["8.8.8.8".to_string()],
                },
            },
            os_image: OsImageConfig {
                source: "test.img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "~/.ssh/id.pub".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec { sdi_pools: pools },
        }
    }

    fn make_node_spec(
        name: &str,
        cpu: u32,
        mem_gb: u32,
        disk_gb: u32,
        host: Option<&str>,
    ) -> NodeSpec {
        NodeSpec {
            node_name: name.to_string(),
            ip: "192.168.88.100".to_string(),
            cpu,
            mem_gb,
            disk_gb,
            host: host.map(|h| h.to_string()),
            roles: vec![],
            devices: None,
        }
    }

    #[test]
    fn test_explicit_host_preserved() {
        let pool = SdiPool {
            pool_name: "tower".to_string(),
            purpose: "mgmt".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![make_node_spec("vm-0", 2, 4, 30, Some("playbox-0"))],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![("playbox-0", 8, 32768, 500)]);
        let names = vec!["playbox-0".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].host, "playbox-0");
        assert!(!plan.assignments[0].was_auto);
    }

    #[test]
    fn test_auto_placement_picks_most_resourceful_node() {
        let pool = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![make_node_spec("vm-0", 4, 8, 60, None)],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        // playbox-1 has more resources
        let summary = make_pool_summary(vec![
            ("playbox-0", 8, 16384, 200),  // 8 cpu, 16GB, 200GB
            ("playbox-1", 16, 65536, 500), // 16 cpu, 64GB, 500GB
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(plan.assignments[0].host, "playbox-1");
        assert!(plan.assignments[0].was_auto);
    }

    #[test]
    fn test_cumulative_deduction() {
        // Two VMs, each needing 6 CPU. playbox-0 has 8 CPU — can only fit one.
        let pool = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("vm-0", 6, 4, 30, None),
                make_node_spec("vm-1", 6, 4, 30, None),
            ],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![
            ("playbox-0", 8, 32768, 500),
            ("playbox-1", 8, 32768, 500),
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        // VMs should be on different hosts due to cumulative deduction
        assert_ne!(plan.assignments[0].host, plan.assignments[1].host);
    }

    #[test]
    fn test_spread_anti_affinity() {
        // Two VMs that both fit on either node — anti-affinity should spread them
        let pool = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("vm-0", 2, 4, 30, None),
                make_node_spec("vm-1", 2, 4, 30, None),
            ],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        // Equal resources on both nodes
        let summary = make_pool_summary(vec![
            ("playbox-0", 16, 65536, 500),
            ("playbox-1", 16, 65536, 500),
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_ne!(
            plan.assignments[0].host, plan.assignments[1].host,
            "anti-affinity should spread VMs across nodes"
        );
    }

    #[test]
    fn test_soft_anti_affinity_allows_colocate() {
        // Only one node available — soft anti-affinity allows co-locate
        let pool = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("vm-0", 2, 4, 30, None),
                make_node_spec("vm-1", 2, 4, 30, None),
            ],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![("playbox-0", 16, 65536, 500)]);
        let names = vec!["playbox-0".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(plan.assignments[0].host, "playbox-0");
        assert_eq!(plan.assignments[1].host, "playbox-0");
    }

    #[test]
    fn test_insufficient_resources_error() {
        let pool = SdiPool {
            pool_name: "big".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![make_node_spec("vm-huge", 64, 128, 1000, None)],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![("playbox-0", 8, 32768, 500)]);
        let names = vec!["playbox-0".to_string()];

        let result = resolve_placement(&mut spec, &summary, &names);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("no bare-metal node has sufficient"));
    }

    #[test]
    fn test_mixed_explicit_and_auto() {
        let pool = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("vm-0", 4, 8, 60, Some("playbox-0")), // explicit
                make_node_spec("vm-1", 4, 8, 60, None),              // auto
            ],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![
            ("playbox-0", 16, 65536, 500),
            ("playbox-1", 16, 65536, 500),
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(plan.assignments[0].host, "playbox-0");
        assert!(!plan.assignments[0].was_auto);
        // Auto-placed VM should go to playbox-1 (anti-affinity from playbox-0)
        assert_eq!(plan.assignments[1].host, "playbox-1");
        assert!(plan.assignments[1].was_auto);
    }

    #[test]
    fn test_mem_weighted_scoring() {
        // playbox-0: more CPU but less memory
        // playbox-1: less CPU but more memory
        // With 1:2:1 weighting, memory-rich node should win
        let pool = SdiPool {
            pool_name: "test".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![make_node_spec("vm-0", 2, 4, 30, None)],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![
            ("playbox-0", 32, 16384, 500), // 32 cpu, 16GB mem
            ("playbox-1", 8, 131072, 500), // 8 cpu, 128GB mem
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(
            plan.assignments[0].host, "playbox-1",
            "memory-rich node should win with 1:2:1 weighting"
        );
    }

    #[test]
    fn test_no_baremetal_nodes_error() {
        let pool = SdiPool {
            pool_name: "test".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![make_node_spec("vm-0", 2, 4, 30, None)],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![]);
        let names: Vec<String> = vec![];

        let result = resolve_placement(&mut spec, &summary, &names);
        assert!(result.is_err());
    }
}
