//! `pollen` — deploy Agent Skills declared in `pollen.yaml`.

use anyhow::{Result, bail};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use pollen::cli::{Cli, Command};
use pollen::{command, shutdown};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cli.log_filter))
        .with_writer(std::io::stderr)
        .init();

    tokio::select! {
        result = run(&cli) => result,
        () = shutdown::signal() => {
            tracing::warn!("interrupted; the target directory was left as it was found");
            bail!("interrupted")
        }
    }
}

async fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Update { dry_run, force } => command::update::run(cli, *dry_run, *force).await,
        Command::List => command::list::run(cli),
        Command::Validate {
            configs,
            config_only,
            pinned,
        } => command::validate::run(cli, configs, *config_only, *pinned).await,
        Command::Schema => command::schema::run(),
        Command::Outdated => command::outdated::run(cli).await,
        Command::Autoupdate { freeze, dry_run } => {
            command::autoupdate::run(cli, *freeze, *dry_run).await
        }
    }
}
