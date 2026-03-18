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

/// Fraction of host resources reserved for hypervisor + host OS.
/// Matches resource_planner::HOST_RESERVE_FRACTION.
const HOST_RESERVE_FRACTION: f64 = 0.15;

/// Cross-pool penalty per additional VM on the same host (multiplicative).
/// Encourages spreading VMs across all hosts even across pools.
const CROSS_POOL_VM_PENALTY: f64 = 0.85;

/// Available resources on a bare-metal node after cumulative deduction.
#[derive(Clone, Debug)]
struct NodeBudget {
    cpu: f64,
    mem_gb: f64,
    disk_gb: f64,
    gpu_count: usize,
    gpu_assigned: usize,
    /// Total VMs assigned to this node (across all pools)
    vm_count: usize,
}

impl NodeBudget {
    fn score(&self) -> f64 {
        self.cpu * WEIGHT_CPU + self.mem_gb * WEIGHT_MEM + self.disk_gb * WEIGHT_DISK
    }
}

/// Descriptor for an unplaced VM, used for FFD sorting.
#[derive(Clone, Debug)]
struct UnplacedVm {
    pool_idx: usize,
    node_idx: usize,
    vm_name: String,
    pool_name: String,
    cpu: f64,
    mem_gb: f64,
    disk_gb: f64,
    needs_gpu: bool,
    /// Pool-level candidate hosts (empty = all hosts eligible)
    candidate_hosts: Vec<String>,
    /// Whether pool has spread enabled
    spread: bool,
}

impl UnplacedVm {
    /// Resource demand score for FFD sorting (largest first)
    fn demand_score(&self) -> f64 {
        self.cpu * WEIGHT_CPU + self.mem_gb * WEIGHT_MEM + self.disk_gb * WEIGHT_DISK
    }
}

