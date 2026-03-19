use clap::Args;

use crate::dash;

#[derive(Args)]
pub struct DashArgs {
    /// Run in headless mode (JSON output for AI agents)
    #[arg(long)]
    pub headless: bool,

    /// Run all E2E health checks once, print a human-readable colored report, then exit.
    /// Known-degraded items are rendered with a distinct [KNWN] marker (cyan) so they
    /// are never confused with actual failures ([FAIL] red) or passes ([PASS] green).
    /// Exits 0 unless there are real (non-known-degraded) failures.
    #[arg(long)]
    pub once: bool,

    /// Directory containing per-cluster kubeconfig files
    #[arg(long, env = "SCALEX_KUBECONFIG_DIR")]
    pub kubeconfig_dir: Option<std::path::PathBuf>,

    /// Filter to a specific cluster name
    #[arg(long)]
    pub cluster: Option<String>,

    /// Filter to a specific namespace
    #[arg(long)]
    pub namespace: Option<String>,

    /// Resource type filter for headless mode (pods, deployments, services, nodes, configmaps, infra, checks)
    #[arg(long)]
    pub resource: Option<String>,

    /// Data refresh interval in seconds
    #[arg(long, default_value = "1")]
    pub refresh: u64,
}

pub fn run(args: DashArgs) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(dash::run(args))
}
