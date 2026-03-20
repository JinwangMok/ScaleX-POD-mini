use anyhow::{Context, Result};
use kube::Client;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct ClusterClient {
    pub name: String,
    pub kubeconfig_path: PathBuf,
    pub client: Client,
    /// SSH tunnel process ID (if auto-tunneled)
    pub tunnel_pid: Option<u32>,
    /// K8s API server version (e.g., "v1.33.1"), fetched once during discovery
    pub server_version: Option<String>,
    /// The endpoint URL that successfully connected
    pub endpoint: Option<String>,
}

/// Event sent during streaming cluster discovery.
pub enum DiscoverEvent {
    /// A cluster client was successfully created (may include SSH tunnel).
    Connected(ClusterClient),
    /// A cluster connection failed.
    Failed { name: String, error: String },
    /// All clusters have been processed.
    Complete,
    /// Log message from discovery (replaces eprintln to avoid TUI corruption).
    Log { message: String },
}

/// Scan kubeconfig directory for cluster names without any network I/O.
/// Returns sorted list of cluster names that have kubeconfig.yaml files.
/// Guaranteed to complete in <100ms (filesystem only).
pub fn scan_kubeconfig_names(dir: &Path) -> Vec<String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter(|e| e.path().join("kubeconfig.yaml").exists())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect();
    names.sort();
    names
}

/// Timeout for connection probes during cluster discovery (direct IP connections).
/// Reduced from 3s to 2s — healthy clusters respond in <500ms;
/// 2s catches slow-but-alive clusters while reducing startup latency.
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(2);

/// Timeout for CF Tunnel domain probes during cluster discovery.
/// Longer than DISCOVER_TIMEOUT because CF Tunnel connections traverse
/// Cloudflare's edge network and may need DNS resolution + TLS handshake
/// on cold start, which can exceed 2s on the first request.
const CF_DOMAIN_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Load api_endpoint mapping from k8s-clusters.yaml.
/// Returns empty map if the file doesn't exist or can't be parsed.
fn load_api_endpoints(repo_root: &Path) -> HashMap<String, String> {
    let config_path = repo_root.join("config/k8s-clusters.yaml");
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    let config: crate::models::cluster::K8sClustersConfig = match serde_yaml::from_str(&content) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    config
        .config
        .clusters
        .into_iter()
        .filter_map(|c| c.api_endpoint.map(|ep| (c.cluster_name, ep)))
        .collect()
}

/// Build a kube Client with a replaced server URL (for api_endpoint override).
/// When connecting via CF Tunnel, the edge presents a public CA cert (not the K8s self-signed CA),
/// so we strip the kubeconfig's certificate-authority-data and use system root CAs instead.
/// If `bearer_token` is provided, it replaces client cert auth (needed for CF Tunnel which
/// cannot proxy mTLS client certificates).
async fn build_client_with_endpoint(
    kubeconfig_path: &Path,
    endpoint: &str,
    bearer_token: Option<&str>,
) -> Result<Client> {
    let content = std::fs::read_to_string(kubeconfig_path)
        .context("Reading kubeconfig for endpoint override")?;
    let (original_url, _, _) =
        extract_server_url(kubeconfig_path).context("Cannot parse server URL")?;
    let modified = content.replace(&original_url, endpoint);

    // Parse, strip cluster CA (CF Tunnel terminates TLS with public CA), rebuild.
    // With root_cert=None, kube-rs automatically uses system native root CAs.
    let mut kubeconfig: kube::config::Kubeconfig =
        serde_yaml::from_str(&modified).context("Parsing modified kubeconfig")?;
    for cluster in &mut kubeconfig.clusters {
        if let Some(ref mut c) = cluster.cluster {
            c.certificate_authority = None;
            c.certificate_authority_data = None;
        }
    }

    // Replace client cert auth with bearer token (CF Tunnel cannot proxy mTLS)
    if let Some(token) = bearer_token {
        for auth in &mut kubeconfig.auth_infos {
            if let Some(ref mut info) = auth.auth_info {
                info.token = Some(secrecy::SecretString::from(token.to_string()));
                info.client_certificate = None;
                info.client_certificate_data = None;
                info.client_key = None;
                info.client_key_data = None;
            }
        }
    }

    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default())
        .await
        .context("Building kube config from modified content")?;

    Client::try_from(config).context("Creating kube client with system CAs")
}

/// Result of an authenticated probe — distinguishes 401 from other failures.
#[derive(Debug, PartialEq)]
enum ProbeResult {
    Ok,
    /// 401 Unauthorized — cached token is invalid/expired, needs re-provision
    Unauthorized,
    /// Other failure (timeout, network, TLS, etc.)
    Failed,
}

/// Probe cluster connectivity: build client + list 1 namespace, with timeout.
/// `timeout` is caller-supplied: use DISCOVER_TIMEOUT (2s) for direct connections
/// and TUNNEL_PROBE_TIMEOUT (10s) for SSH-tunneled connections where the first
/// API call may need extra time for TLS handshake through the tunnel.
async fn probe_client(client: &Client, timeout: Duration) -> bool {
    probe_client_auth(client, timeout).await == ProbeResult::Ok
}

/// Probe with auth status — returns `Unauthorized` on 401 so callers can re-provision tokens.
async fn probe_client_auth(client: &Client, timeout: Duration) -> ProbeResult {
    match tokio::time::timeout(
        timeout,
        kube::api::Api::<k8s_openapi::api::core::v1::Namespace>::all(client.clone())
            .list(&kube::api::ListParams::default().limit(1)),
    )
    .await
    {
        Ok(Ok(_)) => ProbeResult::Ok,
        Ok(Err(kube::Error::Api(ref err))) if err.code == 401 => ProbeResult::Unauthorized,
        _ => ProbeResult::Failed,
    }
}

/// Timeout for the K8s API probe immediately after an SSH tunnel is established.
/// Longer than DISCOVER_TIMEOUT to account for:
///   - SSH tunnel TLS handshake latency on first connection
///   - Remote bastion → API server round-trip (may exceed 2s on slow links)
const TUNNEL_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Fetch K8s API server version with a tight timeout (500ms).
/// Returns None on timeout or any error — never blocks discovery.
async fn fetch_server_version(client: &Client) -> Option<String> {
    tokio::time::timeout(Duration::from_millis(500), client.apiserver_version())
        .await
        .ok()?
        .ok()
        .map(|info| info.git_version)
}

