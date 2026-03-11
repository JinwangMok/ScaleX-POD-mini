use crate::models::sdi::SdiPoolState;
use std::collections::HashSet;

/// Result of computing the diff between desired and current node sets.
#[derive(Debug, PartialEq)]
pub struct SyncDiff {
    pub to_add: Vec<String>,
    pub to_remove: Vec<String>,
    pub unchanged: Vec<String>,
}

/// Compute the diff between desired node names and current node names.
/// Pure function: no IO, no side effects.
pub fn compute_sync_diff(desired: &[String], current: &[String]) -> SyncDiff {
    let desired_set: HashSet<&str> = desired.iter().map(|s| s.as_str()).collect();
    let current_set: HashSet<&str> = current.iter().map(|s| s.as_str()).collect();

    let mut to_add: Vec<String> = desired_set
        .difference(&current_set)
        .map(|s| s.to_string())
        .collect();
    let mut to_remove: Vec<String> = current_set
        .difference(&desired_set)
        .map(|s| s.to_string())
        .collect();
    let mut unchanged: Vec<String> = desired_set
        .intersection(&current_set)
        .map(|s| s.to_string())
        .collect();

    to_add.sort();
    to_remove.sort();
    unchanged.sort();

    SyncDiff {
        to_add,
        to_remove,
        unchanged,
    }
}

/// A VM that would be affected by removing its host node.
#[derive(Debug, PartialEq)]
pub struct VmConflict {
    pub vm_name: String,
    pub pool_name: String,
    pub host: String,
}

/// Detect VMs hosted on nodes that are about to be removed.
/// Pure function: takes pool state and removal list, returns conflicts.
pub fn detect_vm_conflicts(pools: &[SdiPoolState], nodes_to_remove: &[String]) -> Vec<VmConflict> {
    let remove_set: HashSet<&str> = nodes_to_remove.iter().map(|s| s.as_str()).collect();
    let mut conflicts = Vec::new();

    for pool in pools {
        for node in &pool.nodes {
            if remove_set.contains(node.host.as_str()) {
                conflicts.push(VmConflict {
                    vm_name: node.node_name.clone(),
                    pool_name: pool.pool_name.clone(),
                    host: node.host.clone(),
                });
            }
        }
    }

    conflicts.sort_by(|a, b| a.vm_name.cmp(&b.vm_name));
    conflicts
}

/// Severity level for a VM conflict when removing a host node.
#[derive(Debug, PartialEq, Clone)]
pub enum ConflictSeverity {
    /// Removing management cluster VM — blocks entire platform management
    Critical,
    /// Removing control-plane VM — cluster loses quorum/availability
    High,
    /// Removing worker VM — workloads disrupted but cluster survives
    Medium,
}

/// Classify the severity of a VM conflict based on pool purpose and VM roles.
/// Pure function: no I/O, no side effects.
pub fn classify_conflict_severity(
    conflict: &VmConflict,
    pools: &[SdiPoolState],
) -> ConflictSeverity {
    // Find the pool this VM belongs to
    let pool = pools.iter().find(|p| p.pool_name == conflict.pool_name);

    match pool {
        Some(p) if p.purpose == "management" => ConflictSeverity::Critical,
        _ => {
            // Check if VM name suggests control-plane role (convention: contains "cp")
            if conflict.vm_name.contains("-cp-") || conflict.vm_name.contains("-master-") {
                ConflictSeverity::High
            } else {
                ConflictSeverity::Medium
            }
        }
    }
}

