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

# ─── Logging Setup ───────────────────────────────────────────────────────────
# Every phase marker is written to LOG_FILE with an ISO-8601 timestamp.
# The log persists on the remote node at /var/log/scalex-clean.log for
# post-hoc diagnosis. Falls back to /tmp if /var/log is not writable.
LOG_FILE=/var/log/scalex-clean.log
sudo touch "$LOG_FILE" 2>/dev/null && sudo chmod 644 "$LOG_FILE" 2>/dev/null \
    || LOG_FILE=/tmp/scalex-clean.log

log_step() {
    local msg="$1"
    local ts
    ts=$(date '+%Y-%m-%dT%H:%M:%S')
    echo "[scalex] ${msg}"
    printf '%s [scalex] %s\n' "${ts}" "${msg}" | sudo tee -a "${LOG_FILE}" >/dev/null
}
# ─────────────────────────────────────────────────────────────────────────────

log_step "=== Node Cleanup: Resetting to bare-metal state ==="
# SAFETY: Never modify network interfaces (br0/bond) — SSH depends on them.

# ─── Pre-flight: Enumerate and protect network interfaces ───
# Build the protected-interface list BEFORE any removal logic executes.
# Interfaces captured here will never be removed, even if their names happen
# to match a CNI/overlay pattern.
log_step "Pre-flight: Cataloguing network interfaces to protect..."

SCALEX_PROTECTED_IFACES=""
while IFS= read -r _iface_entry; do
    _iface=$(echo "$_iface_entry" | awk -F': ' '{print $2}' | cut -d'@' -f1)
    case "$_iface" in
        lo) ;;  # loopback — skip (not a data-plane iface)
        br0|br[0-9]*)
            SCALEX_PROTECTED_IFACES="$SCALEX_PROTECTED_IFACES $_iface"
            log_step "  PROTECTED (bridge): $_iface" ;;
        bond*)
            SCALEX_PROTECTED_IFACES="$SCALEX_PROTECTED_IFACES $_iface"
            log_step "  PROTECTED (bond): $_iface" ;;
        eth*|enp*|ens*|em*|eno*)
            SCALEX_PROTECTED_IFACES="$SCALEX_PROTECTED_IFACES $_iface"
            log_step "  PROTECTED (physical NIC): $_iface" ;;
    esac
done < <(ip -o link show 2>/dev/null || true)

# Explicitly protect the interface carrying the default route.
# This covers interfaces (e.g. a directly-assigned physical NIC or a tunnel)
# that carry the default route but were not already matched by the name-pattern
# loop above.  The awk extracts the 'dev <name>' token from the default route line.
_default_route_iface=$(ip route show default 2>/dev/null \
    | awk '/^default/ {for(i=1;i<=NF;i++) if ($i=="dev") {print $(i+1); exit}}' || true)
if [ -n "$_default_route_iface" ]; then
    _already_protected=0
    for _p in $SCALEX_PROTECTED_IFACES; do
        [ "$_p" = "$_default_route_iface" ] && _already_protected=1 && break
    done
    if [ "$_already_protected" -eq 0 ]; then
        SCALEX_PROTECTED_IFACES="$SCALEX_PROTECTED_IFACES $_default_route_iface"
        log_step "  PROTECTED (default-route carrier): $_default_route_iface"
    fi
fi
log_step "Pre-flight complete. Protected interfaces:${SCALEX_PROTECTED_IFACES:- (none detected)}"

# Helper: returns 0 (true) if the given interface name is in the protected list.
_scalex_is_protected() {
    local _check="$1"
    for _p in $SCALEX_PROTECTED_IFACES; do
        [ "$_check" = "$_p" ] && return 0
    done
    return 1
}

# Early-exit guard: abort the cleanup script immediately if the named interface
# is in the protected list.  Use this before any explicit (non-loop) ip link delete
# call so that a mismatch between expected and actual network topology never
# silently removes a management interface.  On match, logs a CRITICAL banner and
# terminates with a non-zero exit code — no further cleanup steps execute.
_scalex_abort_if_protected() {
    local _check="$1"
    if _scalex_is_protected "$_check"; then
        log_step "CRITICAL: attempt to remove protected interface '$_check' — aborting cleanup to preserve SSH/network"
        exit 1
    fi
}

# ─── Pre-clean network verification ─────────────────────────────────────────
# Assert br0 and the default route exist BEFORE any cleanup logic executes.
# If either check fails the node is already in a degraded network state — we
# refuse to proceed so we do not make a broken node worse, and to alert the
# operator immediately rather than after destructive changes have been made.
# Exit code 2 signals a pre-clean network pre-condition failure.
log_step "Pre-clean network verification: asserting br0 and default route..."
_SCALEX_PRECLEAN_FAIL=0
if ip link show br0 &>/dev/null; then
    log_step "  PRE-CLEAN OK: br0 interface present"
else
    log_step "  PRE-CLEAN CRITICAL: br0 interface NOT found — refusing to proceed with cleanup"
    _SCALEX_PRECLEAN_FAIL=1
fi
if ip route show default 2>/dev/null | grep -q 'default'; then
    log_step "  PRE-CLEAN OK: default route present"
