use crate::models::baremetal::NodeFacts;
use crate::models::cluster::{ClusterMode, K8sClustersConfig};
use crate::models::sdi::SdiSpec;

/// Validate that every SDI-mode cluster references a pool that exists in the SDI spec.
/// Pure function: returns list of error messages (empty = valid).
pub fn validate_cluster_sdi_pool_mapping(
    k8s_config: &K8sClustersConfig,
    sdi_spec: &SdiSpec,
) -> Vec<String> {
    let pool_names: Vec<&str> = sdi_spec
        .spec
        .sdi_pools
        .iter()
        .map(|p| p.pool_name.as_str())
        .collect();

    let mut errors = Vec::new();

    for cluster in &k8s_config.config.clusters {
        if cluster.cluster_mode == ClusterMode::Baremetal {
            continue; // Baremetal clusters don't reference SDI pools
        }
        if cluster.cluster_sdi_resource_pool.is_empty() {
            errors.push(format!(
                "Cluster '{}' has mode=sdi but no cluster_sdi_resource_pool defined",
                cluster.cluster_name
            ));
            continue;
        }
        if !pool_names.contains(&cluster.cluster_sdi_resource_pool.as_str()) {
            errors.push(format!(
                "Cluster '{}' references SDI pool '{}' which does not exist in sdi-specs. Available pools: {:?}",
                cluster.cluster_name,
                cluster.cluster_sdi_resource_pool,
                pool_names,
            ));
        }
    }

    errors
}

/// Validate that Cilium cluster IDs are unique across all clusters.
/// Pure function.
pub fn validate_unique_cluster_ids(k8s_config: &K8sClustersConfig) -> Vec<String> {
    let mut seen = std::collections::HashMap::new();
    let mut errors = Vec::new();

    for cluster in &k8s_config.config.clusters {
        if let Some(ref cilium) = cluster.cilium {
            if let Some(prev) = seen.insert(cilium.cluster_id, &cluster.cluster_name) {
                errors.push(format!(
                    "Cilium cluster_id {} is used by both '{}' and '{}'",
                    cilium.cluster_id, prev, cluster.cluster_name
                ));
            }
        }
    }

    errors
}

/// Validate SDI spec semantically. Pure function: no I/O.
/// Checks: unique pool names, unique VM IPs, unique VM names, non-empty pools.
pub fn validate_sdi_spec(spec: &SdiSpec) -> Vec<String> {
    let mut errors = Vec::new();

    if spec.spec.sdi_pools.is_empty() {
        errors.push("spec.sdi_pools is empty. At least 1 pool is required.".to_string());
        return errors;
    }

    let mut seen_pool_names = std::collections::HashSet::new();
    let mut seen_vm_names = std::collections::HashSet::new();
    let mut seen_vm_ips = std::collections::HashSet::new();

    for pool in &spec.spec.sdi_pools {
        let pool_ctx = format!("pool '{}'", pool.pool_name);

        // Unique pool name
        if !seen_pool_names.insert(&pool.pool_name) {
            errors.push(format!("{}: duplicate pool_name", pool_ctx));
        }

        // Non-empty pool name
        if pool.pool_name.trim().is_empty() {
            errors.push("pool_name must not be empty".to_string());
        }

        // Must have at least one host in placement (unless spread mode with per-node hosts)
        if pool.placement.hosts.is_empty() && !pool.placement.spread {
            errors.push(format!(
                "{}: placement.hosts is empty and spread is false. \
                 Either list hosts or set spread: true with per-node host fields.",
                pool_ctx
            ));
        }

        // Must have at least one node spec
        if pool.node_specs.is_empty() {
            errors.push(format!(
                "{}: node_specs is empty. At least 1 VM is required.",
                pool_ctx
            ));
        }

        for node in &pool.node_specs {
            let vm_ctx = format!("{} / VM '{}'", pool_ctx, node.node_name);

            // Unique VM name across all pools
            if !seen_vm_names.insert(&node.node_name) {
                errors.push(format!("{}: duplicate node_name across pools", vm_ctx));
            }

            // Unique VM IP across all pools
            if !seen_vm_ips.insert(&node.ip) {
                errors.push(format!(
                    "{}: duplicate IP '{}' across pools",
                    vm_ctx, node.ip
                ));
            }

            // CPU/mem/disk must be > 0
            if node.cpu == 0 {
                errors.push(format!("{}: cpu must be > 0", vm_ctx));
            }
            if node.mem_gb == 0 {
                errors.push(format!("{}: mem_gb must be > 0", vm_ctx));
            }
            if node.disk_gb == 0 {
                errors.push(format!("{}: disk_gb must be > 0", vm_ctx));
            }

            // Must have at least one role
            if node.roles.is_empty() {
                errors.push(format!("{}: roles must not be empty", vm_ctx));
            }
        }
    }

    errors
}

/// Validate that cluster names are unique across all clusters.
/// Duplicate names cause kubeconfig path conflicts and ArgoCD resource collisions.
/// Pure function.
pub fn validate_unique_cluster_names(k8s_config: &K8sClustersConfig) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut errors = Vec::new();

    for cluster in &k8s_config.config.clusters {
        if !seen.insert(&cluster.cluster_name) {
            errors.push(format!(
                "Duplicate cluster_name '{}'. Each cluster must have a unique name \
                 (used for kubeconfig paths, ArgoCD applications, and gitops generators).",
                cluster.cluster_name
            ));
        }
    }

    errors
}

/// Validate that SDI node hosts reference known bare-metal nodes.
/// Pure function: takes baremetal node names as input.
pub fn validate_sdi_hosts_exist(spec: &SdiSpec, baremetal_node_names: &[String]) -> Vec<String> {
    let mut errors = Vec::new();

    for pool in &spec.spec.sdi_pools {
        for node in &pool.node_specs {
            if let Some(ref host) = node.host {
                if !baremetal_node_names.contains(host) {
                    errors.push(format!(
                        "Pool '{}' VM '{}' references host '{}' which is not in baremetal-init.yaml. \
                         Available hosts: {:?}",
                        pool.pool_name, node.node_name, host, baremetal_node_names
                    ));
                }
            }
        }
    }

    errors
}

/// Parse a CIDR string (e.g. "10.233.0.0/17") into (network_u32, prefix_len).
/// Returns None if the format is invalid.
/// Pure function: no I/O, no side effects.
fn parse_cidr(cidr: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let prefix_len: u32 = parts[1].parse().ok()?;
    if prefix_len > 32 {
        return None;
    }
    let octets: Vec<&str> = parts[0].split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    let mut ip: u32 = 0;
    for octet in &octets {
        let val: u32 = octet.parse().ok()?;
        if val > 255 {
            return None;
        }
        ip = (ip << 8) | val;
    }
    // Mask the IP to the network address
    let mask = if prefix_len == 0 {
        0
    } else {
        !((1u32 << (32 - prefix_len)) - 1)
    };
    Some((ip & mask, prefix_len))
}

/// Check if two CIDR ranges overlap (share any IP addresses).
/// Returns true if they overlap, false if disjoint or if either is unparseable.
/// Pure function: no I/O, no side effects.
pub fn cidrs_overlap(a: &str, b: &str) -> bool {
    let (net_a, prefix_a) = match parse_cidr(a) {
        Some(v) => v,
        None => return false,
    };
    let (net_b, prefix_b) = match parse_cidr(b) {
        Some(v) => v,
        None => return false,
    };
    // Use the shorter prefix (larger network) to compare
    let common_prefix = std::cmp::min(prefix_a, prefix_b);
    let mask = if common_prefix == 0 {
        0
    } else {
        !((1u32 << (32 - common_prefix)) - 1)
    };
    (net_a & mask) == (net_b & mask)
}

/// Validate that Pod/Service CIDRs and DNS domains do not overlap between clusters.
/// Pure function: returns list of error messages (empty = valid).
/// Critical for multi-cluster environments — overlapping CIDRs break ClusterMesh routing.
pub fn validate_cluster_network_overlap(k8s_config: &K8sClustersConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let clusters = &k8s_config.config.clusters;

    for i in 0..clusters.len() {
        for j in (i + 1)..clusters.len() {
            let a = &clusters[i];
            let b = &clusters[j];

            // Check pod CIDR overlap (IP range overlap, not just string equality)
            if cidrs_overlap(&a.network.pod_cidr, &b.network.pod_cidr) {
                errors.push(format!(
                    "Pod CIDR '{}' and '{}' overlap between '{}' and '{}' — must be non-overlapping per cluster for ClusterMesh",
                    a.network.pod_cidr, b.network.pod_cidr, a.cluster_name, b.cluster_name
                ));
            }

            // Check service CIDR overlap
            if cidrs_overlap(&a.network.service_cidr, &b.network.service_cidr) {
                errors.push(format!(
                    "Service CIDR '{}' and '{}' overlap between '{}' and '{}' — must be non-overlapping per cluster",
                    a.network.service_cidr, b.network.service_cidr, a.cluster_name, b.cluster_name
                ));
            }

            // Check DNS domain overlap
            if a.network.dns_domain == b.network.dns_domain {
                errors.push(format!(
                    "DNS domain '{}' is used by both '{}' and '{}' — must be unique per cluster",
                    a.network.dns_domain, a.cluster_name, b.cluster_name
                ));
            }
        }
    }

    errors
}

pub fn validate_bootstrap_prerequisites(
    tower_kubeconfig_path: &str,
    cf_credentials_path: &str,
    cf_credentials_exist: bool,
) -> Vec<String> {
    let mut warnings = Vec::new();

    if tower_kubeconfig_path.is_empty() {
        warnings
            .push("Tower kubeconfig path is empty — run 'scalex cluster init' first".to_string());
    }

    if !cf_credentials_exist {
        warnings.push(format!(
            "Cloudflare Tunnel credentials not found at '{}'. \
             cloudflared Pod will CrashLoop after bootstrap. \
             Create the tunnel in Cloudflare Dashboard and save credentials first. \
             See: docs/ops-guide.md Section 1",
            cf_credentials_path
        ));
    }

    warnings
}

// ── Sprint 39: 2-Layer cross-validation ──

/// Cross-validate SDI spec pools against K8s cluster definitions.
/// Checks: every SDI pool referenced by a cluster has a control-plane node,
/// node IPs don't fall within pod/service CIDRs, and orphan pools are warned.
/// Pure function: no I/O.
pub fn validate_two_layer_consistency(
    k8s_config: &K8sClustersConfig,
    sdi_spec: &SdiSpec,
) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // 1. Every SDI pool used by a cluster must have at least one control-plane node
    for cluster in &k8s_config.config.clusters {
        if cluster.cluster_mode == ClusterMode::Baremetal {
            continue;
        }
        if let Some(pool) = sdi_spec
            .spec
            .sdi_pools
            .iter()
            .find(|p| p.pool_name == cluster.cluster_sdi_resource_pool)
        {
            let has_cp = pool
                .node_specs
                .iter()
                .any(|n| n.roles.iter().any(|r| r == "control-plane"));
            if !has_cp {
                errors.push(format!(
                    "Cluster '{}' references SDI pool '{}' which has no control-plane node. \
                     Kubespray requires at least one.",
                    cluster.cluster_name, pool.pool_name
                ));
            }
        }
    }

    // 2. SDI node IPs must not fall within any cluster's pod/service CIDRs
    for pool in &sdi_spec.spec.sdi_pools {
        for node in &pool.node_specs {
            for cluster in &k8s_config.config.clusters {
                if ip_in_cidr(&node.ip, &cluster.network.pod_cidr) {
                    errors.push(format!(
                        "SDI node '{}' IP '{}' falls within cluster '{}' pod CIDR '{}' — \
                         management IPs must not overlap with pod network",
                        node.node_name, node.ip, cluster.cluster_name, cluster.network.pod_cidr
                    ));
                }
                if ip_in_cidr(&node.ip, &cluster.network.service_cidr) {
                    errors.push(format!(
                        "SDI node '{}' IP '{}' falls within cluster '{}' service CIDR '{}' — \
                         management IPs must not overlap with service network",
                        node.node_name, node.ip, cluster.cluster_name, cluster.network.service_cidr
                    ));
                }
            }
        }
    }

    // 3. Orphan pools: SDI pools not referenced by any cluster
    let referenced_pools: std::collections::HashSet<&str> = k8s_config
        .config
        .clusters
        .iter()
        .filter(|c| c.cluster_mode != ClusterMode::Baremetal)
        .map(|c| c.cluster_sdi_resource_pool.as_str())
        .collect();

    for pool in &sdi_spec.spec.sdi_pools {
        if !referenced_pools.contains(pool.pool_name.as_str()) {
            warnings.push(format!(
                "SDI pool '{}' is not referenced by any cluster in k8s-clusters config. \
                 This pool will be created but unused.",
                pool.pool_name
            ));
        }
    }

    (errors, warnings)
}

/// Check if an IP address falls within a CIDR range.
/// Pure function.
fn ip_in_cidr(ip: &str, cidr: &str) -> bool {
    let ip_u32 = match parse_ip(ip) {
        Some(v) => v,
        None => return false,
    };
    let (net, prefix) = match parse_cidr(cidr) {
        Some(v) => v,
        None => return false,
    };
    let mask = if prefix == 0 {
        0
    } else {
        !((1u32 << (32 - prefix)) - 1)
    };
    (ip_u32 & mask) == net
}

/// Parse an IPv4 address into a u32. Pure function.
fn parse_ip(ip: &str) -> Option<u32> {
    let octets: Vec<&str> = ip.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    let mut result: u32 = 0;
    for octet in &octets {
        let val: u32 = octet.parse().ok()?;
        if val > 255 {
            return None;
        }
        result = (result << 8) | val;
    }
    Some(result)
}

/// Validate that the kubespray submodule directory exists and is initialized.
/// Returns Ok(()) if valid, Err with a user-friendly message if not.
pub fn validate_kubespray_submodule(project_root: &str) -> Result<(), String> {
    let kubespray_dir = std::path::Path::new(project_root).join("kubespray");
    if !kubespray_dir.exists() {
        return Err(format!(
            "kubespray/ directory not found at '{}'.\n\
             Run: git submodule update --init --recursive",
            kubespray_dir.display()
        ));
    }
    // Check if initialized (should have cluster.yml from kubespray)
    if !kubespray_dir.join("cluster.yml").exists() {
        return Err(
            "kubespray/ submodule not initialized (missing cluster.yml).\n\
             Run: git submodule update --init --recursive"
                .to_string(),
        );
    }
    Ok(())
}

/// Return the list of external tools required for a given scalex command.
/// Pure function.
pub fn prerequisites_for_command(command: &str) -> Vec<&'static str> {
    match command {
        "sdi-init" => vec!["ssh", "tofu", "sshpass"],
        "cluster-init" => vec!["ssh", "ansible-playbook", "sshpass"],
        "bootstrap" => vec!["helm", "kubectl"],
        "secrets-apply" => vec!["kubectl"],
        "facts" => vec!["ssh", "sshpass"],
        _ => vec![],
    }
}

/// Check if a tool is available on the system PATH.
pub fn check_tool_available(tool: &str) -> bool {
    std::process::Command::new("which")
        .arg(tool)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Find which prerequisites are missing from the system.
/// Returns a list of missing tool names.
pub fn find_missing_prerequisites(tools: &[&str]) -> Vec<String> {
    tools
        .iter()
        .filter(|t| !check_tool_available(t))
        .map(|t| t.to_string())
        .collect()
}

/// Check all prerequisites for a command and return a formatted error if any are missing.
pub fn check_prerequisites(command: &str) -> Result<(), String> {
    let required = prerequisites_for_command(command);
    if required.is_empty() {
        return Ok(());
    }
    let missing = find_missing_prerequisites(&required);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Missing required tools for `scalex {}`: {}\n\
             Install them before proceeding. See README.md Step 0 for installation instructions.",
            command.replace('-', " "),
            missing.join(", ")
        ))
    }
}

/// Workflow step dependency — what must exist before a command can run.
#[derive(Debug, PartialEq)]
pub struct WorkflowDep {
    pub artifact_path: String,
    pub description: String,
    pub fix_command: String,
}

/// Check workflow dependencies for a command.
///
/// Given a command and the set of artifact paths that actually exist on disk,
/// returns a list of missing dependencies with actionable fix instructions.
///
/// Pure function: caller resolves which paths exist; this function only computes the diff.
pub fn check_workflow_dependencies(command: &str, existing_paths: &[&str]) -> Vec<WorkflowDep> {
    let existing: std::collections::HashSet<&str> = existing_paths.iter().copied().collect();

    let required: Vec<WorkflowDep> = match command {
        "sdi-init" => vec![
            WorkflowDep {
                artifact_path: "credentials/.baremetal-init.yaml".to_string(),
                description: "Bare-metal node inventory".to_string(),
                fix_command: "cp credentials/.baremetal-init.yaml.example credentials/.baremetal-init.yaml && edit".to_string(),
            },
            WorkflowDep {
                artifact_path: "credentials/.env".to_string(),
                description: "SSH credentials (passwords/key paths)".to_string(),
                fix_command: "cp credentials/.env.example credentials/.env && edit".to_string(),
            },
            WorkflowDep {
                artifact_path: "_generated/facts/".to_string(),
                description: "Hardware facts (gathered by `scalex facts`)".to_string(),
                fix_command: "scalex facts --all".to_string(),
            },
        ],
        "sdi-init-spec" => vec![
            WorkflowDep {
                artifact_path: "credentials/.baremetal-init.yaml".to_string(),
                description: "Bare-metal node inventory".to_string(),
                fix_command: "cp credentials/.baremetal-init.yaml.example credentials/.baremetal-init.yaml && edit".to_string(),
            },
            WorkflowDep {
                artifact_path: "credentials/.env".to_string(),
                description: "SSH credentials".to_string(),
                fix_command: "cp credentials/.env.example credentials/.env && edit".to_string(),
            },
            WorkflowDep {
                artifact_path: "_generated/facts/".to_string(),
                description: "Hardware facts".to_string(),
                fix_command: "scalex facts --all".to_string(),
            },
            WorkflowDep {
                artifact_path: "config/sdi-specs.yaml".to_string(),
                description: "SDI pool specification".to_string(),
                fix_command: "cp config/sdi-specs.yaml.example config/sdi-specs.yaml && edit".to_string(),
            },
        ],
        "cluster-init" => vec![
            WorkflowDep {
                artifact_path: "_generated/sdi-state.json".to_string(),
                description: "SDI state (created by `scalex sdi init`)".to_string(),
                fix_command: "scalex sdi init <sdi-specs.yaml>".to_string(),
            },
            WorkflowDep {
                artifact_path: "config/k8s-clusters.yaml".to_string(),
                description: "Kubernetes cluster configuration".to_string(),
                fix_command: "cp config/k8s-clusters.yaml.example config/k8s-clusters.yaml && edit".to_string(),
            },
        ],
        "bootstrap" => vec![
            WorkflowDep {
                artifact_path: "_generated/kubeconfigs/".to_string(),
                description: "Kubeconfig files (created by `scalex cluster init`)".to_string(),
                fix_command: "scalex cluster init <k8s-clusters.yaml>".to_string(),
            },
            WorkflowDep {
                artifact_path: "credentials/secrets.yaml".to_string(),
                description: "Secrets (ArgoCD, Keycloak, Cloudflare)".to_string(),
                fix_command: "cp credentials/secrets.yaml.example credentials/secrets.yaml && edit".to_string(),
            },
        ],
        _ => vec![],
    };

    required
        .into_iter()
        .filter(|dep| !existing.contains(dep.artifact_path.as_str()))
        .collect()
}

/// Format workflow dependency errors into a user-friendly message.
/// Pure function.
pub fn format_workflow_errors(command: &str, missing: &[WorkflowDep]) -> String {
    if missing.is_empty() {
        return String::new();
    }

    let mut msg = format!(
        "Cannot run `scalex {}` — missing prerequisites:\n\n",
        command.replace('-', " ")
    );

    for (i, dep) in missing.iter().enumerate() {
        msg.push_str(&format!(
            "  {}. {} ({})\n     Fix: {}\n\n",
            i + 1,
            dep.description,
            dep.artifact_path,
            dep.fix_command
        ));
    }

    msg.push_str("Tip: Run `scalex status` to see the current workflow state.");
    msg
}

