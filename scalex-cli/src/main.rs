mod commands;
mod core;
mod models;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "scalex", version, about = "Multi-cluster SDI platform CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Gather bare-metal hardware facts via SSH
    Facts(commands::facts::FactsArgs),
    /// Query resources
    Get(commands::get::GetArgs),
    /// Software-Defined Infrastructure operations
    Sdi(commands::sdi::SdiArgs),
    /// Kubernetes cluster operations
    Cluster(commands::cluster::ClusterArgs),
    /// Manage pre-bootstrap K8s secrets
    Secrets(commands::secrets::SecretsArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Facts(args) => commands::facts::run(args),
        Commands::Get(args) => commands::get::run(args),
        Commands::Sdi(args) => commands::sdi::run(args),
        Commands::Cluster(args) => commands::cluster::run(args),
        Commands::Secrets(args) => commands::secrets::run(args),
    }
}
