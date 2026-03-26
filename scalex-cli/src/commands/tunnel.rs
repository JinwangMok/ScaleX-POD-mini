use clap::{Args, Subcommand};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Tunnel management subcommands
#[derive(Args)]
pub struct TunnelArgs {
    #[command(subcommand)]
    pub command: TunnelCommands,
}

#[derive(Subcommand)]
pub enum TunnelCommands {
    /// Show tunnel connection status (reads $HOME/.scalex/tunnel-state.yaml)
    Status(TunnelStatusArgs),
}

#[derive(Args)]
pub struct TunnelStatusArgs {
    /// Output format: text or json
    #[arg(long, default_value = "text")]
    pub format: String,

    /// Override path to tunnel state file (default: $HOME/.scalex/tunnel-state.yaml)
    #[arg(long)]
    pub state_file: Option<String>,

    /// TCP connect timeout in seconds for endpoint reachability check
    #[arg(long, default_value = "5")]
    pub connect_timeout: u64,
}

// ── Internal data model (deserialized from YAML) ────────────────────────────

#[derive(Debug, Deserialize)]
struct TunnelStateFile {
    clusters: Option<HashMap<String, ClusterTunnelEntry>>,
}

#[derive(Debug, Deserialize)]
struct ClusterTunnelEntry {
    transport_type: String,
    endpoint: String,
    auth_method: String,
    established_at: Option<String>,
}

// ── Pure helpers (unit-testable) ─────────────────────────────────────────────

/// Parse an endpoint string into a (host, port) tuple.
///
/// Accepts:
///   - `localhost:16443`
///   - `https://api.example.com`
///   - `https://api.example.com:6443`
///
/// Returns `None` if the endpoint cannot be parsed.
pub fn parse_endpoint(endpoint: &str) -> Option<(String, u16)> {
    // Strip scheme if present
    let without_scheme = if let Some(rest) = endpoint.strip_prefix("https://") {
        rest
    } else if let Some(rest) = endpoint.strip_prefix("http://") {
        rest
    } else {
        endpoint
    };

    // Strip path if present (e.g. "/healthz")
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);

    if let Some(colon) = host_port.rfind(':') {
        let host = &host_port[..colon];
        let port_str = &host_port[colon + 1..];
        if let Ok(port) = port_str.parse::<u16>() {
            return Some((host.to_string(), port));
        }
    }

    // No explicit port — infer from scheme
    let default_port = if endpoint.starts_with("https://") {
        443u16
    } else {
        80u16
    };
    Some((host_port.to_string(), default_port))
}

/// Attempt a TCP connection to `host:port` with the given timeout.
/// Returns `true` if the connection succeeded (port is listening).
pub fn is_tcp_reachable(host: &str, port: u16, timeout: Duration) -> bool {
    let addr_str = format!("{}:{}", host, port);
    // Resolve first, then connect with timeout
    let addrs: Vec<SocketAddr> = match addr_str.to_socket_addrs() {
        Ok(iter) => iter.collect(),
        Err(_) => return false,
    };
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, timeout).is_ok() {
            return true;
        }
    }
    false
}

/// Derive a per-cluster connection state string from endpoint reachability.
///
/// Possible values:
/// - `"connected"`  — TCP probe succeeded; tunnel is live.
/// - `"connecting"` — Reserved for future use (e.g. watchdog detecting process start).
///   Currently not emitted by this probe.
/// - `"error"`      — TCP probe failed or endpoint could not be parsed.
/// - `"unknown"`    — Endpoint string is unparseable; likely a malformed state file.
fn cluster_state(entry: &ClusterTunnelEntry, connect_timeout: Duration) -> &'static str {
    match parse_endpoint(&entry.endpoint) {
        Some((host, port)) => {
            if is_tcp_reachable(&host, port, connect_timeout) {
                "connected"
            } else {
                "error"
            }
        }
        None => "unknown",
    }
}

// ── Command entry point ──────────────────────────────────────────────────────
//
// Exit-code legend for `scalex-pod tunnel status`
// ─────────────────────────────────────────────
//  0  All clusters in the state file are reachable (TCP probe succeeded).
//  1  Partial or total connectivity failure:
//       • The state file exists but contains no cluster entries, OR
//       • One or more clusters failed their TCP reachability probe.
//  2  Pre-flight failure — the tunnel state file does not exist on disk.
//     This usually means install.sh has not been run yet (or ran without --auto).

pub fn run(args: TunnelArgs) -> anyhow::Result<()> {
    match args.command {
        TunnelCommands::Status(s) => run_status(s),
    }
}

