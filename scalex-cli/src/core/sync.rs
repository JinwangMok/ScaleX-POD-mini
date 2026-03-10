use crate::models::sdi::SdiPoolState;
use std::collections::HashSet;

/// Result of computing the diff between desired and current node sets.
#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub struct SyncDiff {
    pub to_add: Vec<String>,
    pub to_remove: Vec<String>,
    pub unchanged: Vec<String>,
}

/// Compute the diff between desired node names and current node names.
/// Pure function: no IO, no side effects.
#[allow(dead_code)]
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
#[allow(dead_code)]
pub struct VmConflict {
    pub vm_name: String,
    pub pool_name: String,
    pub host: String,
}

/// Detect VMs hosted on nodes that are about to be removed.
/// Pure function: takes pool state and removal list, returns conflicts.
#[allow(dead_code)]
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
}
