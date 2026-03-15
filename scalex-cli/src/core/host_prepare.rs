use crate::core::config::NodeConnectionConfig;
use crate::models::baremetal::NodeFacts;

/// Generate the shell script to install KVM/libvirt on a host.
/// Pure function.
pub fn generate_kvm_install_script() -> String {
    r#"#!/bin/bash
set -euo pipefail

echo "[scalex] Installing KVM/libvirt packages..."
sudo apt-get update -qq
sudo apt-get install -y -qq \
    qemu-kvm \
    libvirt-daemon-system \
    libvirt-clients \
    virtinst \
    bridge-utils \
    cpu-checker \
    genisoimage

echo "[scalex] Enabling libvirtd..."
sudo systemctl enable --now libvirtd

echo "[scalex] Verifying KVM support..."
kvm-ok || echo "WARNING: KVM acceleration may not be available"

echo "[scalex] KVM/libvirt installation complete."
"#
    .to_string()
}

/// Generate the shell script to set up br0 bridge on a host.
/// Pure function: takes NIC name, IP config, and whether the NIC is a bond interface.
/// When `is_bond` is true, the netplan omits the ethernets section and strips the
/// IP from any existing bond netplan config so the bridge takes over addressing.
pub fn generate_bridge_setup_script(
    primary_nic: &str,
    ip_address: &str,
    gateway: &str,
    cidr_prefix: u8,
    is_bond: bool,
) -> String {
    let netplan_body = if is_bond {
        format!(
            r#"network:
  version: 2
  renderer: networkd
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
        forward-delay: 0"#
        )
    } else {
        format!(
            r#"network:
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
        forward-delay: 0"#
        )
    };

    let strip_bond_ip = if is_bond {
        format!(
            r#"
# Strip IP addresses/routes from existing bond netplan so br0 takes over
echo "[scalex] Removing IP config from {primary_nic} in existing netplan files..."
for f in /etc/netplan/*.yaml; do
    [ "$f" = "/etc/netplan/50-scalex-bridge.yaml" ] && continue
    if grep -q '{primary_nic}' "$f" 2>/dev/null; then
        sudo sed -i '/{primary_nic}/,/^[^ ]/ {{
            /addresses:/,/^[^ ]/{{ /addresses:/d; /^ *- /d; }}
            /routes:/,/^[^ ]/{{ /routes:/d; /^ *- /d; /^ *to:/d; /^ *via:/d; }}
            /gateway4:/d
            /dhcp4:/d
        }}' "$f"
    fi
done"#
        )
    } else {
        String::new()
    };

    format!(
        r#"#!/bin/bash
set -euo pipefail

echo "[scalex] Setting up br0 bridge on NIC: {primary_nic} (bond={is_bond})"

# Backup existing netplan
sudo mkdir -p /etc/netplan/backup
sudo cp /etc/netplan/*.yaml /etc/netplan/backup/ 2>/dev/null || true
{strip_bond_ip}

sudo tee /etc/netplan/50-scalex-bridge.yaml > /dev/null << 'NETPLAN'
{netplan_body}
NETPLAN

echo "[scalex] Applying netplan with 120s timeout (auto-revert on failure)..."
sudo netplan try --timeout 120

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

/// Generate the shell script to fully clean a node (remove K8s, KVM, bridge).
/// Used by `sdi clean --hard` to reset nodes to bare-metal state.
/// Pure function. Preserves SSH access and basic networking.
pub fn generate_node_cleanup_script() -> String {
    r#"#!/bin/bash
set -euo pipefail

echo "[scalex] === Node Cleanup: Resetting to bare-metal state ==="

# ─── Phase 1: Kubernetes cleanup ───
echo "[scalex] Phase 1: Removing Kubernetes components..."

if command -v kubeadm &>/dev/null; then
    echo "[scalex] Running kubeadm reset..."
    kubeadm reset -f --cri-socket unix:///run/containerd/containerd.sock 2>/dev/null || true
fi

echo "[scalex] Stopping Kubernetes services..."
systemctl stop kubelet 2>/dev/null || true
systemctl stop containerd 2>/dev/null || true
systemctl disable kubelet 2>/dev/null || true
systemctl disable containerd 2>/dev/null || true

echo "[scalex] Removing Kubernetes packages..."
apt-get purge -y -qq kubeadm kubelet kubectl containerd.io 2>/dev/null || true
apt-get purge -y -qq kubernetes-cni cri-tools 2>/dev/null || true

echo "[scalex] Cleaning Kubernetes directories..."
rm -rf /etc/kubernetes
rm -rf /var/lib/kubelet
rm -rf /var/lib/etcd
rm -rf /etc/cni
rm -rf /opt/cni
rm -rf /var/lib/containerd
rm -rf /run/containerd
rm -rf /var/lib/calico
rm -rf /etc/calico
rm -rf /var/run/calico
rm -rf /var/lib/cni

echo "[scalex] Flushing iptables rules..."
iptables -F 2>/dev/null || true
iptables -t nat -F 2>/dev/null || true
iptables -t mangle -F 2>/dev/null || true
iptables -X 2>/dev/null || true
ip6tables -F 2>/dev/null || true

# ─── Phase 2: KVM/libvirt cleanup ───
echo "[scalex] Phase 2: Removing KVM/libvirt..."

echo "[scalex] Destroying all VMs..."
for vm in $(virsh list --all --name 2>/dev/null); do
    virsh destroy "$vm" 2>/dev/null || true
    virsh undefine "$vm" --remove-all-storage 2>/dev/null || true
done

echo "[scalex] Stopping libvirt services..."
systemctl stop libvirtd 2>/dev/null || true
systemctl disable libvirtd 2>/dev/null || true

echo "[scalex] Removing KVM/libvirt packages..."
apt-get purge -y -qq qemu-kvm libvirt-daemon-system libvirt-clients virtinst bridge-utils 2>/dev/null || true
rm -rf /var/lib/libvirt || true
rm -rf /etc/libvirt || true

# ─── Phase 3: Bridge cleanup ───
echo "[scalex] Phase 3: Removing br0 bridge configuration..."

if ip link show br0 &>/dev/null; then
    ip link set br0 down 2>/dev/null || true
    ip link delete br0 type bridge 2>/dev/null || true
fi
rm -f /etc/netplan/50-scalex-bridge.yaml

echo "[scalex] Restoring original netplan from backup..."
if [ -d /etc/netplan/backup ]; then
    cp /etc/netplan/backup/*.yaml /etc/netplan/ 2>/dev/null || true
fi
netplan apply 2>/dev/null || true

# ─── Phase 4: Final cleanup ───
echo "[scalex] Phase 4: Cleaning up remaining packages..."
apt-get autoremove -y -qq 2>/dev/null || true
apt-get clean

echo "[scalex] === Node cleanup complete. SSH access preserved. ==="
"#
    .to_string()
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
    fn test_generate_bridge_script_ethernet() {
        let script =
            generate_bridge_setup_script("eno1", "192.168.88.10", "192.168.88.1", 24, false);
        assert!(script.contains("br0"));
        assert!(script.contains("eno1"));
        assert!(script.contains("ethernets:"));
        assert!(script.contains("netplan try"));
    }

    #[test]
    fn test_generate_bridge_script_bond() {
        let script =
            generate_bridge_setup_script("bond0", "192.168.88.10", "192.168.88.1", 24, true);
        assert!(script.contains("br0"));
        assert!(script.contains("bond0"));
        assert!(
            !script.contains("ethernets:"),
            "bond bridge must not have ethernets section"
        );
        assert!(script.contains("netplan try"));
        assert!(script.contains("bond=true"));
    }

    #[test]
    fn test_generate_vfio_script() {
        let script = generate_vfio_setup_script(&["10de:2544".to_string()]);
        assert!(script.contains("vfio-pci"));
        assert!(script.contains("10de:2544"));
        assert!(script.contains("intel_iommu=on"));
    }

    #[test]
    fn test_generate_node_cleanup_script_structure() {
        let script = generate_node_cleanup_script();
        // Must be a proper bash script
        assert!(script.starts_with("#!/bin/bash\nset -euo pipefail"));
        // Must clean up Kubernetes components
        assert!(script.contains("kubeadm reset"));
        assert!(script.contains("kubelet"));
        assert!(script.contains("kubectl"));
        // Must clean up container runtime
        assert!(script.contains("containerd"));
        // Must clean up KVM/libvirt
        assert!(script.contains("libvirt"));
        assert!(script.contains("qemu"));
        // Must remove bridge configuration
        assert!(script.contains("br0"));
        // Must clean up data directories
        assert!(script.contains("/etc/cni"));
        assert!(script.contains("/var/lib/kubelet"));
        assert!(script.contains("/etc/kubernetes"));
        // Must preserve SSH access — never kill sshd or remove openssh
        assert!(!script.contains("openssh-server"));
        assert!(!script.contains("stop sshd"));
        // Must have [scalex] logging prefix
        assert!(script.contains("[scalex]"));
    }

    /// Edge case: GPU passthrough requested but node has no GPUs → VFIO must be skipped.
    #[test]
    fn test_plan_host_gpu_passthrough_requested_but_no_gpus() {
        let facts = make_test_facts(true, false); // has bridge, NO GPU
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
        // GPU passthrough requested but no GPUs detected
        let steps = plan_host_preparation(&node, &facts, true);
        assert!(
            !steps.contains(&PrepStep::ConfigureVfio),
            "VFIO should be skipped when no GPUs detected"
        );
    }

    /// Edge case: VFIO script with empty GPU list should produce a skip message.
    #[test]
    fn test_generate_vfio_script_empty_gpus() {
        let script = generate_vfio_setup_script(&[]);
        assert!(
            script.contains("skipping VFIO"),
            "empty GPU list should skip VFIO setup"
        );
        assert!(
            !script.contains("intel_iommu"),
            "should not configure IOMMU when no GPUs"
        );
    }

    #[test]
    fn test_generate_node_cleanup_script_ordering() {
        let script = generate_node_cleanup_script();
        // Kubernetes must be cleaned before KVM (dependent services first)
        let k8s_pos = script.find("kubeadm reset").unwrap();
        let kvm_pos = script.find("libvirt").unwrap();
        assert!(k8s_pos < kvm_pos, "k8s cleanup must precede KVM cleanup");
        // Bridge removal must come after KVM cleanup
        let bridge_pos = script.find("br0").unwrap();
        assert!(
            kvm_pos < bridge_pos,
            "KVM cleanup must precede bridge removal"
        );
    }
}
