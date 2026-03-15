use crate::core::resource_pool::NodeResourceSummary;
use crate::models::cluster::ClusterDef;
use crate::models::sdi::*;
use serde::{Deserialize, Serialize};

// ─── Placement Tier ───

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlacementTier {
    Minimal,  // 1 VM: CP+worker combined
    Standard, // 1 CP + 2 workers
    Ha,       // 3 CPs (etcd quorum) + 2 workers
}

impl std::fmt::Display for PlacementTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlacementTier::Minimal => write!(f, "Minimal (1 VM)"),
            PlacementTier::Standard => write!(f, "Standard (1 CP + 2 Workers)"),
            PlacementTier::Ha => write!(f, "HA (3 CPs + 2 Workers)"),
        }
    }
}

// ─── Resource Budget ───

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceBudget {
    pub cpu_millicores: u32,
    pub memory_mb: u32,
    pub disk_mb: u32,
}

impl ResourceBudget {
    pub const fn new(cpu_millicores: u32, memory_mb: u32, disk_mb: u32) -> Self {
        Self {
            cpu_millicores,
            memory_mb,
            disk_mb,
        }
    }

    pub fn add(&self, other: &ResourceBudget) -> ResourceBudget {
        ResourceBudget {
            cpu_millicores: self.cpu_millicores + other.cpu_millicores,
            memory_mb: self.memory_mb + other.memory_mb,
            disk_mb: self.disk_mb + other.disk_mb,
        }
    }
}

// ─── Component Budgets (the estimation logic in code) ───

/// Base OS + kubelet overhead per VM
const VM_BASE_OS: ResourceBudget = ResourceBudget::new(200, 256, 2048);

/// Kubernetes control plane (apiserver + scheduler + controller-manager + etcd)
const K8S_CONTROL_PLANE: ResourceBudget = ResourceBudget::new(500, 1024, 4096);

/// Per-component resource budgets derived from Helm values and observed usage
const COMPONENT_BUDGETS: &[(&str, ResourceBudget)] = &[
    // Common (every cluster)
    ("cilium-agent", ResourceBudget::new(200, 256, 512)),
    ("cilium-operator", ResourceBudget::new(100, 128, 256)),
    ("coredns", ResourceBudget::new(100, 128, 256)),
    // Management-only
    ("argocd", ResourceBudget::new(500, 1024, 2048)),
    ("cert-manager", ResourceBudget::new(100, 128, 256)),
    ("kyverno", ResourceBudget::new(200, 384, 512)),
    ("keycloak", ResourceBudget::new(500, 512, 1024)),
    ("cloudflared-tunnel", ResourceBudget::new(50, 64, 128)),
    // Workload-only
    ("local-path-provisioner", ResourceBudget::new(50, 64, 128)),
];

/// Components deployed on management clusters
const MANAGEMENT_COMPONENTS: &[&str] = &[
    "cilium-agent",
    "cilium-operator",
    "coredns",
    "argocd",
    "cert-manager",
    "kyverno",
    "keycloak",
    "cloudflared-tunnel",
];

/// Components deployed on workload clusters
const WORKLOAD_COMPONENTS: &[&str] = &[
    "cilium-agent",
    "cilium-operator",
    "coredns",
    "cert-manager",
    "kyverno",
    "local-path-provisioner",
];

/// Fraction of host resources reserved for hypervisor + OS
const HOST_RESERVE_FRACTION: f64 = 0.15;

// ─── Estimation Output ───

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterResourceEstimate {
    pub cluster_name: String,
    pub cluster_role: String,
    pub total: ResourceBudget,
    pub breakdown: Vec<(String, ResourceBudget)>,
}

/// Estimate resource requirements for a single cluster based on its role.
/// Pure function.
pub fn estimate_cluster_resources(cluster: &ClusterDef) -> ClusterResourceEstimate {
    let mut total = VM_BASE_OS.add(&K8S_CONTROL_PLANE);
    let mut breakdown = vec![
        ("base-os".to_string(), VM_BASE_OS.clone()),
        ("k8s-control-plane".to_string(), K8S_CONTROL_PLANE.clone()),
    ];

    let components = if cluster.cluster_role == "management" {
        MANAGEMENT_COMPONENTS
    } else {
        WORKLOAD_COMPONENTS
    };

    for &comp_name in components {
        if let Some((_, budget)) = COMPONENT_BUDGETS
            .iter()
            .find(|(name, _)| *name == comp_name)
        {
            total = total.add(budget);
            breakdown.push((comp_name.to_string(), budget.clone()));
        }
    }

    ClusterResourceEstimate {
        cluster_name: cluster.cluster_name.clone(),
        cluster_role: cluster.cluster_role.clone(),
        total,
        breakdown,
    }
}

