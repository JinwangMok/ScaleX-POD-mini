use crate::core::config::BaremetalInitConfig;
use crate::core::validation;
use crate::models::cluster::K8sClustersConfig;
use crate::models::sdi::SdiSpec;
use clap::Args;
use std::path::{Path, PathBuf};

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

#[derive(Args)]
pub struct ValidateArgs {
    /// Directory containing config files (sdi-specs.yaml, k8s-clusters.yaml)
    #[arg(long, default_value = "config")]
    config_dir: PathBuf,

    /// Directory containing credential files (.baremetal-init.yaml, secrets.yaml)
    #[arg(long, default_value = "credentials")]
    credentials_dir: PathBuf,

    /// Project root for kubespray submodule check
    #[arg(long, default_value = ".")]
    project_root: PathBuf,
}

/// Result of a single validation check
pub(crate) struct CheckResult {
    name: String,
    errors: Vec<String>,
}

impl CheckResult {
    fn pass(name: &str) -> Self {
        Self {
            name: name.to_string(),
            errors: vec![],
        }
    }

    fn fail(name: &str, errors: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            errors,
        }
    }

    fn is_pass(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Format a check result line with color
pub(crate) fn format_check_line(result: &CheckResult) -> String {
    if result.is_pass() {
        format!("  {}{}✓{} {}", GREEN, BOLD, RESET, result.name)
    } else {
        let mut lines = vec![format!("  {}{}✗{} {}", RED, BOLD, RESET, result.name)];
        for err in &result.errors {
            lines.push(format!("      {}{}{}", RED, err, RESET));
        }
        lines.join("\n")
    }
}

/// Format the summary line
pub(crate) fn format_summary(passed: usize, total: usize) -> String {
    if passed == total {
        format!(
            "\n{}{}{}/{} checks passed{}",
            GREEN, BOLD, passed, total, RESET
        )
    } else {
        format!(
            "\n{}{}{}/{} checks passed{}",
            RED, BOLD, passed, total, RESET
        )
    }
}

/// Load sdi-specs.yaml from config_dir. Returns Err with user-friendly message on failure.
pub fn load_sdi_spec(config_dir: &Path) -> Result<SdiSpec, String> {
    let path = config_dir.join("sdi-specs.yaml");
    let raw = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "Config file not found: {}\n  Copy the example: cp {}.example {}",
            path.display(),
            path.display(),
            path.display()
        )
    })?;
    serde_yaml::from_str(&raw).map_err(|e| {
        format!(
            "Failed to parse {}: {}\n  Check indentation against the .example template.",
            path.display(),
            e
        )
    })
}

/// Load k8s-clusters.yaml from config_dir. Returns Err with user-friendly message on failure.
pub fn load_k8s_config(config_dir: &Path) -> Result<K8sClustersConfig, String> {
    let path = config_dir.join("k8s-clusters.yaml");
    let raw = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "Config file not found: {}\n  Copy the example: cp {}.example {}",
            path.display(),
            path.display(),
            path.display()
        )
    })?;
    serde_yaml::from_str(&raw).map_err(|e| {
        format!(
            "Failed to parse {}: {}\n  Check indentation against the .example template.",
            path.display(),
            e
        )
    })
}

/// Load .baremetal-init.yaml from credentials_dir. Returns Err with user-friendly message on failure.
pub fn load_baremetal(credentials_dir: &Path) -> Result<BaremetalInitConfig, String> {
    let path = credentials_dir.join(".baremetal-init.yaml");
    let raw = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "Credentials file not found: {}\n  Copy the example: cp {}.example {}",
            path.display(),
            path.display(),
            path.display()
        )
    })?;
    serde_yaml::from_str(&raw).map_err(|e| {
        format!(
            "Failed to parse {}: {}\n  Check indentation against the .example template.",
            path.display(),
            e
        )
    })
}