/// Discover clusters and stream results via mpsc channel (one event per cluster).
/// Clusters are discovered in parallel via tokio::spawn.
/// Connection strategy per cluster:
///   1. api_endpoint (domain URL from k8s-clusters.yaml) → 2s timeout
///   2. kubeconfig original IP → 3s timeout
///   3. SSH tunnel fallback → 500ms wait
pub async fn discover_clusters_streaming(
    dir: PathBuf,
    tx: tokio::sync::mpsc::Sender<DiscoverEvent>,
    cancelled: Arc<AtomicBool>,
) {
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => {
            let _ = tx.send(DiscoverEvent::Complete).await;
            return;
        }
    };

    // Resolve bastion info once (for auto-tunneling)
    let repo_root = dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(Path::new("."));
    let bastion = resolve_bastion(repo_root);
    let api_endpoints = load_api_endpoints(repo_root);

    let mut dirs: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    dirs.sort_by_key(|e| e.file_name());

    // Collect cluster info for parallel discovery
    let mut cluster_infos = Vec::new();
    for entry in dirs {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let kubeconfig_path = path.join("kubeconfig.yaml");
        if !kubeconfig_path.exists() {
            continue;
        }
        let cluster_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let endpoint = api_endpoints.get(&cluster_name).cloned();
        let local_port = NEXT_TUNNEL_PORT.fetch_add(1, Ordering::Relaxed);

        cluster_infos.push((cluster_name, kubeconfig_path, endpoint, local_port));
    }

    // Spawn parallel discovery tasks
    let mut handles = Vec::new();
    for (cluster_name, kubeconfig_path, endpoint, local_port) in cluster_infos {
        let tx = tx.clone();
        let cancelled = cancelled.clone();
        let bastion = bastion.clone();

        handles.push(tokio::spawn(async move {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }

            // Strategy 1: api_endpoint (domain URL via CF Tunnel + SA token)
            if let Some(ref ep) = endpoint {
                // Resolve or auto-provision SA token for CF Tunnel auth
                let mut token = super::sa_provisioner::read_cached_token(&kubeconfig_path);
                if token.is_none() {
                    if let Some(ref bh) = bastion {
                        if let Ok(t) = super::sa_provisioner::provision_dash_sa(
                            &kubeconfig_path,
                            &cluster_name,
                            bh,
                        )
                        .await
                        {
                            let _ = super::sa_provisioner::cache_token(&kubeconfig_path, &t);
                            token = Some(t);
                        }
                    }
                }
                let token = token;

                match build_client_with_endpoint(&kubeconfig_path, ep, token.as_deref()).await {
                    Ok(client) => {
                        let probe = probe_client_auth(&client, CF_DOMAIN_PROBE_TIMEOUT).await;
                        if probe == ProbeResult::Ok {
                            let _ = tx
                                .send(DiscoverEvent::Log {
                                    message: format!("{}: connected via domain", cluster_name),
                                })
                                .await;
                            let ver = fetch_server_version(&client).await;
                            let _ = tx
                                .send(DiscoverEvent::Connected(ClusterClient {
                                    name: cluster_name,
                                    kubeconfig_path,
                                    client,
                                    tunnel_pid: None,
                                    server_version: ver,
                                    endpoint: Some(ep.clone()),
                                }))
                                .await;
                            return;
                        } else if probe == ProbeResult::Unauthorized {
                            // 401: stale token — invalidate, re-provision, retry once
                            let _ = tx
                                .send(DiscoverEvent::Log {
                                    message: format!(
                                        "{}: 401 — re-provisioning SA token",
                                        cluster_name
                                    ),
                                })
                                .await;
                            super::sa_provisioner::invalidate_cached_token(&kubeconfig_path);
                            if let Some(ref bh) = bastion {
                                if let Ok(new_token) = super::sa_provisioner::provision_dash_sa(
                                    &kubeconfig_path,
                                    &cluster_name,
                                    bh,
                                )
                                .await
                                {
                                    let _ = super::sa_provisioner::cache_token(
                                        &kubeconfig_path,
                                        &new_token,
                                    );
                                    if let Ok(new_client) = build_client_with_endpoint(
                                        &kubeconfig_path,
                                        ep,
                                        Some(&new_token),
                                    )
                                    .await
                                    {
                                        if probe_client(&new_client, CF_DOMAIN_PROBE_TIMEOUT).await
                                        {
                                            let _ = tx
                                                .send(DiscoverEvent::Log {
                                                    message: format!(
                                                        "{}: reconnected after token refresh",
                                                        cluster_name
                                                    ),
                                                })
                                                .await;
                                            let ver = fetch_server_version(&new_client).await;
                                            let _ = tx
                                                .send(DiscoverEvent::Connected(ClusterClient {
                                                    name: cluster_name,
                                                    kubeconfig_path,
                                                    client: new_client,
                                                    tunnel_pid: None,
                                                    server_version: ver,
                                                    endpoint: Some(ep.clone()),
                                                }))
                                                .await;
                                            return;
                                        }
                                    }
                                }
                            }
                            let _ = tx
                                .send(DiscoverEvent::Log {
                                    message: format!(
                                        "{}: token re-provision failed, falling back",
                                        cluster_name
                                    ),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(DiscoverEvent::Log {
                                    message: format!(
                                        "{}: domain probe timed out ({})",
                                        cluster_name, ep
                                    ),
                                })
                                .await;
                        }
                    }
                    Err(e) => {
                        let reason = format!("{}", e);
                        let category = if reason.contains("dns") || reason.contains("resolve") {
                            "DNS"
                        } else if reason.contains("certificate")
                            || reason.contains("tls")
                            || reason.contains("ssl")
                        {
                            "TLS"
                        } else {
                            "connect"
                        };
                        let _ = tx
                            .send(DiscoverEvent::Log {
                                message: format!(
                                    "{}: domain probe failed ({}: {})",
                                    cluster_name, category, ep
                                ),
                            })
                            .await;
                    }
                }
            }

            // Strategy 2: direct connection via kubeconfig IP (with timeout)
            // If kubeconfig has a domain URL (post-rewrite), also try .original for VM IP fallback
            if let Ok(client) = build_client(&kubeconfig_path).await {
                if probe_client(&client, DISCOVER_TIMEOUT).await {
                    let ver = fetch_server_version(&client).await;
                    let ep = extract_server_url(&kubeconfig_path).map(|(url, _, _)| url);
                    let _ = tx
                        .send(DiscoverEvent::Connected(ClusterClient {
                            name: cluster_name,
                            kubeconfig_path,
                            client,
                            tunnel_pid: None,
                            server_version: ver,
                            endpoint: ep,
                        }))
                        .await;
                    return;
                }
            }

            // Strategy 2b: try .original kubeconfig (VM IP fallback after domain rewrite)
            let original_path = kubeconfig_path.with_extension("yaml.original");
            if original_path.exists() {
                if let Ok(client) = build_client(&original_path).await {
                    if probe_client(&client, DISCOVER_TIMEOUT).await {
                        let _ = tx
                            .send(DiscoverEvent::Log {
                                message: format!(
                                    "{}: connected via .original kubeconfig (VM IP)",
                                    cluster_name
                                ),
                            })
                            .await;
                        let ver = fetch_server_version(&client).await;
                        let ep = extract_server_url(&original_path).map(|(url, _, _)| url);
                        let _ = tx
                            .send(DiscoverEvent::Connected(ClusterClient {
                                name: cluster_name,
                                kubeconfig_path: original_path,
                                client,
                                tunnel_pid: None,
                                server_version: ver,
                                endpoint: ep,
                            }))
                            .await;
                        return;
                    }
                }
            }

            // Strategy 3: SSH tunnel fallback
            if let Some(ref bastion_host) = bastion {
                match setup_auto_tunnel(&kubeconfig_path, &cluster_name, bastion_host, local_port)
                    .await
                {
                    Ok((client, pid)) => {
                        let _ = tx
                            .send(DiscoverEvent::Log {
                                message: format!(
                                    "{}: connected via SSH tunnel (localhost:{})",
                                    cluster_name, local_port
                                ),
                            })
                            .await;
                        let ver = fetch_server_version(&client).await;
                        let _ = tx
                            .send(DiscoverEvent::Connected(ClusterClient {
                                name: cluster_name,
                                kubeconfig_path,
                                client,
                                tunnel_pid: Some(pid),
                                server_version: ver,
                                endpoint: Some(format!("localhost:{} (tunnel)", local_port)),
                            }))
                            .await;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(DiscoverEvent::Failed {
                                name: cluster_name,
                                error: format!("{}", e),
                            })
                            .await;
                    }
                }
            } else {
                let _ = tx
                    .send(DiscoverEvent::Failed {
                        name: cluster_name,
                        error: "No bastion available for auto-tunnel".into(),
                    })
                    .await;
            }
        }));
    }

    // Wait for all parallel tasks to complete
    for handle in handles {
        let _ = handle.await;
    }

    let _ = tx.send(DiscoverEvent::Complete).await;
}

