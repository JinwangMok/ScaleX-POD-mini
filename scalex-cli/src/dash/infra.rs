use crate::models::sdi::{SdiNodeState, SdiPoolState};
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Default)]
pub struct InfraSnapshot {
    pub sdi_pools: Vec<SdiPoolInfo>,
    pub total_vms: usize,
    pub running_vms: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SdiPoolInfo {
    pub pool_name: String,
    pub purpose: String,
    pub nodes: Vec<SdiVmInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SdiVmInfo {
    pub name: String,
    pub ip: String,
    pub host: String,
    pub cpu: u32,
    pub mem_gb: u32,
    pub disk_gb: u32,
    pub status: String,
    pub gpu: bool,
}

/// Load SDI pool state from _generated/sdi/ directory.
/// Returns empty InfraSnapshot if directory doesn't exist or has no data.
pub fn load_sdi_state(sdi_dir: &Path) -> InfraSnapshot {
    let pools = match read_sdi_pools(sdi_dir) {
        Ok(p) => p,
        Err(_) => return InfraSnapshot::default(),
    };

    let sdi_pools: Vec<SdiPoolInfo> = pools
        .iter()
        .map(|pool| SdiPoolInfo {
            pool_name: pool.pool_name.clone(),
            purpose: pool.purpose.clone(),
            nodes: pool.nodes.iter().map(node_state_to_vm_info).collect(),
        })
        .collect();

    let total_vms: usize = sdi_pools.iter().map(|p| p.nodes.len()).sum();
    let running_vms: usize = sdi_pools
        .iter()
        .flat_map(|p| &p.nodes)
        .filter(|vm| vm.status == "running")
        .count();

    InfraSnapshot {
        sdi_pools,
        total_vms,
        running_vms,
    }
}

fn node_state_to_vm_info(node: &SdiNodeState) -> SdiVmInfo {
    SdiVmInfo {
        name: node.node_name.clone(),
        ip: node.ip.clone(),
        host: node.host.clone(),
        cpu: node.cpu,
        mem_gb: node.mem_gb,
        disk_gb: node.disk_gb,
        status: node.status.clone(),
        gpu: node.gpu_passthrough,
    }
}

fn read_sdi_pools(sdi_dir: &Path) -> Result<Vec<SdiPoolState>> {
    if !sdi_dir.exists() {
        return Ok(Vec::new());
    }

    let mut pools = Vec::new();
    let entries = std::fs::read_dir(sdi_dir)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                // Try parsing as single pool or array of pools
                if let Ok(pool) = serde_json::from_str::<SdiPoolState>(&content) {
                    pools.push(pool);
                } else if let Ok(mut pool_list) =
                    serde_json::from_str::<Vec<SdiPoolState>>(&content)
                {
                    pools.append(&mut pool_list);
                }
            }
        }
    }

    pools.sort_by(|a, b| a.pool_name.cmp(&b.pool_name));
    Ok(pools)
}

/// Build VM-to-cluster-node mapping: which SDI VM corresponds to which K8s node.
#[allow(dead_code)]
pub fn build_vm_node_map(infra: &InfraSnapshot) -> Vec<(String, String)> {
    infra
        .sdi_pools
        .iter()
        .flat_map(|pool| {
            pool.nodes
                .iter()
                .map(move |vm| (vm.name.clone(), pool.pool_name.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_sdi_state_returns_default_for_missing_dir() {
        let snap = load_sdi_state(Path::new("/nonexistent"));
        assert!(snap.sdi_pools.is_empty());
        assert_eq!(snap.total_vms, 0);
    }

    #[test]
    fn load_sdi_state_returns_default_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let snap = load_sdi_state(dir.path());
        assert!(snap.sdi_pools.is_empty());
    }

    #[test]
    fn load_sdi_state_parses_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{
            "pool_name": "tower",
            "purpose": "management",
            "nodes": [{
                "node_name": "tower-cp-0",
                "ip": "10.0.0.100",
                "host": "node-0",
                "cpu": 2,
                "mem_gb": 4,
                "disk_gb": 30,
                "status": "running",
                "gpu_passthrough": false
            }]
        }"#;
        std::fs::write(dir.path().join("tower.json"), json).unwrap();

        let snap = load_sdi_state(dir.path());
        assert_eq!(snap.sdi_pools.len(), 1);
        assert_eq!(snap.sdi_pools[0].pool_name, "tower");
        assert_eq!(snap.total_vms, 1);
        assert_eq!(snap.running_vms, 1);
    }

    #[test]
    fn build_vm_node_map_works() {
        let infra = InfraSnapshot {
            sdi_pools: vec![SdiPoolInfo {
                pool_name: "tower".into(),
                purpose: "mgmt".into(),
                nodes: vec![SdiVmInfo {
                    name: "tower-cp-0".into(),
                    ip: "10.0.0.100".into(),
                    host: "node-0".into(),
                    cpu: 2,
                    mem_gb: 4,
                    disk_gb: 30,
                    status: "running".into(),
                    gpu: false,
                }],
            }],
            total_vms: 1,
            running_vms: 1,
        };
        let map = build_vm_node_map(&infra);
        assert_eq!(map, vec![("tower-cp-0".into(), "tower".into())]);
    }
}
