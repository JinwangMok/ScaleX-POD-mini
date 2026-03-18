use clap::Args;

#[derive(Args)]
pub struct StatusArgs {
    /// Show detailed status for each layer
    #[arg(long, short)]
    verbose: bool,
}

/// Platform layer health status
#[derive(Debug, Clone, PartialEq)]
pub enum LayerHealth {
    /// Layer is fully operational
    Ready,
    /// Layer has partial data (e.g., some nodes scanned but not all)
    Partial(String),
    /// Layer is not initialized
    NotReady(String),
}

/// Status summary for a single platform layer
#[derive(Debug, Clone)]
pub struct LayerStatus {
    pub name: String,
    pub health: LayerHealth,
    pub details: Vec<String>,
}

/// Aggregate platform status across all layers
#[derive(Debug)]
pub struct PlatformStatus {
    pub layers: Vec<LayerStatus>,
}

// ── Pure functions for status computation ──

/// Compute facts layer status from node count
pub fn compute_facts_status(node_count: usize) -> LayerStatus {
    let health = if node_count == 0 {
        LayerHealth::NotReady("No hardware facts gathered. Run `scalex facts --all`.".to_string())
    } else {
        LayerHealth::Ready
    };
    LayerStatus {
        name: "Facts".to_string(),
        health,
        details: vec![format!("{} node(s) scanned", node_count)],
    }
}

/// Compute SDI layer status from pool/node counts
pub fn compute_sdi_status(pool_count: usize, node_count: usize) -> LayerStatus {
    let health = if pool_count == 0 {
        LayerHealth::NotReady("No SDI pools. Run `scalex sdi init <spec>`.".to_string())
    } else if node_count == 0 {
        LayerHealth::Partial("Pools defined but no nodes provisioned.".to_string())
    } else {
        LayerHealth::Ready
    };
    LayerStatus {
        name: "SDI".to_string(),
        health,
        details: vec![format!("{} pool(s), {} node(s)", pool_count, node_count)],
    }
}

/// Compute cluster layer status from cluster info
pub fn compute_cluster_status(clusters: &[(String, u32, bool)]) -> LayerStatus {
    // clusters: Vec<(name, node_count, has_kubeconfig)>
    if clusters.is_empty() {
        return LayerStatus {
            name: "Clusters".to_string(),
            health: LayerHealth::NotReady(
                "No clusters. Run `scalex cluster init <config>`.".to_string(),
            ),
            details: vec![],
        };
    }

    let mut details = Vec::new();
    let mut all_ready = true;

    for (name, nodes, has_kc) in clusters {
        let kc_str = if *has_kc {
            "kubeconfig OK"
        } else {
            "no kubeconfig"
        };
        details.push(format!("{}: {} node(s), {}", name, nodes, kc_str));
        if !has_kc || *nodes == 0 {
            all_ready = false;
        }
    }

    let health = if all_ready {
        LayerHealth::Ready
    } else {
        LayerHealth::Partial("Some clusters missing kubeconfig or nodes.".to_string())
    };

    LayerStatus {
        name: "Clusters".to_string(),
        health,
        details,
    }
}

/// Compute cluster layer status with node readiness detail.
/// clusters: Vec<(name, total_nodes, ready_nodes, has_kubeconfig)>
pub fn compute_cluster_status_with_readiness(
    clusters: &[(String, u32, u32, bool)],
) -> LayerStatus {
    if clusters.is_empty() {
        return LayerStatus {
            name: "Clusters".to_string(),
            health: LayerHealth::NotReady(
                "No clusters. Run `scalex cluster init <config>`.".to_string(),
            ),
            details: vec![],
        };
    }

    let mut details = Vec::new();
    let mut all_ready = true;

    for (name, total, ready, has_kc) in clusters {
        let kc_str = if *has_kc {
            "kubeconfig OK"
        } else {
            "no kubeconfig"
        };
        details.push(format!(
            "{}: {}/{} node(s) Ready, {}",
            name, ready, total, kc_str
        ));
        if !has_kc || *total == 0 || *ready != *total {
            all_ready = false;
        }
    }

    let health = if all_ready {
        LayerHealth::Ready
    } else {
        let total_nodes: u32 = clusters.iter().map(|(_, t, _, _)| t).sum();
        let ready_nodes: u32 = clusters.iter().map(|(_, _, r, _)| r).sum();
        if ready_nodes == 0 && total_nodes > 0 {
            LayerHealth::NotReady(format!(
                "0/{} nodes Ready across all clusters.",
                total_nodes
            ))
        } else {
            LayerHealth::Partial(format!(
                "{}/{} nodes Ready across all clusters.",
                ready_nodes, total_nodes
            ))
        }
    };

    LayerStatus {
        name: "Clusters".to_string(),
        health,
        details,
    }
}

