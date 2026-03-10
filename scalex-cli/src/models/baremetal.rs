use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeFacts {
    pub node_name: String,
    pub timestamp: String,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disks: Vec<DiskInfo>,
    pub nics: Vec<NicInfo>,
    pub gpus: Vec<GpuInfo>,
    pub iommu_groups: Vec<IommuGroup>,
    pub kernel: KernelInfo,
    pub bridges: Vec<String>,
    pub bonds: Vec<String>,
    pub pcie: Vec<PcieDevice>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CpuInfo {
    pub model: String,
    pub cores: u32,
    pub threads: u32,
    pub architecture: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_mb: u64,
    pub available_mb: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiskInfo {
    pub name: String,
    pub size_gb: u64,
    pub disk_type: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NicInfo {
    pub name: String,
    pub mac: String,
    pub speed: String,
    pub driver: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpuInfo {
    pub pci_id: String,
    pub model: String,
    pub vendor: String,
    pub driver: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IommuGroup {
    pub id: u32,
    pub devices: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KernelInfo {
    pub version: String,
    pub params: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PcieDevice {
    pub id: String,
    pub class: String,
    pub vendor: String,
    pub device: String,
}
