use anyhow::{Context, Result};
use kube::Client;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct ClusterClient {
    pub name: String,
    pub kubeconfig_path: PathBuf,
    pub client: Client,
}

/// Discover kubeconfig files from the given directory.
/// Expects structure: `{dir}/{cluster_name}/kubeconfig.yaml`
pub async fn discover_clusters(dir: &Path) -> Result<Vec<ClusterClient>> {
    let mut clusters = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(clusters),
        Err(e) => return Err(e).context(format!("Reading kubeconfig dir: {}", dir.display())),
    };

    for entry in entries {
        let entry = entry?;
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

        match build_client(&kubeconfig_path).await {
            Ok(client) => {
                clusters.push(ClusterClient {
                    name: cluster_name,
                    kubeconfig_path,
                    client,
                });
            }
            Err(e) => {
                eprintln!("Warning: Failed to load kubeconfig for {}: {}", cluster_name, e);
            }
        }
    }

    clusters.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(clusters)
}

async fn build_client(kubeconfig_path: &Path) -> Result<Client> {
    let kubeconfig = kube::config::Kubeconfig::read_from(kubeconfig_path)
        .context(format!("Reading {}", kubeconfig_path.display()))?;

    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default())
        .await
        .context("Building kube config")?;

    Client::try_from(config).context("Creating kube client")
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
}
