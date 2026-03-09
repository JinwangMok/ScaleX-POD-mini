# Setup Guide

## Prerequisites

### On your workstation (openclaw-vm)
```bash
# Required tools
sudo apt install -y ansible python3-pip
pip install jinja2 pyyaml
sudo snap install yq
sudo snap install helm --classic
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
sudo install kubectl /usr/local/bin/

# OpenTofu (if tower enabled)
curl -fsSL https://get.opentofu.org/install-opentofu.sh | sudo bash -s -- --install-method standalone
```

### SSH Access
1. Generate SSH key: `ssh-keygen -t ed25519`
2. Copy to playbox-0: `ssh-copy-id jinwang@192.168.88.8`
3. From playbox-0, copy to other nodes: `ssh-copy-id jinwang@192.168.88.{9,10,11}`

### Cloudflare Tunnel
1. Go to Cloudflare Zero Trust dashboard
2. Create a tunnel named `playbox-admin-static`
3. Download credentials JSON file
4. Download cert.pem
5. Set paths in values.yaml: `cloudflare.credentials_file` and `cloudflare.cert_file`

### NIC Discovery
Run `./playbox discover-nics` to get MAC addresses for values.yaml.

## Configuration

Edit `values.yaml` — this is the only file you need to modify:

1. Verify node IPs and MAC addresses match your hardware
2. Set Cloudflare tunnel credentials paths
3. Set Keycloak passwords (change defaults!)
4. Verify component versions

## Provisioning

```bash
./playbox up              # Full provisioning
# Or step-by-step with --dry-run first:
./playbox up --dry-run    # Preview
./playbox up              # Execute
```