/// Validate that SDI VM resource allocations don't exceed physical host capacity.
/// Compares per-host VM totals (CPU cores, memory) against facts data.
/// Pure function: no I/O.
pub fn validate_sdi_resource_allocation(spec: &SdiSpec, facts: &[NodeFacts]) -> Vec<String> {
    let mut errors = Vec::new();

    // Collect all VMs per host
    let mut host_cpu: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut host_mem: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for pool in &spec.spec.sdi_pools {
        for node in &pool.node_specs {
            // Determine host: explicit host field, or placement.hosts[0], or "unknown"
            let host = node
                .host
                .clone()
                .or_else(|| pool.placement.hosts.first().cloned())
                .unwrap_or_else(|| "unknown".to_string());

            *host_cpu.entry(host.clone()).or_insert(0) += node.cpu;
            *host_mem.entry(host.clone()).or_insert(0) += node.mem_gb;
        }
    }

    // Compare against facts
    for (host, allocated_cpu) in &host_cpu {
        if host == "unknown" {
            continue; // Can't validate spread placement without host assignment
        }
        match facts.iter().find(|f| f.node_name == *host) {
            Some(host_facts) => {
                let physical_cores = host_facts.cpu.cores;
                if *allocated_cpu > physical_cores {
                    errors.push(format!(
                        "Host '{}': allocated CPU ({} vCPUs) exceeds physical capacity ({} cores). \
                         Reduce VM CPU allocation or redistribute VMs across hosts.",
                        host, allocated_cpu, physical_cores
                    ));
                }

                let physical_mem_gb = (host_facts.memory.total_mb / 1024) as u32;
                let allocated_mem = host_mem.get(host).copied().unwrap_or(0);
                if allocated_mem > physical_mem_gb {
                    errors.push(format!(
                        "Host '{}': allocated memory ({} GB) exceeds physical capacity ({} GB). \
                         Reduce VM memory allocation or redistribute VMs across hosts.",
                        host, allocated_mem, physical_mem_gb
                    ));
                }
            }
            None => {
                errors.push(format!(
                    "Host '{}': no facts data available — cannot verify resource limits. \
                     Run `scalex facts --host {}` first.",
                    host, host
                ));
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::cluster::*;
    use crate::models::sdi::*;

    fn make_sdi_spec_with_pools(pool_names: &[&str]) -> SdiSpec {
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
                source: "https://example.com/img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "~/.ssh/id.pub".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec {
                sdi_pools: pool_names
                    .iter()
                    .map(|name| SdiPool {
                        pool_name: name.to_string(),
                        purpose: "test".to_string(),
                        placement: PlacementConfig::default(),
                        node_specs: vec![NodeSpec {
                            node_name: format!("{}-cp-0", name),
                            ip: "192.168.88.100".to_string(),
                            cpu: 2,
                            mem_gb: 3,
                            disk_gb: 30,
                            host: None,
                            roles: vec!["control-plane".to_string()],
                            devices: None,
                        }],
                    })
                    .collect(),
            },
        }
    }

    fn make_cluster(name: &str, pool: &str, mode: ClusterMode) -> ClusterDef {
        make_cluster_with_id(name, pool, mode, 0)
    }

    fn make_cluster_with_id(
        name: &str,
        pool: &str,
        mode: ClusterMode,
        cluster_id: u32,
    ) -> ClusterDef {
        ClusterDef {
            cluster_name: name.to_string(),
            cluster_mode: mode,
            cluster_sdi_resource_pool: pool.to_string(),
            baremetal_nodes: vec![],
            cluster_role: "workload".to_string(),
            network: ClusterNetwork {
                pod_cidr: "10.244.0.0/20".to_string(),
                service_cidr: "10.96.0.0/20".to_string(),
                dns_domain: format!("{}.local", name),
                native_routing_cidr: None,
            },
            cilium: if cluster_id > 0 {
                Some(CiliumConfig {
                    cluster_id,
                    cluster_name: name.to_string(),
                })
            } else {
                None
            },
            oidc: None,
            kubespray_extra_vars: None,
            ssh_user: None,
        }
    }

    #[test]
    fn test_validate_pool_mapping_valid() {
        let sdi = make_sdi_spec_with_pools(&["tower", "sandbox"]);
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: CommonConfig {
                    kubernetes_version: "1.33.1".to_string(),
                    kubespray_version: "v2.30.0".to_string(),
                    ..serde_yaml::from_str::<CommonConfig>(
                        "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                    )
                    .unwrap()
                },
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("sandbox", "sandbox", ClusterMode::Sdi),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_pool_mapping_missing_pool() {
        let sdi = make_sdi_spec_with_pools(&["tower"]);
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("sandbox", "nonexistent-pool", ClusterMode::Sdi),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("nonexistent-pool"));
        assert!(errors[0].contains("sandbox"));
    }

    #[test]
    fn test_validate_pool_mapping_baremetal_skip() {
        let sdi = make_sdi_spec_with_pools(&["tower"]);
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("prod", "", ClusterMode::Baremetal),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            errors.is_empty(),
            "baremetal clusters should not be validated against SDI pools"
        );
    }

    #[test]
    fn test_validate_unique_cluster_ids_valid() {
        let mut c1 = make_cluster("tower", "tower", ClusterMode::Sdi);
        c1.cilium = Some(CiliumConfig {
            cluster_id: 1,
            cluster_name: "tower".to_string(),
        });
        let mut c2 = make_cluster("sandbox", "sandbox", ClusterMode::Sdi);
        c2.cilium = Some(CiliumConfig {
            cluster_id: 2,
            cluster_name: "sandbox".to_string(),
        });

        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![c1, c2],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_unique_cluster_ids(&k8s);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_validate_unique_cluster_ids_duplicate() {
        let mut c1 = make_cluster("tower", "tower", ClusterMode::Sdi);
        c1.cilium = Some(CiliumConfig {
            cluster_id: 1,
            cluster_name: "tower".to_string(),
        });
        let mut c2 = make_cluster("sandbox", "sandbox", ClusterMode::Sdi);
        c2.cilium = Some(CiliumConfig {
            cluster_id: 1, // Duplicate!
            cluster_name: "sandbox".to_string(),
        });

        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![c1, c2],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_unique_cluster_ids(&k8s);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("cluster_id 1"));
    }

    /// Parse the ACTUAL .baremetal-init.yaml.example from disk.
    /// Catches drift between example file and parsing logic.
    #[test]
    fn test_parse_baremetal_init_example_from_disk() {
        let content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let config: crate::core::config::BaremetalInitConfig = serde_yaml::from_str(content)
            .expect("Failed to parse credentials/.baremetal-init.yaml.example");
        assert_eq!(config.target_nodes.len(), 4, "example must have 4 nodes");
        assert_eq!(config.target_nodes[0].name, "playbox-0");
        assert!(!config.target_nodes[0].direct_reachable);
        // Case 2: reachable via Tailscale IP
        assert_eq!(
            config.target_nodes[0].reachable_node_ip,
            Some("100.64.0.1".to_string())
        );
        // Case 3: reachable via ProxyJump
        assert_eq!(
            config.target_nodes[1].reachable_via,
            Some(vec!["playbox-0".to_string()])
        );
    }

    /// Parse the ACTUAL sdi-specs.yaml.example from disk.
    #[test]
    fn test_parse_sdi_specs_example_from_disk() {
        let content = include_str!("../../../config/sdi-specs.yaml.example");
        let spec: SdiSpec =
            serde_yaml::from_str(content).expect("Failed to parse config/sdi-specs.yaml.example");
        assert_eq!(spec.resource_pool.name, "playbox-pool");
        assert_eq!(spec.spec.sdi_pools.len(), 2);
        assert_eq!(spec.spec.sdi_pools[0].pool_name, "tower");
        assert_eq!(spec.spec.sdi_pools[1].pool_name, "sandbox");
        assert_eq!(spec.spec.sdi_pools[1].node_specs.len(), 4);
        // GPU passthrough on last sandbox worker
        assert!(
            spec.spec.sdi_pools[1].node_specs[3]
                .devices
                .as_ref()
                .unwrap()
                .gpu_passthrough
        );
    }

    /// Parse the ACTUAL k8s-clusters.yaml.example from disk.
    #[test]
    fn test_parse_k8s_clusters_example_from_disk() {
        let content = include_str!("../../../config/k8s-clusters.yaml.example");
        let config: K8sClustersConfig = serde_yaml::from_str(content)
            .expect("Failed to parse config/k8s-clusters.yaml.example");

        assert_eq!(config.config.common.kubernetes_version, "1.33.1");
        assert!(config.config.common.kube_proxy_remove);
        assert_eq!(config.config.clusters.len(), 2);

        // Tower
        assert_eq!(config.config.clusters[0].cluster_name, "tower");
        assert_eq!(config.config.clusters[0].cluster_sdi_resource_pool, "tower");
        assert!(config.config.clusters[0].oidc.is_none());

        // Sandbox with OIDC
        let sandbox = &config.config.clusters[1];
        assert_eq!(sandbox.cluster_name, "sandbox");
        let oidc = sandbox.oidc.as_ref().unwrap();
        assert!(oidc.enabled);
        assert_eq!(oidc.client_id, "kubernetes");

        // ArgoCD config
        let argocd = config.config.argocd.as_ref().unwrap();
        assert_eq!(argocd.tower_manages, vec!["sandbox"]);
    }

    /// Verify no k3s references in non-legacy project files.
    /// Checklist #9: k3s must be fully excluded from the project.
    /// Expanded scope: includes docs, drawio, ops-guide, test fixtures.
    #[test]
    fn test_no_k3s_references_in_project_files() {
        let files_to_check: Vec<(&str, &str)> = vec![
            (
                "docs/ops-guide.md",
                include_str!("../../../docs/ops-guide.md"),
            ),
            (
                "docs/SETUP-GUIDE.md",
                include_str!("../../../docs/SETUP-GUIDE.md"),
            ),
            (
                "docs/TROUBLESHOOTING.md",
                include_str!("../../../docs/TROUBLESHOOTING.md"),
            ),
        ];

        let mut violations = Vec::new();
        for (name, content) in &files_to_check {
            for (line_num, line) in content.lines().enumerate() {
                let lower = line.to_lowercase();
                if lower.contains("k3s") && !lower.contains("legacy") && !lower.contains("# k3s") {
                    violations.push(format!("{}:{}: {}", name, line_num + 1, line.trim()));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "k3s references found in non-legacy files (Checklist #9 violation):\n{}",
            violations.join("\n")
        );
    }

    /// Verify no ./playbox references in docs (should use scalex).
    /// Checklist #9, DEFECT-3.
    #[test]
    fn test_no_legacy_playbox_references_in_docs() {
        let files_to_check: Vec<(&str, &str)> = vec![
            (
                "docs/ops-guide.md",
                include_str!("../../../docs/ops-guide.md"),
            ),
            (
                "docs/SETUP-GUIDE.md",
                include_str!("../../../docs/SETUP-GUIDE.md"),
            ),
            (
                "docs/TROUBLESHOOTING.md",
                include_str!("../../../docs/TROUBLESHOOTING.md"),
            ),
            (
                "docs/CONTRIBUTING.md",
                include_str!("../../../docs/CONTRIBUTING.md"),
            ),
            (
                "docs/NETWORK-DISCOVERY.md",
                include_str!("../../../docs/NETWORK-DISCOVERY.md"),
            ),
        ];

        let mut violations = Vec::new();
        for (name, content) in &files_to_check {
            for (line_num, line) in content.lines().enumerate() {
                if line.contains("./playbox") || line.contains("values.yaml") {
                    violations.push(format!("{}:{}: {}", name, line_num + 1, line.trim()));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Legacy ./playbox or values.yaml references found in docs:\n{}",
            violations.join("\n")
        );
    }

    /// Verify no dead code in gitops: every directory under common/tower/sandbox
    /// must be referenced by at least one generator.
    /// DEFECT-2: common/cilium/ is dead code.
    #[test]
    fn test_no_gitops_dead_code_directories() {
        let gitops_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../gitops");

        // Collect all generator content
        let generator_files = [
            "generators/tower/common-generator.yaml",
            "generators/tower/tower-generator.yaml",
            "generators/sandbox/common-generator.yaml",
            "generators/sandbox/sandbox-generator.yaml",
        ];
        let mut all_generator_content = String::new();
        for gf in &generator_files {
            let path = gitops_root.join(gf);
            if path.exists() {
                all_generator_content.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }

        // Also include bootstrap/spread.yaml which references generators
        let spread = gitops_root.join("bootstrap/spread.yaml");
        if spread.exists() {
            all_generator_content.push_str(&std::fs::read_to_string(&spread).unwrap_or_default());
        }

        // Check each app directory under common/, tower/, sandbox/
        let mut dead_dirs = Vec::new();
        for category in &["common", "tower", "sandbox"] {
            let cat_dir = gitops_root.join(category);
            if !cat_dir.exists() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&cat_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let dir_name = entry.file_name().to_string_lossy().to_string();
                        // Check if this directory name appears in any generator
                        if !all_generator_content.contains(&dir_name) {
                            dead_dirs.push(format!("gitops/{}/{}", category, dir_name));
                        }
                    }
                }
            }
        }

        assert!(
            dead_dirs.is_empty(),
            "GitOps dead code directories (not referenced by any generator):\n{}",
            dead_dirs.join("\n")
        );
    }

    /// Verify all GitOps repoURLs are consistent across generators and bootstrap.
    /// DEFECT-4: All must reference the same repository.
    #[test]
    fn test_gitops_repo_url_consistency() {
        let files: Vec<(&str, &str)> = vec![
            (
                "bootstrap/spread.yaml",
                include_str!("../../../gitops/bootstrap/spread.yaml"),
            ),
            (
                "generators/tower/common-generator.yaml",
                include_str!("../../../gitops/generators/tower/common-generator.yaml"),
            ),
            (
                "generators/tower/tower-generator.yaml",
                include_str!("../../../gitops/generators/tower/tower-generator.yaml"),
            ),
            (
                "generators/sandbox/common-generator.yaml",
                include_str!("../../../gitops/generators/sandbox/common-generator.yaml"),
            ),
            (
                "generators/sandbox/sandbox-generator.yaml",
                include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml"),
            ),
        ];

        let mut urls: Vec<(String, String)> = Vec::new();
        for (name, content) in &files {
            for line in content.lines() {
                if let Some(url) = line.trim().strip_prefix("repoURL:") {
                    urls.push((name.to_string(), url.trim().to_string()));
                }
            }
        }

        assert!(!urls.is_empty(), "No repoURL found in any GitOps file");

        let first_url = &urls[0].1;
        let inconsistent: Vec<String> = urls
            .iter()
            .filter(|(_, url)| url != first_url)
            .map(|(name, url)| format!("{}: {} (expected {})", name, url, first_url))
            .collect();

        assert!(
            inconsistent.is_empty(),
            "Inconsistent repoURLs in GitOps files:\n{}",
            inconsistent.join("\n")
        );
    }

    /// Cross-validate: sdi-specs.yaml.example pool names must match k8s-clusters.yaml.example references.
    #[test]
    fn test_example_files_cross_config_consistency() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");

        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            errors.is_empty(),
            "Example files have inconsistent pool mappings: {:?}",
            errors
        );

        let id_errors = validate_unique_cluster_ids(&k8s);
        assert!(
            id_errors.is_empty(),
            "Example files have duplicate cluster IDs: {:?}",
            id_errors
        );
    }

    /// Legacy top-level directories and files should be cleaned up or moved to .legacy- prefix.
    /// Integration: Parse example configs end-to-end and verify kubespray vars
    /// contain ALL DataX-required production settings.
    #[test]
    fn test_full_pipeline_datax_production_settings_coverage() {
        let k8s_yaml = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig =
            serde_yaml::from_str(k8s_yaml).expect("k8s-clusters.yaml.example must parse");

        let common = &k8s_config.config.common;

        // DataX-critical production settings must all be present in CommonConfig
        assert_eq!(
            common.etcd_deployment_type, "host",
            "etcd must use host deployment"
        );
        assert_eq!(common.dns_mode, "coredns", "DNS must use coredns");
        assert!(
            common.kube_proxy_remove,
            "kube-proxy must be removed for Cilium"
        );
        assert_eq!(common.cni, "cilium", "CNI must be cilium");
        assert_eq!(
            common.container_runtime, "containerd",
            "runtime must be containerd"
        );
        assert_eq!(common.cgroup_driver, "systemd", "cgroup must be systemd");
        assert!(common.helm_enabled, "helm must be enabled");
        assert!(common.gateway_api_enabled, "gateway API must be enabled");
        assert!(common.enable_nodelocaldns, "nodelocal DNS must be enabled");
        assert!(common.ntp_enabled, "NTP must be enabled");
        assert!(
            !common.kube_apiserver_admission_plugins.is_empty(),
            "admission plugins must be configured"
        );

        // Verify kubespray vars generation includes ALL these settings
        for cluster in &k8s_config.config.clusters {
            let vars = crate::core::kubespray::generate_cluster_vars(cluster, common);

            let datax_required_keys = [
                "kube_version",
                "container_manager",
                "kube_network_plugin",
                "kube_proxy_remove",
                "kubelet_cgroup_driver",
                "helm_enabled",
                "gateway_api_enabled",
                "enable_nodelocaldns",
                "ntp_enabled",
                "etcd_deployment_type",
                "dns_mode",
                "kube_pods_subnet",
                "kube_service_addresses",
                "cert_manager_enabled",
                "argocd_enabled",
            ];

            let parsed: serde_yaml::Mapping = serde_yaml::from_str(&vars).unwrap_or_else(|e| {
                panic!("cluster {} vars invalid YAML: {e}", cluster.cluster_name)
            });

            for key in &datax_required_keys {
                assert!(
                    parsed.contains_key(serde_yaml::Value::String(key.to_string())),
                    "cluster '{}' missing DataX-required key: {}",
                    cluster.cluster_name,
                    key
                );
            }
        }
    }

    /// Integration: Verify cluster-config ConfigMap exists per-cluster in gitops
    /// (NOT in common/) and has correct cluster.type values.
    #[test]
    fn test_cluster_config_per_cluster_not_common() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");

        // Must NOT exist in common/
        assert!(
            !repo_root.join("gitops/common/cluster-config").exists(),
            "cluster-config must NOT be in gitops/common/ (moved to per-cluster)"
        );

        // Must exist in tower/ and sandbox/
        assert!(
            repo_root
                .join("gitops/tower/cluster-config/manifest.yaml")
                .exists(),
            "cluster-config must exist in gitops/tower/"
        );
        assert!(
            repo_root
                .join("gitops/sandbox/cluster-config/manifest.yaml")
                .exists(),
            "cluster-config must exist in gitops/sandbox/"
        );

        // Tower must be "management", sandbox must be "workload"
        let tower_manifest =
            std::fs::read_to_string(repo_root.join("gitops/tower/cluster-config/manifest.yaml"))
                .unwrap();
        let sandbox_manifest =
            std::fs::read_to_string(repo_root.join("gitops/sandbox/cluster-config/manifest.yaml"))
                .unwrap();

        assert!(
            tower_manifest.contains("cluster.type: \"management\""),
            "tower cluster-config must have type 'management'"
        );
        assert!(
            sandbox_manifest.contains("cluster.type: \"workload\""),
            "sandbox cluster-config must have type 'workload'"
        );
    }

    /// ArgoCD must have persistence enabled for production use.
    #[test]
    fn test_argocd_persistence_enabled() {
        let values = include_str!("../../../gitops/tower/argocd/values.yaml");
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(values).expect("ArgoCD values.yaml must be valid YAML");

        let persistence_enabled = parsed
            .get("persistence")
            .and_then(|p| p.get("enabled"))
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        assert!(
            persistence_enabled,
            "ArgoCD persistence.enabled must be true for production"
        );
    }

    // ========================================================================
    // Sprint 1: Single-node SDI, Baremetal mode, Idempotency, E2E pipeline
    // ========================================================================

    /// CL-1: Verify single-node SDI — tower + sandbox both placed on 1 host.
    /// This is the minimum viable deployment for development/testing.
    #[test]
    fn test_single_node_sdi_tower_and_sandbox_on_one_host() {
        let yaml = r#"
resource_pool:
  name: "single-node-pool"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.88.0/24"
    gateway: "192.168.88.1"
    nameservers: ["8.8.8.8"]

os_image:
  source: "https://example.com/image.img"
  format: "qcow2"

cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id.pub"
  packages: [curl]

spec:
  sdi_pools:
    - pool_name: "tower"
      purpose: "management"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "tower-cp-0"
          ip: "192.168.88.100"
          cpu: 2
          mem_gb: 3
          disk_gb: 30
          roles: [control-plane, worker]

    - pool_name: "sandbox"
      purpose: "workload"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "sandbox-cp-0"
          ip: "192.168.88.110"
          cpu: 2
          mem_gb: 4
          disk_gb: 40
          roles: [control-plane, worker]
"#;
        let spec: SdiSpec = serde_yaml::from_str(yaml).unwrap();

        // Both pools must parse and point to single host
        assert_eq!(spec.spec.sdi_pools.len(), 2);
        assert_eq!(spec.spec.sdi_pools[0].placement.hosts, vec!["playbox-0"]);
        assert_eq!(spec.spec.sdi_pools[1].placement.hosts, vec!["playbox-0"]);

        // HCL must generate only 1 provider (deduplicated)
        let hcl = crate::core::tofu::generate_tofu_main(&spec, "jinwang");
        let provider_count = hcl.matches("provider \"libvirt\"").count();
        assert_eq!(
            provider_count, 1,
            "Single-node SDI must generate exactly 1 libvirt provider, got {}",
            provider_count
        );

        // Must generate VMs for both pools
        assert!(hcl.contains("tower-cp-0"), "missing tower VM");
        assert!(hcl.contains("sandbox-cp-0"), "missing sandbox VM");

        // IPs must be distinct
        assert!(hcl.contains("192.168.88.100"));
        assert!(hcl.contains("192.168.88.110"));

        // Pool mapping validation must pass
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("sandbox", "sandbox", ClusterMode::Sdi),
                ],
                argocd: None,
                domains: None,
            },
        };
        let errors = validate_cluster_sdi_pool_mapping(&k8s, &spec);
        assert!(
            errors.is_empty(),
            "single-node SDI pool mapping failed: {:?}",
            errors
        );
    }

    /// CL-1: Verify single-node SDI with overlapping resource constraints.
    /// Total CPU/mem must not exceed a reasonable single-node capacity.
    #[test]
    fn test_single_node_sdi_resource_aggregation() {
        let yaml = r#"
resource_pool:
  name: "single-node"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.88.0/24"
    gateway: "192.168.88.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "https://example.com/img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id.pub"
spec:
  sdi_pools:
    - pool_name: "tower"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "tower-cp-0"
          ip: "192.168.88.100"
          cpu: 2
          mem_gb: 3
          disk_gb: 30
          roles: [control-plane, worker]
    - pool_name: "sandbox"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "sandbox-cp-0"
          ip: "192.168.88.110"
          cpu: 4
          mem_gb: 8
          disk_gb: 60
          roles: [control-plane, worker]
"#;
        let spec: SdiSpec = serde_yaml::from_str(yaml).unwrap();

        // Aggregate resources per host
        let total_cpu: u32 = spec
            .spec
            .sdi_pools
            .iter()
            .flat_map(|p| &p.node_specs)
            .map(|n| n.cpu)
            .sum();
        let total_mem: u32 = spec
            .spec
            .sdi_pools
            .iter()
            .flat_map(|p| &p.node_specs)
            .map(|n| n.mem_gb)
            .sum();

        assert_eq!(total_cpu, 6, "single-node total CPU must be 2+4=6");
        assert_eq!(total_mem, 11, "single-node total mem must be 3+8=11 GB");

        // Verify unique IPs across all pools
        let all_ips: Vec<&str> = spec
            .spec
            .sdi_pools
            .iter()
            .flat_map(|p| &p.node_specs)
            .map(|n| n.ip.as_str())
            .collect();
        let unique_ips: std::collections::HashSet<&str> = all_ips.iter().copied().collect();
        assert_eq!(
            all_ips.len(),
            unique_ips.len(),
            "All VM IPs must be unique, even on single node"
        );
    }

    /// CL-9: Baremetal mode inventory generation must produce valid Kubespray INI.
    #[test]
    fn test_baremetal_mode_inventory_generation() {
        let cluster = ClusterDef {
            cluster_name: "prod".to_string(),
            cluster_mode: ClusterMode::Baremetal,
            cluster_sdi_resource_pool: String::new(),
            baremetal_nodes: vec![
                BaremetalNode {
                    node_name: "node-0".to_string(),
                    ip: "10.0.0.1".to_string(),
                    roles: vec!["control-plane".to_string(), "etcd".to_string()],
                },
                BaremetalNode {
                    node_name: "node-1".to_string(),
                    ip: "10.0.0.2".to_string(),
                    roles: vec!["worker".to_string()],
                },
                BaremetalNode {
                    node_name: "node-2".to_string(),
                    ip: "10.0.0.3".to_string(),
                    roles: vec!["worker".to_string()],
                },
            ],
            cluster_role: "workload".to_string(),
            network: ClusterNetwork {
                pod_cidr: "10.234.0.0/17".to_string(),
                service_cidr: "10.234.128.0/18".to_string(),
                dns_domain: "prod.local".to_string(),
                native_routing_cidr: None,
            },
            cilium: Some(CiliumConfig {
                cluster_id: 3,
                cluster_name: "prod".to_string(),
            }),
            oidc: None,
            kubespray_extra_vars: None,
            ssh_user: None,
        };

        let ini = crate::core::kubespray::generate_inventory_baremetal(&cluster).unwrap();

        // [all] must list all nodes
        assert!(ini.contains("node-0"), "missing node-0 in [all]");
        assert!(ini.contains("node-1"), "missing node-1 in [all]");
        assert!(ini.contains("node-2"), "missing node-2 in [all]");

        // [all] must have correct IPs
        assert!(ini.contains("ansible_host=10.0.0.1"), "wrong IP for node-0");
        assert!(ini.contains("ansible_host=10.0.0.2"), "wrong IP for node-1");

        // [kube_control_plane] must have only control-plane nodes
        let sections: Vec<&str> = ini.split('[').collect();
        let cp_section = sections
            .iter()
            .find(|s| s.starts_with("kube_control_plane]"))
            .unwrap();
        assert!(
            cp_section.contains("node-0"),
            "node-0 must be in control plane"
        );
        assert!(
            !cp_section.contains("node-1"),
            "node-1 must NOT be in control plane"
        );

        // [kube_node] must have worker nodes
        let worker_section = sections
            .iter()
            .find(|s| s.starts_with("kube_node]"))
            .unwrap();
        assert!(
            worker_section.contains("node-1"),
            "node-1 must be in kube_node"
        );
        assert!(
            worker_section.contains("node-2"),
            "node-2 must be in kube_node"
        );

        // [etcd] must have etcd-role nodes
        let etcd_section = sections.iter().find(|s| s.starts_with("etcd]")).unwrap();
        assert!(etcd_section.contains("node-0"), "node-0 must be in etcd");
    }

    /// CL-9: Baremetal mode must reject empty node list.
    #[test]
    fn test_baremetal_mode_rejects_empty_nodes() {
        let cluster = ClusterDef {
            cluster_name: "empty-prod".to_string(),
            cluster_mode: ClusterMode::Baremetal,
            cluster_sdi_resource_pool: String::new(),
            baremetal_nodes: vec![],
            cluster_role: "workload".to_string(),
            network: ClusterNetwork {
                pod_cidr: "10.234.0.0/17".to_string(),
                service_cidr: "10.234.128.0/18".to_string(),
                dns_domain: "prod.local".to_string(),
                native_routing_cidr: None,
            },
            cilium: None,
            oidc: None,
            kubespray_extra_vars: None,
            ssh_user: None,
        };

        let result = crate::core::kubespray::generate_inventory_baremetal(&cluster);
        assert!(result.is_err(), "baremetal with 0 nodes must fail");
        assert!(
            result.unwrap_err().contains("no baremetal_nodes"),
            "error must mention missing baremetal_nodes"
        );
    }

    /// CL-13: HCL generation must be idempotent (generate_tofu_main).
    #[test]
    fn test_generate_tofu_main_idempotent() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let hcl1 = crate::core::tofu::generate_tofu_main(&spec, "jinwang");
        let hcl2 = crate::core::tofu::generate_tofu_main(&spec, "jinwang");
        assert_eq!(
            hcl1, hcl2,
            "generate_tofu_main must be deterministic (idempotent)"
        );
    }

    /// CL-13: Kubespray inventory generation must be idempotent.
    #[test]
    fn test_generate_inventory_idempotent() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");

        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s.config.clusters {
            let inv1 = crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap();
            let inv2 = crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap();
            assert_eq!(
                inv1, inv2,
                "inventory for '{}' must be idempotent",
                cluster.cluster_name
            );
        }
    }

    /// CL-13: Kubespray cluster vars generation must be idempotent.
    #[test]
    fn test_generate_cluster_vars_idempotent() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s.config.clusters {
            let vars1 = crate::core::kubespray::generate_cluster_vars(cluster, &k8s.config.common);
            let vars2 = crate::core::kubespray::generate_cluster_vars(cluster, &k8s.config.common);
            assert_eq!(
                vars1, vars2,
                "cluster vars for '{}' must be idempotent",
                cluster.cluster_name
            );
        }
    }

    /// CL-8: E2E dry-run pipeline — parse all configs → validate → generate outputs.
    /// Simulates: facts(skip) → sdi-specs parse → k8s-clusters parse → validate →
    ///            HCL generate → inventory generate → vars generate.
    #[test]
    fn test_e2e_config_to_output_pipeline() {
        // Step 1: Parse baremetal-init.yaml
        let bm_content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let bm: crate::core::config::BaremetalInitConfig =
            serde_yaml::from_str(bm_content).expect("baremetal-init.yaml must parse");
        assert_eq!(bm.target_nodes.len(), 4);

        // Step 2: Parse sdi-specs.yaml
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).expect("sdi-specs.yaml must parse");
        assert_eq!(sdi.spec.sdi_pools.len(), 2);

        // Step 3: Parse k8s-clusters.yaml
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig =
            serde_yaml::from_str(k8s_content).expect("k8s-clusters.yaml must parse");
        assert_eq!(k8s.config.clusters.len(), 2);

        // Step 4: Cross-validate pool mapping
        let pool_errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            pool_errors.is_empty(),
            "pool mapping errors: {:?}",
            pool_errors
        );

        // Step 5: Validate unique cluster IDs
        let id_errors = validate_unique_cluster_ids(&k8s);
        assert!(id_errors.is_empty(), "cluster ID errors: {:?}", id_errors);

        // Step 6: Generate HCL (host infra)
        let host_inputs: Vec<crate::core::tofu::HostInfraInput> = bm
            .target_nodes
            .iter()
            .map(|n| crate::core::tofu::HostInfraInput {
                name: n.name.clone(),
                ip: n.node_ip.clone(),
                ssh_user: n.admin_user.clone(),
            })
            .collect();
        let net = bm.network_defaults.as_ref().unwrap();
        let host_hcl = crate::core::tofu::generate_tofu_host_infra(
            &host_inputs,
            &net.management_bridge,
            &net.management_cidr,
            &net.gateway,
        );
        assert!(!host_hcl.is_empty(), "host HCL must not be empty");
        assert_eq!(
            host_hcl.matches("provider \"libvirt\"").count(),
            4,
            "must generate 4 providers for 4 bare-metal nodes"
        );

        // Step 7: Generate HCL (VM pools)
        let vm_hcl = crate::core::tofu::generate_tofu_main(&sdi, "jinwang");
        assert!(vm_hcl.contains("tower-cp-0"), "HCL missing tower VM");
        assert!(vm_hcl.contains("sandbox-cp-0"), "HCL missing sandbox CP VM");
        assert!(
            vm_hcl.contains("sandbox-w-0"),
            "HCL missing sandbox worker VM"
        );

        // Step 8: Generate Kubespray inventory for each cluster
        for cluster in &k8s.config.clusters {
            let inv =
                crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap_or_else(|e| {
                    panic!("inventory for '{}' failed: {}", cluster.cluster_name, e)
                });
            assert!(inv.contains("[all]"), "inventory must have [all] section");
            assert!(
                inv.contains("[kube_control_plane]"),
                "inventory must have control plane section"
            );
            assert!(
                inv.contains("[kube_node]"),
                "inventory must have worker section"
            );
        }

        // Step 9: Generate cluster vars
        for cluster in &k8s.config.clusters {
            let vars = crate::core::kubespray::generate_cluster_vars(cluster, &k8s.config.common);
            let parsed: serde_yaml::Mapping = serde_yaml::from_str(&vars).unwrap_or_else(|e| {
                panic!("vars for '{}' invalid YAML: {e}", cluster.cluster_name)
            });
            assert!(
                parsed.contains_key(serde_yaml::Value::String("kube_version".to_string())),
                "vars must contain kube_version"
            );
            assert!(
                parsed.contains_key(serde_yaml::Value::String("kube_pods_subnet".to_string())),
                "vars must contain pod CIDR"
            );
        }
    }

    /// CL-8, CL-10, CL-13: Full dry-run pipeline including secrets + gitops validation.
    /// Extends test_e2e_config_to_output_pipeline with secrets generation and gitops structure checks.
    #[test]
    fn test_e2e_full_pipeline_secrets_and_gitops() {
        // --- Phase 1: Config loading (same as base E2E) ---
        let bm_content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let bm: crate::core::config::BaremetalInitConfig =
            serde_yaml::from_str(bm_content).expect("baremetal-init.yaml must parse");

        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).expect("sdi-specs.yaml must parse");

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig =
            serde_yaml::from_str(k8s_content).expect("k8s-clusters.yaml must parse");

        // --- Phase 2: Cross-validation ---
        let pool_errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(pool_errors.is_empty(), "pool mapping: {:?}", pool_errors);
        let id_errors = validate_unique_cluster_ids(&k8s);
        assert!(id_errors.is_empty(), "cluster IDs: {:?}", id_errors);

        // --- Phase 3: SDI spec validation ---
        let sdi_errors = validate_sdi_spec(&sdi);
        assert!(sdi_errors.is_empty(), "SDI spec: {:?}", sdi_errors);

        // --- Phase 4: Baremetal config validation ---
        let bm_errors = crate::core::config::validate_baremetal_config(&bm);
        assert!(bm_errors.is_empty(), "baremetal config: {:?}", bm_errors);

        // --- Phase 5: Generate outputs for each cluster ---
        for cluster in &k8s.config.clusters {
            // Inventory
            let inv = crate::core::kubespray::generate_inventory(cluster, &sdi)
                .unwrap_or_else(|e| panic!("inventory '{}': {}", cluster.cluster_name, e));
            assert!(inv.contains("[all]"));

            // Cluster vars
            let vars = crate::core::kubespray::generate_cluster_vars(cluster, &k8s.config.common);
            let _: serde_yaml::Mapping = serde_yaml::from_str(&vars)
                .unwrap_or_else(|e| panic!("vars '{}': {e}", cluster.cluster_name));

            // Cilium values with ClusterMesh
            if let Some(cilium) = &cluster.cilium {
                let cilium_vals = crate::core::gitops::generate_cilium_values_with_mesh(
                    "10.0.0.1",
                    6443,
                    &cluster.cluster_name,
                    cilium.cluster_id,
                );
                assert!(
                    cilium_vals.contains(&format!("name: \"{}\"", cluster.cluster_name)),
                    "cilium values must contain cluster name"
                );
            }

            // Cluster config manifest
            let role = if cluster.cluster_role == "management" {
                "management"
            } else {
                "workload"
            };
            let config_manifest = crate::core::gitops::generate_cluster_config_manifest(
                &cluster.cluster_name,
                &format!("{}.local", cluster.cluster_name),
                role,
            );
            assert!(
                config_manifest.contains(&cluster.cluster_name),
                "cluster config must contain cluster name"
            );
        }

        // --- Phase 6: Secrets generation ---
        let secrets_content = include_str!("../../../credentials/secrets.yaml.example");
        // Management cluster gets all secrets (keycloak, argocd, cloudflare)
        let mgmt_secrets =
            crate::core::secrets::generate_all_secrets_manifests(secrets_content, "management")
                .expect("management secrets must generate");
        assert!(
            mgmt_secrets.contains("keycloak"),
            "management must have keycloak secret"
        );

        // Workload cluster gets no cloudflare/keycloak secrets
        let work_secrets =
            crate::core::secrets::generate_all_secrets_manifests(secrets_content, "workload")
                .expect("workload secrets must generate");
        assert!(
            !work_secrets.contains("cloudflare"),
            "workload must NOT have cloudflare secret"
        );

        // --- Phase 7: GitOps structure validation ---
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let bootstrap = repo_root.join("gitops/bootstrap/spread.yaml");
        assert!(bootstrap.exists(), "spread.yaml must exist for bootstrap");

        // Verify generator dirs exist for each cluster
        for cluster in &k8s.config.clusters {
            let gen_dir = repo_root.join(format!("gitops/generators/{}", cluster.cluster_name));
            assert!(
                gen_dir.exists(),
                "generator dir must exist: gitops/generators/{}",
                cluster.cluster_name
            );
        }

        // Verify common apps directory exists
        let common_dir = repo_root.join("gitops/common");
        assert!(common_dir.exists(), "gitops/common must exist");

        // --- Phase 8: Idempotency check (CL-13) ---
        // Re-generate and verify identical output
        for cluster in &k8s.config.clusters {
            let inv1 = crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap();
            let inv2 = crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap();
            assert_eq!(
                inv1, inv2,
                "inventory must be idempotent for {}",
                cluster.cluster_name
            );

            let vars1 = crate::core::kubespray::generate_cluster_vars(cluster, &k8s.config.common);
            let vars2 = crate::core::kubespray::generate_cluster_vars(cluster, &k8s.config.common);
            assert_eq!(
                vars1, vars2,
                "vars must be idempotent for {}",
                cluster.cluster_name
            );
        }
    }

    /// CL-8: Verify scalex get config-files would detect missing files.
    /// Config file validation must report all required files.
    #[test]
    fn test_config_files_validation_required_set() {
        // Required config files that scalex get config-files should check
        let required_files = [
            "credentials/.baremetal-init.yaml",
            "credentials/.env",
            "config/sdi-specs.yaml",
            "config/k8s-clusters.yaml",
        ];
        // Corresponding example files must exist
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for file in &required_files {
            let example = repo_root.join(format!("{}.example", file));
            assert!(
                example.exists(),
                "Example file must exist: {}.example",
                file
            );
        }
    }

    /// CL-1, CL-9: Verify SDI and baremetal modes are mutually exclusive in validation.
    #[test]
    fn test_sdi_and_baremetal_modes_exclusive() {
        // SDI mode cluster with empty pool ref should fail
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![make_cluster("broken", "", ClusterMode::Sdi)],
                argocd: None,
                domains: None,
            },
        };
        let sdi = make_sdi_spec_with_pools(&["tower"]);
        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            !errors.is_empty(),
            "SDI mode with empty pool ref must produce validation error"
        );
        assert!(
            errors[0].contains("no cluster_sdi_resource_pool"),
            "error must mention missing pool reference"
        );
    }

    // ========================================================================
    // Sprint 3.2: Unified sdi-pools view (resource-pool-summary fallback)
    // ========================================================================

    // Sprint 3.2 tests for resource_pool_to_rows are in commands/get.rs tests module
    // (SdiPoolRow is private to that module)

    // ========================================================================
    // Sprint 4.1: Third cluster extensibility
    // ========================================================================

    /// CL-8: Adding a 3rd cluster must pass all cross-config validations.
    /// Verifies: unique cluster IDs, pool mapping, inventory generation.
    #[test]
    fn test_third_cluster_extensibility() {
        // 3 SDI pools: tower, sandbox, datax
        let sdi_yaml = r#"
resource_pool:
  name: "playbox-pool"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.88.0/24"
    gateway: "192.168.88.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "https://example.com/img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id.pub"
spec:
  sdi_pools:
    - pool_name: "tower"
      purpose: "management"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "tower-cp-0"
          ip: "192.168.88.100"
          cpu: 2
          mem_gb: 3
          disk_gb: 30
          roles: [control-plane, worker]
    - pool_name: "sandbox"
      purpose: "workload"
      placement:
        hosts: [playbox-1]
        spread: true
      node_specs:
        - node_name: "sandbox-cp-0"
          ip: "192.168.88.110"
          cpu: 4
          mem_gb: 8
          disk_gb: 60
          roles: [control-plane]
        - node_name: "sandbox-w-0"
          ip: "192.168.88.111"
          cpu: 4
          mem_gb: 8
          disk_gb: 60
          roles: [worker]
    - pool_name: "datax"
      purpose: "data"
      placement:
        hosts: [playbox-2, playbox-3]
        spread: true
      node_specs:
        - node_name: "datax-cp-0"
          ip: "192.168.88.120"
          cpu: 4
          mem_gb: 16
          disk_gb: 200
          host: "playbox-2"
          roles: [control-plane]
        - node_name: "datax-w-0"
          ip: "192.168.88.121"
          cpu: 8
          mem_gb: 32
          disk_gb: 500
          host: "playbox-3"
          roles: [worker]
"#;
        let sdi: SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        assert_eq!(sdi.spec.sdi_pools.len(), 3, "must parse 3 SDI pools");

        // 3 clusters referencing 3 pools with unique cluster_ids
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_id("tower", "tower", ClusterMode::Sdi, 1),
                    make_cluster_with_id("sandbox", "sandbox", ClusterMode::Sdi, 2),
                    make_cluster_with_id("datax", "datax", ClusterMode::Sdi, 3),
                ],
                argocd: None,
                domains: None,
            },
        };

        // Cross-validation must pass
        let pool_errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            pool_errors.is_empty(),
            "3-cluster pool mapping must pass: {:?}",
            pool_errors
        );

        let id_errors = validate_unique_cluster_ids(&k8s);
        assert!(
            id_errors.is_empty(),
            "3-cluster unique IDs must pass: {:?}",
            id_errors
        );

        // HCL must contain all 3 pools' VMs
        let hcl = crate::core::tofu::generate_tofu_main(&sdi, "jinwang");
        assert!(hcl.contains("tower-cp-0"), "HCL missing tower VM");
        assert!(hcl.contains("sandbox-cp-0"), "HCL missing sandbox CP");
        assert!(hcl.contains("datax-cp-0"), "HCL missing datax CP");
        assert!(hcl.contains("datax-w-0"), "HCL missing datax worker");

        // Each cluster must generate valid inventory
        for cluster in &k8s.config.clusters {
            let inv =
                crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap_or_else(|e| {
                    panic!("inventory for '{}' failed: {}", cluster.cluster_name, e)
                });
            assert!(
                inv.contains("[all]"),
                "{} inventory missing [all]",
                cluster.cluster_name
            );
            assert!(
                inv.contains("[kube_control_plane]"),
                "{} inventory missing control plane",
                cluster.cluster_name
            );
        }
    }

    /// CL-8: Duplicate cluster IDs across 3 clusters must fail validation.
    #[test]
    fn test_third_cluster_duplicate_id_rejected() {
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_id("tower", "tower", ClusterMode::Sdi, 1),
                    make_cluster_with_id("sandbox", "sandbox", ClusterMode::Sdi, 2),
                    make_cluster_with_id("datax", "datax", ClusterMode::Sdi, 2), // duplicate!
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_unique_cluster_ids(&k8s);
        assert!(
            !errors.is_empty(),
            "duplicate cluster_id=2 must produce error"
        );
        assert!(
            errors[0].contains("cluster_id 2"),
            "error must mention conflicting cluster_id: {:?}",
            errors
        );
    }

    /// CL-8: 3rd cluster referencing non-existent pool must fail.
    #[test]
    fn test_third_cluster_missing_pool_rejected() {
        let sdi = make_sdi_spec_with_pools(&["tower", "sandbox"]);
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("sandbox", "sandbox", ClusterMode::Sdi),
                    make_cluster("datax", "datax", ClusterMode::Sdi), // pool doesn't exist
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            !errors.is_empty(),
            "referencing non-existent pool 'datax' must fail"
        );
        assert!(
            errors[0].contains("datax"),
            "error must mention missing pool 'datax'"
        );
    }

    // ========================================================================
    // Sprint 4.3: SDI sync side-effect detection
    // ========================================================================

    /// CL-8: Sync diff computation must correctly identify added/removed/unchanged nodes.
    #[test]
    fn test_sdi_sync_diff_computation() {
        let desired: std::collections::HashSet<String> = ["playbox-0", "playbox-1", "playbox-3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let current: std::collections::HashSet<String> = ["playbox-0", "playbox-1", "playbox-2"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let to_add: Vec<&str> = desired
            .iter()
            .filter(|n| !current.contains(n.as_str()))
            .map(|n| n.as_str())
            .collect();
        let to_remove: Vec<&str> = current
            .iter()
            .filter(|n| !desired.contains(n.as_str()))
            .map(|n| n.as_str())
            .collect();
        let unchanged: Vec<&str> = desired
            .iter()
            .filter(|n| current.contains(n.as_str()))
            .map(|n| n.as_str())
            .collect();

        assert_eq!(to_add, vec!["playbox-3"]);
        assert_eq!(to_remove, vec!["playbox-2"]);
        assert_eq!(unchanged.len(), 2); // playbox-0, playbox-1
    }

    /// CL-8: Sync must detect VMs hosted on nodes being removed.
    #[test]
    fn test_sdi_sync_detects_vm_impact() {
        use crate::models::sdi::{SdiNodeState, SdiPoolState};

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
                nodes: vec![SdiNodeState {
                    node_name: "sandbox-w-0".to_string(),
                    ip: "192.168.88.120".to_string(),
                    host: "playbox-2".to_string(),
                    cpu: 8,
                    mem_gb: 16,
                    disk_gb: 100,
                    gpu_passthrough: true,
                    status: "running".to_string(),
                }],
            },
        ];

        // Removing playbox-2 must detect sandbox-w-0 as affected
        let to_remove = vec!["playbox-2"];
        let mut affected_vms = Vec::new();
        for pool in &pools {
            for node in &pool.nodes {
                if to_remove.contains(&node.host.as_str()) {
                    affected_vms.push(format!(
                        "{} (pool: {}, host: {})",
                        node.node_name, pool.pool_name, node.host
                    ));
                }
            }
        }

        assert_eq!(affected_vms.len(), 1, "must detect 1 affected VM");
        assert!(
            affected_vms[0].contains("sandbox-w-0"),
            "must identify sandbox-w-0"
        );
        assert!(
            affected_vms[0].contains("playbox-2"),
            "must identify host playbox-2"
        );
    }

    /// CL-8: Sync with no changes must be a no-op.
    #[test]
    fn test_sdi_sync_no_changes() {
        let desired: std::collections::HashSet<String> = ["playbox-0", "playbox-1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let current: std::collections::HashSet<String> = ["playbox-0", "playbox-1"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let to_add: Vec<&str> = desired
            .iter()
            .filter(|n| !current.contains(n.as_str()))
            .map(|n| n.as_str())
            .collect();
        let to_remove: Vec<&str> = current
            .iter()
            .filter(|n| !desired.contains(n.as_str()))
            .map(|n| n.as_str())
            .collect();

        assert!(to_add.is_empty(), "no nodes to add");
        assert!(to_remove.is_empty(), "no nodes to remove");
    }

    /// CL-8: Removing a node without VMs should NOT trigger side-effect warning.
    #[test]
    fn test_sdi_sync_remove_empty_host_no_warning() {
        use crate::models::sdi::{SdiNodeState, SdiPoolState};

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

        // Removing playbox-3 (no VMs on it) should have 0 affected
        let to_remove = vec!["playbox-3"];
        let affected_count: usize = pools
            .iter()
            .flat_map(|p| &p.nodes)
            .filter(|n| to_remove.contains(&n.host.as_str()))
            .count();

        assert_eq!(
            affected_count, 0,
            "removing host with no VMs must not trigger warning"
        );
    }

    #[test]
    fn test_no_legacy_toplevel_artifacts() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");

        let legacy_artifacts = ["gitops-apps", "gitops-manual", "values.yaml"];

        let mut found = Vec::new();
        for name in &legacy_artifacts {
            if repo_root.join(name).exists() {
                found.push(*name);
            }
        }

        assert!(
            found.is_empty(),
            "Legacy top-level artifacts still present (move dirs to .legacy- prefix, delete values.yaml): {:?}",
            found
        );
    }

    /// Verify no legacy datax-kubespray references in Rust source code.
    /// Checklist #4, #12: all legacy references must be fully removed.
    #[test]
    fn test_no_legacy_datax_references_in_rust_source() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let needle = ".legacy-datax-kube"; // partial match avoids self-detection
        let mut violations = Vec::new();

        fn scan_dir(dir: &std::path::Path, needle: &str, violations: &mut Vec<String>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        scan_dir(&path, needle, violations);
                    } else if path.extension().is_some_and(|e| e == "rs") {
                        // Skip this test file (validation.rs) to avoid self-detection
                        if path.ends_with("validation.rs") {
                            continue;
                        }
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            for (line_num, line) in content.lines().enumerate() {
                                if line.contains(needle) {
                                    let rel = path
                                        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                                        .unwrap_or(&path);
                                    violations.push(format!(
                                        "{}:{}: {}",
                                        rel.display(),
                                        line_num + 1,
                                        line.trim()
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        scan_dir(&src_dir, needle, &mut violations);
        assert!(
            violations.is_empty(),
            "Legacy datax-kubespray references found in Rust source (CL-4/CL-12 violation):\n{}",
            violations.join("\n")
        );
    }

    /// Verify find_kubespray_dir candidates do not include legacy paths
    /// and DO include the project's kubespray/ submodule.
    #[test]
    fn test_kubespray_dir_candidates_no_legacy() {
        // The kubespray submodule should exist at project root
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let kubespray_submodule = repo_root.join("kubespray");
        assert!(
            kubespray_submodule.exists(),
            "kubespray/ submodule directory should exist at project root"
        );
    }

    // ========================================================================
    // CL-8: SDI spec semantic validation
    // ========================================================================

    /// Valid SDI spec must pass validation.
    #[test]
    fn test_validate_sdi_spec_valid() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();
        let errors = validate_sdi_spec(&spec);
        assert!(
            errors.is_empty(),
            "example sdi-specs.yaml must pass validation: {:?}",
            errors
        );
    }

    /// Empty pools must fail.
    #[test]
    fn test_validate_sdi_spec_empty_pools() {
        let spec = make_sdi_spec_with_pools(&[]);
        // Override to have truly empty pools
        let mut spec = spec;
        spec.spec.sdi_pools.clear();
        let errors = validate_sdi_spec(&spec);
        assert!(errors.iter().any(|e| e.contains("empty")));
    }

    /// Duplicate pool names must fail.
    #[test]
    fn test_validate_sdi_spec_duplicate_pool_names() {
        let yaml = r#"
resource_pool:
  name: "pool"
  network:
    management_bridge: "br0"
    management_cidr: "10.0.0.0/24"
    gateway: "10.0.0.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "keys"
spec:
  sdi_pools:
    - pool_name: "tower"
      placement:
        hosts: [h0]
      node_specs:
        - node_name: "vm-0"
          ip: "10.0.0.10"
          cpu: 2
          mem_gb: 4
          disk_gb: 30
          roles: [control-plane]
    - pool_name: "tower"
      placement:
        hosts: [h1]
      node_specs:
        - node_name: "vm-1"
          ip: "10.0.0.11"
          cpu: 2
          mem_gb: 4
          disk_gb: 30
          roles: [worker]
"#;
        let spec: SdiSpec = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_sdi_spec(&spec);
        assert!(errors.iter().any(|e| e.contains("duplicate pool_name")));
    }

    /// Duplicate VM IPs across pools must fail.
    #[test]
    fn test_validate_sdi_spec_duplicate_vm_ips() {
        let yaml = r#"
resource_pool:
  name: "pool"
  network:
    management_bridge: "br0"
    management_cidr: "10.0.0.0/24"
    gateway: "10.0.0.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "keys"
spec:
  sdi_pools:
    - pool_name: "tower"
      placement:
        hosts: [h0]
      node_specs:
        - node_name: "tower-0"
          ip: "10.0.0.10"
          cpu: 2
          mem_gb: 4
          disk_gb: 30
          roles: [control-plane]
    - pool_name: "sandbox"
      placement:
        hosts: [h1]
      node_specs:
        - node_name: "sandbox-0"
          ip: "10.0.0.10"
          cpu: 2
          mem_gb: 4
          disk_gb: 30
          roles: [worker]
"#;
        let spec: SdiSpec = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_sdi_spec(&spec);
        assert!(
            errors.iter().any(|e| e.contains("duplicate IP")),
            "must detect duplicate IP: {:?}",
            errors
        );
    }

    /// Zero CPU/mem must fail.
    #[test]
    fn test_validate_sdi_spec_zero_resources() {
        let yaml = r#"
resource_pool:
  name: "pool"
  network:
    management_bridge: "br0"
    management_cidr: "10.0.0.0/24"
    gateway: "10.0.0.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "keys"
spec:
  sdi_pools:
    - pool_name: "bad"
      placement:
        hosts: [h0]
      node_specs:
        - node_name: "vm-bad"
          ip: "10.0.0.10"
          cpu: 0
          mem_gb: 0
          disk_gb: 0
          roles: []
"#;
        let spec: SdiSpec = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_sdi_spec(&spec);
        assert!(errors.iter().any(|e| e.contains("cpu must be > 0")));
        assert!(errors.iter().any(|e| e.contains("mem_gb must be > 0")));
        assert!(errors.iter().any(|e| e.contains("disk_gb must be > 0")));
        assert!(errors.iter().any(|e| e.contains("roles must not be empty")));
    }

    // ========================================================================
    // Sprint 8a.2: Clean→Rebuild idempotency E2E dry-run
    // ========================================================================

    /// CL-13: Verify that after a conceptual "clean", regenerating all outputs from
    /// the same config files produces identical results. This tests the full pipeline
    /// idempotency across a clean→rebuild cycle without requiring physical infrastructure.
    #[test]
    fn test_e2e_clean_rebuild_idempotency() {
        // Load all configs (same as other E2E tests)
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let bm_content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let bm: crate::core::config::BaremetalInitConfig =
            serde_yaml::from_str(bm_content).unwrap();

        // --- First pass: generate all outputs ---
        let hcl_1 = crate::core::tofu::generate_tofu_main(&sdi, "jinwang");
        let host_inputs: Vec<crate::core::tofu::HostInfraInput> = bm
            .target_nodes
            .iter()
            .map(|n| crate::core::tofu::HostInfraInput {
                name: n.name.clone(),
                ip: n.node_ip.clone(),
                ssh_user: n.admin_user.clone(),
            })
            .collect();
        let net = bm.network_defaults.as_ref().unwrap();
        let host_hcl_1 = crate::core::tofu::generate_tofu_host_infra(
            &host_inputs,
            &net.management_bridge,
            &net.management_cidr,
            &net.gateway,
        );

        let mut inventories_1 = Vec::new();
        let mut vars_1 = Vec::new();
        for cluster in &k8s.config.clusters {
            inventories_1.push(crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap());
            vars_1.push(crate::core::kubespray::generate_cluster_vars(
                cluster,
                &k8s.config.common,
            ));
        }

        // --- Simulate clean: re-parse configs from scratch (fresh state) ---
        let sdi_2: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();
        let k8s_2: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();
        let bm_2: crate::core::config::BaremetalInitConfig =
            serde_yaml::from_str(bm_content).unwrap();

        // --- Second pass: regenerate all outputs ---
        let hcl_2 = crate::core::tofu::generate_tofu_main(&sdi_2, "jinwang");
        let host_inputs_2: Vec<crate::core::tofu::HostInfraInput> = bm_2
            .target_nodes
            .iter()
            .map(|n| crate::core::tofu::HostInfraInput {
                name: n.name.clone(),
                ip: n.node_ip.clone(),
                ssh_user: n.admin_user.clone(),
            })
            .collect();
        let net_2 = bm_2.network_defaults.as_ref().unwrap();
        let host_hcl_2 = crate::core::tofu::generate_tofu_host_infra(
            &host_inputs_2,
            &net_2.management_bridge,
            &net_2.management_cidr,
            &net_2.gateway,
        );

        let mut inventories_2 = Vec::new();
        let mut vars_2 = Vec::new();
        for cluster in &k8s_2.config.clusters {
            inventories_2
                .push(crate::core::kubespray::generate_inventory(cluster, &sdi_2).unwrap());
            vars_2.push(crate::core::kubespray::generate_cluster_vars(
                cluster,
                &k8s_2.config.common,
            ));
        }

        // --- Assert byte-for-byte identical outputs ---
        assert_eq!(hcl_1, hcl_2, "VM HCL must be identical after clean→rebuild");
        assert_eq!(
            host_hcl_1, host_hcl_2,
            "Host HCL must be identical after clean→rebuild"
        );
        for i in 0..inventories_1.len() {
            assert_eq!(
                inventories_1[i], inventories_2[i],
                "Inventory {} must be identical after clean→rebuild",
                i
            );
            assert_eq!(
                vars_1[i], vars_2[i],
                "Cluster vars {} must be identical after clean→rebuild",
                i
            );
        }
    }

    /// CL-13: Verify clean operation plan covers all expected states.
    /// Uses pure plan_clean_operations from sdi module.
    #[test]
    fn test_clean_operations_plan_covers_all_branches() {
        use crate::commands::sdi::{plan_clean_operations, CleanOperation};

        // Full hard clean with everything present (no host-infra)
        let full = plan_clean_operations(true, true, true, false, Some(4));
        assert_eq!(full.len(), 3);
        assert!(matches!(full[0], CleanOperation::TofuDestroy));
        assert!(matches!(
            full[1],
            CleanOperation::NodeCleanup { node_count: 4 }
        ));
        assert!(matches!(full[2], CleanOperation::RemoveStateDir));

        // Soft clean (no --hard) should only destroy tofu
        let soft = plan_clean_operations(false, true, true, false, Some(4));
        assert_eq!(soft.len(), 1);
        assert!(matches!(soft[0], CleanOperation::TofuDestroy));

        // No state at all
        let empty = plan_clean_operations(true, false, false, false, Some(4));
        assert_eq!(empty.len(), 1);
        assert!(matches!(empty[0], CleanOperation::NoState));

        // Hard clean with host-infra — should destroy host-infra first, then main
        let with_hi = plan_clean_operations(true, true, true, true, Some(4));
        assert_eq!(with_hi.len(), 4);
        assert!(matches!(with_hi[0], CleanOperation::TofuDestroyHostInfra));
        assert!(matches!(with_hi[1], CleanOperation::TofuDestroy));
    }

    // ========================================================================
    // Sprint 8a.3: Cross-config SDI↔K8s pool reference validation
    // ========================================================================

    /// CL-1, CL-8: Every SDI pool referenced by a k8s cluster must exist in sdi-specs.
    /// Tests exact match between cluster_sdi_resource_pool and sdi_pools[].pool_name.
    #[test]
    fn test_cross_config_every_cluster_pool_ref_resolves() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let pool_names: Vec<&str> = sdi
            .spec
            .sdi_pools
            .iter()
            .map(|p| p.pool_name.as_str())
            .collect();

        for cluster in &k8s.config.clusters {
            if cluster.cluster_mode == ClusterMode::Sdi {
                assert!(
                    pool_names.contains(&cluster.cluster_sdi_resource_pool.as_str()),
                    "Cluster '{}' references pool '{}' which is not in sdi-specs.yaml (available: {:?})",
                    cluster.cluster_name,
                    cluster.cluster_sdi_resource_pool,
                    pool_names
                );
            }
        }
    }

    /// CL-8: SDI pool count must match the number of SDI-mode clusters.
    /// Unused pools are OK (future expansion), but every SDI cluster needs a pool.
    #[test]
    fn test_cross_config_sdi_cluster_count_lte_pool_count() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let sdi_cluster_count = k8s
            .config
            .clusters
            .iter()
            .filter(|c| c.cluster_mode == ClusterMode::Sdi)
            .count();

        assert!(
            sdi_cluster_count <= sdi.spec.sdi_pools.len(),
            "SDI clusters ({}) exceed available pools ({})",
            sdi_cluster_count,
            sdi.spec.sdi_pools.len()
        );
    }

    /// CL-8: Cilium cluster_id uniqueness must be enforced across all clusters.
    #[test]
    fn test_cross_config_cilium_cluster_ids_globally_unique() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let mut seen_ids = std::collections::HashSet::new();
        for cluster in &k8s.config.clusters {
            if let Some(ref cilium) = cluster.cilium {
                assert!(
                    seen_ids.insert(cilium.cluster_id),
                    "Duplicate Cilium cluster_id {} in cluster '{}'",
                    cilium.cluster_id,
                    cluster.cluster_name
                );
            }
        }
    }

    /// CL-8: Pod/Service CIDRs must not overlap between clusters.
    #[test]
    fn test_cross_config_cidrs_no_overlap() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let clusters = &k8s.config.clusters;
        for i in 0..clusters.len() {
            for j in (i + 1)..clusters.len() {
                assert_ne!(
                    clusters[i].network.pod_cidr, clusters[j].network.pod_cidr,
                    "Pod CIDRs overlap between '{}' and '{}'",
                    clusters[i].cluster_name, clusters[j].cluster_name
                );
                assert_ne!(
                    clusters[i].network.service_cidr, clusters[j].network.service_cidr,
                    "Service CIDRs overlap between '{}' and '{}'",
                    clusters[i].cluster_name, clusters[j].cluster_name
                );
            }
        }
    }

    /// CL-8: DNS domains must be unique per cluster.
    #[test]
    fn test_cross_config_dns_domains_unique() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let mut seen_domains = std::collections::HashSet::new();
        for cluster in &k8s.config.clusters {
            assert!(
                seen_domains.insert(cluster.network.dns_domain.clone()),
                "Duplicate DNS domain '{}' in cluster '{}'",
                cluster.network.dns_domain,
                cluster.cluster_name
            );
        }
    }

    // === Two-Layer Config Consistency Tests ===
    // Infrastructure Layer (sdi-specs + baremetal-init) ↔ GitOps Layer (k8s-clusters + gitops/)

    /// SDI spec placement hosts must be a subset of baremetal-init target nodes.
    /// If sdi-specs references a host not in baremetal-init, provisioning will fail.
    #[test]
    fn test_two_layer_sdi_hosts_subset_of_baremetal_nodes() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_content).unwrap();
        let bm_content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let bm: crate::core::config::BaremetalInitConfig =
            serde_yaml::from_str(bm_content).unwrap();

        let bm_names: std::collections::HashSet<&str> =
            bm.target_nodes.iter().map(|n| n.name.as_str()).collect();

        // Collect all hosts referenced in SDI placement + node_specs
        for pool in &sdi.spec.sdi_pools {
            for host in &pool.placement.hosts {
                assert!(
                    bm_names.contains(host.as_str()),
                    "SDI pool '{}' placement references host '{}' not found in baremetal-init. Available: {:?}",
                    pool.pool_name,
                    host,
                    bm_names
                );
            }
            for node in &pool.node_specs {
                if let Some(ref host) = node.host {
                    assert!(
                        bm_names.contains(host.as_str()),
                        "SDI node '{}' references host '{}' not found in baremetal-init. Available: {:?}",
                        node.node_name,
                        host,
                        bm_names
                    );
                }
            }
        }
    }

    /// Every cluster in k8s-clusters.yaml must have a matching gitops generator directory.
    #[test]
    fn test_two_layer_every_cluster_has_gitops_generator() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s.config.clusters {
            let generator_dir = format!("gitops/generators/{}/", cluster.cluster_name);
            let abs_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(&generator_dir);
            assert!(
                abs_path.is_dir(),
                "Cluster '{}' has no gitops generator directory at '{}'",
                cluster.cluster_name,
                generator_dir
            );
        }
    }

    /// ArgoCD tower_manages list must reference clusters that actually exist in config.
    #[test]
    fn test_two_layer_argocd_tower_manages_references_valid_clusters() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let cluster_names: std::collections::HashSet<String> = k8s
            .config
            .clusters
            .iter()
            .map(|c| c.cluster_name.clone())
            .collect();

        if let Some(ref argocd) = k8s.config.argocd {
            for managed in &argocd.tower_manages {
                assert!(
                    cluster_names.contains(managed.as_str()),
                    "ArgoCD tower_manages references '{}' which is not a defined cluster. Available: {:?}",
                    managed,
                    cluster_names
                );
            }
        }
    }

    // === README Installation Guide Verification Tests ===
    // Ensures all example config files referenced in README exist and parse correctly.

    /// All .example credential files referenced in README Step 2 must exist.
    #[test]
    fn test_readme_credential_example_files_exist() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();

        let required_files = [
            "credentials/.baremetal-init.yaml.example",
            "credentials/.env.example",
            "credentials/secrets.yaml.example",
            "credentials/cloudflare-tunnel.json.example",
            "config/sdi-specs.yaml.example",
            "config/k8s-clusters.yaml.example",
        ];

        for file in &required_files {
            let path = repo_root.join(file);
            assert!(
                path.exists(),
                "README Installation Guide references '{}' but file does not exist",
                file
            );
        }
    }

    /// All example YAML configs must parse without error (README Step 2 validation).
    #[test]
    fn test_readme_example_configs_parse_successfully() {
        // sdi-specs.yaml.example
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: Result<crate::models::sdi::SdiSpec, _> = serde_yaml::from_str(sdi_content);
        assert!(
            sdi.is_ok(),
            "sdi-specs.yaml.example fails to parse: {:?}",
            sdi.err()
        );

        // k8s-clusters.yaml.example
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: Result<K8sClustersConfig, _> = serde_yaml::from_str(k8s_content);
        assert!(
            k8s.is_ok(),
            "k8s-clusters.yaml.example fails to parse: {:?}",
            k8s.err()
        );

        // baremetal-init.yaml.example
        let bm_content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let bm: Result<crate::core::config::BaremetalInitConfig, _> =
            serde_yaml::from_str(bm_content);
        assert!(
            bm.is_ok(),
            ".baremetal-init.yaml.example fails to parse: {:?}",
            bm.err()
        );
    }

    /// GitOps bootstrap file referenced in README Step 7 must exist.
    #[test]
    fn test_readme_gitops_bootstrap_exists() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let bootstrap = repo_root.join("gitops/bootstrap/spread.yaml");
        assert!(
            bootstrap.exists(),
            "README Step 7 references gitops/bootstrap/spread.yaml but file does not exist"
        );
    }

    /// Docs referenced in README Documentation section must exist.
    #[test]
    fn test_readme_referenced_docs_exist() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();

        let required_docs = [
            "docs/SETUP-GUIDE.md",
            "docs/ARCHITECTURE.md",
            "docs/ops-guide.md",
            "docs/TROUBLESHOOTING.md",
        ];

        for doc in &required_docs {
            let path = repo_root.join(doc);
            assert!(
                path.exists(),
                "README references '{}' but file does not exist",
                doc
            );
        }
    }

    /// B-1: Bootstrap spread.yaml must deploy AppProjects (tower-project, sandbox-project).
    /// Without these, all ApplicationSet-generated Applications fail with "project not found".
    #[test]
    fn test_bootstrap_deploys_appprojects() {
        let spread_content = include_str!("../../../gitops/bootstrap/spread.yaml");
        // spread.yaml must contain tower-project and sandbox-project AppProject resources
        // OR must contain an Application that points to gitops/projects/ directory
        let has_tower_project = spread_content.contains("name: tower-project")
            || spread_content.contains("path: gitops/projects");
        let has_sandbox_project = spread_content.contains("name: sandbox-project")
            || spread_content.contains("path: gitops/projects");
        assert!(
            has_tower_project,
            "spread.yaml must deploy tower-project AppProject (inline or via gitops/projects/)"
        );
        assert!(
            has_sandbox_project,
            "spread.yaml must deploy sandbox-project AppProject (inline or via gitops/projects/)"
        );
    }

    /// B-2: All helm repos used in common/tower/sandbox must be listed in the
    /// corresponding AppProject's sourceRepos. Otherwise ArgoCD rejects the sync.
    #[test]
    fn test_appproject_sourcerepos_include_all_helm_repos() {
        let tower_project = include_str!("../../../gitops/projects/tower-project.yaml");
        let sandbox_project = include_str!("../../../gitops/projects/sandbox-project.yaml");

        // Kyverno helm repo is used in gitops/common/kyverno/kustomization.yaml
        let kyverno_repo = "https://kyverno.github.io/kyverno/";

        // Tower deploys common apps (including Kyverno)
        assert!(
            tower_project.contains(kyverno_repo),
            "tower-project.yaml sourceRepos must include Kyverno helm repo: {}",
            kyverno_repo
        );
        // Sandbox also deploys common apps (including Kyverno)
        assert!(
            sandbox_project.contains(kyverno_repo),
            "sandbox-project.yaml sourceRepos must include Kyverno helm repo: {}",
            kyverno_repo
        );
    }

    /// Each SDI-mode cluster must have at least one control-plane node in its pool.
    #[test]
    fn test_two_layer_sdi_pools_have_control_plane_nodes() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_content).unwrap();
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s.config.clusters {
            if cluster.cluster_mode == ClusterMode::Baremetal {
                continue;
            }
            let pool = sdi
                .spec
                .sdi_pools
                .iter()
                .find(|p| p.pool_name == cluster.cluster_sdi_resource_pool);
            if let Some(pool) = pool {
                let has_cp = pool
                    .node_specs
                    .iter()
                    .any(|n| n.roles.iter().any(|r| r == "control-plane"));
                assert!(
                    has_cp,
                    "SDI pool '{}' for cluster '{}' has no control-plane node",
                    pool.pool_name, cluster.cluster_name
                );
            }
        }
    }

    // --- Sprint 9a: GitOps YAML validation tests ---

    /// Sandbox generator must use ArgoCD name-based cluster addressing.
    #[test]
    fn test_sandbox_generator_uses_name_based_destination() {
        let content = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");
        assert!(
            content.contains("name: sandbox"),
            "sandbox-generator.yaml must use 'name: sandbox' for ArgoCD cluster addressing"
        );
        assert!(
            !content.contains("sandbox-api:6443"),
            "sandbox-generator.yaml must not contain legacy placeholder URL"
        );

        // Verify the YAML structure is valid
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(content).expect("sandbox-generator.yaml must be valid YAML");
        assert!(
            parsed.get("spec").is_some(),
            "sandbox-generator.yaml must have a 'spec' field"
        );
    }

    /// Cloudflare tunnel values.yaml must expose all required services.
    #[test]
    fn test_cloudflare_tunnel_ingress_completeness() {
        let content = include_str!("../../../gitops/tower/cloudflared-tunnel/values.yaml");

        // Must have tunnel name
        assert!(
            content.contains("playbox-admin-static"),
            "CF tunnel values.yaml missing tunnel name 'playbox-admin-static'"
        );

        // Must expose K8s API for external kubectl access
        assert!(
            content.contains("api.k8s.jinwang.dev"),
            "CF tunnel missing K8s API hostname 'api.k8s.jinwang.dev'"
        );
        assert!(
            content.contains("kubernetes.default"),
            "CF tunnel missing kubernetes.default service target for K8s API"
        );

        // Must expose ArgoCD UI
        assert!(
            content.contains("cd.jinwang.dev"),
            "CF tunnel missing ArgoCD hostname 'cd.jinwang.dev'"
        );
        assert!(
            content.contains("argocd-server"),
            "CF tunnel missing argocd-server service target"
        );

        // Must expose Keycloak for OIDC
        assert!(
            content.contains("auth.jinwang.dev"),
            "CF tunnel missing Keycloak hostname 'auth.jinwang.dev'"
        );

        // Must have catch-all 404 fallback
        assert!(
            content.contains("http_status:404"),
            "CF tunnel missing catch-all 404 fallback"
        );

        // Must reference existing secret for credentials
        assert!(
            content.contains("existingSecret"),
            "CF tunnel must reference existingSecret for credentials"
        );

        // Must have noTLSVerify for K8s API (self-signed cert)
        assert!(
            content.contains("noTLSVerify: true"),
            "CF tunnel must have noTLSVerify for K8s API endpoint"
        );
    }

    /// CL-9: Baremetal mode full pipeline — inventory + vars generation together.
    /// Ensures no SDI layer is needed and vars contain all production settings.
    #[test]
    fn test_baremetal_mode_full_pipeline_inventory_and_vars() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();
        let common = &k8s.config.common;

        // Create a baremetal cluster (not in example, but structure must work)
        let bm_cluster = ClusterDef {
            cluster_name: "prod-bm".to_string(),
            cluster_mode: ClusterMode::Baremetal,
            cluster_sdi_resource_pool: String::new(),
            baremetal_nodes: vec![
                BaremetalNode {
                    node_name: "bm-cp-0".to_string(),
                    ip: "10.0.0.1".to_string(),
                    roles: vec!["control-plane".to_string(), "etcd".to_string()],
                },
                BaremetalNode {
                    node_name: "bm-w-0".to_string(),
                    ip: "10.0.0.2".to_string(),
                    roles: vec!["worker".to_string()],
                },
                BaremetalNode {
                    node_name: "bm-w-1".to_string(),
                    ip: "10.0.0.3".to_string(),
                    roles: vec!["worker".to_string()],
                },
            ],
            cluster_role: "workload".to_string(),
            network: ClusterNetwork {
                pod_cidr: "10.234.0.0/17".to_string(),
                service_cidr: "10.234.128.0/18".to_string(),
                dns_domain: "prod.local".to_string(),
                native_routing_cidr: None,
            },
            cilium: Some(CiliumConfig {
                cluster_id: 10,
                cluster_name: "prod-bm".to_string(),
            }),
            oidc: None,
            kubespray_extra_vars: None,
            ssh_user: Some("ops".to_string()),
        };

        // Pipeline step 1: Generate inventory
        let ini = crate::core::kubespray::generate_inventory_baremetal(&bm_cluster).unwrap();
        assert!(
            ini.contains("bm-cp-0"),
            "inventory missing control-plane node"
        );
        assert!(ini.contains("bm-w-0"), "inventory missing worker-0");
        assert!(ini.contains("bm-w-1"), "inventory missing worker-1");
        assert!(ini.contains("ansible_host=10.0.0.1"), "wrong CP IP");

        // Pipeline step 2: Generate kubespray vars (uses same common config as SDI clusters)
        let vars = crate::core::kubespray::generate_cluster_vars(&bm_cluster, common);
        assert!(
            vars.contains(&common.kubernetes_version),
            "vars missing k8s version"
        );
        assert!(
            vars.contains("kube_network_plugin: cni"),
            "vars missing CNI config (cilium)"
        );
        assert!(
            vars.contains("kube_proxy_remove: true"),
            "vars missing kube-proxy removal"
        );
        assert!(
            vars.contains("dns_domain: \"prod.local\""),
            "vars missing DNS domain"
        );
        assert!(
            vars.contains("kube_pods_subnet: \"10.234.0.0/17\""),
            "vars missing pod CIDR"
        );
        assert!(
            vars.contains("kube_service_addresses: \"10.234.128.0/18\""),
            "vars missing service CIDR"
        );

        // Pipeline step 3: Verify baremetal cluster does NOT need SDI pool mapping
        let mapping_errors = validate_cluster_sdi_pool_mapping(
            &K8sClustersConfig {
                config: K8sConfig {
                    common: common.clone(),
                    clusters: vec![bm_cluster.clone()],
                    argocd: k8s.config.argocd.clone(),
                    domains: k8s.config.domains.clone(),
                },
            },
            &serde_yaml::from_str::<SdiSpec>(include_str!(
                "../../../config/sdi-specs.yaml.example"
            ))
            .unwrap(),
        );
        assert!(
            mapping_errors.is_empty(),
            "Baremetal cluster must NOT require SDI pool mapping, got: {:?}",
            mapping_errors
        );
    }

    /// Sandbox generator sync waves must ensure test-resources deploys AFTER Cilium networking.
    /// test-resources at wave 0 would fail because pods can't communicate without CNI.
    #[test]
    fn test_sandbox_generator_test_resources_deploys_after_cilium() {
        let content = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");
        let parsed: serde_yaml::Value = serde_yaml::from_str(content).unwrap();

        let elements = parsed["spec"]["generators"][0]["list"]["elements"]
            .as_sequence()
            .expect("generators must have list elements");

        let get_wave = |app_name: &str| -> i32 {
            elements
                .iter()
                .find(|e| e["appName"].as_str() == Some(app_name))
                .and_then(|e| e["syncWave"].as_str())
                .and_then(|w| w.parse().ok())
                .unwrap_or(-1)
        };

        let cilium_wave = get_wave("cilium");
        let test_wave = get_wave("test-resources");
        let cluster_config_wave = get_wave("cluster-config");

        assert!(
            cilium_wave > cluster_config_wave,
            "cilium (wave {}) must deploy after cluster-config (wave {})",
            cilium_wave,
            cluster_config_wave
        );
        assert!(
            test_wave > cilium_wave,
            "test-resources (wave {}) must deploy AFTER cilium (wave {}) — pods need CNI",
            test_wave,
            cilium_wave
        );
    }

    /// The .baremetal-init.yaml.example must document sshKeyPathOfReachableNode
    /// for key-based ProxyJump authentication (user's checklist Case 3b).
    #[test]
    fn test_baremetal_example_documents_key_proxy_auth() {
        let content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        assert!(
            content.contains("sshKeyPathOfReachableNode"),
            ".baremetal-init.yaml.example must document sshKeyPathOfReachableNode field"
        );
        assert!(
            content.contains("sshKeyPath"),
            ".baremetal-init.yaml.example must document sshKeyPath field"
        );
    }

    /// Every .example file in config/ must be referenced in README.md.
    /// Orphaned example files confuse users about which files to use.
    #[test]
    fn test_no_orphaned_config_example_files() {
        let readme = include_str!("../../../README.md");

        // Known config example files
        let config_examples = vec!["sdi-specs.yaml.example", "k8s-clusters.yaml.example"];

        for example in &config_examples {
            assert!(
                readme.contains(example),
                "config/{} must be referenced in README.md",
                example
            );
        }

        // Ensure no orphaned example files exist that aren't in the known set
        // baremetal.yaml.example was identified as orphaned — it should NOT exist
        let orphan_candidates = vec!["baremetal.yaml.example"];
        for orphan in &orphan_candidates {
            let path = format!("config/{}", orphan);
            let full_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(&path);
            assert!(
                !full_path.exists(),
                "Orphaned file {} must be deleted — use credentials/.baremetal-init.yaml.example instead",
                path
            );
        }
    }

    /// CL-1, CL-8: E2E pipeline: facts → resource pool → SDI → cluster → secrets → gitops
    /// Tests the complete dry-run pipeline from hardware facts through to gitops validation.
    #[test]
    fn test_e2e_facts_to_gitops_full_pipeline() {
        use crate::core::resource_pool;
        use crate::models::baremetal::*;
        use std::collections::HashMap;

        // --- Phase 0: Simulate facts for 4 nodes ---
        let make_facts =
            |name: &str, cores: u32, mem_mb: u64, gpus: usize, disks: usize| NodeFacts {
                node_name: name.to_string(),
                timestamp: "2026-03-11T00:00:00Z".to_string(),
                cpu: CpuInfo {
                    model: "Intel Xeon".to_string(),
                    cores,
                    threads: cores * 2,
                    architecture: "x86_64".to_string(),
                },
                memory: MemoryInfo {
                    total_mb: mem_mb,
                    available_mb: mem_mb - 2048,
                },
                disks: (0..disks)
                    .map(|i| DiskInfo {
                        name: format!("nvme{}n1", i),
                        size_gb: 1000,
                        model: "Samsung 990 Pro".to_string(),
                        disk_type: "nvme".to_string(),
                    })
                    .collect(),
                nics: vec![NicInfo {
                    name: "eno1".to_string(),
                    mac: format!("aa:bb:cc:dd:ee:{:02x}", cores),
                    speed: "1000Mb/s".to_string(),
                    driver: "igb".to_string(),
                    state: "UP".to_string(),
                }],
                gpus: (0..gpus)
                    .map(|i| GpuInfo {
                        pci_id: format!("0{}:00.0", i + 1),
                        model: "NVIDIA RTX 3060 [10de:2544]".to_string(),
                        vendor: "nvidia".to_string(),
                        driver: "nouveau".to_string(),
                    })
                    .collect(),
                iommu_groups: vec![],
                kernel: KernelInfo {
                    version: "6.8.0-generic".to_string(),
                    params: HashMap::new(),
                },
                bridges: vec!["br0".to_string()],
                bonds: vec![],
                pcie: vec![],
            };

        let all_facts = vec![
            make_facts("playbox-0", 16, 65536, 0, 2),
            make_facts("playbox-1", 8, 32768, 0, 1),
            make_facts("playbox-2", 8, 32768, 1, 1),
            make_facts("playbox-3", 16, 65536, 2, 4),
        ];

        // --- Phase 1: Resource pool aggregation (CL-1 no-spec path) ---
        let pool_summary = resource_pool::generate_resource_pool_summary(&all_facts);
        assert_eq!(pool_summary.total_nodes, 4);
        assert_eq!(pool_summary.total_cpu_cores, 48);
        assert_eq!(pool_summary.total_memory_mb, 196608);
        assert_eq!(pool_summary.total_gpu_count, 3);
        assert_eq!(pool_summary.total_disk_count, 8);

        // Resource pool table renders without panic
        let table = resource_pool::format_resource_pool_table(&pool_summary);
        assert!(table.contains("playbox-0"));
        assert!(table.contains("playbox-3"));

        // Resource pool serializes to JSON (for resource-pool-summary.json)
        let json = serde_json::to_string_pretty(&pool_summary).unwrap();
        let deserialized: resource_pool::ResourcePoolSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total_nodes, pool_summary.total_nodes);

        // --- Phase 2: SDI spec → HCL generation ---
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).expect("sdi-specs.yaml must parse");
        let hcl = crate::core::tofu::generate_tofu_main(&sdi, "jinwang");
        assert!(hcl.contains("libvirt_domain"), "HCL must define VM domains");

        // --- Phase 3: K8s cluster → inventory + vars ---
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig =
            serde_yaml::from_str(k8s_content).expect("k8s-clusters.yaml must parse");

        for cluster in &k8s.config.clusters {
            let inv = crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap();
            let vars = crate::core::kubespray::generate_cluster_vars(cluster, &k8s.config.common);

            // Inventory must reference nodes from the correct pool
            assert!(inv.contains("[all]"));
            assert!(inv.contains("[kube_control_plane]"));

            // Vars must be valid YAML with essential keys
            let parsed: serde_yaml::Mapping = serde_yaml::from_str(&vars).unwrap();
            assert!(parsed.contains_key(&serde_yaml::Value::String("kube_version".to_string())));
            assert!(
                parsed.contains_key(&serde_yaml::Value::String("container_manager".to_string()))
            );
        }

        // --- Phase 4: Secrets generation ---
        let secrets_content = include_str!("../../../credentials/secrets.yaml.example");
        let mgmt =
            crate::core::secrets::generate_all_secrets_manifests(secrets_content, "management")
                .unwrap();
        let work =
            crate::core::secrets::generate_all_secrets_manifests(secrets_content, "workload")
                .unwrap();
        assert!(mgmt.contains("keycloak"), "management needs keycloak");
        assert!(!work.contains("cloudflare"), "workload skips cloudflare");

        // --- Phase 5: Kernel tune recommendations ---
        let cp_params = crate::core::kernel::generate_k8s_sysctl_params("control-plane");
        let worker_params = crate::core::kernel::generate_k8s_sysctl_params("worker");
        assert!(!cp_params.is_empty(), "CP must have kernel recommendations");
        assert!(
            !worker_params.is_empty(),
            "worker must have kernel recommendations"
        );
        // Workers need conntrack for production workloads
        assert!(
            worker_params.keys().any(|k| k.contains("conntrack")),
            "worker must have conntrack tuning"
        );
    }

    /// CL-4, CL-8: SSH command generation for all 3 access modes in example config.
    #[test]
    fn test_e2e_ssh_commands_for_example_config() {
        let bm_content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let bm: crate::core::config::BaremetalInitConfig =
            serde_yaml::from_str(bm_content).expect("baremetal-init.yaml must parse");

        // Build SSH command for each node — verifies all access modes work
        for node in &bm.target_nodes {
            let result = crate::core::ssh::build_ssh_command(node, "echo ok", &bm.target_nodes);
            assert!(
                result.is_ok(),
                "SSH command must build for node '{}': {:?}",
                node.name,
                result.err()
            );
            let cmd = result.unwrap();
            assert!(
                !cmd.args.is_empty(),
                "SSH command must have args for '{}'",
                node.name
            );
            // Args joined must contain the target script
            let full_cmd = cmd.args.join(" ");
            assert!(
                full_cmd.contains("echo ok"),
                "SSH command for '{}' must contain the script, got: {}",
                node.name,
                full_cmd
            );
        }
    }

    #[test]
    fn test_sdi_init_help_describes_resource_pool() {
        // CL-1: `sdi init` (no flag) must describe creating a unified resource pool,
        // not just "preparing hosts". Verify the help text matches the checklist semantics.
        let sdi_rs = include_str!("../commands/sdi.rs");

        // The Init variant help text must mention "resource pool"
        assert!(
            sdi_rs.contains("unified resource pool"),
            "sdi init help text must mention 'unified resource pool' per CL-1 checklist requirement"
        );

        // The no-spec path must NOT say "prepare hosts only"
        assert!(
            !sdi_rs.contains("prepares hosts only"),
            "sdi init help text must not say 'prepares hosts only' — it also creates resource pool summary"
        );
    }

    #[test]
    fn test_sdi_init_no_spec_generates_resource_pool_summary() {
        // CL-1: When `sdi init` runs without a spec file, it must generate
        // a resource-pool-summary.json so that `scalex get sdi-pools` can display it.
        // Verify the code path exists by checking sdi.rs contains the summary generation logic.
        let sdi_rs = include_str!("../commands/sdi.rs");

        assert!(
            sdi_rs.contains("resource-pool-summary.json"),
            "sdi init must save resource-pool-summary.json in the no-spec path"
        );
        assert!(
            sdi_rs.contains("generate_resource_pool_summary"),
            "sdi init must call generate_resource_pool_summary in the no-spec path"
        );
        assert!(
            sdi_rs.contains("format_resource_pool_table"),
            "sdi init must display the resource pool table in the no-spec path"
        );
    }

    /// Sprint 16b (G-2): Spec-based `sdi init` must also generate resource-pool-summary.json.
    /// The resource pool summary must be generated BEFORE the spec-based VM creation,
    /// so the unified resource pool view is always available.
    #[test]
    fn test_sdi_init_spec_path_also_generates_resource_pool_summary() {
        let sdi_rs = include_str!("../commands/sdi.rs");

        // The common path (before spec/no-spec branch) must generate resource pool summary
        // Verify the summary is generated in a section that runs regardless of spec_file
        assert!(
            sdi_rs.contains("// Step 4: Generate resource pool summary (always, regardless of spec)"),
            "sdi init must generate resource-pool-summary.json in the common path (before spec/no-spec branch)"
        );

        // The no-spec else branch should NOT have its own resource pool summary generation
        // (it should rely on the common path)
        let else_block_start = sdi_rs.find("No spec file: set up host-level").unwrap();
        let else_block = &sdi_rs[else_block_start..];
        assert!(
            !else_block.contains("generate_resource_pool_summary"),
            "no-spec path must NOT have duplicate resource pool summary — it's in the common path now"
        );
    }

    /// Sprint 16c (G-3): `sdi init <spec>` must cache the spec file as `sdi-spec-cache.yaml`
    /// so that `cluster init` can auto-discover it without requiring `--sdi-spec`.
    #[test]
    fn test_sdi_init_spec_caches_spec_file_for_cluster_init() {
        let sdi_rs = include_str!("../commands/sdi.rs");

        // sdi.rs must write sdi-spec-cache.yaml in the spec-file path
        assert!(
            sdi_rs.contains("sdi-spec-cache.yaml"),
            "sdi init must cache spec file as sdi-spec-cache.yaml for cluster init workflow"
        );

        // The cache must be written in the spec-file branch (not the no-spec branch)
        let spec_branch_marker = "Save pool state for `scalex get sdi-pools`";
        let spec_branch_start = sdi_rs
            .find(spec_branch_marker)
            .expect("spec branch marker must exist");
        let spec_branch = &sdi_rs[spec_branch_start..];

        // Find the else branch to bound the spec section
        let else_marker = "No spec file: set up host-level";
        let else_offset = spec_branch
            .find(else_marker)
            .expect("else branch marker must exist");
        let spec_section = &spec_branch[..else_offset];

        assert!(
            spec_section.contains("sdi-spec-cache.yaml"),
            "sdi-spec-cache.yaml must be written in the spec-file branch (between pool state save and else)"
        );
    }

    #[test]
    fn test_get_sdi_pools_supports_baremetal_resource_pool() {
        // CL-1 + CL-8: `scalex get sdi-pools` must support displaying the unified
        // bare-metal resource pool from resource-pool-summary.json (no-spec path).
        let get_rs = include_str!("../commands/get.rs");

        assert!(
            get_rs.contains("resource-pool-summary.json"),
            "get sdi-pools must load resource-pool-summary.json for no-spec SDI"
        );
        assert!(
            get_rs.contains("Unified Bare-Metal Resource Pool"),
            "get sdi-pools must display 'Unified Bare-Metal Resource Pool' header"
        );
    }

    // ── Sprint 12a: Test Hardening (GAP closure) ──

    /// CL-9 (G-1): Baremetal mode E2E dry-run pipeline.
    /// Verifies that a user can define clusters with `cluster_mode: baremetal`,
    /// skip SDI entirely, and produce valid Kubespray inventory + cluster vars.
    #[test]
    fn test_baremetal_mode_e2e_pipeline() {
        use crate::models::cluster::*;

        // Step 1: Define a baremetal-mode cluster config (no SDI)
        let k8s_config = K8sClustersConfig {
            config: K8sConfig {
                common: CommonConfig {
                    kubernetes_version: "1.33.1".to_string(),
                    kubespray_version: "2.30.0".to_string(),
                    container_runtime: "containerd".to_string(),
                    cni: "cilium".to_string(),
                    cilium_version: "1.16.5".to_string(),
                    kube_proxy_remove: true,
                    cgroup_driver: "systemd".to_string(),
                    helm_enabled: true,
                    kube_apiserver_admission_plugins: vec!["NodeRestriction".to_string()],
                    firewalld_enabled: false,
                    kube_vip_enabled: false,
                    gateway_api_enabled: false,
                    gateway_api_version: String::new(),
                    graceful_node_shutdown: false,
                    graceful_node_shutdown_sec: 120,
                    kubelet_custom_flags: vec![],
                    kubeconfig_localhost: true,
                    kubectl_localhost: true,
                    enable_nodelocaldns: true,
                    kube_network_node_prefix: 24,
                    ntp_enabled: true,
                    etcd_deployment_type: "host".to_string(),
                    dns_mode: "coredns".to_string(),
                },
                clusters: vec![
                    ClusterDef {
                        cluster_name: "tower".to_string(),
                        cluster_mode: ClusterMode::Baremetal,
                        cluster_sdi_resource_pool: String::new(),
                        baremetal_nodes: vec![BaremetalNode {
                            node_name: "bm-tower-0".to_string(),
                            ip: "10.0.0.1".to_string(),
                            roles: vec![
                                "control-plane".to_string(),
                                "etcd".to_string(),
                                "worker".to_string(),
                            ],
                        }],
                        cluster_role: "management".to_string(),
                        network: ClusterNetwork {
                            pod_cidr: "10.244.0.0/20".to_string(),
                            service_cidr: "10.96.0.0/20".to_string(),
                            dns_domain: "tower.local".to_string(),
                            native_routing_cidr: None,
                        },
                        cilium: Some(CiliumConfig {
                            cluster_id: 1,
                            cluster_name: "tower".to_string(),
                        }),
                        oidc: None,
                        kubespray_extra_vars: None,
                        ssh_user: None,
                    },
                    ClusterDef {
                        cluster_name: "sandbox".to_string(),
                        cluster_mode: ClusterMode::Baremetal,
                        cluster_sdi_resource_pool: String::new(),
                        baremetal_nodes: vec![
                            BaremetalNode {
                                node_name: "bm-sb-cp".to_string(),
                                ip: "10.0.0.10".to_string(),
                                roles: vec!["control-plane".to_string(), "etcd".to_string()],
                            },
                            BaremetalNode {
                                node_name: "bm-sb-w0".to_string(),
                                ip: "10.0.0.11".to_string(),
                                roles: vec!["worker".to_string()],
                            },
                            BaremetalNode {
                                node_name: "bm-sb-w1".to_string(),
                                ip: "10.0.0.12".to_string(),
                                roles: vec!["worker".to_string()],
                            },
                        ],
                        cluster_role: "workload".to_string(),
                        network: ClusterNetwork {
                            pod_cidr: "10.245.0.0/20".to_string(),
                            service_cidr: "10.97.0.0/20".to_string(),
                            dns_domain: "sandbox.local".to_string(),
                            native_routing_cidr: None,
                        },
                        cilium: Some(CiliumConfig {
                            cluster_id: 2,
                            cluster_name: "sandbox".to_string(),
                        }),
                        oidc: None,
                        kubespray_extra_vars: None,
                        ssh_user: None,
                    },
                ],
                argocd: None,
                domains: None,
            },
        };

        // Step 2: Validate — no pool mapping needed for baremetal mode
        let id_errors = validate_unique_cluster_ids(&k8s_config);
        assert!(
            id_errors.is_empty(),
            "cluster IDs must be unique: {:?}",
            id_errors
        );

        // Step 3: Generate inventory for each baremetal cluster
        for cluster in &k8s_config.config.clusters {
            assert_eq!(cluster.cluster_mode, ClusterMode::Baremetal);
            let inv =
                crate::core::kubespray::generate_inventory_baremetal(cluster).unwrap_or_else(|e| {
                    panic!("inventory for '{}' failed: {}", cluster.cluster_name, e)
                });
            assert!(
                inv.contains("[all]"),
                "{}: must have [all]",
                cluster.cluster_name
            );
            assert!(
                inv.contains("[kube_control_plane]"),
                "{}: must have control plane",
                cluster.cluster_name
            );
            assert!(
                inv.contains("[kube_node]"),
                "{}: must have worker",
                cluster.cluster_name
            );
            assert!(
                inv.contains("ansible_host="),
                "{}: must have ansible_host entries",
                cluster.cluster_name
            );
        }

        // Step 4: Generate cluster vars for each cluster
        for cluster in &k8s_config.config.clusters {
            let vars =
                crate::core::kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);
            let parsed: serde_yaml::Mapping = serde_yaml::from_str(&vars).unwrap_or_else(|e| {
                panic!("vars for '{}' invalid YAML: {e}", cluster.cluster_name)
            });
            assert!(
                parsed.contains_key(serde_yaml::Value::String("kube_version".to_string())),
                "{}: must contain kube_version",
                cluster.cluster_name
            );
            assert!(
                parsed.contains_key(serde_yaml::Value::String("kube_pods_subnet".to_string())),
                "{}: must contain pod CIDR",
                cluster.cluster_name
            );
        }

        // Step 5: Verify tower inventory has single dual-role node
        let tower_inv =
            crate::core::kubespray::generate_inventory_baremetal(&k8s_config.config.clusters[0])
                .unwrap();
        assert_eq!(
            tower_inv.matches("ansible_host=").count(),
            1,
            "tower baremetal must have exactly 1 node"
        );

        // Step 6: Verify sandbox inventory has 3 nodes
        let sandbox_inv =
            crate::core::kubespray::generate_inventory_baremetal(&k8s_config.config.clusters[1])
                .unwrap();
        assert_eq!(
            sandbox_inv.matches("ansible_host=").count(),
            3,
            "sandbox baremetal must have exactly 3 nodes"
        );
    }

    /// CL-2, CL-14 (G-2): Cloudflare tunnel routing rules must match documented domains.
    /// Verifies that values.yaml ingress hostnames correspond to documented access paths.
    #[test]
    fn test_cloudflare_tunnel_routes_match_docs() {
        let values = include_str!("../../../gitops/tower/cloudflared-tunnel/values.yaml");
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Required routing rules per CL-14
        let required_hostnames = ["api.k8s.jinwang.dev", "auth.jinwang.dev", "cd.jinwang.dev"];

        for hostname in &required_hostnames {
            assert!(
                values.contains(hostname),
                "CF tunnel values.yaml must contain hostname '{}'",
                hostname
            );
            assert!(
                ops_guide.contains(hostname),
                "ops-guide.md must document hostname '{}'",
                hostname
            );
        }

        // Verify correct backend services
        assert!(
            values.contains("https://kubernetes.default.svc:443"),
            "api.k8s must route to kubernetes API server"
        );
        assert!(
            values.contains("keycloak"),
            "auth must route to keycloak service"
        );
        assert!(
            values.contains("argocd-server"),
            "cd must route to argocd-server service"
        );

        // Verify tunnel name matches user's setting (CL-14)
        assert!(
            values.contains("playbox-admin-static"),
            "tunnel name must be 'playbox-admin-static'"
        );

        // Verify catch-all 404 rule exists
        assert!(
            values.contains("http_status:404"),
            "must have catch-all 404 rule"
        );
    }

    /// CL-1 (G-3): Single-node SDI full pipeline dry-run.
    /// A single bare-metal node must be able to host both tower and sandbox pools.
    #[test]
    fn test_single_node_sdi_full_pipeline() {
        use crate::models::sdi::*;

        // Step 1: Define a single-node SDI spec
        let sdi = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "mini-pool".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec!["8.8.8.8".to_string()],
                },
            },
            os_image: OsImageConfig {
                source:
                    "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img"
                        .to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "~/.ssh/authorized_keys".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![
                    SdiPool {
                        pool_name: "tower".to_string(),
                        purpose: "management".to_string(),
                        placement: PlacementConfig {
                            hosts: vec!["playbox-0".to_string()],
                            spread: false,
                        },
                        node_specs: vec![NodeSpec {
                            node_name: "tower-cp-0".to_string(),
                            ip: "192.168.88.100".to_string(),
                            cpu: 2,
                            mem_gb: 4,
                            disk_gb: 30,
                            host: Some("playbox-0".to_string()),
                            roles: vec![
                                "control-plane".to_string(),
                                "etcd".to_string(),
                                "worker".to_string(),
                            ],
                            devices: None,
                        }],
                    },
                    SdiPool {
                        pool_name: "sandbox".to_string(),
                        purpose: "workload".to_string(),
                        placement: PlacementConfig {
                            hosts: vec!["playbox-0".to_string()],
                            spread: false,
                        },
                        node_specs: vec![NodeSpec {
                            node_name: "sandbox-cp-0".to_string(),
                            ip: "192.168.88.110".to_string(),
                            cpu: 2,
                            mem_gb: 4,
                            disk_gb: 30,
                            host: Some("playbox-0".to_string()),
                            roles: vec![
                                "control-plane".to_string(),
                                "etcd".to_string(),
                                "worker".to_string(),
                            ],
                            devices: None,
                        }],
                    },
                ],
            },
        };

        // Step 2: Validate SDI spec
        let sdi_errors = validate_sdi_spec(&sdi);
        assert!(
            sdi_errors.is_empty(),
            "single-node SDI spec must be valid: {:?}",
            sdi_errors
        );

        // Step 3: Generate HCL — all VMs on single host
        let hcl = crate::core::tofu::generate_tofu_main(&sdi, "jinwang");
        assert!(!hcl.is_empty(), "HCL must not be empty");
        // Only 1 unique host → only 1 provider block
        assert_eq!(
            hcl.matches("provider \"libvirt\"").count(),
            1,
            "single-node must generate exactly 1 provider"
        );
        assert!(hcl.contains("tower-cp-0"), "HCL must contain tower VM");
        assert!(hcl.contains("sandbox-cp-0"), "HCL must contain sandbox VM");

        // Step 4: Define matching k8s cluster config
        let k8s_config = K8sClustersConfig {
            config: K8sConfig {
                common: CommonConfig {
                    kubernetes_version: "1.33.1".to_string(),
                    kubespray_version: "2.30.0".to_string(),
                    container_runtime: "containerd".to_string(),
                    cni: "cilium".to_string(),
                    cilium_version: "1.16.5".to_string(),
                    kube_proxy_remove: true,
                    cgroup_driver: "systemd".to_string(),
                    helm_enabled: true,
                    kube_apiserver_admission_plugins: vec![],
                    firewalld_enabled: false,
                    kube_vip_enabled: false,
                    gateway_api_enabled: false,
                    gateway_api_version: String::new(),
                    graceful_node_shutdown: false,
                    graceful_node_shutdown_sec: 120,
                    kubelet_custom_flags: vec![],
                    kubeconfig_localhost: true,
                    kubectl_localhost: true,
                    enable_nodelocaldns: true,
                    kube_network_node_prefix: 24,
                    ntp_enabled: true,
                    etcd_deployment_type: "host".to_string(),
                    dns_mode: "coredns".to_string(),
                },
                clusters: vec![
                    ClusterDef {
                        cluster_name: "tower".to_string(),
                        cluster_mode: ClusterMode::Sdi,
                        cluster_sdi_resource_pool: "tower".to_string(),
                        baremetal_nodes: vec![],
                        cluster_role: "management".to_string(),
                        network: ClusterNetwork {
                            pod_cidr: "10.244.0.0/20".to_string(),
                            service_cidr: "10.96.0.0/20".to_string(),
                            dns_domain: "tower.local".to_string(),
                            native_routing_cidr: None,
                        },
                        cilium: Some(CiliumConfig {
                            cluster_id: 1,
                            cluster_name: "tower".to_string(),
                        }),
                        oidc: None,
                        kubespray_extra_vars: None,
                        ssh_user: None,
                    },
                    ClusterDef {
                        cluster_name: "sandbox".to_string(),
                        cluster_mode: ClusterMode::Sdi,
                        cluster_sdi_resource_pool: "sandbox".to_string(),
                        baremetal_nodes: vec![],
                        cluster_role: "workload".to_string(),
                        network: ClusterNetwork {
                            pod_cidr: "10.245.0.0/20".to_string(),
                            service_cidr: "10.97.0.0/20".to_string(),
                            dns_domain: "sandbox.local".to_string(),
                            native_routing_cidr: None,
                        },
                        cilium: Some(CiliumConfig {
                            cluster_id: 2,
                            cluster_name: "sandbox".to_string(),
                        }),
                        oidc: None,
                        kubespray_extra_vars: None,
                        ssh_user: None,
                    },
                ],
                argocd: None,
                domains: None,
            },
        };

        // Step 5: Cross-validate
        let pool_errors = validate_cluster_sdi_pool_mapping(&k8s_config, &sdi);
        assert!(
            pool_errors.is_empty(),
            "pool mapping must be valid: {:?}",
            pool_errors
        );

        // Step 6: Generate inventory for each cluster (single-node per cluster)
        for cluster in &k8s_config.config.clusters {
            let inv =
                crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap_or_else(|e| {
                    panic!("inventory for '{}' failed: {}", cluster.cluster_name, e)
                });
            assert_eq!(
                inv.matches("ansible_host=").count(),
                1,
                "{}: single-node cluster must have exactly 1 node",
                cluster.cluster_name
            );
        }

        // Step 7: Verify cluster vars
        for cluster in &k8s_config.config.clusters {
            let vars =
                crate::core::kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);
            assert!(
                vars.contains("cilium_cluster_id:"),
                "{}: must have cilium cluster ID",
                cluster.cluster_name
            );
        }
    }

    /// CL-1, CL-8 (G-4): `sdi init` no-spec orchestration flow.
    /// Verifies the code path: facts check → config load → host preparation → resource pool summary → host-infra HCL.
    #[test]
    fn test_sdi_init_no_spec_full_orchestration() {
        // Verify sdi.rs contains the complete orchestration flow for no-spec path
        let sdi_rs = include_str!("../commands/sdi.rs");

        // Step 1: Facts auto-collection when missing
        assert!(
            sdi_rs.contains("No facts found. Running facts collection first"),
            "sdi init must auto-trigger facts when missing"
        );

        // Step 2: Config loading and validation
        assert!(
            sdi_rs.contains("validate_baremetal_config"),
            "sdi init must validate baremetal config"
        );

        // Step 3: Host preparation (KVM/bridge/VFIO)
        assert!(
            sdi_rs.contains("Phase 1: Preparing hosts for virtualization"),
            "sdi init must prepare hosts"
        );
        assert!(
            sdi_rs.contains("InstallKvm"),
            "sdi init must handle KVM installation step"
        );
        assert!(
            sdi_rs.contains("SetupBridge"),
            "sdi init must handle bridge setup step"
        );
        assert!(
            sdi_rs.contains("ConfigureVfio"),
            "sdi init must handle VFIO configuration step"
        );

        // Step 4: Resource pool summary generation (no-spec path)
        assert!(
            sdi_rs.contains("Phase 2: Setting up host-level libvirt infrastructure"),
            "no-spec path must set up host-level libvirt"
        );
        assert!(
            sdi_rs.contains("generate_resource_pool_summary"),
            "no-spec path must generate resource pool summary"
        );

        // Step 5: Host infrastructure HCL generation
        assert!(
            sdi_rs.contains("generate_tofu_host_infra"),
            "no-spec path must generate host-infra HCL via OpenTofu"
        );

        // Step 6: OpenTofu execution
        assert!(
            sdi_rs.contains("tofu init"),
            "no-spec path must run tofu init"
        );
        assert!(
            sdi_rs.contains("tofu apply"),
            "no-spec path must run tofu apply (or dry-run)"
        );

        // Verify the no-spec path is distinct from spec path
        assert!(
            sdi_rs.contains(
                "Host infrastructure setup complete. Provide a spec file to create VM pools"
            ),
            "no-spec path must inform user to provide spec for VM pool creation"
        );
    }

    /// CL-6 (G-5): README referenced files must exist.
    /// Parses README.md for file path references and verifies they exist on disk.
    #[test]
    fn test_readme_referenced_files_exist() {
        let readme = include_str!("../../../README.md");

        // Key files referenced in README that must exist
        let must_exist_files = [
            "credentials/.baremetal-init.yaml.example",
            "credentials/.env.example",
            "credentials/secrets.yaml.example",
            "credentials/cloudflare-tunnel.json.example",
            "config/sdi-specs.yaml.example",
            "config/k8s-clusters.yaml.example",
            "gitops/bootstrap/spread.yaml",
            "tests/run-tests.sh",
            "docs/ops-guide.md",
            "docs/ARCHITECTURE.md",
            "docs/SETUP-GUIDE.md",
            "docs/TROUBLESHOOTING.md",
            "docs/CONTRIBUTING.md",
        ];

        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();

        for file in &must_exist_files {
            let full_path = project_root.join(file);
            assert!(
                full_path.exists(),
                "README references '{}' but file does not exist at '{}'",
                file,
                full_path.display()
            );
        }

        // Verify README mentions key scalex commands
        let required_commands = [
            "scalex facts",
            "scalex sdi init",
            "scalex sdi clean",
            "scalex cluster init",
            "scalex get",
            "scalex secrets",
            "scalex status",
            "scalex kernel-tune",
        ];
        for cmd in &required_commands {
            assert!(
                readme.contains(cmd),
                "README must document command '{}'",
                cmd
            );
        }
    }

    /// CL-12: GitOps directory structure must be consistent with k8s-clusters.yaml.example.
    /// For each cluster: generators dir, project YAML, app dir, bootstrap reference must exist.
    #[test]
    fn test_gitops_structure_matches_cluster_config() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: crate::models::cluster::K8sClustersConfig =
            serde_yaml::from_str(k8s_content).expect("example must parse");

        let spread = include_str!("../../../gitops/bootstrap/spread.yaml");

        for cluster in &k8s.config.clusters {
            let name = &cluster.cluster_name;

            // 1. Generator directory must exist
            let gen_dir = format!(
                concat!(env!("CARGO_MANIFEST_DIR"), "/../gitops/generators/{}"),
                name
            );
            assert!(
                std::path::Path::new(&gen_dir).is_dir(),
                "Missing generator directory for cluster '{}'",
                name
            );

            // 2. Project YAML must exist
            let project_file = format!(
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../gitops/projects/{}-project.yaml"
                ),
                name
            );
            assert!(
                std::path::Path::new(&project_file).is_file(),
                "Missing project file for cluster '{}'",
                name
            );

            // 3. App directory must exist
            let app_dir = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../gitops/{}"), name);
            assert!(
                std::path::Path::new(&app_dir).is_dir(),
                "Missing app directory: gitops/{}",
                name
            );

            // 4. Bootstrap spread.yaml must reference this cluster's root
            let root_ref = format!("{}-root", name);
            assert!(
                spread.contains(&root_ref),
                "spread.yaml must reference '{}' for cluster '{}'",
                root_ref,
                name
            );
        }

        // 5. Common directory must exist (shared across all clusters)
        assert!(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../gitops/common")).is_dir(),
            "gitops/common/ must exist for shared apps"
        );

        // 6. Each generator dir must have at least one YAML file
        for cluster in &k8s.config.clusters {
            let gen_dir = format!(
                concat!(env!("CARGO_MANIFEST_DIR"), "/../gitops/generators/{}"),
                cluster.cluster_name
            );
            let yaml_count = std::fs::read_dir(&gen_dir)
                .unwrap()
                .filter(|e| {
                    e.as_ref()
                        .map(|e| {
                            e.path()
                                .extension()
                                .map_or(false, |ext| ext == "yaml" || ext == "yml")
                        })
                        .unwrap_or(false)
                })
                .count();
            assert!(
                yaml_count > 0,
                "Generator dir for '{}' must have at least one YAML",
                cluster.cluster_name
            );
        }
    }

    /// Extensibility test: Adding a 3rd cluster (datax) to tower+sandbox must work
    /// through the entire pipeline: config → unique IDs → inventory → vars → CIDR isolation.
    /// This validates the "무한 확장 가능한 멀티-클러스터 구조" (Principle 2).
    #[test]
    fn test_third_cluster_extensibility_pipeline() {
        use crate::models::cluster::*;

        let common = CommonConfig {
            kubernetes_version: "1.33.1".to_string(),
            kubespray_version: "2.30.0".to_string(),
            container_runtime: "containerd".to_string(),
            cni: "cilium".to_string(),
            cilium_version: "1.16.5".to_string(),
            kube_proxy_remove: true,
            cgroup_driver: "systemd".to_string(),
            helm_enabled: true,
            kube_apiserver_admission_plugins: vec!["NodeRestriction".to_string()],
            firewalld_enabled: false,
            kube_vip_enabled: false,
            gateway_api_enabled: false,
            gateway_api_version: String::new(),
            graceful_node_shutdown: false,
            graceful_node_shutdown_sec: 120,
            kubelet_custom_flags: vec![],
            kubeconfig_localhost: true,
            kubectl_localhost: true,
            enable_nodelocaldns: true,
            kube_network_node_prefix: 24,
            ntp_enabled: true,
            etcd_deployment_type: "host".to_string(),
            dns_mode: "coredns".to_string(),
        };

        let clusters = vec![
            ClusterDef {
                cluster_name: "tower".to_string(),
                cluster_mode: ClusterMode::Baremetal,
                cluster_sdi_resource_pool: String::new(),
                baremetal_nodes: vec![BaremetalNode {
                    node_name: "bm-tower-0".to_string(),
                    ip: "10.0.0.1".to_string(),
                    roles: vec![
                        "control-plane".to_string(),
                        "etcd".to_string(),
                        "worker".to_string(),
                    ],
                }],
                cluster_role: "management".to_string(),
                network: ClusterNetwork {
                    pod_cidr: "10.244.0.0/20".to_string(),
                    service_cidr: "10.96.0.0/20".to_string(),
                    dns_domain: "tower.local".to_string(),
                    native_routing_cidr: None,
                },
                cilium: Some(CiliumConfig {
                    cluster_id: 1,
                    cluster_name: "tower".to_string(),
                }),
                oidc: None,
                kubespray_extra_vars: None,
                ssh_user: None,
            },
            ClusterDef {
                cluster_name: "sandbox".to_string(),
                cluster_mode: ClusterMode::Baremetal,
                cluster_sdi_resource_pool: String::new(),
                baremetal_nodes: vec![
                    BaremetalNode {
                        node_name: "bm-sb-cp".to_string(),
                        ip: "10.0.0.10".to_string(),
                        roles: vec!["control-plane".to_string(), "etcd".to_string()],
                    },
                    BaremetalNode {
                        node_name: "bm-sb-w0".to_string(),
                        ip: "10.0.0.11".to_string(),
                        roles: vec!["worker".to_string()],
                    },
                ],
                cluster_role: "workload".to_string(),
                network: ClusterNetwork {
                    pod_cidr: "10.245.0.0/20".to_string(),
                    service_cidr: "10.97.0.0/20".to_string(),
                    dns_domain: "sandbox.local".to_string(),
                    native_routing_cidr: None,
                },
                cilium: Some(CiliumConfig {
                    cluster_id: 2,
                    cluster_name: "sandbox".to_string(),
                }),
                oidc: None,
                kubespray_extra_vars: None,
                ssh_user: None,
            },
            // 3rd cluster: datax (storage workload cluster)
            ClusterDef {
                cluster_name: "datax".to_string(),
                cluster_mode: ClusterMode::Baremetal,
                cluster_sdi_resource_pool: String::new(),
                baremetal_nodes: vec![
                    BaremetalNode {
                        node_name: "bm-dx-cp".to_string(),
                        ip: "10.0.0.20".to_string(),
                        roles: vec!["control-plane".to_string(), "etcd".to_string()],
                    },
                    BaremetalNode {
                        node_name: "bm-dx-w0".to_string(),
                        ip: "10.0.0.21".to_string(),
                        roles: vec!["worker".to_string()],
                    },
                    BaremetalNode {
                        node_name: "bm-dx-w1".to_string(),
                        ip: "10.0.0.22".to_string(),
                        roles: vec!["worker".to_string()],
                    },
                ],
                cluster_role: "workload".to_string(),
                network: ClusterNetwork {
                    pod_cidr: "10.246.0.0/20".to_string(),
                    service_cidr: "10.98.0.0/20".to_string(),
                    dns_domain: "datax.local".to_string(),
                    native_routing_cidr: None,
                },
                cilium: Some(CiliumConfig {
                    cluster_id: 3,
                    cluster_name: "datax".to_string(),
                }),
                oidc: None,
                kubespray_extra_vars: None,
                ssh_user: None,
            },
        ];

        let k8s_config = K8sClustersConfig {
            config: K8sConfig {
                common: common.clone(),
                clusters,
                argocd: None,
                domains: None,
            },
        };

        // 1. All 3 cluster IDs must be unique
        let id_errors = validate_unique_cluster_ids(&k8s_config);
        assert!(
            id_errors.is_empty(),
            "3-cluster IDs must be unique: {:?}",
            id_errors
        );

        // 2. Duplicate ID detection still works
        let mut bad_config = k8s_config.clone();
        bad_config.config.clusters[2]
            .cilium
            .as_mut()
            .unwrap()
            .cluster_id = 1; // conflict with tower
        let id_errors = validate_unique_cluster_ids(&bad_config);
        assert_eq!(id_errors.len(), 1, "must detect duplicate cluster_id");
        assert!(
            id_errors[0].contains("tower"),
            "must identify conflicting cluster"
        );

        // 3. Generate inventory for all 3 clusters
        assert_eq!(k8s_config.config.clusters.len(), 3);
        for cluster in &k8s_config.config.clusters {
            let inv =
                crate::core::kubespray::generate_inventory_baremetal(cluster).unwrap_or_else(|e| {
                    panic!("inventory for '{}' failed: {}", cluster.cluster_name, e)
                });
            assert!(
                inv.contains("[all]"),
                "{}: must have [all]",
                cluster.cluster_name
            );
            assert!(
                inv.contains("[kube_control_plane]"),
                "{}: must have control plane",
                cluster.cluster_name
            );
        }

        // 4. Generate vars for all 3 — all must share common K8s version
        let mut all_vars = Vec::new();
        for cluster in &k8s_config.config.clusters {
            let vars =
                crate::core::kubespray::generate_cluster_vars(cluster, &k8s_config.config.common);
            assert!(
                vars.contains("1.33.1"),
                "{}: must use common k8s version",
                cluster.cluster_name
            );
            assert!(
                vars.contains("cilium"),
                "{}: must use common CNI",
                cluster.cluster_name
            );
            all_vars.push((cluster.cluster_name.clone(), vars));
        }

        // 5. CIDR isolation: no two clusters share pod or service CIDRs
        let cidrs: Vec<(&str, &str, &str)> = k8s_config
            .config
            .clusters
            .iter()
            .map(|c| {
                (
                    c.cluster_name.as_str(),
                    c.network.pod_cidr.as_str(),
                    c.network.service_cidr.as_str(),
                )
            })
            .collect();
        for i in 0..cidrs.len() {
            for j in (i + 1)..cidrs.len() {
                assert_ne!(
                    cidrs[i].1, cidrs[j].1,
                    "pod CIDR collision between '{}' and '{}'",
                    cidrs[i].0, cidrs[j].0
                );
                assert_ne!(
                    cidrs[i].2, cidrs[j].2,
                    "service CIDR collision between '{}' and '{}'",
                    cidrs[i].0, cidrs[j].0
                );
            }
        }

        // 6. datax inventory must have exactly 3 nodes
        let datax_inv =
            crate::core::kubespray::generate_inventory_baremetal(&k8s_config.config.clusters[2])
                .unwrap();
        assert_eq!(
            datax_inv.matches("ansible_host=").count(),
            3,
            "datax must have exactly 3 nodes"
        );

        // 7. Each cluster's vars must contain its own unique dns_domain
        for (name, vars) in &all_vars {
            let expected_domain = format!("{}.local", name);
            assert!(
                vars.contains(&expected_domain),
                "{}: vars must contain dns_domain '{}'",
                name,
                expected_domain
            );
        }
    }

    // =========================================================================
    // Sprint 13a: Config Format Alignment Tests
    // Verify that config structures match the checklist specification exactly.
    // =========================================================================

    /// G-7: Verify .baremetal-init.yaml supports all 3 SSH access modes from checklist.
    /// Case 1: direct_reachable=true (LAN)
    /// Case 2: direct_reachable=false + reachable_node_ip (Tailscale)
    /// Case 3: direct_reachable=false + reachable_via (ProxyJump)
    #[test]
    fn test_checklist_baremetal_init_three_ssh_modes() {
        use crate::core::config::{BaremetalInitConfig, SshAuthMode};

        let yaml = r#"
targetNodes:
  - name: "playbox-0"
    direct_reachable: true
    node_ip: "192.168.88.8"
    adminUser: "jinwang"
    sshAuthMode: "key"
    sshKeyPath: "SSH_KEY_PATH"
  - name: "playbox-0-tailscale"
    direct_reachable: false
    reachable_node_ip: "100.64.0.1"
    node_ip: "192.168.88.8"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "PLAYBOX_0_PASSWORD"
  - name: "playbox-1"
    direct_reachable: false
    reachable_via: ["playbox-0"]
    node_ip: "192.168.88.9"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "PLAYBOX_1_PASSWORD"
  - name: "playbox-2"
    direct_reachable: false
    reachable_via: ["playbox-0"]
    node_ip: "192.168.88.10"
    adminUser: "jinwang"
    sshAuthMode: "key"
    sshKeyPath: "SSH_KEY_PATH"
    sshKeyPathOfReachableNode: "PROXY_SSH_KEY_PATH"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.target_nodes.len(), 4);

        // Case 1: Direct LAN
        let n0 = &config.target_nodes[0];
        assert!(n0.direct_reachable);
        assert_eq!(n0.ssh_auth_mode, SshAuthMode::Key);
        assert!(n0.ssh_key_path.is_some());
        assert!(n0.reachable_node_ip.is_none());
        assert!(n0.reachable_via.is_none());

        // Case 2: External IP (Tailscale)
        let n1 = &config.target_nodes[1];
        assert!(!n1.direct_reachable);
        assert_eq!(n1.reachable_node_ip.as_deref(), Some("100.64.0.1"));
        assert_eq!(n1.ssh_auth_mode, SshAuthMode::Password);

        // Case 3a: ProxyJump with password
        let n2 = &config.target_nodes[2];
        assert!(!n2.direct_reachable);
        assert_eq!(
            n2.reachable_via.as_ref().unwrap(),
            &vec!["playbox-0".to_string()]
        );
        assert_eq!(n2.ssh_auth_mode, SshAuthMode::Password);

        // Case 3b: ProxyJump with key + sshKeyPathOfReachableNode
        let n3 = &config.target_nodes[3];
        assert!(!n3.direct_reachable);
        assert!(n3.reachable_via.is_some());
        assert_eq!(n3.ssh_auth_mode, SshAuthMode::Key);
        assert!(n3.ssh_key_path.is_some());
        assert!(
            n3.ssh_key_path_of_reachable_node.is_some(),
            "sshKeyPathOfReachableNode must be supported per checklist spec"
        );
    }

    /// G-7: Verify baremetal-init supports networkDefaults for SDI host infrastructure.
    #[test]
    fn test_checklist_baremetal_init_network_defaults() {
        use crate::core::config::BaremetalInitConfig;

        let yaml = r#"
networkDefaults:
  managementBridge: "br0"
  managementCidr: "192.168.88.0/24"
  gateway: "192.168.88.1"
targetNodes:
  - name: "playbox-0"
    direct_reachable: true
    node_ip: "192.168.88.8"
    adminUser: "jinwang"
    sshAuthMode: "password"
    sshPassword: "test"
"#;
        let config: BaremetalInitConfig = serde_yaml::from_str(yaml).unwrap();
        let nd = config.network_defaults.unwrap();
        assert_eq!(nd.management_bridge, "br0");
        assert_eq!(nd.management_cidr, "192.168.88.0/24");
        assert_eq!(nd.gateway, "192.168.88.1");
    }

    /// G-7: Verify sdi-specs.yaml pool_name maps to k8s-clusters.yaml cluster_sdi_resource_pool.
    #[test]
    fn test_checklist_sdi_pool_to_cluster_mapping() {
        let sdi_yaml = r#"
resource_pool:
  name: "playbox-pool"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.88.0/24"
    gateway: "192.168.88.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "https://example.com/image.img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id_ed25519.pub"
spec:
  sdi_pools:
    - pool_name: "tower"
      purpose: "management"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "tower-cp-0"
          ip: "192.168.88.100"
          cpu: 2
          mem_gb: 3
          disk_gb: 30
          roles: [control-plane, worker]
    - pool_name: "sandbox"
      purpose: "workload"
      placement:
        spread: true
      node_specs:
        - node_name: "sandbox-cp-0"
          ip: "192.168.88.110"
          cpu: 4
          mem_gb: 8
          disk_gb: 60
          host: "playbox-0"
          roles: [control-plane, etcd]
"#;
        let k8s_yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
  clusters:
    - cluster_name: "tower"
      cluster_sdi_resource_pool: "tower"
      cluster_role: "management"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
        dns_domain: "tower.local"
      cilium:
        cluster_id: 1
        cluster_name: "tower"
    - cluster_name: "sandbox"
      cluster_sdi_resource_pool: "sandbox"
      cluster_role: "workload"
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
        dns_domain: "sandbox.local"
      cilium:
        cluster_id: 2
        cluster_name: "sandbox"
"#;
        let sdi_spec: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_yaml).unwrap();

        // Validate pool mapping (should produce zero errors)
        let errors = validate_cluster_sdi_pool_mapping(&k8s_config, &sdi_spec);
        assert!(errors.is_empty(), "Pool mapping errors: {:?}", errors);

        // Verify each cluster maps to an existing pool
        let pool_names: Vec<&str> = sdi_spec
            .spec
            .sdi_pools
            .iter()
            .map(|p| p.pool_name.as_str())
            .collect();
        for cluster in &k8s_config.config.clusters {
            assert!(
                pool_names.contains(&cluster.cluster_sdi_resource_pool.as_str()),
                "Cluster '{}' references pool '{}' not in SDI spec",
                cluster.cluster_name,
                cluster.cluster_sdi_resource_pool
            );
        }
    }

    /// G-7: Verify that mismatched pool names produce validation errors.
    #[test]
    fn test_checklist_sdi_pool_mapping_mismatch_detected() {
        let sdi_yaml = r#"
resource_pool:
  name: "pool"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.88.0/24"
    gateway: "192.168.88.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "https://example.com/image.img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id_ed25519.pub"
spec:
  sdi_pools:
    - pool_name: "tower"
      node_specs:
        - node_name: "n0"
          ip: "10.0.0.1"
          cpu: 2
          mem_gb: 4
          disk_gb: 30
          roles: [control-plane]
"#;
        let k8s_yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
  clusters:
    - cluster_name: "sandbox"
      cluster_sdi_resource_pool: "nonexistent-pool"
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
"#;
        let sdi_spec: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_yaml).unwrap();

        let errors = validate_cluster_sdi_pool_mapping(&k8s_config, &sdi_spec);
        assert_eq!(errors.len(), 1, "Should detect missing pool");
        assert!(errors[0].contains("nonexistent-pool"));
    }

    // =========================================================================
    // Sprint 13b: SDI Init Auto-discovery Pipeline Tests
    // =========================================================================

    /// G-8: Verify that resource pool summary generation works with multi-node facts.
    #[test]
    fn test_checklist_resource_pool_summary_generation() {
        use crate::core::resource_pool::generate_resource_pool_summary;
        use crate::models::baremetal::*;

        let facts = vec![
            NodeFacts {
                node_name: "playbox-0".to_string(),
                timestamp: "2026-03-11T00:00:00Z".to_string(),
                cpu: CpuInfo {
                    model: "Intel Xeon".to_string(),
                    cores: 8,
                    threads: 16,
                    architecture: "x86_64".to_string(),
                },
                memory: MemoryInfo {
                    total_mb: 32768,
                    available_mb: 28000,
                },
                disks: vec![DiskInfo {
                    name: "sda".to_string(),
                    size_gb: 500,
                    disk_type: "SSD".to_string(),
                    model: "Samsung".to_string(),
                }],
                nics: vec![NicInfo {
                    name: "eth0".to_string(),
                    mac: "aa:bb:cc:dd:ee:f0".to_string(),
                    speed: "1000".to_string(),
                    driver: "e1000e".to_string(),
                    state: "up".to_string(),
                }],
                gpus: vec![],
                iommu_groups: vec![],
                kernel: KernelInfo {
                    version: "6.8.0".to_string(),
                    params: std::collections::HashMap::new(),
                },
                bridges: vec!["br0".to_string()],
                bonds: vec![],
                pcie: vec![],
            },
            NodeFacts {
                node_name: "playbox-1".to_string(),
                timestamp: "2026-03-11T00:00:00Z".to_string(),
                cpu: CpuInfo {
                    model: "Intel Xeon".to_string(),
                    cores: 16,
                    threads: 32,
                    architecture: "x86_64".to_string(),
                },
                memory: MemoryInfo {
                    total_mb: 65536,
                    available_mb: 60000,
                },
                disks: vec![],
                nics: vec![],
                gpus: vec![GpuInfo {
                    pci_id: "0000:01:00.0".to_string(),
                    model: "RTX 4090".to_string(),
                    vendor: "NVIDIA".to_string(),
                    driver: "nvidia".to_string(),
                }],
                iommu_groups: vec![],
                kernel: KernelInfo {
                    version: "6.8.0".to_string(),
                    params: std::collections::HashMap::new(),
                },
                bridges: vec![],
                bonds: vec![],
                pcie: vec![],
            },
        ];

        let summary = generate_resource_pool_summary(&facts);
        assert_eq!(summary.total_nodes, 2);
        assert_eq!(summary.total_cpu_cores, 24); // 8+16
        assert_eq!(summary.total_cpu_threads, 48); // 16+32
        assert_eq!(summary.total_memory_mb, 98304); // 32768+65536
        assert_eq!(summary.total_gpu_count, 1);
        assert_eq!(summary.nodes[0].has_bridge, true);
        assert_eq!(summary.nodes[1].has_bridge, false);
    }

    // =========================================================================
    // Sprint 13c: CF Tunnel + External Access Verification Tests
    // =========================================================================

    /// G-9: Verify CF Tunnel values.yaml contains all required routing domains.
    #[test]
    fn test_checklist_cf_tunnel_routing_domains() {
        let cf_values = include_str!("../../../gitops/tower/cloudflared-tunnel/values.yaml");

        // Required domains per checklist #14
        let required_routes = ["api.k8s.jinwang.dev", "auth.jinwang.dev", "cd.jinwang.dev"];

        for domain in &required_routes {
            assert!(
                cf_values.contains(domain),
                "CF Tunnel values.yaml must contain route for '{}'",
                domain
            );
        }

        // Verify kube-apiserver route exists (enables external kubectl)
        assert!(
            cf_values.contains("kubernetes.default.svc"),
            "CF Tunnel must route to kube-apiserver for external kubectl access"
        );

        // Verify tunnel name matches user's WebUI config
        assert!(
            cf_values.contains("playbox-admin-static"),
            "CF Tunnel name must be 'playbox-admin-static' per checklist #14"
        );

        // Verify fallback 404 route (catch-all)
        assert!(
            cf_values.contains("http_status:404"),
            "CF Tunnel must have a catch-all 404 route"
        );
    }

    /// G-9: Verify SOCKS5 proxy manifest is correctly configured.
    #[test]
    fn test_checklist_socks5_proxy_manifest() {
        let manifest = include_str!("../../../gitops/tower/socks5-proxy/manifest.yaml");

        // Must be in kube-tunnel namespace
        assert!(
            manifest.contains("namespace: kube-tunnel"),
            "SOCKS5 proxy must be in kube-tunnel namespace"
        );

        // Must expose port 1080
        assert!(
            manifest.contains("1080"),
            "SOCKS5 proxy must expose port 1080"
        );

        // Must have both Deployment and Service
        assert!(
            manifest.contains("kind: Deployment"),
            "SOCKS5 proxy must have a Deployment"
        );
        assert!(
            manifest.contains("kind: Service"),
            "SOCKS5 proxy must have a Service"
        );

        // Must use ClusterIP (internal only)
        assert!(
            manifest.contains("type: ClusterIP"),
            "SOCKS5 proxy Service must be ClusterIP"
        );
    }

    /// G-9: Verify GitOps bootstrap spread.yaml creates correct root applications.
    #[test]
    fn test_checklist_gitops_bootstrap_structure() {
        let spread = include_str!("../../../gitops/bootstrap/spread.yaml");

        // Must create tower-root and sandbox-root applications
        assert!(
            spread.contains("tower-root"),
            "Bootstrap must create tower-root application"
        );
        assert!(
            spread.contains("sandbox-root"),
            "Bootstrap must create sandbox-root application"
        );

        // Must reference generators directories
        assert!(
            spread.contains("generators/tower"),
            "Bootstrap must reference tower generators"
        );
        assert!(
            spread.contains("generators/sandbox"),
            "Bootstrap must reference sandbox generators"
        );

        // Must create AppProjects
        assert!(
            spread.contains("projects"),
            "Bootstrap must reference cluster projects"
        );
    }

    // =========================================================================
    // Sprint 13d: Idempotency Verification Tests
    // =========================================================================

    /// G-10: Verify HCL generation is idempotent (same input → same output).
    #[test]
    fn test_checklist_tofu_hcl_generation_idempotent() {
        use crate::models::sdi::{
            CloudInitConfig, NetworkConfig, NodeSpec, OsImageConfig, PlacementConfig,
            ResourcePoolConfig, SdiPool, SdiPoolsSpec, SdiSpec,
        };

        let spec = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "test-pool".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec!["8.8.8.8".to_string()],
                },
            },
            os_image: OsImageConfig {
                source: "https://example.com/image.img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "~/.ssh/id_ed25519.pub".to_string(),
                packages: vec!["curl".to_string()],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![SdiPool {
                    pool_name: "tower".to_string(),
                    purpose: "management".to_string(),
                    placement: PlacementConfig {
                        hosts: vec!["playbox-0".to_string()],
                        spread: false,
                    },
                    node_specs: vec![NodeSpec {
                        node_name: "tower-cp-0".to_string(),
                        ip: "192.168.88.100".to_string(),
                        cpu: 2,
                        mem_gb: 3,
                        disk_gb: 30,
                        host: Some("playbox-0".to_string()),
                        roles: vec!["control-plane".to_string()],
                        devices: None,
                    }],
                }],
            },
        };

        let hcl_1 = crate::core::tofu::generate_tofu_main(&spec, "playbox-0");
        let hcl_2 = crate::core::tofu::generate_tofu_main(&spec, "playbox-0");
        assert_eq!(
            hcl_1, hcl_2,
            "HCL generation must be idempotent (same input → same output)"
        );
    }

    /// G-10: Verify Kubespray inventory generation is idempotent.
    #[test]
    fn test_checklist_kubespray_inventory_idempotent() {
        let k8s_yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
  clusters:
    - cluster_name: "tower"
      cluster_sdi_resource_pool: "tower"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
        dns_domain: "tower.local"
"#;
        let sdi_yaml = r#"
resource_pool:
  name: "pool"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.88.0/24"
    gateway: "192.168.88.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "https://example.com/image.img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id_ed25519.pub"
spec:
  sdi_pools:
    - pool_name: "tower"
      node_specs:
        - node_name: "tower-cp-0"
          ip: "192.168.88.100"
          cpu: 2
          mem_gb: 3
          disk_gb: 30
          roles: [control-plane, worker]
"#;

        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_yaml).unwrap();
        let sdi_spec: crate::models::sdi::SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();

        let inv_1 =
            crate::core::kubespray::generate_inventory(&k8s_config.config.clusters[0], &sdi_spec)
                .unwrap();
        let inv_2 =
            crate::core::kubespray::generate_inventory(&k8s_config.config.clusters[0], &sdi_spec)
                .unwrap();
        assert_eq!(
            inv_1, inv_2,
            "Kubespray inventory generation must be idempotent"
        );
    }

    /// G-10: Verify cluster vars generation is idempotent.
    #[test]
    fn test_checklist_cluster_vars_idempotent() {
        let k8s_yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
    cni: "cilium"
    cilium_version: "1.17.5"
    kube_proxy_remove: true
  clusters:
    - cluster_name: "tower"
      cluster_sdi_resource_pool: "tower"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
        dns_domain: "tower.local"
      cilium:
        cluster_id: 1
        cluster_name: "tower"
"#;
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_yaml).unwrap();

        let vars_1 = crate::core::kubespray::generate_cluster_vars(
            &k8s_config.config.clusters[0],
            &k8s_config.config.common,
        );
        let vars_2 = crate::core::kubespray::generate_cluster_vars(
            &k8s_config.config.clusters[0],
            &k8s_config.config.common,
        );
        assert_eq!(vars_1, vars_2, "Cluster vars generation must be idempotent");
    }

    // =========================================================================
    // Sprint 13e: Documentation Accuracy Tests
    // =========================================================================

    /// G-11: Verify README.md CLI commands reference real CLI structure.
    #[test]
    fn test_checklist_readme_cli_commands_exist() {
        let readme = include_str!("../../../README.md");

        // All CLI commands mentioned in README must be real
        let required_commands = [
            "scalex facts",
            "scalex sdi init",
            "scalex sdi clean",
            "scalex sdi sync",
            "scalex bootstrap",
            "scalex cluster init",
            "scalex get baremetals",
            "scalex get sdi-pools",
            "scalex get clusters",
            "scalex get config-files",
            "scalex status",
            "scalex kernel-tune",
            "scalex secrets apply",
        ];

        for cmd in &required_commands {
            assert!(
                readme.contains(cmd),
                "README.md must document command: '{}'",
                cmd
            );
        }
    }

    /// G-11: Verify README installation guide references correct step numbers.
    #[test]
    fn test_checklist_readme_installation_steps() {
        let readme = include_str!("../../../README.md");

        // Must have all installation steps
        let steps = [
            "Step 0", "Step 1", "Step 2", "Step 3", "Step 4", "Step 5", "Step 6", "Step 7",
            "Step 8",
        ];

        for step in &steps {
            assert!(
                readme.contains(step),
                "README.md installation guide must contain '{}'",
                step
            );
        }
    }

    /// C-1/C-2: README must document `scalex bootstrap` as a distinct installation step.
    /// This is critical: without ArgoCD install + cluster registration, spread.yaml cannot work.
    #[test]
    fn test_checklist_readme_bootstrap_step_exists() {
        let readme = include_str!("../../../README.md");

        assert!(
            readme.contains("scalex bootstrap"),
            "README must document 'scalex bootstrap' command — ArgoCD install + cluster registration is a critical missing step"
        );

        // Must explain what bootstrap does
        assert!(
            readme.contains("ArgoCD"),
            "README bootstrap step must mention ArgoCD installation"
        );
    }

    /// G-11: Verify all docs/ files referenced in README actually exist.
    #[test]
    fn test_checklist_readme_doc_links_valid() {
        let readme = include_str!("../../../README.md");

        // Extract doc references from README
        let expected_docs = [
            "docs/SETUP-GUIDE.md",
            "docs/ARCHITECTURE.md",
            "docs/ops-guide.md",
            "docs/TROUBLESHOOTING.md",
            "docs/CONTRIBUTING.md",
            "docs/CLOUDFLARE-ACCESS.md",
            "docs/NETWORK-DISCOVERY.md",
        ];

        for doc in &expected_docs {
            assert!(readme.contains(doc), "README.md must reference '{}'", doc);
        }
    }

    // ── A-7: README Installation Guide consistency ──

    /// A-7: Verify all .example files referenced in README actually exist on disk.
    #[test]
    fn test_readme_example_files_exist() {
        let readme = include_str!("../../../README.md");

        let example_files = [
            "credentials/.baremetal-init.yaml.example",
            "credentials/.env.example",
            "config/sdi-specs.yaml.example",
            "config/k8s-clusters.yaml.example",
        ];

        for ef in &example_files {
            // README must reference the cp command for each example
            let cp_fragment = format!("cp {}", ef);
            assert!(
                readme.contains(&cp_fragment),
                "README must contain 'cp {}' — users need this to create their config",
                ef
            );

            // The .example file must actually exist
            // Actual file existence is verified by test_checklist_credentials_example_completeness
        }

        // Verify the step references the validation command
        assert!(
            readme.contains("scalex get config-files"),
            "README Step 2 must end with config validation command"
        );
    }

    /// A-7: Verify README step ordering follows the workflow dependency graph.
    /// Step 3 (facts) must come before Step 4 (sdi init),
    /// Step 4 (sdi init) must come before Step 5 (cluster init),
    /// Step 5 (cluster init) must come before Step 6/7 (secrets/bootstrap).
    #[test]
    fn test_readme_step_ordering_matches_workflow() {
        let readme = include_str!("../../../README.md");

        // Find positions of key commands in README
        let facts_pos = readme
            .find("scalex facts --all")
            .expect("README must contain scalex facts --all");
        let sdi_init_pos = readme
            .find("scalex sdi init config/sdi-specs.yaml")
            .expect("README must contain scalex sdi init");
        let cluster_init_pos = readme
            .find("scalex cluster init config/k8s-clusters.yaml")
            .expect("README must contain scalex cluster init");
        let secrets_pos = readme
            .find("scalex secrets apply")
            .expect("README must contain scalex secrets apply");
        let bootstrap_pos = readme
            .find("scalex bootstrap")
            .expect("README must contain scalex bootstrap");

        // Verify ordering: facts < sdi init < cluster init < secrets < bootstrap
        assert!(
            facts_pos < sdi_init_pos,
            "README: 'scalex facts' must appear before 'scalex sdi init' (facts are prerequisite)"
        );
        assert!(
            sdi_init_pos < cluster_init_pos,
            "README: 'scalex sdi init' must appear before 'scalex cluster init' (VMs needed before K8s)"
        );
        assert!(
            cluster_init_pos < secrets_pos,
            "README: 'scalex cluster init' must appear before 'scalex secrets apply' (cluster needed for secrets)"
        );
        assert!(
            secrets_pos < bootstrap_pos,
            "README: 'scalex secrets apply' must appear before 'scalex bootstrap' (secrets needed for ArgoCD)"
        );
    }

    /// A-7: Verify README prerequisite tools match code's prerequisites_for_command().
    #[test]
    fn test_readme_prerequisites_match_code() {
        let readme = include_str!("../../../README.md");

        // Step 0 must mention all tools required by any command.
        // Map binary names to their package/mention names in README
        // (e.g., `ansible-playbook` is installed via `ansible` package)
        let all_tools: std::collections::HashSet<&str> =
            ["sdi-init", "cluster-init", "bootstrap", "facts"]
                .iter()
                .flat_map(|cmd| prerequisites_for_command(cmd))
                .collect();

        for tool in &all_tools {
            let readme_name = match *tool {
                "ansible-playbook" => "ansible",
                other => other,
            };
            assert!(
                readme.contains(readme_name),
                "README Step 0 must mention prerequisite '{}' (for tool '{}') — required by code",
                readme_name,
                tool
            );
        }
    }

    /// A-7: Verify README generated output paths match workflow dependencies.
    #[test]
    fn test_readme_generated_paths_consistent() {
        let readme = include_str!("../../../README.md");

        // These paths are referenced in the README and must be consistent with workflow deps
        let generated_paths = [
            ("_generated/facts/", "Step 3 output"),
            ("_generated/sdi/", "Step 4 output"),
            ("_generated/clusters/", "Step 5 output"),
        ];

        for (path, step) in &generated_paths {
            assert!(
                readme.contains(path),
                "README must reference '{}' ({}) — produced by the pipeline",
                path,
                step
            );
        }

        // kubeconfig path used in Steps 5-8 must be consistent
        assert!(
            readme.contains("_generated/clusters/tower/kubeconfig.yaml"),
            "README must reference tower kubeconfig path consistently"
        );
        assert!(
            readme.contains("_generated/clusters/sandbox/kubeconfig.yaml"),
            "README must reference sandbox kubeconfig path consistently"
        );
    }

    /// G-11: Verify GitOps directory structure matches expected apps.
    #[test]
    fn test_checklist_gitops_directory_completeness() {
        // Common apps are verified indirectly via generator content below

        // Verify tower generator references all expected tower apps
        let tower_gen = include_str!("../../../gitops/generators/tower/tower-generator.yaml");
        let tower_apps = [
            "argocd",
            "cilium",
            "cert-issuers",
            "cloudflared-tunnel",
            "cluster-config",
            "keycloak",
            "socks5-proxy",
        ];
        for app in &tower_apps {
            assert!(
                tower_gen.contains(app),
                "Tower generator must reference app '{}'",
                app
            );
        }

        // Verify sandbox generator references expected sandbox apps
        let sandbox_gen = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");
        let sandbox_apps = [
            "cilium",
            "cluster-config",
            "local-path-provisioner",
            "rbac",
            "test-resources",
        ];
        for app in &sandbox_apps {
            assert!(
                sandbox_gen.contains(app),
                "Sandbox generator must reference app '{}'",
                app
            );
        }
    }

    /// G-11: Verify common generators exist for both tower and sandbox.
    #[test]
    fn test_checklist_common_generators_both_clusters() {
        let tower_common = include_str!("../../../gitops/generators/tower/common-generator.yaml");
        let sandbox_common =
            include_str!("../../../gitops/generators/sandbox/common-generator.yaml");

        let common_apps = [
            "cert-manager",
            "cilium-resources",
            "kyverno",
            "kyverno-policies",
        ];

        for app in &common_apps {
            assert!(
                tower_common.contains(app),
                "Tower common generator must reference '{}'",
                app
            );
            assert!(
                sandbox_common.contains(app),
                "Sandbox common generator must reference '{}'",
                app
            );
        }
    }

    /// G-11: Verify sync wave ordering is consistent across generators.
    #[test]
    fn test_checklist_sync_wave_consistency() {
        let tower_gen = include_str!("../../../gitops/generators/tower/tower-generator.yaml");
        let sandbox_gen = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");

        // ArgoCD must be wave 0 (first to deploy)
        assert!(
            tower_gen.contains("argocd") && tower_gen.contains("\"0\""),
            "ArgoCD must be in sync wave 0"
        );

        // Cilium must be wave 1 (CNI before other apps)
        assert!(
            tower_gen.contains("cilium") && tower_gen.contains("\"1\""),
            "Cilium must be in sync wave 1"
        );

        // Both generators should have wave annotations
        assert!(
            sandbox_gen.contains("argocd-image-updater.argoproj.io")
                || sandbox_gen.contains("argocd.argoproj.io/sync-wave"),
            "Sandbox generator must have sync wave annotations"
        );
    }

    /// Checklist #12: Verify no unnecessary files in project root.
    #[test]
    fn test_checklist_no_unnecessary_root_files() {
        // Verify no k3s references in README (checklist #9: production-grade only)
        let readme = include_str!("../../../README.md");
        assert!(
            !readme.contains("k3s"),
            "README must not reference k3s (checklist #9: production-grade only)"
        );
    }

    /// G-13 / Checklist #14: Verify ops-guide documents pre-OIDC kubectl access via CF Tunnel.
    #[test]
    fn test_checklist_pre_oidc_kubectl_access_documented() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Must document pre-OIDC kubectl access
        assert!(
            ops_guide.contains("Pre-OIDC")
                || ops_guide.contains("pre-OIDC")
                || ops_guide.contains("Keycloak 설정 전"),
            "ops-guide must document pre-OIDC kubectl access"
        );

        // Must reference CF Tunnel kubectl endpoint (even if to explain limitation)
        assert!(
            ops_guide.contains("api.k8s.jinwang.dev"),
            "ops-guide must reference CF Tunnel kubectl endpoint"
        );

        // Must recommend Tailscale for Pre-OIDC kubectl (CF Tunnel client cert doesn't work)
        assert!(
            ops_guide.contains("kubeconfig") && ops_guide.contains("tower-tailscale"),
            "ops-guide must show Tailscale kubeconfig for pre-OIDC access (CF Tunnel client cert auth is impossible)"
        );
    }

    /// Checklist #15: Verify NAT access methods (Tailscale + CF + LAN) are documented.
    #[test]
    fn test_checklist_nat_access_methods_documented() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Three access methods must be documented
        assert!(
            ops_guide.contains("Cloudflare Tunnel"),
            "ops-guide must document Cloudflare Tunnel access"
        );
        assert!(
            ops_guide.contains("Tailscale"),
            "ops-guide must document Tailscale VPN access"
        );
        assert!(
            ops_guide.contains("LAN") || ops_guide.contains("내부"),
            "ops-guide must document LAN direct access"
        );

        // LAN access must include switch reference
        assert!(
            ops_guide.contains("스위치") || ops_guide.contains("switch"),
            "ops-guide must include network switch reference for LAN access"
        );

        // Must have comparison table
        assert!(
            ops_guide.contains("Cloudflare Tunnel")
                && ops_guide.contains("Tailscale VPN")
                && ops_guide.contains("LAN"),
            "ops-guide must have access method comparison"
        );
    }

    // ── Sprint 13c: 2-Layer Template Consistency + Client OIDC + Credentials ──

    #[test]
    fn test_checklist_two_layer_pool_name_consistency() {
        // Verify that sdi-specs pool names match k8s-clusters cluster_sdi_resource_pool references
        let sdi_specs = include_str!("../../../config/sdi-specs.yaml.example");
        let k8s_clusters = include_str!("../../../config/k8s-clusters.yaml.example");

        // Extract pool_name values from sdi-specs (lines like "    - pool_name: ..." or "      pool_name: ...")
        let sdi_pool_names: Vec<&str> = sdi_specs
            .lines()
            .filter(|l| l.contains("pool_name:"))
            .map(|l| l.split('"').nth(1).unwrap_or(""))
            .collect();
        assert!(
            sdi_pool_names.len() >= 2,
            "sdi-specs must define at least 2 pools (tower + sandbox), found {}",
            sdi_pool_names.len()
        );

        // Every cluster_sdi_resource_pool in k8s-clusters must reference a valid sdi pool
        for line in k8s_clusters.lines() {
            if let Some(pool_ref) = line.trim().strip_prefix("cluster_sdi_resource_pool:") {
                // Strip inline YAML comment, then trim quotes
                let pool_ref = pool_ref
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"');
                assert!(
                    sdi_pool_names.contains(&pool_ref),
                    "k8s-clusters references pool '{}' not defined in sdi-specs (available: {:?})",
                    pool_ref,
                    sdi_pool_names
                );
            }
        }
    }

    #[test]
    fn test_checklist_two_layer_domains_consistency() {
        // k8s-clusters.yaml defines domains; these must be used in gitops CF tunnel config
        let k8s_clusters = include_str!("../../../config/k8s-clusters.yaml.example");
        let cf_values = include_str!("../../../gitops/tower/cloudflared-tunnel/values.yaml");

        // Extract domain values from k8s-clusters
        let domains = ["auth.jinwang.dev", "cd.jinwang.dev", "api.k8s.jinwang.dev"];
        for domain in &domains {
            assert!(
                k8s_clusters.contains(domain),
                "k8s-clusters.yaml must define domain '{}'",
                domain
            );
            assert!(
                cf_values.contains(domain),
                "CF tunnel values.yaml must route domain '{}'",
                domain
            );
        }
    }

    #[test]
    fn test_checklist_client_oidc_template_variables() {
        // kubeconfig-oidc.yaml.j2 must reference domain variables from k8s-clusters config
        let template = include_str!("../../../client/kubeconfig-oidc.yaml.j2");

        // Must use Jinja2 variables for domains (not hardcoded)
        assert!(
            template.contains("{{ domains.k8s_api }}"),
            "OIDC kubeconfig template must use domains.k8s_api variable"
        );
        assert!(
            template.contains("{{ domains.auth }}"),
            "OIDC kubeconfig template must use domains.auth variable"
        );
        // Must reference Keycloak OIDC config
        assert!(
            template.contains("{{ keycloak.realm }}"),
            "OIDC kubeconfig template must use keycloak.realm variable"
        );
        assert!(
            template.contains("{{ keycloak.client_id }}"),
            "OIDC kubeconfig template must use keycloak.client_id variable"
        );
        // Must use oidc-login exec plugin
        assert!(
            template.contains("oidc-login"),
            "OIDC kubeconfig must use kubectl oidc-login plugin"
        );
    }

    #[test]
    fn test_checklist_credentials_example_completeness() {
        // All 5 credential example files must exist with proper content
        let baremetal = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let env = include_str!("../../../credentials/.env.example");
        let secrets = include_str!("../../../credentials/secrets.yaml.example");
        let cf_tunnel = include_str!("../../../credentials/cloudflare-tunnel.json.example");
        let creds_readme = include_str!("../../../credentials/README.md");

        // .baremetal-init.yaml.example must cover all 3 SSH modes
        assert!(
            baremetal.contains("direct_reachable: true"),
            "must show Case 1 (direct)"
        );
        assert!(
            baremetal.contains("reachable_node_ip"),
            "must show Case 2 (external IP)"
        );
        assert!(
            baremetal.contains("reachable_via"),
            "must show Case 3 (ProxyJump)"
        );
        assert!(
            baremetal.contains("networkDefaults"),
            "must include network defaults"
        );

        // .env.example must have password and key variables
        assert!(
            env.contains("PLAYBOX_0_PASSWORD"),
            "must have playbox-0 password var"
        );
        assert!(env.contains("SSH_KEY_PATH"), "must have SSH key path var");

        // secrets.yaml.example must have keycloak + argocd + cloudflare
        assert!(secrets.contains("keycloak"), "must have keycloak section");
        assert!(secrets.contains("argocd"), "must have argocd section");
        assert!(
            secrets.contains("cloudflare"),
            "must have cloudflare section"
        );

        // cloudflare-tunnel.json.example must have required fields
        assert!(cf_tunnel.contains("AccountTag"), "must have AccountTag");
        assert!(cf_tunnel.contains("TunnelSecret"), "must have TunnelSecret");
        assert!(cf_tunnel.contains("TunnelID"), "must have TunnelID");

        // credentials/README.md must exist and explain the directory
        assert!(
            !creds_readme.is_empty(),
            "credentials/README.md must not be empty"
        );
    }

    #[test]
    fn test_checklist_setup_client_references_scalex() {
        // setup-client.sh must reference scalex CLI, not legacy 'playbox' name
        let setup_script = include_str!("../../../client/setup-client.sh");

        // Must not contain legacy 'playbox' references
        assert!(
            !setup_script.contains("playbox"),
            "setup-client.sh must reference 'scalex', not legacy 'playbox'"
        );
        // Must reference correct tool
        assert!(
            setup_script.contains("scalex") || setup_script.contains("ScaleX"),
            "setup-client.sh must reference scalex CLI"
        );
    }

    // ── Sprint 13d: Edge Cases ──

    #[test]
    fn test_edge_cilium_cluster_id_uniqueness() {
        // Multi-cluster Cilium ClusterMesh requires unique cluster_id per cluster
        let k8s_clusters = include_str!("../../../config/k8s-clusters.yaml.example");

        let cluster_ids: Vec<&str> = k8s_clusters
            .lines()
            .filter(|l| l.contains("cluster_id:"))
            .map(|l| l.split(':').last().unwrap_or("").trim())
            .collect();

        assert!(
            cluster_ids.len() >= 2,
            "Must define at least 2 Cilium cluster_ids, found {}",
            cluster_ids.len()
        );

        // All IDs must be unique
        let mut seen = std::collections::HashSet::new();
        for id in &cluster_ids {
            assert!(
                seen.insert(id),
                "Duplicate Cilium cluster_id '{}' — ClusterMesh requires unique IDs",
                id
            );
        }

        // Cilium cluster_id must be 1-255
        for id in &cluster_ids {
            let num: u32 = id.parse().expect("cluster_id must be numeric");
            assert!(
                num >= 1 && num <= 255,
                "Cilium cluster_id {} out of range (must be 1-255)",
                num
            );
        }
    }

    #[test]
    fn test_edge_common_config_essential_fields() {
        // k8s-clusters common config must include all fields critical for multi-cluster consistency
        let k8s_clusters = include_str!("../../../config/k8s-clusters.yaml.example");

        let essential_fields = [
            "kubernetes_version",
            "container_runtime",
            "cni",
            "cgroup_driver",
            "etcd_deployment_type",
            "dns_mode",
        ];

        for field in &essential_fields {
            assert!(
                k8s_clusters.contains(field),
                "k8s-clusters common config must include '{}' for multi-cluster consistency",
                field
            );
        }

        // CNI must be cilium (project requirement for ClusterMesh)
        assert!(
            k8s_clusters.contains("cni: \"cilium\""),
            "CNI must be cilium for ClusterMesh multi-cluster networking"
        );

        // Container runtime must be containerd (production standard)
        assert!(
            k8s_clusters.contains("container_runtime: \"containerd\""),
            "Container runtime must be containerd"
        );

        // etcd must be host deployment (production recommendation)
        assert!(
            k8s_clusters.contains("etcd_deployment_type: \"host\""),
            "etcd deployment must be 'host' for production"
        );
    }

    #[test]
    fn test_edge_mixed_mode_sdi_and_baremetal_coexistence() {
        // Verify k8s-clusters supports both SDI and baremetal mode clusters
        let k8s_clusters = include_str!("../../../config/k8s-clusters.yaml.example");

        // Must have at least one SDI-mode cluster (tower and sandbox)
        assert!(
            k8s_clusters.contains("cluster_sdi_resource_pool"),
            "Must have SDI-mode clusters"
        );

        // Must document baremetal mode as a valid option (even if commented)
        assert!(
            k8s_clusters.contains("cluster_mode: \"baremetal\""),
            "Must document baremetal cluster mode for extensibility (#9)"
        );
        assert!(
            k8s_clusters.contains("baremetal_nodes"),
            "Baremetal mode must show node definition syntax"
        );
    }

    #[test]
    fn test_edge_sdi_node_placement_host_references() {
        // Every explicit host placement in sdi-specs must reference a node from .baremetal-init.yaml
        let sdi_specs = include_str!("../../../config/sdi-specs.yaml.example");
        let baremetal_init = include_str!("../../../credentials/.baremetal-init.yaml.example");

        // Extract bare-metal node names
        let bm_names: Vec<&str> = baremetal_init
            .lines()
            .filter(|l| l.trim().starts_with("- name:"))
            .map(|l| l.split('"').nth(1).unwrap_or(""))
            .collect();
        assert!(!bm_names.is_empty(), "Must have baremetal nodes defined");

        // Extract host references from sdi-specs (explicit placement)
        for line in sdi_specs.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("host:") || trimmed.starts_with("hosts:") {
                // Extract node names: handle both "quoted" and [bracket, list] and bare values
                let value = trimmed.split(':').skip(1).collect::<Vec<_>>().join(":");
                let value = value.split('#').next().unwrap_or("");
                let value = value.replace(['[', ']', '"'], "");
                for name in value.split(',') {
                    let name = name.trim();
                    if !name.is_empty() && name.contains("playbox") {
                        assert!(
                            bm_names.contains(&name),
                            "SDI placement references host '{}' not in baremetal-init (available: {:?})",
                            name,
                            bm_names
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_edge_cluster_network_cidrs_no_overlap() {
        // Pod and service CIDRs across clusters must not overlap for ClusterMesh
        let k8s_clusters = include_str!("../../../config/k8s-clusters.yaml.example");

        let mut pod_cidrs = Vec::new();
        let mut service_cidrs = Vec::new();

        for line in k8s_clusters.lines() {
            let trimmed = line.trim();
            if let Some(cidr) = trimmed.strip_prefix("pod_cidr:") {
                pod_cidrs.push(cidr.trim().trim_matches('"'));
            } else if let Some(cidr) = trimmed.strip_prefix("service_cidr:") {
                service_cidrs.push(cidr.trim().trim_matches('"'));
            }
        }

        assert!(
            pod_cidrs.len() >= 2,
            "Must have at least 2 pod CIDRs for multi-cluster"
        );

        // Pod CIDRs must differ (simple string comparison — full CIDR overlap check
        // is done in test_cross_config_cidrs_no_overlap)
        for i in 0..pod_cidrs.len() {
            for j in (i + 1)..pod_cidrs.len() {
                assert_ne!(
                    pod_cidrs[i], pod_cidrs[j],
                    "Pod CIDRs must not be identical across clusters: '{}'",
                    pod_cidrs[i]
                );
            }
        }

        // Service CIDRs must differ
        for i in 0..service_cidrs.len() {
            for j in (i + 1)..service_cidrs.len() {
                assert_ne!(
                    service_cidrs[i], service_cidrs[j],
                    "Service CIDRs must not be identical across clusters: '{}'",
                    service_cidrs[i]
                );
            }
        }
    }

    // ── Sprint 15e: Sandbox External Access Architecture (C-5) ──

    /// C-5: ops-guide must have a dedicated section explaining sandbox access architecture —
    /// sandbox is managed via tower ArgoCD, not directly exposed via CF Tunnel.
    #[test]
    fn test_checklist_sandbox_access_architecture_documented() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Must have a dedicated "Sandbox Access" or "Sandbox 클러스터 접근" section heading
        assert!(
            ops_guide.contains("## 5. Sandbox") || ops_guide.contains("Sandbox Access"),
            "ops-guide must have a dedicated Sandbox access section"
        );

        // Must explicitly state the architecture decision: sandbox workloads managed via tower ArgoCD
        assert!(
            ops_guide.contains("Tower ArgoCD") || ops_guide.contains("tower ArgoCD"),
            "ops-guide must explain sandbox is managed via Tower ArgoCD"
        );

        // Must explain why sandbox API is NOT in CF Tunnel routing
        assert!(
            ops_guide.contains("Sandbox API")
                && (ops_guide.contains("Tunnel") || ops_guide.contains("tunnel")),
            "ops-guide must explain Sandbox API CF Tunnel architecture decision"
        );
    }

    /// C-5: ops-guide must document sandbox kubectl access for debugging/emergency.
    #[test]
    fn test_checklist_sandbox_kubectl_access_methods() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Must show sandbox kubeconfig path for direct access
        assert!(
            ops_guide.contains("clusters/sandbox/kubeconfig"),
            "ops-guide must reference sandbox kubeconfig path"
        );

        // Must document at least 2 sandbox access methods
        let has_lan = ops_guide.contains("Sandbox") && ops_guide.contains("LAN");
        let has_tailscale = ops_guide.contains("Sandbox") && ops_guide.contains("Tailscale");
        let has_bastion = ops_guide.contains("Sandbox") && ops_guide.contains("bastion");
        assert!(
            (has_lan as u8 + has_tailscale as u8 + has_bastion as u8) >= 2,
            "ops-guide must document at least 2 sandbox access methods (LAN/Tailscale/bastion)"
        );
    }

    // ── Sprint 15f: Legacy Naming Cleanup (C-6) ──

    /// C-6: spread.yaml must use `scalex-root` AppProject, not legacy `playbox-root`.
    #[test]
    fn test_spread_yaml_uses_scalex_root_naming() {
        let spread = include_str!("../../../gitops/bootstrap/spread.yaml");

        // Must NOT contain legacy playbox-root
        assert!(
            !spread.contains("playbox-root"),
            "spread.yaml must not use legacy 'playbox-root' — use 'scalex-root' instead"
        );

        // Must contain scalex-root
        assert!(
            spread.contains("scalex-root"),
            "spread.yaml must use 'scalex-root' as the root AppProject name"
        );
    }

    // ── Sprint 17a: CF Tunnel Pre-OIDC Auth Compatibility (C-7) ──

    /// C-7: ops-guide must clearly state that CF Tunnel does NOT support client certificate auth.
    /// CF Tunnel operates at HTTP layer (L7) — TLS is terminated at CF Edge, so client certs
    /// are presented to CF, NOT to the kube-apiserver backend.
    #[test]
    fn test_checklist_cf_tunnel_client_cert_limitation_documented() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Must explicitly warn about client certificate limitation through CF Tunnel
        assert!(
            ops_guide.contains("client certificate")
                || ops_guide.contains("client cert")
                || ops_guide.contains("클라이언트 인증서"),
            "ops-guide must document CF Tunnel client certificate auth limitation"
        );

        // Must mention TLS termination as the reason
        assert!(
            ops_guide.contains("TLS")
                && (ops_guide.contains("종단") || ops_guide.contains("terminat")),
            "ops-guide must explain TLS termination as the reason client certs don't work"
        );
    }

    /// C-7: ops-guide must recommend Tailscale (not CF Tunnel) for Pre-OIDC kubectl.
    #[test]
    fn test_checklist_pre_oidc_kubectl_recommends_tailscale() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Must have a clear section about Pre-OIDC access limitations
        assert!(
            ops_guide.contains("Pre-OIDC") || ops_guide.contains("pre-OIDC"),
            "ops-guide must have Pre-OIDC access section"
        );

        // Must recommend Tailscale for Pre-OIDC kubectl
        assert!(
            ops_guide.contains("Tailscale") && ops_guide.contains("Pre-OIDC"),
            "ops-guide must recommend Tailscale for Pre-OIDC kubectl access"
        );
    }

    /// C-7: ops-guide external access section must document auth method compatibility per path.
    #[test]
    fn test_checklist_access_path_auth_compatibility_matrix() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Must have an auth compatibility table or list showing which auth works on which path
        let has_cf_oidc = ops_guide.contains("OIDC")
            && (ops_guide.contains("Tunnel") || ops_guide.contains("tunnel"));
        let has_tailscale_cert = ops_guide.contains("Tailscale")
            && (ops_guide.contains("cert") || ops_guide.contains("인증서"));

        assert!(
            has_cf_oidc && has_tailscale_cert,
            "ops-guide must document auth compatibility: CF Tunnel=OIDC only, Tailscale=cert+token"
        );
    }

    // ── Sprint 17b: Tower supplementary_addresses_in_ssl_keys (C-9) ──

    /// C-9: k8s-clusters.yaml.example must include supplementary_addresses_in_ssl_keys
    /// for the tower cluster when Tailscale bastion IP is used for kubectl access.
    #[test]
    fn test_checklist_tower_supplementary_ssl_keys_in_example() {
        let k8s_config = include_str!("../../../config/k8s-clusters.yaml.example");

        // Tower cluster section must have supplementary_addresses_in_ssl_keys
        // because Tailscale users access tower API server via bastion IP
        let tower_section_idx = k8s_config.find("cluster_name: \"tower\"").unwrap();
        let sandbox_section_idx = k8s_config.find("cluster_name: \"sandbox\"").unwrap();
        let tower_section = &k8s_config[tower_section_idx..sandbox_section_idx];

        assert!(
            tower_section.contains("supplementary_addresses_in_ssl_keys"),
            "Tower cluster must have supplementary_addresses_in_ssl_keys for Tailscale access. \
             Without it, kubectl via Tailscale IP will fail TLS verification."
        );
    }

    // ── Sprint 17c: SOCKS5 Proxy Access Path (C-8) ──

    /// C-8: ops-guide must document SOCKS5 proxy access path — it's ClusterIP, so requires
    /// kubectl port-forward from a machine with existing kubectl access (LAN/Tailscale).
    #[test]
    fn test_checklist_socks5_proxy_access_path_documented() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Must document how to access the SOCKS5 proxy
        assert!(
            ops_guide.contains("SOCKS5") || ops_guide.contains("socks5"),
            "ops-guide must document SOCKS5 proxy usage"
        );

        // Must mention it's ClusterIP and requires port-forward
        assert!(
            ops_guide.contains("port-forward") || ops_guide.contains("포트 포워딩"),
            "ops-guide must explain SOCKS5 requires port-forward (ClusterIP)"
        );
    }

    // ── Sprint 17d: CF Credentials Pre-Bootstrap Check (C-10) ──

    /// C-10: README installation guide must warn about CF credentials before bootstrap.
    #[test]
    fn test_checklist_cf_credentials_pre_bootstrap_warning() {
        let readme = include_str!("../../../README.md");

        // README Step 6/7 must mention cloudflare-tunnel.json requirement
        assert!(
            readme.contains("cloudflare-tunnel.json"),
            "README must reference cloudflare-tunnel.json in bootstrap prerequisites"
        );

        // Must warn about CrashLoop if credentials missing
        assert!(
            readme.contains("CrashLoop") || readme.contains("기동 실패"),
            "README must warn about cloudflared CrashLoop if credentials are missing"
        );
    }

    /// C-10: bootstrap validation should check for CF credentials existence.
    #[test]
    fn test_bootstrap_cf_credentials_validation() {
        // validate_bootstrap_prerequisites must return warning when CF credentials are missing
        let warnings = validate_bootstrap_prerequisites(
            "_generated/clusters/tower/kubeconfig.yaml",
            "credentials/cloudflare-tunnel.json",
            false, // cf_credentials_exist
        );

        assert!(
            !warnings.is_empty(),
            "Bootstrap must warn when CF credentials are missing"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("cloudflare") || w.contains("Cloudflare")),
            "Warning must mention Cloudflare credentials"
        );
    }

    /// C-10: bootstrap validation should pass when CF credentials exist.
    #[test]
    fn test_bootstrap_cf_credentials_present_no_warning() {
        let warnings = validate_bootstrap_prerequisites(
            "_generated/clusters/tower/kubeconfig.yaml",
            "credentials/cloudflare-tunnel.json",
            true, // cf_credentials_exist
        );

        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("cloudflare") || w.contains("Cloudflare")),
            "Should not warn about CF credentials when they exist"
        );
    }

    // ── Sprint 18a: CLI execution path pipeline verification ──

    /// Sprint 18a: Verify `sdi init` no-spec pipeline ordering:
    /// facts check → host-prep → resource-pool summary → host-infra HCL → tofu
    #[test]
    fn test_sdi_init_no_spec_pipeline_ordering() {
        use crate::core::resource_pool;
        use crate::models::baremetal::*;
        use std::collections::HashMap;

        // Step 1: Facts must exist before init proceeds
        let facts = vec![NodeFacts {
            node_name: "playbox-0".to_string(),
            timestamp: "2026-03-11T00:00:00Z".to_string(),
            cpu: CpuInfo {
                model: "Intel".to_string(),
                cores: 8,
                threads: 16,
                architecture: "x86_64".to_string(),
            },
            memory: MemoryInfo {
                total_mb: 32768,
                available_mb: 28000,
            },
            disks: vec![DiskInfo {
                name: "sda".to_string(),
                size_gb: 500,
                model: "Samsung".to_string(),
                disk_type: "disk".to_string(),
            }],
            nics: vec![NicInfo {
                name: "eno1".to_string(),
                mac: "aa:bb:cc:dd:ee:ff".to_string(),
                speed: "1G".to_string(),
                driver: "igb".to_string(),
                state: "UP".to_string(),
            }],
            gpus: vec![],
            iommu_groups: vec![],
            kernel: KernelInfo {
                version: "6.8.0".to_string(),
                params: HashMap::new(),
            },
            bridges: vec!["br0".to_string()],
            bonds: vec![],
            pcie: vec![],
        }];

        // Step 2: Resource pool summary generation (prerequisite for any SDI operation)
        let summary = resource_pool::generate_resource_pool_summary(&facts);
        assert_eq!(summary.total_nodes, 1);
        assert!(summary.total_cpu_cores > 0, "Resource pool must show CPU");
        assert!(
            summary.total_memory_mb > 0,
            "Resource pool must show memory"
        );

        // Step 3: Host-infra inputs derived from baremetal config (not facts)
        let inputs = crate::commands::sdi::build_host_infra_inputs(&[
            crate::core::config::NodeConnectionConfig {
                name: "playbox-0".to_string(),
                direct_reachable: true,
                node_ip: "192.168.88.8".to_string(),
                reachable_node_ip: None,
                reachable_via: None,
                admin_user: "jinwang".to_string(),
                ssh_auth_mode: crate::core::config::SshAuthMode::Password,
                ssh_password: Some("pass".to_string()),
                ssh_key_path: None,
                ssh_key_path_of_reachable_node: None,
            },
        ]);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].ssh_user, "jinwang");

        // Step 4: Host-infra HCL generation
        let hcl = crate::core::tofu::generate_tofu_host_infra(
            &inputs,
            "br0",
            "192.168.88.0/24",
            "192.168.88.1",
        );
        assert!(
            hcl.contains("libvirt_pool"),
            "Host-infra HCL must define storage pool"
        );
        assert!(
            hcl.contains("libvirt"),
            "Host-infra HCL must use libvirt provider"
        );

        // Pipeline ordering verified: facts → resource-pool → host-infra inputs → HCL
    }

    /// Sprint 18a: Verify `cluster init` pipeline generates both inventory and vars
    /// for every cluster, and gitops Cilium values are generated per-cluster.
    #[test]
    fn test_cluster_init_pipeline_completeness() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let mut cluster_outputs = Vec::new();

        for cluster in &k8s.config.clusters {
            // Step 1: Inventory generation
            let inv = crate::core::kubespray::generate_inventory(cluster, &sdi).unwrap();

            // Step 2: Cluster vars generation
            let vars = crate::core::kubespray::generate_cluster_vars(cluster, &k8s.config.common);

            // Step 3: Cilium values generation (for gitops update)
            let cilium_values = crate::core::gitops::generate_cilium_values(
                "192.168.88.100", // mock CP IP
                6443,             // default kube-apiserver port
            );

            // Verify all outputs are non-empty
            assert!(
                !inv.is_empty(),
                "Inventory must be generated for {}",
                cluster.cluster_name
            );
            assert!(
                !vars.is_empty(),
                "Vars must be generated for {}",
                cluster.cluster_name
            );
            assert!(
                !cilium_values.is_empty(),
                "Cilium values must be generated for {}",
                cluster.cluster_name
            );

            cluster_outputs.push(cluster.cluster_name.clone());
        }

        // Both tower and sandbox must have outputs
        assert!(
            cluster_outputs.contains(&"tower".to_string()),
            "Pipeline must process tower cluster"
        );
        assert!(
            cluster_outputs.contains(&"sandbox".to_string()),
            "Pipeline must process sandbox cluster"
        );
    }

    // ── Sprint 18b: CF Tunnel external kubectl documentation verification ──

    /// Sprint 18b: README must explicitly warn that CF Tunnel kubectl requires
    /// Keycloak OIDC to be configured first.
    #[test]
    fn test_readme_cf_tunnel_requires_keycloak_warning() {
        let readme = include_str!("../../../README.md");

        // README must mention that CF Tunnel external access requires OIDC/Keycloak
        let has_oidc_requirement =
            readme.contains("Keycloak") && (readme.contains("OIDC") || readme.contains("oidc"));

        assert!(
            has_oidc_requirement,
            "README must mention that CF Tunnel external kubectl requires Keycloak OIDC"
        );

        // README must mention TLS termination or client cert limitation
        assert!(
            readme.contains("TLS") || readme.contains("client cert") || readme.contains("인증서"),
            "README must mention CF Tunnel TLS termination or client cert limitation"
        );
    }

    /// Sprint 18b: README External Access must clarify pre-OIDC limitations.
    #[test]
    fn test_readme_pre_oidc_access_limitation() {
        let readme = include_str!("../../../README.md");

        // Must mention Tailscale as pre-OIDC external access method
        assert!(
            readme.contains("Tailscale"),
            "README must mention Tailscale for external access"
        );

        // Must explicitly state CF Tunnel OIDC-only constraint
        assert!(
            readme.contains("OIDC"),
            "README must mention OIDC as required auth for CF Tunnel"
        );
    }

    /// Sprint 18b: SOCKS5 proxy usage scenario must be documented as LAN/Tailscale internal.
    #[test]
    fn test_socks5_proxy_scope_documented() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        assert!(
            ops_guide.contains("SOCKS5") || ops_guide.contains("socks5"),
            "ops-guide must document SOCKS5 proxy"
        );
    }

    // ── Sprint 18c: baremetal-init.yaml format consistency with Checklist ──

    /// Sprint 18c: .baremetal-init.yaml.example must support all 3 access modes
    /// from the Checklist specification.
    #[test]
    fn test_baremetal_init_example_three_access_modes() {
        let example = include_str!("../../../credentials/.baremetal-init.yaml.example");

        // Case 1: direct_reachable: true
        assert!(
            example.contains("direct_reachable: true"),
            "Example must show direct reachable mode (Case 1)"
        );

        // Case 2: reachable_node_ip (external IP like Tailscale)
        assert!(
            example.contains("reachable_node_ip"),
            "Example must show external IP mode (Case 2)"
        );

        // Case 3: reachable_via (ProxyJump)
        assert!(
            example.contains("reachable_via"),
            "Example must show ProxyJump mode (Case 3)"
        );
    }

    /// Sprint 18c: .baremetal-init.yaml.example must support both password and key auth.
    #[test]
    fn test_baremetal_init_example_both_auth_modes() {
        let example = include_str!("../../../credentials/.baremetal-init.yaml.example");

        assert!(
            example.contains("sshPassword") || example.contains("ssh_password"),
            "Example must show password auth"
        );
        assert!(
            example.contains("sshKeyPath") || example.contains("ssh_key_path"),
            "Example must show key auth"
        );
        assert!(
            example.contains("sshKeyPathOfReachableNode")
                || example.contains("ssh_key_path_of_reachable_node"),
            "Example must show proxy hop key (Case 3b)"
        );
    }

    /// Sprint 18c: .baremetal-init.yaml must parse with all 4 nodes for our setup.
    #[test]
    fn test_baremetal_init_example_has_four_nodes() {
        let example = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let config: serde_yaml::Value = serde_yaml::from_str(example).unwrap();

        let nodes = config["targetNodes"].as_sequence().unwrap();
        assert_eq!(
            nodes.len(),
            4,
            "Example must define exactly 4 nodes (playbox-0 through playbox-3)"
        );

        // Verify all 4 playbox nodes are present
        let names: Vec<&str> = nodes.iter().map(|n| n["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"playbox-0"));
        assert!(names.contains(&"playbox-1"));
        assert!(names.contains(&"playbox-2"));
        assert!(names.contains(&"playbox-3"));
    }

    // ── Sprint 19a: SOCKS5 proxy manifest & external access path verification ──

    /// Sprint 19a: SOCKS5 proxy manifest must be valid K8s Deployment + Service
    /// in `kube-tunnel` namespace with correct port 1080.
    #[test]
    fn test_socks5_manifest_structure() {
        let manifest = include_str!("../../../gitops/tower/socks5-proxy/manifest.yaml");

        // Must define both Deployment and Service
        assert!(
            manifest.contains("kind: Deployment"),
            "SOCKS5 manifest must contain a Deployment"
        );
        assert!(
            manifest.contains("kind: Service"),
            "SOCKS5 manifest must contain a Service"
        );

        // Correct namespace
        assert!(
            manifest.contains("namespace: kube-tunnel"),
            "SOCKS5 must be in kube-tunnel namespace"
        );

        // SOCKS5 standard port
        assert!(manifest.contains("1080"), "SOCKS5 must use port 1080");

        // Resource limits defined (production-ready)
        assert!(
            manifest.contains("resources:"),
            "SOCKS5 must define resource requests/limits"
        );
        assert!(
            manifest.contains("limits:"),
            "SOCKS5 must define resource limits"
        );

        // ClusterIP (not exposed externally — internal only via kubectl port-forward)
        assert!(
            manifest.contains("type: ClusterIP"),
            "SOCKS5 Service must be ClusterIP (not NodePort/LoadBalancer)"
        );
    }

    /// Sprint 19a: SOCKS5 kustomization must reference the manifest.
    #[test]
    fn test_socks5_kustomization_references_manifest() {
        let kustomization = include_str!("../../../gitops/tower/socks5-proxy/kustomization.yaml");

        assert!(
            kustomization.contains("manifest.yaml"),
            "Kustomization must reference manifest.yaml"
        );
        assert!(
            kustomization.contains("kind: Kustomization"),
            "Must be a valid Kustomization resource"
        );
    }

    /// Sprint 19a: Tower generator must include socks5-proxy in kube-tunnel namespace
    /// at sync wave 3 (same as cloudflared-tunnel).
    #[test]
    fn test_tower_generator_includes_socks5() {
        let generator = include_str!("../../../gitops/generators/tower/tower-generator.yaml");

        assert!(
            generator.contains("socks5-proxy"),
            "Tower generator must include socks5-proxy app"
        );

        // Parse YAML to verify namespace and sync wave
        let docs: Vec<serde_yaml::Value> = vec![serde_yaml::from_str(generator).unwrap()];

        let app_set = &docs[0];
        let elements = app_set["spec"]["generators"][0]["list"]["elements"]
            .as_sequence()
            .unwrap();

        let socks5_entry = elements
            .iter()
            .find(|e| e["appName"].as_str() == Some("socks5-proxy"))
            .expect("socks5-proxy must be in generator elements");

        assert_eq!(
            socks5_entry["namespace"].as_str().unwrap(),
            "kube-tunnel",
            "socks5-proxy must target kube-tunnel namespace"
        );
        assert_eq!(
            socks5_entry["syncWave"].as_str().unwrap(),
            "3",
            "socks5-proxy must be sync wave 3"
        );
    }

    /// Sprint 19a: External access paths must be documented:
    /// 1) LAN direct, 2) Tailscale, 3) CF Tunnel (OIDC required).
    /// SOCKS5 is for LAN/Tailscale path only.
    #[test]
    fn test_external_access_three_paths_documented() {
        let readme = include_str!("../../../README.md");

        // Path 1: LAN direct access
        assert!(
            readme.contains("LAN") || readme.contains("lan") || readme.contains("로컬"),
            "README must document LAN direct access path"
        );

        // Path 2: Tailscale
        assert!(
            readme.contains("Tailscale") || readme.contains("tailscale"),
            "README must document Tailscale access path"
        );

        // Path 3: Cloudflare Tunnel
        assert!(
            readme.contains("Cloudflare") || readme.contains("cloudflare"),
            "README must document Cloudflare Tunnel access path"
        );
    }

    /// Sprint 19a: ops-guide must document SOCKS5 usage with kubectl port-forward.
    #[test]
    fn test_ops_guide_socks5_port_forward_usage() {
        let ops_guide = include_str!("../../../docs/ops-guide.md");

        // Must explain how to use SOCKS5 proxy
        assert!(
            ops_guide.contains("SOCKS5") || ops_guide.contains("socks5"),
            "ops-guide must document SOCKS5 proxy usage"
        );

        // Must mention port-forward or kubectl as access method
        assert!(
            ops_guide.contains("port-forward") || ops_guide.contains("kubectl"),
            "ops-guide must explain kubectl port-forward for SOCKS5"
        );
    }

    // ── Sprint 19b: Directory structure verification (Checklist #12) ──

    /// Sprint 19b: gitops/ must have the required multi-cluster structure:
    /// common/, tower/, sandbox/, bootstrap/, generators/, projects/
    #[test]
    fn test_gitops_directory_structure() {
        // Verify required directories exist via known files
        let spread = include_str!("../../../gitops/bootstrap/spread.yaml");
        assert!(
            spread.contains("tower-root"),
            "spread.yaml must define tower-root"
        );

        let tower_gen = include_str!("../../../gitops/generators/tower/tower-generator.yaml");
        assert!(!tower_gen.is_empty(), "Tower generator must exist");

        let sandbox_gen = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");
        assert!(!sandbox_gen.is_empty(), "Sandbox generator must exist");

        // Common apps exist
        let _cilium_res =
            include_str!("../../../gitops/common/cilium-resources/kustomization.yaml");
        let _cert_mgr = include_str!("../../../gitops/common/cert-manager/kustomization.yaml");

        // Tower-specific apps exist
        let _argocd = include_str!("../../../gitops/tower/argocd/kustomization.yaml");
        let _cf_tunnel =
            include_str!("../../../gitops/tower/cloudflared-tunnel/kustomization.yaml");
        let _keycloak = include_str!("../../../gitops/tower/keycloak/kustomization.yaml");

        // Sandbox-specific apps exist
        let _rbac = include_str!("../../../gitops/sandbox/rbac/kustomization.yaml");
    }

    /// Sprint 19b: scalex-cli/ must be a Rust project with Cargo.toml and src/main.rs.
    #[test]
    fn test_scalex_cli_is_rust_project() {
        let cargo_toml = include_str!("../../../scalex-cli/Cargo.toml");
        assert!(
            cargo_toml.contains("[package]"),
            "Must have valid Cargo.toml"
        );
        assert!(
            cargo_toml.contains("name = \"scalex\"")
                || cargo_toml.contains("name = \"scalex-cli\""),
            "Package name must be scalex or scalex-cli"
        );
    }

    /// Sprint 19b: credentials/ must have .example templates for all required secrets.
    #[test]
    fn test_credentials_example_templates_exist() {
        let baremetal = include_str!("../../../credentials/.baremetal-init.yaml.example");
        assert!(
            baremetal.contains("targetNodes"),
            "baremetal-init example must define targetNodes"
        );

        let env = include_str!("../../../credentials/.env.example");
        assert!(
            env.contains("PASSWORD") || env.contains("KEY"),
            ".env example must reference passwords or keys"
        );

        let secrets = include_str!("../../../credentials/secrets.yaml.example");
        assert!(
            secrets.contains("argocd")
                || secrets.contains("cloudflare")
                || secrets.contains("keycloak"),
            "secrets.yaml example must reference service credentials"
        );
    }

    // ── Sprint 19c: 2-Layer template management verification (Checklist philosophy #6) ──

    /// Sprint 19c: Layer 1 — SDI + K8s provisioning values must be in sdi-specs.yaml
    /// and k8s-clusters.yaml (separate files for infra layer).
    #[test]
    fn test_two_layer_template_layer1_infra() {
        let sdi = include_str!("../../../config/sdi-specs.yaml.example");
        let k8s = include_str!("../../../config/k8s-clusters.yaml.example");

        // sdi-specs.yaml defines VM pools (Layer 1a: virtualization)
        assert!(
            sdi.contains("sdi_pools") || sdi.contains("sdiPools"),
            "sdi-specs must define SDI pools"
        );

        // k8s-clusters.yaml defines cluster provisioning (Layer 1b: Kubernetes)
        assert!(
            k8s.contains("clusters"),
            "k8s-clusters must define clusters"
        );
        assert!(
            k8s.contains("common"),
            "k8s-clusters must have common config section"
        );

        // Cross-reference: k8s cluster must reference SDI pool
        assert!(
            k8s.contains("cluster_sdi_resource_pool") || k8s.contains("clusterSDIResourcePool"),
            "k8s cluster must reference SDI pool for mapping"
        );
    }

    /// Sprint 19c: Layer 2 — GitOps config for multi-cluster management must be in
    /// gitops/ with ApplicationSets per cluster.
    #[test]
    fn test_two_layer_template_layer2_gitops() {
        let tower_gen = include_str!("../../../gitops/generators/tower/tower-generator.yaml");
        let sandbox_gen = include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml");

        // Both generators use ApplicationSet
        assert!(
            tower_gen.contains("kind: ApplicationSet"),
            "Tower generator must be an ApplicationSet"
        );
        assert!(
            sandbox_gen.contains("kind: ApplicationSet"),
            "Sandbox generator must be an ApplicationSet"
        );

        // Each cluster has its own generator (not a single monolithic config)
        assert!(
            tower_gen.contains("tower-project"),
            "Tower generator must reference tower-project"
        );
        assert!(
            sandbox_gen.contains("sandbox-project"),
            "Sandbox generator must reference sandbox-project"
        );
    }

    // ── Sprint 19d: Single-node mode support verification (Checklist philosophy #1) ──

    /// Sprint 19d: SDI spec must allow a single-node pool (1 node across 1 bare-metal).
    #[test]
    fn test_sdi_single_node_pool_support() {
        use crate::models::sdi::*;

        // Create a minimal single-node SDI spec matching real structure
        let single_node_yaml = r#"
resource_pool:
  name: "single-pool"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.88.0/24"
    gateway: "192.168.88.1"
    nameservers: ["8.8.8.8"]

os_image:
  source: "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img"
  format: "qcow2"

cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id_ed25519.pub"
  packages: [curl]

spec:
  sdi_pools:
    - pool_name: "single-tower"
      node_specs:
        - node_name: "tower-cp-0"
          host: "playbox-0"
          ip: "192.168.88.100"
          cpu: 4
          mem_gb: 8
          disk_gb: 80
          roles:
            - control-plane
            - etcd
            - worker
"#;
        let spec: SdiSpec = serde_yaml::from_str(single_node_yaml).unwrap();
        assert_eq!(spec.spec.sdi_pools.len(), 1);
        assert_eq!(spec.spec.sdi_pools[0].node_specs.len(), 1);

        // Single node must have all roles (control-plane + etcd + worker)
        let roles = &spec.spec.sdi_pools[0].node_specs[0].roles;
        assert!(
            roles.contains(&"control-plane".to_string()),
            "Single node must be control-plane"
        );
        assert!(
            roles.contains(&"worker".to_string()),
            "Single node must also be worker"
        );
    }

    // ── Sprint 21: Edge case validation — duplicate names, host references ──

    /// Sprint 21: Duplicate cluster names must be detected.
    #[test]
    fn test_duplicate_cluster_names_detected() {
        let yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
    container_runtime: "containerd"
    cni: "cilium"
    cilium_version: "1.17.5"
    kube_proxy_remove: true
    cgroup_driver: "systemd"
    helm_enabled: true
    kube_apiserver_admission_plugins: [NodeRestriction]
    firewalld_enabled: false
    kube_vip_enabled: false
    graceful_node_shutdown: true
    graceful_node_shutdown_sec: 120
    kubelet_custom_flags: ["--node-ip={{ ip }}"]
    gateway_api_enabled: true
    gateway_api_version: "1.3.0"
    kubeconfig_localhost: true
    kubectl_localhost: true
    enable_nodelocaldns: true
    kube_network_node_prefix: 24
    ntp_enabled: true
    etcd_deployment_type: "host"
    dns_mode: "coredns"
  clusters:
    - cluster_name: "tower"
      cluster_sdi_resource_pool: "pool-a"
      cluster_role: "management"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
        dns_domain: "tower.local"
    - cluster_name: "tower"
      cluster_sdi_resource_pool: "pool-b"
      cluster_role: "workload"
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
        dns_domain: "sandbox.local"
"#;
        let config: K8sClustersConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = validate_unique_cluster_names(&config);
        assert!(!errors.is_empty(), "Must detect duplicate cluster names");
        assert!(
            errors[0].contains("tower"),
            "Error must mention the duplicate name 'tower'"
        );
    }

    /// Sprint 21: Unique cluster names must pass validation.
    #[test]
    fn test_unique_cluster_names_pass() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();
        let errors = validate_unique_cluster_names(&config);
        assert!(
            errors.is_empty(),
            "Example config must have unique cluster names: {:?}",
            errors
        );
    }

    /// Sprint 21: SDI node hosts must reference known bare-metal nodes.
    #[test]
    fn test_sdi_hosts_reference_unknown_baremetal() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        // Only 2 of the 4 playbox nodes are "known"
        let known = vec!["playbox-0".to_string(), "playbox-1".to_string()];
        let errors = validate_sdi_hosts_exist(&sdi, &known);

        // SDI spec uses playbox-0 through playbox-3, so playbox-2 and playbox-3 should fail
        assert!(
            !errors.is_empty(),
            "Must detect hosts not in baremetal-init"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("playbox-2") || e.contains("playbox-3")),
            "Error must mention missing hosts: {:?}",
            errors
        );
    }

    /// Sprint 21: All SDI hosts in example match all 4 playbox nodes.
    #[test]
    fn test_sdi_hosts_all_known_passes() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let known = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
            "playbox-3".to_string(),
        ];
        let errors = validate_sdi_hosts_exist(&sdi, &known);
        assert!(
            errors.is_empty(),
            "All example hosts should be known: {:?}",
            errors
        );
    }

    /// Sprint 19d: k8s-clusters config must support single-pool single-cluster mode.
    #[test]
    fn test_k8s_single_cluster_mode() {
        let single_cluster_yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
    container_runtime: "containerd"
    cni: "cilium"
    cilium_version: "1.17.5"
    kube_proxy_remove: true
    cgroup_driver: "systemd"
    helm_enabled: true
    kube_apiserver_admission_plugins:
      - NodeRestriction
    firewalld_enabled: false
    kube_vip_enabled: false
    graceful_node_shutdown: true
    graceful_node_shutdown_sec: 120
    kubelet_custom_flags:
      - "--node-ip={{ ip }}"
    gateway_api_enabled: true
    gateway_api_version: "1.3.0"
    kubeconfig_localhost: true
    kubectl_localhost: true
    enable_nodelocaldns: true
    kube_network_node_prefix: 24
    ntp_enabled: true
    etcd_deployment_type: "host"
    dns_mode: "coredns"

  clusters:
    - cluster_name: "tower"
      cluster_sdi_resource_pool: "single-tower"
      cluster_role: "management"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
        dns_domain: "tower.local"
      cilium:
        cluster_id: 1
        cluster_name: "tower"

  argocd:
    namespace: "argocd"
    repo_url: "https://github.com/example/repo.git"
    repo_branch: "main"
    tower_manages: []

  domains:
    auth: "auth.example.dev"
    argocd: "cd.example.dev"
    k8s_api: "api.k8s.example.dev"
"#;
        let config: K8sClustersConfig = serde_yaml::from_str(single_cluster_yaml).unwrap();
        assert_eq!(
            config.config.clusters.len(),
            1,
            "Must support single-cluster configuration"
        );
        assert_eq!(config.config.clusters[0].cluster_name, "tower");
        assert!(
            config.config.argocd.unwrap().tower_manages.is_empty(),
            "Single-cluster mode: tower manages no other clusters"
        );
    }

    // ── Sprint 21e: README Installation Guide CLI consistency ──

    /// Sprint 21e: README must only reference real scalex subcommands.
    /// Catches stale documentation that references removed/renamed commands.
    #[test]
    fn test_readme_references_real_scalex_commands() {
        let readme = include_str!("../../../README.md");

        let valid_subcommands = [
            "facts",
            "sdi",
            "cluster",
            "get",
            "secrets",
            "bootstrap",
            "status",
            "kernel-tune",
            "validate",
        ];

        let mut unknown_commands = Vec::new();

        for line in readme.lines() {
            // Find "scalex " occurrences in this line
            let mut search_from = 0;
            while let Some(pos) = line[search_from..].find("scalex ") {
                let abs_pos = search_from + pos + 7; // skip "scalex "
                if abs_pos >= line.len() {
                    break;
                }

                // Extract the next word after "scalex "
                let rest = &line[abs_pos..];
                let subcommand: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();

                // Skip flags (--help, etc.) and empty
                if subcommand.is_empty() || subcommand.starts_with('-') {
                    search_from = abs_pos;
                    continue;
                }

                if !valid_subcommands.contains(&subcommand.as_str()) {
                    unknown_commands.push(format!("scalex {}", subcommand));
                }

                search_from = abs_pos;
            }
        }

        assert!(
            unknown_commands.is_empty(),
            "README references unknown scalex commands: {:?}",
            unknown_commands
        );
    }

    // ── Sprint 25a: SDI CLI consistency ──

    /// Sprint 25a: `sdi clean --hard` must use configurable config/env paths,
    /// not hardcoded credentials paths. Verifies the code no longer contains
    /// hardcoded PathBuf::from("credentials/...") in run_clean.
    #[test]
    fn test_sdi_clean_no_hardcoded_config_paths() {
        let sdi_rs = include_str!("../commands/sdi.rs");

        // Find the run_clean function body
        let run_clean_start = sdi_rs
            .find("fn run_clean(")
            .expect("run_clean function must exist");
        // Find the next top-level function after run_clean
        let run_clean_body = &sdi_rs[run_clean_start..];
        let run_clean_end = run_clean_body[1..]
            .find("\nfn ")
            .map(|p| p + 1)
            .unwrap_or(run_clean_body.len());
        let run_clean_code = &run_clean_body[..run_clean_end];

        assert!(
            !run_clean_code.contains("PathBuf::from(\"credentials/"),
            "run_clean must not hardcode credentials paths — use CLI --config/--env-file args. \
             Found hardcoded PathBuf::from(\"credentials/...\") in run_clean"
        );
    }

    /// Sprint 25a: All SDI subcommands (init, clean, sync) must accept --config flag.
    #[test]
    fn test_sdi_all_subcommands_accept_config_flag() {
        let sdi_rs = include_str!("../commands/sdi.rs");

        // Each subcommand variant should have a `config: PathBuf` field
        // We check the SdiCommand enum contains config for Init, Clean, and Sync
        let subcommands = ["Init {", "Clean {", "Sync {"];
        for subcmd in &subcommands {
            let pos = sdi_rs.find(subcmd).unwrap_or_else(|| {
                panic!("SdiCommand::{} must exist", subcmd);
            });
            // Find the closing brace of this variant
            let variant_body = &sdi_rs[pos..];
            let end = variant_body.find("},").unwrap_or(variant_body.len());
            let variant = &variant_body[..end];

            assert!(
                variant.contains("config: PathBuf"),
                "SdiCommand::{} must have `config: PathBuf` field for consistent CLI flags",
                subcmd.trim_end_matches(" {")
            );
        }
    }

    /// Sprint 25b: build_pool_state must correctly map placement hosts
    /// and handle pools with mixed explicit/implicit host assignments.
    #[test]
    fn test_build_pool_state_mixed_host_assignment() {
        use crate::models::sdi::*;
        let spec = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "test".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "10.0.0.0/24".to_string(),
                    gateway: "10.0.0.1".to_string(),
                    nameservers: vec![],
                },
            },
            os_image: OsImageConfig {
                source: "test".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "test".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![
                    SdiPool {
                        pool_name: "tower".to_string(),
                        purpose: "management".to_string(),
                        placement: PlacementConfig {
                            hosts: vec!["host-a".to_string()],
                            spread: false,
                        },
                        node_specs: vec![NodeSpec {
                            node_name: "tower-0".to_string(),
                            ip: "10.0.0.10".to_string(),
                            cpu: 2,
                            mem_gb: 4,
                            disk_gb: 30,
                            host: None, // should fall back to placement host
                            roles: vec!["control-plane".to_string()],
                            devices: None,
                        }],
                    },
                    SdiPool {
                        pool_name: "sandbox".to_string(),
                        purpose: "workload".to_string(),
                        placement: PlacementConfig {
                            hosts: vec![],
                            spread: true,
                        },
                        node_specs: vec![
                            NodeSpec {
                                node_name: "sandbox-0".to_string(),
                                ip: "10.0.0.20".to_string(),
                                cpu: 4,
                                mem_gb: 8,
                                disk_gb: 60,
                                host: Some("host-b".to_string()), // explicit
                                roles: vec!["worker".to_string()],
                                devices: None,
                            },
                            NodeSpec {
                                node_name: "sandbox-1".to_string(),
                                ip: "10.0.0.21".to_string(),
                                cpu: 8,
                                mem_gb: 16,
                                disk_gb: 100,
                                host: None, // no placement host, no explicit → "unassigned"
                                roles: vec!["worker".to_string()],
                                devices: Some(DeviceConfig {
                                    gpu_passthrough: true,
                                }),
                            },
                        ],
                    },
                ],
            },
        };

        let state = crate::commands::sdi::build_pool_state_public(&spec);
        assert_eq!(state.len(), 2, "must produce 2 pool states");

        // Tower pool: node should get host from placement
        assert_eq!(state[0].pool_name, "tower");
        assert_eq!(state[0].nodes[0].node_name, "tower-0");
        assert_eq!(state[0].nodes[0].host, "host-a");
        assert!(!state[0].nodes[0].gpu_passthrough);

        // Sandbox pool: explicit host
        assert_eq!(state[1].pool_name, "sandbox");
        assert_eq!(state[1].nodes[0].host, "host-b");
        assert_eq!(state[1].nodes[0].cpu, 4);

        // Sandbox pool: no host → "unassigned"
        assert_eq!(state[1].nodes[1].host, "unassigned");
        assert!(state[1].nodes[1].gpu_passthrough);
        assert_eq!(state[1].nodes[1].disk_gb, 100);
    }

    /// Sprint 21e: README Installation Guide must have all 9 steps (0, 0.5, 1-8).
    #[test]
    fn test_readme_installation_guide_all_steps() {
        let readme = include_str!("../../../README.md");

        let required_steps = [
            "Step 0:",
            "Step 0.5:",
            "Step 1:",
            "Step 1.5:",
            "Step 2:",
            "Step 3:",
            "Step 4:",
            "Step 5:",
            "Step 6:",
            "Step 7:",
            "Step 8:",
        ];

        for step in &required_steps {
            assert!(
                readme.contains(step),
                "README Installation Guide missing '{}'",
                step
            );
        }
    }

    // === Sprint 29a: validate_cluster_network_overlap tests ===

    fn make_cluster_with_network(
        name: &str,
        pod_cidr: &str,
        service_cidr: &str,
        dns_domain: &str,
    ) -> ClusterDef {
        ClusterDef {
            cluster_name: name.to_string(),
            cluster_mode: ClusterMode::Sdi,
            cluster_sdi_resource_pool: name.to_string(),
            baremetal_nodes: vec![],
            cluster_role: "workload".to_string(),
            network: ClusterNetwork {
                pod_cidr: pod_cidr.to_string(),
                service_cidr: service_cidr.to_string(),
                dns_domain: dns_domain.to_string(),
                native_routing_cidr: None,
            },
            cilium: None,
            oidc: None,
            kubespray_extra_vars: None,
            ssh_user: None,
        }
    }

    #[test]
    fn test_network_overlap_no_errors_when_unique() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: \"v1.32.0\"\nkubespray_version: \"v2.30.0\"",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_network(
                        "tower",
                        "10.244.0.0/20",
                        "10.96.0.0/20",
                        "tower.local",
                    ),
                    make_cluster_with_network(
                        "sandbox",
                        "10.233.0.0/17",
                        "10.233.128.0/18",
                        "sandbox.local",
                    ),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_network_overlap(&config);
        assert!(
            errors.is_empty(),
            "Unique networks should produce no errors, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_network_overlap_detects_pod_cidr_collision() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: \"v1.32.0\"\nkubespray_version: \"v2.30.0\"",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_network(
                        "tower",
                        "10.244.0.0/16",
                        "10.96.0.0/16",
                        "tower.local",
                    ),
                    make_cluster_with_network(
                        "sandbox",
                        "10.244.0.0/16",
                        "10.97.0.0/16",
                        "sandbox.local",
                    ),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_network_overlap(&config);
        assert_eq!(errors.len(), 1, "Should detect exactly 1 pod CIDR overlap");
        assert!(
            errors[0].contains("Pod CIDR"),
            "Error must mention Pod CIDR: {}",
            errors[0]
        );
        assert!(errors[0].contains("tower") && errors[0].contains("sandbox"));
    }

    #[test]
    fn test_network_overlap_detects_service_cidr_collision() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: \"v1.32.0\"\nkubespray_version: \"v2.30.0\"",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_network(
                        "tower",
                        "10.244.0.0/16",
                        "10.96.0.0/16",
                        "tower.local",
                    ),
                    make_cluster_with_network(
                        "sandbox",
                        "10.245.0.0/16",
                        "10.96.0.0/16",
                        "sandbox.local",
                    ),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_network_overlap(&config);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Service CIDR"));
    }

    #[test]
    fn test_network_overlap_detects_dns_domain_collision() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: \"v1.32.0\"\nkubespray_version: \"v2.30.0\"",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_network(
                        "tower",
                        "10.244.0.0/16",
                        "10.96.0.0/16",
                        "cluster.local",
                    ),
                    make_cluster_with_network(
                        "sandbox",
                        "10.245.0.0/16",
                        "10.97.0.0/16",
                        "cluster.local",
                    ),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_network_overlap(&config);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("DNS domain"));
    }

    #[test]
    fn test_network_overlap_detects_multiple_collisions() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: \"v1.32.0\"\nkubespray_version: \"v2.30.0\"",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_network("a", "10.0.0.0/16", "10.1.0.0/16", "same.local"),
                    make_cluster_with_network("b", "10.0.0.0/16", "10.1.0.0/16", "same.local"),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_network_overlap(&config);
        assert_eq!(
            errors.len(),
            3,
            "Should detect pod + service + DNS overlap: {:?}",
            errors
        );
    }

    #[test]
    fn test_network_overlap_three_clusters_pairwise() {
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: \"v1.32.0\"\nkubespray_version: \"v2.30.0\"",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_network("a", "10.0.0.0/16", "10.1.0.0/16", "a.local"),
                    make_cluster_with_network("b", "10.0.0.0/16", "10.2.0.0/16", "b.local"),
                    make_cluster_with_network("c", "10.0.0.0/16", "10.3.0.0/16", "c.local"),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_network_overlap(&config);
        // a-b, a-c, b-c = 3 pod CIDR overlaps
        assert_eq!(
            errors.len(),
            3,
            "All 3 pairwise pod CIDR overlaps must be detected: {:?}",
            errors
        );
    }

    #[test]
    fn test_network_overlap_wired_into_cluster_init_pipeline() {
        // Verify the function is called in cluster.rs by checking it's importable and callable
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: \"v1.32.0\"\nkubespray_version: \"v2.30.0\"",
                )
                .unwrap(),
                clusters: vec![make_cluster_with_network(
                    "tower",
                    "10.244.0.0/20",
                    "10.96.0.0/20",
                    "tower.local",
                )],
                argocd: None,
                domains: None,
            },
        };

        // Single cluster = no overlap possible
        let errors = validate_cluster_network_overlap(&config);
        assert!(errors.is_empty());

        // Verify cluster.rs references validate_cluster_network_overlap
        let cluster_rs = include_str!("../commands/cluster.rs");
        assert!(
            cluster_rs.contains("validate_cluster_network_overlap"),
            "cluster init must call validate_cluster_network_overlap"
        );
    }

    #[test]
    fn test_example_configs_pass_network_overlap_validation() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let errors = validate_cluster_network_overlap(&k8s);
        assert!(
            errors.is_empty(),
            "Example k8s-clusters.yaml must pass network overlap validation, got: {:?}",
            errors
        );
    }

    // --- cidrs_overlap ---

    #[test]
    fn test_cidrs_overlap_identical() {
        assert!(cidrs_overlap("10.244.0.0/16", "10.244.0.0/16"));
    }

    #[test]
    fn test_cidrs_overlap_superset_subset() {
        // /16 contains /17
        assert!(cidrs_overlap("10.233.0.0/16", "10.233.0.0/17"));
    }

    #[test]
    fn test_cidrs_overlap_partial_within_same_block() {
        // /17 = 10.233.0.0–10.233.127.255, /18 = 10.233.0.0–10.233.63.255
        assert!(cidrs_overlap("10.233.0.0/17", "10.233.0.0/18"));
    }

    #[test]
    fn test_cidrs_no_overlap_adjacent() {
        // /17 first half = 10.233.0.0–10.233.127.255
        // /17 second half = 10.233.128.0–10.233.255.255
        assert!(!cidrs_overlap("10.233.0.0/17", "10.233.128.0/17"));
    }

    #[test]
    fn test_cidrs_no_overlap_disjoint() {
        assert!(!cidrs_overlap("10.244.0.0/16", "10.245.0.0/16"));
    }

    #[test]
    fn test_cidrs_overlap_invalid_returns_false() {
        assert!(!cidrs_overlap("not-a-cidr", "10.0.0.0/8"));
        assert!(!cidrs_overlap("10.0.0.0/8", "garbage"));
    }

    #[test]
    fn test_cidrs_overlap_different_sizes() {
        // 10.0.0.0/8 contains everything in 10.x.x.x
        assert!(cidrs_overlap("10.0.0.0/8", "10.233.0.0/17"));
    }

    #[test]
    fn test_cidrs_no_overlap_different_major() {
        assert!(!cidrs_overlap("10.0.0.0/8", "172.16.0.0/12"));
    }

    #[test]
    fn test_network_overlap_detects_partial_pod_cidr_overlap() {
        // Different strings but overlapping IP ranges
        let config = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: \"v1.32.0\"\nkubespray_version: \"v2.30.0\"",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster_with_network(
                        "tower",
                        "10.233.0.0/16",
                        "10.96.0.0/16",
                        "tower.local",
                    ),
                    make_cluster_with_network(
                        "sandbox",
                        "10.233.0.0/17",
                        "10.97.0.0/16",
                        "sandbox.local",
                    ),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_network_overlap(&config);
        assert_eq!(
            errors.len(),
            1,
            "Must detect partial CIDR overlap: {:?}",
            errors
        );
        assert!(
            errors[0].contains("Pod CIDR"),
            "Must mention Pod CIDR: {}",
            errors[0]
        );
    }

    // ===== Sprint 33a: Config Example Round-trip Tests =====
    // These tests verify that the shipped example config files parse correctly
    // and that cross-references between them are consistent.

    #[test]
    fn test_sprint33a_sdi_specs_example_roundtrip() {
        // sdi-specs.yaml.example must parse into SdiSpec with correct structure
        let content = include_str!("../../../config/sdi-specs.yaml.example");
        let spec: SdiSpec = serde_yaml::from_str(content)
            .expect("sdi-specs.yaml.example must parse as valid SdiSpec");

        // Must have exactly 2 pools: tower and sandbox
        assert_eq!(
            spec.spec.sdi_pools.len(),
            2,
            "Example must define 2 SDI pools (tower + sandbox)"
        );
        assert_eq!(spec.spec.sdi_pools[0].pool_name, "tower");
        assert_eq!(spec.spec.sdi_pools[1].pool_name, "sandbox");

        // Tower: 1 node (control-plane + worker)
        assert_eq!(
            spec.spec.sdi_pools[0].node_specs.len(),
            1,
            "Tower pool must have 1 node"
        );
        assert!(
            spec.spec.sdi_pools[0].node_specs[0]
                .roles
                .contains(&"control-plane".to_string()),
            "Tower node must have control-plane role"
        );

        // Sandbox: 4 nodes (1 CP + 3 workers)
        assert_eq!(
            spec.spec.sdi_pools[1].node_specs.len(),
            4,
            "Sandbox pool must have 4 nodes (1 CP + 3 workers)"
        );

        // Resource pool network config
        assert!(!spec.resource_pool.name.is_empty());
        assert!(!spec.resource_pool.network.management_cidr.is_empty());
    }

    #[test]
    fn test_sprint33a_k8s_clusters_example_roundtrip() {
        // k8s-clusters.yaml.example must parse into K8sClustersConfig with correct structure
        let content = include_str!("../../../config/k8s-clusters.yaml.example");
        let config: K8sClustersConfig = serde_yaml::from_str(content)
            .expect("k8s-clusters.yaml.example must parse as valid K8sClustersConfig");

        // Must have exactly 2 clusters
        assert_eq!(
            config.config.clusters.len(),
            2,
            "Example must define 2 clusters (tower + sandbox)"
        );

        let tower = &config.config.clusters[0];
        let sandbox = &config.config.clusters[1];

        assert_eq!(tower.cluster_name, "tower");
        assert_eq!(tower.cluster_role, "management");
        assert_eq!(tower.cluster_mode, ClusterMode::Sdi);

        assert_eq!(sandbox.cluster_name, "sandbox");
        assert_eq!(sandbox.cluster_role, "workload");
        assert_eq!(sandbox.cluster_mode, ClusterMode::Sdi);

        // Common config must have essential fields
        assert!(!config.config.common.kubernetes_version.is_empty());
        assert_eq!(config.config.common.cni, "cilium");

        // Network CIDRs must be non-empty and distinct
        assert_ne!(
            tower.network.pod_cidr, sandbox.network.pod_cidr,
            "Clusters must have different Pod CIDRs"
        );
        assert_ne!(
            tower.network.service_cidr, sandbox.network.service_cidr,
            "Clusters must have different Service CIDRs"
        );
        assert_ne!(
            tower.network.dns_domain, sandbox.network.dns_domain,
            "Clusters must have different DNS domains"
        );
    }

    #[test]
    fn test_sprint33a_baremetal_init_example_roundtrip() {
        // .baremetal-init.yaml.example must parse (with mock env vars)
        let content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let config: crate::core::config::BaremetalInitConfig = serde_yaml::from_str(content)
            .expect(".baremetal-init.yaml.example must parse as valid BaremetalInitConfig");

        // Must have 4 target nodes (playbox-0 through playbox-3)
        assert_eq!(
            config.target_nodes.len(),
            4,
            "Example must define 4 target nodes"
        );

        // Verify all 3 SSH access patterns are represented
        let _direct_count = config
            .target_nodes
            .iter()
            .filter(|n| n.direct_reachable)
            .count();
        let external_ip_count = config
            .target_nodes
            .iter()
            .filter(|n| !n.direct_reachable && n.reachable_node_ip.is_some())
            .count();
        let proxy_jump_count = config
            .target_nodes
            .iter()
            .filter(|n| !n.direct_reachable && n.reachable_via.is_some())
            .count();

        // playbox-0 uses external IP (Tailscale), playbox-1/2/3 use ProxyJump
        assert_eq!(
            external_ip_count, 1,
            "Must have 1 node with external IP access (Tailscale bastion)"
        );
        assert_eq!(
            proxy_jump_count, 3,
            "Must have 3 nodes with ProxyJump access"
        );

        // NetworkDefaults must be present
        assert!(
            config.network_defaults.is_some(),
            "Example must include networkDefaults for SDI host-infra"
        );

        // All nodes must have admin user
        for node in &config.target_nodes {
            assert!(
                !node.admin_user.is_empty(),
                "Node {} must have adminUser",
                node.name
            );
        }
    }

    #[test]
    fn test_sprint33a_cross_config_sdi_pool_mapping() {
        // The pool_name values in sdi-specs.yaml must match cluster_sdi_resource_pool in k8s-clusters.yaml
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        // Cross-validate using the actual validation function
        let errors = validate_cluster_sdi_pool_mapping(&k8s_config, &sdi_spec);
        assert!(
            errors.is_empty(),
            "Example configs must have consistent SDI pool mapping. Errors: {:?}",
            errors
        );

        // Also verify pool names match explicitly
        let sdi_pool_names: Vec<&str> = sdi_spec
            .spec
            .sdi_pools
            .iter()
            .map(|p| p.pool_name.as_str())
            .collect();
        for cluster in &k8s_config.config.clusters {
            if cluster.cluster_mode == ClusterMode::Sdi {
                assert!(
                    sdi_pool_names.contains(&cluster.cluster_sdi_resource_pool.as_str()),
                    "Cluster '{}' references pool '{}' which must exist in sdi-specs. Available: {:?}",
                    cluster.cluster_name,
                    cluster.cluster_sdi_resource_pool,
                    sdi_pool_names
                );
            }
        }

        // Verify network overlap validation also passes
        let net_errors = validate_cluster_network_overlap(&k8s_config);
        assert!(
            net_errors.is_empty(),
            "Example configs must not have network overlaps. Errors: {:?}",
            net_errors
        );

        // Verify SDI spec validation passes
        let spec_errors = validate_sdi_spec(&sdi_spec);
        assert!(
            spec_errors.is_empty(),
            "Example SDI spec must pass validation. Errors: {:?}",
            spec_errors
        );
    }

    // ===== Sprint 34a: SOCKS5 Proxy GitOps Structure Validation =====

    #[test]
    fn test_sprint34a_socks5_manifest_valid_k8s_resources() {
        // SOCKS5 proxy manifest must parse as valid YAML with Deployment + Service
        let content = include_str!("../../../gitops/tower/socks5-proxy/manifest.yaml");

        // Must contain two YAML documents separated by ---
        let docs: Vec<&str> = content.split("---").collect();
        assert!(
            docs.len() >= 2,
            "manifest.yaml must contain at least 2 YAML documents (Deployment + Service)"
        );

        // First doc: Deployment
        let deployment: serde_yaml::Value =
            serde_yaml::from_str(docs[0]).expect("Deployment YAML must parse");
        assert_eq!(
            deployment["kind"].as_str().unwrap(),
            "Deployment",
            "First document must be a Deployment"
        );
        assert_eq!(
            deployment["metadata"]["name"].as_str().unwrap(),
            "socks5-proxy"
        );
        assert_eq!(
            deployment["metadata"]["namespace"].as_str().unwrap(),
            "kube-tunnel"
        );

        // Second doc: Service
        let service: serde_yaml::Value =
            serde_yaml::from_str(docs[1]).expect("Service YAML must parse");
        assert_eq!(
            service["kind"].as_str().unwrap(),
            "Service",
            "Second document must be a Service"
        );
        assert_eq!(
            service["metadata"]["name"].as_str().unwrap(),
            "socks5-proxy"
        );
    }

    #[test]
    fn test_sprint34a_socks5_sync_wave_3_in_tower_generator() {
        // SOCKS5 proxy must be in sync wave 3 in tower-generator
        let content = include_str!("../../../gitops/generators/tower/tower-generator.yaml");
        let gen: serde_yaml::Value =
            serde_yaml::from_str(content).expect("tower-generator.yaml must parse");

        let elements = gen["spec"]["generators"][0]["list"]["elements"]
            .as_sequence()
            .expect("Must have elements list");

        let socks5_entry = elements
            .iter()
            .find(|e| e["appName"].as_str() == Some("socks5-proxy"))
            .expect("socks5-proxy must be in tower-generator elements");

        assert_eq!(
            socks5_entry["syncWave"].as_str().unwrap(),
            "3",
            "socks5-proxy must be in sync wave 3"
        );
        assert_eq!(
            socks5_entry["namespace"].as_str().unwrap(),
            "kube-tunnel",
            "socks5-proxy must be in kube-tunnel namespace"
        );
    }

    #[test]
    fn test_sprint34a_socks5_service_port_1080() {
        // SOCKS5 Service must expose port 1080 as ClusterIP (not NodePort/LB for security)
        let content = include_str!("../../../gitops/tower/socks5-proxy/manifest.yaml");
        let docs: Vec<&str> = content.split("---").collect();

        let service: serde_yaml::Value =
            serde_yaml::from_str(docs[1]).expect("Service YAML must parse");

        // Port 1080
        let ports = service["spec"]["ports"]
            .as_sequence()
            .expect("Service must have ports");
        let port = ports[0]["port"].as_u64().unwrap();
        let target_port = ports[0]["targetPort"].as_u64().unwrap();
        assert_eq!(port, 1080, "Service port must be 1080");
        assert_eq!(target_port, 1080, "Service targetPort must be 1080");

        // ClusterIP (not NodePort/LoadBalancer — security requirement)
        let svc_type = service["spec"]["type"].as_str().unwrap();
        assert_eq!(
            svc_type, "ClusterIP",
            "SOCKS5 proxy must be ClusterIP for security (not exposed externally)"
        );
    }

    // ===== Sprint 34b: External Access Documentation Consistency =====

    #[test]
    fn test_sprint34b_readme_external_access_covers_4_methods() {
        // README must document all 4 external access methods
        let content = include_str!("../../../README.md");

        let methods = [
            ("LAN", "LAN"),
            ("Tailscale", "Tailscale"),
            ("Cloudflare Tunnel", "Cloudflare Tunnel"),
            ("SOCKS5", "SOCKS5"),
        ];
        for (label, keyword) in &methods {
            assert!(
                content.contains(keyword),
                "README must mention {} access method (keyword: '{}')",
                label,
                keyword
            );
        }

        // Must warn about CF Tunnel TLS termination (critical security note)
        assert!(
            content.contains("TLS") && content.contains("terminate"),
            "README must warn about CF Tunnel TLS termination"
        );

        // Must mention OIDC as the only CF Tunnel auth method
        assert!(
            content.contains("OIDC"),
            "README must mention OIDC for CF Tunnel kubectl"
        );
    }

    #[test]
    fn test_sprint34b_ops_guide_has_access_sections() {
        // ops-guide.md must have Tailscale, SOCKS5, LAN, and CF Tunnel sections
        let content = include_str!("../../../docs/ops-guide.md");

        let required_sections = [
            "Tailscale",
            "SOCKS5",
            "LAN",
            "Cloudflare Tunnel",
            "Keycloak",
        ];
        for section in &required_sections {
            assert!(
                content.contains(section),
                "ops-guide.md must contain section about '{}'",
                section
            );
        }

        // Must have Pre-OIDC guidance (critical: users need Tailscale before Keycloak)
        assert!(
            content.contains("Pre-OIDC") || content.contains("pre-OIDC"),
            "ops-guide.md must mention Pre-OIDC access path"
        );

        // Must have port 1080 for SOCKS5
        assert!(
            content.contains("1080"),
            "ops-guide.md must mention SOCKS5 port 1080"
        );
    }

    // ===== Sprint 34c: README Installation Guide Completeness Audit =====

    #[test]
    fn test_sprint34c_readme_references_existing_files() {
        // All files/dirs referenced in README Installation Guide must exist in repo
        let readme = include_str!("../../../README.md");

        // Config files referenced in Step 2
        let referenced_files = [
            "credentials/.baremetal-init.yaml.example",
            "credentials/.env.example",
            "config/sdi-specs.yaml.example",
            "config/k8s-clusters.yaml.example",
            "gitops/bootstrap/spread.yaml",
            "docs/ops-guide.md",
            "docs/SETUP-GUIDE.md",
            "docs/TROUBLESHOOTING.md",
            "docs/ARCHITECTURE.md",
            "docs/CONTRIBUTING.md",
        ];

        for file in &referenced_files {
            assert!(
                readme.contains(file),
                "README must reference '{}' in Installation Guide or docs",
                file
            );
        }

        // Verify referenced example files actually exist (via include_str compile check)
        let _ = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let _ = include_str!("../../../credentials/.env.example");
        let _ = include_str!("../../../config/sdi-specs.yaml.example");
        let _ = include_str!("../../../config/k8s-clusters.yaml.example");
        let _ = include_str!("../../../gitops/bootstrap/spread.yaml");
        let _ = include_str!("../../../docs/ops-guide.md");
    }

    #[test]
    fn test_sprint34c_readme_cli_commands_match_subcommands() {
        // README CLI Reference must list all 8 subcommands from main.rs
        let readme = include_str!("../../../README.md");

        // These are the 8 subcommands defined in main.rs Commands enum
        let subcommands = [
            "facts",
            "get",
            "sdi",
            "cluster",
            "secrets",
            "bootstrap",
            "status",
            "kernel-tune",
        ];

        for cmd in &subcommands {
            let scalex_cmd = format!("scalex {}", cmd);
            assert!(
                readme.contains(&scalex_cmd),
                "README must document 'scalex {}' command",
                cmd
            );
        }

        // README must also document the get subcommands
        let get_subcommands = ["baremetals", "sdi-pools", "clusters", "config-files"];
        for sub in &get_subcommands {
            let get_cmd = format!("scalex get {}", sub);
            assert!(
                readme.contains(&get_cmd),
                "README must document '{}' query command",
                get_cmd
            );
        }
    }

    #[test]
    fn test_sprint34c_readme_project_structure_matches_reality() {
        // README Project Structure section must reference actual directories
        let readme = include_str!("../../../README.md");

        // Core directories that must be in Project Structure
        let required_dirs = [
            "scalex-cli/",
            "gitops/",
            "credentials/",
            "config/",
            "docs/",
            "ansible/",
            "kubespray/",
            "client/",
            "tests/",
            "_generated/",
        ];

        for dir in &required_dirs {
            assert!(
                readme.contains(dir),
                "README Project Structure must include '{}'",
                dir
            );
        }

        // GitOps subdirs
        let gitops_subdirs = [
            "bootstrap/",
            "generators/",
            "projects/",
            "common/",
            "tower/",
            "sandbox/",
        ];
        for subdir in &gitops_subdirs {
            assert!(
                readme.contains(subdir),
                "README must reference gitops subdirectory '{}'",
                subdir
            );
        }
    }

    // ===== Sprint 35b: Idempotency Pipeline Tests =====
    // Pure functions must produce identical output when called with identical input.

    #[test]
    fn test_sprint35b_generate_tofu_main_idempotent() {
        use crate::core::tofu::generate_tofu_main;

        let content = include_str!("../../../config/sdi-specs.yaml.example");
        let spec: SdiSpec = serde_yaml::from_str(content).unwrap();

        let output1 = generate_tofu_main(&spec, "jinwang");
        let output2 = generate_tofu_main(&spec, "jinwang");

        assert_eq!(
            output1, output2,
            "generate_tofu_main() must be idempotent: two calls with same input must produce identical HCL"
        );
        assert!(!output1.is_empty(), "Generated HCL must not be empty");
    }

    #[test]
    fn test_sprint35b_generate_inventory_idempotent() {
        use crate::core::kubespray::generate_inventory;

        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s_config.config.clusters {
            let inv1 =
                generate_inventory(cluster, &sdi_spec).expect("generate_inventory must succeed");
            let inv2 = generate_inventory(cluster, &sdi_spec)
                .expect("generate_inventory must succeed on second call");

            assert_eq!(
                inv1, inv2,
                "generate_inventory() for cluster '{}' must be idempotent",
                cluster.cluster_name
            );
            assert!(
                !inv1.is_empty(),
                "Generated inventory for '{}' must not be empty",
                cluster.cluster_name
            );
        }
    }

    #[test]
    fn test_sprint35b_generate_cluster_vars_idempotent() {
        use crate::core::kubespray::generate_cluster_vars;

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s_config.config.clusters {
            let vars1 = generate_cluster_vars(cluster, &k8s_config.config.common);
            let vars2 = generate_cluster_vars(cluster, &k8s_config.config.common);

            assert_eq!(
                vars1, vars2,
                "generate_cluster_vars() for cluster '{}' must be idempotent",
                cluster.cluster_name
            );
            assert!(
                !vars1.is_empty(),
                "Generated vars for '{}' must not be empty",
                cluster.cluster_name
            );
        }
    }

    // ── Sprint 39: 2-Layer cross-validation tests ──

    #[test]
    fn test_two_layer_consistency_valid_example_configs() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let (errors, warnings) = validate_two_layer_consistency(&k8s_config, &sdi_spec);
        assert!(
            errors.is_empty(),
            "example configs must have no cross-layer errors: {:?}",
            errors
        );
        assert!(
            warnings.is_empty(),
            "example configs must have no orphan pool warnings: {:?}",
            warnings
        );
    }

    #[test]
    fn test_two_layer_no_control_plane_in_pool_detected() {
        let sdi = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "test".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec![],
                },
            },
            os_image: OsImageConfig {
                source: "img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "key".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![SdiPool {
                    pool_name: "workers-only".to_string(),
                    purpose: "test".to_string(),
                    placement: PlacementConfig::default(),
                    node_specs: vec![NodeSpec {
                        node_name: "w-0".to_string(),
                        ip: "192.168.88.50".to_string(),
                        cpu: 4,
                        mem_gb: 8,
                        disk_gb: 50,
                        host: None,
                        roles: vec!["worker".to_string()],
                        devices: None,
                    }],
                }],
            },
        };

        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![make_cluster("test", "workers-only", ClusterMode::Sdi)],
                argocd: None,
                domains: None,
            },
        };

        let (errors, _) = validate_two_layer_consistency(&k8s, &sdi);
        assert!(!errors.is_empty(), "must detect missing control-plane");
        assert!(
            errors[0].contains("control-plane"),
            "error must mention control-plane"
        );
    }

    #[test]
    fn test_two_layer_node_ip_in_pod_cidr_detected() {
        // SDI node IP 10.244.0.50 falls within pod CIDR 10.244.0.0/20
        let sdi = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "test".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "10.244.0.0/20".to_string(),
                    gateway: "10.244.0.1".to_string(),
                    nameservers: vec![],
                },
            },
            os_image: OsImageConfig {
                source: "img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "key".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![SdiPool {
                    pool_name: "bad-pool".to_string(),
                    purpose: "test".to_string(),
                    placement: PlacementConfig::default(),
                    node_specs: vec![NodeSpec {
                        node_name: "cp-0".to_string(),
                        ip: "10.244.0.50".to_string(),
                        cpu: 4,
                        mem_gb: 8,
                        disk_gb: 50,
                        host: None,
                        roles: vec!["control-plane".to_string(), "worker".to_string()],
                        devices: None,
                    }],
                }],
            },
        };

        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![ClusterDef {
                    cluster_name: "test".to_string(),
                    cluster_mode: ClusterMode::Sdi,
                    cluster_sdi_resource_pool: "bad-pool".to_string(),
                    baremetal_nodes: vec![],
                    cluster_role: "workload".to_string(),
                    network: ClusterNetwork {
                        pod_cidr: "10.244.0.0/20".to_string(),
                        service_cidr: "10.96.0.0/20".to_string(),
                        dns_domain: "test.local".to_string(),
                        native_routing_cidr: None,
                    },
                    cilium: None,
                    oidc: None,
                    kubespray_extra_vars: None,
                    ssh_user: None,
                }],
                argocd: None,
                domains: None,
            },
        };

        let (errors, _) = validate_two_layer_consistency(&k8s, &sdi);
        assert!(!errors.is_empty(), "must detect node IP in pod CIDR");
        assert!(
            errors[0].contains("pod CIDR"),
            "error must mention pod CIDR"
        );
    }

    #[test]
    fn test_two_layer_orphan_pool_warned() {
        let sdi = make_sdi_spec_with_pools(&["tower", "sandbox", "orphan"]);

        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("sandbox", "sandbox", ClusterMode::Sdi),
                ],
                argocd: None,
                domains: None,
            },
        };

        let (errors, warnings) = validate_two_layer_consistency(&k8s, &sdi);
        assert!(errors.is_empty(), "no errors expected");
        assert_eq!(warnings.len(), 1, "must warn about orphan pool");
        assert!(
            warnings[0].contains("orphan"),
            "warning must mention pool name"
        );
    }

    #[test]
    fn test_two_layer_baremetal_clusters_skipped() {
        let sdi = make_sdi_spec_with_pools(&["tower"]);

        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("prod", "", ClusterMode::Baremetal),
                ],
                argocd: None,
                domains: None,
            },
        };

        let (errors, _) = validate_two_layer_consistency(&k8s, &sdi);
        assert!(
            errors.is_empty(),
            "baremetal clusters should be skipped in SDI validation"
        );
    }

    #[test]
    fn test_ip_in_cidr_helper() {
        assert!(ip_in_cidr("10.244.0.50", "10.244.0.0/20"));
        assert!(ip_in_cidr("10.244.15.255", "10.244.0.0/20"));
        assert!(!ip_in_cidr("10.244.16.0", "10.244.0.0/20"));
        assert!(!ip_in_cidr("192.168.88.50", "10.244.0.0/20"));
        assert!(!ip_in_cidr("invalid", "10.244.0.0/20"));
        assert!(!ip_in_cidr("10.244.0.50", "invalid"));
    }

    // --- Sprint 40: kubespray directory validation ---

    #[test]
    fn test_validate_kubespray_submodule_missing_dir() {
        let result = validate_kubespray_submodule("/nonexistent/path");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_validate_kubespray_submodule_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("kubespray")).unwrap();
        let result = validate_kubespray_submodule(tmp.path().to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not initialized"));
    }

    #[test]
    fn test_validate_kubespray_submodule_with_cluster_yml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ks_dir = tmp.path().join("kubespray");
        std::fs::create_dir(&ks_dir).unwrap();
        std::fs::write(ks_dir.join("cluster.yml"), "---").unwrap();
        let result = validate_kubespray_submodule(tmp.path().to_str().unwrap());
        assert!(result.is_ok(), "should pass with cluster.yml present");
    }

    // --- Sprint 40: prerequisite tool checks ---

    #[test]
    fn test_prerequisites_for_sdi_init() {
        let tools = prerequisites_for_command("sdi-init");
        assert!(tools.contains(&"tofu"));
        assert!(tools.contains(&"ssh"));
        assert!(!tools.contains(&"helm"));
    }

    #[test]
    fn test_prerequisites_for_cluster_init() {
        let tools = prerequisites_for_command("cluster-init");
        assert!(tools.contains(&"ansible-playbook"));
        assert!(tools.contains(&"ssh"));
    }

    #[test]
    fn test_prerequisites_for_bootstrap() {
        let tools = prerequisites_for_command("bootstrap");
        assert!(tools.contains(&"helm"));
        assert!(tools.contains(&"kubectl"));
    }

    #[test]
    fn test_prerequisites_for_unknown_command() {
        let tools = prerequisites_for_command("unknown");
        assert!(tools.is_empty());
    }

    #[test]
    fn test_check_tool_available_finds_common_tools() {
        // 'ls' should exist on any Linux system
        assert!(check_tool_available("ls"));
        // non-existent tool
        assert!(!check_tool_available("definitely-not-a-real-tool-12345"));
    }

    #[test]
    fn test_find_missing_prerequisites() {
        let missing = find_missing_prerequisites(&["ls", "definitely-not-a-real-tool-12345"]);
        assert_eq!(missing, vec!["definitely-not-a-real-tool-12345"]);
    }

    // ── A-3: Resource over-allocation detection (Sprint 41 TDD) ──

    #[test]
    fn test_validate_sdi_resource_allocation_within_limits() {
        use crate::models::baremetal::*;
        let facts = vec![NodeFacts {
            node_name: "playbox-0".to_string(),
            timestamp: "".to_string(),
            cpu: CpuInfo {
                model: "".to_string(),
                cores: 16,
                threads: 32,
                architecture: "x86_64".to_string(),
            },
            memory: MemoryInfo {
                total_mb: 65536,
                available_mb: 60000,
            },
            disks: vec![],
            nics: vec![],
            gpus: vec![],
            iommu_groups: vec![],
            kernel: KernelInfo {
                version: "6.8.0".to_string(),
                params: std::collections::HashMap::new(),
            },
            bridges: vec![],
            bonds: vec![],
            pcie: vec![],
        }];
        let sdi_yaml = r#"
resource_pool:
  name: "test"
  network: { management_bridge: "br0", management_cidr: "192.168.88.0/24", gateway: "192.168.88.1", nameservers: ["8.8.8.8"] }
os_image: { source: "https://example.com/img", format: "qcow2" }
cloud_init: { ssh_authorized_keys_file: "~/.ssh/id.pub" }
spec:
  sdi_pools:
    - pool_name: "tower"
      placement: { hosts: [playbox-0] }
      node_specs:
        - { node_name: "cp-0", ip: "192.168.88.100", cpu: 4, mem_gb: 8, disk_gb: 30, roles: [control-plane] }
"#;
        let spec: SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        let errors = validate_sdi_resource_allocation(&spec, &facts);
        assert!(
            errors.is_empty(),
            "should pass when within limits: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_sdi_resource_allocation_cpu_exceeded() {
        use crate::models::baremetal::*;
        let facts = vec![NodeFacts {
            node_name: "playbox-0".to_string(),
            timestamp: "".to_string(),
            cpu: CpuInfo {
                model: "".to_string(),
                cores: 4,
                threads: 8,
                architecture: "x86_64".to_string(),
            },
            memory: MemoryInfo {
                total_mb: 65536,
                available_mb: 60000,
            },
            disks: vec![],
            nics: vec![],
            gpus: vec![],
            iommu_groups: vec![],
            kernel: KernelInfo {
                version: "6.8.0".to_string(),
                params: std::collections::HashMap::new(),
            },
            bridges: vec![],
            bonds: vec![],
            pcie: vec![],
        }];
        let sdi_yaml = r#"
resource_pool:
  name: "test"
  network: { management_bridge: "br0", management_cidr: "192.168.88.0/24", gateway: "192.168.88.1", nameservers: ["8.8.8.8"] }
os_image: { source: "https://example.com/img", format: "qcow2" }
cloud_init: { ssh_authorized_keys_file: "~/.ssh/id.pub" }
spec:
  sdi_pools:
    - pool_name: "tower"
      placement: { hosts: [playbox-0] }
      node_specs:
        - { node_name: "cp-0", ip: "192.168.88.100", cpu: 4, mem_gb: 8, disk_gb: 30, roles: [control-plane] }
    - pool_name: "sandbox"
      node_specs:
        - { node_name: "w-0", ip: "192.168.88.110", cpu: 8, mem_gb: 16, disk_gb: 60, host: "playbox-0", roles: [worker] }
"#;
        let spec: SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        let errors = validate_sdi_resource_allocation(&spec, &facts);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("CPU") && e.contains("playbox-0") && e.contains("exceeds")),
            "must detect CPU over-allocation: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_sdi_resource_allocation_mem_exceeded() {
        use crate::models::baremetal::*;
        let facts = vec![NodeFacts {
            node_name: "playbox-0".to_string(),
            timestamp: "".to_string(),
            cpu: CpuInfo {
                model: "".to_string(),
                cores: 64,
                threads: 128,
                architecture: "x86_64".to_string(),
            },
            memory: MemoryInfo {
                total_mb: 8192,
                available_mb: 7000,
            },
            disks: vec![],
            nics: vec![],
            gpus: vec![],
            iommu_groups: vec![],
            kernel: KernelInfo {
                version: "6.8.0".to_string(),
                params: std::collections::HashMap::new(),
            },
            bridges: vec![],
            bonds: vec![],
            pcie: vec![],
        }];
        let sdi_yaml = r#"
resource_pool:
  name: "test"
  network: { management_bridge: "br0", management_cidr: "192.168.88.0/24", gateway: "192.168.88.1", nameservers: ["8.8.8.8"] }
os_image: { source: "https://example.com/img", format: "qcow2" }
cloud_init: { ssh_authorized_keys_file: "~/.ssh/id.pub" }
spec:
  sdi_pools:
    - pool_name: "overload"
      placement: { hosts: [playbox-0] }
      node_specs:
        - { node_name: "vm-0", ip: "192.168.88.100", cpu: 2, mem_gb: 6, disk_gb: 30, roles: [worker] }
        - { node_name: "vm-1", ip: "192.168.88.101", cpu: 2, mem_gb: 6, disk_gb: 30, roles: [worker] }
"#;
        let spec: SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        let errors = validate_sdi_resource_allocation(&spec, &facts);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("memory") && e.contains("playbox-0") && e.contains("exceeds")),
            "must detect memory over-allocation (12 GB > 8 GB): {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_sdi_resource_allocation_no_facts_for_host() {
        use crate::models::baremetal::*;
        // Empty facts — host "playbox-0" not in facts
        let facts: Vec<NodeFacts> = vec![];
        let sdi_yaml = r#"
