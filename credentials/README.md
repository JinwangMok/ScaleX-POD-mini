# credentials/

This directory stores sensitive configuration files. All files except examples and this README are gitignored.

## Required Files

### `secrets.yaml`

Copy from `secrets.yaml.example` and fill in actual values:

```bash
cp credentials/secrets.yaml.example credentials/secrets.yaml
```

Contains: Keycloak passwords, ArgoCD PAT, Cloudflare credential paths.

### `.baremetal-init.yaml`

Copy from `.baremetal-init.yaml.example` and fill in node connection info:

```bash
cp credentials/.baremetal-init.yaml.example credentials/.baremetal-init.yaml
```

Contains: SSH access info for each bare-metal node (IP, user, auth mode).

### `.env`

Copy from `.env.example` and fill in actual secrets:

```bash
cp credentials/.env.example credentials/.env
```

Contains: SSH passwords and key paths referenced by `.baremetal-init.yaml`.

### `cloudflare-tunnel.json` (optional)

Download from Cloudflare dashboard after creating a tunnel. Referenced by `secrets.yaml`.
