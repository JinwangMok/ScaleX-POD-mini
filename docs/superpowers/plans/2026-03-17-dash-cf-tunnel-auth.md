# Dash CF Tunnel Authentication Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable `scalex dash` to authenticate K8s API calls through Cloudflare Tunnel using a ServiceAccount token, replacing client cert auth which CF Tunnel cannot proxy.

**Architecture:** When `api_endpoint` is configured and a CF Tunnel is in use, `scalex dash` connects using a `scalex-dash` ServiceAccount with `view` ClusterRole. The SA and token are auto-provisioned via SSH tunnel on first run if they don't exist. Token is cached at `_generated/clusters/{name}/dash-token`. `build_client_with_endpoint()` injects the bearer token instead of client cert.

**Tech Stack:** Rust (kube-rs 0.98), K8s RBAC (ServiceAccount, ClusterRoleBinding), kubectl via SSH

**Future: Keycloak OIDC per-user auth** — The current SA token approach is a stepping stone. The long-term plan is to integrate Keycloak OIDC login into the TUI dashboard itself: user presses a key to trigger browser-based OIDC flow (similar to `kubectl oidc-login`), the returned token carries the user's identity, and K8s RBAC policies (already configured via `oidc` in `k8s-clusters.yaml`) grant per-user permissions (view, edit, admin). This enables create/delete operations based on the logged-in user's role. The SA token remains as a fallback for headless/CI usage. See Task 4 for details.

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `scalex-cli/src/dash/kube_client.rs` | Modify | Add token injection to `build_client_with_endpoint`, auto-provision SA flow |
| `scalex-cli/src/dash/sa_provisioner.rs` | Create | SA + ClusterRoleBinding + token creation via kubectl over SSH |
| `scalex-cli/src/dash/mod.rs` | Modify | Add `mod sa_provisioner` |
| `scalex-cli/src/core/kubespray.rs` | None | No changes (SAN auto-derive already done) |
| `scalex-cli/src/core/validation.rs` | Modify | Add validation test for dash-token flow |

---

## Chunk 1: SA Token Auth

### Task 1: SA Provisioner Module

**Files:**
- Create: `scalex-cli/src/dash/sa_provisioner.rs`
- Modify: `scalex-cli/src/dash/mod.rs`

- [ ] **Step 1: Create `sa_provisioner.rs` with SA provisioning logic**

The module provisions a `scalex-dash` ServiceAccount with `view` ClusterRole on a target cluster via SSH + kubectl. It:
1. SSHs to the control plane via bastion (ProxyJump)
2. Creates namespace `scalex-system` if not exists
3. Creates ServiceAccount `scalex-dash`
4. Creates ClusterRoleBinding `scalex-dash-view` binding SA to `view` ClusterRole
5. Creates a token Secret and extracts the token
6. Returns the token string

Key function signatures:
```rust
/// Provision scalex-dash ServiceAccount on a cluster and return the bearer token.
/// Connects via SSH through bastion to run kubectl commands on the control plane.
pub async fn provision_dash_sa(
    kubeconfig_path: &Path,
    cluster_name: &str,
    bastion: &str,
) -> Result<String>;

/// Read cached token from _generated/clusters/{name}/dash-token.
/// Returns None if file doesn't exist.
pub fn read_cached_token(kubeconfig_path: &Path) -> Option<String>;

/// Write token to _generated/clusters/{name}/dash-token.
pub fn cache_token(kubeconfig_path: &Path, token: &str) -> Result<()>;
```

SSH kubectl commands to execute (as a single `kubectl apply -f -` stdin):
```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: scalex-system
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: scalex-dash
  namespace: scalex-system
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: scalex-dash-view
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: view
subjects:
  - kind: ServiceAccount
    name: scalex-dash
    namespace: scalex-system
---
apiVersion: v1
kind: Secret
metadata:
  name: scalex-dash-token
  namespace: scalex-system
  annotations:
    kubernetes.io/service-account.name: scalex-dash
type: kubernetes.io/service-account-token
```

Then extract token: `kubectl get secret scalex-dash-token -n scalex-system -o jsonpath='{.data.token}' | base64 -d`

Implementation: use `std::process::Command` with SSH to bastion, ProxyJump to control plane IP (parsed from kubeconfig), run kubectl.

- [ ] **Step 2: Add `mod sa_provisioner` to `dash/mod.rs`**

- [ ] **Step 3: Build to verify compilation**

Run: `cargo build`
Expected: PASS

- [ ] **Step 4: Commit**

```
feat(dash): add SA provisioner for CF Tunnel bearer token auth
```

---

### Task 2: Token Injection in `build_client_with_endpoint`

**Files:**
- Modify: `scalex-cli/src/dash/kube_client.rs`

- [ ] **Step 1: Modify `build_client_with_endpoint` to accept optional token**