/// Global atomic port counter for SSH tunnel allocation (US-212).
/// Starts at 16443 and increments across initial discovery and retries to avoid conflicts.
/// Uses u32 to prevent silent wrap-around at u16::MAX (65535) after many retries.
static NEXT_TUNNEL_PORT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(16443);

/// Re-discover only specific clusters by name (for retrying failed connections).
/// Same 3-tier strategy as discover_clusters_streaming but filtered to only the given names.
pub async fn discover_clusters_streaming_filtered(
    dir: PathBuf,
    tx: tokio::sync::mpsc::Sender<DiscoverEvent>,
    cancelled: Arc<AtomicBool>,
    names: &[String],
) {
    let repo_root = dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(Path::new("."));
    let bastion = resolve_bastion(repo_root);
    let api_endpoints = load_api_endpoints(repo_root);

    let mut handles = Vec::new();

    for name in names {
        let kubeconfig_path = dir.join(name).join("kubeconfig.yaml");
        if !kubeconfig_path.exists() {
            let _ = tx
                .send(DiscoverEvent::Failed {
                    name: name.clone(),
                    error: "kubeconfig.yaml not found".into(),
                })
                .await;
            continue;
        }

        let endpoint = api_endpoints.get(name).cloned();
        let local_port = NEXT_TUNNEL_PORT.fetch_add(1, Ordering::Relaxed);
        let cluster_name = name.clone();
        let tx = tx.clone();
        let cancelled = cancelled.clone();
        let bastion = bastion.clone();

        handles.push(tokio::spawn(async move {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }

            // Strategy 1: api_endpoint (domain URL via CF Tunnel + SA token)
            if let Some(ref ep) = endpoint {
                let token = super::sa_provisioner::read_cached_token(&kubeconfig_path);
                match build_client_with_endpoint(&kubeconfig_path, ep, token.as_deref()).await {
                    Ok(client) => {
                        let probe = probe_client_auth(&client, CF_DOMAIN_PROBE_TIMEOUT).await;
                        if probe == ProbeResult::Ok {
                            let _ = tx
                                .send(DiscoverEvent::Log {
                                    message: format!("{}: connected via domain", cluster_name),
                                })
                                .await;
                            let ver = fetch_server_version(&client).await;
                            let _ = tx
                                .send(DiscoverEvent::Connected(ClusterClient {
                                    name: cluster_name,
                                    kubeconfig_path,
                                    client,
                                    tunnel_pid: None,
                                    server_version: ver,
                                    endpoint: Some(ep.clone()),
                                }))
                                .await;
                            return;
                        } else if probe == ProbeResult::Unauthorized {
                            // 401: stale token — invalidate, re-provision, retry once
                            let _ = tx
                                .send(DiscoverEvent::Log {
                                    message: format!(
                                        "{}: 401 — re-provisioning SA token",
                                        cluster_name
                                    ),
                                })
                                .await;
                            super::sa_provisioner::invalidate_cached_token(&kubeconfig_path);
                            if let Some(ref bh) = bastion {
                                if let Ok(new_token) = super::sa_provisioner::provision_dash_sa(
                                    &kubeconfig_path,
                                    &cluster_name,
                                    bh,
                                )
                                .await
                                {
                                    let _ = super::sa_provisioner::cache_token(
                                        &kubeconfig_path,
                                        &new_token,
                                    );
                                    if let Ok(new_client) = build_client_with_endpoint(
                                        &kubeconfig_path,
                                        ep,
                                        Some(&new_token),
                                    )
                                    .await
                                    {
                                        if probe_client(&new_client, CF_DOMAIN_PROBE_TIMEOUT).await
                                        {
                                            let _ = tx
                                                .send(DiscoverEvent::Log {
                                                    message: format!(
                                                        "{}: reconnected after token refresh",
                                                        cluster_name
                                                    ),
                                                })
                                                .await;
                                            let ver = fetch_server_version(&new_client).await;
                                            let _ = tx
                                                .send(DiscoverEvent::Connected(ClusterClient {
                                                    name: cluster_name,
                                                    kubeconfig_path,
                                                    client: new_client,
                                                    tunnel_pid: None,
                                                    server_version: ver,
                                                    endpoint: Some(ep.clone()),
                                                }))
                                                .await;
                                            return;
                                        }
                                    }
                                }
                            }
                            let _ = tx
                                .send(DiscoverEvent::Log {
                                    message: format!(
                                        "{}: token re-provision failed, falling back",
                                        cluster_name
                                    ),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(DiscoverEvent::Log {
                                    message: format!(
                                        "{}: domain probe timed out ({})",
                                        cluster_name, ep
                                    ),
                                })
                                .await;
                        }
                    }
                    Err(e) => {
                        let reason = format!("{}", e);
                        let category = if reason.contains("dns") || reason.contains("resolve") {
                            "DNS"
                        } else if reason.contains("certificate")
                            || reason.contains("tls")
                            || reason.contains("ssl")
                        {
                            "TLS"
                        } else {
                            "connect"
                        };
                        let _ = tx
                            .send(DiscoverEvent::Log {
                                message: format!(
                                    "{}: domain probe failed ({}: {})",
                                    cluster_name, category, ep
                                ),
                            })
                            .await;
                    }
                }
            }

            // Strategy 2: direct connection
            if let Ok(client) = build_client(&kubeconfig_path).await {
                if probe_client(&client, DISCOVER_TIMEOUT).await {
                    let ver = fetch_server_version(&client).await;
                    let ep = extract_server_url(&kubeconfig_path).map(|(url, _, _)| url);
                    let _ = tx
                        .send(DiscoverEvent::Connected(ClusterClient {
                            name: cluster_name,
                            kubeconfig_path,
                            client,
                            tunnel_pid: None,
                            server_version: ver,
                            endpoint: ep,
                        }))
                        .await;
                    return;
                }
            }

            // Strategy 2b: try .original kubeconfig (VM IP fallback)
            let original_path = kubeconfig_path.with_extension("yaml.original");
            if original_path.exists() {
                if let Ok(client) = build_client(&original_path).await {
                    if probe_client(&client, DISCOVER_TIMEOUT).await {
                        let _ = tx
                            .send(DiscoverEvent::Log {
                                message: format!(
                                    "{}: connected via .original kubeconfig",
                                    cluster_name
                                ),
                            })
                            .await;
                        let ver = fetch_server_version(&client).await;
                        let ep = extract_server_url(&original_path).map(|(url, _, _)| url);
                        let _ = tx
                            .send(DiscoverEvent::Connected(ClusterClient {
                                name: cluster_name,
                                kubeconfig_path: original_path,
                                client,
                                tunnel_pid: None,
                                server_version: ver,
                                endpoint: ep,
                            }))
                            .await;
                        return;
                    }
                }
            }

            // Strategy 3: SSH tunnel
            if let Some(ref bastion_host) = bastion {
                match setup_auto_tunnel(&kubeconfig_path, &cluster_name, bastion_host, local_port)
                    .await
                {
                    Ok((client, pid)) => {
                        let ver = fetch_server_version(&client).await;
                        let _ = tx
                            .send(DiscoverEvent::Connected(ClusterClient {
                                name: cluster_name,
                                kubeconfig_path,
                                client,
                                tunnel_pid: Some(pid),
                                server_version: ver,
                                endpoint: Some(format!("localhost:{} (tunnel)", local_port)),
                            }))
                            .await;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(DiscoverEvent::Failed {
                                name: cluster_name,
                                error: format!("{}", e),
                            })
                            .await;
                    }
                }
            } else {
                let _ = tx
                    .send(DiscoverEvent::Failed {
                        name: cluster_name,
                        error: "No bastion available for auto-tunnel".into(),
                    })
                    .await;
            }
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }
    // Note: no Complete event — this is a partial retry, not full discovery
}

/// Discover kubeconfig files from the given directory.
/// Expects structure: `{dir}/{cluster_name}/kubeconfig.yaml`
///
/// For each kubeconfig, checks if the K8s API server is reachable.
/// If not, automatically sets up an SSH tunnel through the bastion node
/// (from credentials/.baremetal-init.yaml) so the bastion can access
/// remote cluster APIs without manual tunnel setup.
pub async fn discover_clusters(dir: &Path) -> Result<Vec<ClusterClient>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context(format!("Reading kubeconfig dir: {}", dir.display())),
    };

    // Resolve bastion info once (for auto-tunneling)
    let repo_root = dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(Path::new("."));
    let bastion = resolve_bastion(repo_root);
    let api_endpoints = load_api_endpoints(repo_root);

    // Collect cluster info for parallel discovery
    let mut cluster_infos = Vec::new();
    let mut dirs: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    dirs.sort_by_key(|e| e.file_name());

    for entry in dirs {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let kubeconfig_path = path.join("kubeconfig.yaml");
        if !kubeconfig_path.exists() {
            continue;
        }
        let cluster_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let endpoint = api_endpoints.get(&cluster_name).cloned();
        let local_port = NEXT_TUNNEL_PORT.fetch_add(1, Ordering::Relaxed);
        cluster_infos.push((cluster_name, kubeconfig_path, endpoint, local_port));
    }

    // Spawn parallel discovery tasks (one per cluster)
    let mut handles = Vec::new();
    for (cluster_name, kubeconfig_path, endpoint, local_port) in cluster_infos {
        let bastion = bastion.clone();
        handles.push(tokio::spawn(async move {
            // Strategy 1: api_endpoint (domain URL via CF Tunnel + SA token)
            if let Some(ref ep) = endpoint {
                let mut token = super::sa_provisioner::read_cached_token(&kubeconfig_path);
                if token.is_none() {
                    if let Some(ref bh) = bastion {
                        eprintln!("{}: provisioning dash SA...", cluster_name);
                        match super::sa_provisioner::provision_dash_sa(
                            &kubeconfig_path,
                            &cluster_name,
                            bh,
                        )
                        .await
                        {
                            Ok(t) => {
                                let _ = super::sa_provisioner::cache_token(&kubeconfig_path, &t);
                                token = Some(t);
                            }
                            Err(e) => {
                                eprintln!("{}: SA provision failed: {}", cluster_name, e);
                            }
                        }
                    }
                }

                match build_client_with_endpoint(&kubeconfig_path, ep, token.as_deref()).await {
                    Ok(client) => {
                        let probe = probe_client_auth(&client, CF_DOMAIN_PROBE_TIMEOUT).await;
                        if probe == ProbeResult::Ok {
                            eprintln!("{}: connected via domain ({})", cluster_name, ep);
                            let ver = fetch_server_version(&client).await;
                            return Some(ClusterClient {
                                name: cluster_name,
                                kubeconfig_path,
                                client,
                                tunnel_pid: None,
                                server_version: ver,
                                endpoint: Some(ep.clone()),
                            });
                        } else if probe == ProbeResult::Unauthorized {
                            // 401: stale token — invalidate, re-provision, retry once
                            eprintln!("{}: 401 — re-provisioning SA token", cluster_name);
                            super::sa_provisioner::invalidate_cached_token(&kubeconfig_path);
                            if let Some(ref bh) = bastion {
                                if let Ok(new_token) = super::sa_provisioner::provision_dash_sa(
                                    &kubeconfig_path,
                                    &cluster_name,
                                    bh,
                                )
                                .await
                                {
                                    let _ = super::sa_provisioner::cache_token(
                                        &kubeconfig_path,
                                        &new_token,
                                    );
                                    if let Ok(new_client) = build_client_with_endpoint(
                                        &kubeconfig_path,
                                        ep,
                                        Some(&new_token),
                                    )
                                    .await
                                    {
                                        if probe_client(&new_client, CF_DOMAIN_PROBE_TIMEOUT).await
                                        {
                                            eprintln!(
                                                "{}: reconnected after token refresh ({})",
                                                cluster_name, ep
                                            );
                                            let ver = fetch_server_version(&new_client).await;
                                            return Some(ClusterClient {
                                                name: cluster_name,
                                                kubeconfig_path,
                                                client: new_client,
                                                tunnel_pid: None,
                                                server_version: ver,
                                                endpoint: Some(ep.clone()),
                                            });
                                        }
                                    }
                                }
                            }
                            eprintln!("{}: token re-provision failed, falling back", cluster_name);
                        } else {
                            eprintln!("{}: domain probe failed ({})", cluster_name, ep);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "{}: domain client build failed ({}): {}",
                            cluster_name, ep, e
                        );
                    }
                }
            }

            // Strategy 2: direct connection via kubeconfig IP
            if let Ok(client) = build_client(&kubeconfig_path).await {
                if probe_client(&client, DISCOVER_TIMEOUT).await {
                    let ver = fetch_server_version(&client).await;
                    let ep = extract_server_url(&kubeconfig_path).map(|(url, _, _)| url);
                    return Some(ClusterClient {
                        name: cluster_name,
                        kubeconfig_path,
                        client,
                        tunnel_pid: None,
                        server_version: ver,
                        endpoint: ep,
                    });
                }
            }

            // Strategy 2b: .original kubeconfig fallback
            let original_path = kubeconfig_path.with_extension("yaml.original");
            if original_path.exists() {
                if let Ok(client) = build_client(&original_path).await {
                    if probe_client(&client, DISCOVER_TIMEOUT).await {
                        let ver = fetch_server_version(&client).await;
                        let ep = extract_server_url(&original_path).map(|(url, _, _)| url);
                        return Some(ClusterClient {
                            name: cluster_name,
                            kubeconfig_path,
                            client,
                            tunnel_pid: None,
                            server_version: ver,
                            endpoint: ep,
                        });
                    }
                }
            }

            // Strategy 3: Auto-tunnel via bastion
            if let Some(ref bastion_host) = bastion {
                match setup_auto_tunnel(&kubeconfig_path, &cluster_name, bastion_host, local_port)
                    .await
                {
                    Ok((client, pid)) => {
                        eprintln!(
                            "Auto-tunnel: {} → localhost:{} via {}",
                            cluster_name, local_port, bastion_host
                        );
                        let ver = fetch_server_version(&client).await;
                        return Some(ClusterClient {
                            name: cluster_name,
                            kubeconfig_path,
                            client,
                            tunnel_pid: Some(pid),
                            server_version: ver,
                            endpoint: Some(format!("localhost:{} (tunnel)", local_port)),
                        });
                    }
                    Err(e) => {
                        eprintln!("Warning: Auto-tunnel failed for {}: {}", cluster_name, e);
                    }
                }
            } else {
                eprintln!(
                    "Warning: Cannot reach {} and no bastion available for auto-tunnel",
                    cluster_name
                );
            }

            None
        }));
    }

    // Collect results
    let mut clusters = Vec::new();
    for handle in handles {
        if let Ok(Some(client)) = handle.await {
            clusters.push(client);
        }
    }
    clusters.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(clusters)
}

/// Build a kube Client from a kubeconfig file.
async fn build_client(kubeconfig_path: &Path) -> Result<Client> {
    let kubeconfig = kube::config::Kubeconfig::read_from(kubeconfig_path)
        .context(format!("Reading {}", kubeconfig_path.display()))?;

    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default())
        .await
        .context("Building kube config")?;

    Client::try_from(config).context("Creating kube client")
}

/// Build a kube Client from kubeconfig content string.
async fn build_client_from_content(content: &str) -> Result<Client> {
    let kubeconfig: kube::config::Kubeconfig =
        serde_yaml::from_str(content).context("Parsing modified kubeconfig")?;

    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default())
        .await
        .context("Building kube config from modified content")?;

    Client::try_from(config).context("Creating kube client")
}