else
    log_step "  PRE-CLEAN CRITICAL: default route NOT found — refusing to proceed with cleanup"
    _SCALEX_PRECLEAN_FAIL=1
fi
if [ "$_SCALEX_PRECLEAN_FAIL" -ne 0 ]; then
    log_step "ABORT: pre-clean network verification failed — br0 or default route missing. Cleanup refused."
    exit 2
fi
log_step "Pre-clean network verification passed. Proceeding with cleanup."

# ─── Phase 1: Kubernetes cluster teardown ───
log_step "Phase 1: K8s cluster teardown..."

if command -v kubeadm &>/dev/null; then
    log_step "Running kubeadm reset (with --cleanup-tmp-dir)..."
    # Auto-detect CRI socket: prefer containerd socket, fall back to kubeadm default
    if [ -S /run/containerd/containerd.sock ]; then
        sudo kubeadm reset -f --cleanup-tmp-dir \
            --cri-socket unix:///run/containerd/containerd.sock 2>/dev/null || true
    else
        sudo kubeadm reset -f --cleanup-tmp-dir 2>/dev/null || true
    fi
fi

# ─── Phase 1a: CNI cleanup (Cilium) ───
log_step "Phase 1a: CNI cleanup — removing Cilium interfaces and eBPF state..."

# Remove Cilium virtual network interfaces
for iface in cilium_host cilium_net cilium_vxlan; do
    if ip link show "$iface" &>/dev/null; then
        # Abort immediately if a protected interface somehow matches a Cilium name
        _scalex_abort_if_protected "$iface"
        sudo ip link set "$iface" down 2>/dev/null || true
        sudo ip link delete "$iface" 2>/dev/null || true
        log_step "  Removed Cilium interface: $iface"
    fi
done

# Remove per-pod lxc* virtual interfaces created by Cilium
for iface in $(ip link show 2>/dev/null | grep -oP '(?<=\d: )lxc[^\s@:]+' 2>/dev/null || true); do
    _scalex_abort_if_protected "$iface"
    sudo ip link set "$iface" down 2>/dev/null || true
    sudo ip link delete "$iface" 2>/dev/null || true
done

# Remove CNI/overlay virtual interfaces (Flannel VXLAN, Calico VXLAN, veth pairs, dummy)
# SAFETY: Never modify network interfaces (br0/bond) — SSH depends on them.
# Only matches prefixes that are exclusively K8s/CNI-created (never physical or bridge).
# Double-guarded: regex whitelist AND pre-flight protected-interface check.
while IFS= read -r virt_iface; do
    [ -z "$virt_iface" ] && continue
    if _scalex_is_protected "$virt_iface"; then
        log_step "  SKIP (protected interface, will not remove): $virt_iface"
        continue
    fi
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
log_step "Phase 1b: etcd wipe — removing data dir, WAL, snapshots, member data..."

# Primary etcd data directory (contains WAL/ and snap/ subdirs for member data)
sudo rm -rf /var/lib/etcd
# Separate etcd events store (used when --experimental-backend-quota-bytes is split)
sudo rm -rf /var/lib/etcd-events
# External etcd certificate directory (separate from /etc/kubernetes for stacked etcd)
sudo rm -rf /etc/etcd

log_step "Stopping Kubernetes services..."
sudo systemctl stop kubelet 2>/dev/null || true
sudo systemctl stop containerd 2>/dev/null || true
sudo systemctl disable kubelet 2>/dev/null || true
sudo systemctl disable containerd 2>/dev/null || true

log_step "Removing Kubernetes packages..."
sudo apt-get purge -y -qq kubeadm kubelet kubectl containerd.io 2>/dev/null || true
sudo apt-get purge -y -qq kubernetes-cni cri-tools 2>/dev/null || true

log_step "Cleaning Kubernetes directories..."
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

log_step "Flushing iptables rules (all tables, policy reset to ACCEPT)..."
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
log_step "Phase 2: Removing KVM/libvirt..."

if command -v virsh &>/dev/null; then
    VIRSH="virsh -c qemu:///system"

    log_step "Stopping all running VM domains..."
    for vm in $($VIRSH list --state-running --name 2>/dev/null | grep -v '^$'); do
        $VIRSH destroy "$vm" 2>/dev/null || true
    done

    log_step "Removing storage volumes from all pools..."
    for pool in $($VIRSH pool-list --all --name 2>/dev/null | grep -v '^$'); do
        $VIRSH pool-refresh "$pool" 2>/dev/null || true
        for vol in $($VIRSH vol-list "$pool" 2>/dev/null | tail -n +3 | awk '{print $1}' | grep -v '^$'); do
            $VIRSH vol-delete "$vol" --pool "$pool" 2>/dev/null || true
        done
        $VIRSH pool-destroy "$pool" 2>/dev/null || true
        $VIRSH pool-undefine "$pool" 2>/dev/null || true
    done

    log_step "Undefining all VM domains..."
    for vm in $($VIRSH list --all --name 2>/dev/null | grep -v '^$'); do
        $VIRSH undefine "$vm" --nvram 2>/dev/null || \
            $VIRSH undefine "$vm" 2>/dev/null || true
    done

    $VIRSH net-destroy default 2>/dev/null || true
    $VIRSH net-undefine default 2>/dev/null || true
