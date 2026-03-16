# Dash TUI — Keycloak OIDC Per-User Auth (Future Plan)

## Background

The current `scalex dash` uses a shared `scalex-dash` ServiceAccount with `view` ClusterRole for
CF Tunnel connections. This is read-only and sufficient for monitoring, but doesn't support
per-user actions (create, delete, scale) or audit trails.

## Goal

Enable per-user authentication in the TUI dashboard via Keycloak OIDC, so each user gets
role-appropriate permissions (viewer, editor, admin) with full audit trail.

## Prerequisites (Already in Place)

- `k8s-clusters.yaml` OIDC config: `kube_oidc_issuer_url`, `kube_oidc_client_id`,
  `kube_oidc_username_claim`, `kube_oidc_groups_claim` are generated into cluster-vars
- Keycloak deployed at `auth.jinwang.dev` via CF Tunnel
- K8s API servers configured with OIDC token verification

## Design

### Authentication Flow

1. User presses `L` (login) in TUI
2. TUI spawns a local HTTP callback server on `localhost:<random_port>`
3. TUI opens system browser to Keycloak authorization endpoint with PKCE
4. User authenticates in browser → Keycloak redirects to `localhost:<port>/callback`
5. Callback server receives authorization code, exchanges for id_token + refresh_token
6. TUI stores tokens in memory, replaces SA bearer token in kube client
7. K8s API server validates OIDC token, maps to user identity + groups → RBAC

### Token Lifecycle

- **Access token**: Short-lived (~5min), used for API calls
- **Refresh token**: Long-lived (~24h), used to refresh access token
- **Background refresh**: TUI refreshes token before expiry (no user interaction)
- **Expiry handling**: If refresh fails, TUI shows login prompt overlay
- **Logout**: `Shift+L` clears tokens, reverts to SA token (read-only fallback)

### RBAC Mapping

| Keycloak Group | K8s ClusterRole | TUI Capabilities |
|----------------|-----------------|-------------------|
| `k8s-viewers` | `view` | Read-only (current behavior) |
| `k8s-editors` | `edit` | Create, update, scale resources |
| `k8s-admins` | `cluster-admin` | Full access including delete, drain |

Groups are mapped via `kube_oidc_groups_claim: "groups"` and
`kube_oidc_groups_prefix: "oidc:"` in cluster-vars.

### TUI UX Changes

- **Status bar**: Shows logged-in username or "anonymous (SA)"
- **Login key (`L`)**: Triggers OIDC flow
- **Permission-gated actions**: Delete/scale buttons greyed out for viewers
- **Action confirmation**: Destructive actions require explicit confirmation dialog

### Connection Mode Matrix

| Mode | Auth Method | Capabilities |
|------|-------------|-------------|
| SSH tunnel (default) | kubeconfig client cert | Full (cert = cluster-admin) |
| CF Tunnel (no login) | SA bearer token | Read-only (view) |
| CF Tunnel (logged in) | OIDC id_token | Per-user RBAC |
| Headless (`--headless`) | SA bearer token | Read-only (view) |
| Headless + `--token` | Provided bearer token | Per-token RBAC |

### Dependencies

- `openidconnect` crate — OIDC client with PKCE
- `open` crate — Cross-platform browser launch
- `tokio` tiny HTTP server — Callback receiver (already have tokio)

### Implementation Phases

1. **Phase 1**: OIDC login flow + token injection (browser → callback → token → kube client)
2. **Phase 2**: Token refresh + expiry handling
3. **Phase 3**: Permission-gated TUI actions (scale, delete, cordon)
4. **Phase 4**: Headless `--token` flag for CI/CD pipelines

## Notes

- SA token remains as fallback — never remove it
- OIDC flow only triggers on user action (no auto-redirect)
- Token never written to disk (memory only, security)
- Works with any OIDC provider, not just Keycloak
