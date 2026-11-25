//! The declarative input: `pollen.yaml`, its versions, and the model the
//! commands run on.
//!
//! Parsing and execution are kept apart. [`raw`] holds one set of serde types
//! per format version, which own the schema and the validation of that
//! version; [`Config`] and [`Repo`] are what every command reads, whatever
//! version the file was written in.

pub mod raw;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use validator::Validate;

pub use raw::{LATEST, PathSpec, SkillPin};

/// The sentinel `repo:` value meaning "paths are relative to this config file".
pub const LOCAL_REPO: &str = "local";

/// An Agent Skills client pollen deploys for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    /// Claude Code.
    Claude,
    /// Every other Agent Skills client, through the cross-client convention
    /// they all scan.
    AgentSkills,
}

/// Where a client reads what pollen deploys, relative to the working
/// directory, or to the home directory with `--global`.
#[derive(Debug, PartialEq, Eq)]
pub struct ClientSpec {
    /// The directory holding the client's skills.
    pub skills_dir: &'static str,
}

/// A client named in the config or on the command line: one client, or all
/// of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ClientChoice {
    /// Claude Code, into `.claude/skills`.
    Claude,
    /// Every other Agent Skills client, into `.agents/skills`.
    AgentSkills,
    /// Every client pollen knows.
    All,
}

/// A parsed and validated `pollen.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The format version the file is written in.
    pub version: u32,
    /// Directories the skills are deployed into, when the file names them.
    pub targets: Option<Vec<PathBuf>>,
    /// The clients the skills are deployed for, when the file names them.
    pub clients: Option<Vec<Client>>,
    /// The sources to pull skills from, in the order the file lists them.
    pub repos: Vec<Repo>,
    /// Directory holding the config file; local paths resolve against it.
    pub base_dir: PathBuf,
    /// Absolute path of the config file, recorded as the owner of every
    /// target it deploys into. Empty for a config not read from a file.
    pub file: PathBuf,
}

/// One source of skills, and what to take from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repo {
    /// The directory holding `pollen.yaml`.
    Local {
        /// Where to look for skills.
        paths: Vec<PathSpec>,
    },
    /// A git repository, pinned to a revision.
    Git {
        /// The URL or path passed to `git`.
        url: String,
        /// The tag, branch or commit to check out.
        revision: String,
        /// Where to look for skills.
        paths: Vec<PathSpec>,
    },
    /// An origin publishing an Agent Skills discovery index.
    WellKnown {
        /// The origin, `scheme://host[:port]`, without a trailing slash.
        origin: String,
        /// Which skills of the index to take.
        skills: IndexSelection,
    },
}

/// Which skills a `well-known` source takes from its index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexSelection {
    /// Every skill, except the ones whose name `exclude` matches.
    All {
        /// Regular expression rejecting skills by name.
        exclude: Option<String>,
    },
    /// Only these, each optionally pinned to a digest.
    Only(Vec<SkillPin>),
}

impl Config {
    /// Read, parse and validate the config file at `path`, in any format
    /// version this pollen reads.
    ///
    /// # Errors
    ///
    /// When the file cannot be read, is not valid YAML, declares a version
    /// this pollen does not read, or breaks a rule.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read the config file {}", path.display()))?;
        let config = parse(&text, path)?;
        let base_dir = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        // Canonical, so `../pollen.yaml` from a subdirectory and a symlinked
        // path name the same owner as the plain path.
        let file = path
            .canonicalize()
            .with_context(|| format!("cannot resolve {}", path.display()))?;
        Ok(Self {
            base_dir,
            file,
            ..config
        })
    }

    /// The git sources whose `revision` is not a full commit id, and the
    /// `well-known` sources not listing every skill with its `digest`.
    #[must_use]
    pub fn unpinned(&self) -> Vec<&Repo> {
        self.repos
            .iter()
            .filter(|repo| match repo {
                Repo::Local { .. } => false,
                Repo::Git { revision, .. } => !is_commit_id(revision),
                Repo::WellKnown { skills, .. } => match skills {
                    IndexSelection::All { .. } => true,
                    IndexSelection::Only(pins) => pins.iter().any(|pin| pin.digest.is_none()),
                },
            })
            .collect()
    }

    /// The directories to deploy into: the CLI's `targets`, else the skill
    /// directories of the CLI's `clients`, else the config's `targets`, else
    /// the skill directories of its `clients`, every client by default. A
    /// client's directory is in the working directory, or in the home
    /// directory when `global`.
    ///
    /// Duplicates are dropped so a directory named twice is written once.
    #[must_use]
    pub fn targets(&self, targets: &[PathBuf], clients: &[Client], global: bool) -> Vec<PathBuf> {
        let dirs = |clients: &[Client]| {
            clients
                .iter()
                .map(|client| client.skills_dir(global))
                .collect()
        };
        let chosen = if !targets.is_empty() {
            targets.to_vec()
        } else if !clients.is_empty() {
            dirs(clients)
        } else if let Some(targets) = &self.targets {
            targets.clone()
        } else {
            dirs(self.clients.as_deref().unwrap_or(&Client::ALL))
        };

        let mut seen = BTreeSet::new();
        chosen
            .iter()
            .map(|target| expand_home(target))
            .filter(|target| seen.insert(target.clone()))
            .collect()
    }
}

