use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::builder::PossibleValue;
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

use crate::config::{Client, ClientChoice};
use crate::{command, shutdown};

/// What separates the directories of a list, as in `PATH`.
///
/// A `:` on Windows would split `C:\skills` into `C` and `\skills`.
const PATH_LIST_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

pub const DEFAULT_CONFIG: &str = "pollen.yaml";

/// Deploy Agent Skills declared in `pollen.yaml`.
#[derive(Debug, Parser)]
#[command(name = "pollen", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub options: Options,

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

    /// Format of the logs on stderr.
    #[arg(
        long,
        env = "POLLEN_LOG_FORMAT",
        value_enum,
        default_value_t = LogFormat::Text,
        global = true
    )]
    pub log_format: LogFormat,

    #[command(subcommand)]
    pub command: Command,
}

/// Where the config, the targets and the git cache are, and whether to go
/// online: what every subcommand reads, from the CLI or from a library caller.
#[derive(Debug, Clone, clap::Args)]
pub struct Options {
    /// Path to the configuration file.
    #[arg(
        long,
        short = 'c',
        env = "POLLEN_CONFIG_FILE",
        default_value_os_t = PathBuf::from(DEFAULT_CONFIG),
        global = true
    )]
    pub config: PathBuf,

    /// Use the user-level config and deploy into the user's skill directories.
    ///
    /// Reads `$XDG_CONFIG_HOME/pollen/pollen.yaml` (`~/.config/pollen/pollen.yaml`
    /// when unset, `%APPDATA%\pollen\pollen.yaml` on Windows); when it sets no
    /// `targets`, its `clients` deploy under the home directory, into
    /// `~/.claude/skills` and `~/.agents/skills` by default.
    #[arg(
        long,
        short = 'g',
        env = "POLLEN_GLOBAL",
        conflicts_with = "config",
        global = true
    )]
    pub global: bool,

    /// Directory to deploy into, overriding the config's `targets` and `clients`.
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

    /// Client to deploy for, overriding the config's `targets` and `clients`.
    ///
    /// Repeat the flag for several clients. The environment variable takes a
    /// comma-separated list.
    #[arg(
        long = "client",
        env = "POLLEN_CLIENTS",
        value_enum,
        value_delimiter = ',',
        conflicts_with = "targets",
        global = true
    )]
    pub clients: Vec<ClientChoice>,

    /// Directory holding the git checkouts pollen reuses between runs.
    #[arg(long, env = "POLLEN_CACHE_DIR", global = true)]
    pub cache_dir: Option<PathBuf>,

    /// Work from the cache only, without contacting any git remote or
    /// `well-known` origin.
    #[arg(long, env = "POLLEN_OFFLINE", global = true)]
    pub offline: bool,

    /// File declaring how to obtain a token for each `well-known` origin.
    ///
    /// Defaults to `$XDG_CONFIG_HOME/pollen/credentials.yaml`
    /// (`~/.config/pollen/credentials.yaml` when unset,
    /// `%APPDATA%\pollen\credentials.yaml` on Windows), which may be absent.
    /// A file named here must exist.
    #[arg(long, env = "POLLEN_CREDENTIALS_FILE", global = true)]
    pub credentials_file: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            config: PathBuf::from(DEFAULT_CONFIG),
            global: false,
            targets: Vec::new(),
            clients: Vec::new(),
            cache_dir: None,
            offline: false,
            credentials_file: None,
        }
    }
}

impl ValueEnum for ClientChoice {
    fn value_variants<'a>() -> &'a [Self] {
        &Self::ALL
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        let help = match self {
            Self::Claude => "Claude Code, into `.claude/skills`",
            Self::AgentSkills => "Every other Agent Skills client, into `.agents/skills`",
            Self::All => "Every client",
        };
        Some(PossibleValue::new(self.name()).help(help))
    }
}

impl Options {
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

    /// The clients `--client` names, empty when it is not given.
    #[must_use]
    pub fn clients(&self) -> Vec<Client> {
        Client::expand(&self.clients)
    }

    /// The credentials file: `--credentials-file` with `~` expanded, which
    /// must exist, else the default one, which may not.
    #[must_use]
    pub fn credentials_file(&self) -> crate::source::well_known::CredentialsFile {
        match &self.credentials_file {
            Some(path) => crate::source::well_known::CredentialsFile {
                path: crate::config::expand_home(path),
                required: true,
            },
            None => crate::source::well_known::CredentialsFile {
                path: crate::config::default_credentials_path(),
                required: false,
            },
        }
    }

