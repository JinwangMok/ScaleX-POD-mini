# /scalex-full-reset

Complete clean-state re-provisioning: kill stale tunnels → sdi clean → install.sh --auto.

## When to Use

- Full E2E re-provisioning from clean state
- After code changes that affect SDI/cluster/bootstrap pipeline
- "clean and reinstall", "re-provision from scratch", "sdi reset"

## Steps

Execute the following steps sequentially. Do NOT skip any step.

### 1. Kill Stale SSH Tunnels

Kill any lingering SSH tunnels on SDI ports (22101-22110) and K8s API ports (16443-16444):

```bash
for port in 22101 22102 22103 22104 22105 22106 22107 22108 22109 22110 16443 16444; do
  pids=$(lsof -ti ":$port" 2>/dev/null || true)
  if [ -n "$pids" ]; then
    kill $pids 2>/dev/null && echo "[reset] Killed tunnel PID(s) $pids on :$port"
  fi
done
```

### 2. Set Up SSH Tunnel for sdi clean

`sdi clean` needs libvirt access to remote nodes but doesn't auto-tunnel. Check if any non-primary nodes exist in `credentials/.baremetal-init.yaml` and set up tunnels for them:

```bash
# For playbox-1 (the only ProxyJump node with VMs currently):
ssh -fN -o StrictHostKeyChecking=no -o ExitOnForwardFailure=yes \
  -L22101:192.168.88.9:22 jinwang@100.72.90.110
```

Adapt the IPs from `credentials/.baremetal-init.yaml` if the node configuration has changed.

### 3. Run sdi clean

```bash
cd /home/jinwang/local-workspace/ScaleX-POD-mini
scalex sdi clean --hard --yes-i-really-want-to
```

### 4. Kill the sdi-clean Tunnel

**CRITICAL**: The tunnel from Step 2 MUST be killed before install.sh, otherwise sdi init will port-conflict.

```bash
lsof -ti :22101 | xargs kill 2>/dev/null
```

### 5. Clear Installer State

```bash
rm -f ~/.scalex/installer/phase_completed ~/.scalex/installer/state.env
rm -rf _generated/clusters _generated/facts
```

### 6. Verify Clean State

```bash
ssh playbox-0 "sudo virsh list --all"   # Should show no VMs
ssh playbox-1 "sudo virsh list --all"   # Should show no VMs
ls _generated/                           # Should be empty or minimal
```

### 7. Run install.sh --auto

```bash
AUTO_MODE=true bash install.sh --auto 2>&1 | tee /tmp/scalex-install-$(date +%Y%m%d-%H%M%S).log
```

This takes ~45 minutes. Run in background and monitor with `tail -f /tmp/scalex-install-*.log`.

### 8. Post-Install Verification

After install.sh completes, run `/scalex-verify-cluster` to confirm health.

## Expected Timeline

| Phase | Duration |
|-------|----------|
| Tunnel cleanup + sdi clean | ~2 min |
| install.sh --auto (total) | ~45 min |
| - facts | ~1 min |
| - sdi init (VM creation) | ~3 min |
| - cluster init tower (Kubespray) | ~15 min |
| - cluster init sandbox (Kubespray) | ~15 min |
| - tunnels + secrets + bootstrap | ~10 min |

## Known Issues

- Tower health may show "yellow" for 5-15 min after install (keycloak/cloudflared startup delay) — this is normal
- If `install.sh --auto` fails mid-run, re-run it — it resumes from the last completed phase
