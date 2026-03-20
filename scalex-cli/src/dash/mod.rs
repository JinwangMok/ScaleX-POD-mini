pub mod app;
#[allow(dead_code)]
pub mod command_mode;
pub mod degradation;
#[allow(dead_code)]
pub mod container_selector;
pub mod data;
#[allow(dead_code)]
pub mod dynamic_resource;
pub mod event;
pub mod filter;
#[allow(dead_code)]
pub mod help_overlay;
pub mod infra;
pub mod infra_view;
#[allow(dead_code)]
pub mod keybinding_registry;
pub mod kube_client;
pub mod log_viewer;
#[allow(dead_code, clippy::too_many_arguments, clippy::single_match)]
pub mod pod_exec;
#[allow(dead_code)]
pub mod port_detect;
pub mod port_forward;
#[allow(dead_code)]
pub mod port_picker;
#[allow(dead_code)]
pub mod resource_registry;
#[allow(dead_code)]
pub mod resource_watcher;
pub mod sa_provisioner;
#[allow(dead_code)]
pub mod theme;
#[allow(dead_code)]
pub mod toast;
#[allow(dead_code)]
pub mod tui_suspend;
pub mod ui;
pub mod yaml_modal;

use crate::commands::dash::DashArgs;

pub async fn run(args: DashArgs) -> anyhow::Result<()> {
    let kubeconfig_dir = args
        .kubeconfig_dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("_generated/clusters"));

    if args.headless || args.once {
        // Headless / once mode: synchronous discover (blocking is fine for JSON output)
        let clusters = kube_client::discover_clusters(&kubeconfig_dir).await?;
        if clusters.is_empty() {
            anyhow::bail!(
                "No kubeconfig files found in {}. Run 'scalex cluster init' first.",
                kubeconfig_dir.display()
            );
        }
        if args.once {
            once::run_once(&args, &clusters).await
        } else {
            headless::run_headless(&args, &clusters).await
        }
    } else {
        // TUI mode: non-blocking startup — pass dir, discover in background
        app::run_tui(args, kubeconfig_dir).await
    }
}

pub mod headless {
    use super::*;
    use crate::dash::data;
    use crate::dash::infra;
    use crate::dash::kube_client::ClusterClient;

