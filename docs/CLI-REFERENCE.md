# CLI Reference (`scalex`)

The `scalex` CLI is the primary tool for managing the entire ScaleX-POD-mini platform — from hardware facts gathering through SDI virtualization to multi-cluster provisioning.

## Build

```bash
cd scalex-cli && cargo build --release
# Binary: target/release/scalex
```

## Core Commands

| Command | Description |
|---------|-------------|
| `scalex facts --all` | Gather hardware info from all bare-metal nodes |
| `scalex facts --host <name>` | Gather from a single node |
| `scalex sdi init <sdi-specs.yaml>` | Virtualize bare-metal → resource pool → VM pools |
| `scalex sdi clean --hard --yes-i-really-want-to` | Full infrastructure reset |
| `scalex sdi sync` | Reconcile bare-metal changes (add/remove nodes) |
| `scalex cluster init <k8s-clusters.yaml>` | Kubespray → multi-cluster provisioning |
| `scalex secrets apply` | Generate and apply pre-bootstrap K8s secrets |
| `scalex bootstrap` | Install ArgoCD + register clusters + apply spread.yaml |

## Query Commands

| Command | Description |
|---------|-------------|
| `scalex get baremetals` | Hardware facts table |
| `scalex get sdi-pools` | VM pool status |
| `scalex get clusters` | Cluster inventory |
| `scalex get config-files` | Config file validation |
| `scalex validate` | Pre-provisioning config validation (YAML parsing, IP/CIDR conflicts, pool mapping) |
| `scalex status` | 5-layer platform status report |
| `scalex kernel-tune` | Kernel parameter recommendations and diff |

## Quick Reference

```bash
scalex facts --all                           # HW facts
scalex sdi init config/sdi-specs.yaml        # VM pools
scalex cluster init config/k8s-clusters.yaml # K8s clusters
scalex secrets apply                         # Secrets
scalex bootstrap                             # ArgoCD + GitOps
scalex status                                # Full status
scalex get clusters                          # Cluster list
scalex sdi clean --hard --yes-i-really-want-to  # Full reset
```

## VM Resource Budget

`scalex plan` computes VM specs from component budgets. Source: `resource_planner.rs`

| Cluster Role | CPU (mc) | RAM (MB) | Disk (MB) | VM Spec (vCPU / GB / disk GB) |
|--------------|----------|----------|-----------|-------------------------------|
| **Management** (Tower) | 2,450 | 3,904 | 11,136 | **3 / 4 / 20** |
| **Workload** (Sandbox) | 1,450 | 2,368 | 8,064 | **2 / 3 / 20** |

> Included: base-os, k8s-cp, cilium, coredns, argocd(mgmt), cert-manager, kyverno, keycloak(mgmt), cloudflared(mgmt), local-path(workload)
> VM conversion: `vCPU = ceil(mc/1000)`, `GB = ceil(MB/1024)`, `disk = max(ceil(MB/1024), 20)`

## Config Files

| File | Purpose |
|------|---------|
| `credentials/.baremetal-init.yaml` | SSH access to bare-metal nodes |
| `credentials/.env` | SSH passwords/key paths |
| `credentials/secrets.yaml` | Keycloak, ArgoCD, Cloudflare secrets |
| `config/sdi-specs.yaml` | VM pool definitions (CPU, RAM, disk, GPU) |
| `config/k8s-clusters.yaml` | Cluster definitions (mode, role, addons) |