/// Compute config files status from presence checks
pub fn compute_config_status(required_present: usize, required_total: usize) -> LayerStatus {
    let health = if required_present == required_total {
        LayerHealth::Ready
    } else if required_present > 0 {
        LayerHealth::Partial(format!(
            "{}/{} config files present.",
            required_present, required_total
        ))
    } else {
        LayerHealth::NotReady("No config files found. See credentials/*.example.".to_string())
    };
    LayerStatus {
        name: "Config".to_string(),
        health,
        details: vec![format!(
            "{}/{} required files",
            required_present, required_total
        )],
    }
}

/// Compute GitOps layer status
pub fn compute_gitops_status(bootstrap_exists: bool, generator_count: usize) -> LayerStatus {
    let health = if bootstrap_exists && generator_count >= 2 {
        LayerHealth::Ready
    } else if bootstrap_exists {
        LayerHealth::Partial("Bootstrap exists but generators incomplete.".to_string())
    } else {
        LayerHealth::NotReady("No bootstrap. Check gitops/bootstrap/spread.yaml.".to_string())
    };
    LayerStatus {
        name: "GitOps".to_string(),
        health,
        details: vec![
            format!(
                "bootstrap: {}",
                if bootstrap_exists { "OK" } else { "MISSING" }
            ),
            format!("{} generator(s)", generator_count),
        ],
    }
}

/// Format a single layer status as a display line
pub fn format_layer_line(status: &LayerStatus) -> String {
    let icon = match &status.health {
        LayerHealth::Ready => "[OK]",
        LayerHealth::Partial(_) => "[!!]",
        LayerHealth::NotReady(_) => "[--]",
    };
    let reason = match &status.health {
        LayerHealth::Ready => String::new(),
        LayerHealth::Partial(msg) | LayerHealth::NotReady(msg) => format!(" - {}", msg),
    };
    format!("{} {}{}", icon, status.name, reason)
}

/// Format full platform status report
pub fn format_platform_report(platform: &PlatformStatus, verbose: bool) -> String {
    let mut lines = Vec::new();
    lines.push("ScaleX Platform Status".to_string());
    lines.push("======================".to_string());

    for layer in &platform.layers {
        lines.push(format_layer_line(layer));
        if verbose {
            for detail in &layer.details {
                lines.push(format!("  {}", detail));
            }
        }
    }

    let ready_count = platform
        .layers
        .iter()
        .filter(|l| l.health == LayerHealth::Ready)
        .count();
    let total = platform.layers.len();
    lines.push(String::new());
    lines.push(format!("Overall: {}/{} layers ready", ready_count, total));

    lines.join("\n")
}