impl Client {
    /// Every client pollen knows, in deployment order.
    pub const ALL: [Self; 2] = [Self::Claude, Self::AgentSkills];

    /// Where this client reads what pollen deploys.
    #[must_use]
    pub const fn spec(self) -> &'static ClientSpec {
        const CLAUDE: ClientSpec = ClientSpec {
            skills_dir: ".claude/skills",
        };
        const AGENT_SKILLS: ClientSpec = ClientSpec {
            skills_dir: ".agents/skills",
        };
        match self {
            Self::Claude => &CLAUDE,
            Self::AgentSkills => &AGENT_SKILLS,
        }
    }

    /// The clients `choices` name, each once, in the order first named.
    #[must_use]
    pub fn expand(choices: &[ClientChoice]) -> Vec<Self> {
        let mut clients = Vec::new();
        for client in choices.iter().flat_map(|choice| choice.clients()) {
            if !clients.contains(client) {
                clients.push(*client);
            }
        }
        clients
    }

    /// The skill directory, under the home directory when `global`.
    fn skills_dir(self, global: bool) -> PathBuf {
        let dir = self.spec().skills_dir;
        if global {
            PathBuf::from(format!("~/{dir}"))
        } else {
            PathBuf::from(dir)
        }
    }
}

impl ClientChoice {
    /// Every choice, in the order they are documented.
    pub const ALL: [Self; 3] = [Self::Claude, Self::AgentSkills, Self::All];

    /// The name the config and the command line give this choice.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::AgentSkills => "agent-skills",
            Self::All => "all",
        }
    }

    /// The clients this choice names.
    #[must_use]
    pub const fn clients(self) -> &'static [Client] {
        match self {
            Self::Claude => &[Client::Claude],
            Self::AgentSkills => &[Client::AgentSkills],
            Self::All => &Client::ALL,
        }
    }
}

impl std::str::FromStr for ClientChoice {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|choice| choice.name() == value)
            .ok_or_else(|| {
                format!("unknown client `{value}`: expected `claude`, `agent-skills` or `all`")
            })
    }
}

/// The format version `text` declares: 1 when it sets none.
///
/// # Errors
///
/// When `version` is not an integer this pollen reads.
pub fn version_of(text: &str) -> Result<u32> {
    // A file that does not even parse as a mapping is left to the full
    // parse, whose error points at the problem.
    let Some(value) = serde_norway::from_str::<raw::Probe>(text)
        .ok()
        .and_then(|probe| probe.version)
    else {
        return Ok(1);
    };
    let version = value
        .as_u64()
        .filter(|version| *version >= 1)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| anyhow!("`version` must be a whole number such as {LATEST}"))?;
    if version > LATEST {
        bail!(
            "the file is written in version {version} of the config format, and this pollen reads versions 1 to {LATEST}; upgrade pollen"
        );
    }
    Ok(version)
}

/// Parse and validate `text` in the version it declares, then convert it,
/// through each newer version, into the model. The model has no file yet.
fn parse(text: &str, path: &Path) -> Result<Config> {
    let version = version_of(text).with_context(|| format!("{} is invalid", path.display()))?;
    let latest = if version == 1 {
        let v1: raw::V1 = deserialize(text, version, path)?;
        check(&v1, path)?;
        raw::V2::from(v1)
    } else {
        let v2: raw::V2 = deserialize(text, version, path)?;
        check(&v2, path)?;
        v2
    };
    if latest.targets.is_some() && latest.clients.is_some() {
        bail!(
            "{} is invalid: set either `targets` (directories) or `clients` (their skill directories), not both",
            path.display()
        );
    }
    let targets = latest.targets.clone();
    let clients = latest
        .clients
        .as_ref()
        .map(|clients| Client::expand(clients.list()));
    Ok(Config {
        version,
        targets,
        clients,
        repos: latest.into_sources(),
        base_dir: PathBuf::new(),
        file: PathBuf::new(),
    })
}

fn deserialize<T: DeserializeOwned>(text: &str, version: u32, path: &Path) -> Result<T> {
    serde_norway::from_str(text)
        .map_err(|error| match version_hint(text, version) {
            Some(hint) => anyhow!("{error}; {hint}"),
            None => anyhow::Error::new(error),
        })
        .with_context(|| format!("{} is not a valid pollen config", path.display()))
}