/// Resolve auto-placement for all VMs in the SDI spec that lack an explicit `host`.
///
/// Algorithm: First-Fit Decreasing (FFD) bin-packing with weighted scoring.
///
/// 1. Initialize per-node budgets from ResourcePoolSummary (with 15% host reserve)
/// 2. First pass: deduct resources for explicitly placed VMs
/// 3. Collect all unplaced VMs and sort by resource demand (descending — FFD)
/// 4. For each unplaced VM (largest first), score all candidate nodes:
///    - Hard constraint: must fit resource requirements (CPU, memory, disk, GPU)
///    - Pool-level `placement.hosts` constrains eligible nodes
///    - Weighted composite score based on remaining headroom (CPU:MEM:DISK = 1:2:1)
///    - Same-pool anti-affinity penalty (x0.3) when `spread: true`
///    - Cross-pool VM count penalty (x0.85 per existing VM) to balance load
///    Assign to the best candidate. Deduct resources cumulatively.
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

    // Initialize budgets from resource pool summary, applying host reserve
    let mut budgets: HashMap<String, NodeBudget> = HashMap::new();
    for node in &pool_summary.nodes {
        if baremetal_names.contains(&node.node_name) {
            budgets.insert(
                node.node_name.clone(),
                NodeBudget {
                    cpu: (node.cpu_cores as f64) * (1.0 - HOST_RESERVE_FRACTION),
                    mem_gb: ((node.memory_mb as f64) / 1024.0) * (1.0 - HOST_RESERVE_FRACTION),
                    disk_gb: (node.disk_gb as f64) * (1.0 - HOST_RESERVE_FRACTION),
                    gpu_count: node.gpu_count,
                    gpu_assigned: 0,
                    vm_count: 0,
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
            gpu_count: 0,
            gpu_assigned: 0,
            vm_count: 0,
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
                    budget.vm_count += 1;
                    if node
                        .devices
                        .as_ref()
                        .map_or(false, |d| d.gpu_passthrough)
                    {
                        budget.gpu_assigned += 1;
                    }
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

    // Collect all unplaced VMs across all pools
    let mut unplaced: Vec<UnplacedVm> = Vec::new();
    for (pool_idx, pool) in spec.spec.sdi_pools.iter().enumerate() {
        for (node_idx, node) in pool.node_specs.iter().enumerate() {
            if node.host.is_some() {
                continue;
            }
            unplaced.push(UnplacedVm {
                pool_idx,
                node_idx,
                vm_name: node.node_name.clone(),
                pool_name: pool.pool_name.clone(),
                cpu: node.cpu as f64,
                mem_gb: node.mem_gb as f64,
                disk_gb: node.disk_gb as f64,
                needs_gpu: node
                    .devices
                    .as_ref()
                    .map_or(false, |d| d.gpu_passthrough),
                candidate_hosts: pool.placement.hosts.clone(),
                spread: pool.placement.spread,
            });
        }
    }

    // FFD: Sort unplaced VMs by resource demand, largest first
    unplaced.sort_by(|a, b| {
        b.demand_score()
            .partial_cmp(&a.demand_score())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Track which bare-metal hosts have VMs from each pool (for same-pool anti-affinity)
    let mut pool_host_counts: HashMap<String, HashMap<String, usize>> = HashMap::new();
    // Initialize with explicitly placed VMs
    for a in &assignments {
        *pool_host_counts
            .entry(a.pool_name.clone())
            .or_default()
            .entry(a.host.clone())
            .or_insert(0) += 1;
    }

    // Second pass: place each unplaced VM using FFD bin-packing
    for vm in &unplaced {
        let mut best_host: Option<String> = None;
        let mut best_score: f64 = f64::NEG_INFINITY;

        for name in baremetal_names {
            // Enforce pool-level host constraint
            if !vm.candidate_hosts.is_empty() && !vm.candidate_hosts.contains(name) {
                continue;
            }

            let budget = match budgets.get(name) {
                Some(b) => b,
                None => continue,
            };

            // Hard constraint: must have enough resources
            if budget.cpu < vm.cpu || budget.mem_gb < vm.mem_gb || budget.disk_gb < vm.disk_gb {
                continue;
            }

            // Hard constraint: GPU if needed
            if vm.needs_gpu && budget.gpu_count <= budget.gpu_assigned {
                continue;
            }

            // Compute remaining headroom score after placing this VM
            let remaining = NodeBudget {
                cpu: budget.cpu - vm.cpu,
                mem_gb: budget.mem_gb - vm.mem_gb,
                disk_gb: budget.disk_gb - vm.disk_gb,
                gpu_count: budget.gpu_count,
                gpu_assigned: budget.gpu_assigned,
                vm_count: budget.vm_count,
            };
            let mut score = remaining.score();

            // Soft anti-affinity: penalize nodes already hosting VMs from this pool
            if vm.spread {
                let pool_count = pool_host_counts
                    .get(&vm.pool_name)
                    .and_then(|m| m.get(name))
                    .copied()
                    .unwrap_or(0);
                if pool_count > 0 {
                    // Apply penalty for each existing VM from same pool
                    for _ in 0..pool_count {
                        score *= ANTI_AFFINITY_FACTOR;
                    }
                }
            }

            // Cross-pool VM count penalty: prefer less-loaded hosts globally
            if budget.vm_count > 0 {
                for _ in 0..budget.vm_count {
                    score *= CROSS_POOL_VM_PENALTY;
                }
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
                    budget.cpu -= vm.cpu;
                    budget.mem_gb -= vm.mem_gb;
                    budget.disk_gb -= vm.disk_gb;
                    budget.vm_count += 1;
                    if vm.needs_gpu {
                        budget.gpu_assigned += 1;
                    }
                }
                *pool_host_counts
                    .entry(vm.pool_name.clone())
                    .or_default()
                    .entry(host.clone())
                    .or_insert(0) += 1;

                assignments.push(PlacementAssignment {
                    vm_name: vm.vm_name.clone(),
                    pool_name: vm.pool_name.clone(),
                    host,
                    was_auto: true,
                });
            }
            None => {
                return Err(format!(
                    "Cannot place VM '{}' (pool '{}', requires {}cpu/{}GB mem/{}GB disk{}): \
                     no bare-metal node has sufficient remaining resources",
                    vm.vm_name,
                    vm.pool_name,
                    vm.cpu,
                    vm.mem_gb,
                    vm.disk_gb,
                    if vm.needs_gpu { " + GPU" } else { "" },
                ));
            }
        }
    }

    // Apply assignments back to spec (mutate in-place)
    for vm in &unplaced {
        let assignment = assignments
            .iter()
            .find(|a| a.vm_name == vm.vm_name && a.was_auto)
            .expect("all unplaced VMs should have assignments at this point");
        spec.spec.sdi_pools[vm.pool_idx].node_specs[vm.node_idx].host =
            Some(assignment.host.clone());
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

/// Format placement plan with per-host utilization summary.
pub fn format_placement_detail(
    plan: &PlacementPlan,
    pool_summary: &ResourcePoolSummary,
    baremetal_names: &[String],
) -> String {
    let mut out = format_placement_table(plan);

    // Compute per-host VM counts
    let mut host_vms: HashMap<String, usize> = HashMap::new();
    for a in &plan.assignments {
        *host_vms.entry(a.host.clone()).or_default() += 1;
    }

    out.push_str("\n╔══════════════════════════════════════════════════════════════╗\n");
    out.push_str("║                  Host Utilization Summary                   ║\n");
    out.push_str("╠══════════════════════════════════════════════════════════════╣\n");

    for name in baremetal_names {
        let vms = host_vms.get(name).copied().unwrap_or(0);
        let node_info = pool_summary.nodes.iter().find(|n| &n.node_name == name);
        let (cpu, mem_gb) = node_info
            .map(|n| (n.cpu_cores, n.memory_mb / 1024))
            .unwrap_or((0, 0));
        out.push_str(&format!(
            "║  {:<14} │ {} VMs │ {} cores │ {} GB RAM             ║\n",
            name, vms, cpu, mem_gb,
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
        make_pool_summary_with_gpu(
            nodes
                .into_iter()
                .map(|(n, c, m, d)| (n, c, m, d, 0))
                .collect(),
        )
    }

    fn make_pool_summary_with_gpu(
        nodes: Vec<(&str, u32, u64, u64, usize)>,
    ) -> ResourcePoolSummary {
        let node_summaries: Vec<NodeResourceSummary> = nodes
            .iter()
            .map(|(name, cpu, mem_mb, disk_gb, gpus)| NodeResourceSummary {
                node_name: name.to_string(),
                cpu_model: "test".to_string(),
                cpu_cores: *cpu,
                cpu_threads: cpu * 2,
                memory_mb: *mem_mb,
                gpu_count: *gpus,
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
            total_gpu_count: node_summaries.iter().map(|n| n.gpu_count).sum(),
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

    fn make_node_spec_with_gpu(
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
            devices: Some(DeviceConfig {
                gpu_passthrough: true,
            }),
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
        // Two VMs, each needing 6 CPU. After 15% host reserve, 8-core host has 6.8 CPU — can fit one.
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
        // VMs should be on different hosts due to cumulative deduction + anti-affinity
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

    // ===== FFD bin-packing tests =====

    /// FFD: Larger VMs should be placed first for better packing.
    /// Given a small VM and a large VM, the large VM gets priority access to
    /// the most resourceful host regardless of YAML ordering.
    #[test]
    fn test_ffd_largest_first() {
        let pool = SdiPool {
            pool_name: "test".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![
                make_node_spec("small-vm", 1, 2, 20, None), // small, listed first
                make_node_spec("large-vm", 6, 12, 80, None), // large, listed second
            ],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        // playbox-0: just enough for large VM (after 15% reserve: 6.8 cpu, 13.3G)
        // playbox-1: can only fit small VM
        let summary = make_pool_summary(vec![
            ("playbox-0", 8, 16384, 200), // after reserve: 6.8cpu, 13.6GB
            ("playbox-1", 4, 4096, 200),  // after reserve: 3.4cpu, 3.4GB
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();

        // large-vm must go to playbox-0 (only host that fits it)
        let large = plan
            .assignments
            .iter()
            .find(|a| a.vm_name == "large-vm")
            .unwrap();
        assert_eq!(
            large.host, "playbox-0",
            "FFD should place large VM on capable host"
        );

        // small-vm gets the remaining host
        let small = plan
            .assignments
            .iter()
            .find(|a| a.vm_name == "small-vm")
            .unwrap();
        assert_eq!(
            small.host, "playbox-1",
            "small VM should go to remaining host"
        );
    }

    /// Pool-level `placement.hosts` constrains which bare-metal nodes a pool's VMs can land on.
    #[test]
    fn test_pool_host_constraint() {
        let pool = SdiPool {
            pool_name: "restricted".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec!["playbox-1".to_string()], // only playbox-1 allowed
                spread: false,
            },
            node_specs: vec![make_node_spec("vm-0", 2, 4, 30, None)],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        // playbox-0 has more resources, but is not in the allowed hosts
        let summary = make_pool_summary(vec![
            ("playbox-0", 16, 65536, 500),
            ("playbox-1", 8, 32768, 500),
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(
            plan.assignments[0].host, "playbox-1",
            "VM must be placed on allowed host only"
        );
    }

    /// GPU-aware placement: VMs requiring GPU passthrough only land on GPU-equipped hosts.
    #[test]
    fn test_gpu_placement() {
        let pool = SdiPool {
            pool_name: "gpu-pool".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![make_node_spec_with_gpu("gpu-vm", 4, 8, 60, None)],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary_with_gpu(vec![
            ("playbox-0", 16, 65536, 500, 0), // no GPU
            ("playbox-1", 8, 32768, 500, 2),  // has GPUs
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(
            plan.assignments[0].host, "playbox-1",
            "GPU VM must land on GPU-equipped host"
        );
    }

    /// Cross-pool anti-affinity: VMs from different pools should still spread
    /// across hosts when resources allow.
    #[test]
    fn test_cross_pool_spreading() {
        let pool_a = SdiPool {
            pool_name: "pool-a".to_string(),
            purpose: "mgmt".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![make_node_spec("vm-a", 2, 4, 30, None)],
        };
        let pool_b = SdiPool {
            pool_name: "pool-b".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![make_node_spec("vm-b", 2, 4, 30, None)],
        };
        let mut spec = make_sdi_spec(vec![pool_a, pool_b]);
        // Equal resources on both
        let summary = make_pool_summary(vec![
            ("playbox-0", 16, 65536, 500),
            ("playbox-1", 16, 65536, 500),
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        // Cross-pool penalty should cause them to land on different hosts
        assert_ne!(
            plan.assignments[0].host, plan.assignments[1].host,
            "cross-pool VMs should spread across hosts"
        );
    }

    /// Host reserve deduction: A node with exactly the VM's needs should fail
    /// placement due to 15% host reserve.
    #[test]
    fn test_host_reserve_prevents_overcommit() {
        let pool = SdiPool {
            pool_name: "tight".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            // VM needs exactly 8 CPU, but 8 * 0.85 = 6.8 available after reserve
            node_specs: vec![make_node_spec("vm-tight", 8, 4, 30, None)],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![("playbox-0", 8, 32768, 500)]);
        let names = vec!["playbox-0".to_string()];

        let result = resolve_placement(&mut spec, &summary, &names);
        assert!(
            result.is_err(),
            "8 vCPU VM should not fit on 8-core host with 15% reserve"
        );
    }

    // ===== Production scenario tests =====

    /// Validate the actual 6-VM HA production spec against real playbox hardware.
    /// Ensures all 6 VMs are assigned with correct distribution and no over-commit.
    ///
    /// Production layout (sdi-specs.yaml):
    ///   Tower pool (HA mgmt): tower-cp-0 → playbox-0, tower-cp-1 → playbox-1, tower-cp-2 → playbox-2
    ///   Sandbox pool (workload): sandbox-cp-0 → playbox-0, sandbox-worker-0 → playbox-1, sandbox-worker-1 → playbox-2
    ///
    /// Each of playbox-0/1/2 hosts exactly 2 VMs (4+4=8 CPU, 6+8=14 GB RAM).
    /// playbox-3 is unused (inaccessible).
    #[test]
    fn test_production_6vm_ha_placement_all_explicit() {
        // Tower: 3 CPs spread across playbox-0/1/2
        let tower = SdiPool {
            pool_name: "tower".to_string(),
            purpose: "management".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("tower-cp-0", 4, 6, 30, Some("playbox-0")),
                make_node_spec("tower-cp-1", 4, 6, 30, Some("playbox-1")),
                make_node_spec("tower-cp-2", 4, 6, 30, Some("playbox-2")),
            ],
        };
        // Sandbox: 1 CP + 2 workers spread across playbox-0/1/2
        let sandbox = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("sandbox-cp-0", 4, 8, 60, Some("playbox-0")),
                make_node_spec("sandbox-worker-0", 4, 8, 60, Some("playbox-1")),
                make_node_spec("sandbox-worker-1", 4, 8, 60, Some("playbox-2")),
            ],
        };

        let mut spec = make_sdi_spec(vec![tower, sandbox]);

        // Real playbox hardware (from resource-pool-summary.json)
        let summary = make_pool_summary(vec![
            ("playbox-0", 8, 15898, 351),    // 8C, ~15.5GB, 351GB
            ("playbox-1", 8, 15897, 351),    // 8C, ~15.5GB, 351GB
            ("playbox-2", 8, 15897, 351),    // 8C, ~15.5GB, 351GB
            ("playbox-3", 16, 257656, 4657), // 16C, ~251GB, 4657GB (unused)
        ]);
        let names = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
            "playbox-3".to_string(),
        ];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();

        // === Validate all 6 VMs are assigned ===
        assert_eq!(
            plan.assignments.len(),
            6,
            "must have exactly 6 VM assignments"
        );

        // === Validate correct host assignments ===
        let find = |name: &str| -> &PlacementAssignment {
            plan.assignments.iter().find(|a| a.vm_name == name).unwrap()
        };
        assert_eq!(find("tower-cp-0").host, "playbox-0");
        assert_eq!(find("tower-cp-1").host, "playbox-1");
        assert_eq!(find("tower-cp-2").host, "playbox-2");
        assert_eq!(find("sandbox-cp-0").host, "playbox-0");
        assert_eq!(find("sandbox-worker-0").host, "playbox-1");
        assert_eq!(find("sandbox-worker-1").host, "playbox-2");

        // === Validate all are explicit (not auto-placed) ===
        for a in &plan.assignments {
            assert!(
                !a.was_auto,
                "VM {} should be explicitly placed, not auto",
                a.vm_name
            );
        }

        // === Validate no node is over-committed ===
        let mut node_cpu: HashMap<String, f64> = HashMap::new();
        let mut node_mem: HashMap<String, f64> = HashMap::new();
        let mut node_disk: HashMap<String, f64> = HashMap::new();

        for pool in &spec.spec.sdi_pools {
            for node in &pool.node_specs {
                let host = node.host.as_ref().unwrap();
                *node_cpu.entry(host.clone()).or_default() += node.cpu as f64;
                *node_mem.entry(host.clone()).or_default() += node.mem_gb as f64;
                *node_disk.entry(host.clone()).or_default() += node.disk_gb as f64;
            }
        }

        for node_summary in &summary.nodes {
            let name = &node_summary.node_name;
            let total_cpu = node_cpu.get(name).copied().unwrap_or(0.0);
            let total_mem = node_mem.get(name).copied().unwrap_or(0.0);
            let total_disk = node_disk.get(name).copied().unwrap_or(0.0);
            let avail_mem_gb = node_summary.memory_mb as f64 / 1024.0;

            assert!(
                total_cpu <= node_summary.cpu_cores as f64,
                "Node {} over-committed on CPU: {} allocated > {} available",
                name,
                total_cpu,
                node_summary.cpu_cores
            );
            assert!(
                total_mem <= avail_mem_gb,
                "Node {} over-committed on memory: {} GB allocated > {:.1} GB available",
                name,
                total_mem,
                avail_mem_gb
            );
            assert!(
                total_disk <= node_summary.disk_gb as f64,
                "Node {} over-committed on disk: {} GB allocated > {} GB available",
                name,
                total_disk,
                node_summary.disk_gb
            );
        }

        // === Validate distribution: each of playbox-0/1/2 has exactly 2 VMs ===
        let mut vm_count: HashMap<String, usize> = HashMap::new();
        for a in &plan.assignments {
            *vm_count.entry(a.host.clone()).or_default() += 1;
        }
        assert_eq!(
            vm_count.get("playbox-0"),
            Some(&2),
            "playbox-0 should host 2 VMs"
        );
        assert_eq!(
            vm_count.get("playbox-1"),
            Some(&2),
            "playbox-1 should host 2 VMs"
        );
        assert_eq!(
            vm_count.get("playbox-2"),
            Some(&2),
            "playbox-2 should host 2 VMs"
        );
        assert_eq!(
            vm_count.get("playbox-3"),
            None,
            "playbox-3 should host 0 VMs (inaccessible)"
        );
    }

    /// Validate that auto-placement of 6 VMs across 3 nodes (playbox-3 excluded)
    /// correctly reports insufficient resources with host reserve.
    /// With 15% host reserve, each 8-core node has 6.8 CPU available — can only fit
    /// one 4-CPU VM. 3 nodes = 3 VMs max, but we need 6.
    #[test]
    fn test_production_6vm_3_nodes_insufficient_with_reserve() {
        let tower = SdiPool {
            pool_name: "tower".to_string(),
            purpose: "management".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("tower-cp-0", 4, 6, 30, None),
                make_node_spec("tower-cp-1", 4, 6, 30, None),
                make_node_spec("tower-cp-2", 4, 6, 30, None),
            ],
        };
        let sandbox = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("sandbox-cp-0", 4, 8, 60, None),
                make_node_spec("sandbox-worker-0", 4, 8, 60, None),
                make_node_spec("sandbox-worker-1", 4, 8, 60, None),
            ],
        };

        let mut spec = make_sdi_spec(vec![tower, sandbox]);

        // Only 3 nodes with 8 cores each — after 15% reserve = 6.8 CPU per node
        // 6 VMs × 4 CPU = 24 CPU needed, 3 × 6.8 = 20.4 available → insufficient
        let summary = make_pool_summary(vec![
            ("playbox-0", 8, 15898, 351),
            ("playbox-1", 8, 15897, 351),
            ("playbox-2", 8, 15897, 351),
        ]);
        let names = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
        ];

        let result = resolve_placement(&mut spec, &summary, &names);
        assert!(
            result.is_err(),
            "6 VMs of 4 CPU should not fit on 3×8-core nodes with 15% host reserve"
        );
    }

    /// With larger nodes (12 cores), 6 VMs of 4 CPU each should fit across 3 nodes
    /// with proper spread and no overcommit.
    #[test]
    fn test_production_6vm_auto_placement_adequate_nodes() {
        let tower = SdiPool {
            pool_name: "tower".to_string(),
            purpose: "management".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("tower-cp-0", 4, 6, 30, None),
                make_node_spec("tower-cp-1", 4, 6, 30, None),
                make_node_spec("tower-cp-2", 4, 6, 30, None),
            ],
        };
        let sandbox = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("sandbox-cp-0", 4, 8, 60, None),
                make_node_spec("sandbox-worker-0", 4, 8, 60, None),
                make_node_spec("sandbox-worker-1", 4, 8, 60, None),
            ],
        };

        let mut spec = make_sdi_spec(vec![tower, sandbox]);

        // 3 nodes with 12 cores each — after 15% reserve = 10.2 CPU per node
        // Can fit 2 VMs of 4 CPU each (8 CPU used, 2.2 remaining)
        let summary = make_pool_summary(vec![
            ("playbox-0", 12, 32768, 500),
            ("playbox-1", 12, 32768, 500),
            ("playbox-2", 12, 32768, 500),
        ]);
        let names = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
        ];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(plan.assignments.len(), 6, "all 6 VMs must be assigned");

        // Tower CPs should spread across 3 different hosts
        let tower_hosts: Vec<&str> = plan
            .assignments
            .iter()
            .filter(|a| a.pool_name == "tower")
            .map(|a| a.host.as_str())
            .collect();
        let unique_tower: std::collections::HashSet<&&str> = tower_hosts.iter().collect();
        assert_eq!(
            unique_tower.len(),
            3,
            "Tower CPs should spread across 3 hosts"
        );

        // Each host should have exactly 2 VMs (balanced)
        let mut host_counts: HashMap<String, usize> = HashMap::new();
        for a in &plan.assignments {
            *host_counts.entry(a.host.clone()).or_default() += 1;
        }
        for (host, count) in &host_counts {
            assert_eq!(
                *count, 2,
                "host {} should have exactly 2 VMs, got {}",
                host, count
            );
        }
    }

    /// The real scenario: 6 VMs across 4 nodes (playbox-0/1/2/3) with auto-placement.
    /// playbox-3 has massive resources (16 cores, 256GB RAM) — the FFD bin-packer
    /// should leverage it, spreading VMs across all 4 hosts when possible.
    #[test]
    fn test_6_vms_across_4_nodes_real_scenario() {
        let tower = SdiPool {
            pool_name: "tower".to_string(),
            purpose: "management".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("tower-cp-0", 4, 6, 30, None),
                make_node_spec("tower-cp-1", 4, 6, 30, None),
                make_node_spec("tower-cp-2", 4, 6, 30, None),
            ],
        };
        let sandbox = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("sandbox-cp-0", 4, 8, 60, None),
                make_node_spec("sandbox-worker-0", 4, 8, 60, None),
                make_node_spec("sandbox-worker-1", 4, 8, 60, None),
            ],
        };

        let mut spec = make_sdi_spec(vec![tower, sandbox]);

        // Real hardware: playbox-0/1/2 = 8 cores, 16GB; playbox-3 = 16 cores, 256GB
        let summary = make_pool_summary(vec![
            ("playbox-0", 8, 16384, 351),
            ("playbox-1", 8, 16384, 351),
            ("playbox-2", 8, 16384, 351),
            ("playbox-3", 16, 262144, 4657),
        ]);
        let names = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
            "playbox-3".to_string(),
        ];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(plan.assignments.len(), 6, "all 6 VMs must be placed");

        // All VMs must have auto placement
        assert!(
            plan.assignments.iter().all(|a| a.was_auto),
            "all VMs should be auto-placed"
        );

        // Verify spread: VMs should be distributed across hosts.
        // With soft anti-affinity only, the massive playbox-3 (16C/256GB vs 8C/16GB)
        // may attract more VMs due to its much higher headroom score.
        let mut host_counts: HashMap<String, usize> = HashMap::new();
        for a in &plan.assignments {
            *host_counts.entry(a.host.clone()).or_insert(0) += 1;
        }

        // At least 3 different hosts should be used (soft spread across 4 nodes)
        assert!(
            host_counts.len() >= 3,
            "should use at least 3 different hosts, used: {:?}",
            host_counts
        );

        // No single host should have more than 3 VMs (soft constraint with uneven resources)
        for (host, count) in &host_counts {
            assert!(
                *count <= 3,
                "host {} has {} VMs, expected at most 3 with soft anti-affinity",
                host,
                count
            );
        }

        // Tower CPs should be on different hosts (HA requirement via spread)
        let tower_hosts: Vec<&str> = plan
            .assignments
            .iter()
            .filter(|a| a.pool_name == "tower")
            .map(|a| a.host.as_str())
            .collect();
        let unique_tower: std::collections::HashSet<&&str> = tower_hosts.iter().collect();
        assert_eq!(
            unique_tower.len(),
            3,
            "tower HA CPs must be on 3 different hosts, got: {:?}",
            tower_hosts
        );
    }

    /// Test with partial explicit placement — algorithm must pack remaining VMs
    /// around the explicit constraints without overcommitting.
    #[test]
    fn test_6_vms_with_partial_explicit_placement() {
        let tower = SdiPool {
            pool_name: "tower".to_string(),
            purpose: "management".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("tower-cp-0", 4, 6, 30, Some("playbox-0")),
                make_node_spec("tower-cp-1", 4, 6, 30, None),
                make_node_spec("tower-cp-2", 4, 6, 30, None),
            ],
        };
        let sandbox = SdiPool {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig {
                hosts: vec![],
                spread: true,
            },
            node_specs: vec![
                make_node_spec("sandbox-cp-0", 4, 8, 60, Some("playbox-0")),
                make_node_spec("sandbox-worker-0", 4, 8, 60, None),
                make_node_spec("sandbox-worker-1", 4, 8, 60, None),
            ],
        };

        let mut spec = make_sdi_spec(vec![tower, sandbox]);
        let summary = make_pool_summary(vec![
            ("playbox-0", 8, 16384, 351),
            ("playbox-1", 8, 16384, 351),
            ("playbox-2", 8, 16384, 351),
            ("playbox-3", 16, 262144, 4657),
        ]);
        let names = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
            "playbox-3".to_string(),
        ];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(plan.assignments.len(), 6);

        // Explicit placements preserved
        let explicit: Vec<_> = plan.assignments.iter().filter(|a| !a.was_auto).collect();
        assert_eq!(explicit.len(), 2);
        assert!(explicit.iter().all(|a| a.host == "playbox-0"));

        // Auto VMs should avoid playbox-0 (already loaded with 2 VMs consuming 8 CPU)
        let auto_on_pb0 = plan
            .assignments
            .iter()
            .filter(|a| a.was_auto && a.host == "playbox-0")
            .count();
        assert_eq!(
            auto_on_pb0, 0,
            "auto-placed VMs should avoid already-loaded playbox-0"
        );
    }

    /// Placement with an unreachable node: if a node has 0 resources (facts unavailable),
    /// VMs should not be placed there unless no other option.
    #[test]
    fn test_unreachable_node_avoided() {
        let pool = SdiPool {
            pool_name: "test".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![make_node_spec("vm-0", 2, 4, 30, None)],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        // playbox-3 has facts, playbox-missing does not (will get zero budget)
        let summary = make_pool_summary(vec![("playbox-3", 16, 262144, 4657)]);
        let names = vec!["playbox-missing".to_string(), "playbox-3".to_string()];

        let plan = resolve_placement(&mut spec, &summary, &names).unwrap();
        assert_eq!(
            plan.assignments[0].host, "playbox-3",
            "should avoid node with zero budget (no facts)"
        );
    }

    /// Verify spec is mutated in-place after placement.
    #[test]
    fn test_spec_mutation_after_placement() {
        let pool = SdiPool {
            pool_name: "test".to_string(),
            purpose: "workload".to_string(),
            placement: PlacementConfig::default(),
            node_specs: vec![
                make_node_spec("vm-0", 2, 4, 30, None),
                make_node_spec("vm-1", 2, 4, 30, None),
            ],
        };
        let mut spec = make_sdi_spec(vec![pool]);
        let summary = make_pool_summary(vec![
            ("playbox-0", 16, 65536, 500),
            ("playbox-1", 16, 65536, 500),
        ]);
        let names = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let _ = resolve_placement(&mut spec, &summary, &names).unwrap();

        // All nodes should now have hosts assigned
        for node in &spec.spec.sdi_pools[0].node_specs {
            assert!(
                node.host.is_some(),
                "node {} should have host assigned after placement",
                node.node_name
            );
        }
    }
}
