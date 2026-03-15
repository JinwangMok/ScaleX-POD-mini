# /scalex-verify-cluster

Post-install cluster health verification using `scalex dash --headless`.

## When to Use

- After `install.sh --auto` completes
- After `scalex bootstrap`
- "verify cluster", "check cluster health", "post-install check", "is the cluster healthy"

## Steps

### 1. Node Readiness Check

```bash
scalex dash --headless --resource nodes 2>/dev/null | python3 -c "
import sys, json
data = json.load(sys.stdin)
all_ready = True
for c in data.get('clusters', []):
    name = c.get('name', '?')
    health = c.get('health', '?')
    nodes = c.get('nodes', [])
    not_ready = [n for n in nodes if n.get('status') != 'Ready']
    print(f'[{name}] health={health}, nodes={len(nodes)} total, {len(not_ready)} not-ready')
    for n in nodes:
        cpu = n.get('cpu_capacity', '?')
        mem_ki = int(n.get('mem_capacity', '0').replace('Ki',''))
        mem_gi = f'{mem_ki/1024/1024:.1f}Gi'
        print(f'  {n[\"name\"]:20s} {n[\"status\"]:8s} cpu={cpu} mem={mem_gi}')
    if not_ready:
        all_ready = False
print()
print('NODE CHECK:', 'PASS' if all_ready else 'FAIL')
"
```

**Expected**: All nodes Ready, both clusters green or yellow.

### 2. Critical Pod Check

```bash
scalex dash --headless --resource pods 2>/dev/null | python3 -c "
import sys, json
data = json.load(sys.stdin)
critical_namespaces = {'argocd', 'kube-system', 'cert-manager', 'kyverno'}
issues = []
for c in data.get('clusters', []):
    name = c.get('name', '?')
    pods = c.get('pods', [])
    for ns in critical_namespaces:
        ns_pods = [p for p in pods if p.get('namespace') == ns]
        bad = [p for p in ns_pods if p.get('status') not in ('Running', 'Succeeded')]
        if bad:
            for p in bad:
                issues.append(f'[{name}/{ns}] {p[\"name\"]}: {p.get(\"status\",\"?\")}')
        if ns_pods:
            running = len([p for p in ns_pods if p.get('status') == 'Running'])
            print(f'[{name}] {ns}: {running}/{len(ns_pods)} running')
print()
if issues:
    print('ISSUES:')
    for i in issues:
        print(f'  - {i}')
    print()
print('POD CHECK:', 'PASS' if not issues else 'WARN (non-critical pods may still be starting)')
"
```

**Expected**: ArgoCD 7/7, Cilium running, cert-manager 3/3, Kyverno 4/4.

### 3. Cluster Inventory Check

```bash
scalex get clusters
```

**Expected**: Both tower and sandbox listed with kubeconfig paths.

### 4. TUI Launch Test

```bash
timeout 5 scalex dash 2>/tmp/scalex-dash-test.log
code=$?
stderr=$(cat /tmp/scalex-dash-test.log)
if [ $code -eq 124 ] && [ -z "$stderr" ]; then
  echo "TUI CHECK: PASS (exit 124, no crash)"
else
  echo "TUI CHECK: FAIL (exit=$code, stderr=$stderr)"
fi
```

**Expected**: Exit code 124 (timeout), no stderr.

### 5. Summary

After all checks, report:

| Check | Result |
|-------|--------|
| Nodes | PASS/FAIL (count Ready/total) |
| Pods | PASS/WARN/FAIL (critical namespaces) |
| Inventory | PASS/FAIL (both clusters listed) |
| TUI | PASS/FAIL (no crash) |

## Health Status Guide

| Status | Meaning |
|--------|---------|
| **green** | All deployments available, all nodes Ready |
| **yellow** | Some non-critical deployments not ready (keycloak, cloudflared, hubble-relay) — normal for 5-15 min after install |
| **red** | Critical component failure — investigate immediately |
| **unknown** | Cannot connect to cluster API — check tunnels |

## Notes

- `scalex dash --headless` auto-tunnels through bastion — no manual tunnel setup needed
- Direct `kubectl` to 192.168.88.x IPs will NOT work from the workstation — always use `scalex dash` or set up tunnels manually
- Tower yellow after fresh install is expected; wait 5-15 min for keycloak/cloudflared to settle
