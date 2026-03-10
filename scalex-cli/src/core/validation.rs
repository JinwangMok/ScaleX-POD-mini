use crate::models::cluster::{ClusterMode, K8sClustersConfig};
use crate::models::sdi::SdiSpec;

/// Validate that every SDI-mode cluster references a pool that exists in the SDI spec.
/// Pure function: returns list of error messages (empty = valid).
#[allow(dead_code)]
pub fn validate_cluster_sdi_pool_mapping(
    k8s_config: &K8sClustersConfig,
    sdi_spec: &SdiSpec,
) -> Vec<String> {
    let pool_names: Vec<&str> = sdi_spec
        .spec
        .sdi_pools
        .iter()
        .map(|p| p.pool_name.as_str())
        .collect();

    let mut errors = Vec::new();

    for cluster in &k8s_config.config.clusters {
        if cluster.cluster_mode == ClusterMode::Baremetal {
            continue; // Baremetal clusters don't reference SDI pools
        }
        if cluster.cluster_sdi_resource_pool.is_empty() {
            errors.push(format!(
                "Cluster '{}' has mode=sdi but no cluster_sdi_resource_pool defined",
                cluster.cluster_name
            ));
            continue;
        }
        if !pool_names.contains(&cluster.cluster_sdi_resource_pool.as_str()) {
            errors.push(format!(
                "Cluster '{}' references SDI pool '{}' which does not exist in sdi-specs. Available pools: {:?}",
                cluster.cluster_name,
                cluster.cluster_sdi_resource_pool,
                pool_names,
            ));
        }
    }

    errors
}

