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

    /// Use the user-level config and deploy into the user's skill directories.
    ///
    /// Reads `$XDG_CONFIG_HOME/pollen/pollen.yaml` (`~/.config/pollen/pollen.yaml`
    /// when unset, `%APPDATA%\pollen\pollen.yaml` on Windows); when it sets no
    /// `targets`, the skills go to `~/.claude/skills` and `~/.agents/skills`.
    #[arg(
        long,
        short = 'g',
        env = "POLLEN_GLOBAL",
        conflicts_with = "config",
        global = true
    )]
    pub global: bool,

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
    /// Warnings and errors only by default: the summary on stdout already
    /// lists every skill, so `info` adds a second line per skill. Pass `info`
    /// to follow an update as it runs.
    ///
    /// Syntax: <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives>
    #[arg(
        long = "log-filter",
        env = "POLLEN_LOG_FILTER",
        default_value = "warn",
        global = true
    )]
    pub log_filter: String,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// The config file to read: the user-level one with `--global`, else
    /// `--config`.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        if self.global {
            crate::config::global_config_path()
        } else {
            self.config.clone()
        }
    }

    /// Where skills land when neither the CLI nor the config names a target.
    #[must_use]
    pub fn default_targets(&self) -> &'static [&'static str] {
        if self.global {
            &crate::config::GLOBAL_DEFAULT_TARGETS
        } else {
            &crate::config::DEFAULT_TARGETS
        }
    }
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

    #[test]
    fn logs_only_warnings_and_errors_by_default() {
        // Read from the definition, so an exported POLLEN_LOG_FILTER cannot
        // change the outcome.
        let command = <Cli as clap::CommandFactory>::command();
        let log_filter = command
            .get_arguments()
            .find(|argument| argument.get_id() == "log_filter")
            .unwrap();

        assert_eq!(log_filter.get_default_values(), ["warn"]);
    }

    #[test]
    fn global_reads_the_user_level_config_and_targets() {
        let cli = Cli::parse_from(["pollen", "--global", "update"]);

        assert_eq!(cli.config_path(), crate::config::global_config_path());
        assert_eq!(cli.default_targets(), crate::config::GLOBAL_DEFAULT_TARGETS);
    }

    #[test]
    fn global_and_an_explicit_config_are_mutually_exclusive() {
        let result = Cli::try_parse_from(["pollen", "-g", "--config", "other.yaml", "update"]);

        assert!(result.is_err());
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