/// What a file that does not parse most likely meant, when it uses a key of
/// one version in the other: version 2's `clients` or `discovery` in
/// version 1, a `repo`'s `paths` or a `well-known` entry in version 2, or a
/// `repo`'s `skills` in version 1. Read from the YAML itself, so it does not
/// depend on how serde words its error.
fn version_hint(text: &str, version: u32) -> Option<&'static str> {
    let document: serde_norway::Value = serde_norway::from_str(text).ok()?;
    if version == 1 && (document.get("clients").is_some() || document.get("discovery").is_some()) {
        return Some(
            "`clients` and `discovery` need `version: 2`: take them out, run `pollen config migrate`, then put them back",
        );
    }
    let repos = document.get("repos")?.as_sequence()?;
    let uses = |key: &str| {
        repos
            .iter()
            .any(|entry| entry.get("repo").is_some() && entry.get(key).is_some())
    };
    match version {
        1 if uses("skills") => {
            Some("a `repo` lists where to look under `paths`, or under `skills` with `version: 2`")
        }
        2 if repos.iter().any(|entry| entry.get("well-known").is_some()) => Some(
            "version 2 lists the origins serving a discovery index under `discovery`, each as `- url: <origin>`",
        ),
        2 if uses("paths") => Some("version 2 lists a `repo`'s paths under `skills`"),
        _ => None,
    }
}

fn check(raw: &impl Validate, path: &Path) -> Result<()> {
    // `validator` files a struct-level failure under the field name
    // `__all__`, which means nothing to whoever is reading the error.
    raw.validate().map_err(|errors| {
        anyhow!(
            "{} is invalid: {}",
            path.display(),
            errors.to_string().replace(".__all__", "")
        )
    })
}

impl Repo {
    /// The `repo:` value, or the origin of a `well-known` source: what the
    /// state file records as the skill's repo.
    #[must_use]
    pub fn location(&self) -> String {
        match self {
            Self::Local { .. } => LOCAL_REPO.to_owned(),
            Self::Git { url, .. } => url.clone(),
            Self::WellKnown { origin, .. } => origin.clone(),
        }
    }

    /// How this entry is named in output.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Git { url, revision, .. } => format!("{url}@{revision}"),
            _ => self.location(),
        }
    }

    /// The `revision:` of a git source.
    #[must_use]
    pub fn revision(&self) -> Option<&str> {
        match self {
            Self::Git { revision, .. } => Some(revision),
            _ => None,
        }
    }

    /// Where to look for skills inside a git or `local` source; empty for a
    /// `well-known` one, which selects them by name.
    #[must_use]
    pub fn paths(&self) -> &[PathSpec] {
        match self {
            Self::Local { paths } | Self::Git { paths, .. } => paths,
            Self::WellKnown { .. } => &[],
        }
    }
}

impl IndexSelection {
    /// The compiled `exclude` pattern, when the source takes every skill.
    ///
    /// # Errors
    ///
    /// When the pattern is not a valid regular expression, which validation
    /// already rules out for a config read by [`Config::load`].
    pub fn exclude_regex(&self) -> Result<Option<Regex>> {
        match self {
            Self::All { exclude } => compile(exclude.as_deref()),
            Self::Only(_) => Ok(None),
        }
    }
}

/// `text` as an origin, `scheme://host[:port]`, when it is an `http` or
/// `https` URL with nothing after the authority but an optional `/`.
#[must_use]
pub fn normalize_origin(text: &str) -> Option<String> {
    let url = reqwest::Url::parse(text).ok()?;
    let bare = matches!(url.scheme(), "http" | "https")
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    bare.then(|| url.origin().ascii_serialization())
}

impl PathSpec {
    /// The compiled `exclude` pattern.
    ///
    /// Validation already proved the expression compiles, so this only fails
    /// on a `PathSpec` built outside [`Config::load`].
    ///
    /// # Errors
    ///
    /// When the pattern is not a valid regular expression.
    pub fn exclude_regex(&self) -> Result<Option<Regex>> {
        compile(self.exclude.as_deref())
    }
}

fn compile(pattern: Option<&str>) -> Result<Option<Regex>> {
    pattern
        .map(|pattern| {
            Regex::new(pattern)
                .with_context(|| format!("`{pattern}` is not a valid regular expression"))
        })
        .transpose()
}

/// Whether `digest` is written as an index writes it: `sha256:` and 64
/// lowercase hex digits.
#[must_use]
pub fn is_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Whether `revision` is a full commit id — SHA-1 or SHA-256 — which no push
/// can move, unlike a branch or a tag.
#[must_use]
pub fn is_commit_id(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64) && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The user-level config `--global` reads: `pollen/pollen.yaml` in the
/// user's configuration directory.
#[must_use]
pub fn global_config_path() -> PathBuf {
    user_config_dir(
        std::env::var_os("XDG_CONFIG_HOME"),
        dirs::home_dir(),
        dirs::config_dir(),
    )
    .join("pollen")
    .join("pollen.yaml")
}

