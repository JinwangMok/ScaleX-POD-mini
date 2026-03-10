use serde::{Deserialize, Serialize};

/// Top-level SDI specification file structure
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdiSpec {
    pub resource_pool: ResourcePoolConfig,
    pub os_image: OsImageConfig,
    pub cloud_init: CloudInitConfig,
    pub spec: SdiPoolsSpec,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourcePoolConfig {
    pub name: String,
    pub network: NetworkConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub management_bridge: String,
    pub management_cidr: String,
    pub gateway: String,
    pub nameservers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OsImageConfig {
    pub source: String,
    pub format: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudInitConfig {
    pub ssh_authorized_keys_file: String,
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdiPoolsSpec {
    pub sdi_pools: Vec<SdiPool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdiPool {
    pub pool_name: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub placement: PlacementConfig,
    pub node_specs: Vec<NodeSpec>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlacementConfig {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub spread: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeSpec {
    pub node_name: String,
    pub ip: String,
    pub cpu: u32,
    pub mem_gb: u32,
    pub disk_gb: u32,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub devices: Option<DeviceConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceConfig {
    #[serde(default)]
    pub gpu_passthrough: bool,
}

/// Runtime state of an SDI pool (after creation)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdiPoolState {
    pub pool_name: String,
    pub purpose: String,
    pub nodes: Vec<SdiNodeState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdiNodeState {
    pub node_name: String,
    pub ip: String,
    pub host: String,
    pub cpu: u32,
    pub mem_gb: u32,
    pub disk_gb: u32,
    pub status: String,
    pub gpu_passthrough: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sdi_spec() {
        let yaml = r#"
resource_pool:
  name: "test-pool"
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
        spread: true
      node_specs:
        - node_name: "sandbox-w-0"
          ip: "192.168.88.120"
          cpu: 8
          mem_gb: 16
          disk_gb: 100
          host: "playbox-1"
          roles: [worker]
          devices:
            gpu_passthrough: true
"#;
        let spec: SdiSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.resource_pool.name, "test-pool");
        assert_eq!(spec.spec.sdi_pools.len(), 2);
        assert_eq!(spec.spec.sdi_pools[0].pool_name, "tower");
        assert_eq!(spec.spec.sdi_pools[0].node_specs[0].cpu, 2);
        assert_eq!(
            spec.spec.sdi_pools[1].node_specs[0].host,
            Some("playbox-1".to_string())
        );
        assert!(
            spec.spec.sdi_pools[1].node_specs[0]
                .devices
                .as_ref()
                .unwrap()
                .gpu_passthrough
        );
    }
}