    /// The git cache: `--cache-dir` with `~` expanded, else the default one.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        self.cache_dir
            .as_deref()
            .map_or_else(crate::source::default_cache_dir, crate::config::expand_home)
    }
}

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

        /// Deploy skills whose files hold invisible characters, with a warning
        /// for each, instead of refusing them.
        ///
        /// Zero-width and bidirectional controls, tag characters and stray
        /// variation selectors are what hidden prompt injections are made of:
        /// pass this only for a source you trust.
        #[arg(long, env = "POLLEN_ALLOW_HIDDEN_CHARACTERS")]
        allow_hidden_characters: bool,
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

        /// Also require every git source to be pinned to a full commit SHA, and
        /// every `well-known` source to list each skill with its `digest`.
        ///
        /// A tag can be moved and a branch moves on every push; a commit
        /// cannot. Keep the tag readable in a comment:
        /// `revision: <sha>  # v1.2.3`. Works offline, with `--config-only`.
        #[arg(long, env = "POLLEN_PINNED")]
        pinned: bool,

        /// Accept skills whose files hold invisible characters, with a warning
        /// for each, instead of reporting them as invalid.
        #[arg(long, env = "POLLEN_ALLOW_HIDDEN_CHARACTERS")]
        allow_hidden_characters: bool,
    },

    /// Print the JSON Schema for `pollen.yaml`.
    Schema,

    /// Show which git sources have a newer release than the one pinned, and
    /// which pinned `well-known` skills a newer digest.
    Outdated {
        /// Leave out releases younger than this: whole days or hours, `7d`
        /// or `12h`. `0d` turns it off.
        ///
        /// A release's age is the date on its tag: the tagger date of an
        /// annotated tag, the committer date of a lightweight one. Each
        /// candidate tag is fetched one commit deep, into the cache, to read it.
        #[arg(
            long,
            env = "POLLEN_MIN_RELEASE_AGE",
            value_name = "AGE",
            value_parser = parse_age
        )]
        min_release_age: Option<Duration>,
    },

    /// Move every git source to its newest release, and every pinned
    /// `well-known` skill to its published digest, rewriting `pollen.yaml`.
    ///
    /// Only `revision:` and `digest:` lines change; comments and layout are
    /// kept, and the file is written only if it still validates.
    Autoupdate {
        /// Pin each source to the commit of its release, with the release in a
        /// trailing comment: `revision: <sha>  # v1.2.3`, and each skill listed
        /// under a `well-known` source to its published `digest`.
        #[arg(long, env = "POLLEN_FREEZE")]
        freeze: bool,

        /// Show what would change without writing the file.
        #[arg(long, env = "POLLEN_DRY_RUN")]
        dry_run: bool,

        /// Leave out releases younger than this: whole days or hours, `7d`
        /// or `12h`. `0d` turns it off.
        ///
        /// A release's age is the date on its tag: the tagger date of an
        /// annotated tag, the committer date of a lightweight one. Each
        /// candidate tag is fetched one commit deep, into the cache, to read it.
        #[arg(
            long,
            env = "POLLEN_MIN_RELEASE_AGE",
            value_name = "AGE",
            value_parser = parse_age
        )]
        min_release_age: Option<Duration>,
    },

    /// Work on `pollen.yaml` itself.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// List the skills a git source provides, before adding it to a config.
    ///
    /// Reads the newest release unless `--revision` names another, and prints
    /// each skill's path (the form a `repo` entry's `skills:` takes) and description, then the
    /// commit to pin. Writes nothing.
    Show {
        /// A git URL or a path to a git repository, as in `repo:`.
        repo: String,

        /// The tag, branch or commit to read instead of the newest release.
        #[arg(long, env = "POLLEN_REVISION")]
        revision: Option<String>,

        /// Only list the skills under this directory of the source.
        #[arg(long, env = "POLLEN_PATH")]
        path: Option<PathBuf>,

        /// Output format.
        #[arg(long, env = "POLLEN_FORMAT", value_enum, default_value_t = Format::Text)]
        format: Format,
    },
}

