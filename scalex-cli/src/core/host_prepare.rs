use crate::core::config::NodeConnectionConfig;
use crate::models::baremetal::NodeFacts;

/// Generate the shell script to install KVM/libvirt on a host.
/// Pure function.
pub fn generate_kvm_install_script() -> String {
    r#"#!/bin/bash
set -euo pipefail

echo "[scalex] Installing KVM/libvirt packages..."
apt-get update -qq
apt-get install -y -qq \
    qemu-kvm \
    libvirt-daemon-system \
    libvirt-clients \
    virtinst \
    bridge-utils \
    cpu-checker \
    genisoimage

echo "[scalex] Enabling libvirtd..."
systemctl enable --now libvirtd

echo "[scalex] Verifying KVM support..."
kvm-ok || echo "WARNING: KVM acceleration may not be available"

echo "[scalex] KVM/libvirt installation complete."
"#
    .to_string()
}

/// Generate the shell script to set up br0 bridge on a host.
/// Pure function: takes NIC name and current IP config as input.
pub fn generate_bridge_setup_script(
    primary_nic: &str,
    ip_address: &str,
    gateway: &str,
    cidr_prefix: u8,
) -> String {
    format!(
        r#"#!/bin/bash
set -euo pipefail

echo "[scalex] Setting up br0 bridge on NIC: {primary_nic}"

# Backup existing netplan
cp /etc/netplan/*.yaml /etc/netplan/backup/ 2>/dev/null || mkdir -p /etc/netplan/backup && cp /etc/netplan/*.yaml /etc/netplan/backup/

cat > /etc/netplan/50-scalex-bridge.yaml << 'NETPLAN'
network:
  version: 2
  renderer: networkd
  ethernets:
    {primary_nic}:
      dhcp4: false
      dhcp6: false
  bridges:
    br0:
      interfaces: [{primary_nic}]
      addresses:
        - {ip_address}/{cidr_prefix}
      routes:
        - to: default
          via: {gateway}
      nameservers:
        addresses: [8.8.8.8, 8.8.4.4]
      parameters:
        stp: false
        forward-delay: 0
NETPLAN

echo "[scalex] Applying netplan with 120s timeout (auto-revert on failure)..."
netplan try --timeout 120

echo "[scalex] br0 bridge setup complete."
"#
    )
}

/// Generate the shell script to configure VFIO-PCI for GPU passthrough.
/// Pure function: takes GPU PCI IDs and IOMMU group info.
pub fn generate_vfio_setup_script(gpu_pci_ids: &[String]) -> String {
    if gpu_pci_ids.is_empty() {
        return "echo '[scalex] No GPUs specified for passthrough, skipping VFIO setup.'\n"
            .to_string();
    }

    let pci_ids_str = gpu_pci_ids.join(",");

    format!(
        r#"#!/bin/bash
set -euo pipefail

echo "[scalex] Configuring VFIO-PCI for GPU passthrough..."

# Ensure IOMMU is enabled in GRUB
if ! grep -q 'intel_iommu=on' /etc/default/grub; then
    echo "[scalex] Adding intel_iommu=on to GRUB..."
    sed -i 's/GRUB_CMDLINE_LINUX_DEFAULT="\(.*\)"/GRUB_CMDLINE_LINUX_DEFAULT="\1 intel_iommu=on iommu=pt"/' /etc/default/grub
    update-grub
    NEEDS_REBOOT=true
fi

# Load VFIO modules
cat > /etc/modules-load.d/vfio.conf << 'MODULES'
vfio
vfio_iommu_type1
vfio_pci
MODULES

# Blacklist GPU drivers so VFIO can claim devices
cat > /etc/modprobe.d/blacklist-gpu.conf << 'BLACKLIST'
blacklist nouveau
blacklist nvidia
blacklist nvidiafb
blacklist nvidia_drm
blacklist nvidia_modeset
blacklist amdgpu
blacklist radeon
BLACKLIST

# Bind specific PCI devices to vfio-pci
cat > /etc/modprobe.d/vfio-pci.conf << VFIO
options vfio-pci ids={pci_ids_str}
VFIO

echo "[scalex] Regenerating initramfs..."
update-initramfs -u

if [ "${{NEEDS_REBOOT:-false}}" = "true" ]; then
    echo "[scalex] REBOOT REQUIRED: IOMMU was enabled in GRUB. Reboot to activate."
else
    echo "[scalex] VFIO-PCI configuration complete. Reboot recommended."
fi
"#
    )
}

/// Determine which GPU PCI vendor:device IDs to pass through from node facts.
/// Pure function.
pub fn extract_gpu_pci_ids(facts: &NodeFacts) -> Vec<String> {
    facts
        .gpus
        .iter()
        .filter_map(|gpu| {
            // Extract PCI vendor:device ID from lspci output like:
            // "01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA106 [GeForce RTX 3060] [10de:2544]"
            let model = &gpu.model;
            if let Some(start) = model.rfind('[') {
                if let Some(end) = model.rfind(']') {
                    let id = &model[start + 1..end];
                    if id.contains(':') {
                        return Some(id.to_string());
                    }
                }
            }
            None
        })
        .collect()
}

/// Check if a host already has br0 configured.
/// Pure function: examines facts data.
pub fn has_bridge(facts: &NodeFacts) -> bool {
    facts.bridges.iter().any(|b| b == "br0")
}

/// Build the preparation plan for a single host.
/// Pure function: returns list of steps needed.
pub fn plan_host_preparation(
    _node: &NodeConnectionConfig,
    facts: &NodeFacts,
    needs_gpu_passthrough: bool,
) -> Vec<PrepStep> {
    let mut steps = Vec::new();

    // Always ensure KVM/libvirt
    steps.push(PrepStep::InstallKvm);

    // Bridge setup if not present
    if !has_bridge(facts) {
        steps.push(PrepStep::SetupBridge);
    }

    // VFIO if GPU passthrough needed
    if needs_gpu_passthrough && !facts.gpus.is_empty() {
        steps.push(PrepStep::ConfigureVfio);
    }

    steps
}

#[derive(Clone, Debug, PartialEq)]
pub enum PrepStep {
    InstallKvm,
    SetupBridge,
    ConfigureVfio,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::baremetal::*;
    use std::collections::HashMap;

    fn make_test_facts(has_br0: bool, has_gpu: bool) -> NodeFacts {
        NodeFacts {
            node_name: "test-node".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            cpu: CpuInfo {
                model: "Intel i7".to_string(),
                cores: 8,
                threads: 16,
                architecture: "x86_64".to_string(),
            },
            memory: MemoryInfo {
                total_mb: 32768,
                available_mb: 30000,
            },
            disks: vec![],
            nics: vec![],
            gpus: if has_gpu {
                vec![GpuInfo {
                    pci_id: "01:00.0".to_string(),
                    model: "01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA106 [10de:2544]".to_string(),
                    vendor: "nvidia".to_string(),
                    driver: String::new(),
                }]
            } else {
                vec![]
            },
            iommu_groups: vec![],
            kernel: KernelInfo {
                version: "6.8.0".to_string(),
                params: HashMap::new(),
            },
            bridges: if has_br0 {
                vec!["br0".to_string()]
            } else {
                vec![]
            },
            bonds: vec![],
            pcie: vec![],
        }
    }

    #[test]
    fn test_has_bridge() {
        assert!(has_bridge(&make_test_facts(true, false)));
        assert!(!has_bridge(&make_test_facts(false, false)));
    }

    #[test]
    fn test_extract_gpu_pci_ids() {
        let facts = make_test_facts(false, true);
        let ids = extract_gpu_pci_ids(&facts);
        assert_eq!(ids, vec!["10de:2544"]);
    }

    #[test]
    fn test_extract_gpu_pci_ids_empty() {
        let facts = make_test_facts(false, false);
        let ids = extract_gpu_pci_ids(&facts);
        assert!(ids.is_empty());
    }

    #[test]
    fn test_plan_host_no_bridge_no_gpu() {
        let facts = make_test_facts(false, false);
        let node = crate::core::config::NodeConnectionConfig {
            name: "n0".to_string(),
            direct_reachable: true,
            node_ip: "10.0.0.1".to_string(),
            reachable_node_ip: None,
            reachable_via: None,
            admin_user: "admin".to_string(),
            ssh_auth_mode: crate::core::config::SshAuthMode::Key,
            ssh_password: None,
            ssh_key_path: None,
            ssh_key_path_of_reachable_node: None,
        };
        let steps = plan_host_preparation(&node, &facts, false);
        assert_eq!(steps, vec![PrepStep::InstallKvm, PrepStep::SetupBridge]);
    }

    #[test]
    fn test_plan_host_with_bridge_and_gpu() {
        let facts = make_test_facts(true, true);
        let node = crate::core::config::NodeConnectionConfig {
            name: "n0".to_string(),
            direct_reachable: true,
            node_ip: "10.0.0.1".to_string(),
            reachable_node_ip: None,
            reachable_via: None,
            admin_user: "admin".to_string(),
            ssh_auth_mode: crate::core::config::SshAuthMode::Key,
            ssh_password: None,
            ssh_key_path: None,
            ssh_key_path_of_reachable_node: None,
        };
        let steps = plan_host_preparation(&node, &facts, true);
        assert_eq!(steps, vec![PrepStep::InstallKvm, PrepStep::ConfigureVfio]);
    }

    #[test]
    fn test_generate_kvm_script() {
        let script = generate_kvm_install_script();
        assert!(script.contains("qemu-kvm"));
        assert!(script.contains("libvirt-daemon-system"));
    }

    #[test]
    fn test_generate_bridge_script() {
        let script = generate_bridge_setup_script("eno1", "192.168.88.10", "192.168.88.1", 24);
        assert!(script.contains("br0"));
        assert!(script.contains("eno1"));
        assert!(script.contains("netplan try"));
    }

    #[test]
    fn test_generate_vfio_script() {
        let script = generate_vfio_setup_script(&["10de:2544".to_string()]);
        assert!(script.contains("vfio-pci"));
        assert!(script.contains("10de:2544"));
        assert!(script.contains("intel_iommu=on"));
    }
}