pub fn run(args: StatusArgs) -> anyhow::Result<()> {
    let facts_status = {
        let facts_dir = std::path::Path::new("_generated/facts");
        let count = if facts_dir.exists() {
            std::fs::read_dir(facts_dir)
                .map(|d| {
                    d.filter(|e| {
                        e.as_ref()
                            .is_ok_and(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                    })
                    .count()
                })
                .unwrap_or(0)
        } else {
            0
        };
        compute_facts_status(count)
    };

    let sdi_status = {
        let state_path = std::path::Path::new("_generated/sdi/sdi-state.json");
        if state_path.exists() {
            let content = std::fs::read_to_string(state_path).unwrap_or_default();
            let pools: Vec<crate::models::sdi::SdiPoolState> =
                serde_json::from_str(&content).unwrap_or_default();
            let node_count: usize = pools.iter().map(|p| p.nodes.len()).sum();
            compute_sdi_status(pools.len(), node_count)
        } else {
            compute_sdi_status(0, 0)
        }
    };

    let cluster_status = {
        let clusters_dir = std::path::Path::new("_generated/clusters");
        let mut cluster_info: Vec<(String, u32, bool)> = Vec::new();
        if clusters_dir.exists() {
            let mut entries: Vec<_> = std::fs::read_dir(clusters_dir)
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .collect()
                })
                .unwrap_or_default();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let name = entry.file_name().to_string_lossy().to_string();
                let inv_path = entry.path().join("inventory.ini");
                let node_count = if inv_path.exists() {
                    let content = std::fs::read_to_string(&inv_path).unwrap_or_default();
                    crate::commands::get::count_nodes_from_inventory(&content)
                } else {
                    0
                };
                let has_kc = entry.path().join("kubeconfig.yaml").exists();
                cluster_info.push((name, node_count, has_kc));
            }
        }
        compute_cluster_status(&cluster_info)
    };

    let config_status = {
        let required_files = [
            "credentials/.baremetal-init.yaml",
            "credentials/.env",
            "credentials/secrets.yaml",
            "config/sdi-specs.yaml",
            "config/k8s-clusters.yaml",
            "credentials/cloudflare-tunnel.json",
        ];
        let present = required_files
            .iter()
            .filter(|p| std::path::Path::new(p).exists())
            .count();
        compute_config_status(present, required_files.len())
    };

    let gitops_status = {
        let bootstrap_exists = std::path::Path::new("gitops/bootstrap/spread.yaml").exists();
        let gen_dir = std::path::Path::new("gitops/generators");
        let generator_count = if gen_dir.exists() {
            std::fs::read_dir(gen_dir)
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .count()
                })
                .unwrap_or(0)
        } else {
            0
        };
        compute_gitops_status(bootstrap_exists, generator_count)
    };

    let platform = PlatformStatus {
        layers: vec![
            facts_status,
            sdi_status,
            cluster_status,
            config_status,
            gitops_status,
        ],
    };

    let report = format_platform_report(&platform, args.verbose);
    println!("{}", report);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Facts layer ──

    #[test]
    fn test_facts_status_no_nodes() {
        let status = compute_facts_status(0);
        assert_eq!(status.name, "Facts");
        assert!(matches!(status.health, LayerHealth::NotReady(_)));
        assert!(status.details[0].contains("0 node(s)"));
    }

    #[test]
    fn test_facts_status_with_nodes() {
        let status = compute_facts_status(4);
        assert_eq!(status.health, LayerHealth::Ready);
        assert!(status.details[0].contains("4 node(s)"));
    }

    // ── SDI layer ──

    #[test]
    fn test_sdi_status_no_pools() {
        let status = compute_sdi_status(0, 0);
        assert!(matches!(status.health, LayerHealth::NotReady(_)));
    }

    #[test]
    fn test_sdi_status_pools_no_nodes() {
        let status = compute_sdi_status(2, 0);
        assert!(matches!(status.health, LayerHealth::Partial(_)));
    }

    #[test]
    fn test_sdi_status_ready() {
        let status = compute_sdi_status(2, 5);
        assert_eq!(status.health, LayerHealth::Ready);
        assert!(status.details[0].contains("2 pool(s), 5 node(s)"));
    }

    // ── Cluster layer ──

    #[test]
    fn test_cluster_status_empty() {
        let status = compute_cluster_status(&[]);
        assert!(matches!(status.health, LayerHealth::NotReady(_)));
        assert!(status.details.is_empty());
    }

    #[test]
    fn test_cluster_status_all_ready() {
        let clusters = vec![
            ("tower".to_string(), 3, true),
            ("sandbox".to_string(), 4, true),
        ];
        let status = compute_cluster_status(&clusters);
        assert_eq!(status.health, LayerHealth::Ready);
        assert_eq!(status.details.len(), 2);
        assert!(status.details[0].contains("tower"));
        assert!(status.details[0].contains("kubeconfig OK"));
    }

    #[test]
    fn test_cluster_status_partial_no_kubeconfig() {
        let clusters = vec![
            ("tower".to_string(), 3, true),
            ("sandbox".to_string(), 4, false),
        ];
        let status = compute_cluster_status(&clusters);
        assert!(matches!(status.health, LayerHealth::Partial(_)));
        assert!(status.details[1].contains("no kubeconfig"));
    }

    #[test]
    fn test_cluster_status_partial_zero_nodes() {
        let clusters = vec![("tower".to_string(), 0, true)];
        let status = compute_cluster_status(&clusters);
        assert!(matches!(status.health, LayerHealth::Partial(_)));
    }

    // ── Cluster layer with readiness ──

    #[test]
    fn test_cluster_status_with_readiness_all_ready() {
        let clusters = vec![
            ("tower".to_string(), 3, 3, true),
            ("sandbox".to_string(), 3, 3, true),
        ];
        let status = compute_cluster_status_with_readiness(&clusters);
        assert_eq!(status.health, LayerHealth::Ready);
        assert!(status.details[0].contains("3/3"));
        assert!(status.details[0].contains("tower"));
    }

    #[test]
    fn test_cluster_status_with_readiness_partial() {
        let clusters = vec![
            ("tower".to_string(), 3, 3, true),
            ("sandbox".to_string(), 3, 1, true),
        ];
        let status = compute_cluster_status_with_readiness(&clusters);
        assert!(matches!(status.health, LayerHealth::Partial(_)));
        assert!(status.details[1].contains("1/3"));
    }

    #[test]
    fn test_cluster_status_with_readiness_none_ready() {
        let clusters = vec![("tower".to_string(), 3, 0, true)];
        let status = compute_cluster_status_with_readiness(&clusters);
        assert!(matches!(status.health, LayerHealth::NotReady(_)));
    }

    #[test]
    fn test_cluster_status_with_readiness_empty() {
        let status = compute_cluster_status_with_readiness(&[]);
        assert!(matches!(status.health, LayerHealth::NotReady(_)));
    }

    #[test]
    fn test_cluster_status_with_readiness_no_kubeconfig() {
        let clusters = vec![("tower".to_string(), 3, 3, false)];
        let status = compute_cluster_status_with_readiness(&clusters);
        assert!(matches!(status.health, LayerHealth::Partial(_)));
    }

    // ── Config layer ──

    #[test]
    fn test_config_status_all_present() {
        let status = compute_config_status(6, 6);
        assert_eq!(status.health, LayerHealth::Ready);
    }

    #[test]
    fn test_config_status_some_missing() {
        let status = compute_config_status(3, 6);
        assert!(matches!(status.health, LayerHealth::Partial(_)));
        assert!(status.details[0].contains("3/6"));
    }

    #[test]
    fn test_config_status_none_present() {
        let status = compute_config_status(0, 6);
        assert!(matches!(status.health, LayerHealth::NotReady(_)));
    }

    // ── GitOps layer ──

    #[test]
    fn test_gitops_status_ready() {
        let status = compute_gitops_status(true, 4);
        assert_eq!(status.health, LayerHealth::Ready);
        assert!(status.details[0].contains("OK"));
    }

    #[test]
    fn test_gitops_status_no_bootstrap() {
        let status = compute_gitops_status(false, 0);
        assert!(matches!(status.health, LayerHealth::NotReady(_)));
    }

    #[test]
    fn test_gitops_status_partial_generators() {
        let status = compute_gitops_status(true, 1);
        assert!(matches!(status.health, LayerHealth::Partial(_)));
    }

    // ── Formatting ──

    #[test]
    fn test_format_layer_line_ready() {
        let status = compute_facts_status(4);
        let line = format_layer_line(&status);
        assert!(line.starts_with("[OK]"));
        assert!(line.contains("Facts"));
    }

    #[test]
    fn test_format_layer_line_not_ready() {
        let status = compute_facts_status(0);
        let line = format_layer_line(&status);
        assert!(line.starts_with("[--]"));
        assert!(line.contains("Facts"));
        assert!(line.contains("No hardware facts"));
    }

    #[test]
    fn test_format_layer_line_partial() {
        let status = compute_sdi_status(2, 0);
        let line = format_layer_line(&status);
        assert!(line.starts_with("[!!]"));
    }

    #[test]
    fn test_format_platform_report_basic() {
        let platform = PlatformStatus {
            layers: vec![
                compute_facts_status(4),
                compute_sdi_status(2, 5),
                compute_cluster_status(&[
                    ("tower".to_string(), 3, true),
                    ("sandbox".to_string(), 4, true),
                ]),
                compute_config_status(6, 6),
                compute_gitops_status(true, 4),
            ],
        };
        let report = format_platform_report(&platform, false);
        assert!(report.contains("ScaleX Platform Status"));
        assert!(report.contains("[OK] Facts"));
        assert!(report.contains("[OK] SDI"));
        assert!(report.contains("[OK] Clusters"));
        assert!(report.contains("5/5 layers ready"));
    }

    #[test]
    fn test_format_platform_report_verbose() {
        let platform = PlatformStatus {
            layers: vec![compute_facts_status(4), compute_sdi_status(0, 0)],
        };
        let report = format_platform_report(&platform, true);
        assert!(report.contains("4 node(s) scanned"));
        assert!(report.contains("0 pool(s)"));
        assert!(report.contains("1/2 layers ready"));
    }

    #[test]
    fn test_format_platform_report_all_not_ready() {
        let platform = PlatformStatus {
            layers: vec![
                compute_facts_status(0),
                compute_sdi_status(0, 0),
                compute_cluster_status(&[]),
                compute_config_status(0, 6),
                compute_gitops_status(false, 0),
            ],
        };
        let report = format_platform_report(&platform, false);
        assert!(report.contains("0/5 layers ready"));
    }
}