/// Read an age given as whole days or hours: `7d`, `36h`.
fn parse_age(text: &str) -> Result<Duration, String> {
    const HOUR: u64 = 60 * 60;

    [('d', 24 * HOUR), ('h', HOUR)]
        .into_iter()
        .find_map(|(suffix, unit)| {
            let count = text.strip_suffix(suffix)?;
            // `u64::from_str` also takes a leading `+`.
            if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            count.parse::<u64>().ok()?.checked_mul(unit)
        })
        .map(Duration::from_secs)
        .ok_or_else(|| {
            format!("`{text}` is not a whole number of days or hours, like `7d` or `12h`")
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    /// One human-readable line per event.
    Text,
    /// One JSON object per line, for a log pipeline.
    Json,
}

/// How `show` prints its listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// One aligned line per skill.
    Text,
    /// The full listing as JSON, for scripts and agents.
    Json,
}

/// Run the command line on `args`, program name first, and return the exit
/// status.
///
/// The `pollen` binary and the Python package's `pollen` command both start
/// here. A failure is printed to stderr as `Error: …`, as a `main` returning
/// it would.
pub fn main(args: impl IntoIterator<Item = impl Into<OsString> + Clone>) -> u8 {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        // `--help` and `--version` come through here too, with status 0.
        Err(error) => return usage_error(&error),
    };
    // `EnvFilter::new` skips a directive it cannot parse and carries on.
    let filter = match EnvFilter::try_new(&cli.log_filter) {
        Ok(filter) => filter,
        Err(error) => {
            return usage_error(&Cli::command().error(
                ErrorKind::ValueValidation,
                format!("invalid log filter `{}`: {error}", cli.log_filter),
            ));
        }
    };
    // Not `init`: it also bridges `log` into `tracing`, and fails in Python,
    // where pyo3-log already owns `log`. A subscriber left by an earlier call
    // in the same process is kept.
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    let _ = match cli.log_format {
        LogFormat::Text => tracing::subscriber::set_global_default(subscriber.finish()),
        LogFormat::Json => tracing::subscriber::set_global_default(subscriber.json().finish()),
    };

    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|runtime| {
            runtime.block_on(async {
                tokio::select! {
                    result = run(&cli) => result,
                    () = shutdown::signal() => {
                        tracing::warn!("interrupted; the target directory was left as it was found");
                        bail!("interrupted")
                    }
                }
            })
        });
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Error: {error:?}");
            1
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Rewrite `pollen.yaml` in the newest format version.
    ///
    /// Comments and layout are kept, and the file is written only if it
    /// declares the same sources and targets as before. A file already in the
    /// newest version is left alone.
    Migrate {
        /// Show what would change without writing the file.
        #[arg(long, env = "POLLEN_DRY_RUN")]
        dry_run: bool,
    },
}

/// Print what clap has to say and return its exit status.
fn usage_error(error: &clap::Error) -> u8 {
    let _ = error.print();
    u8::try_from(error.exit_code()).unwrap_or(2)
}