fi

log_step "Stopping libvirt services..."
sudo systemctl stop libvirtd 2>/dev/null || true
sudo systemctl disable libvirtd 2>/dev/null || true

log_step "Removing KVM/libvirt packages..."
# SAFETY: bridge-utils is intentionally excluded — it provides brctl which may be
# needed by br0/bond0. Removing it can disrupt network interfaces SSH depends on.
sudo apt-get purge -y -qq qemu-kvm libvirt-daemon-system libvirt-clients virtinst 2>/dev/null || true
sudo rm -rf /var/lib/libvirt || true
sudo rm -rf /etc/libvirt || true

# ─── Phase 3: Final cleanup ───
# SAFETY: Never modify network interfaces (br0/bond) — SSH depends on them.
# Only libvirt's own virtual network interfaces (virbr*, vnet*) were already
# removed above by virsh net-destroy. br0, bond0, and physical NICs are untouched.
log_step "Phase 3: Cleaning up remaining packages..."

# SAFETY: Pin network-critical packages as manually installed.
# Orphan package removal (e.g. apt-get with --autoremove flag) is intentionally
# NEVER used here — it risks removing bridge-utils and ebtables that br0/bond0
# network interfaces depend on. SSH connectivity would be lost. We pin explicitly.
# bridge-utils provides brctl needed for br0; ebtables provides bridge netfilter.
log_step "Pinning network-critical packages (explicit list, no orphan removal)..."
for pkg in bridge-utils ebtables iptables iproute2 netplan.io systemd; do
    sudo apt-mark manual "$pkg" 2>/dev/null || true
done

sudo apt-get clean

# ─── Post-clean network verification ────────────────────────────────────────
# Assert br0 and the default route are STILL present after all cleanup phases.
# If either is now missing, the cleanup violated network safety — fail loudly so
# the operator knows to restore connectivity before attempting reprovisioning.
# Exit code 3 signals a post-clean network regression.
# NOTE: Also satisfies the `ip link show br0` / `ip route show default` assertions
# checked by test_cleanup_verifies_network_after_package_ops.
log_step "Verifying network connectivity after cleanup..."
_SCALEX_POSTCLEAN_FAIL=0
if ip link show br0 &>/dev/null; then
    log_step "  POST-CLEAN OK: br0 interface present"
else
    log_step "  POST-CLEAN CRITICAL: br0 interface LOST after cleanup — SSH/network may be broken!"
    _SCALEX_POSTCLEAN_FAIL=1
fi
if ip route show default 2>/dev/null | grep -q 'default'; then
    log_step "  POST-CLEAN OK: default route present"
else
    log_step "  POST-CLEAN CRITICAL: default route LOST after cleanup — network broken!"
    _SCALEX_POSTCLEAN_FAIL=1
fi
if [ "$_SCALEX_POSTCLEAN_FAIL" -ne 0 ]; then
    log_step "ABORT: post-clean network verification failed — br0 or default route was lost during cleanup!"
    exit 3
fi
log_step "Post-clean network verification passed."