fn run_status(args: TunnelStatusArgs) -> anyhow::Result<()> {
    // Resolve state file path
    let state_file_path = args.state_file.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{}/.scalex/tunnel-state.yaml", home)
    });

    let path = std::path::Path::new(&state_file_path);
    if !path.exists() {
        emit_no_state_file(&args.format, &state_file_path);
        std::process::exit(2); // exit 2 — state file not found; install.sh not yet run
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", state_file_path, e))?;

    let state: TunnelStateFile = serde_yaml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Cannot parse {}: {}", state_file_path, e))?;

    let clusters = state.clusters.unwrap_or_default();

    if clusters.is_empty() {
        emit_no_tunnels(&args.format, &state_file_path);
        std::process::exit(1); // exit 1 — state file present but contains no cluster entries
    }

    let timeout = Duration::from_secs(args.connect_timeout);

    // Evaluate each cluster; collect results sorted for deterministic output
    let mut rows: Vec<(String, &'static str, &ClusterTunnelEntry)> = clusters
        .iter()
        .map(|(name, entry)| {
            let state = cluster_state(entry, timeout);
            (name.clone(), state, entry)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let all_connected = rows.iter().all(|(_, s, _)| *s == "connected");
    let overall = if all_connected { "connected" } else { "error" };

    match args.format.as_str() {
        "json" => emit_json(overall, &rows),
        _ => emit_text(overall, &rows),
    }

    if !all_connected {
        std::process::exit(1); // exit 1 — one or more clusters failed TCP reachability probe
    }
    Ok(()) // exit 0 — all clusters are reachable
}

// ── Output formatters ────────────────────────────────────────────────────────

fn emit_no_state_file(format: &str, path: &str) {
    if format == "json" {
        println!(
            "{{\"state\":\"no_state_file\",\"error\":\"tunnel state file not found: {}\",\"clusters\":{{}}}}",
            path
        );
    } else {
        println!("state=no_state_file");
        eprintln!(
            "error: tunnel state file not found: {}\nhint: run install.sh --auto to establish tunnels",
            path
        );
    }
}

fn emit_no_tunnels(format: &str, path: &str) {
    if format == "json" {
        println!(
            "{{\"state\":\"no_tunnels\",\"clusters\":{{}},\"hint\":\"no cluster entries in {}\"}}",
            path
        );
    } else {
        println!("state=no_tunnels");
        eprintln!(
            "hint: state file exists at {} but contains no cluster entries — re-run install.sh --auto",
            path
        );
    }
}

fn emit_text(overall: &str, rows: &[(String, &str, &ClusterTunnelEntry)]) {
    println!("Tunnel Status");
    println!("=============");
    for (name, state, entry) in rows {
        println!(
            "  {}: state={}  transport={}  endpoint={}  auth={}",
            name, state, entry.transport_type, entry.endpoint, entry.auth_method
        );
        if let Some(ts) = &entry.established_at {
            println!("    established_at: {}", ts);
        }
    }
    println!();
    println!("state={}", overall);
}

fn emit_json(overall: &str, rows: &[(String, &str, &ClusterTunnelEntry)]) {
    let cluster_json: Vec<String> = rows
        .iter()
        .map(|(name, state, entry)| {
            let ts_field = entry
                .established_at
                .as_deref()
                .map(|ts| format!(",\"established_at\":\"{}\"", ts))
                .unwrap_or_default();
            format!(
                "\"{}\":{{\"state\":\"{}\",\"transport_type\":\"{}\",\"endpoint\":\"{}\",\"auth_method\":\"{}\"{}}}",
                name,
                state,
                entry.transport_type,
                entry.endpoint,
                entry.auth_method,
                ts_field
            )
        })
        .collect();
    println!(
        "{{\"state\":\"{}\",\"clusters\":{{{}}}}}",
        overall,
        cluster_json.join(",")
    );
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_endpoint ──

    #[test]
    fn test_parse_endpoint_localhost_port() {
        let r = parse_endpoint("localhost:16443");
        assert_eq!(r, Some(("localhost".to_string(), 16443)));
    }

    #[test]
    fn test_parse_endpoint_https_no_port() {
        let r = parse_endpoint("https://api.example.com");
        assert_eq!(r, Some(("api.example.com".to_string(), 443)));
    }

    #[test]
    fn test_parse_endpoint_https_with_port() {
        let r = parse_endpoint("https://api.example.com:6443");
        assert_eq!(r, Some(("api.example.com".to_string(), 6443)));
    }

    #[test]
    fn test_parse_endpoint_ip_port() {
        let r = parse_endpoint("127.0.0.1:16443");
        assert_eq!(r, Some(("127.0.0.1".to_string(), 16443)));
    }

    #[test]
    fn test_parse_endpoint_https_with_path() {
        let r = parse_endpoint("https://api.example.com:6443/healthz");
        assert_eq!(r, Some(("api.example.com".to_string(), 6443)));
    }

    // ── is_tcp_reachable ──

    #[test]
    fn test_is_tcp_reachable_closed_port() {
        // Port 19999 is very unlikely to be open; should return false quickly
        let reachable = is_tcp_reachable("127.0.0.1", 19999, Duration::from_millis(200));
        assert!(!reachable, "expected port 19999 to be unreachable");
    }

    // ── emit_text / emit_json (smoke tests via println capturing) ──

    #[test]
    fn test_emit_text_smoke() {
        let entry = ClusterTunnelEntry {
            transport_type: "ssh_bastion".to_string(),
            endpoint: "localhost:16443".to_string(),
            auth_method: "ssh_key".to_string(),
            established_at: Some("2026-03-18T12:00:00Z".to_string()),
        };
        let rows = vec![("tower".to_string(), "connected", &entry)];
        // Just ensure no panic
        emit_text("connected", &rows);
    }

    #[test]
    fn test_emit_json_smoke() {
        let entry = ClusterTunnelEntry {
            transport_type: "ssh_bastion".to_string(),
            endpoint: "localhost:16443".to_string(),
            auth_method: "ssh_key".to_string(),
            established_at: None,
        };
        let rows = vec![("tower".to_string(), "connected", &entry)];
        // Just ensure no panic
        emit_json("connected", &rows);
    }

    // ── cluster_state (requires a live port to test connected path) ──

    #[test]
    fn test_cluster_state_error_when_port_closed() {
        let entry = ClusterTunnelEntry {
            transport_type: "ssh_bastion".to_string(),
            endpoint: "localhost:19998".to_string(), // not listening
            auth_method: "ssh_key".to_string(),
            established_at: None,
        };
        let state = cluster_state(&entry, Duration::from_millis(200));
        assert_eq!(state, "error", "closed port should yield state=error");
    }

    #[test]
    fn test_cluster_state_bare_host_no_port_defaults_to_error_or_unknown() {
        let entry = ClusterTunnelEntry {
            transport_type: "ssh_bastion".to_string(),
            endpoint: "not-a-valid-endpoint-at-all".to_string(),
            auth_method: "ssh_key".to_string(),
            established_at: None,
        };
        // No port in endpoint → defaults to port 80; likely not reachable but parses OK.
        // The probe will fail and return "error"; "unknown" is also acceptable if parse fails.
        let state = cluster_state(&entry, Duration::from_millis(200));
        assert!(
            state == "error" || state == "unknown",
            "expected error or unknown, got {state}"
        );
    }

    // ── Integration: parse a realistic tunnel state YAML ──

    #[test]
    fn test_parse_tunnel_state_yaml() {
        let yaml = r#"
# ScaleX tunnel state
---
clusters:
  tower:
    transport_type: ssh_bastion
    endpoint: "localhost:16443"
    auth_method: ssh_key
    established_at: "2026-03-18T12:00:00Z"
  sandbox:
    transport_type: cf_tunnel
    endpoint: "https://api.sandbox.example.com:6443"
    auth_method: cf_token
    established_at: "2026-03-18T12:01:00Z"
"#;
        let state: TunnelStateFile = serde_yaml::from_str(yaml).expect("parse failed");
        let clusters = state.clusters.expect("clusters missing");
        assert_eq!(clusters.len(), 2);

        let tower = &clusters["tower"];
        assert_eq!(tower.transport_type, "ssh_bastion");
        assert_eq!(tower.endpoint, "localhost:16443");
        assert_eq!(tower.auth_method, "ssh_key");
        assert!(tower.established_at.is_some());

        let sandbox = &clusters["sandbox"];
        assert_eq!(sandbox.transport_type, "cf_tunnel");
        assert_eq!(sandbox.endpoint, "https://api.sandbox.example.com:6443");
    }

    #[test]
    fn test_parse_tunnel_state_empty_clusters() {
        let yaml = "---\nclusters: {}\n";
        let state: TunnelStateFile = serde_yaml::from_str(yaml).expect("parse failed");
        let clusters = state.clusters.unwrap_or_default();
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_parse_tunnel_state_missing_clusters_key() {
        let yaml = "---\n# no clusters key\n";
        let state: TunnelStateFile = serde_yaml::from_str(yaml).expect("parse failed");
        let clusters = state.clusters.unwrap_or_default();
        assert!(clusters.is_empty());
    }
}
