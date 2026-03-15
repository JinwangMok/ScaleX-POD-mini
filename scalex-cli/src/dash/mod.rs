pub mod app;
pub mod data;
pub mod event;
pub mod kube_client;
pub mod theme;
pub mod ui;

use crate::commands::dash::DashArgs;

pub async fn run(args: DashArgs) -> anyhow::Result<()> {
    let kubeconfig_dir = args.kubeconfig_dir.clone().unwrap_or_else(|| {
        std::path::PathBuf::from("_generated/clusters")
    });

    let clusters = kube_client::discover_clusters(&kubeconfig_dir).await?;

    if clusters.is_empty() {
        anyhow::bail!(
            "No kubeconfig files found in {}. Run 'scalex cluster init' first.",
            kubeconfig_dir.display()
        );
    }

    if args.headless {
        headless::run_headless(&args, &clusters).await
    } else {
        app::run_tui(args, clusters).await
    }
}

pub mod headless {
    use super::*;
    use crate::dash::data;
    use crate::dash::kube_client::ClusterClient;

    pub async fn run_headless(
        args: &DashArgs,
        clusters: &[ClusterClient],
    ) -> anyhow::Result<()> {
        let target_clusters: Vec<&ClusterClient> = match &args.cluster {
            Some(name) => clusters
                .iter()
                .filter(|c| c.name == *name)
                .collect(),
            None => clusters.iter().collect(),
        };

        if target_clusters.is_empty() {
            let output = serde_json::json!({
                "error": format!("Cluster '{}' not found", args.cluster.as_deref().unwrap_or(""))
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            std::process::exit(1);
        }

        let mut cluster_data = Vec::new();
        for cluster in &target_clusters {
            let snapshot = data::fetch_cluster_snapshot(
                &cluster.client,
                &cluster.name,
                args.namespace.as_deref(),
            )
            .await?;
            cluster_data.push(snapshot);
        }

        // Filter by resource type if specified
        let output = if let Some(ref resource) = args.resource {
            data::filter_snapshot_by_resource(&cluster_data, resource)
        } else {
            serde_json::to_value(&cluster_data)?
        };

        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}