// ─── Tier Selection ───

/// Select the best placement tier based on available resources.
/// Pure function.
pub fn select_tier(
    estimates: &[ClusterResourceEstimate],
    hosts: &[NodeResourceSummary],
) -> (PlacementTier, Vec<String>) {
    let mut warnings = Vec::new();

    // Calculate total available resources (after host reserve)
    let total_cpu_cores: u32 = hosts.iter().map(|h| h.cpu_cores).sum();
    let total_mem_mb: u64 = hosts.iter().map(|h| h.memory_mb).sum();
    let avail_cpu_mc = ((total_cpu_cores as f64) * 1000.0 * (1.0 - HOST_RESERVE_FRACTION)) as u64;
    let avail_mem_mb = ((total_mem_mb as f64) * (1.0 - HOST_RESERVE_FRACTION)) as u64;

    // Calculate demand per tier
    let demand_ha = tier_demand(estimates, &PlacementTier::Ha);
    let demand_standard = tier_demand(estimates, &PlacementTier::Standard);
    let demand_minimal = tier_demand(estimates, &PlacementTier::Minimal);

    // Select tier with headroom
    if avail_cpu_mc >= (demand_ha.0 as f64 * 1.2) as u64
        && avail_mem_mb >= (demand_ha.1 as f64 * 1.2) as u64
    {
        (PlacementTier::Ha, warnings)
    } else if avail_cpu_mc >= (demand_standard.0 as f64 * 1.1) as u64
        && avail_mem_mb >= (demand_standard.1 as f64 * 1.1) as u64
    {
        (PlacementTier::Standard, warnings)
    } else if avail_cpu_mc >= demand_minimal.0 && avail_mem_mb >= demand_minimal.1 {
        (PlacementTier::Minimal, warnings)
    } else {
        warnings.push(format!(
            "Resources insufficient even for minimal tier: need {}mc CPU / {}MB RAM, have {}mc / {}MB available",
            demand_minimal.0, demand_minimal.1, avail_cpu_mc, avail_mem_mb
        ));
        (PlacementTier::Minimal, warnings)
    }
}

/// Calculate total demand (cpu_millicores, memory_mb) for a given tier.
fn tier_demand(estimates: &[ClusterResourceEstimate], tier: &PlacementTier) -> (u64, u64) {
    let vm_count = match tier {
        PlacementTier::Minimal => 1u64,
        PlacementTier::Standard => 3,
        PlacementTier::Ha => 5,
    };
    let mut total_cpu: u64 = 0;
    let mut total_mem: u64 = 0;
    for est in estimates {
        // CP gets the full estimate, workers get base + proportional share
        let cp_cpu = est.total.cpu_millicores as u64;
        let cp_mem = est.total.memory_mb as u64;
        match tier {
            PlacementTier::Minimal => {
                total_cpu += cp_cpu;
                total_mem += cp_mem;
            }
            PlacementTier::Standard => {
                // 1 CP + 2 workers (workers get base OS overhead each)
                total_cpu += cp_cpu + (VM_BASE_OS.cpu_millicores as u64 * 2);
                total_mem += cp_mem + (VM_BASE_OS.memory_mb as u64 * 2);
            }
            PlacementTier::Ha => {
                // 3 CPs + 2 workers
                total_cpu += cp_cpu * 3 / 2 + (VM_BASE_OS.cpu_millicores as u64 * 2);
                total_mem += cp_mem * 3 / 2 + (VM_BASE_OS.memory_mb as u64 * 2);
            }
        }
    }
    let _ = vm_count; // used conceptually above
    (total_cpu, total_mem)
}

