use crate::core::secrets;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct SecretsArgs {
    #[command(subcommand)]
    command: SecretsCommand,
}

#[derive(Subcommand)]
enum SecretsCommand {
    /// Generate and apply K8s secrets for a cluster from credentials/secrets.yaml
    Apply {
        /// Path to secrets config file
        #[arg(long, default_value = "credentials/secrets.yaml")]
        config: String,

        /// Cluster role (management or workload)
        #[arg(long, default_value = "management")]
        role: String,

        /// Dry run — print manifests without applying
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Kubeconfig to use for kubectl apply
        #[arg(long)]
        kubeconfig: Option<String>,
    },
}

pub fn run(args: SecretsArgs) -> anyhow::Result<()> {
    match args.command {
        SecretsCommand::Apply {
            config,
            role,
            dry_run,
            kubeconfig,
        } => run_apply(config, role, dry_run, kubeconfig),
    }
}

/// Parse secrets config and resolve cloudflare credentials_file if it's a path.
/// This is the I/O boundary — reads the file and replaces the path with content
/// in the parsed config struct, then generates manifests directly.
fn load_and_resolve_secrets(
    yaml_content: &str,
    config_path: &str,
    cluster_role: &str,
) -> anyhow::Result<String> {
    let mut config = secrets::parse_secrets_config(yaml_content).map_err(|e| anyhow::anyhow!(e))?;

    // If credentials_file looks like a file path, read the file content
    if secrets::is_credentials_file_path(&config.cloudflare.credentials_file) {
        let creds_path = &config.cloudflare.credentials_file;
        // Resolve relative to the project root (parent of credentials/)
        let base_dir = std::path::Path::new(config_path)
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(std::path::Path::new("."));
        let resolved_path = if std::path::Path::new(creds_path).is_relative() {
            base_dir.join(creds_path)
        } else {
            std::path::PathBuf::from(creds_path)
        };

        let content = std::fs::read_to_string(&resolved_path)
            .map_err(|e| anyhow::anyhow!(
                "Failed to read cloudflare credentials file '{}': {}\n\
                 Hint: Create this file from the example: cp credentials/cloudflare-tunnel.json.example {}",
                resolved_path.display(), e, creds_path
            ))?;

        config.cloudflare.credentials_file = content.trim().to_string();
    }

    let specs = secrets::secrets_for_cluster(cluster_role, &config);
    if specs.is_empty() {
        return Ok(String::new());
    }
    let manifests: Vec<String> = specs
        .iter()
        .map(secrets::generate_k8s_secret_yaml)
        .collect();
    Ok(manifests.join("---\n"))
}

fn run_apply(
    config_path: String,
    cluster_role: String,
    dry_run: bool,
    kubeconfig: Option<String>,
) -> anyhow::Result<()> {
    println!("[secrets] Loading secrets from {}...", config_path);
    let yaml_content = std::fs::read_to_string(&config_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", config_path, e))?;

    // Parse, resolve cloudflare credentials file path → content, generate manifests
    let manifests = load_and_resolve_secrets(&yaml_content, &config_path, &cluster_role)?;

    if manifests.is_empty() {
        println!(
            "[secrets] No secrets needed for cluster role '{}'.",
            cluster_role
        );
        return Ok(());
    }

    if dry_run {
        println!("[secrets] Dry-run manifests for role '{}':\n", cluster_role);
        // Redact sensitive values in stringData to avoid leaking secrets to stdout/logs
        let redacted: String = manifests
            .lines()
            .map(|line| {
                let trimmed = line.trim_start();
                // Redact stringData values (indented key: "value" pairs under stringData)
                if !trimmed.starts_with("kind:")
                    && !trimmed.starts_with("name:")
                    && !trimmed.starts_with("namespace:")
                    && !trimmed.starts_with("stringData:")
                    && !trimmed.starts_with("apiVersion:")
                    && !trimmed.starts_with("metadata:")
                    && !trimmed.starts_with("type:")
                    && !trimmed.starts_with("---")
                    && trimmed.contains(": ")
                    && line.starts_with("    ")
                {
                    let key = line.split(": ").next().unwrap_or(line);
                    format!("{}: [REDACTED]", key)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        println!("{}", redacted);
        return Ok(());
    }

    // Apply via kubectl
    println!(
        "[secrets] Applying {} secret(s) for role '{}'...",
        manifests.matches("kind: Secret").count(),
        cluster_role
    );

    // Auto-detect kubeconfig from _generated/clusters/ if not explicitly provided
    let resolved_kubeconfig = kubeconfig.or_else(|| {
        let cluster_dir = match cluster_role.as_str() {
            "management" => "tower",
            "workload" => "sandbox",
            _ => return None,
        };
        let path = format!("_generated/clusters/{}/kubeconfig.yaml", cluster_dir);
        if std::path::Path::new(&path).exists() {
            println!("[secrets] Using kubeconfig: {}", path);
            Some(path)
        } else {
            None
        }
    });

    let mut cmd = std::process::Command::new("kubectl");
    cmd.args(["apply", "-f", "-"]);
    if let Some(ref kc) = resolved_kubeconfig {
        cmd.args(["--kubeconfig", kc]);
    }
    cmd.stdin(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to run kubectl: {}. Is kubectl installed?", e))?;

    if let Some(ref mut stdin) = child.stdin {
        use std::io::Write;
        stdin.write_all(manifests.as_bytes())?;
    }

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("kubectl apply failed with exit code: {:?}", status.code());
    }

    println!("[secrets] All secrets applied successfully.");
    Ok(())
}