    pub async fn run_headless(args: &DashArgs, clusters: &[ClusterClient]) -> anyhow::Result<()> {
        let target_clusters: Vec<&ClusterClient> = match &args.cluster {
            Some(name) => clusters.iter().filter(|c| c.name == *name).collect(),
            None => clusters.iter().collect(),
        };

        if target_clusters.is_empty() {
            let output = serde_json::json!({
                "error": format!("Cluster '{}' not found", args.cluster.as_deref().unwrap_or(""))
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            std::process::exit(1);
        }

        // Handle infra-only request
        if args.resource.as_deref() == Some("infra") {
            let sdi_dir = std::path::Path::new("_generated/sdi");
            let infra_snap = infra::load_sdi_state(sdi_dir);
            println!("{}", serde_json::to_string_pretty(&infra_snap)?);
            return Ok(());
        }

        // Handle checks mode — the 5 E2E health checks returning pass/fail JSON.
        // Expected clusters are derived from target_clusters (what we intend to check).
        let is_checks = args.resource.as_deref() == Some("checks")
            || (args.resource.is_none() && std::env::var("SCALEX_DASH_CHECKS").is_ok());

        // Fetch all clusters in parallel for minimum latency.
        // Tunnel and kubeconfig connections are fully initialized by discover_clusters()
        // before we reach this point — ordering is guaranteed by setup_auto_tunnel(),
        // which polls port readiness + probes the K8s API before returning.
        let mut handles: Vec<(String, _)> = Vec::new();
        for cluster in &target_clusters {
            let client = cluster.client.clone();
            let name = cluster.name.clone();
            let ns = args.namespace.clone();
            handles.push((
                name.clone(),
                tokio::spawn(async move {
                    data::fetch_cluster_snapshot(
                        &client,
                        &name,
                        ns.as_deref(),
                        None,
                        data::HEADLESS_API_TIMEOUT,
                    )
                    .await
                }),
            ));
        }
        let mut cluster_data = Vec::new();
        let mut fetch_errors: Vec<(String, String)> = Vec::new();
        for (cluster_name, handle) in handles {
            match handle.await {
                Ok(Ok(snapshot)) => cluster_data.push(snapshot),
                Ok(Err(e)) => fetch_errors.push((cluster_name, format!("{:#}", e))),
                Err(e) => fetch_errors.push((cluster_name, format!("task panic: {}", e))),
            }
        }

        // Surface fetch errors so callers can diagnose empty-data issues.
        // Previously errors were silently dropped, causing empty JSON output
        // with no indication of what went wrong.
        if !fetch_errors.is_empty() && cluster_data.is_empty() {
            let error_detail: Vec<serde_json::Value> = fetch_errors
                .iter()
                .map(|(n, e)| serde_json::json!({"cluster": n, "error": e}))
                .collect();
            let output = serde_json::json!({
                "error": "All cluster data fetches failed",
                "details": error_detail
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            std::process::exit(1);
        }
        // Partial failures: warn on stderr, include successful clusters in output
        if !fetch_errors.is_empty() {
            let warn: Vec<serde_json::Value> = fetch_errors
                .iter()
                .map(|(n, e)| serde_json::json!({"cluster": n, "error": e}))
                .collect();
            eprintln!(
                "warning: {}/{} cluster(s) failed to fetch data: {}",
                fetch_errors.len(),
                target_clusters.len(),
                serde_json::to_string(&warn).unwrap_or_default()
            );
        }

        // Checks mode: run 7 E2E health checks and return pass/fail JSON
        if is_checks {
            let expected: Vec<&str> = target_clusters.iter().map(|c| c.name.as_str()).collect();
            let degradations = degradation::load(
                &std::path::PathBuf::from("config/known_degradations.yaml"),
            );
            let report = data::run_e2e_checks(&cluster_data, &expected, &degradations);
            let output = serde_json::to_string_pretty(&report)?;
            println!("{}", output);
            // Exit with non-zero if any check failed
            if report.overall == data::CheckStatus::Fail {
                std::process::exit(1);
            }
            return Ok(());
        }

        // Filter by resource type if specified
        let output = if let Some(ref resource) = args.resource {
            data::filter_snapshot_by_resource(&cluster_data, resource)
        } else {
            // Include infrastructure data in full output
            let sdi_dir = std::path::Path::new("_generated/sdi");
            let infra_snap = infra::load_sdi_state(sdi_dir);
            serde_json::json!({
                "clusters": serde_json::to_value(&cluster_data)?,
                "infrastructure": serde_json::to_value(&infra_snap)?
            })
        };

        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}

/// `--once` mode: run all 7 E2E health checks, print a human-readable colored report,
/// then exit. Known-degraded items render with a distinct cyan `[KNWN]` marker so they
/// are never confused with actual failures (red `[FAIL]`) or clean passes (green `[PASS]`).
pub mod once {
    use super::*;
    use crate::dash::data::{self, CheckStatus};
    use crate::dash::kube_client::ClusterClient;

    // ANSI color codes used in the report
    const RESET: &str = "\x1b[0m";
    const BOLD: &str = "\x1b[1m";
    const GREEN: &str = "\x1b[32m";
    const RED: &str = "\x1b[31m";
    const YELLOW: &str = "\x1b[33m";
    const CYAN: &str = "\x1b[36m";
    const DIM: &str = "\x1b[2m";

    pub async fn run_once(args: &DashArgs, clusters: &[ClusterClient]) -> anyhow::Result<()> {
        let target_clusters: Vec<&ClusterClient> = match &args.cluster {
            Some(name) => clusters.iter().filter(|c| c.name == *name).collect(),
            None => clusters.iter().collect(),
        };

        if target_clusters.is_empty() {
            anyhow::bail!(
                "Cluster '{}' not found",
                args.cluster.as_deref().unwrap_or("")
            );
        }

        // Fetch all cluster snapshots in parallel
        let mut handles: Vec<(String, _)> = Vec::new();
        for cluster in &target_clusters {
            let client = cluster.client.clone();
            let name = cluster.name.clone();
            let ns = args.namespace.clone();
            handles.push((
                name.clone(),
                tokio::spawn(async move {
                    data::fetch_cluster_snapshot(
                        &client,
                        &name,
                        ns.as_deref(),
                        None,
                        data::HEADLESS_API_TIMEOUT,
                    )
                    .await
                }),
            ));
        }
        let mut cluster_data = Vec::new();
        for (_cluster_name, handle) in handles {
            if let Ok(Ok(snapshot)) = handle.await {
                cluster_data.push(snapshot);
            }
        }

        // Load known-degradation inventory
        let degradations =
            degradation::load(&std::path::PathBuf::from("config/known_degradations.yaml"));

        // Run checks with known-degradation suppression
        let expected: Vec<&str> = target_clusters.iter().map(|c| c.name.as_str()).collect();
        let report = data::run_e2e_checks(&cluster_data, &expected, &degradations);

        // Print header
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        println!(
            "\n{}{}ScaleX E2E Health Check Report — {}{}",
            BOLD, GREEN, now, RESET
        );
        let sep = "─".repeat(60);
        println!("{}{}{}", DIM, sep, RESET);

        // Print each check with distinct visual style
        for check in &report.checks {
            let (color, label, symbol) = match check.status {
                CheckStatus::Pass => (GREEN, "PASS", "✓"),
                CheckStatus::Fail => (RED, "FAIL", "✗"),
                CheckStatus::Warn => (YELLOW, "WARN", "△"),
                CheckStatus::Skip => (DIM, "SKIP", "─"),
                CheckStatus::KnownDegraded => (CYAN, "KNWN", "≈"),
            };
            println!(
                " {}{}{} [{}]{} {}{}{}",
                BOLD, color, symbol, label, RESET,
                color, check.name, RESET
            );
            println!(
                "        {}{}{}",
                DIM, check.message, RESET
            );
            // Per-cluster detail lines (indented)
            for detail in &check.details {
                let cluster_prefix = format!("  {}", detail.cluster);
                if detail.passed {
                    println!("        {}  {}{}{}", DIM, cluster_prefix, RESET, format!(" — {}", detail.message));
                } else {
                    println!(
                        "        {}{}  {}{}{}",
                        color, cluster_prefix, detail.message, RESET, ""
                    );
                }
            }
        }

        // Footer summary
        println!("{}{}{}", DIM, sep, RESET);
        let (overall_color, overall_label) = match report.overall {
            CheckStatus::Pass => (GREEN, "PASS"),
            CheckStatus::Fail => (RED, "FAIL"),
            CheckStatus::Warn => (YELLOW, "WARN"),
            CheckStatus::Skip => (DIM, "SKIP"),
            CheckStatus::KnownDegraded => (CYAN, "KNWN"),
        };
        println!(
            " {}{}Overall: [{}]{} — {} pass  {} fail  {} known-degraded  {} total",
            BOLD, overall_color, overall_label, RESET,
            report.passed,
            report.failed,
            report.known_degraded,
            report.total
        );
        let n_degradations = degradations.known_degradations.len();
        if n_degradations > 0 {
            println!(
                " {}Known-degradation inventory: {} entr{} loaded from config/known_degradations.yaml{}",
                DIM,
                n_degradations,
                if n_degradations == 1 { "y" } else { "ies" },
                RESET
            );
        }
        println!();

        // Exit 1 if any real (non-known-degraded) failures
        if report.failed > 0 {
            std::process::exit(1);
        }
        Ok(())
    }
}