/// Validate that it is safe to remove nodes when SDI state file is absent.
/// If state is missing AND nodes are being removed, we cannot check for VM conflicts —
/// return warnings so the caller can block or alert.
/// Pure function: no I/O, no side effects.
pub fn validate_removal_safety(has_state: bool, nodes_to_remove: &[String]) -> Vec<String> {
    if has_state || nodes_to_remove.is_empty() {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    warnings.push(format!(
        "SDI state file not found but {} node(s) scheduled for removal: {}. \
         Cannot verify whether these hosts have active VMs. \
         Run `scalex sdi init` first to establish state, or use `--force` to proceed at your own risk.",
        nodes_to_remove.len(),
        nodes_to_remove.join(", ")
    ));
    warnings
}

/// Check if any conflicts would affect the management cluster.
/// Returns true if removing nodes would destroy the management plane — this should be a hard block.
/// Pure function: no I/O, no side effects.
pub fn has_management_cluster_conflict(conflicts: &[VmConflict], pools: &[SdiPoolState]) -> bool {
    conflicts
        .iter()
        .any(|c| classify_conflict_severity(c, pools) == ConflictSeverity::Critical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::sdi::{SdiNodeState, SdiPoolState};

    #[test]
    fn test_compute_sync_diff_add_node() {
        let desired = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
        ];
        let current = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let diff = compute_sync_diff(&desired, &current);
        assert_eq!(diff.to_add, vec!["playbox-2"]);
        assert!(diff.to_remove.is_empty());
        assert_eq!(diff.unchanged, vec!["playbox-0", "playbox-1"]);
    }

    #[test]
    fn test_compute_sync_diff_remove_node() {
        let desired = vec!["playbox-0".to_string()];
        let current = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
        ];

        let diff = compute_sync_diff(&desired, &current);
        assert!(diff.to_add.is_empty());
        assert_eq!(diff.to_remove, vec!["playbox-1", "playbox-2"]);
        assert_eq!(diff.unchanged, vec!["playbox-0"]);
    }

    #[test]
    fn test_compute_sync_diff_no_change() {
        let desired = vec!["playbox-0".to_string(), "playbox-1".to_string()];
        let current = vec!["playbox-0".to_string(), "playbox-1".to_string()];

        let diff = compute_sync_diff(&desired, &current);
        assert!(diff.to_add.is_empty());
        assert!(diff.to_remove.is_empty());
        assert_eq!(diff.unchanged, vec!["playbox-0", "playbox-1"]);
    }

    #[test]
    fn test_compute_sync_diff_empty_current() {
        let desired = vec!["playbox-0".to_string(), "playbox-1".to_string()];
        let current: Vec<String> = vec![];

        let diff = compute_sync_diff(&desired, &current);
        assert_eq!(diff.to_add, vec!["playbox-0", "playbox-1"]);
        assert!(diff.to_remove.is_empty());
        assert!(diff.unchanged.is_empty());
    }

    #[test]
    fn test_detect_vm_conflicts_found() {
        let pools = vec![SdiPoolState {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            nodes: vec![
                SdiNodeState {
                    node_name: "sandbox-w-0".to_string(),
                    ip: "192.168.88.120".to_string(),
                    host: "playbox-1".to_string(),
                    cpu: 8,
                    mem_gb: 16,
                    disk_gb: 100,
                    status: "running".to_string(),
                    gpu_passthrough: false,
                },
                SdiNodeState {
                    node_name: "sandbox-w-1".to_string(),
                    ip: "192.168.88.121".to_string(),
                    host: "playbox-2".to_string(),
                    cpu: 8,
                    mem_gb: 16,
                    disk_gb: 100,
                    status: "running".to_string(),
                    gpu_passthrough: false,
                },
            ],
        }];

        let to_remove = vec!["playbox-1".to_string()];
        let conflicts = detect_vm_conflicts(&pools, &to_remove);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].vm_name, "sandbox-w-0");
        assert_eq!(conflicts[0].pool_name, "sandbox");
        assert_eq!(conflicts[0].host, "playbox-1");
    }

    #[test]
    fn test_detect_vm_conflicts_none() {
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
                status: "running".to_string(),
                gpu_passthrough: false,
            }],
        }];

        let to_remove = vec!["playbox-3".to_string()];
        let conflicts = detect_vm_conflicts(&pools, &to_remove);

        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_detect_vm_conflicts_multi_pool() {
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
                    status: "running".to_string(),
                    gpu_passthrough: false,
                }],
            },
            SdiPoolState {
                pool_name: "sandbox".to_string(),
                purpose: "workload".to_string(),
                nodes: vec![SdiNodeState {
                    node_name: "sandbox-w-0".to_string(),
                    ip: "192.168.88.120".to_string(),
                    host: "playbox-0".to_string(),
                    cpu: 8,
                    mem_gb: 16,
                    disk_gb: 100,
                    status: "running".to_string(),
                    gpu_passthrough: false,
                }],
            },
        ];

        let to_remove = vec!["playbox-0".to_string()];
        let conflicts = detect_vm_conflicts(&pools, &to_remove);

        // Both VMs on playbox-0 should be detected
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0].vm_name, "sandbox-w-0");
        assert_eq!(conflicts[1].vm_name, "tower-cp-0");
    }

    #[test]
    fn test_compute_sync_diff_simultaneous_add_and_remove() {
        let desired = vec![
            "playbox-0".to_string(),
            "playbox-3".to_string(),
            "playbox-4".to_string(),
        ];
        let current = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
        ];

        let diff = compute_sync_diff(&desired, &current);
        assert_eq!(diff.to_add, vec!["playbox-3", "playbox-4"]);
        assert_eq!(diff.to_remove, vec!["playbox-1", "playbox-2"]);
        assert_eq!(diff.unchanged, vec!["playbox-0"]);
    }

    #[test]
    fn test_compute_sync_diff_empty_desired_removes_all() {
        let desired: Vec<String> = vec![];
        let current = vec![
            "playbox-0".to_string(),
            "playbox-1".to_string(),
            "playbox-2".to_string(),
        ];

        let diff = compute_sync_diff(&desired, &current);
        assert!(diff.to_add.is_empty());
        assert_eq!(diff.to_remove, vec!["playbox-0", "playbox-1", "playbox-2"]);
        assert!(diff.unchanged.is_empty());
    }

    #[test]
    fn test_compute_sync_diff_both_empty() {
        let desired: Vec<String> = vec![];
        let current: Vec<String> = vec![];

        let diff = compute_sync_diff(&desired, &current);
        assert!(diff.to_add.is_empty());
        assert!(diff.to_remove.is_empty());
        assert!(diff.unchanged.is_empty());
    }

    #[test]
    fn test_compute_sync_diff_complete_replacement() {
        // All nodes replaced — no overlap
        let desired = vec!["new-0".to_string(), "new-1".to_string()];
        let current = vec!["old-0".to_string(), "old-1".to_string()];

        let diff = compute_sync_diff(&desired, &current);
        assert_eq!(diff.to_add, vec!["new-0", "new-1"]);
        assert_eq!(diff.to_remove, vec!["old-0", "old-1"]);
        assert!(diff.unchanged.is_empty());
    }

    #[test]
    fn test_detect_vm_conflicts_removing_multiple_hosts() {
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
                    status: "running".to_string(),
                    gpu_passthrough: false,
                }],
            },
            SdiPoolState {
                pool_name: "sandbox".to_string(),
                purpose: "workload".to_string(),
                nodes: vec![
                    SdiNodeState {
                        node_name: "sandbox-w-0".to_string(),
                        ip: "192.168.88.120".to_string(),
                        host: "playbox-1".to_string(),
                        cpu: 8,
                        mem_gb: 16,
                        disk_gb: 100,
                        status: "running".to_string(),
                        gpu_passthrough: false,
                    },
                    SdiNodeState {
                        node_name: "sandbox-w-1".to_string(),
                        ip: "192.168.88.121".to_string(),
                        host: "playbox-2".to_string(),
                        cpu: 8,
                        mem_gb: 16,
                        disk_gb: 100,
                        status: "running".to_string(),
                        gpu_passthrough: false,
                    },
                ],
            },
        ];

        // Removing playbox-0 AND playbox-2 should catch tower-cp-0 + sandbox-w-1
        let to_remove = vec!["playbox-0".to_string(), "playbox-2".to_string()];
        let conflicts = detect_vm_conflicts(&pools, &to_remove);

        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0].vm_name, "sandbox-w-1");
        assert_eq!(conflicts[0].pool_name, "sandbox");
        assert_eq!(conflicts[1].vm_name, "tower-cp-0");
        assert_eq!(conflicts[1].pool_name, "tower");
    }

    // --- classify_conflict_severity ---

    #[test]
    fn test_classify_severity_management_pool_is_critical() {
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
                status: "running".to_string(),
                gpu_passthrough: false,
            }],
        }];

        let conflict = VmConflict {
            vm_name: "tower-cp-0".to_string(),
            pool_name: "tower".to_string(),
            host: "playbox-0".to_string(),
        };

        assert_eq!(
            classify_conflict_severity(&conflict, &pools),
            ConflictSeverity::Critical,
            "Management pool VM removal must be CRITICAL"
        );
    }

    #[test]
    fn test_classify_severity_control_plane_is_high() {
        let pools = vec![SdiPoolState {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            nodes: vec![SdiNodeState {
                node_name: "sandbox-cp-0".to_string(),
                ip: "192.168.88.110".to_string(),
                host: "playbox-0".to_string(),
                cpu: 4,
                mem_gb: 8,
                disk_gb: 60,
                status: "running".to_string(),
                gpu_passthrough: false,
            }],
        }];

        let conflict = VmConflict {
            vm_name: "sandbox-cp-0".to_string(),
            pool_name: "sandbox".to_string(),
            host: "playbox-0".to_string(),
        };

        assert_eq!(
            classify_conflict_severity(&conflict, &pools),
            ConflictSeverity::High,
            "Control-plane VM (contains -cp-) must be HIGH severity"
        );
    }

    #[test]
    fn test_classify_severity_worker_is_medium() {
        let pools = vec![SdiPoolState {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            nodes: vec![SdiNodeState {
                node_name: "sandbox-w-0".to_string(),
                ip: "192.168.88.120".to_string(),
                host: "playbox-1".to_string(),
                cpu: 8,
                mem_gb: 16,
                disk_gb: 100,
                status: "running".to_string(),
                gpu_passthrough: false,
            }],
        }];

        let conflict = VmConflict {
            vm_name: "sandbox-w-0".to_string(),
            pool_name: "sandbox".to_string(),
            host: "playbox-1".to_string(),
        };

        assert_eq!(
            classify_conflict_severity(&conflict, &pools),
            ConflictSeverity::Medium,
            "Worker VM must be MEDIUM severity"
        );
    }

    // --- has_management_cluster_conflict ---

    #[test]
    fn test_has_management_conflict_true() {
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
                    status: "running".to_string(),
                    gpu_passthrough: false,
                }],
            },
            SdiPoolState {
                pool_name: "sandbox".to_string(),
                purpose: "workload".to_string(),
                nodes: vec![SdiNodeState {
                    node_name: "sandbox-w-0".to_string(),
                    ip: "192.168.88.120".to_string(),
                    host: "playbox-1".to_string(),
                    cpu: 8,
                    mem_gb: 16,
                    disk_gb: 100,
                    status: "running".to_string(),
                    gpu_passthrough: false,
                }],
            },
        ];

        let conflicts = vec![VmConflict {
            vm_name: "tower-cp-0".to_string(),
            pool_name: "tower".to_string(),
            host: "playbox-0".to_string(),
        }];

        assert!(
            has_management_cluster_conflict(&conflicts, &pools),
            "Must detect management cluster conflict"
        );
    }

    #[test]
    fn test_has_management_conflict_false_workers_only() {
        let pools = vec![SdiPoolState {
            pool_name: "sandbox".to_string(),
            purpose: "workload".to_string(),
            nodes: vec![SdiNodeState {
                node_name: "sandbox-w-0".to_string(),
                ip: "192.168.88.120".to_string(),
                host: "playbox-1".to_string(),
                cpu: 8,
                mem_gb: 16,
                disk_gb: 100,
                status: "running".to_string(),
                gpu_passthrough: false,
            }],
        }];

        let conflicts = vec![VmConflict {
            vm_name: "sandbox-w-0".to_string(),
            pool_name: "sandbox".to_string(),
            host: "playbox-1".to_string(),
        }];

        assert!(
            !has_management_cluster_conflict(&conflicts, &pools),
            "Worker-only conflicts must NOT be flagged as management conflict"
        );
    }

    #[test]
    fn test_has_management_conflict_empty_conflicts() {
        let pools = vec![SdiPoolState {
            pool_name: "tower".to_string(),
            purpose: "management".to_string(),
            nodes: vec![],
        }];

        let conflicts: Vec<VmConflict> = vec![];
        assert!(
            !has_management_cluster_conflict(&conflicts, &pools),
            "Empty conflicts must return false"
        );
    }

    // --- validate_removal_safety ---

    #[test]
    fn test_removal_safety_no_state_with_removals_warns() {
        let to_remove = vec!["playbox-1".to_string(), "playbox-2".to_string()];
        let warnings = validate_removal_safety(false, &to_remove);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("SDI state file not found"),
            "Must warn about missing state — got: {}",
            warnings[0]
        );
        assert!(
            warnings[0].contains("playbox-1"),
            "Must list affected nodes"
        );
        assert!(
            warnings[0].contains("playbox-2"),
            "Must list affected nodes"
        );
    }

    #[test]
    fn test_removal_safety_has_state_no_warning() {
        let to_remove = vec!["playbox-1".to_string()];
        let warnings = validate_removal_safety(true, &to_remove);
        assert!(
            warnings.is_empty(),
            "When state file exists, no safety warning needed — conflict detection handles it"
        );
    }

    #[test]
    fn test_removal_safety_no_state_empty_removals_no_warning() {
        let to_remove: Vec<String> = vec![];
        let warnings = validate_removal_safety(false, &to_remove);
        assert!(
            warnings.is_empty(),
            "No removals = no risk, even without state file"
        );
    }

    #[test]
    fn test_removal_safety_has_state_empty_removals_no_warning() {
        let to_remove: Vec<String> = vec![];
        let warnings = validate_removal_safety(true, &to_remove);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_detect_vm_conflicts_empty_removal_list() {
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
                status: "running".to_string(),
                gpu_passthrough: false,
            }],
        }];

        let to_remove: Vec<String> = vec![];
        let conflicts = detect_vm_conflicts(&pools, &to_remove);
        assert!(conflicts.is_empty());
    }
}