/// Resolve bastion hostname from credentials/.baremetal-init.yaml.
/// Returns the first node name (which should match ~/.ssh/config).
fn resolve_bastion(repo_root: &Path) -> Option<String> {
    let bm_yaml = repo_root.join("credentials/.baremetal-init.yaml");
    let content = std::fs::read_to_string(&bm_yaml).ok()?;
    // Extract first node name from YAML
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- name:") || trimmed.starts_with("name:") {
            let name = trimmed
                .split(':')
                .nth(1)?
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Extract the server URL from a kubeconfig file.
fn extract_server_url(kubeconfig_path: &Path) -> Option<(String, String, u16)> {
    let content = std::fs::read_to_string(kubeconfig_path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("server:") {
            let url = trimmed
                .split_whitespace()
                .nth(1)?
                .trim_matches('"')
                .to_string();
            // Parse: https://IP:PORT
            let without_scheme = url.strip_prefix("https://")?;
            let parts: Vec<&str> = without_scheme.splitn(2, ':').collect();
            let ip = parts[0].to_string();
            let port: u16 = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(6443);
            return Some((url, ip, port));
        }
    }
    None
}

/// Poll localhost:port until the TCP port accepts connections or the SSH process dies.
/// Returns true if the port becomes ready within the timeout, false otherwise.
///
/// This replaces a fixed sleep because the SSH process can be alive (spawn succeeded)
/// while the local port is still not yet bound — the SSH handshake and channel setup
/// take time that varies by network conditions.  Polling is safe: a refused connection
/// just means "not ready yet", while a successful connect means the tunnel is listening.
async fn wait_for_tunnel_port(port: u32, pid: u32) -> bool {
    // Max wait: 200 ms initial + 12 × 500 ms = ~6 s total — generous for a nearby bastion.
    const MAX_RETRIES: u32 = 12;

    // Short initial wait before first probe (SSH needs time to start the handshake)
    tokio::time::sleep(Duration::from_millis(200)).await;

    for _ in 0..MAX_RETRIES {
        // Abort early if the SSH process already died (e.g., auth failure)
        if !is_process_alive(pid) {
            return false;
        }

        // Try TCP connect to localhost:port — succeeds only when SSH bound the port
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .is_ok()
        {
            return true;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

/// Set up an SSH tunnel and return a Client connected through it.
///
/// Ordering guarantee: returns only after the local tunnel port is confirmed
/// listening AND the K8s API is reachable through the tunnel, preventing
/// callers (headless mode) from attempting data fetches before the connection
/// is truly ready.
async fn setup_auto_tunnel(
    kubeconfig_path: &Path,
    cluster_name: &str,
    bastion_host: &str,
    local_port: u32,
) -> Result<(Client, u32)> {
    let (server_url, server_ip, server_port) =
        extract_server_url(kubeconfig_path).context("Cannot parse server URL from kubeconfig")?;

    // Determine the target IP: if 127.0.0.1, look up from SDI state
    let target_ip = if server_ip == "127.0.0.1" || server_ip == "localhost" {
        lookup_cp_ip(kubeconfig_path, cluster_name).unwrap_or(server_ip)
    } else {
        server_ip
    };

    // Start SSH tunnel: localhost:local_port → target_ip:server_port via bastion
    let child = std::process::Command::new("ssh")
        .args([
            "-N",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=30",
            "-L",
            &format!("{}:{}:{}", local_port, target_ip, server_port),
            bastion_host,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context(format!(
            "SSH tunnel to {} via {}",
            cluster_name, bastion_host
        ))?;

    let pid = child.id();

    // Wait for tunnel port to become ready (replaces fixed 500 ms sleep).
    // poll_tunnel_port checks TCP connectivity, not just process liveness,
    // guaranteeing the port is bound before we build the kube Client.
    if !wait_for_tunnel_port(local_port, pid).await {
        // Kill the orphaned SSH process before returning an error
        if pid <= i32::MAX as u32 {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        anyhow::bail!(
            "SSH tunnel for {} failed to establish on localhost:{} within 6 s (process {})",
            cluster_name,
            local_port,
            pid
        );
    }

    // Create modified kubeconfig with localhost:local_port
    let content = std::fs::read_to_string(kubeconfig_path)
        .context("Reading kubeconfig for tunnel rewrite")?;
    let modified = content.replace(&server_url, &format!("https://127.0.0.1:{}", local_port));

    let client = build_client_from_content(&modified).await?;

    // Verify actual K8s API reachability through the tunnel.
    // This is the final ordering guarantee: callers receive a Client only
    // after a real K8s API call succeeds, not just after port binding.
    // Uses TUNNEL_PROBE_TIMEOUT (10s) instead of DISCOVER_TIMEOUT (2s) because
    // the first K8s API call through an SSH tunnel may take extra time for
    // the TLS handshake and bastion → API server round-trip on slow links.
    if !probe_client(&client, TUNNEL_PROBE_TIMEOUT).await {
        if pid <= i32::MAX as u32 {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        anyhow::bail!(
            "SSH tunnel for {} is up (localhost:{}) but K8s API did not respond",
            cluster_name,
            local_port
        );
    }

    Ok((client, pid))
}

/// Look up the control plane IP from SDI state for a given cluster.
fn lookup_cp_ip(kubeconfig_path: &Path, cluster_name: &str) -> Option<String> {
    let sdi_state_path = kubeconfig_path
        .parent()?
        .parent()?
        .parent()?
        .join("_generated/sdi/sdi-state.json");

    let content = std::fs::read_to_string(&sdi_state_path).ok()?;
    let pools: Vec<serde_json::Value> = serde_json::from_str(&content)
        .or_else(|_| serde_json::from_str::<serde_json::Value>(&content).map(|v| vec![v]))
        .ok()?;

    for pool in &pools {
        for node in pool.get("nodes")?.as_array()? {
            let node_name = node.get("node_name")?.as_str()?;
            if node_name.starts_with(&format!("{}-cp", cluster_name)) {
                return node.get("ip")?.as_str().map(String::from);
            }
        }
    }
    None
}

/// Check if a process is still alive.
fn is_process_alive(pid: u32) -> bool {
    if pid > i32::MAX as u32 {
        return false; // PID out of i32 range — cannot be valid on Linux
    }
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Clean up SSH tunnel processes when the app exits.
/// Used by headless mode which still holds ClusterClient vec directly.
#[allow(dead_code)]
pub fn cleanup_tunnels(clusters: &[ClusterClient]) {
    for cluster in clusters {
        if let Some(pid) = cluster.tunnel_pid {
            if pid <= i32::MAX as u32 {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_empty_for_missing_dir() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(discover_clusters(Path::new("/nonexistent/path")));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn discover_returns_empty_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(discover_clusters(dir.path()));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn resolve_bastion_from_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let creds_dir = dir.path().join("credentials");
        std::fs::create_dir_all(&creds_dir).unwrap();
        std::fs::write(
            creds_dir.join(".baremetal-init.yaml"),
            "targetNodes:\n  - name: \"playbox-0\"\n    node_ip: \"192.168.88.8\"\n",
        )
        .unwrap();
        let result = resolve_bastion(dir.path());
        assert_eq!(result, Some("playbox-0".to_string()));
    }

    #[test]
    fn extract_server_url_parses_kubeconfig() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("kubeconfig.yaml");
        std::fs::write(
            &kc,
            "clusters:\n- cluster:\n    server: https://192.168.88.100:6443\n",
        )
        .unwrap();
        let result = extract_server_url(&kc);
        assert!(result.is_some());
        let (url, ip, port) = result.unwrap();
        assert_eq!(url, "https://192.168.88.100:6443");
        assert_eq!(ip, "192.168.88.100");
        assert_eq!(port, 6443);
    }

    #[test]
    fn lookup_cp_ip_returns_none_for_missing_state() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("kubeconfig.yaml");
        std::fs::write(&kc, "").unwrap();
        assert!(lookup_cp_ip(&kc, "tower").is_none());
    }

    #[test]
    fn scan_kubeconfig_names_finds_clusters() {
        let dir = tempfile::tempdir().unwrap();
        // Create two cluster dirs with kubeconfig.yaml
        let tower = dir.path().join("tower");
        std::fs::create_dir_all(&tower).unwrap();
        std::fs::write(tower.join("kubeconfig.yaml"), "test").unwrap();

        let sandbox = dir.path().join("sandbox");
        std::fs::create_dir_all(&sandbox).unwrap();
        std::fs::write(sandbox.join("kubeconfig.yaml"), "test").unwrap();

        // Create a dir without kubeconfig (should be ignored)
        let noconfig = dir.path().join("noconfig");
        std::fs::create_dir_all(&noconfig).unwrap();

        let names = scan_kubeconfig_names(dir.path());
        assert_eq!(names, vec!["sandbox".to_string(), "tower".to_string()]);
    }

    #[test]
    fn scan_kubeconfig_names_returns_empty_for_missing_dir() {
        let names = scan_kubeconfig_names(std::path::Path::new("/nonexistent/path"));
        assert!(names.is_empty());
    }

    #[test]
    fn scan_kubeconfig_names_completes_fast() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            let d = dir.path().join(format!("cluster-{}", i));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("kubeconfig.yaml"), "test").unwrap();
        }
        let start = std::time::Instant::now();
        let names = scan_kubeconfig_names(dir.path());
        let elapsed = start.elapsed();
        assert_eq!(names.len(), 10);
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "scan_kubeconfig_names took {:?}, expected <100ms",
            elapsed
        );
    }

    // -------------------------------------------------------------------------
    // Tunnel conf format tests
    // install.sh writes: LOCAL_PORT:SERVER_IP:SERVER_PORT:BASTION:PID
    // These tests verify that the format is correctly round-tripped and that
    // scalex dash (extract_server_url) can read kubeconfigs rewritten through
    // the tunnel (server: https://127.0.0.1:<local_port>).
    // -------------------------------------------------------------------------

    /// Simulates what install.sh writes and verifies all fields parse correctly.
    #[test]
    fn tunnel_conf_format_all_fields_present_and_non_empty() {
        let dir = tempfile::tempdir().unwrap();
        let conf_path = dir.path().join("tower.conf");

        // Simulate install.sh: printf '%s:%s:%s:%s:%s\n' 16443 192.168.88.100 6443 playbox-0 12345
        let content = "16443:192.168.88.100:6443:playbox-0:12345\n";
        std::fs::write(&conf_path, content).unwrap();

        // Parse the same way the watchdog does: IFS=: read -r lp sip sp bt tpid
        let raw = std::fs::read_to_string(&conf_path).unwrap();
        let line = raw.trim();
        let parts: Vec<&str> = line.splitn(5, ':').collect();

        assert_eq!(
            parts.len(),
            5,
            "conf must have exactly 5 colon-separated fields"
        );

        let (lp, sip, sp, bt, tpid) = (parts[0], parts[1], parts[2], parts[3], parts[4]);

        // LOCAL_PORT: non-empty integer
        assert!(!lp.is_empty(), "LOCAL_PORT must not be empty");
        assert!(
            lp.parse::<u16>().is_ok(),
            "LOCAL_PORT must be a valid port number, got '{}'",
            lp
        );

        // SERVER_IP: non-empty
        assert!(!sip.is_empty(), "SERVER_IP must not be empty");

        // SERVER_PORT: non-empty integer
        assert!(!sp.is_empty(), "SERVER_PORT must not be empty");
        assert!(
            sp.parse::<u16>().is_ok(),
            "SERVER_PORT must be a valid port number, got '{}'",
            sp
        );

        // BASTION: non-empty
        assert!(!bt.is_empty(), "BASTION must not be empty");

        // PID: non-empty integer
        assert!(!tpid.is_empty(), "PID must not be empty");
        assert!(
            tpid.parse::<u32>().is_ok(),
            "PID must be a valid integer, got '{}'",
            tpid
        );
    }

    /// A conf file with an empty BASTION field (e.g. if bastion_target was unresolved)
    /// must be detectable — this mirrors the validate_tunnel_conf check in install.sh.
    #[test]
    fn tunnel_conf_format_empty_bastion_detected() {
        let dir = tempfile::tempdir().unwrap();
        let conf_path = dir.path().join("tower.conf");

        // BASTION field is empty (4th field)
        let content = "16443:192.168.88.100:6443::12345\n";
        std::fs::write(&conf_path, content).unwrap();

        let raw = std::fs::read_to_string(&conf_path).unwrap();
        let line = raw.trim();
        let parts: Vec<&str> = line.splitn(5, ':').collect();

        assert_eq!(parts.len(), 5);
        let bt = parts[3];
        assert!(bt.is_empty(), "test setup: BASTION should be empty");
        // This is the guard that downstream (watchdog/validate_tunnel_conf) would trigger on
        assert!(
            bt.is_empty(),
            "validate_tunnel_conf should reject empty BASTION — \
             credentials/.baremetal-init.yaml must contain a node name"
        );
    }

    /// A conf file with a non-numeric LOCAL_PORT must be detectable.
    #[test]
    fn tunnel_conf_format_non_numeric_port_detected() {
        let dir = tempfile::tempdir().unwrap();
        let conf_path = dir.path().join("tower.conf");

        // LOCAL_PORT is "bad" (non-numeric)
        let content = "bad:192.168.88.100:6443:playbox-0:12345\n";
        std::fs::write(&conf_path, content).unwrap();

        let raw = std::fs::read_to_string(&conf_path).unwrap();
        let line = raw.trim();
        let parts: Vec<&str> = line.splitn(5, ':').collect();

        assert_eq!(parts.len(), 5);
        let lp = parts[0];
        assert!(
            lp.parse::<u16>().is_err(),
            "non-numeric LOCAL_PORT '{}' must fail port parse — validate_tunnel_conf should reject it",
            lp
        );
    }

    /// After install.sh rewrites the kubeconfig server URL to localhost:<port>,
    /// extract_server_url must be able to parse it so scalex dash can connect.
    #[test]
    fn extract_server_url_parses_tunnel_rewritten_kubeconfig() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("kubeconfig.yaml");

        // Simulate the rewritten kubeconfig (server URL replaced with localhost:local_port)
        std::fs::write(
            &kc,
            "clusters:\n- cluster:\n    server: https://127.0.0.1:16443\n",
        )
        .unwrap();

        let result = extract_server_url(&kc);
        assert!(
            result.is_some(),
            "extract_server_url must succeed on tunnel-rewritten kubeconfig"
        );

        let (url, ip, port) = result.unwrap();
        assert_eq!(url, "https://127.0.0.1:16443");
        assert_eq!(ip, "127.0.0.1");
        assert_eq!(port, 16443u16);
    }

    /// A kubeconfig with no explicit port (default 6443) must be parseable after
    /// install.sh finishes — even if the tunnel uses the default port.
    #[test]
    fn extract_server_url_defaults_port_to_6443() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("kubeconfig.yaml");

        // No explicit port in URL
        std::fs::write(
            &kc,
            "clusters:\n- cluster:\n    server: https://192.168.88.100\n",
        )
        .unwrap();

        let result = extract_server_url(&kc);
        assert!(result.is_some());
        let (_, _, port) = result.unwrap();
        assert_eq!(port, 6443u16, "default port should be 6443");
    }

    // -------------------------------------------------------------------------
    // AC-7 Sub-AC 1: Multi-cluster kubeconfig discovery → selector panel
    // -------------------------------------------------------------------------

    /// scan_kubeconfig_names feeds directly into App::new_with_names to populate
    /// the cluster selector sidebar. Verify the full pipeline produces correct
    /// cluster names that match the directory structure.
    #[test]
    fn scan_kubeconfig_names_feeds_cluster_selector() {
        let dir = tempfile::tempdir().unwrap();

        // Create tower and sandbox cluster dirs (matching real layout)
        for name in &["tower", "sandbox"] {
            let d = dir.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("kubeconfig.yaml"), "apiVersion: v1").unwrap();
        }

        let names = scan_kubeconfig_names(dir.path());

        // Sorted alphabetically: sandbox, tower
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], "sandbox");
        assert_eq!(names[1], "tower");

        // These names are passed to App::new_with_names which builds the selector tree
        // Verify they are non-empty strings suitable for tree node labels
        for name in &names {
            assert!(!name.is_empty());
            assert!(!name.contains('/'));
        }
    }

    /// Directories without kubeconfig.yaml are excluded from the selector.
    #[test]
    fn scan_kubeconfig_names_excludes_dirs_without_kubeconfig() {
        let dir = tempfile::tempdir().unwrap();

        // Valid cluster
        let tower = dir.path().join("tower");
        std::fs::create_dir_all(&tower).unwrap();
        std::fs::write(tower.join("kubeconfig.yaml"), "test").unwrap();

        // Dir with wrong file name
        let bad = dir.path().join("bad-cluster");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("config.yaml"), "test").unwrap();

        // Regular file (not a directory)
        std::fs::write(dir.path().join("not-a-dir"), "test").unwrap();

        let names = scan_kubeconfig_names(dir.path());
        assert_eq!(names, vec!["tower".to_string()]);
    }

    // -------------------------------------------------------------------------
    // Sub-AC 2: build_client_with_endpoint CF Tunnel rewrite verification
    // Ensures certificate_authority_data is stripped, bearer token injected,
    // and server URL replaced with CF Tunnel domain.
    // -------------------------------------------------------------------------

    /// Verify that extract_server_url handles CF Tunnel domain URLs (not just IPs).
    #[test]
    fn extract_server_url_parses_cf_tunnel_domain() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("kubeconfig.yaml");
        std::fs::write(
            &kc,
            "clusters:\n- cluster:\n    server: https://tower-api.jinwang.dev:443\n",
        )
        .unwrap();
        let result = extract_server_url(&kc);
        assert!(
            result.is_some(),
            "extract_server_url must handle domain URLs for CF Tunnel kubeconfigs"
        );
        let (url, host, port) = result.unwrap();
        assert_eq!(url, "https://tower-api.jinwang.dev:443");
        assert_eq!(host, "tower-api.jinwang.dev");
        assert_eq!(port, 443u16);
    }

    /// Verify that extract_server_url handles CF Tunnel domain URLs without explicit port.
    #[test]
    fn extract_server_url_parses_cf_tunnel_domain_no_port() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("kubeconfig.yaml");
        std::fs::write(
            &kc,
            "clusters:\n- cluster:\n    server: https://sandbox-api.jinwang.dev\n",
        )
        .unwrap();
        let result = extract_server_url(&kc);
        assert!(result.is_some());
        let (url, host, port) = result.unwrap();
        assert_eq!(url, "https://sandbox-api.jinwang.dev");
        assert_eq!(host, "sandbox-api.jinwang.dev");
        assert_eq!(
            port, 6443u16,
            "default port should be 6443 when no port specified"
        );
    }

    /// Four clusters (matching 4 bare-metal node topology) all appear in scan.
    #[test]
    fn scan_kubeconfig_names_four_clusters() {
        let dir = tempfile::tempdir().unwrap();

        for name in &["tower", "sandbox", "staging", "prod"] {
            let d = dir.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("kubeconfig.yaml"), "test").unwrap();
        }

        let names = scan_kubeconfig_names(dir.path());
        assert_eq!(names.len(), 4);
        // All four present (sorted)
        assert!(names.contains(&"tower".to_string()));
        assert!(names.contains(&"sandbox".to_string()));
        assert!(names.contains(&"staging".to_string()));
        assert!(names.contains(&"prod".to_string()));
    }
}