// ─── VM Placement ───

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlacementPlan {
    pub tier: PlacementTier,
    pub pools: Vec<PlannedPool>,
    pub host_utilization: Vec<HostUtilization>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannedPool {
    pub pool_name: String,
    pub purpose: String,
    pub vms: Vec<PlannedVm>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannedVm {
    pub node_name: String,
    pub host: String,
    pub ip: String,
    pub cpu: u32,
    pub mem_gb: u32,
    pub disk_gb: u32,
    pub roles: Vec<String>,
    pub needs_gpu: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostUtilization {
    pub host_name: String,
    pub total_cpu: u32,
    pub used_cpu: u32,
    pub total_mem_mb: u64,
    pub used_mem_mb: u64,
    pub gpu_count: usize,
    pub gpu_assigned: usize,
}

/// Place VMs onto bare-metal hosts using First-Fit-Decreasing bin-packing.
/// Pure function.
pub fn place_vms(
    estimates: &[ClusterResourceEstimate],
    hosts: &[NodeResourceSummary],
    tier: &PlacementTier,
    base_ip_octet: u8,
) -> PlacementPlan {
    let mut warnings = Vec::new();

    // Build mutable host capacity tracker
    let mut host_caps: Vec<HostCap> = hosts
        .iter()
        .map(|h| HostCap {
            name: h.node_name.clone(),
            avail_cpu: ((h.cpu_cores as f64 * (1.0 - HOST_RESERVE_FRACTION)) as u32).max(1),
            avail_mem_mb: ((h.memory_mb as f64 * (1.0 - HOST_RESERVE_FRACTION)) as u64),
            gpu_count: h.gpu_count,
            gpu_assigned: 0,
        })
        .collect();

    // Expand clusters into concrete VMs
    let mut all_vms: Vec<(String, String, PlannedVm)> = Vec::new(); // (pool_name, purpose, vm)
    let mut ip_octet = base_ip_octet;

    for est in estimates {
        let pool_name = est.cluster_name.clone();
        let purpose = est.cluster_role.clone();
        let prefix = &est.cluster_name;

        match tier {
            PlacementTier::Minimal => {
                let vm = make_vm(
                    &format!("{}-cp-0", prefix),
                    ip_octet,
                    &est.total,
                    vec!["control-plane".into(), "worker".into()],
                    false,
                );
                all_vms.push((pool_name, purpose, vm));
                ip_octet += 10;
            }
            PlacementTier::Standard => {
                // 1 CP
                let cp_vm = make_vm(
                    &format!("{}-cp-0", prefix),
                    ip_octet,
                    &est.total,
                    vec!["control-plane".into(), "etcd".into()],
                    false,
                );
                all_vms.push((pool_name.clone(), purpose.clone(), cp_vm));
                ip_octet += 10;

                // 2 workers with proportional resources
                let worker_budget = ResourceBudget::new(
                    (est.total.cpu_millicores / 2).max(500),
                    (est.total.memory_mb / 2).max(1024),
                    (est.total.disk_mb / 2).max(10240),
                );
                for i in 0..2 {
                    let wvm = make_vm(
                        &format!("{}-w-{}", prefix, i),
                        ip_octet + i as u8,
                        &worker_budget,
                        vec!["worker".into()],
                        false,
                    );
                    all_vms.push((pool_name.clone(), purpose.clone(), wvm));
                }
                ip_octet += 10;
            }
            PlacementTier::Ha => {
                // 3 CPs
                for i in 0..3 {
                    let cp_vm = make_vm(
                        &format!("{}-cp-{}", prefix, i),
                        ip_octet + i as u8,
                        &est.total,
                        vec!["control-plane".into(), "etcd".into()],
                        false,
                    );
                    all_vms.push((pool_name.clone(), purpose.clone(), cp_vm));
                }
                ip_octet += 10;

                // 2 workers
                let worker_budget = ResourceBudget::new(
                    (est.total.cpu_millicores / 2).max(500),
                    (est.total.memory_mb / 2).max(1024),
                    (est.total.disk_mb / 2).max(10240),
                );
                for i in 0..2 {
                    let wvm = make_vm(
                        &format!("{}-w-{}", prefix, i),
                        ip_octet + i as u8,
                        &worker_budget,
                        vec!["worker".into()],
                        false,
                    );
                    all_vms.push((pool_name.clone(), purpose.clone(), wvm));
                }
                ip_octet += 10;
            }
        }
    }

    // Sort VMs by resource demand (FFD: largest first)
    all_vms.sort_by(|a, b| {
        let a_score = a.2.mem_gb as u64 * 1000 + a.2.cpu as u64;
        let b_score = b.2.mem_gb as u64 * 1000 + b.2.cpu as u64;
        b_score.cmp(&a_score)
    });

    // Bin-pack: assign each VM to host with most remaining capacity
    for (_, _, vm) in &mut all_vms {
        let best_host = host_caps
            .iter_mut()
            .filter(|h| h.avail_cpu >= vm.cpu && h.avail_mem_mb >= (vm.mem_gb as u64 * 1024))
            .filter(|h| !vm.needs_gpu || h.gpu_count > h.gpu_assigned)
            .max_by_key(|h| h.avail_mem_mb);

        match best_host {
            Some(host) => {
                vm.host = host.name.clone();
                host.avail_cpu = host.avail_cpu.saturating_sub(vm.cpu);
                host.avail_mem_mb = host.avail_mem_mb.saturating_sub(vm.mem_gb as u64 * 1024);
                if vm.needs_gpu {
                    host.gpu_assigned += 1;
                }
            }
            None => {
                // Fallback: assign to host with most memory (even if overcommit)
                if let Some(host) = host_caps.iter_mut().max_by_key(|h| h.avail_mem_mb) {
                    warnings.push(format!(
                        "VM {} overcommits host {} resources",
                        vm.node_name, host.name
                    ));
                    vm.host = host.name.clone();
                }
            }
        }
    }

    // Group VMs into pools
    let mut pools: Vec<PlannedPool> = Vec::new();
    for (pool_name, purpose, vm) in all_vms {
        if let Some(pool) = pools.iter_mut().find(|p| p.pool_name == pool_name) {
            pool.vms.push(vm);
        } else {
            pools.push(PlannedPool {
                pool_name,
                purpose,
                vms: vec![vm],
            });
        }
    }

    // Build utilization report
    let host_utilization = hosts
        .iter()
        .map(|h| {
            let cap = host_caps.iter().find(|c| c.name == h.node_name);
            let reserved_cpu = ((h.cpu_cores as f64 * (1.0 - HOST_RESERVE_FRACTION)) as u32).max(1);
            let reserved_mem = (h.memory_mb as f64 * (1.0 - HOST_RESERVE_FRACTION)) as u64;
            HostUtilization {
                host_name: h.node_name.clone(),
                total_cpu: reserved_cpu,
                used_cpu: reserved_cpu - cap.map_or(0, |c| c.avail_cpu),
                total_mem_mb: reserved_mem,
                used_mem_mb: reserved_mem - cap.map_or(0, |c| c.avail_mem_mb),
                gpu_count: h.gpu_count,
                gpu_assigned: cap.map_or(0, |c| c.gpu_assigned),
            }
        })
        .collect();

    PlacementPlan {
        tier: tier.clone(),
        pools,
        host_utilization,
        warnings,
    }
}

struct HostCap {
    name: String,
    avail_cpu: u32,
    avail_mem_mb: u64,
    gpu_count: usize,
    gpu_assigned: usize,
}

fn make_vm(
    name: &str,
    ip_last_octet: u8,
    budget: &ResourceBudget,
    roles: Vec<String>,
    needs_gpu: bool,
) -> PlannedVm {
    // Convert millicores to vCPUs (round up, minimum 1)
    let cpu = budget.cpu_millicores.div_ceil(1000).max(1);
    // Convert MB to GB (round up, minimum 2)
    let mem_gb = budget.memory_mb.div_ceil(1024).max(2);
    // Convert MB to GB (round up, minimum 20)
    let disk_gb = budget.disk_mb.div_ceil(1024).max(20);

    PlannedVm {
        node_name: name.to_string(),
        host: String::new(), // assigned during bin-packing
        ip: format!("192.168.88.{}", ip_last_octet),
        cpu,
        mem_gb,
        disk_gb,
        roles,
        needs_gpu,
    }
}

// ─── Conversion: PlacementPlan → SdiSpec ───

/// Convert a PlacementPlan into an SdiSpec ready for YAML serialization.
/// Pure function.
pub fn to_sdi_spec(
    plan: &PlacementPlan,
    network: &NetworkConfig,
    os_image: &OsImageConfig,
    cloud_init: &CloudInitConfig,
) -> SdiSpec {
    let sdi_pools = plan
        .pools
        .iter()
        .map(|pool| {
            let hosts: Vec<String> = pool
                .vms
                .iter()
                .map(|vm| vm.host.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let spread = hosts.len() > 1;

            SdiPool {
                pool_name: pool.pool_name.clone(),
                purpose: pool.purpose.clone(),
                placement: PlacementConfig { hosts, spread },
                node_specs: pool
                    .vms
                    .iter()
                    .map(|vm| NodeSpec {
                        node_name: vm.node_name.clone(),
                        ip: vm.ip.clone(),
                        cpu: vm.cpu,
                        mem_gb: vm.mem_gb,
                        disk_gb: vm.disk_gb,
                        host: Some(vm.host.clone()),
                        roles: vm.roles.clone(),
                        devices: if vm.needs_gpu {
                            Some(DeviceConfig {
                                gpu_passthrough: true,
                            })
                        } else {
                            None
                        },
                    })
                    .collect(),
            }
        })
        .collect();

    SdiSpec {
        resource_pool: ResourcePoolConfig {
            name: "playbox-pool".to_string(),
            network: network.clone(),
        },
        os_image: os_image.clone(),
        cloud_init: cloud_init.clone(),
        spec: SdiPoolsSpec { sdi_pools },
    }
}

// ─── Display Helpers ───

/// Format placement plan as a human-readable summary.
pub fn format_plan_summary(plan: &PlacementPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!("Placement Tier: {}\n\n", plan.tier));

    for pool in &plan.pools {
        out.push_str(&format!("Pool: {} ({})\n", pool.pool_name, pool.purpose));
        for vm in &pool.vms {
            out.push_str(&format!(
                "  {} → {} | {} vCPU, {} GB RAM, {} GB disk | roles: {}\n",
                vm.node_name,
                vm.host,
                vm.cpu,
                vm.mem_gb,
                vm.disk_gb,
                vm.roles.join(", ")
            ));
        }
        out.push('\n');
    }

    out.push_str("Host Utilization:\n");
    for hu in &plan.host_utilization {
        let cpu_pct = if hu.total_cpu > 0 {
            hu.used_cpu * 100 / hu.total_cpu
        } else {
            0
        };
        let mem_pct = if hu.total_mem_mb > 0 {
            hu.used_mem_mb * 100 / hu.total_mem_mb
        } else {
            0
        };
        out.push_str(&format!(
            "  {} | CPU: {}/{} ({}%) | MEM: {}/{}MB ({}%)",
            hu.host_name,
            hu.used_cpu,
            hu.total_cpu,
            cpu_pct,
            hu.used_mem_mb,
            hu.total_mem_mb,
            mem_pct
        ));
        if hu.gpu_count > 0 {
            out.push_str(&format!(" | GPU: {}/{}", hu.gpu_assigned, hu.gpu_count));
        }
        out.push('\n');
    }

    if !plan.warnings.is_empty() {
        out.push_str("\nWarnings:\n");
        for w in &plan.warnings {
            out.push_str(&format!("  ⚠ {}\n", w));
        }
    }

    out
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    fn make_management_cluster() -> ClusterDef {
        ClusterDef {
            cluster_name: "tower".to_string(),
            cluster_mode: crate::models::cluster::ClusterMode::Sdi,
            cluster_sdi_resource_pool: "tower".to_string(),
            baremetal_nodes: vec![],
            cluster_role: "management".to_string(),
            network: crate::models::cluster::ClusterNetwork {
                pod_cidr: "10.233.0.0/16".to_string(),
                service_cidr: "10.96.0.0/16".to_string(),
                dns_domain: "cluster.local".to_string(),
                native_routing_cidr: None,
            },
            cilium: None,
            oidc: None,
            kubespray_extra_vars: None,
            ssh_user: None,
        }
    }

    fn make_workload_cluster() -> ClusterDef {
        let mut c = make_management_cluster();
        c.cluster_name = "sandbox".to_string();
        c.cluster_role = "workload".to_string();
        c.cluster_sdi_resource_pool = "sandbox".to_string();
        c
    }

    fn make_host(name: &str, cores: u32, mem_mb: u64, gpus: usize) -> NodeResourceSummary {
        NodeResourceSummary {
            node_name: name.to_string(),
            cpu_model: "Intel".to_string(),
            cpu_cores: cores,
            cpu_threads: cores * 2,
            memory_mb: mem_mb,
            gpu_count: gpus,
            gpu_models: vec![],
            disk_count: 2,
            disk_gb: 500,
            nic_count: 2,
            kernel_version: "6.8.0".to_string(),
            has_bridge: true,
        }
    }

    #[test]
    fn test_estimate_management_includes_argocd() {
        let cluster = make_management_cluster();
        let est = estimate_cluster_resources(&cluster);
        let has_argocd = est.breakdown.iter().any(|(name, _)| name == "argocd");
        let has_keycloak = est.breakdown.iter().any(|(name, _)| name == "keycloak");
        assert!(has_argocd, "management cluster must include argocd budget");
        assert!(
            has_keycloak,
            "management cluster must include keycloak budget"
        );
    }

    #[test]
    fn test_estimate_workload_no_argocd() {
        let cluster = make_workload_cluster();
        let est = estimate_cluster_resources(&cluster);
        let has_argocd = est.breakdown.iter().any(|(name, _)| name == "argocd");
        let has_local_path = est
            .breakdown
            .iter()
            .any(|(name, _)| name == "local-path-provisioner");
        assert!(
            !has_argocd,
            "workload cluster must NOT include argocd budget"
        );
        assert!(
            has_local_path,
            "workload cluster must include local-path-provisioner"
        );
    }

    #[test]
    fn test_management_budget_larger_than_workload() {
        let mgmt = estimate_cluster_resources(&make_management_cluster());
        let work = estimate_cluster_resources(&make_workload_cluster());
        assert!(
            mgmt.total.memory_mb > work.total.memory_mb,
            "management cluster should need more memory than workload"
        );
    }

    #[test]
    fn test_select_tier_beefy_hosts_returns_ha() {
        let estimates = vec![
            estimate_cluster_resources(&make_management_cluster()),
            estimate_cluster_resources(&make_workload_cluster()),
        ];
        let hosts = vec![
            make_host("h0", 32, 131072, 0),
            make_host("h1", 32, 131072, 0),
            make_host("h2", 32, 131072, 0),
            make_host("h3", 32, 131072, 0),
        ];
        let (tier, warnings) = select_tier(&estimates, &hosts);
        assert_eq!(tier, PlacementTier::Ha);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_select_tier_modest_hosts_returns_standard() {
        let estimates = vec![
            estimate_cluster_resources(&make_management_cluster()),
            estimate_cluster_resources(&make_workload_cluster()),
        ];
        // 2 hosts with 4 cores/6GB — enough for Standard but not HA
        let hosts = vec![make_host("h0", 4, 6144, 0), make_host("h1", 4, 6144, 0)];
        let (tier, _) = select_tier(&estimates, &hosts);
        assert_eq!(tier, PlacementTier::Standard);
    }

    #[test]
    fn test_select_tier_tiny_host_returns_minimal_with_warning() {
        let estimates = vec![estimate_cluster_resources(&make_management_cluster())];
        let hosts = vec![make_host("h0", 2, 2048, 0)];
        let (tier, warnings) = select_tier(&estimates, &hosts);
        assert_eq!(tier, PlacementTier::Minimal);
        assert!(
            !warnings.is_empty(),
            "tiny host should produce resource warning"
        );
    }

    #[test]
    fn test_place_vms_minimal_single_vm_per_cluster() {
        let estimates = vec![
            estimate_cluster_resources(&make_management_cluster()),
            estimate_cluster_resources(&make_workload_cluster()),
        ];
        let hosts = vec![make_host("h0", 8, 16384, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Minimal, 100);
        let total_vms: usize = plan.pools.iter().map(|p| p.vms.len()).sum();
        assert_eq!(total_vms, 2, "minimal tier should produce 1 VM per cluster");
    }

    #[test]
    fn test_place_vms_standard_three_vms_per_cluster() {
        let estimates = vec![estimate_cluster_resources(&make_workload_cluster())];
        let hosts = vec![make_host("h0", 16, 65536, 0), make_host("h1", 16, 65536, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Standard, 100);
        let sandbox_pool = plan
            .pools
            .iter()
            .find(|p| p.pool_name == "sandbox")
            .unwrap();
        assert_eq!(
            sandbox_pool.vms.len(),
            3,
            "standard tier should produce 3 VMs per cluster"
        );
    }

    #[test]
    fn test_place_vms_spreads_across_hosts() {
        let estimates = vec![estimate_cluster_resources(&make_workload_cluster())];
        let hosts = vec![make_host("h0", 16, 65536, 0), make_host("h1", 16, 65536, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Standard, 100);
        let sandbox_pool = plan
            .pools
            .iter()
            .find(|p| p.pool_name == "sandbox")
            .unwrap();
        let unique_hosts: std::collections::HashSet<_> =
            sandbox_pool.vms.iter().map(|v| &v.host).collect();
        assert!(
            unique_hosts.len() > 1,
            "VMs should be spread across multiple hosts"
        );
    }

    #[test]
    fn test_to_sdi_spec_roundtrip() {
        let estimates = vec![estimate_cluster_resources(&make_management_cluster())];
        let hosts = vec![make_host("h0", 8, 16384, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Minimal, 100);

        let network = NetworkConfig {
            management_bridge: "br0".to_string(),
            management_cidr: "192.168.88.0/24".to_string(),
            gateway: "192.168.88.1".to_string(),
            nameservers: vec!["8.8.8.8".to_string()],
        };
        let os_image = OsImageConfig {
            source: "https://example.com/image.img".to_string(),
            format: "qcow2".to_string(),
        };
        let cloud_init = CloudInitConfig {
            ssh_authorized_keys_file: "~/.ssh/id_ed25519.pub".to_string(),
            packages: vec!["curl".to_string()],
        };

        let spec = to_sdi_spec(&plan, &network, &os_image, &cloud_init);
        let yaml = serde_yaml::to_string(&spec).unwrap();
        let parsed: SdiSpec = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.spec.sdi_pools.len(), 1);
        assert_eq!(parsed.spec.sdi_pools[0].pool_name, "tower");
    }

    #[test]
    fn test_component_budgets_coverage() {
        // Verify all referenced components exist in COMPONENT_BUDGETS
        for &comp in MANAGEMENT_COMPONENTS {
            assert!(
                COMPONENT_BUDGETS.iter().any(|(name, _)| *name == comp),
                "management component '{}' missing from COMPONENT_BUDGETS",
                comp
            );
        }
        for &comp in WORKLOAD_COMPONENTS {
            assert!(
                COMPONENT_BUDGETS.iter().any(|(name, _)| *name == comp),
                "workload component '{}' missing from COMPONENT_BUDGETS",
                comp
            );
        }
    }

    // ===== README ↔ Planner Consistency Tests =====

    /// Verify management cluster budget totals match README documented values.
    /// README VM Budget table: Management = 2,450mc CPU / 3,904 MB RAM / 11,136 MB disk
    #[test]
    fn test_readme_management_budget_totals() {
        let est = estimate_cluster_resources(&make_management_cluster());
        assert_eq!(est.total.cpu_millicores, 2450, "management CPU mc");
        assert_eq!(est.total.memory_mb, 3904, "management memory MB");
        assert_eq!(est.total.disk_mb, 11136, "management disk MB");
    }

    /// Verify workload cluster budget totals match README documented values.
    /// README VM Budget table: Workload = 1,450mc CPU / 2,368 MB RAM / 8,064 MB disk
    #[test]
    fn test_readme_workload_budget_totals() {
        let est = estimate_cluster_resources(&make_workload_cluster());
        assert_eq!(est.total.cpu_millicores, 1450, "workload CPU mc");
        assert_eq!(est.total.memory_mb, 2368, "workload memory MB");
        assert_eq!(est.total.disk_mb, 8064, "workload disk MB");
    }

    /// Verify management VM specs after conversion: 3 vCPU / 4 GB / 20 GB disk.
    #[test]
    fn test_readme_management_vm_specs() {
        let estimates = vec![estimate_cluster_resources(&make_management_cluster())];
        let hosts = vec![make_host("h0", 64, 131072, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Minimal, 100);
        let vm = &plan.pools[0].vms[0];
        assert_eq!(vm.cpu, 3, "Tower VM vCPU (README: 3C)");
        assert_eq!(vm.mem_gb, 4, "Tower VM RAM (README: 4G)");
        assert_eq!(vm.disk_gb, 20, "Tower VM disk (README: 20G)");
    }

    /// Verify workload VM specs after conversion: 2 vCPU / 3 GB / 20 GB disk.
    #[test]
    fn test_readme_workload_vm_specs() {
        let estimates = vec![estimate_cluster_resources(&make_workload_cluster())];
        let hosts = vec![make_host("h0", 64, 131072, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Minimal, 100);
        let vm = &plan.pools[0].vms[0];
        assert_eq!(vm.cpu, 2, "Sandbox VM vCPU (README: 2C)");
        assert_eq!(vm.mem_gb, 3, "Sandbox VM RAM (README: 3G)");
        assert_eq!(vm.disk_gb, 20, "Sandbox VM disk (README: 20G)");
    }

    /// README Minimal: Tower-only on 1 node (4+ cores, 8+ GB).
    /// Planner must place Tower VM (3C/4G) without overcommit warnings.
    #[test]
    fn test_readme_minimal_placement_on_minimum_host() {
        let estimates = vec![estimate_cluster_resources(&make_management_cluster())];
        let hosts = vec![make_host("h0", 4, 8192, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Minimal, 100);
        assert!(
            plan.warnings.is_empty(),
            "Minimal on 4C/8G must not warn: {:?}",
            plan.warnings
        );
        assert_eq!(plan.pools[0].vms[0].host, "h0");
    }

    /// README Basic: Tower + Sandbox on 1 node (8+ cores, 16+ GB).
    /// Planner must place both VMs (3C/4G + 2C/3G = 5C/7G) without overcommit.
    #[test]
    fn test_readme_basic_placement_on_minimum_host() {
        let estimates = vec![
            estimate_cluster_resources(&make_management_cluster()),
            estimate_cluster_resources(&make_workload_cluster()),
        ];
        let hosts = vec![make_host("h0", 8, 16384, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Minimal, 100);
        assert!(
            plan.warnings.is_empty(),
            "Basic on 8C/16G must not warn: {:?}",
            plan.warnings
        );
        let total_vms: usize = plan.pools.iter().map(|p| p.vms.len()).sum();
        assert_eq!(total_vms, 2);
    }

    /// README Standard: 2+ nodes, each 8+ cores / 16+ GB.
    /// Planner must place all VMs (Tower 1CP+2W + Sandbox 1CP+2W) without overcommit.
    #[test]
    fn test_readme_standard_placement_on_minimum_hosts() {
        let estimates = vec![
            estimate_cluster_resources(&make_management_cluster()),
            estimate_cluster_resources(&make_workload_cluster()),
        ];
        let hosts = vec![
            make_host("h0", 8, 16384, 0),
            make_host("h1", 8, 16384, 0),
        ];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Standard, 100);
        assert!(
            plan.warnings.is_empty(),
            "Standard on 2×8C/16G must not warn: {:?}",
            plan.warnings
        );
    }

    /// README HA: 4+ nodes, each 8+ cores / 16+ GB.
    /// Planner must place all VMs (Tower 3CP+2W + Sandbox 3CP+2W = 21C/29G) without overcommit.
    #[test]
    fn test_readme_ha_placement_on_minimum_hosts() {
        let estimates = vec![
            estimate_cluster_resources(&make_management_cluster()),
            estimate_cluster_resources(&make_workload_cluster()),
        ];
        let hosts = vec![
            make_host("h0", 8, 16384, 0),
            make_host("h1", 8, 16384, 0),
            make_host("h2", 8, 16384, 0),
            make_host("h3", 8, 16384, 0),
        ];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Ha, 100);
        assert!(
            plan.warnings.is_empty(),
            "HA on 4×8C/16G must not warn: {:?}",
            plan.warnings
        );
    }

    /// 3×8C/16G is NOT enough for HA (need 21 vCPU, available 18 after 15% reserve).
    #[test]
    fn test_readme_ha_insufficient_on_three_8c_hosts() {
        let estimates = vec![
            estimate_cluster_resources(&make_management_cluster()),
            estimate_cluster_resources(&make_workload_cluster()),
        ];
        let hosts = vec![
            make_host("h0", 8, 16384, 0),
            make_host("h1", 8, 16384, 0),
            make_host("h2", 8, 16384, 0),
        ];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Ha, 100);
        assert!(
            !plan.warnings.is_empty(),
            "HA on 3×8C/16G must produce overcommit warnings"
        );
    }

    /// Standard worker VM specs: management worker = 2C/2G, workload worker = 1C/2G.
    #[test]
    fn test_readme_standard_worker_vm_specs() {
        let estimates = vec![
            estimate_cluster_resources(&make_management_cluster()),
            estimate_cluster_resources(&make_workload_cluster()),
        ];
        let hosts = vec![make_host("h0", 64, 131072, 0)];
        let plan = place_vms(&estimates, &hosts, &PlacementTier::Standard, 100);

        let tower_pool = plan.pools.iter().find(|p| p.pool_name == "tower").unwrap();
        let tower_worker = tower_pool
            .vms
            .iter()
            .find(|v| v.roles.contains(&"worker".to_string()))
            .unwrap();
        assert_eq!(tower_worker.cpu, 2, "Tower worker vCPU");
        assert_eq!(tower_worker.mem_gb, 2, "Tower worker RAM GB");

        let sandbox_pool = plan
            .pools
            .iter()
            .find(|p| p.pool_name == "sandbox")
            .unwrap();
        let sandbox_worker = sandbox_pool
            .vms
            .iter()
            .find(|v| v.roles.contains(&"worker".to_string()))
            .unwrap();
        assert_eq!(sandbox_worker.cpu, 1, "Sandbox worker vCPU");
        assert_eq!(sandbox_worker.mem_gb, 2, "Sandbox worker RAM GB");
    }
}
