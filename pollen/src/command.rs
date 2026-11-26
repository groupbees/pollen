pub mod autoupdate;
pub mod list;
pub mod migrate;
pub mod outdated;
pub mod plan;
pub mod schema;
pub mod show;
pub mod update;
pub mod validate;
mod yaml_lines;

use std::path::{Path, PathBuf};

use crate::cli::Options;
use crate::config::{Client, Config, Scope, directory_key};
use crate::source::Fetcher;
use crate::state::State;

/// Everything a subcommand needs, resolved from the CLI and the config file.
pub struct Context {
    pub config: Config,
    /// How the entries' destinations resolve to directories in this run.
    pub scope: Scope,
}

impl Context {
    /// Load the config and resolve the scope of the run.
    ///
    /// # Errors
    ///
    /// When the config file cannot be read, parsed or validated.
    pub fn open(options: &Options) -> anyhow::Result<Self> {
        Self::open_at(options, &options.config_path())
    }

    /// Load a named config file and resolve the scope of the run.
    ///
    /// # Errors
    ///
    /// When the config file cannot be read, parsed or validated.
    pub fn open_at(options: &Options, config_path: &Path) -> anyhow::Result<Self> {
        let config = Config::load(config_path)?;
        let scope = Scope::new(&options.targets, &options.clients(), options.global);
        tracing::debug!(
            scope = ?scope,
            config = %config_path.display(),
            "resolved"
        );
        Ok(Self { config, scope })
    }

    /// Every directory a run visits: see [`directories`].
    #[must_use]
    pub fn directories(&self) -> Vec<PathBuf> {
        directories(&self.config, &self.scope)
    }

    #[must_use]
    pub fn fetcher(&self, options: &Options) -> Fetcher {
        Fetcher::new(
            self.config.base_dir.clone(),
            options.cache_dir(),
            options.offline,
            options.credentials_file(),
        )
    }
}

/// Every directory a run of `config` visits, in order: those its entries
/// deploy into, then, unless the command line overrides every destination,
/// each known client skill directory in `scope` whose state file names
/// `config`, so what the config stopped sending there is pruned.
#[must_use]
pub fn directories(config: &Config, scope: &Scope) -> Vec<PathBuf> {
    let mut dirs = config.all_directories(scope);
    if scope.overridden() || config.file.as_os_str().is_empty() {
        return dirs;
    }
    for dir in scope.client_dirs(&Client::ALL) {
        let key = directory_key(&dir);
        let named = dirs.iter().any(|named| directory_key(named) == key);
        if !named && owned_by(&dir, &config.file) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Whether the state file of `dir` names `config_file` as its owner.
fn owned_by(dir: &Path, config_file: &Path) -> bool {
    match State::load(dir) {
        Ok(state) => state.config.as_deref() == Some(config_file),
        Err(error) => {
            tracing::warn!(
                "{}: {error:#}; leaving it alone, as no entry of this config names it",
                dir.display()
            );
            false
        }
    }
}
