# Troubleshooting

## SSH Issues

**Can't SSH to nodes:**
```bash
# Check from workstation
ssh -v jinwang@192.168.88.8

# Check from bastion to workers
ssh jinwang@192.168.88.8 "ssh jinwang@192.168.88.9 hostname"
```

**ansible_user doesn't exist yet:**
The first run uses `jinwang` (superuser). After `prepare-nodes`, `ansible_user` is available.

## Network Issues

**Lost connectivity after netplan:**
Netplan uses `netplan try --timeout 120`. If you lose SSH, wait 120 seconds and the config auto-reverts.

**Bond not working:**
```bash
cat /proc/net/bonding/bond0
ip addr show bond0
```

## Cluster Issues

**Nodes not Ready:**
```bash
kubectl get nodes -o wide
kubectl describe node <node-name>
# Check Cilium
kubectl -n kube-system get pods -l app.kubernetes.io/name=cilium
```

**ArgoCD apps not syncing:**
```bash
kubectl -n argocd get applications
kubectl -n argocd get applicationsets
# Check ArgoCD logs
kubectl -n argocd logs deployment/argocd-server
```

**Keycloak not starting:**
```bash
kubectl -n keycloak get pods
kubectl -n keycloak logs deployment/keycloak
# Check secrets exist
kubectl -n keycloak get secrets
```

## Tower VM Issues

**VM not booting:**
```bash
ssh jinwang@playbox-0 "sudo virsh list --all"
ssh jinwang@playbox-0 "sudo virsh console tower-vm"
```

**OpenTofu state issues:**
```bash
cd _generated/sdi
tofu state list
tofu state show libvirt_domain.tower
```

## Cloudflare Tunnel

**Tunnel not connecting:**
```bash
kubectl -n kube-tunnel get pods
kubectl -n kube-tunnel logs -l app=cloudflared
# Check secret exists
kubectl -n kube-tunnel get secret cloudflared-tunnel-credentials
```

## Reset and Rebuild

```bash
# Full SDI reset (removes all VMs, K8s clusters, keeps SSH access)
scalex sdi clean --hard --yes-i-really-want-to

# Re-provision from scratch
scalex facts --all
scalex sdi init config/sdi-specs.yaml
scalex cluster init config/k8s-clusters.yaml
scalex secrets apply
kubectl apply -f gitops/bootstrap/spread.yaml
```

## Useful Queries

```bash
# Check overall status
scalex get baremetals     # Hardware facts
scalex get sdi-pools      # VM pool status
scalex get clusters       # Cluster inventory
scalex get config-files   # Config file validation
```
