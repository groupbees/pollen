//! The command line surface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// What separates the directories of a list, as in `PATH`.
///
/// A `:` on Windows would split `C:\skills` into `C` and `\skills`.
const PATH_LIST_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

/// Deploy Agent Skills declared in `pollen.yaml`.
#[derive(Debug, Parser)]
#[command(name = "pollen", version, about, long_about = None)]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(
        long,
        short = 'c',
        env = "POLLEN_CONFIG_FILE",
        default_value = "pollen.yaml",
        global = true
    )]
    pub config: PathBuf,

    /// Directory to deploy into, overriding the config's `targets`.
    ///
    /// Repeat the flag for several directories. The environment variable
    /// takes a list separated like `PATH`: `;` on Windows, `:` elsewhere.
    #[arg(
        long = "target",
        env = "POLLEN_SKILLS_DIR",
        value_delimiter = PATH_LIST_SEPARATOR,
        global = true
    )]
    pub targets: Vec<PathBuf>,

    /// Directory holding the git checkouts pollen reuses between runs.
    #[arg(long, env = "POLLEN_CACHE_DIR", global = true)]
    pub cache_dir: Option<PathBuf>,

    /// Work from the cache only, without contacting any git remote.
    #[arg(long, env = "POLLEN_OFFLINE", global = true)]
    pub offline: bool,

    /// `tracing` filter directive (e.g. `info`, `pollen=debug`).
    ///
    /// Syntax: <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives>
    #[arg(
        long = "log-filter",
        env = "POLLEN_LOG_FILTER",
        default_value = "info",
        global = true
    )]
    pub log_filter: String,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands pollen exposes.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install, refresh and prune the declared skills.
    Update {
        /// Report what would change without touching the target directory.
        #[arg(long, env = "POLLEN_DRY_RUN")]
        dry_run: bool,

        /// Take ownership of a skill directory pollen did not install.
        #[arg(long, env = "POLLEN_FORCE")]
        force: bool,
    },

    /// Show the skills pollen manages in the target directory.
    List,

    /// Check the config and every skill it selects, without deploying.
    Validate {
        /// Config files to check, instead of the one `--config` names.
        #[arg(value_name = "CONFIG")]
        configs: Vec<PathBuf>,

        /// Check the config files alone, without fetching any source.
        ///
        /// This is the fast, network-free check a pre-commit hook wants: it
        /// proves the file parses and its rules hold, and says nothing about
        /// the skills the sources would yield.
        #[arg(long, env = "POLLEN_CONFIG_ONLY")]
        config_only: bool,
    },

    /// Print the JSON Schema for `pollen.yaml`.
    Schema,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(value: &str) -> Vec<PathBuf> {
        Cli::parse_from(["pollen", "--target", value, "list"]).targets
    }

    #[cfg(unix)]
    #[test]
    fn a_target_list_splits_on_colons() {
        assert_eq!(
            targets("/one/skills:~/two"),
            [PathBuf::from("/one/skills"), PathBuf::from("~/two")]
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_target_list_splits_on_semicolons_and_keeps_drive_letters() {
        assert_eq!(
            targets(r"C:\one\skills;D:\two"),
            [PathBuf::from(r"C:\one\skills"), PathBuf::from(r"D:\two")]
        );
    }
}