log_step "=== Node cleanup complete. Network interfaces (br0/bond) preserved. ==="
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
            reachable_node_port: None,
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
            reachable_node_port: None,
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
            reachable_node_port: None,
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

    // ===== Sub-AC 5: Idempotent re-run of `scalex-pod sdi clean --hard` =====
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
        // apt-get autoremove must NOT be present (network safety: it can remove
        // bridge-utils/ebtables that br0/bond0 depend on)
        assert!(
            !script.contains("apt-get autoremove"),
            "apt-get autoremove must be completely absent from cleanup script"
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

    #[test]
    fn test_cleanup_pins_network_packages_no_autoremove() {
        let script = generate_node_cleanup_script();
        // apt-mark manual must be present to pin network-critical packages
        assert!(
            script.contains("apt-mark manual"),
            "cleanup must pin network packages via apt-mark manual"
        );
        // apt-get autoremove must be completely absent — it is unsafe on bare-metal
        // because it can remove bridge-utils/ebtables that br0/bond0 depend on.
        assert!(
            !script.contains("apt-get autoremove"),
            "apt-get autoremove must be completely absent (network safety requirement)"
        );
        // Verify critical network packages are pinned
        for pkg in &["bridge-utils", "ebtables", "iptables", "iproute2", "netplan.io"] {
            assert!(
                script.contains(pkg),
                "cleanup must pin network-critical package: {}",
                pkg
            );
        }
    }

    #[test]
    fn test_cleanup_verifies_network_after_package_ops() {
        let script = generate_node_cleanup_script();
        // Network verification must be present after package operations
        assert!(
            script.contains("Verifying network connectivity"),
            "cleanup must verify network connectivity after package operations"
        );
        // apt-get autoremove must be completely absent
        assert!(
            !script.contains("apt-get autoremove"),
            "apt-get autoremove must be completely absent (network safety requirement)"
        );
        // apt-get clean (cache only, no package removal) must still run
        assert!(
            script.contains("apt-get clean"),
            "cleanup must run apt-get clean to free cache"
        );
        assert!(
            script.contains("ip link show br0"),
            "cleanup must check br0 interface after package operations"
        );
        assert!(
            script.contains("ip route show default"),
            "cleanup must check default route after package operations"
        );
    }

    // ===== Sub-AC 2 pre-flight: Protected-interface enumeration =====
    // These tests verify the pre-flight section that catalogues br0/bond/physical
    // NIC interfaces BEFORE any removal logic executes, and that subsequent
    // removal loops respect the resulting protected list.

    /// Pre-flight cataloguing must occur before Phase 1 K8s removal begins.
    /// The protected-interface list must be built at script start — before any
    /// `ip link delete`, kubeadm reset, or package purge runs.
    #[test]
    fn test_cleanup_preflight_catalogues_before_phase1_removal() {
        let script = generate_node_cleanup_script();
        let preflight_pos = script
            .find("Pre-flight: Cataloguing network interfaces")
            .expect("cleanup must include pre-flight interface cataloguing section");
        let phase1_pos = script
            .find("Phase 1: K8s cluster teardown")
            .expect("cleanup must include Phase 1 K8s teardown");
        assert!(
            preflight_pos < phase1_pos,
            "pre-flight interface cataloguing must occur BEFORE Phase 1 removal logic"
        );
    }

    /// SCALEX_PROTECTED_IFACES variable must be initialised before Phase 1.
    /// Any removal loop that references the variable requires it to exist first.
    #[test]
    fn test_cleanup_preflight_variable_set_before_phase1() {
        let script = generate_node_cleanup_script();
        let var_set_pos = script
            .find("SCALEX_PROTECTED_IFACES")
            .expect("SCALEX_PROTECTED_IFACES must be set in cleanup script");
        let phase1_pos = script
            .find("Phase 1: K8s cluster teardown")
            .expect("Phase 1 must exist");
        assert!(
            var_set_pos < phase1_pos,
            "SCALEX_PROTECTED_IFACES must be initialised before Phase 1 removal begins"
        );
    }

    /// Pre-flight must detect and protect bridge interfaces (br0, br*).
    /// Bridge interfaces are the primary management-network carrier on bare-metal.
    #[test]
    fn test_cleanup_preflight_protects_bridge_interfaces() {
        let script = generate_node_cleanup_script();
        // Pattern matching for br0 and br[0-9]* must appear in the pre-flight block
        assert!(
            script.contains("br0") && script.contains("br[0-9]"),
            "pre-flight must detect and protect br0 and br[0-9]* bridge interfaces"
        );
        // Must log the PROTECTED (bridge) label
        assert!(
            script.contains("PROTECTED (bridge)"),
            "pre-flight must log 'PROTECTED (bridge)' for detected bridge interfaces"
        );
    }

    /// Pre-flight must detect and protect bond interfaces (bond*).
    /// Bond interfaces aggregate physical NICs; removing them severs SSH.
    #[test]
    fn test_cleanup_preflight_protects_bond_interfaces() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("bond*)"),
            "pre-flight must match bond* interfaces in case statement"
        );
        assert!(
            script.contains("PROTECTED (bond)"),
            "pre-flight must log 'PROTECTED (bond)' for detected bond interfaces"
        );
    }

    /// Pre-flight must detect and protect physical NIC prefixes (eth*, enp*, ens*, em*, eno*).
    /// Physical NICs are the ultimate SSH transport; they must never be removed.
    #[test]
    fn test_cleanup_preflight_protects_physical_nics() {
        let script = generate_node_cleanup_script();
        // All common physical NIC naming conventions must be covered
        for prefix in &["eth*", "enp*", "ens*", "em*", "eno*"] {
            assert!(
                script.contains(prefix),
                "pre-flight must detect and protect physical NIC prefix: {prefix}"
            );
        }
        assert!(
            script.contains("PROTECTED (physical NIC)"),
            "pre-flight must log 'PROTECTED (physical NIC)' for detected physical NICs"
        );
    }

    /// The _scalex_is_protected() helper function must be defined in the script.
    /// It provides a reusable check used by each removal loop.
    #[test]
    fn test_cleanup_preflight_defines_is_protected_helper() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("_scalex_is_protected()"),
            "cleanup script must define _scalex_is_protected() helper function"
        );
        // Helper must be defined before the CNI virtual-interface loop uses it
        let helper_pos = script
            .find("_scalex_is_protected()")
            .expect("helper must be defined");
        let cni_loop_pos = script
            .find("_scalex_is_protected \"$virt_iface\"")
            .expect("CNI loop must call _scalex_is_protected");
        assert!(
            helper_pos < cni_loop_pos,
            "_scalex_is_protected() must be defined before its first call site"
        );
    }

    /// CNI virtual interface removal loop must call _scalex_is_protected() and skip
    /// any interface found in the protected list.
    #[test]
    fn test_cleanup_cni_loop_skips_protected_interfaces() {
        let script = generate_node_cleanup_script();
        // The CNI virtual-interface loop must contain the protection guard
        assert!(
            script.contains("_scalex_is_protected \"$virt_iface\""),
            "CNI virtual interface removal loop must guard each interface with _scalex_is_protected"
        );
        // Must emit a SKIP log entry when a protected interface is encountered
        assert!(
            script.contains("SKIP (protected interface"),
            "CNI loop must log 'SKIP (protected interface ...)' when skipping a protected iface"
        );
        // Protection check must appear inside the virt_iface loop body (before the delete)
        let guard_pos = script
            .find("_scalex_is_protected \"$virt_iface\"")
            .unwrap();
        let delete_pos = script
            .find("sudo ip link delete \"$virt_iface\"")
            .expect("CNI loop must still contain the ip link delete command");
        assert!(
            guard_pos < delete_pos,
            "protection guard must appear before ip link delete in the CNI loop"
        );
    }

    // ===== Sub-AC 3: Per-step timestamped logging to /var/log/scalex-clean.log =====

    /// Sub-AC 3: Cleanup script must define LOG_FILE pointing to /var/log/scalex-clean.log.
    /// This is the persistent per-node log for post-hoc diagnosis of cleanup runs.
    #[test]
    fn test_cleanup_log_file_path_is_var_log() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("LOG_FILE=/var/log/scalex-clean.log"),
            "cleanup script must set LOG_FILE=/var/log/scalex-clean.log"
        );
    }

    /// Sub-AC 3: Cleanup script must define a log_step() helper function.
    /// log_step() is responsible for writing each phase marker to both stdout
    /// and the persistent log file on the remote node.
    #[test]
    fn test_cleanup_defines_log_step_function() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("log_step()"),
            "cleanup script must define log_step() helper function"
        );
    }

    /// Sub-AC 3: log_step() must emit a timestamp on every call.
    /// Timestamps use ISO-8601 format (date '+%Y-%m-%dT%H:%M:%S') to make log
    /// entries sortable and unambiguous for diagnosis.
    #[test]
    fn test_cleanup_log_step_emits_timestamp() {
        let script = generate_node_cleanup_script();
        // Must capture a timestamp inside the function
        assert!(
            script.contains("date '+%Y-%m-%dT%H:%M:%S'"),
            "log_step() must use date '+%Y-%m-%dT%H:%M:%S' for ISO-8601 timestamps"
        );
    }

    /// Sub-AC 3: log_step() must write to the log file via tee -a.
    /// Using `tee -a` appends each entry without truncating the log, and lets
    /// the timestamp entry appear in both stdout (for SSH session visibility) and
    /// the persistent log file on the remote node.
    #[test]
    fn test_cleanup_log_step_appends_to_log_file() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("tee -a \"${LOG_FILE}\""),
            "log_step() must append to LOG_FILE via tee -a"
        );
    }

    /// Sub-AC 3: log_step() must still emit [scalex] prefix to stdout.
    /// Existing tooling (install.sh, test assertions) relies on the [scalex] prefix
    /// being present in SSH session output.
    #[test]
    fn test_cleanup_log_step_emits_scalex_prefix_to_stdout() {
        let script = generate_node_cleanup_script();
        // The log_step function body must echo the [scalex] prefix
        assert!(
            script.contains("echo \"[scalex] ${msg}\""),
            "log_step() must echo '[scalex] <msg>' to stdout"
        );
    }

    /// Sub-AC 3: LOG_FILE setup must precede all phase markers.
    /// The log file variable and log_step function must be defined before any
    /// log_step call so the first phase marker is always captured in the log.
    #[test]
    fn test_cleanup_log_setup_before_first_phase_marker() {
        let script = generate_node_cleanup_script();
        let log_file_pos = script
            .find("LOG_FILE=/var/log/scalex-clean.log")
            .expect("LOG_FILE must be defined");
        let first_log_step_pos = script
            .find("log_step \"=== Node Cleanup")
            .expect("first log_step call must be present");
        assert!(
            log_file_pos < first_log_step_pos,
            "LOG_FILE must be set before the first log_step call"
        );
    }

    /// Sub-AC 3: log_step() function definition must precede its first call site.
    #[test]
    fn test_cleanup_log_step_defined_before_first_call() {
        let script = generate_node_cleanup_script();
        let fn_def_pos = script
            .find("log_step()")
            .expect("log_step() must be defined");
        let first_call_pos = script
            .find("log_step \"=== Node Cleanup")
            .expect("first log_step call must exist");
        assert!(
            fn_def_pos < first_call_pos,
            "log_step() must be defined before its first call site"
        );
    }

    /// Sub-AC 3: Every major phase header must use log_step, not plain echo.
    /// This ensures each phase is captured in /var/log/scalex-clean.log with
    /// a timestamp, making it possible to diagnose partial-failure scenarios.
    #[test]
    fn test_cleanup_phase_headers_use_log_step() {
        let script = generate_node_cleanup_script();
        // Each phase header should appear as a log_step argument, not a bare echo
        let phases = [
            "Phase 1: K8s cluster teardown",
            "Phase 1a: CNI cleanup",
            "Phase 1b: etcd wipe",
            "Phase 2: Removing KVM/libvirt",
            "Phase 3: Cleaning up remaining packages",
        ];
        for phase in &phases {
            // Must appear as argument to log_step (quoted string)
            let log_step_call = format!("log_step \"{phase}");
            assert!(
                script.contains(&log_step_call),
                "phase header must use log_step: {phase}"
            );
            // Must NOT appear as argument to a bare echo call
            let bare_echo = format!("echo \"[scalex] {phase}");
            assert!(
                !script.contains(&bare_echo),
                "phase header must not use bare echo (use log_step instead): {phase}"
            );
        }
    }

    /// Sub-AC 3: log_step() must fall back to /tmp/scalex-clean.log if
    /// /var/log is not writable (e.g. read-only rootfs or permission denied).
    #[test]
    fn test_cleanup_log_file_has_writable_fallback() {
        let script = generate_node_cleanup_script();
        // Must attempt to create the log file with sudo touch
        assert!(
            script.contains("sudo touch \"$LOG_FILE\""),
            "log setup must create LOG_FILE with sudo touch"
        );
        // Must fall back to /tmp if /var/log is not writable
        assert!(
            script.contains("LOG_FILE=/tmp/scalex-clean.log"),
            "log setup must fall back to /tmp/scalex-clean.log if /var/log is not writable"
        );
    }

    // ===== Sub-AC 6b: Interface-protection guards — default route + early-exit =====

    /// Sub-AC 6b: Pre-flight must detect and protect the interface carrying the
    /// default route.  A bare-metal node may use an interface (e.g. a direct NIC
    /// that does not match br0/bond*/eth*/enp* patterns) as its default-route
    /// carrier.  Removing that interface would sever SSH.
    #[test]
    fn test_cleanup_preflight_protects_default_route_carrier() {
        let script = generate_node_cleanup_script();
        // Must log the PROTECTED (default-route carrier) label when detected
        assert!(
            script.contains("PROTECTED (default-route carrier)"),
            "pre-flight must log 'PROTECTED (default-route carrier)' for the default-route interface"
        );
    }

    /// Sub-AC 6b: Default-route interface detection must use `ip route show default`
    /// and extract the 'dev <name>' field via awk.
    #[test]
    fn test_cleanup_default_route_detection_uses_ip_route_show() {
        let script = generate_node_cleanup_script();
        // Must query the routing table for the default route
        assert!(
            script.contains("ip route show default"),
            "pre-flight must call 'ip route show default' to detect the default-route interface"
        );
        // Must extract the 'dev' token to get the interface name
        assert!(
            script.contains("\"dev\""),
            "pre-flight awk must match the 'dev' token in the default route line"
        );
    }

    /// Sub-AC 6b: Default-route detection must occur before 'Pre-flight complete' log.
    /// The protected-interface list must be fully built before the completion message.
    #[test]
    fn test_cleanup_default_route_detection_before_preflight_complete() {
        let script = generate_node_cleanup_script();
        let detect_pos = script
            .find("ip route show default")
            .expect("ip route show default must be present");
        let complete_pos = script
            .find("Pre-flight complete.")
            .expect("Pre-flight complete message must be present");
        assert!(
            detect_pos < complete_pos,
            "default-route detection must occur before 'Pre-flight complete' log"
        );
    }

    /// Sub-AC 6b: The `_scalex_abort_if_protected()` early-exit guard function
    /// must be defined in the script.
    #[test]
    fn test_cleanup_defines_abort_if_protected() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("_scalex_abort_if_protected()"),
            "cleanup script must define _scalex_abort_if_protected() early-exit guard"
        );
    }

    /// Sub-AC 6b: `_scalex_abort_if_protected()` must call `exit 1` to terminate
    /// the entire cleanup script when a protected interface is about to be touched.
    #[test]
    fn test_cleanup_abort_if_protected_exits_on_match() {
        let script = generate_node_cleanup_script();
        // Must contain exit 1 inside the abort guard context
        assert!(
            script.contains("exit 1"),
            "_scalex_abort_if_protected() must call exit 1 to terminate the script"
        );
        // exit 1 must appear after the CRITICAL log message (ordered within the function)
        let critical_pos = script
            .find("CRITICAL: attempt to remove protected interface")
            .expect("abort guard must log a CRITICAL message");
        let exit1_pos = script
            .find("exit 1")
            .expect("abort guard must contain exit 1");
        assert!(
            critical_pos < exit1_pos,
            "CRITICAL log message must precede exit 1 in the abort guard"
        );
    }

    /// Sub-AC 6b: `_scalex_abort_if_protected()` must emit a CRITICAL banner that
    /// names the interface and explains the abort reason.
    #[test]
    fn test_cleanup_abort_if_protected_logs_critical_message() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("CRITICAL: attempt to remove protected interface"),
            "abort guard must log a CRITICAL message naming the offending interface"
        );
        assert!(
            script.contains("aborting cleanup to preserve SSH/network"),
            "abort guard message must explain the abort reason (SSH/network preservation)"
        );
    }

    /// Sub-AC 6b: `_scalex_abort_if_protected()` must be defined AFTER
    /// `_scalex_is_protected()` (since it delegates to it) and BEFORE Phase 1.
    #[test]
    fn test_cleanup_abort_if_protected_defined_before_phase1() {
        let script = generate_node_cleanup_script();
        let abort_def_pos = script
            .find("_scalex_abort_if_protected()")
            .expect("_scalex_abort_if_protected() must be defined");
        let phase1_pos = script
            .find("Phase 1: K8s cluster teardown")
            .expect("Phase 1 must exist");
        assert!(
            abort_def_pos < phase1_pos,
            "_scalex_abort_if_protected() must be defined before Phase 1 removal begins"
        );
        // Must also be defined after _scalex_is_protected (dependency order)
        let is_protected_def_pos = script
            .find("_scalex_is_protected()")
            .expect("_scalex_is_protected() must be defined");
        assert!(
            is_protected_def_pos < abort_def_pos,
            "_scalex_abort_if_protected() must be defined after _scalex_is_protected()"
        );
    }

    /// Sub-AC 6b: The Cilium named-interface removal loop must call
    /// `_scalex_abort_if_protected` before attempting to delete each interface.
    /// This guards against the (unlikely but catastrophic) case where a node
    /// names a protected interface with a Cilium-like name.
    #[test]
    fn test_cleanup_cilium_loop_uses_abort_guard() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("_scalex_abort_if_protected \"$iface\""),
            "Cilium interface removal loop must call _scalex_abort_if_protected for each iface"
        );
        // The abort guard must precede the ip link delete in the loop body
        let guard_pos = script
            .find("_scalex_abort_if_protected \"$iface\"")
            .expect("abort guard call must exist");
        let delete_pos = script
            .find("sudo ip link delete \"$iface\"")
            .expect("ip link delete must exist in Cilium loop");
        assert!(
            guard_pos < delete_pos,
            "_scalex_abort_if_protected must be called before ip link delete"
        );
    }

    // ===== Sub-AC 6c: Pre-clean and post-clean br0 + default route verification =====
    // These tests verify that the cleanup script asserts br0 and the default route are
    // present both BEFORE and AFTER the cleanup phases, and fails loudly (non-zero exit)
    // if either check is violated.

    /// Sub-AC 6c: Pre-clean verification must be present in the cleanup script.
    /// The block must log a recognisable header so operators can locate it in
    /// /var/log/scalex-clean.log when diagnosing failures.
    #[test]
    fn test_cleanup_preclean_verification_present() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("Pre-clean network verification"),
            "cleanup script must include a pre-clean network verification block"
        );
    }

    /// Sub-AC 6c: Pre-clean verification must assert br0 exists via `ip link show br0`.
    /// br0 is the management-network bridge; its absence before cleanup means the
    /// node is already in a degraded state and cleanup should refuse to proceed.
    #[test]
    fn test_cleanup_preclean_checks_br0() {
        let script = generate_node_cleanup_script();
        // ip link show br0 must appear in the pre-clean block (before Phase 1)
        let preclean_pos = script
            .find("Pre-clean network verification")
            .expect("pre-clean block must be present");
        let phase1_pos = script
            .find("Phase 1: K8s cluster teardown")
            .expect("Phase 1 must be present");
        // ip link show br0 must exist somewhere in the script (checked by other tests too)
        assert!(
            script.contains("ip link show br0"),
            "pre-clean verification must check br0 via 'ip link show br0'"
        );
        // The check must appear within the pre-clean block (before Phase 1)
        let br0_check_pos = script.find("ip link show br0").expect("ip link show br0 must exist");
        assert!(
            preclean_pos < br0_check_pos && br0_check_pos < phase1_pos,
            "ip link show br0 check must be inside the pre-clean block (after pre-clean header, before Phase 1)"
        );
    }

    /// Sub-AC 6c: Pre-clean verification must assert the default route is present.
    /// No default route before cleanup means the node is already unreachable; abort.
    #[test]
    fn test_cleanup_preclean_checks_default_route() {
        let script = generate_node_cleanup_script();
        let preclean_pos = script
            .find("Pre-clean network verification")
            .expect("pre-clean block must be present");
        let phase1_pos = script
            .find("Phase 1: K8s cluster teardown")
            .expect("Phase 1 must be present");
        // Check that the pre-clean block (substring between header and Phase 1) contains
        // the default-route check.  We use the substring to avoid matching the earlier
        // occurrence of 'ip route show default' that lives in the pre-flight section.
        let preclean_block = &script[preclean_pos..phase1_pos];
        assert!(
            preclean_block.contains("ip route show default"),
            "pre-clean block must contain 'ip route show default' to assert the default route"
        );
    }

    /// Sub-AC 6c: Pre-clean verification must fail loudly with exit 2 when
    /// br0 or the default route is missing.  A non-zero exit ensures `set -e`
    /// propagates the failure back to the caller.
    #[test]
    fn test_cleanup_preclean_fails_with_exit2() {
        let script = generate_node_cleanup_script();
        // exit 2 must be present (reserved for pre-clean failures)
        assert!(
            script.contains("exit 2"),
            "pre-clean verification must use 'exit 2' to signal a pre-condition failure"
        );
        // The abort message must be present so operators understand why cleanup stopped
        assert!(
            script.contains("pre-clean network verification failed"),
            "pre-clean abort path must log 'pre-clean network verification failed'"
        );
        // exit 2 must appear after the CRITICAL log messages (structured abort flow)
        let critical_pos = script
            .find("PRE-CLEAN CRITICAL")
            .expect("pre-clean CRITICAL message must be present");
        let exit2_pos = script.find("exit 2").expect("exit 2 must be present");
        assert!(
            critical_pos < exit2_pos,
            "PRE-CLEAN CRITICAL log must precede exit 2 in the pre-clean block"
        );
    }

    /// Sub-AC 6c: Pre-clean verification must run AFTER the pre-flight interface
    /// cataloguing (so the protected-interface list is built) but BEFORE Phase 1
    /// (so no cleanup has occurred yet).
    #[test]
    fn test_cleanup_preclean_ordering() {
        let script = generate_node_cleanup_script();
        let preflight_pos = script
            .find("Pre-flight complete.")
            .expect("pre-flight complete message must be present");
        let preclean_pos = script
            .find("Pre-clean network verification")
            .expect("pre-clean block must be present");
        let phase1_pos = script
            .find("Phase 1: K8s cluster teardown")
            .expect("Phase 1 must be present");
        assert!(
            preflight_pos < preclean_pos,
            "pre-clean verification must run AFTER pre-flight interface cataloguing"
        );
        assert!(
            preclean_pos < phase1_pos,
            "pre-clean verification must run BEFORE Phase 1 removal"
        );
    }

    /// Sub-AC 6c: Post-clean verification must fail loudly with exit 3 when
    /// br0 or the default route is missing after cleanup.
    /// Merely logging CRITICAL is insufficient — the script must exit non-zero
    /// so the caller (install.sh / sdi clean orchestrator) detects the regression.
    #[test]
    fn test_cleanup_postclean_fails_with_exit3() {
        let script = generate_node_cleanup_script();
        // exit 3 must be present (reserved for post-clean regressions)
        assert!(
            script.contains("exit 3"),
            "post-clean verification must use 'exit 3' to signal a post-cleanup network regression"
        );
        // The abort message must identify the phase
        assert!(
            script.contains("post-clean network verification failed"),
            "post-clean abort path must log 'post-clean network verification failed'"
        );
        // exit 3 must appear after the POST-CLEAN CRITICAL log
        let critical_pos = script
            .find("POST-CLEAN CRITICAL")
            .expect("post-clean CRITICAL message must be present");
        let exit3_pos = script.find("exit 3").expect("exit 3 must be present");
        assert!(
            critical_pos < exit3_pos,
            "POST-CLEAN CRITICAL log must precede exit 3 in the post-clean block"
        );
    }

    /// Sub-AC 6c: Post-clean verification must check br0 still exists.
    /// br0 carries SSH; losing it after cleanup means the script violated network safety.
    #[test]
    fn test_cleanup_postclean_checks_br0() {
        let script = generate_node_cleanup_script();
        // POST-CLEAN OK/CRITICAL messages must reference br0
        assert!(
            script.contains("POST-CLEAN OK: br0 interface present")
                || script.contains("POST-CLEAN CRITICAL: br0 interface LOST"),
            "post-clean block must log a br0 OK or CRITICAL result"
        );
    }

    /// Sub-AC 6c: Post-clean verification must check the default route still exists.
    #[test]
    fn test_cleanup_postclean_checks_default_route() {
        let script = generate_node_cleanup_script();
        assert!(
            script.contains("POST-CLEAN OK: default route present")
                || script.contains("POST-CLEAN CRITICAL: default route LOST"),
            "post-clean block must log a default route OK or CRITICAL result"
        );
    }

    /// Sub-AC 6c: Post-clean verification must run AFTER Phase 3 (the last cleanup phase)
    /// and BEFORE the final completion message.
    #[test]
    fn test_cleanup_postclean_ordering() {
        let script = generate_node_cleanup_script();
        let phase3_pos = script
            .find("Phase 3: Cleaning up remaining packages")
            .expect("Phase 3 must be present");
        // "Verifying network connectivity" is used by the existing test contract too
        let postclean_pos = script
            .find("Verifying network connectivity after cleanup")
            .expect("post-clean 'Verifying network connectivity after cleanup' must be present");
        let complete_pos = script
            .find("Node cleanup complete")
            .expect("completion message must be present");
        assert!(
            phase3_pos < postclean_pos,
            "post-clean verification must run AFTER Phase 3"
        );
        assert!(
            postclean_pos < complete_pos,
            "post-clean verification must run BEFORE the completion message"
        );
    }

    /// Sub-AC 6c: Exit codes must be distinct — pre-clean uses 2, post-clean uses 3,
    /// and pre-flight abort (protecting interfaces) uses 1.  Distinct codes allow the
    /// caller to distinguish failure modes without parsing log output.
    #[test]
    fn test_cleanup_exit_codes_are_distinct() {
        let script = generate_node_cleanup_script();
        // exit 1 — protected-interface abort (_scalex_abort_if_protected)
        assert!(script.contains("exit 1"), "exit 1 must be used for protected-interface abort");
        // exit 2 — pre-clean network pre-condition failure
        assert!(script.contains("exit 2"), "exit 2 must be used for pre-clean failure");
        // exit 3 — post-clean network regression
        assert!(script.contains("exit 3"), "exit 3 must be used for post-clean failure");
    }
}