pub fn run(args: ValidateArgs) -> anyhow::Result<()> {
    println!("{}ScaleX Config Validation{}", BOLD, RESET);
    println!("=========================");

    // Load config files
    let sdi_spec = match load_sdi_spec(&args.config_dir) {
        Ok(s) => s,
        Err(e) => {
            println!("{}{}Error:{} {}", RED, BOLD, RESET, e);
            std::process::exit(1);
        }
    };

    let k8s_config = match load_k8s_config(&args.config_dir) {
        Ok(c) => c,
        Err(e) => {
            println!("{}{}Error:{} {}", RED, BOLD, RESET, e);
            std::process::exit(1);
        }
    };

    let baremetal = match load_baremetal(&args.credentials_dir) {
        Ok(b) => b,
        Err(e) => {
            println!("{}{}Error:{} {}", RED, BOLD, RESET, e);
            std::process::exit(1);
        }
    };

    let baremetal_node_names: Vec<String> = baremetal
        .target_nodes
        .iter()
        .map(|n| n.name.clone())
        .collect();

    // Run validations
    let checks: Vec<CheckResult> = vec![
        {
            let errors = validation::validate_sdi_spec(&sdi_spec);
            if errors.is_empty() {
                CheckResult::pass("SDI spec structural validation")
            } else {
                CheckResult::fail("SDI spec structural validation", errors)
            }
        },
        {
            let errors = validation::validate_unique_cluster_names(&k8s_config);
            if errors.is_empty() {
                CheckResult::pass("Unique cluster names")
            } else {
                CheckResult::fail("Unique cluster names", errors)
            }
        },
        {
            let errors = validation::validate_unique_cluster_ids(&k8s_config);
            if errors.is_empty() {
                CheckResult::pass("Unique Cilium cluster IDs")
            } else {
                CheckResult::fail("Unique Cilium cluster IDs", errors)
            }
        },
        {
            let errors = validation::validate_cluster_sdi_pool_mapping(&k8s_config, &sdi_spec);
            if errors.is_empty() {
                CheckResult::pass("Cluster → SDI pool mapping")
            } else {
                CheckResult::fail("Cluster → SDI pool mapping", errors)
            }
        },
        {
            let errors = validation::validate_cluster_network_overlap(&k8s_config);
            if errors.is_empty() {
                CheckResult::pass("No cluster network CIDR overlaps")
            } else {
                CheckResult::fail("No cluster network CIDR overlaps", errors)
            }
        },
        {
            let errors =
                validation::validate_sdi_hosts_exist(&sdi_spec, &baremetal_node_names);
            if errors.is_empty() {
                CheckResult::pass("SDI hosts exist in baremetal inventory")
            } else {
                CheckResult::fail("SDI hosts exist in baremetal inventory", errors)
            }
        },
        {
            let (errors, _warnings) =
                validation::validate_two_layer_consistency(&k8s_config, &sdi_spec);
            if errors.is_empty() {
                CheckResult::pass("SDI ↔ cluster two-layer consistency")
            } else {
                CheckResult::fail("SDI ↔ cluster two-layer consistency", errors)
            }
        },
    ];

    // Print results
    for check in &checks {
        println!("{}", format_check_line(check));
    }

    let passed = checks.iter().filter(|c| c.is_pass()).count();
    let total = checks.len();
    println!("{}", format_summary(passed, total));

    if passed < total {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_check_line_pass() {
        let result = CheckResult::pass("Unique cluster names");
        let line = format_check_line(&result);
        assert!(line.contains('✓'));
        assert!(line.contains("Unique cluster names"));
        assert!(line.contains(GREEN));
    }

    #[test]
    fn test_format_check_line_fail() {
        let result = CheckResult::fail(
            "SDI spec structural validation",
            vec!["Pool 'tower' has 0 VMs".to_string()],
        );
        let line = format_check_line(&result);
        assert!(line.contains('✗'));
        assert!(line.contains("SDI spec structural validation"));
        assert!(line.contains("Pool 'tower' has 0 VMs"));
        assert!(line.contains(RED));
    }

    #[test]
    fn test_format_summary_all_pass() {
        let s = format_summary(7, 7);
        assert!(s.contains("7/7"));
        assert!(s.contains(GREEN));
    }

    #[test]
    fn test_format_summary_some_fail() {
        let s = format_summary(5, 7);
        assert!(s.contains("5/7"));
        assert!(s.contains(RED));
    }

    #[test]
    fn test_check_result_is_pass() {
        assert!(CheckResult::pass("x").is_pass());
        assert!(!CheckResult::fail("x", vec!["err".to_string()]).is_pass());
    }
}
