pub mod app;
#[allow(dead_code)]
pub mod command_mode;
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

    if args.headless {
        // Headless mode: synchronous discover (blocking is fine for JSON output)
        let clusters = kube_client::discover_clusters(&kubeconfig_dir).await?;
        if clusters.is_empty() {
            anyhow::bail!(
                "No kubeconfig files found in {}. Run 'scalex cluster init' first.",
                kubeconfig_dir.display()
            );
        }
        headless::run_headless(&args, &clusters).await
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

        // Fetch all clusters in parallel for minimum latency
        let mut handles = Vec::new();
        for cluster in &target_clusters {
            let client = cluster.client.clone();
            let name = cluster.name.clone();
            let ns = args.namespace.clone();
            handles.push(tokio::spawn(async move {
                data::fetch_cluster_snapshot(&client, &name, ns.as_deref(), None).await
            }));
        }
        let results = futures::future::join_all(handles).await;
        let mut cluster_data = Vec::new();
        for result in results {
            if let Ok(Ok(snapshot)) = result {
                cluster_data.push(snapshot);
            }
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
