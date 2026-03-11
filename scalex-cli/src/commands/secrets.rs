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

fn run_apply(
    config_path: String,
    cluster_role: String,
    dry_run: bool,
    kubeconfig: Option<String>,
) -> anyhow::Result<()> {
    println!("[secrets] Loading secrets from {}...", config_path);
    let yaml_content = std::fs::read_to_string(&config_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", config_path, e))?;

    let manifests = secrets::generate_all_secrets_manifests(&yaml_content, &cluster_role)
        .map_err(|e| anyhow::anyhow!(e))?;

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

    let mut cmd = std::process::Command::new("kubectl");
    cmd.args(["apply", "-f", "-"]);
    if let Some(ref kc) = kubeconfig {
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
