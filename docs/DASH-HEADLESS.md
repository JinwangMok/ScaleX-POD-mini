# scalex dash — Headless Mode (AI Agent API)

`scalex dash --headless` provides JSON output for programmatic access to multi-cluster Kubernetes state. Designed for AI agents, scripts, and automation pipelines.

## CLI Flags

```
scalex dash --headless [OPTIONS]
```

| Flag | Type | Description |
|------|------|-------------|
| `--headless` | bool | Required. Enables JSON output mode (no TUI). |
| `--cluster <NAME>` | string | Filter to a specific cluster (e.g., `tower`, `sandbox`). |
| `--namespace <NS>` | string | Filter resources to a specific namespace. |
| `--resource <TYPE>` | string | Filter to a resource type: `pods`, `deployments`, `services`, `nodes`, `configmaps`, `infra`. |
| `--kubeconfig-dir <PATH>` | path | Directory containing per-cluster kubeconfig files. Default: `_generated/clusters`. Env: `SCALEX_KUBECONFIG_DIR`. |
| `--refresh <SECS>` | u64 | Data refresh interval (unused in headless mode, included for CLI consistency). Default: `5`. |

## Output Schemas

### Full Output (no `--resource` filter)

```bash
scalex dash --headless
```

```json
{
  "clusters": [
    {
      "name": "tower",
      "health": "green",
      "namespaces": ["argocd", "default", "kube-system", ...],
      "nodes": [...],
      "pods": [...],
      "deployments": [...],
      "services": [...],
      "configmaps": [...],
      "resource_usage": {
        "cpu_percent": 42.5,
        "mem_percent": 68.2,
        "total_pods": 25,
        "running_pods": 23,
        "failed_pods": 0,
        "total_nodes": 1,
        "ready_nodes": 1
      }
    }
  ],
  "infrastructure": {
    "sdi_pools": [...],
    "total_vms": 3,
    "running_vms": 3
  }
}
```

### Pods

```bash
scalex dash --headless --resource pods
scalex dash --headless --resource pods --cluster tower --namespace kube-system
```

```json
{
  "clusters": [
    {
      "cluster": "tower",
      "health": "green",
      "pods": [
        {
          "name": "coredns-5d78c9869d-abc12",
          "namespace": "kube-system",
          "status": "Running",
          "ready": "1/1",
          "restarts": 0,
          "age": "3d",
          "node": "tower-cp-0"
        }
      ]
    }
  ]
}
```

**Status values**: `Running`, `Pending`, `Succeeded`, `Failed`, `CrashLoopBackOff`, `ImagePullBackOff`, `ErrImagePull`, `OOMKilled`, `Error`, `Unknown`.

### Deployments

```bash
scalex dash --headless --resource deployments
```

```json
{
  "clusters": [
    {
      "cluster": "tower",
      "health": "green",
      "deployments": [
        {
          "name": "coredns",
          "namespace": "kube-system",
          "ready": "2/2",
          "up_to_date": 2,
          "available": 2,
          "age": "3d"
        }
      ]
    }
  ]
}
```

### Services

```bash
scalex dash --headless --resource services
```

```json
{
  "clusters": [
    {
      "cluster": "tower",
      "health": "green",
      "services": [
        {
          "name": "kubernetes",
          "namespace": "default",
          "svc_type": "ClusterIP",
          "cluster_ip": "10.96.0.1",
          "ports": "443/TCP",
          "age": "3d"
        }
      ]
    }
  ]
}
```

### Nodes

```bash
scalex dash --headless --resource nodes
```

```json
{
  "clusters": [
    {
      "cluster": "tower",
      "health": "green",
      "nodes": [
        {
          "name": "tower-cp-0",
          "status": "Ready",
          "roles": ["control-plane"],
          "cpu_capacity": "4",
          "mem_capacity": "8154832Ki",
          "cpu_allocatable": "4",
          "mem_allocatable": "8052432Ki"
        }
      ]
    }
  ]
}
```

### ConfigMaps

```bash
scalex dash --headless --resource configmaps
scalex dash --headless --resource configmaps --namespace kube-system
```

```json
{
  "clusters": [
    {
      "cluster": "tower",
      "health": "green",
      "configmaps": [
        {
          "name": "coredns",
          "namespace": "kube-system",
          "data_keys_count": 2,
          "age": "3d"
        }
      ]
    }
  ]
}
```

### Infrastructure (SDI)

```bash
scalex dash --headless --resource infra
```

```json
{
  "sdi_pools": [
    {
      "pool_name": "tower",
      "purpose": "management",
      "nodes": [
        {
          "vm_name": "tower-cp-0",
          "ip": "192.168.88.100",
          "host": "playbox-0",
          "vcpu": 4,
          "memory_mb": 8192,
          "disk_gb": 50,
          "status": "running",
          "gpu": null
        }
      ]
    }
  ],
  "total_vms": 3,
  "running_vms": 3
}
```

## Error Format

```json
{
  "error": "Cluster 'nonexistent' not found"
}
```

Exit code `1` on errors, `0` on success.

## Kubeconfig Discovery

Headless mode discovers kubeconfigs from `{kubeconfig_dir}/{cluster_name}/kubeconfig.yaml`. The default directory is `_generated/clusters` relative to the working directory.

Override via:
- `--kubeconfig-dir /path/to/configs`
- `SCALEX_KUBECONFIG_DIR=/path/to/configs`

## Usage Examples for AI Agents

```bash
# Check overall cluster health
scalex dash --headless | jq '.clusters[].health'

# Find failed pods across all clusters
scalex dash --headless --resource pods | jq '.clusters[].pods[] | select(.status != "Running" and .status != "Succeeded")'

# Get node resource capacity for a specific cluster
scalex dash --headless --resource nodes --cluster sandbox | jq '.clusters[].nodes[] | {name, cpu_capacity, mem_capacity}'

# List all namespaces
scalex dash --headless | jq '.clusters[] | {cluster: .name, namespaces}'

# Get configmap count per namespace
scalex dash --headless --resource configmaps | jq '.clusters[].configmaps | group_by(.namespace) | map({namespace: .[0].namespace, count: length})'

# Check infrastructure VM status
scalex dash --headless --resource infra | jq '.sdi_pools[].nodes[] | {vm_name, status, ip}'
```

## Notes

- `cpu_percent` and `mem_percent` in `resource_usage` require metrics-server deployed on the cluster. Without it, values are `0.0`.
- All timestamps are relative ages (e.g., `"3d"`, `"2h"`, `"45m"`, `"30s"`).
- Health status is computed from node readiness and pod failure counts: `green` (all ok), `yellow` (some failed pods), `red` (nodes not ready or many failures), `unknown` (unreachable).
