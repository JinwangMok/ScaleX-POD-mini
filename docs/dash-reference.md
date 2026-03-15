# scalex dash — Multi-cluster Kubernetes TUI Dashboard Reference

## Overview

`scalex dash` provides a real-time, interactive TUI dashboard for monitoring and navigating multiple Kubernetes clusters. It features a VSCode-like layout with a file-tree sidebar, tabbed center panel, and cluster health status bar.

## Usage

```bash
# Interactive TUI mode (default)
scalex dash

# Headless mode — JSON output for AI agents / scripts
scalex dash --headless

# Custom kubeconfig directory
scalex dash --kubeconfig-dir /path/to/kubeconfigs

# Filter to specific cluster/namespace
scalex dash --headless --cluster tower
scalex dash --headless --cluster sandbox --namespace kube-system

# Filter by resource type
scalex dash --headless --resource pods
scalex dash --headless --resource nodes

# Custom refresh interval (seconds)
scalex dash --refresh 10
```

## Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--headless` | bool | false | Output JSON and exit (no TUI) |
| `--kubeconfig-dir` | path | `_generated/clusters` | Directory with `{cluster}/kubeconfig.yaml` |
| `--cluster` | string | all | Filter to specific cluster |
| `--namespace` | string | all | Filter to specific namespace |
| `--resource` | string | all | Resource type: `pods`, `deployments`, `services`, `nodes` |
| `--refresh` | u64 | 5 | Data refresh interval in seconds |

Environment variable `SCALEX_KUBECONFIG_DIR` can also set the kubeconfig directory.

## Kubeconfig Discovery

The dashboard discovers clusters by scanning `{kubeconfig-dir}/{cluster-name}/kubeconfig.yaml`. The default location is `_generated/clusters/`, which is where `scalex cluster init` places generated kubeconfigs.

```
_generated/clusters/
├── tower/
│   └── kubeconfig.yaml
└── sandbox/
    └── kubeconfig.yaml
```

## TUI Keyboard Shortcuts

### Navigation
| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `h` / `←` | Collapse tree node |
| `l` / `→` | Expand tree node |
| `Enter` | Select / Toggle expand |
| `Tab` | Switch panel (sidebar ↔ center) |

### Tabs
| Key | Action |
|-----|--------|
| `Ctrl+1` | Resources tab |
| `Ctrl+2` | Top (utilization) tab |

### Resource Views (center panel)
| Key | View |
|-----|------|
| `p` | Pods |
| `d` | Deployments |
| `s` | Services |
| `n` | Nodes |
| `c` | ConfigMaps |

### Other
| Key | Action |
|-----|--------|
| `r` | Force refresh |
| `?` | Toggle help overlay |
| `q` | Quit |
| `Ctrl+C` | Force quit |

## Layout

```
┌─ Tab Bar ─────────────────────────────────────┐
│ [1] Resources  [2] Top                        │
├─────────┬─────────────────────────────────────┤
│ Sidebar │ Center Panel                        │
│         │                                     │
│ ScaleX  │ Pods | tower > kube-system          │
│ ├tower  │ NAME        STATUS  READY  AGE      │
│ │├All   │ coredns-... Running  1/1   5d       │
│ │├kube..│ etcd-...    Running  1/1   5d       │
│ │└defa..│                                     │
│ ├sandbox│                                     │
│ └Infra  │                                     │
├─────────┴─────────────────────────────────────┤
│ Status: ● tower  ● sandbox | latency: 42ms    │
│ Usage: tower: pods 12/12 nodes 1/1            │
└───────────────────────────────────────────────┘
```

## Headless Mode JSON Schema

### Full output (`scalex dash --headless`)

```json
{
  "clusters": [
    {
      "name": "tower",
      "health": "green",
      "namespaces": ["default", "kube-system", "argocd"],
      "nodes": [
        {
          "name": "tower-cp-0",
          "status": "Ready",
          "roles": ["control-plane"],
          "cpu_capacity": "4",
          "mem_capacity": "8Gi",
          "cpu_allocatable": "3800m",
          "mem_allocatable": "7Gi"
        }
      ],
      "pods": [
        {
          "name": "coredns-abc123",
          "namespace": "kube-system",
          "status": "Running",
          "ready": "1/1",
          "restarts": 0,
          "age": "5d",
          "node": "tower-cp-0"
        }
      ],
      "deployments": [
        {
          "name": "coredns",
          "namespace": "kube-system",
          "ready": "2/2",
          "up_to_date": 2,
          "available": 2,
          "age": "5d"
        }
      ],
      "services": [
        {
          "name": "kubernetes",
          "namespace": "default",
          "svc_type": "ClusterIP",
          "cluster_ip": "10.233.0.1",
          "ports": "443/TCP",
          "age": "5d"
        }
      ],
      "resource_usage": {
        "cpu_percent": 0.0,
        "mem_percent": 0.0,
        "total_pods": 15,
        "running_pods": 15,
        "failed_pods": 0,
        "total_nodes": 1,
        "ready_nodes": 1
      }
    }
  ],
  "infrastructure": {
    "sdi_pools": [
      {
        "pool_name": "tower",
        "purpose": "management",
        "nodes": [
          {
            "name": "tower-cp-0",
            "ip": "10.0.0.100",
            "host": "node-0",
            "cpu": 2,
            "mem_gb": 4,
            "disk_gb": 30,
            "status": "running",
            "gpu": false
          }
        ]
      }
    ],
    "total_vms": 1,
    "running_vms": 1
  }
}
```

### Filtered output (`scalex dash --headless --resource pods`)

```json
{
  "clusters": [
    {
      "cluster": "tower",
      "health": "green",
      "pods": [...]
    }
  ]
}
```

### Error output

```json
{
  "error": "Cluster 'nonexistent' not found"
}
```

## Health Status Logic

| Status | Condition |
|--------|-----------|
| **Green** (●) | All nodes Ready, 0 failed pods |
| **Yellow** (●) | All nodes Ready, 1-5 failed pods |
| **Red** (●) | Any node NotReady, or >5 failed pods |
| **Unknown** (○) | Cannot connect to cluster API |

## Infrastructure (OpenTofu/SDI)

When SDI data is available in `_generated/sdi/`, the sidebar shows an "Infrastructure" section with:
- SDI pool names and purposes
- VM details: name, IP, host, CPU/mem/disk, status
- VM-to-cluster-node mapping

In headless mode, add `--resource infra` for infrastructure-only output.

## Design

- **Theme**: Gruvbox Dark
- **Framework**: ratatui + crossterm
- **K8s client**: kube-rs (native Rust, no kubectl dependency)
- **Architecture**: Functional style with pure data transforms, async I/O isolated to fetch layer