async fn run(cli: &Cli) -> Result<()> {
    let options = &cli.options;
    match &cli.command {
        Command::Update {
            dry_run,
            force,
            allow_hidden_characters,
        } => command::update::run(options, *dry_run, *force, *allow_hidden_characters).await,
        Command::List => command::list::run(options),
        Command::Validate {
            configs,
            config_only,
            pinned,
            allow_hidden_characters,
        } => {
            command::validate::run(
                options,
                configs,
                *config_only,
                *pinned,
                *allow_hidden_characters,
            )
            .await
        }
        Command::Schema => command::schema::run(),
        Command::Outdated { min_release_age } => {
            command::outdated::run(options, *min_release_age).await
        }
        Command::Autoupdate {
            freeze,
            dry_run,
            min_release_age,
        } => command::autoupdate::run(options, *freeze, *dry_run, *min_release_age).await,
        Command::Config {
            command: ConfigCommand::Migrate { dry_run },
        } => command::migrate::run(options, *dry_run),
        Command::Show {
            repo,
            revision,
            path,
            format,
        } => command::show::run(options, repo, revision.as_deref(), path.as_deref(), *format).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(value: &str) -> Vec<PathBuf> {
        Cli::parse_from(["pollen", "--target", value, "list"])
            .options
            .targets
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
    fn a_log_filter_directive_that_does_not_parse_is_a_usage_error() {
        let status = main(["pollen", "--log-filter", "pollen=loud", "schema"]);

        assert_eq!(status, 2);
    }

    #[test]
    fn logs_are_text_unless_json_is_asked_for() {
        // Read from the definition, so an exported POLLEN_LOG_FORMAT cannot
        // change the outcome.
        let command = <Cli as clap::CommandFactory>::command();
        let log_format = command
            .get_arguments()
            .find(|argument| argument.get_id() == "log_format")
            .unwrap();
        let json = Cli::parse_from(["pollen", "list", "--log-format", "json"]);

        assert_eq!(log_format.get_default_values(), ["text"]);
        assert_eq!(json.log_format, LogFormat::Json);
        assert!(Cli::try_parse_from(["pollen", "--log-format", "yaml", "list"]).is_err());
    }

    #[test]
    fn global_reads_the_user_level_config() {
        let options = Cli::parse_from(["pollen", "--global", "update"]).options;

        assert!(options.global);
        assert_eq!(options.config_path(), crate::config::global_config_path());
    }

    #[test]
    fn the_library_defaults_match_the_command_line_ones() {
        let command = <Cli as clap::CommandFactory>::command();
        let config = command
            .get_arguments()
            .find(|argument| argument.get_id() == "config")
            .unwrap();

        assert_eq!(
            config.get_default_values(),
            [Options::default().config.as_os_str()]
        );
    }

    #[test]
    fn global_and_an_explicit_config_are_mutually_exclusive() {
        let result = Cli::try_parse_from(["pollen", "-g", "--config", "other.yaml", "update"]);

        assert!(result.is_err());
    }

    #[test]
    fn a_minimum_release_age_reads_days_or_hours() {
        const HOUR: u64 = 60 * 60;

        assert_eq!(parse_age("7d"), Ok(Duration::from_secs(7 * 24 * HOUR)));
        assert_eq!(parse_age("36h"), Ok(Duration::from_secs(36 * HOUR)));
        assert_eq!(parse_age("0d"), Ok(Duration::ZERO));
        for text in ["7", "d", "", "-1d", "+1d", "1.5d", "7 d", "7D", "1w", "７d"] {
            assert!(parse_age(text).is_err(), "{text}");
        }
        assert!(parse_age(&format!("{}d", u64::MAX)).is_err());
    }

    #[test]
    fn outdated_and_autoupdate_take_a_minimum_release_age() {
        let week = Some(Duration::from_secs(7 * 24 * 60 * 60));

        let outdated = Cli::parse_from(["pollen", "outdated", "--min-release-age", "7d"]);
        let autoupdate = Cli::parse_from([
            "pollen",
            "autoupdate",
            "--freeze",
            "--min-release-age",
            "7d",
        ]);

        assert!(matches!(
            outdated.command,
            Command::Outdated { min_release_age } if min_release_age == week
        ));
        assert!(matches!(
            autoupdate.command,
            Command::Autoupdate { min_release_age, .. } if min_release_age == week
        ));
        assert!(Cli::try_parse_from(["pollen", "outdated", "--min-release-age", "7"]).is_err());
    }

    #[test]
    fn clients_are_repeated_or_comma_separated_and_all_expands() {
        let repeated = Cli::parse_from([
            "pollen",
            "--client",
            "agent-skills",
            "--client",
            "claude",
            "update",
        ]);
        let listed = Cli::parse_from(["pollen", "update", "--client", "agent-skills,claude"]);
        let all = Cli::parse_from(["pollen", "update", "--client", "all"]);

        assert_eq!(
            repeated.options.clients(),
            [Client::AgentSkills, Client::Claude]
        );
        assert_eq!(listed.options.clients(), repeated.options.clients());
        assert_eq!(all.options.clients(), Client::ALL);
    }

    #[test]
    fn migrate_is_a_config_subcommand() {
        let cli = Cli::parse_from(["pollen", "config", "migrate", "--dry-run", "-g"]);

        assert!(matches!(
            cli.command,
            Command::Config {
                command: ConfigCommand::Migrate { dry_run: true }
            }
        ));
        assert!(cli.options.global);
        assert!(Cli::try_parse_from(["pollen", "migrate"]).is_err());
    }

    #[test]
    fn rejects_an_unknown_client_and_a_client_with_a_target() {
        assert!(Cli::try_parse_from(["pollen", "update", "--client", "cursor"]).is_err());
        assert!(
            Cli::try_parse_from([
                "pollen", "update", "--client", "claude", "--target", "skills"
            ])
            .is_err()
        );
    }

    #[test]
    fn a_client_parses_from_its_config_name() {
        for choice in ClientChoice::ALL {
            assert_eq!(
                <ClientChoice as ValueEnum>::from_str(choice.name(), false),
                Ok(choice)
            );
        }
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