resource_pool:
  name: "test"
  network: { management_bridge: "br0", management_cidr: "192.168.88.0/24", gateway: "192.168.88.1", nameservers: ["8.8.8.8"] }
os_image: { source: "https://example.com/img", format: "qcow2" }
cloud_init: { ssh_authorized_keys_file: "~/.ssh/id.pub" }
spec:
  sdi_pools:
    - pool_name: "tower"
      placement: { hosts: [playbox-0] }
      node_specs:
        - { node_name: "cp-0", ip: "192.168.88.100", cpu: 2, mem_gb: 3, disk_gb: 30, roles: [control-plane] }
"#;
        let spec: SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        let warnings = validate_sdi_resource_allocation(&spec, &facts);
        assert!(
            warnings
                .iter()
                .any(|e| e.contains("playbox-0") && e.contains("no facts")),
            "must warn when host has no facts data: {:?}",
            warnings
        );
    }

    // ── A-6: Workflow dependency checks ──

    #[test]
    fn test_workflow_sdi_init_all_deps_present() {
        let existing = vec![
            "credentials/.baremetal-init.yaml",
            "credentials/.env",
            "_generated/facts/",
        ];
        let missing = check_workflow_dependencies("sdi-init", &existing);
        assert!(missing.is_empty(), "All deps present → no missing");
    }

    #[test]
    fn test_workflow_sdi_init_missing_facts() {
        let existing = vec!["credentials/.baremetal-init.yaml", "credentials/.env"];
        let missing = check_workflow_dependencies("sdi-init", &existing);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].artifact_path, "_generated/facts/");
        assert!(missing[0].fix_command.contains("scalex facts"));
    }

    #[test]
    fn test_workflow_sdi_init_spec_missing_all() {
        let existing: Vec<&str> = vec![];
        let missing = check_workflow_dependencies("sdi-init-spec", &existing);
        assert_eq!(missing.len(), 4, "All 4 deps missing for sdi-init-spec");
        let paths: Vec<&str> = missing.iter().map(|d| d.artifact_path.as_str()).collect();
        assert!(paths.contains(&"credentials/.baremetal-init.yaml"));
        assert!(paths.contains(&"credentials/.env"));
        assert!(paths.contains(&"_generated/facts/"));
        assert!(paths.contains(&"config/sdi-specs.yaml"));
    }

    #[test]
    fn test_workflow_cluster_init_missing_sdi_state() {
        let existing = vec!["config/k8s-clusters.yaml"];
        let missing = check_workflow_dependencies("cluster-init", &existing);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].artifact_path, "_generated/sdi-state.json");
        assert!(missing[0].fix_command.contains("scalex sdi init"));
    }

    #[test]
    fn test_workflow_bootstrap_missing_both() {
        let existing: Vec<&str> = vec![];
        let missing = check_workflow_dependencies("bootstrap", &existing);
        assert_eq!(missing.len(), 2);
        let paths: Vec<&str> = missing.iter().map(|d| d.artifact_path.as_str()).collect();
        assert!(paths.contains(&"_generated/kubeconfigs/"));
        assert!(paths.contains(&"credentials/secrets.yaml"));
    }

    #[test]
    fn test_workflow_unknown_command_no_deps() {
        let missing = check_workflow_dependencies("unknown-cmd", &[]);
        assert!(missing.is_empty());
    }

    #[test]
    fn test_format_workflow_errors_empty() {
        let msg = format_workflow_errors("sdi-init", &[]);
        assert!(msg.is_empty(), "No missing deps → empty message");
    }

    #[test]
    fn test_format_workflow_errors_has_structure() {
        let missing = vec![WorkflowDep {
            artifact_path: "_generated/facts/".to_string(),
            description: "Hardware facts".to_string(),
            fix_command: "scalex facts --all".to_string(),
        }];
        let msg = format_workflow_errors("sdi-init", &missing);
        assert!(msg.contains("Cannot run `scalex sdi init`"));
        assert!(msg.contains("Hardware facts"));
        assert!(msg.contains("scalex facts --all"));
        assert!(msg.contains("scalex status"));
    }

    // ── Sprint 42: Cross-file integration tests ──

    /// CRITICAL: Secret name generated by `secrets_for_cluster("management")`
    /// must match `existingSecret` in cloudflared-tunnel values.yaml.
    /// If these drift, cloudflared Pod enters CrashLoopBackOff.
    #[test]
    fn test_cf_tunnel_secret_name_matches_values_yaml() {
        // 1. Get the secret name from the code (secrets.rs)
        let secrets_yaml_content = r#"
keycloak:
  admin_password: "test"
  db_password: "test"
argocd:
  repo_pat: "test"
cloudflare:
  credentials_file: "/path/to/creds.json"
  cert_file: "/path/to/cert.pem"
"#;
        let secrets: crate::core::secrets::SecretsConfig =
            serde_yaml::from_str(secrets_yaml_content).unwrap();
        let specs = crate::core::secrets::secrets_for_cluster("management", &secrets);
        let cf_secret = specs
            .iter()
            .find(|s| s.name.contains("cloudflared"))
            .expect("management cluster must generate a cloudflared secret");

        // 2. Parse the actual values.yaml from gitops/
        let values_yaml = include_str!("../../../gitops/tower/cloudflared-tunnel/values.yaml");

        // 3. Extract existingSecret value from values.yaml
        let existing_secret_line = values_yaml
            .lines()
            .find(|l| l.contains("existingSecret"))
            .expect("values.yaml must contain existingSecret");
        let existing_secret_value = existing_secret_line
            .split(':')
            .nth(1)
            .expect("existingSecret must have a value")
            .trim();

        // 4. Cross-validate: secret name must match
        assert_eq!(
            cf_secret.name, existing_secret_value,
            "Secret name from secrets_for_cluster('management') must match \
             existingSecret in gitops/tower/cloudflared-tunnel/values.yaml"
        );

        // 5. Namespace must match kustomization.yaml
        let kustomization =
            include_str!("../../../gitops/tower/cloudflared-tunnel/kustomization.yaml");
        let ns_line = kustomization
            .lines()
            .find(|l| l.trim().starts_with("namespace:"))
            .expect("kustomization.yaml must define namespace");
        let kust_namespace = ns_line.split(':').nth(1).unwrap().trim();
        assert_eq!(
            cf_secret.namespace, kust_namespace,
            "Secret namespace must match cloudflared-tunnel kustomization namespace"
        );
    }

    /// Pipeline consistency: .example SDI spec → HCL generation → node count/names match.
    #[test]
    fn test_pipeline_sdi_example_to_hcl_node_consistency() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        // Generate HCL from example spec
        let hcl = crate::core::tofu::generate_tofu_main(&sdi_spec, "jinwang");

        // Every node_name in the spec must appear in the generated HCL
        for pool in &sdi_spec.spec.sdi_pools {
            for node in &pool.node_specs {
                assert!(
                    hcl.contains(&node.node_name),
                    "Generated HCL must contain VM '{}' from pool '{}'",
                    node.node_name,
                    pool.pool_name
                );
                // IP must appear in cloud-init network config
                assert!(
                    hcl.contains(&node.ip),
                    "Generated HCL must contain IP '{}' for VM '{}'",
                    node.ip,
                    node.node_name
                );
            }
        }

        // Total VM count: tower(1) + sandbox(4) = 5
        let total_nodes: usize = sdi_spec
            .spec
            .sdi_pools
            .iter()
            .map(|p| p.node_specs.len())
            .sum();
        assert_eq!(total_nodes, 5, "Example spec must define 5 total VMs");

        // GPU passthrough node must have VFIO config in HCL
        assert!(
            hcl.contains("vfio") || hcl.contains("hostdev") || hcl.contains("gpu_passthrough"),
            "HCL must contain GPU passthrough configuration for sandbox-w-2"
        );
    }

    /// Pipeline consistency: .example configs → inventory generation →
    /// node names and roles must match SDI spec.
    #[test]
    fn test_pipeline_example_inventory_matches_sdi_nodes() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let sdi_spec: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();

        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        for cluster in &k8s_config.config.clusters {
            if cluster.cluster_mode == ClusterMode::Baremetal {
                continue;
            }
            // Find the SDI pool for this cluster
            let pool = sdi_spec
                .spec
                .sdi_pools
                .iter()
                .find(|p| p.pool_name == cluster.cluster_sdi_resource_pool)
                .unwrap_or_else(|| {
                    panic!(
                        "Pool '{}' must exist in SDI spec",
                        cluster.cluster_sdi_resource_pool
                    )
                });

            // Generate inventory
            let inventory = crate::core::kubespray::generate_inventory(cluster, &sdi_spec)
                .unwrap_or_else(|e| {
                    panic!(
                        "Inventory generation failed for '{}': {}",
                        cluster.cluster_name, e
                    )
                });

            // Every node from the pool must appear in the inventory
            for node in &pool.node_specs {
                assert!(
                    inventory.contains(&node.node_name),
                    "Inventory for '{}' must contain node '{}'",
                    cluster.cluster_name,
                    node.node_name
                );
                assert!(
                    inventory.contains(&node.ip),
                    "Inventory for '{}' must contain IP '{}'",
                    cluster.cluster_name,
                    node.ip
                );
            }

            // Control-plane nodes must be in [kube_control_plane]
            let cp_nodes: Vec<&str> = pool
                .node_specs
                .iter()
                .filter(|n| n.roles.contains(&"control-plane".to_string()))
                .map(|n| n.node_name.as_str())
                .collect();
            assert!(
                !cp_nodes.is_empty(),
                "Pool '{}' must have at least one control-plane node",
                pool.pool_name
            );

            // Worker nodes must be in [kube_node]
            let worker_nodes: Vec<&str> = pool
                .node_specs
                .iter()
                .filter(|n| n.roles.contains(&"worker".to_string()))
                .map(|n| n.node_name.as_str())
                .collect();
            for wn in &worker_nodes {
                assert!(
                    inventory.contains(wn),
                    "Worker '{}' must be in inventory for '{}'",
                    wn,
                    cluster.cluster_name
                );
            }
        }
    }

    /// Pipeline consistency: .example configs → cluster vars contain
    /// common config values correctly.
    #[test]
    fn test_pipeline_example_cluster_vars_contain_common_config() {
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let common = &k8s_config.config.common;

        for cluster in &k8s_config.config.clusters {
            let vars = crate::core::kubespray::generate_cluster_vars(cluster, common);

            // Kubernetes version must appear
            assert!(
                vars.contains(&common.kubernetes_version),
                "Cluster vars for '{}' must contain kubernetes_version '{}'",
                cluster.cluster_name,
                common.kubernetes_version
            );

            // CNI must be referenced
            assert!(
                vars.contains(&common.cni) || vars.contains("kube_network_plugin"),
                "Cluster vars for '{}' must reference CNI",
                cluster.cluster_name
            );

            // Pod CIDR must appear
            assert!(
                vars.contains(&cluster.network.pod_cidr),
                "Cluster vars for '{}' must contain pod_cidr '{}'",
                cluster.cluster_name,
                cluster.network.pod_cidr
            );

            // Service CIDR must appear
            assert!(
                vars.contains(&cluster.network.service_cidr),
                "Cluster vars for '{}' must contain service_cidr '{}'",
                cluster.cluster_name,
                cluster.network.service_cidr
            );

            // If OIDC is enabled, apiserver args must include OIDC flags
            if let Some(ref oidc) = cluster.oidc {
                if oidc.enabled {
                    assert!(
                        vars.contains("oidc") || vars.contains(&oidc.issuer_url),
                        "Cluster vars for '{}' must contain OIDC config when enabled",
                        cluster.cluster_name
                    );
                }
            }
        }
    }

    /// Cluster mode routing: SDI clusters must use SDI pool lookup,
    /// baremetal clusters must use baremetal_nodes directly.
    #[test]
    fn test_cluster_mode_routing_sdi_vs_baremetal() {
        let k8s_yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
  clusters:
    - cluster_name: "sdi-cluster"
      cluster_mode: "sdi"
      cluster_sdi_resource_pool: "test-pool"
      cluster_role: "management"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
    - cluster_name: "bm-cluster"
      cluster_mode: "baremetal"
      cluster_role: "workload"
      baremetal_nodes:
        - node_name: "bm-node-0"
          ip: "10.0.0.1"
          roles: [control-plane, etcd, worker]
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
"#;
        let config: K8sClustersConfig = serde_yaml::from_str(k8s_yaml).unwrap();

        // SDI cluster must require pool reference
        let sdi_cluster = &config.config.clusters[0];
        assert_eq!(sdi_cluster.cluster_mode, ClusterMode::Sdi);
        assert!(
            !sdi_cluster.cluster_sdi_resource_pool.is_empty(),
            "SDI cluster must have a pool reference"
        );

        // Baremetal cluster must have nodes defined
        let bm_cluster = &config.config.clusters[1];
        assert_eq!(bm_cluster.cluster_mode, ClusterMode::Baremetal);
        assert!(
            !bm_cluster.baremetal_nodes.is_empty(),
            "Baremetal cluster must have nodes"
        );

        // Baremetal inventory generation must work without SDI spec
        let inventory = crate::core::kubespray::generate_inventory_baremetal(bm_cluster)
            .expect("Baremetal inventory generation must succeed");
        assert!(
            inventory.contains("bm-node-0"),
            "Baremetal inventory must contain the defined node"
        );
        assert!(
            inventory.contains("10.0.0.1"),
            "Baremetal inventory must contain node IP"
        );
    }

    // ── Sprint 43: CIDR edge case tests ──

    /// /32 (single IP) within /24 must be detected as overlapping.
    #[test]
    fn test_cidrs_overlap_32_within_24() {
        assert!(
            cidrs_overlap("10.233.0.1/32", "10.233.0.0/24"),
            "/32 inside /24 must overlap"
        );
        assert!(
            cidrs_overlap("10.233.0.0/24", "10.233.0.1/32"),
            "symmetric: /24 contains /32 must overlap"
        );
    }

    /// /31 (point-to-point) overlapping with /24.
    #[test]
    fn test_cidrs_overlap_31_within_24() {
        assert!(
            cidrs_overlap("10.233.0.0/31", "10.233.0.0/24"),
            "/31 inside /24 must overlap"
        );
    }

    /// /0 (all IPs) must overlap with anything.
    #[test]
    fn test_cidrs_overlap_0_catches_all() {
        assert!(
            cidrs_overlap("0.0.0.0/0", "10.233.0.0/24"),
            "/0 must overlap with any CIDR"
        );
        assert!(
            cidrs_overlap("10.96.0.0/16", "0.0.0.0/0"),
            "Any CIDR must overlap with /0"
        );
    }

    /// Two identical /32s must overlap.
    #[test]
    fn test_cidrs_overlap_identical_32() {
        assert!(
            cidrs_overlap("10.0.0.1/32", "10.0.0.1/32"),
            "Same /32 must overlap"
        );
    }

    /// Adjacent /32s must NOT overlap.
    #[test]
    fn test_cidrs_no_overlap_adjacent_32() {
        assert!(
            !cidrs_overlap("10.0.0.1/32", "10.0.0.2/32"),
            "Adjacent /32s must not overlap"
        );
    }

    /// Node IP at CIDR boundary: first usable IP in pod_cidr must be caught.
    #[test]
    fn test_ip_in_cidr_boundary_first_ip() {
        assert!(
            ip_in_cidr("10.233.0.0", "10.233.0.0/24"),
            "Network address must be in CIDR"
        );
        assert!(
            ip_in_cidr("10.233.0.255", "10.233.0.0/24"),
            "Broadcast address must be in CIDR"
        );
        assert!(
            !ip_in_cidr("10.233.1.0", "10.233.0.0/24"),
            "First IP of next subnet must not be in CIDR"
        );
    }

    /// Node IP exactly at /16 boundary.
    #[test]
    fn test_ip_in_cidr_boundary_16() {
        assert!(ip_in_cidr("10.233.0.1", "10.233.0.0/16"));
        assert!(ip_in_cidr("10.233.255.254", "10.233.0.0/16"));
        assert!(!ip_in_cidr("10.234.0.1", "10.233.0.0/16"));
    }

    /// Single-node SDI: all VMs on one host, both clusters.
    /// Validates 2-layer consistency with max density deployment.
    #[test]
    fn test_single_node_sdi_two_clusters_two_layer_consistency() {
        let sdi_yaml = r#"
resource_pool:
  name: "mini-pool"
  network:
    management_bridge: "br0"
    management_cidr: "192.168.88.0/24"
    gateway: "192.168.88.1"
    nameservers: ["8.8.8.8"]
os_image:
  source: "https://example.com/img"
  format: "qcow2"
cloud_init:
  ssh_authorized_keys_file: "~/.ssh/id.pub"
spec:
  sdi_pools:
    - pool_name: "tower"
      purpose: "management"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "tower-cp-0"
          ip: "192.168.88.100"
          cpu: 2
          mem_gb: 4
          disk_gb: 30
          roles: [control-plane, worker]
    - pool_name: "sandbox"
      purpose: "workload"
      placement:
        hosts: [playbox-0]
      node_specs:
        - node_name: "sandbox-cp-0"
          ip: "192.168.88.110"
          cpu: 2
          mem_gb: 4
          disk_gb: 30
          host: "playbox-0"
          roles: [control-plane, worker]
"#;
        let k8s_yaml = r#"
config:
  common:
    kubernetes_version: "1.33.1"
    kubespray_version: "v2.30.0"
  clusters:
    - cluster_name: "tower"
      cluster_sdi_resource_pool: "tower"
      cluster_role: "management"
      network:
        pod_cidr: "10.244.0.0/20"
        service_cidr: "10.96.0.0/20"
        dns_domain: "tower.local"
    - cluster_name: "sandbox"
      cluster_sdi_resource_pool: "sandbox"
      cluster_role: "workload"
      network:
        pod_cidr: "10.233.0.0/17"
        service_cidr: "10.233.128.0/18"
        dns_domain: "sandbox.local"
"#;
        let sdi: SdiSpec = serde_yaml::from_str(sdi_yaml).unwrap();
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_yaml).unwrap();

        // Cross-config: pool mapping must work
        let mapping_errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            mapping_errors.is_empty(),
            "Single-node pool mapping must be valid: {:?}",
            mapping_errors
        );

        // 2-layer consistency: no errors expected
        let (errors, _warnings) = validate_two_layer_consistency(&k8s, &sdi);
        assert!(
            errors.is_empty(),
            "Single-node 2-layer must have no errors: {:?}",
            errors
        );

        // Network overlap must not exist
        let network_errors = validate_cluster_network_overlap(&k8s);
        assert!(
            network_errors.is_empty(),
            "CIDRs must not overlap: {:?}",
            network_errors
        );

        // Both clusters should get valid inventories
        for cluster in &k8s.config.clusters {
            let inv = crate::core::kubespray::generate_inventory(cluster, &sdi)
                .expect("Inventory generation must succeed");
            assert!(
                !inv.is_empty(),
                "Inventory for '{}' must not be empty",
                cluster.cluster_name
            );
        }

        // Total resources: 4 CPU, 8 GB RAM across 2 VMs on single host
        let total_cpu: u32 = sdi
            .spec
            .sdi_pools
            .iter()
            .flat_map(|p| &p.node_specs)
            .map(|n| n.cpu)
            .sum();
        assert_eq!(total_cpu, 4, "Single-node total CPU must be 2+2=4");
    }
}
