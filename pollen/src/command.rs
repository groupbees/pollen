//! The subcommand implementations.

pub mod autoupdate;
pub mod list;
pub mod outdated;
pub mod plan;
pub mod schema;
pub mod show;
pub mod update;
pub mod validate;

use std::path::PathBuf;

use crate::cli::Options;
use crate::config::Config;
use crate::source::Fetcher;

/// Everything a subcommand needs, resolved from the CLI and the config file.
pub struct Context {
    /// The parsed configuration.
    pub config: Config,
    /// The directories skills are deployed into.
    pub targets: Vec<PathBuf>,
}

impl Context {
    /// Load the config and resolve the target directory.
    ///
    /// # Errors
    ///
    /// When the config file cannot be read, parsed or validated.
    pub fn open(options: &Options) -> anyhow::Result<Self> {
        Self::open_at(options, &options.config_path())
    }

    /// Load a named config file and resolve the target directories.
    ///
    /// # Errors
    ///
    /// When the config file cannot be read, parsed or validated.
    pub fn open_at(options: &Options, config_path: &std::path::Path) -> anyhow::Result<Self> {
        let config = Config::load(config_path)?;
        let targets = config.targets(&options.targets, options.default_targets());
        tracing::debug!(
            targets = ?targets,
            config = %config_path.display(),
            "resolved"
        );
        Ok(Self { config, targets })
    }

    /// The materializer the config's sources are fetched through.
    #[must_use]
    pub fn fetcher(&self, options: &Options) -> Fetcher {
        Fetcher::new(
            self.config.base_dir.clone(),
            options.cache_dir(),
            options.offline,
        )
    }
}