Change signature:
```rust
async fn build_client_with_endpoint(
    kubeconfig_path: &Path,
    endpoint: &str,
    bearer_token: Option<&str>,
) -> Result<Client>
```

When `bearer_token` is `Some(token)`:
- Strip cluster CA (as before)
- Strip user client-certificate-data and client-key-data
- Inject bearer token into the user's auth info:
  ```rust
  for auth in &mut kubeconfig.auth_infos {
      if let Some(ref mut info) = auth.auth_info {
          info.token = Some(secrecy::SecretString::new(token.to_string()));
          info.client_certificate = None;
          info.client_certificate_data = None;
          info.client_key = None;
          info.client_key_data = None;
      }
  }
  ```

When `bearer_token` is `None`: behave as before (system CAs, original user creds).

- [ ] **Step 2: Update all call sites of `build_client_with_endpoint`**

In `discover_clusters_streaming()` (~line 192):
```rust
build_client_with_endpoint(&kubeconfig_path, ep, token.as_deref()).await
```

In `discover_clusters()` (~line 593):
```rust
build_client_with_endpoint(&kubeconfig_path, ep, token.as_deref()).await
```

In `discover_clusters_streaming_filtered()` (if exists): same pattern.

Before calling, resolve token:
```rust
let token = sa_provisioner::read_cached_token(&kubeconfig_path);
```

- [ ] **Step 3: Build and test**

Run: `cargo test`
Expected: 776+ tests pass

- [ ] **Step 4: Commit**

```
feat(dash): inject SA bearer token for CF Tunnel connections
```

---

### Task 3: Auto-Provision on First Run

**Files:**
- Modify: `scalex-cli/src/dash/kube_client.rs`

- [ ] **Step 1: Add auto-provision flow in both discovery functions**

In both `discover_clusters` and `discover_clusters_streaming`, when Strategy 1 (domain) is attempted:

```rust
// Resolve or provision SA token for CF Tunnel auth
let token = sa_provisioner::read_cached_token(&kubeconfig_path)
    .or_else(|| {
        // Auto-provision via SSH tunnel if bastion available
        if let Some(ref bastion_host) = bastion {
            eprintln!("{}: provisioning dash SA...", cluster_name);
            match tokio::runtime::Handle::current()
                .block_on(sa_provisioner::provision_dash_sa(
                    &kubeconfig_path, &cluster_name, bastion_host,
                )) {
                Ok(t) => {
                    let _ = sa_provisioner::cache_token(&kubeconfig_path, &t);
                    Some(t)
                }
                Err(e) => {
                    eprintln!("{}: SA provision failed: {}", cluster_name, e);
                    None
                }
            }
        } else {
            None
        }
    });
```

Note: In the streaming async context, use `.await` directly instead of `block_on`.

- [ ] **Step 2: Add debug logging for domain probe success/failure**

Already partially done. Ensure both streaming and headless paths log clearly.

- [ ] **Step 3: Build and test**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 4: Integration test (manual)**

```bash
# Remove cached token to force re-provision
rm _generated/clusters/tower/dash-token
# Run headless — should auto-provision then connect via domain
scalex dash --headless --resource nodes 2>&1 | head -10
# Expected: "tower: provisioning dash SA..." then "tower: connected via domain (...)"
```

- [ ] **Step 5: Commit**

```
feat(dash): auto-provision SA token on first CF Tunnel connection
```

---

## Chunk 2: Future OIDC Auth Plan (Documentation Only)

### Task 4: Document Keycloak OIDC TUI Auth Plan

**Files:**
- Create: `docs/superpowers/plans/2026-03-17-dash-oidc-auth-future.md`

- [ ] **Step 1: Write future plan document**

Document the planned Keycloak OIDC integration for the TUI dashboard:

1. **Trigger**: User presses `L` (login) in TUI → opens system browser to `auth.jinwang.dev` (Keycloak)
2. **Flow**: OIDC authorization code flow with PKCE → local HTTP callback server (localhost:random_port) → receives token
3. **Token usage**: OIDC id_token replaces SA bearer token in kube client → K8s API verifies via `--oidc-issuer-url` (already configured in `k8s-clusters.yaml` OIDC section)
4. **RBAC**: Per-user permissions via Keycloak groups mapped to K8s ClusterRoles (`kube_oidc_groups_claim`/`kube_oidc_groups_prefix` already in cluster-vars)
5. **Roles**: viewer (default), editor, admin — mapped from Keycloak group membership
6. **Fallback**: SA token for headless/CI mode, OIDC token for interactive TUI
7. **Token refresh**: Background refresh before expiry, re-login prompt if refresh fails
8. **Dependencies**: `openidconnect` crate, `open` crate (browser launch), tiny HTTP server for callback

This enables the dash to support create/delete/scale operations per-user with proper audit trail.

- [ ] **Step 2: Commit**

```
docs: plan for Keycloak OIDC per-user auth in dash TUI
```