/// Validate that Cilium cluster IDs are unique across all clusters.
/// Pure function.
#[allow(dead_code)]
pub fn validate_unique_cluster_ids(k8s_config: &K8sClustersConfig) -> Vec<String> {
    let mut seen = std::collections::HashMap::new();
    let mut errors = Vec::new();

    for cluster in &k8s_config.config.clusters {
        if let Some(ref cilium) = cluster.cilium {
            if let Some(prev) = seen.insert(cilium.cluster_id, &cluster.cluster_name) {
                errors.push(format!(
                    "Cilium cluster_id {} is used by both '{}' and '{}'",
                    cilium.cluster_id, prev, cluster.cluster_name
                ));
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::cluster::*;
    use crate::models::sdi::*;

    fn make_sdi_spec_with_pools(pool_names: &[&str]) -> SdiSpec {
        SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "test".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec!["8.8.8.8".to_string()],
                },
            },
            os_image: OsImageConfig {
                source: "https://example.com/img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "~/.ssh/id.pub".to_string(),
                packages: vec![],
            },
            spec: SdiPoolsSpec {
                sdi_pools: pool_names
                    .iter()
                    .map(|name| SdiPool {
                        pool_name: name.to_string(),
                        purpose: "test".to_string(),
                        placement: PlacementConfig::default(),
                        node_specs: vec![NodeSpec {
                            node_name: format!("{}-cp-0", name),
                            ip: "192.168.88.100".to_string(),
                            cpu: 2,
                            mem_gb: 3,
                            disk_gb: 30,
                            host: None,
                            roles: vec!["control-plane".to_string()],
                            devices: None,
                        }],
                    })
                    .collect(),
            },
        }
    }

    fn make_cluster(name: &str, pool: &str, mode: ClusterMode) -> ClusterDef {
        ClusterDef {
            cluster_name: name.to_string(),
            cluster_mode: mode,
            cluster_sdi_resource_pool: pool.to_string(),
            baremetal_nodes: vec![],
            cluster_role: "workload".to_string(),
            network: ClusterNetwork {
                pod_cidr: "10.244.0.0/20".to_string(),
                service_cidr: "10.96.0.0/20".to_string(),
                dns_domain: "test.local".to_string(),
                native_routing_cidr: None,
            },
            cilium: None,
            oidc: None,
            kubespray_extra_vars: None,
        }
    }

    #[test]
    fn test_validate_pool_mapping_valid() {
        let sdi = make_sdi_spec_with_pools(&["tower", "sandbox"]);
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: CommonConfig {
                    kubernetes_version: "1.33.1".to_string(),
                    kubespray_version: "v2.30.0".to_string(),
                    ..serde_yaml::from_str::<CommonConfig>(
                        "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                    )
                    .unwrap()
                },
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("sandbox", "sandbox", ClusterMode::Sdi),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_pool_mapping_missing_pool() {
        let sdi = make_sdi_spec_with_pools(&["tower"]);
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("sandbox", "nonexistent-pool", ClusterMode::Sdi),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("nonexistent-pool"));
        assert!(errors[0].contains("sandbox"));
    }

    #[test]
    fn test_validate_pool_mapping_baremetal_skip() {
        let sdi = make_sdi_spec_with_pools(&["tower"]);
        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![
                    make_cluster("tower", "tower", ClusterMode::Sdi),
                    make_cluster("prod", "", ClusterMode::Baremetal),
                ],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            errors.is_empty(),
            "baremetal clusters should not be validated against SDI pools"
        );
    }

    #[test]
    fn test_validate_unique_cluster_ids_valid() {
        let mut c1 = make_cluster("tower", "tower", ClusterMode::Sdi);
        c1.cilium = Some(CiliumConfig {
            cluster_id: 1,
            cluster_name: "tower".to_string(),
        });
        let mut c2 = make_cluster("sandbox", "sandbox", ClusterMode::Sdi);
        c2.cilium = Some(CiliumConfig {
            cluster_id: 2,
            cluster_name: "sandbox".to_string(),
        });

        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![c1, c2],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_unique_cluster_ids(&k8s);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_validate_unique_cluster_ids_duplicate() {
        let mut c1 = make_cluster("tower", "tower", ClusterMode::Sdi);
        c1.cilium = Some(CiliumConfig {
            cluster_id: 1,
            cluster_name: "tower".to_string(),
        });
        let mut c2 = make_cluster("sandbox", "sandbox", ClusterMode::Sdi);
        c2.cilium = Some(CiliumConfig {
            cluster_id: 1, // Duplicate!
            cluster_name: "sandbox".to_string(),
        });

        let k8s = K8sClustersConfig {
            config: K8sConfig {
                common: serde_yaml::from_str(
                    "kubernetes_version: '1.33.1'\nkubespray_version: 'v2.30.0'",
                )
                .unwrap(),
                clusters: vec![c1, c2],
                argocd: None,
                domains: None,
            },
        };

        let errors = validate_unique_cluster_ids(&k8s);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("cluster_id 1"));
    }

    /// Parse the ACTUAL .baremetal-init.yaml.example from disk.
    /// Catches drift between example file and parsing logic.
    #[test]
    fn test_parse_baremetal_init_example_from_disk() {
        let content = include_str!("../../../credentials/.baremetal-init.yaml.example");
        let config: crate::core::config::BaremetalInitConfig = serde_yaml::from_str(content)
            .expect("Failed to parse credentials/.baremetal-init.yaml.example");
        assert_eq!(config.target_nodes.len(), 4, "example must have 4 nodes");
        assert_eq!(config.target_nodes[0].name, "playbox-0");
        assert!(!config.target_nodes[0].direct_reachable);
        // Case 2: reachable via Tailscale IP
        assert_eq!(
            config.target_nodes[0].reachable_node_ip,
            Some("100.64.0.1".to_string())
        );
        // Case 3: reachable via ProxyJump
        assert_eq!(
            config.target_nodes[1].reachable_via,
            Some(vec!["playbox-0".to_string()])
        );
    }

    /// Parse the ACTUAL sdi-specs.yaml.example from disk.
    #[test]
    fn test_parse_sdi_specs_example_from_disk() {
        let content = include_str!("../../../config/sdi-specs.yaml.example");
        let spec: SdiSpec =
            serde_yaml::from_str(content).expect("Failed to parse config/sdi-specs.yaml.example");
        assert_eq!(spec.resource_pool.name, "playbox-pool");
        assert_eq!(spec.spec.sdi_pools.len(), 2);
        assert_eq!(spec.spec.sdi_pools[0].pool_name, "tower");
        assert_eq!(spec.spec.sdi_pools[1].pool_name, "sandbox");
        assert_eq!(spec.spec.sdi_pools[1].node_specs.len(), 4);
        // GPU passthrough on last sandbox worker
        assert!(
            spec.spec.sdi_pools[1].node_specs[3]
                .devices
                .as_ref()
                .unwrap()
                .gpu_passthrough
        );
    }

    /// Parse the ACTUAL k8s-clusters.yaml.example from disk.
    #[test]
    fn test_parse_k8s_clusters_example_from_disk() {
        let content = include_str!("../../../config/k8s-clusters.yaml.example");
        let config: K8sClustersConfig = serde_yaml::from_str(content)
            .expect("Failed to parse config/k8s-clusters.yaml.example");

        assert_eq!(config.config.common.kubernetes_version, "1.33.1");
        assert!(config.config.common.kube_proxy_remove);
        assert_eq!(config.config.clusters.len(), 2);

        // Tower
        assert_eq!(config.config.clusters[0].cluster_name, "tower");
        assert_eq!(config.config.clusters[0].cluster_sdi_resource_pool, "tower");
        assert!(config.config.clusters[0].oidc.is_none());

        // Sandbox with OIDC
        let sandbox = &config.config.clusters[1];
        assert_eq!(sandbox.cluster_name, "sandbox");
        let oidc = sandbox.oidc.as_ref().unwrap();
        assert!(oidc.enabled);
        assert_eq!(oidc.client_id, "kubernetes");

        // ArgoCD config
        let argocd = config.config.argocd.as_ref().unwrap();
        assert_eq!(argocd.tower_manages, vec!["sandbox"]);
    }

    /// Verify no k3s references in non-legacy project files.
    /// Checklist #9: k3s must be fully excluded from the project.
    /// Expanded scope: includes docs, drawio, ops-guide, test fixtures.
    #[test]
    fn test_no_k3s_references_in_project_files() {
        let files_to_check: Vec<(&str, &str)> = vec![
            (
                "tests/fixtures/values-full.yaml",
                include_str!("../../../tests/fixtures/values-full.yaml"),
            ),
            (
                "tests/fixtures/values-minimal.yaml",
                include_str!("../../../tests/fixtures/values-minimal.yaml"),
            ),
            (
                "docs/ops-guide.md",
                include_str!("../../../docs/ops-guide.md"),
            ),
            (
                "docs/SETUP-GUIDE.md",
                include_str!("../../../docs/SETUP-GUIDE.md"),
            ),
            (
                "docs/TROUBLESHOOTING.md",
                include_str!("../../../docs/TROUBLESHOOTING.md"),
            ),
        ];

        let mut violations = Vec::new();
        for (name, content) in &files_to_check {
            for (line_num, line) in content.lines().enumerate() {
                let lower = line.to_lowercase();
                if lower.contains("k3s") && !lower.contains("legacy") && !lower.contains("# k3s") {
                    violations.push(format!("{}:{}: {}", name, line_num + 1, line.trim()));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "k3s references found in non-legacy files (Checklist #9 violation):\n{}",
            violations.join("\n")
        );
    }

    /// Verify no ./playbox references in docs (should use scalex).
    /// Checklist #9, DEFECT-3.
    #[test]
    fn test_no_legacy_playbox_references_in_docs() {
        let files_to_check: Vec<(&str, &str)> = vec![
            (
                "docs/ops-guide.md",
                include_str!("../../../docs/ops-guide.md"),
            ),
            (
                "docs/SETUP-GUIDE.md",
                include_str!("../../../docs/SETUP-GUIDE.md"),
            ),
            (
                "docs/TROUBLESHOOTING.md",
                include_str!("../../../docs/TROUBLESHOOTING.md"),
            ),
            (
                "docs/CONTRIBUTING.md",
                include_str!("../../../docs/CONTRIBUTING.md"),
            ),
            (
                "docs/NETWORK-DISCOVERY.md",
                include_str!("../../../docs/NETWORK-DISCOVERY.md"),
            ),
        ];

        let mut violations = Vec::new();
        for (name, content) in &files_to_check {
            for (line_num, line) in content.lines().enumerate() {
                if line.contains("./playbox") || line.contains("values.yaml") {
                    violations.push(format!("{}:{}: {}", name, line_num + 1, line.trim()));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Legacy ./playbox or values.yaml references found in docs:\n{}",
            violations.join("\n")
        );
    }

    /// Verify no dead code in gitops: every directory under common/tower/sandbox
    /// must be referenced by at least one generator.
    /// DEFECT-2: common/cilium/ is dead code.
    #[test]
    fn test_no_gitops_dead_code_directories() {
        let gitops_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../gitops");

        // Collect all generator content
        let generator_files = [
            "generators/tower/common-generator.yaml",
            "generators/tower/tower-generator.yaml",
            "generators/sandbox/common-generator.yaml",
            "generators/sandbox/sandbox-generator.yaml",
        ];
        let mut all_generator_content = String::new();
        for gf in &generator_files {
            let path = gitops_root.join(gf);
            if path.exists() {
                all_generator_content.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }

        // Also include bootstrap/spread.yaml which references generators
        let spread = gitops_root.join("bootstrap/spread.yaml");
        if spread.exists() {
            all_generator_content.push_str(&std::fs::read_to_string(&spread).unwrap_or_default());
        }

        // Check each app directory under common/, tower/, sandbox/
        let mut dead_dirs = Vec::new();
        for category in &["common", "tower", "sandbox"] {
            let cat_dir = gitops_root.join(category);
            if !cat_dir.exists() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&cat_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let dir_name = entry.file_name().to_string_lossy().to_string();
                        // Check if this directory name appears in any generator
                        if !all_generator_content.contains(&dir_name) {
                            dead_dirs.push(format!("gitops/{}/{}", category, dir_name));
                        }
                    }
                }
            }
        }

        assert!(
            dead_dirs.is_empty(),
            "GitOps dead code directories (not referenced by any generator):\n{}",
            dead_dirs.join("\n")
        );
    }

    /// Verify all GitOps repoURLs are consistent across generators and bootstrap.
    /// DEFECT-4: All must reference the same repository.
    #[test]
    fn test_gitops_repo_url_consistency() {
        let files: Vec<(&str, &str)> = vec![
            (
                "bootstrap/spread.yaml",
                include_str!("../../../gitops/bootstrap/spread.yaml"),
            ),
            (
                "generators/tower/common-generator.yaml",
                include_str!("../../../gitops/generators/tower/common-generator.yaml"),
            ),
            (
                "generators/tower/tower-generator.yaml",
                include_str!("../../../gitops/generators/tower/tower-generator.yaml"),
            ),
            (
                "generators/sandbox/common-generator.yaml",
                include_str!("../../../gitops/generators/sandbox/common-generator.yaml"),
            ),
            (
                "generators/sandbox/sandbox-generator.yaml",
                include_str!("../../../gitops/generators/sandbox/sandbox-generator.yaml"),
            ),
        ];

        let mut urls: Vec<(String, String)> = Vec::new();
        for (name, content) in &files {
            for line in content.lines() {
                if let Some(url) = line.trim().strip_prefix("repoURL:") {
                    urls.push((name.to_string(), url.trim().to_string()));
                }
            }
        }

        assert!(!urls.is_empty(), "No repoURL found in any GitOps file");

        let first_url = &urls[0].1;
        let inconsistent: Vec<String> = urls
            .iter()
            .filter(|(_, url)| url != first_url)
            .map(|(name, url)| format!("{}: {} (expected {})", name, url, first_url))
            .collect();

        assert!(
            inconsistent.is_empty(),
            "Inconsistent repoURLs in GitOps files:\n{}",
            inconsistent.join("\n")
        );
    }

    /// Cross-validate: sdi-specs.yaml.example pool names must match k8s-clusters.yaml.example references.
    #[test]
    fn test_example_files_cross_config_consistency() {
        let sdi_content = include_str!("../../../config/sdi-specs.yaml.example");
        let k8s_content = include_str!("../../../config/k8s-clusters.yaml.example");

        let sdi: SdiSpec = serde_yaml::from_str(sdi_content).unwrap();
        let k8s: K8sClustersConfig = serde_yaml::from_str(k8s_content).unwrap();

        let errors = validate_cluster_sdi_pool_mapping(&k8s, &sdi);
        assert!(
            errors.is_empty(),
            "Example files have inconsistent pool mappings: {:?}",
            errors
        );

        let id_errors = validate_unique_cluster_ids(&k8s);
        assert!(
            id_errors.is_empty(),
            "Example files have duplicate cluster IDs: {:?}",
            id_errors
        );
    }

    /// Legacy top-level directories and files should be cleaned up or moved to .legacy- prefix.
    /// Integration: Parse example configs end-to-end and verify kubespray vars
    /// contain ALL DataX-required production settings.
    #[test]
    fn test_full_pipeline_datax_production_settings_coverage() {
        let k8s_yaml = include_str!("../../../config/k8s-clusters.yaml.example");
        let k8s_config: K8sClustersConfig =
            serde_yaml::from_str(k8s_yaml).expect("k8s-clusters.yaml.example must parse");

        let common = &k8s_config.config.common;

        // DataX-critical production settings must all be present in CommonConfig
        assert_eq!(
            common.etcd_deployment_type, "host",
            "etcd must use host deployment"
        );
        assert_eq!(common.dns_mode, "coredns", "DNS must use coredns");
        assert!(
            common.kube_proxy_remove,
            "kube-proxy must be removed for Cilium"
        );
        assert_eq!(common.cni, "cilium", "CNI must be cilium");
        assert_eq!(
            common.container_runtime, "containerd",
            "runtime must be containerd"
        );
        assert_eq!(common.cgroup_driver, "systemd", "cgroup must be systemd");
        assert!(common.helm_enabled, "helm must be enabled");
        assert!(common.gateway_api_enabled, "gateway API must be enabled");
        assert!(common.enable_nodelocaldns, "nodelocal DNS must be enabled");
        assert!(common.ntp_enabled, "NTP must be enabled");
        assert!(
            !common.kube_apiserver_admission_plugins.is_empty(),
            "admission plugins must be configured"
        );

        // Verify kubespray vars generation includes ALL these settings
        for cluster in &k8s_config.config.clusters {
            let vars = crate::core::kubespray::generate_cluster_vars(cluster, common);

            let datax_required_keys = [
                "kube_version",
                "container_manager",
                "kube_network_plugin",
                "kube_proxy_remove",
                "kubelet_cgroup_driver",
                "helm_enabled",
                "gateway_api_enabled",
                "enable_nodelocaldns",
                "ntp_enabled",
                "etcd_deployment_type",
                "dns_mode",
                "kube_pods_subnet",
                "kube_service_addresses",
                "cert_manager_enabled",
                "argocd_enabled",
            ];

            let parsed: serde_yaml::Mapping = serde_yaml::from_str(&vars).unwrap_or_else(|e| {
                panic!("cluster {} vars invalid YAML: {e}", cluster.cluster_name)
            });

            for key in &datax_required_keys {
                assert!(
                    parsed.contains_key(serde_yaml::Value::String(key.to_string())),
                    "cluster '{}' missing DataX-required key: {}",
                    cluster.cluster_name,
                    key
                );
            }
        }
    }

    /// Integration: Verify cluster-config ConfigMap exists per-cluster in gitops
    /// (NOT in common/) and has correct cluster.type values.
    #[test]
    fn test_cluster_config_per_cluster_not_common() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");

        // Must NOT exist in common/
        assert!(
            !repo_root.join("gitops/common/cluster-config").exists(),
            "cluster-config must NOT be in gitops/common/ (moved to per-cluster)"
        );

        // Must exist in tower/ and sandbox/
        assert!(
            repo_root
                .join("gitops/tower/cluster-config/manifest.yaml")
                .exists(),
            "cluster-config must exist in gitops/tower/"
        );
        assert!(
            repo_root
                .join("gitops/sandbox/cluster-config/manifest.yaml")
                .exists(),
            "cluster-config must exist in gitops/sandbox/"
        );

        // Tower must be "management", sandbox must be "workload"
        let tower_manifest =
            std::fs::read_to_string(repo_root.join("gitops/tower/cluster-config/manifest.yaml"))
                .unwrap();
        let sandbox_manifest =
            std::fs::read_to_string(repo_root.join("gitops/sandbox/cluster-config/manifest.yaml"))
                .unwrap();

        assert!(
            tower_manifest.contains("cluster.type: \"management\""),
            "tower cluster-config must have type 'management'"
        );
        assert!(
            sandbox_manifest.contains("cluster.type: \"workload\""),
            "sandbox cluster-config must have type 'workload'"
        );
    }

    #[test]
    fn test_no_legacy_toplevel_artifacts() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");

        let legacy_artifacts = ["gitops-apps", "gitops-manual", "values.yaml"];

        let mut found = Vec::new();
        for name in &legacy_artifacts {
            if repo_root.join(name).exists() {
                found.push(*name);
            }
        }

        assert!(
            found.is_empty(),
            "Legacy top-level artifacts still present (move dirs to .legacy- prefix, delete values.yaml): {:?}",
            found
        );
    }
}
