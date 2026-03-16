pub mod app;
pub mod data;
pub mod event;
pub mod infra;
pub mod kube_client;
#[allow(dead_code)]
pub mod theme;
pub mod ui;

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

        let mut cluster_data = Vec::new();
        for cluster in &target_clusters {
            let snapshot = data::fetch_cluster_snapshot(
                &cluster.client,
                &cluster.name,
                args.namespace.as_deref(),
                None, // headless: fetch all resources
            )
            .await?;
            cluster_data.push(snapshot);
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
