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

echo "[scalex] Disabling AppArmor security driver for QEMU (prevents Permission denied on disk images)..."
if ! grep -q '^security_driver' /etc/libvirt/qemu.conf 2>/dev/null; then
    echo 'security_driver = "none"' | sudo tee -a /etc/libvirt/qemu.conf >/dev/null
    sudo systemctl restart libvirtd
fi

echo "[scalex] Ensuring default storage pool exists..."
if ! sudo virsh pool-info default >/dev/null 2>&1; then
    sudo mkdir -p /var/lib/libvirt/images
    sudo virsh pool-define-as default dir --target /var/lib/libvirt/images
    sudo virsh pool-start default
    sudo virsh pool-autostart default
    echo "[scalex] Created default storage pool"
else
    echo "[scalex] Default storage pool already exists"
fi

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

# Idempotency: skip if br0 already exists with the correct IP
if ip addr show br0 2>/dev/null | grep -q '{ip_address}/{cidr_prefix}'; then
    echo "[scalex] br0 already configured with {ip_address}/{cidr_prefix} — skipping"
    exit 0
fi

# Backup existing netplan
sudo mkdir -p /etc/netplan/backup
sudo cp /etc/netplan/*.yaml /etc/netplan/backup/ 2>/dev/null || true
{strip_bond_ip}

sudo bash -c 'cat > /etc/netplan/50-scalex-bridge.yaml << NETPLAN_EOF
{netplan_body}
NETPLAN_EOF
chmod 600 /etc/netplan/50-scalex-bridge.yaml'

echo "[scalex] Applying netplan..."
sudo netplan apply 2>/dev/null || true
# Wait for br0 to come up (netplan apply may take a moment)
for i in $(seq 1 10); do
    if ip addr show br0 2>/dev/null | grep -q '{ip_address}/{cidr_prefix}'; then
        echo "[scalex] br0 bridge setup complete."
        exit 0
    fi
    sleep 1
done
echo "[scalex] WARNING: br0 not yet visible — caller will verify via SSH retry."
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

/// Generate the shell script to perform KVM/libvirt teardown on a node.
/// Handles: destroy running VMs, enumerate + delete storage volumes per pool,
/// destroy and undefine storage pools, undefine all remaining VM domains.
/// Operates on qemu:///system only. Does NOT remove packages or network config.
/// Pure function. Idempotent — safe to re-run on already-clean nodes.
pub fn generate_kvm_teardown_script() -> String {
    r#"#!/bin/bash
set -euo pipefail

echo "[scalex] === KVM/libvirt Teardown ==="

# Guard: if virsh is not present, libvirt is not installed — nothing to do.
if ! command -v virsh &>/dev/null; then
    echo "[scalex] virsh not found — libvirt not installed, skipping KVM teardown."
    exit 0
fi

VIRSH="virsh -c qemu:///system"

# ─── Step 1: Stop (destroy) all running VM domains ───
echo "[scalex] Step 1: Stopping all running VM domains..."
for vm in $($VIRSH list --state-running --name 2>/dev/null | grep -v '^$'); do
    echo "[scalex]   Destroying domain: $vm"
    $VIRSH destroy "$vm" 2>/dev/null || true
done

# ─── Step 2: Remove storage volumes from each pool ───
echo "[scalex] Step 2: Removing storage volumes from all pools..."
for pool in $($VIRSH pool-list --all --name 2>/dev/null | grep -v '^$'); do
    echo "[scalex]   Processing storage pool: $pool"
    # Refresh pool so virsh sees up-to-date volume list
    $VIRSH pool-refresh "$pool" 2>/dev/null || true
    # Enumerate and delete each volume
    for vol in $($VIRSH vol-list "$pool" 2>/dev/null | tail -n +3 | awk '{print $1}' | grep -v '^$'); do
        echo "[scalex]     Deleting volume: $vol (pool: $pool)"
        $VIRSH vol-delete "$vol" --pool "$pool" 2>/dev/null || true
    done
    # Destroy (stop) the pool
    echo "[scalex]   Destroying pool: $pool"
    $VIRSH pool-destroy "$pool" 2>/dev/null || true
    # Undefine (remove definition) the pool
    echo "[scalex]   Undefining pool: $pool"
    $VIRSH pool-undefine "$pool" 2>/dev/null || true
done

# ─── Step 3: Undefine all VM domains (including those already stopped) ───
echo "[scalex] Step 3: Undefining all VM domains..."
for vm in $($VIRSH list --all --name 2>/dev/null | grep -v '^$'); do
    echo "[scalex]   Undefining domain: $vm"
    # Use --nvram to also remove UEFI NVRAM files if present; ignore errors
    $VIRSH undefine "$vm" --nvram 2>/dev/null || \
        $VIRSH undefine "$vm" 2>/dev/null || true
done

# ─── Step 4: Remove default network (if present) ───
echo "[scalex] Step 4: Removing libvirt default network..."
$VIRSH net-destroy default 2>/dev/null || true
$VIRSH net-undefine default 2>/dev/null || true

echo "[scalex] === KVM/libvirt teardown complete. ==="
"#
    .to_string()
}

/// Generate the shell script to fully clean a node (remove K8s, KVM packages).
/// Used by `sdi clean --hard` to reset nodes to bare-metal state.
/// Pure function. Preserves SSH access and basic networking.
///
/// SAFETY: Never modify network interfaces (br0/bond) — SSH depends on them.
/// The script ONLY removes K8s/KVM software and their virtual interfaces (Cilium,
/// lxc*, veth*). It never touches br0, bond0, physical NICs, or netplan config.
///
/// K8s teardown phases: (1) kubeadm reset with CRI socket auto-detection and
/// `--cleanup-tmp-dir`, (1a) Cilium CNI cleanup — virtual interfaces + eBPF maps +
/// state dirs, (1b) etcd wipe — data dir + events store + external cert dir,
/// (1c) stop/purge K8s services/packages, (1d) flush iptables and all CNI dirs.
pub fn generate_node_cleanup_script() -> String {
    r#"#!/bin/bash
set -euo pipefail

echo "[scalex] === Node Cleanup: Resetting to bare-metal state ==="
# SAFETY: Never modify network interfaces (br0/bond) — SSH depends on them.

# ─── Phase 1: Kubernetes cluster teardown ───
echo "[scalex] Phase 1: K8s cluster teardown..."

if command -v kubeadm &>/dev/null; then
    echo "[scalex] Running kubeadm reset (with --cleanup-tmp-dir)..."
    # Auto-detect CRI socket: prefer containerd socket, fall back to kubeadm default
    if [ -S /run/containerd/containerd.sock ]; then
        sudo kubeadm reset -f --cleanup-tmp-dir \
            --cri-socket unix:///run/containerd/containerd.sock 2>/dev/null || true
    else
        sudo kubeadm reset -f --cleanup-tmp-dir 2>/dev/null || true
    fi
fi

# ─── Phase 1a: CNI cleanup (Cilium) ───
echo "[scalex] Phase 1a: CNI cleanup — removing Cilium interfaces and eBPF state..."

# Remove Cilium virtual network interfaces
for iface in cilium_host cilium_net cilium_vxlan; do
    if ip link show "$iface" &>/dev/null; then
        sudo ip link set "$iface" down 2>/dev/null || true
        sudo ip link delete "$iface" 2>/dev/null || true
        echo "[scalex]   Removed Cilium interface: $iface"
    fi
done

# Remove per-pod lxc* virtual interfaces created by Cilium
for iface in $(ip link show 2>/dev/null | grep -oP '(?<=\d: )lxc[^\s@:]+' 2>/dev/null || true); do
    sudo ip link set "$iface" down 2>/dev/null || true
    sudo ip link delete "$iface" 2>/dev/null || true
done

# Remove CNI/overlay virtual interfaces (Flannel VXLAN, Calico VXLAN, veth pairs, dummy)
# SAFETY: Never modify network interfaces (br0/bond) — SSH depends on them.
# Only matches prefixes that are exclusively K8s/CNI-created (never physical or bridge).
while IFS= read -r virt_iface; do
    [ -z "$virt_iface" ] && continue
    sudo ip link set "$virt_iface" down 2>/dev/null || true
    sudo ip link delete "$virt_iface" 2>/dev/null || true
done < <(sudo ip -o link show 2>/dev/null | awk -F': ' '{print $2}' | cut -d'@' -f1 | \
    grep -E '^(flannel\.|vxlan\.|veth[0-9a-f]|dummy)' 2>/dev/null || true)

# Detach eBPF programs and remove Cilium pinned maps from BPF filesystem
if command -v bpftool &>/dev/null; then
    sudo bpftool net detach 2>/dev/null || true
fi
sudo rm -rf /sys/fs/bpf/cilium 2>/dev/null || true
sudo rm -rf /sys/fs/bpf/tc/globals/cilium_* 2>/dev/null || true

# Remove Cilium runtime and state directories
sudo rm -rf /run/cilium
sudo rm -rf /var/lib/cilium

# ─── Phase 1b: etcd wipe ───
echo "[scalex] Phase 1b: etcd wipe — removing data dir, WAL, snapshots, member data..."

# Primary etcd data directory (contains WAL/ and snap/ subdirs for member data)
sudo rm -rf /var/lib/etcd
# Separate etcd events store (used when --experimental-backend-quota-bytes is split)
sudo rm -rf /var/lib/etcd-events
# External etcd certificate directory (separate from /etc/kubernetes for stacked etcd)
sudo rm -rf /etc/etcd

echo "[scalex] Stopping Kubernetes services..."
sudo systemctl stop kubelet 2>/dev/null || true
sudo systemctl stop containerd 2>/dev/null || true
sudo systemctl disable kubelet 2>/dev/null || true
sudo systemctl disable containerd 2>/dev/null || true

echo "[scalex] Removing Kubernetes packages..."
sudo apt-get purge -y -qq kubeadm kubelet kubectl containerd.io 2>/dev/null || true
sudo apt-get purge -y -qq kubernetes-cni cri-tools 2>/dev/null || true

echo "[scalex] Cleaning Kubernetes directories..."
sudo rm -rf /etc/kubernetes
sudo rm -rf /var/lib/kubelet
sudo rm -rf /etc/cni
sudo rm -rf /opt/cni/bin/* 2>/dev/null || true  # remove contents only, not the directory
sudo rm -rf /var/lib/containerd
sudo rm -rf /run/containerd
sudo rm -rf /var/lib/calico
sudo rm -rf /etc/calico
sudo rm -rf /var/run/calico
sudo rm -rf /var/lib/cni

echo "[scalex] Flushing iptables rules (all tables, policy reset to ACCEPT)..."
# Reset filter policies to ACCEPT first — prevents host lockout when rules are flushed
sudo iptables -P INPUT ACCEPT 2>/dev/null || true
sudo iptables -P FORWARD ACCEPT 2>/dev/null || true
sudo iptables -P OUTPUT ACCEPT 2>/dev/null || true
# Flush all chains in the default filter table first
sudo iptables -F 2>/dev/null || true
# Then flush all chains and user-defined chains in every table
for tbl in filter nat mangle raw security; do
    sudo iptables -t "$tbl" -F 2>/dev/null || true
    sudo iptables -t "$tbl" -X 2>/dev/null || true
    sudo iptables -t "$tbl" -Z 2>/dev/null || true
done
# ip6tables: same treatment
sudo ip6tables -P INPUT ACCEPT 2>/dev/null || true
sudo ip6tables -P FORWARD ACCEPT 2>/dev/null || true
sudo ip6tables -P OUTPUT ACCEPT 2>/dev/null || true
for tbl in filter nat mangle raw security; do
    sudo ip6tables -t "$tbl" -F 2>/dev/null || true
    sudo ip6tables -t "$tbl" -X 2>/dev/null || true
    sudo ip6tables -t "$tbl" -Z 2>/dev/null || true
done
# Flush and destroy all ipset sets (used by Calico/kube-proxy)
if command -v ipset &>/dev/null; then
    sudo ipset flush 2>/dev/null || true
    sudo ipset destroy 2>/dev/null || true
fi

# ─── Phase 2: KVM/libvirt teardown ───
echo "[scalex] Phase 2: Removing KVM/libvirt..."

if command -v virsh &>/dev/null; then
    VIRSH="virsh -c qemu:///system"

    echo "[scalex] Stopping all running VM domains..."
    for vm in $($VIRSH list --state-running --name 2>/dev/null | grep -v '^$'); do
        $VIRSH destroy "$vm" 2>/dev/null || true
    done

    echo "[scalex] Removing storage volumes from all pools..."
    for pool in $($VIRSH pool-list --all --name 2>/dev/null | grep -v '^$'); do
        $VIRSH pool-refresh "$pool" 2>/dev/null || true
        for vol in $($VIRSH vol-list "$pool" 2>/dev/null | tail -n +3 | awk '{print $1}' | grep -v '^$'); do
            $VIRSH vol-delete "$vol" --pool "$pool" 2>/dev/null || true
        done
        $VIRSH pool-destroy "$pool" 2>/dev/null || true
        $VIRSH pool-undefine "$pool" 2>/dev/null || true
    done

    echo "[scalex] Undefining all VM domains..."
    for vm in $($VIRSH list --all --name 2>/dev/null | grep -v '^$'); do
        $VIRSH undefine "$vm" --nvram 2>/dev/null || \
            $VIRSH undefine "$vm" 2>/dev/null || true
    done

    $VIRSH net-destroy default 2>/dev/null || true
    $VIRSH net-undefine default 2>/dev/null || true
fi

echo "[scalex] Stopping libvirt services..."
sudo systemctl stop libvirtd 2>/dev/null || true
sudo systemctl disable libvirtd 2>/dev/null || true

echo "[scalex] Removing KVM/libvirt packages..."
# SAFETY: bridge-utils is intentionally excluded — it provides brctl which may be
# needed by br0/bond0. Removing it can disrupt network interfaces SSH depends on.
sudo apt-get purge -y -qq qemu-kvm libvirt-daemon-system libvirt-clients virtinst 2>/dev/null || true
sudo rm -rf /var/lib/libvirt || true
sudo rm -rf /etc/libvirt || true

# ─── Phase 3: Final cleanup ───
# SAFETY: Never modify network interfaces (br0/bond) — SSH depends on them.
# Only libvirt's own virtual network interfaces (virbr*, vnet*) were already
# removed above by virsh net-destroy. br0, bond0, and physical NICs are untouched.
echo "[scalex] Phase 3: Cleaning up remaining packages..."
sudo apt-get autoremove -y -qq 2>/dev/null || true
sudo apt-get clean

echo "[scalex] === Node cleanup complete. Network interfaces (br0/bond) preserved. ==="
"#
    .to_string()
}

/// Generate a self-contained launcher script for bridge setup that survives SSH disconnection.
///
/// The strategy:
/// 1. Write the full bridge setup script to `/tmp/scalex-bridge-setup.sh` on the remote node.
/// 2. Launch it with `nohup bash ... &` — detaches from the SSH session immediately.
/// 3. Exit SSH right away (SSH returns success even though network will briefly drop).
///
/// The caller must then disconnect intentionally, wait ~30-60s, and poll br0 via a fresh SSH
/// connection to verify the bridge came up. This avoids the SSH-disconnects-on-netplan-apply
/// failure mode for bond0→br0 transitions.
///
/// Pure function.
pub fn generate_bridge_nohup_launcher(
    primary_nic: &str,
    ip_address: &str,
    gateway: &str,
    cidr_prefix: u8,
    is_bond: bool,
) -> String {
    let setup_script =
        generate_bridge_setup_script(primary_nic, ip_address, gateway, cidr_prefix, is_bond);
    // Escape single quotes in the script body for embedding in a bash heredoc-style write.
    // We use a base64-encoded payload to avoid any quoting issues with the script content.
    // The launcher: (a) decode and write script, (b) chmod, (c) nohup launch, (d) exit 0.
    let encoded = base64_encode_script(&setup_script);
    format!(
        r#"#!/bin/bash
# Scalex bridge nohup launcher — survives SSH disconnection during netplan apply
# 1. Decode and write the actual setup script
echo '{encoded}' | base64 -d > /tmp/scalex-bridge-setup.sh
chmod +x /tmp/scalex-bridge-setup.sh
# 2. Launch in background via nohup — detaches from this SSH session
nohup bash /tmp/scalex-bridge-setup.sh > /tmp/scalex-bridge-setup.log 2>&1 &
echo "[scalex] Bridge setup launched (PID $!) — SSH will disconnect when netplan applies"
# 3. Exit immediately — do NOT wait; caller polls br0 via fresh SSH after reconnect
exit 0
"#
    )
}

/// Base64-encode a script string (pure, no I/O).
/// Uses standard alphabet, no line wrapping (single line output).
fn base64_encode_script(s: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 2 < bytes.len() {
        let b0 = bytes[i] as usize;
        let b1 = bytes[i + 1] as usize;
        let b2 = bytes[i + 2] as usize;
        out.push(ALPHABET[b0 >> 2] as char);
        out.push(ALPHABET[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(ALPHABET[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
        out.push(ALPHABET[b2 & 0x3f] as char);
        i += 3;
    }
    match bytes.len() - i {
        1 => {
            let b0 = bytes[i] as usize;
            out.push(ALPHABET[b0 >> 2] as char);
            out.push(ALPHABET[(b0 & 3) << 4] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let b0 = bytes[i] as usize;
            let b1 = bytes[i + 1] as usize;
            out.push(ALPHABET[b0 >> 2] as char);
            out.push(ALPHABET[((b0 & 3) << 4) | (b1 >> 4)] as char);
            out.push(ALPHABET[(b1 & 0xf) << 2] as char);
            out.push('=');
        }
        _ => {}
    }
    out
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
        assert!(script.contains("netplan apply"));
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
        assert!(script.contains("netplan apply"));
        assert!(script.contains("bond=true"));
    }

    #[test]
    fn test_generate_bridge_nohup_launcher_contains_nohup_and_base64() {
        let launcher =
            generate_bridge_nohup_launcher("bond0", "192.168.88.10", "192.168.88.1", 24, true);
        // Must launch via nohup so it survives SSH disconnection
        assert!(launcher.contains("nohup"), "launcher must use nohup");
        // Must decode a base64 payload (the actual setup script)
        assert!(
            launcher.contains("base64 -d"),
            "launcher must decode base64-encoded script"
        );
        // Must write to /tmp so no sudo is needed for the write step
        assert!(
            launcher.contains("/tmp/scalex-bridge-setup.sh"),
            "launcher must write script to /tmp"
        );
        // Must exit 0 immediately — caller polls separately
        assert!(
            launcher.contains("exit 0"),
            "launcher must exit 0 immediately"
        );
    }

    #[test]
    fn test_base64_encode_decode_roundtrip() {
        // The launcher embeds a base64-encoded setup script; verify encode is correct
        // by checking that the launcher's payload decodes to something containing br0.
        let launcher = generate_bridge_nohup_launcher("eno1", "10.0.0.5", "10.0.0.1", 24, false);
        // Extract the base64 payload (single-quoted token after "echo '")
        let payload_start = launcher
            .find("echo '")
            .expect("launcher must have echo '...")
            + 6;
        let payload_end =
            launcher[payload_start..].find("'").expect("closing quote") + payload_start;
        let encoded = &launcher[payload_start..payload_end];
        // Decode using standard base64 alphabet
        let decoded = base64_decode_for_test(encoded);
        assert!(
            decoded.contains("br0"),
            "decoded payload must contain br0: {}",
            &decoded[..decoded.len().min(200)]
        );
        assert!(
            decoded.contains("netplan apply"),
            "decoded payload must contain netplan apply"
        );
    }

    /// Test-only base64 decoder to verify the launcher payload.
    fn base64_decode_for_test(s: &str) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let chars: Vec<u8> = s
            .bytes()
            .filter(|&b| b != b'=' && b != b'\n' && b != b'\r')
            .collect();
        let mut i = 0;
        while i + 3 < chars.len() {
            let pos = |c: u8| ALPHABET.iter().position(|&a| a == c).unwrap_or(0) as u8;
            let b0 = pos(chars[i]);
            let b1 = pos(chars[i + 1]);
            let b2 = pos(chars[i + 2]);
            let b3 = pos(chars[i + 3]);
            out.push((b0 << 2) | (b1 >> 4));
            out.push(((b1 & 0xf) << 4) | (b2 >> 2));
            out.push(((b2 & 3) << 6) | b3);
            i += 4;
        }
        String::from_utf8_lossy(&out).to_string()
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
        // Must NOT remove bridge configuration — br0/bond are network infrastructure
        assert!(
            !script.contains("ip link delete") || !script.contains("type bridge"),
            "cleanup must never delete bridge interfaces"
        );
        // Safety comment must be present
        assert!(script.contains("SAFETY: Never modify network interfaces"));
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
        // Bridge interfaces (br0/bond) must NEVER be deleted — network safety
        assert!(
            !script.contains("ip link delete br0"),
            "cleanup must never delete br0 — SSH depends on it"
        );
    }

    // --- KVM teardown script tests ---

    #[test]
    fn test_generate_kvm_teardown_script_is_valid_bash() {
        let script = generate_kvm_teardown_script();
        assert!(
            script.starts_with("#!/bin/bash\nset -euo pipefail"),
            "must be a proper bash script with strict mode"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_destroys_running_vms() {
        let script = generate_kvm_teardown_script();
        // Must stop running domains before trying to undefine
        assert!(
            script.contains("--state-running"),
            "must list only running domains to destroy"
        );
        assert!(
            script.contains("destroy"),
            "must call virsh destroy to stop running VMs"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_handles_storage_pools() {
        let script = generate_kvm_teardown_script();
        // Must enumerate all storage pools
        assert!(
            script.contains("pool-list"),
            "must enumerate all storage pools"
        );
        // Must delete volumes within each pool
        assert!(
            script.contains("vol-list"),
            "must enumerate volumes within each pool"
        );
        assert!(
            script.contains("vol-delete"),
            "must delete volumes from pools"
        );
        // Must destroy (stop) pools before undefining
        assert!(
            script.contains("pool-destroy"),
            "must destroy (stop) each storage pool"
        );
        // Must undefine pools to remove their definitions
        assert!(
            script.contains("pool-undefine"),
            "must undefine each storage pool"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_pool_delete_precedes_pool_undefine() {
        let script = generate_kvm_teardown_script();
        let destroy_pos = script.find("pool-destroy").unwrap();
        let undefine_pos = script.find("pool-undefine").unwrap();
        assert!(
            destroy_pos < undefine_pos,
            "pool-destroy must precede pool-undefine"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_undefines_domains() {
        let script = generate_kvm_teardown_script();
        // Must undefine domains (remove VM definitions)
        assert!(script.contains("undefine"), "must undefine VM domains");
        // Must list ALL domains (not just running) for undefine step
        assert!(
            script.contains("--all"),
            "must list all domains (including stopped) for undefine"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_handles_nvram() {
        let script = generate_kvm_teardown_script();
        // Must attempt to remove UEFI NVRAM files
        assert!(
            script.contains("--nvram"),
            "must attempt to remove NVRAM files with --nvram flag"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_uses_system_connection() {
        let script = generate_kvm_teardown_script();
        // Must use qemu:///system (not user session)
        assert!(
            script.contains("qemu:///system"),
            "must operate on system libvirt connection"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_pool_refresh_before_vol_list() {
        let script = generate_kvm_teardown_script();
        let refresh_pos = script.find("pool-refresh").unwrap();
        let vol_list_pos = script.find("vol-list").unwrap();
        assert!(
            refresh_pos < vol_list_pos,
            "pool-refresh must precede vol-list to ensure up-to-date volume enumeration"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_handles_missing_virsh() {
        let script = generate_kvm_teardown_script();
        // Must guard against missing virsh (libvirt not installed)
        assert!(
            script.contains("command -v virsh"),
            "must check for virsh availability before attempting teardown"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_removes_default_network() {
        let script = generate_kvm_teardown_script();
        assert!(
            script.contains("net-destroy"),
            "must destroy default libvirt network"
        );
        assert!(
            script.contains("net-undefine"),
            "must undefine default libvirt network"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_destroy_vms_before_pool_teardown() {
        let script = generate_kvm_teardown_script();
        // VMs must be stopped before storage volumes are removed
        let vm_destroy_pos = script.find("--state-running").unwrap();
        let pool_list_pos = script.find("pool-list").unwrap();
        assert!(
            vm_destroy_pos < pool_list_pos,
            "VM destruction must precede storage pool teardown"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_pool_teardown_before_domain_undefine() {
        let script = generate_kvm_teardown_script();
        // Storage cleanup must happen before domain undefine
        let pool_list_pos = script.find("pool-list").unwrap();
        // Find the "Undefining all VM domains" step marker
        let domain_undefine_pos = script.find("Undefining all VM domains").unwrap();
        assert!(
            pool_list_pos < domain_undefine_pos,
            "storage pool teardown must precede domain undefine step"
        );
    }

    #[test]
    fn test_generate_kvm_teardown_script_uses_scalex_log_prefix() {
        let script = generate_kvm_teardown_script();
        assert!(
            script.contains("[scalex]"),
            "must use [scalex] logging prefix for consistency"
        );
    }

    #[test]
    fn test_generate_node_cleanup_script_uses_pool_based_kvm_teardown() {
        let script = generate_node_cleanup_script();
        // The monolithic cleanup script must now use proper pool-based teardown
        assert!(
            script.contains("pool-list"),
            "node cleanup must enumerate storage pools"
        );
        assert!(
            script.contains("vol-delete"),
            "node cleanup must delete volumes per pool"
        );
        assert!(
            script.contains("pool-destroy"),
            "node cleanup must destroy storage pools"
        );
        assert!(
            script.contains("pool-undefine"),
            "node cleanup must undefine storage pools"
        );
    }

    // ===== Sub-AC 5: Idempotent re-run of `scalex sdi clean --hard` =====
    // These tests verify the cleanup script is safe to re-run on already-clean
    // nodes — e.g., second run after K8s/KVM/bridge artifacts are already absent.

    /// Sub-AC 5: virsh commands must be inside `command -v virsh` guard.
    /// On second run, libvirt is already removed; virsh is no longer on PATH.
    /// Without this guard, `$(virsh list ...)` in a for-loop header fails under
    /// `set -euo pipefail` with exit code 127 (command not found), causing the
    /// second run to exit non-zero instead of cleanly completing.
    #[test]
    fn test_cleanup_script_virsh_guarded_for_idempotency() {
        let script = generate_node_cleanup_script();
        // Must use command -v virsh guard before any virsh calls
        assert!(
            script.contains("command -v virsh"),
            "virsh operations must be guarded with `command -v virsh` for idempotency on re-run"
        );
        // Guard must appear BEFORE any virsh storage pool operations
        let guard_pos = script.find("command -v virsh").unwrap();
        let pool_list_pos = script.find("pool-list").unwrap();
        assert!(
            guard_pos < pool_list_pos,
            "virsh guard must precede all virsh pool operations"
        );
    }

    /// Sub-AC 5: kubeadm reset must be guarded with `command -v kubeadm`.
    /// On second run after K8s packages are already purged, kubeadm is absent.
    /// The guard ensures re-run exits cleanly instead of failing on command not found.
    #[test]
    fn test_cleanup_script_kubeadm_guarded_for_idempotency() {
        let script = generate_node_cleanup_script();
        // Must guard kubeadm before attempting kubeadm reset
        assert!(
            script.contains("command -v kubeadm"),
            "kubeadm reset must be guarded with `command -v kubeadm` for idempotency"
        );
        let guard_pos = script.find("command -v kubeadm").unwrap();
        let reset_pos = script.find("kubeadm reset").unwrap();
        assert!(
            guard_pos < reset_pos,
            "kubeadm guard must precede `kubeadm reset` invocation"
        );
    }

    /// Sub-AC 5: All systemctl, iptables, apt-get, netplan calls must use `|| true`
    /// so the second run does not fail even when those units/packages are already absent.
    #[test]
    fn test_cleanup_script_soft_failure_on_missing_services() {
        let script = generate_node_cleanup_script();
        // systemctl stop/disable must never hard-fail (units may not exist on re-run)
        // Script uses `sudo systemctl` prefix
        assert!(
            script.contains("systemctl stop kubelet 2>/dev/null || true"),
            "systemctl stop kubelet must use || true"
        );
        assert!(
            script.contains("systemctl stop containerd 2>/dev/null || true"),
            "systemctl stop containerd must use || true"
        );
        assert!(
            script.contains("systemctl stop libvirtd 2>/dev/null || true"),
            "systemctl stop libvirtd must use || true"
        );
        // iptables flush must be soft (not installed on all distros)
        // Script uses loop: `sudo iptables -t "$tbl" -F 2>/dev/null || true`
        assert!(
            script.contains("iptables -t \"$tbl\" -F 2>/dev/null || true")
                || script.contains("iptables -F 2>/dev/null || true"),
            "iptables flush must use || true"
        );
        // apt-get autoremove must be soft
        assert!(
            script.contains("apt-get autoremove -y -qq 2>/dev/null || true"),
            "apt-get autoremove must use || true"
        );
    }

    /// Sub-AC 5: All K8s artifact directories are removed with `rm -rf` (no errors
    /// on missing paths) ensuring a clean re-run even when dirs are already absent.
    #[test]
    fn test_cleanup_script_removes_k8s_artifacts_idempotently() {
        let script = generate_node_cleanup_script();
        // K8s directories: rm -rf never fails on non-existent paths.
        // Script may use `sudo rm -rf` prefix.
        let k8s_dirs = [
            "/etc/kubernetes",
            "/var/lib/kubelet",
            "/var/lib/etcd",
            "/etc/cni",
            "/opt/cni",
            "/var/lib/containerd",
            "/run/containerd",
            "/var/lib/calico",
            "/etc/calico",
            "/var/run/calico",
            "/var/lib/cni",
        ];
        for dir in &k8s_dirs {
            assert!(
                script.contains(&format!("rm -rf {dir}")),
                "cleanup must remove K8s artifact dir: {dir}"
            );
        }
    }

    /// SAFETY: cleanup must never delete bridge interfaces (br0, bond0) — SSH depends on them.
    #[test]
    fn test_cleanup_script_never_deletes_bridge_interfaces() {
        let script = generate_node_cleanup_script();
        // Must NOT enumerate or delete bridge-type interfaces
        assert!(
            !script.contains("link show type bridge"),
            "cleanup must never enumerate bridge interfaces for deletion"
        );
        assert!(
            !script.contains("ip link delete")
                || script
                    .find("ip link delete")
                    .map(|p| !script[p..].starts_with("ip link delete br"))
                    .unwrap_or(true),
            "cleanup must never delete br0 or other bridge interfaces"
        );
        // Must NOT touch netplan config — network changes break SSH
        assert!(
            !script.contains("rm -f /etc/netplan/50-scalex-bridge.yaml"),
            "cleanup must never remove netplan bridge config"
        );
        assert!(
            !script.contains("netplan apply"),
            "cleanup must never run netplan apply — network changes break SSH"
        );
        // Safety comment must be present
        assert!(
            script.contains("SAFETY: Never modify network interfaces"),
            "cleanup script must contain safety comment"
        );
    }

    // ===== Sub-AC 4: Enhanced bridge/network cleanup =====

    /// Sub-AC 4: iptables policy reset to ACCEPT must happen before flushing chains.
    /// This prevents the host from being locked out when DROP policies are active.
    #[test]
    fn test_cleanup_script_resets_iptables_policies_before_flush() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("iptables -P INPUT ACCEPT"),
            "must reset INPUT policy to ACCEPT before flush"
        );
        assert!(
            script.contains("iptables -P FORWARD ACCEPT"),
            "must reset FORWARD policy to ACCEPT before flush"
        );
        assert!(
            script.contains("iptables -P OUTPUT ACCEPT"),
            "must reset OUTPUT policy to ACCEPT before flush"
        );
        // Policy reset must appear before the table flush loop
        let policy_pos = script.find("iptables -P INPUT ACCEPT").unwrap();
        let flush_pos = script.find("iptables -t \"$tbl\" -F").unwrap();
        assert!(
            policy_pos < flush_pos,
            "iptables policy reset must precede chain flush"
        );
    }

    /// Sub-AC 4: All iptables tables must be flushed (filter, nat, mangle, raw, security).
    #[test]
    fn test_cleanup_script_flushes_all_iptables_tables() {
        let script = generate_node_cleanup_script();
        for tbl in &["filter", "nat", "mangle", "raw", "security"] {
            assert!(
                script.contains(tbl),
                "iptables cleanup must flush table: {tbl}"
            );
        }
        assert!(
            script.contains("iptables -t \"$tbl\" -F"),
            "must flush chains"
        );
        assert!(
            script.contains("iptables -t \"$tbl\" -X"),
            "must delete user-defined chains"
        );
        assert!(
            script.contains("iptables -t \"$tbl\" -Z"),
            "must zero packet/byte counters"
        );
    }

    /// Sub-AC 4: ip6tables must receive the same flush treatment as iptables.
    #[test]
    fn test_cleanup_script_flushes_ip6tables() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("ip6tables -P INPUT ACCEPT"),
            "must reset ip6tables INPUT policy"
        );
        assert!(
            script.contains("ip6tables -P FORWARD ACCEPT"),
            "must reset ip6tables FORWARD policy"
        );
        assert!(
            script.contains("ip6tables -P OUTPUT ACCEPT"),
            "must reset ip6tables OUTPUT policy"
        );
        assert!(
            script.contains("ip6tables -t \"$tbl\" -F"),
            "must flush ip6tables chains"
        );
    }

    /// Sub-AC 4: ipset sets must be flushed and destroyed (used by Calico/kube-proxy).
    #[test]
    fn test_cleanup_script_destroys_ipset_sets() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("command -v ipset"),
            "must check for ipset availability"
        );
        assert!(script.contains("ipset flush"), "must flush all ipset sets");
        assert!(
            script.contains("ipset destroy"),
            "must destroy all ipset sets"
        );
    }

    /// Sub-AC 4: CNI/overlay virtual interfaces must be cleaned up.
    #[test]
    fn test_cleanup_script_removes_cni_virtual_interfaces() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("flannel"),
            "must handle Flannel virtual interfaces"
        );
        assert!(
            script.contains("vxlan"),
            "must handle VXLAN overlay interfaces"
        );
        assert!(
            script.contains("veth"),
            "must handle veth pair interfaces from CNI"
        );
    }

    /// SAFETY: cleanup must never modify netplan config or write DHCP fallback.
    /// Network changes would break SSH connectivity to the node.
    #[test]
    fn test_cleanup_script_never_modifies_network_config() {
        let script = generate_node_cleanup_script();
        assert!(
            !script.contains("99-scalex-dhcp-fallback.yaml"),
            "cleanup must never write a DHCP fallback netplan config"
        );
        assert!(
            !script.contains("dhcp4: true"),
            "cleanup must never write DHCP config"
        );
        assert!(
            !script.contains("/etc/netplan/backup"),
            "cleanup must never restore netplan backup"
        );
        assert!(
            !script.contains("netplan apply"),
            "cleanup must never run netplan apply"
        );
    }

    /// Sub-AC 5: The cleanup script must NEVER touch SSH access.
    /// This is the key safety invariant: after clean --hard, the operator
    /// can still SSH into nodes for the next provisioning cycle.
    #[test]
    fn test_cleanup_script_preserves_ssh_access() {
        let script = generate_node_cleanup_script();
        // Must not stop or remove SSH daemon
        assert!(
            !script.contains("stop sshd") && !script.contains("stop ssh"),
            "cleanup must never stop sshd"
        );
        assert!(
            !script.contains("openssh-server"),
            "cleanup must never remove openssh-server"
        );
        assert!(
            !script.contains("openssh-client"),
            "cleanup must never remove openssh-client"
        );
        // Must not remove authorized_keys or SSH host keys
        assert!(
            !script.contains("rm.*authorized_keys"),
            "cleanup must preserve authorized_keys"
        );
        assert!(
            !script.contains("/etc/ssh/ssh_host"),
            "cleanup must preserve SSH host keys"
        );
    }

    /// Sub-AC 5: KVM teardown script exits cleanly (exit 0) when libvirt is absent.
    /// This is the second-run scenario: KVM was cleaned on first run, virsh is gone.
    #[test]
    fn test_kvm_teardown_script_exits_cleanly_when_virsh_absent() {
        let script = generate_kvm_teardown_script();
        // Must guard on virsh presence and exit 0 when not found
        assert!(
            script.contains("command -v virsh"),
            "kvm teardown must check for virsh"
        );
        assert!(
            script.contains("exit 0"),
            "kvm teardown must exit 0 (clean) when virsh not found — idempotent on re-run"
        );
        // The guard must come before any virsh pool or VM operations
        let guard_pos = script.find("command -v virsh").unwrap();
        let pool_list_pos = script.find("pool-list").unwrap();
        assert!(
            guard_pos < pool_list_pos,
            "virsh guard must precede storage pool operations"
        );
    }

    // ===== Sub-AC 2: K8s cluster teardown — kubeadm reset, CNI cleanup, etcd wipe =====

    /// Sub-AC 2: kubeadm reset must use --cleanup-tmp-dir to wipe /etc/kubernetes/tmp.
    /// Without this flag, tmp manifests from previous cluster init are left behind and
    /// can interfere with a fresh `kubeadm init` on the same node.
    #[test]
    fn test_cleanup_kubeadm_reset_uses_cleanup_tmp_dir() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("--cleanup-tmp-dir"),
            "kubeadm reset must use --cleanup-tmp-dir to remove /etc/kubernetes/tmp"
        );
    }

    /// Sub-AC 2: kubeadm reset must auto-detect CRI socket.
    /// Containerd is the CRI used by Kubespray in this project. The script must
    /// prefer the containerd socket path when the socket file is present.
    #[test]
    fn test_cleanup_kubeadm_reset_cri_socket_autodetection() {
        let script = generate_node_cleanup_script();
        // Must check for containerd socket presence
        assert!(
            script.contains("/run/containerd/containerd.sock"),
            "kubeadm reset must reference containerd socket for CRI detection"
        );
        // Must use -S (file socket test) for the detection
        assert!(
            script.contains("[ -S /run/containerd/containerd.sock ]"),
            "must test for containerd socket with [ -S ... ]"
        );
        // Must include the socket flag when socket is detected
        assert!(
            script.contains("--cri-socket unix:///run/containerd/containerd.sock"),
            "must pass CRI socket to kubeadm reset when detected"
        );
    }

    /// Sub-AC 2: CNI cleanup must remove Cilium virtual network interfaces.
    /// Cilium creates cilium_host and cilium_net on every K8s node. These persist
    /// after package removal and block a fresh Cilium install on the same node.
    #[test]
    fn test_cleanup_removes_cilium_virtual_interfaces() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("cilium_host"),
            "must remove cilium_host virtual interface"
        );
        assert!(
            script.contains("cilium_net"),
            "must remove cilium_net virtual interface"
        );
        assert!(
            script.contains("cilium_vxlan"),
            "must remove cilium_vxlan virtual interface"
        );
        // Must also handle per-pod lxc* interfaces
        assert!(
            script.contains("lxc"),
            "must attempt to remove lxc* per-pod Cilium interfaces"
        );
    }

    /// Sub-AC 2: CNI cleanup must remove Cilium eBPF pinned maps and state dirs.
    /// Stale eBPF maps in /sys/fs/bpf/cilium and runtime state in /run/cilium and
    /// /var/lib/cilium prevent Cilium DaemonSet from starting on a re-provisioned node.
    #[test]
    fn test_cleanup_removes_cilium_ebpf_state_and_dirs() {
        let script = generate_node_cleanup_script();
        // BPF filesystem Cilium maps
        assert!(
            script.contains("/sys/fs/bpf/cilium"),
            "must remove Cilium eBPF maps from /sys/fs/bpf/cilium"
        );
        // Cilium runtime state directory
        assert!(
            script.contains("/run/cilium"),
            "must remove Cilium runtime state dir /run/cilium"
        );
        // Cilium persistent state directory
        assert!(
            script.contains("/var/lib/cilium"),
            "must remove Cilium persistent state dir /var/lib/cilium"
        );
    }

    /// Sub-AC 2: CNI cleanup ordering — Cilium interfaces/eBPF removed before
    /// K8s packages are purged (so tools are still usable if needed).
    #[test]
    fn test_cleanup_cilium_before_k8s_packages_removed() {
        let script = generate_node_cleanup_script();
        let cilium_pos = script.find("cilium_host").unwrap();
        let pkg_purge_pos = script.find("apt-get purge").unwrap();
        assert!(
            cilium_pos < pkg_purge_pos,
            "Cilium interface cleanup must occur before K8s package purge"
        );
    }

    /// Sub-AC 2: etcd wipe must remove the primary data directory including
    /// WAL/ and snap/ subdirs. /var/lib/etcd is the default etcd data-dir
    /// used by Kubespray.
    #[test]
    fn test_cleanup_etcd_wipe_removes_primary_data_dir() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("rm -rf /var/lib/etcd"),
            "etcd wipe must remove /var/lib/etcd (contains WAL/ and snap/ subdirs)"
        );
    }

    /// Sub-AC 2: etcd wipe must remove the separate events store.
    /// When Kubernetes uses a dedicated etcd instance for events, data is written
    /// to /var/lib/etcd-events. Leftover data blocks cluster re-init.
    #[test]
    fn test_cleanup_etcd_wipe_removes_events_store() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("rm -rf /var/lib/etcd-events"),
            "etcd wipe must remove /var/lib/etcd-events (events store)"
        );
    }

    /// Sub-AC 2: etcd wipe must remove external etcd certificate directory.
    /// For stacked etcd setups, certificates may be in /etc/etcd (separate from
    /// /etc/kubernetes/pki). These must be wiped to allow fresh cert generation.
    #[test]
    fn test_cleanup_etcd_wipe_removes_external_cert_dir() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("rm -rf /etc/etcd"),
            "etcd wipe must remove /etc/etcd (external etcd certificate directory)"
        );
    }

    /// Sub-AC 2: etcd wipe phase label must precede generic K8s directory cleanup.
    /// Ensures etcd data is removed as part of the cluster teardown phase,
    /// not deferred to generic directory cleanup.
    #[test]
    fn test_cleanup_etcd_wipe_ordering() {
        let script = generate_node_cleanup_script();
        let etcd_wipe_pos = script.find("etcd wipe").unwrap();
        let k8s_dir_pos = script.find("Cleaning Kubernetes directories").unwrap();
        assert!(
            etcd_wipe_pos < k8s_dir_pos,
            "etcd wipe phase must precede generic K8s directory cleanup"
        );
    }

    /// Sub-AC 2: Privileged commands in the cleanup script must use sudo.
    /// The script runs via SSH as the admin user (non-root) with NOPASSWD sudo.
    /// Without sudo, kubeadm reset, systemctl, apt-get, etc. will fail with
    /// EACCES (permission denied) on the remote nodes.
    #[test]
    fn test_cleanup_privileged_commands_use_sudo() {
        let script = generate_node_cleanup_script();
        // kubeadm reset must use sudo
        assert!(
            script.contains("sudo kubeadm reset"),
            "kubeadm reset must use sudo (runs as non-root SSH user)"
        );
        // systemctl stop must use sudo
        assert!(
            script.contains("sudo systemctl stop kubelet"),
            "systemctl stop kubelet must use sudo"
        );
        assert!(
            script.contains("sudo systemctl stop libvirtd"),
            "systemctl stop libvirtd must use sudo"
        );
        // apt-get purge must use sudo
        assert!(
            script.contains("sudo apt-get purge"),
            "apt-get purge must use sudo"
        );
        // etcd data dir removal must use sudo
        assert!(
            script.contains("sudo rm -rf /var/lib/etcd"),
            "etcd data dir removal must use sudo"
        );
        // K8s directory cleanup must use sudo
        assert!(
            script.contains("sudo rm -rf /etc/kubernetes"),
            "K8s directory removal must use sudo"
        );
        // iptables flush must use sudo.
        // Script uses a table loop: `sudo iptables -t "$tbl" -F 2>/dev/null || true`
        // so we check for `sudo iptables -t` rather than the bare `sudo iptables -F`.
        assert!(
            script.contains("sudo iptables -t"),
            "iptables flush must use sudo (table-based loop pattern)"
        );
        // netplan apply must NOT be called — network changes would break SSH
        assert!(
            !script.contains("netplan apply"),
            "cleanup must never run netplan apply"
        );
    }

    /// Sub-AC 2: Full K8s teardown phase ordering.
    /// Correct order: kubeadm reset → CNI cleanup (Cilium) → etcd wipe → services stop.
    #[test]
    fn test_cleanup_k8s_teardown_full_phase_ordering() {
        let script = generate_node_cleanup_script();
        let kubeadm_pos = script.find("kubeadm reset").unwrap();
        let cilium_pos = script.find("cilium_host").unwrap();
        // Use etcd-events as the marker for Phase 1b (etcd wipe)
        let etcd_pos = script.find("etcd-events").unwrap();
        let services_stop_pos = script.find("Stopping Kubernetes services").unwrap();
        assert!(
            kubeadm_pos < cilium_pos,
            "kubeadm reset must precede Cilium CNI cleanup"
        );
        assert!(
            cilium_pos < etcd_pos,
            "Cilium CNI cleanup must precede etcd wipe"
        );
        assert!(
            etcd_pos < services_stop_pos,
            "etcd wipe must precede K8s services stop"
        );
    }
}