/// The user's configuration directory: an absolute `$XDG_CONFIG_HOME` wins
/// everywhere (a relative one is ignored, as the XDG spec says), then the
/// platform's own directory on Windows (`%APPDATA%`), else `~/.config` — on
/// macOS too, where CLI tools follow XDG rather than `~/Library`.
fn user_config_dir(
    xdg: Option<OsString>,
    home: Option<PathBuf>,
    platform: Option<PathBuf>,
) -> PathBuf {
    if let Some(xdg) = xdg.map(PathBuf::from).filter(|dir| dir.is_absolute()) {
        return xdg;
    }
    let fallback = if cfg!(windows) {
        platform
    } else {
        home.map(|home| home.join(".config"))
    };
    fallback.unwrap_or_else(|| PathBuf::from("."))
}

/// Replace a leading `~` with the user's home directory.
#[must_use]
pub fn expand_home(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    dirs::home_dir().map_or_else(|| path.to_path_buf(), |home| home.join(rest))
}

/// The user-level credentials file: `pollen/credentials.yaml` in the user's
/// configuration directory, beside the `--global` config.
#[must_use]
pub fn default_credentials_path() -> PathBuf {
    user_config_dir(
        std::env::var_os("XDG_CONFIG_HOME"),
        dirs::home_dir(),
        dirs::config_dir(),
    )
    .join("pollen")
    .join("credentials.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Result<Config> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pollen.yaml");
        std::fs::write(&path, yaml).unwrap();
        Config::load(&path)
    }

    #[test]
    fn parses_a_git_repo_with_paths() {
        let config = parse(
            "repos:\n  - repo: https://github.com/toto/tata\n    revision: 1.2.3\n    paths:\n      - path: mydir/\n        recurse: true\n        exclude: ^toto\n",
        )
        .unwrap();

        let Repo::Git {
            url,
            revision,
            paths,
        } = &config.repos[0]
        else {
            panic!("not a git source: {:?}", config.repos[0]);
        };
        assert_eq!(url, "https://github.com/toto/tata");
        assert_eq!(revision, "1.2.3");
        assert!(paths[0].recurse);
        assert!(
            paths[0]
                .exclude_regex()
                .unwrap()
                .unwrap()
                .is_match("toto-skill")
        );
    }

    #[test]
    fn defaults_recurse_to_false_and_exclude_to_none() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: skills\n").unwrap();

        assert!(!config.repos[0].paths()[0].recurse);
        assert!(config.repos[0].paths()[0].exclude.is_none());
    }

    #[test]
    fn rejects_a_git_repo_without_a_revision() {
        let error =
            parse("repos:\n  - repo: https://example.com/x.git\n    paths:\n      - path: .\n")
                .unwrap_err()
                .to_string();

        assert!(error.contains("invalid"), "{error}");
    }

    #[test]
    fn rejects_a_local_repo_carrying_a_revision() {
        assert!(
            parse("repos:\n  - repo: local\n    revision: 1.0.0\n    paths:\n      - path: .\n")
                .is_err()
        );
    }

    #[test]
    fn rejects_an_uncompilable_exclude() {
        assert!(
            parse("repos:\n  - repo: local\n    paths:\n      - path: .\n        exclude: \"[\"\n")
                .is_err()
        );
    }

    #[test]
    fn rejects_a_path_escaping_the_source_root() {
        assert!(
            parse("repos:\n  - repo: local\n    paths:\n      - path: ../elsewhere\n").is_err()
        );
        assert!(parse("repos:\n  - repo: local\n    paths:\n      - path: /etc\n").is_err());
    }

    #[test]
    fn rejects_a_windows_path_escaping_the_source_root_on_every_platform() {
        for path in [
            r"..\elsewhere",
            r"skills\..\..\elsewhere",
            r"\elsewhere",
            r"C:\elsewhere",
            "C:elsewhere",
            "c:/elsewhere",
        ] {
            let yaml = format!("repos:\n  - repo: local\n    paths:\n      - path: '{path}'\n");
            assert!(parse(&yaml).is_err(), "{path} was accepted");
        }
    }

    #[test]
    fn accepts_a_nested_path_in_either_separator() {
        for path in ["skills/nested", r"skills\nested", "./skills", "skills..old"] {
            let yaml = format!("repos:\n  - repo: local\n    paths:\n      - path: '{path}'\n");
            assert!(parse(&yaml).is_ok(), "{path} was refused");
        }
    }

    #[test]
    fn reads_a_config_with_windows_line_endings_and_a_byte_order_mark() {
        let config =
            parse("\u{feff}repos:\r\n  - repo: local\r\n    paths:\r\n      - path: skills\r\n")
                .unwrap();

        assert_eq!(config.repos[0].paths()[0].path, PathBuf::from("skills"));
    }

    #[test]
    fn rejects_an_empty_repo_list() {
        assert!(parse("repos: []\n").is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("repos:\n  - repo: local\n    pathz:\n      - path: .\n").is_err());
    }

    #[test]
    fn targets_default_to_claude_code_and_the_cross_client_convention() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap();

        assert_eq!(
            config.targets(&[], &[], false),
            [
                PathBuf::from(".claude/skills"),
                PathBuf::from(".agents/skills")
            ]
        );
    }

    #[test]
    fn targets_prefer_the_cli_overrides_then_the_config() {
        let config = parse(
            "targets:\n  - /opt/skills\nrepos:\n  - repo: local\n    paths:\n      - path: .\n",
        )
        .unwrap();

        assert_eq!(
            config.targets(&[PathBuf::from("/tmp/override")], &[], false),
            [PathBuf::from("/tmp/override")]
        );
        assert_eq!(
            config.targets(&[], &[], false),
            [PathBuf::from("/opt/skills")]
        );
    }

    #[test]
    fn global_targets_default_to_the_home_directory() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap();
        let home = dirs::home_dir().unwrap();

        assert_eq!(
            config.targets(&[], &[], true),
            [home.join(".claude/skills"), home.join(".agents/skills")]
        );
    }

    #[test]
    fn an_absolute_xdg_config_home_wins() {
        let xdg = std::env::temp_dir().join("xdg");

        assert_eq!(
            user_config_dir(
                Some(xdg.clone().into_os_string()),
                Some(PathBuf::from("/home/me")),
                None
            ),
            xdg
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_relative_xdg_config_home_is_ignored_for_dot_config() {
        assert_eq!(
            user_config_dir(
                Some(OsString::from("relative")),
                Some(PathBuf::from("/home/me")),
                Some(PathBuf::from("/home/me/Library/Application Support"))
            ),
            PathBuf::from("/home/me/.config")
        );
    }

    #[cfg(windows)]
    #[test]
    fn falls_back_to_appdata_on_windows() {
        let appdata = PathBuf::from(r"C:\Users\me\AppData\Roaming");

        assert_eq!(
            user_config_dir(
                None,
                Some(PathBuf::from(r"C:\Users\me")),
                Some(appdata.clone())
            ),
            appdata
        );
    }

    #[test]
    fn the_global_config_is_pollen_yaml_in_a_pollen_directory() {
        assert!(global_config_path().ends_with("pollen/pollen.yaml"));
    }

    #[test]
    fn records_the_canonical_path_of_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(
            dir.path().join("pollen.yaml"),
            "repos:\n  - repo: local\n    paths:\n      - path: .\n",
        )
        .unwrap();

        let config = Config::load(&dir.path().join("sub/../pollen.yaml")).unwrap();

        assert_eq!(
            config.file,
            dir.path().join("pollen.yaml").canonicalize().unwrap()
        );
    }

    #[test]
    fn a_commit_id_is_forty_or_sixty_four_hex_digits() {
        assert!(is_commit_id("acad0f52027cf8f8edf7bfa6a55e13c594d8ee71"));
        assert!(is_commit_id(&"ab".repeat(32)));
        assert!(!is_commit_id("v0.1.0"));
        assert!(!is_commit_id("main"));
        assert!(!is_commit_id("acad0f5"));
        assert!(!is_commit_id(&"g".repeat(40)));
    }

    #[test]
    fn lists_the_git_sources_not_pinned_to_a_commit() {
        let config = parse(
            "repos:\n  - repo: https://example.invalid/a.git\n    revision: acad0f52027cf8f8edf7bfa6a55e13c594d8ee71\n    paths:\n      - path: .\n  - repo: https://example.invalid/b.git\n    revision: v1.0.0\n    paths:\n      - path: .\n  - repo: local\n    paths:\n      - path: .\n",
        )
        .unwrap();

        let unpinned: Vec<String> = config.unpinned().iter().map(|repo| repo.label()).collect();

        assert_eq!(unpinned, ["https://example.invalid/b.git@v1.0.0"]);
    }

    const DIGEST: &str = "sha256:3f2a000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn parses_a_well_known_source_taking_every_skill_or_a_pinned_list() {
        let config = parse(&format!(
            "repos:\n  - well-known: https://Gateway.example.com/\n    exclude: ^experimental-\n  - well-known: https://gateway.example.com:8443\n    skills:\n      - name: pdf-summarizer\n        digest: {DIGEST}\n      - name: other\n"
        ))
        .unwrap();

        let Repo::WellKnown { origin, skills } = &config.repos[0] else {
            panic!("not a well-known source: {:?}", config.repos[0]);
        };
        assert_eq!(origin, "https://gateway.example.com");
        assert!(
            skills
                .exclude_regex()
                .unwrap()
                .unwrap()
                .is_match("experimental-x")
        );
        assert_eq!(config.repos[1].label(), "https://gateway.example.com:8443");
        let Repo::WellKnown {
            skills: IndexSelection::Only(skills),
            ..
        } = &config.repos[1]
        else {
            panic!("not a pinned list: {:?}", config.repos[1]);
        };
        assert_eq!(skills[0].digest.as_deref(), Some(DIGEST));
        assert_eq!(skills[1].digest, None);
    }

    #[test]
    fn rejects_a_well_known_source_that_is_not_an_origin() {
        for origin in [
            "https://x.example/skills",
            "https://x.example/?a=b",
            "https://user:pass@x.example",
            "ftp://x.example",
            "x.example",
        ] {
            let yaml = format!("repos:\n  - well-known: '{origin}'\n");
            assert!(parse(&yaml).is_err(), "{origin} was accepted");
        }
    }

    #[test]
    fn rejects_keys_that_do_not_belong_to_the_kind_of_source() {
        for yaml in [
            "repos:\n  - repo: local\n    well-known: https://x.example\n    paths:\n      - path: .\n",
            "repos:\n  - paths:\n      - path: .\n",
            "repos:\n  - well-known: https://x.example\n    revision: v1.0.0\n",
            "repos:\n  - well-known: https://x.example\n    paths:\n      - path: .\n",
            "repos:\n  - well-known: https://x.example\n    exclude: ^a\n    skills:\n      - name: b\n",
            "repos:\n  - well-known: https://x.example\n    skills: []\n",
            "repos:\n  - repo: local\n    exclude: ^a\n    paths:\n      - path: .\n",
            "repos:\n  - repo: local\n    skills:\n      - name: a\n    paths:\n      - path: .\n",
            "repos:\n  - repo: local\n",
        ] {
            assert!(parse(yaml).is_err(), "{yaml}");
        }
    }

    #[test]
    fn rejects_an_invalid_pin() {
        for skill in [
            "name: Not-Valid",
            "name: a\n        digest: sha256:ABC",
            "name: a\n        digest: md5:00",
            "name: a\n        surprise: true",
        ] {
            let yaml = format!(
                "repos:\n  - well-known: https://x.example\n    skills:\n      - {skill}\n"
            );
            assert!(parse(&yaml).is_err(), "{skill}");
        }
        let repeated = "repos:\n  - well-known: https://x.example\n    skills:\n      - name: a\n      - name: a\n";
        assert!(parse(repeated).unwrap_err().to_string().contains("twice"));
    }

    #[test]
    fn a_well_known_source_is_pinned_when_it_lists_every_skill_with_a_digest() {
        let config = parse(&format!(
            "repos:\n  - well-known: https://all.example\n  - well-known: https://partly.example\n    skills:\n      - name: a\n        digest: {DIGEST}\n      - name: b\n  - well-known: https://pinned.example\n    skills:\n      - name: a\n        digest: {DIGEST}\n"
        ))
        .unwrap();

        let unpinned: Vec<String> = config.unpinned().iter().map(|repo| repo.label()).collect();

        assert_eq!(unpinned, ["https://all.example", "https://partly.example"]);
    }

    #[test]
    fn targets_are_deduplicated() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap();
        let repeated = vec![PathBuf::from("skills"), PathBuf::from("skills")];

        assert_eq!(
            config.targets(&repeated, &[], false),
            [PathBuf::from("skills")]
        );
    }

    #[test]
    fn rejects_an_empty_target_list() {
        assert!(
            parse("targets: []\nrepos:\n  - repo: local\n    paths:\n      - path: .\n").is_err()
        );
    }

    #[test]
    fn expands_a_leading_tilde() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            expand_home(Path::new("~/.claude/skills")),
            home.join(".claude/skills")
        );
        assert_eq!(
            expand_home(Path::new("relative/path")),
            PathBuf::from("relative/path")
        );
    }

    #[cfg(windows)]
    #[test]
    fn expands_a_leading_tilde_with_a_backslash() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            expand_home(Path::new(r"~\.claude\skills")),
            home.join(".claude").join("skills")
        );
    }

    #[cfg(unix)]
    #[test]
    fn leaves_a_tilde_backslash_alone_where_backslash_is_a_file_name_character() {
        assert_eq!(
            expand_home(Path::new(r"~\skills")),
            PathBuf::from(r"~\skills")
        );
    }

    const V1_FILE: &str = "targets:\n  - skills-out\nrepos:\n  - repo: https://x.example/a.git\n    revision: v1.0.0\n    paths:\n      - path: skills\n        recurse: true\n        exclude: ^draft-\n  - repo: local\n    paths:\n      - path: mine\n  - well-known: https://x.example\n    skills:\n      - name: a\n";

    #[test]
    fn a_file_without_a_version_is_version_one() {
        assert_eq!(parse(V1_FILE).unwrap().version, 1);
        assert_eq!(parse(&format!("version: 1\n{V1_FILE}")).unwrap().version, 1);
    }

    #[test]
    fn version_two_lists_paths_under_skills_and_origins_under_discovery_and_reads_as_its_version_one_twin()
     {
        let v2 = "version: 2\ntargets:\n  - skills-out\nrepos:\n  - repo: https://x.example/a.git\n    revision: v1.0.0\n    skills:\n      - path: skills\n        recurse: true\n        exclude: ^draft-\n  - repo: local\n    skills:\n      - path: mine\ndiscovery:\n  - url: https://x.example\n    skills:\n      - name: a\n";

        let (v1, v2) = (parse(V1_FILE).unwrap(), parse(v2).unwrap());

        assert_eq!(v2.version, 2);
        assert_eq!(v2.repos, v1.repos);
        assert_eq!(v2.targets, v1.targets);
    }

    #[test]
    fn a_well_known_source_reads_the_same_in_both_versions() {
        for (v1, v2) in [
            (
                "repos:\n  - well-known: https://x.example\n    exclude: ^a\n",
                "version: 2\ndiscovery:\n  - url: https://x.example\n    exclude: ^a\n",
            ),
            (
                "repos:\n  - well-known: https://x.example\n",
                "version: 2\ndiscovery:\n  - url: https://x.example/\n",
            ),
        ] {
            assert_eq!(parse(v1).unwrap().repos, parse(v2).unwrap().repos, "{v2}");
        }
    }

    #[test]
    fn refuses_a_version_this_pollen_does_not_read() {
        let newer = parse("version: 3\nrepos: []\n").unwrap_err();
        assert!(format!("{newer:#}").contains("upgrade pollen"), "{newer:#}");
        for version in ["\"2\"", "0", "-1", "2.5", "two"] {
            let yaml = format!(
                "version: {version}\nrepos:\n  - repo: local\n    paths:\n      - path: .\n"
            );
            let error = parse(&yaml).unwrap_err();
            assert!(
                format!("{error:#}").contains("version"),
                "{version}: {error:#}"
            );
        }
    }

    #[test]
    fn points_each_version_at_the_key_the_other_one_uses() {
        let paths_in_v2 =
            parse("version: 2\nrepos:\n  - repo: local\n    paths:\n      - path: .\n")
                .unwrap_err();
        let skills_in_v1 =
            parse("repos:\n  - repo: local\n    skills:\n      - path: .\n").unwrap_err();

        assert!(
            format!("{paths_in_v2:#}").contains("under `skills`"),
            "{paths_in_v2:#}"
        );
        assert!(
            format!("{skills_in_v1:#}").contains("`version: 2`"),
            "{skills_in_v1:#}"
        );
        let well_known_in_v2 =
            parse("version: 2\nrepos:\n  - well-known: https://x.example\n").unwrap_err();
        let discovery_in_v1 = parse("discovery:\n  - url: https://x.example\n").unwrap_err();
        assert!(
            format!("{well_known_in_v2:#}").contains("under `discovery`"),
            "{well_known_in_v2:#}"
        );
        assert!(
            format!("{discovery_in_v1:#}").contains("`discovery` need `version: 2`"),
            "{discovery_in_v1:#}"
        );

        // A mistake unrelated to the version gets no hint.
        for yaml in [
            "version: 2\nrepos:\n  - repo: local\n    skills:\n      - path: .\n        recurze: true\n",
            "repos:\n  - well-known: https://x.example\n    skills:\n      - name: a\n        surprise: true\n",
        ] {
            let error = format!("{:#}", parse(yaml).unwrap_err());
            assert!(!error.contains("version"), "{yaml}: {error}");
        }
    }

    #[test]
    fn version_two_checks_each_kind_of_source_in_its_own_list() {
        for (yaml, expected) in [
            (
                "repos:\n  - repo: local\n    skills:\n      - name: a\n",
                "name",
            ),
            ("repos:\n  - repo: local\n", "skills"),
            (
                "repos:\n  - repo: local\n    skills: []\n",
                "at least one skill",
            ),
            (
                "repos:\n  - repo: local\n    exclude: ^a\n    skills:\n      - path: .\n",
                "exclude",
            ),
            (
                "repos:\n  - repo: https://x.example/a.git\n    skills:\n      - path: .\n",
                "revision",
            ),
            (
                "repos:\n  - repo: local\n    revision: v1\n    skills:\n      - path: .\n",
                "takes no revision",
            ),
            (
                "repos:\n  - repo: local\n    skills:\n      - path: ../out\n",
                "climb",
            ),
            (
                "repos:\n  - repo: local\n    skills:\n      - path: .\n        exclude: \"[\"\n",
                "regular expression",
            ),
            (
                "repos:\n  - repo: local\n    skills:\n      - path: .\n        surprise: true\n",
                "surprise",
            ),
            ("repos: []\n", "at least one repo"),
            (
                "discovery:\n  - url: https://x.example\n    skills:\n      - path: .\n",
                "path",
            ),
            (
                "discovery:\n  - url: https://x.example\n    revision: v1\n",
                "revision",
            ),
            (
                "discovery:\n  - url: https://x.example\n    exclude: ^a\n    skills:\n      - name: b\n",
                "does not combine",
            ),
            (
                "discovery:\n  - url: https://x.example\n    skills:\n      - name: a\n      - name: a\n",
                "twice",
            ),
            (
                "discovery:\n  - url: https://x.example/skills\n",
                "must be an origin",
            ),
            ("discovery: []\n", "at least one origin"),
            ("targets: [out]\n", "at least one source"),
        ] {
            let error = parse(&format!("version: 2\n{yaml}")).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{yaml}: {error:#}");
        }
    }

    #[test]
    fn version_two_reads_repos_then_discovery_origins() {
        let config = parse("version: 2\ndiscovery:\n  - url: https://g.example\nrepos:\n  - repo: local\n    skills:\n      - path: .\n").unwrap();
        let only_discovery = parse("version: 2\ndiscovery:\n  - url: https://g.example\n").unwrap();

        assert!(matches!(config.repos[0], Repo::Local { .. }), "{config:?}");
        assert!(
            matches!(&config.repos[1], Repo::WellKnown { origin, .. } if origin == "https://g.example"),
            "{config:?}"
        );
        assert_eq!(only_discovery.repos.len(), 1);
    }

    #[test]
    fn a_syntax_error_in_version_two_names_its_line() {
        let error = parse("version: 2\nrepos:\n  - repo: local\n    skills:\n      - path: .\n        recurze: true\n").unwrap_err();

        assert!(format!("{error:#}").contains("line 6"), "{error:#}");
    }

    const V2_LOCAL: &str = "repos:\n  - repo: local\n    skills:\n      - path: .\n";

    fn parse_v2(keys: &str) -> Result<Config> {
        parse(&format!("version: 2\n{keys}{V2_LOCAL}"))
    }

    #[test]
    fn clients_select_their_skill_directories() {
        for (clients, expected) in [
            ("all", vec![".claude/skills", ".agents/skills"]),
            ("claude", vec![".claude/skills"]),
            ("[agent-skills]", vec![".agents/skills"]),
            (
                "[agent-skills, claude, agent-skills]",
                vec![".agents/skills", ".claude/skills"],
            ),
            ("[claude, all]", vec![".claude/skills", ".agents/skills"]),
        ] {
            let config = parse_v2(&format!("clients: {clients}\n")).unwrap();

            assert_eq!(
                config.targets(&[], &[], false),
                expected.iter().map(PathBuf::from).collect::<Vec<_>>(),
                "{clients}"
            );
        }
    }

    #[test]
    fn global_clients_select_the_home_skill_directories() {
        let config = parse_v2("clients: claude\n").unwrap();
        let home = dirs::home_dir().unwrap();

        assert_eq!(
            config.targets(&[], &[], true),
            [home.join(".claude/skills")]
        );
    }

    #[test]
    fn cli_targets_win_over_cli_clients_and_the_config() {
        let config = parse_v2("clients: claude\n").unwrap();

        assert_eq!(
            config.targets(
                &[PathBuf::from("/tmp/override")],
                &[Client::AgentSkills],
                false
            ),
            [PathBuf::from("/tmp/override")]
        );
    }

    #[test]
    fn cli_clients_win_over_the_config() {
        for keys in ["targets: [skills]\n", "clients: agent-skills\n", ""] {
            let config = parse_v2(keys).unwrap();

            assert_eq!(
                config.targets(&[], &[Client::Claude], false),
                [PathBuf::from(".claude/skills")],
                "{keys}"
            );
        }
    }

    #[test]
    fn rejects_targets_with_clients() {
        let error = parse_v2("targets: [skills]\nclients: all\n").unwrap_err();

        assert!(
            format!("{error:#}").ends_with(
                "is invalid: set either `targets` (directories) or `clients` (their skill directories), not both"
            ),
            "{error:#}"
        );
    }

    #[test]
    fn rejects_an_empty_or_unknown_client_list() {
        let empty = parse_v2("clients: []\n").unwrap_err();
        assert!(
            format!("{empty:#}").contains("at least one client is required"),
            "{empty:#}"
        );
        for clients in ["[cursor]", "cursor", "{claude: true}"] {
            assert!(
                parse_v2(&format!("clients: {clients}\n")).is_err(),
                "{clients}"
            );
        }
    }

    #[test]
    fn points_clients_in_version_one_at_version_two() {
        let error =
            parse("clients: claude\nrepos:\n  - repo: local\n    paths:\n      - path: .\n")
                .unwrap_err();

        assert!(
            format!("{error:#}").contains("`clients` and `discovery` need `version: 2`"),
            "{error:#}"
        );
    }

    #[test]
    fn a_client_parses_from_its_name() {
        for choice in ClientChoice::ALL {
            assert_eq!(choice.name().parse(), Ok(choice));
        }
        assert!("cursor".parse::<ClientChoice>().is_err());
    }
}
