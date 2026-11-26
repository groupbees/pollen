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

/// A client pollen deploys for, named as the product names itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Client {
    /// Claude Code.
    ClaudeCode,
    /// OpenCode.
    #[serde(rename = "opencode")]
    OpenCode,
}

/// What pollen knows about a client: where it reads skills and agents, and
/// what makes one of its agent files valid.
#[derive(Debug, PartialEq, Eq)]
pub struct ClientSpec {
    /// The directory holding the client's skills, relative to the working
    /// directory, or to the home directory with `--global`.
    pub skills_dir: &'static str,
    /// The directory holding the client's agents in a project, relative to
    /// the working directory.
    pub agents_dir: &'static str,
    /// The directory holding the client's agents for the user, with
    /// `--global`.
    pub user_agents_dir: UserDir,
    /// What an agent file's name ends with; the rest is the agent's name.
    pub agent_suffix: &'static str,
    /// How the client tells its agents apart.
    pub identity: Identity,
    /// The frontmatter fields an agent must set, each to a non-empty string.
    pub required: &'static [&'static str],
}

/// A user-level directory, as the client locates it.
#[derive(Debug, PartialEq, Eq)]
pub enum UserDir {
    /// Under the home directory.
    Home(&'static str),
    /// Under `$XDG_CONFIG_HOME`, or `~/.config` when it is unset or empty, on
    /// every platform.
    XdgConfig(&'static str),
}

/// How a client names an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    /// The frontmatter `name`, which pollen requires to equal the file name
    /// so two files cannot claim one agent.
    NameField,
    /// The file name.
    FileName,
}

/// A client named in the config or on the command line: one client, or all
/// of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ClientChoice {
    /// Claude Code, into `.claude/skills` and `.claude/agents`.
    ClaudeCode,
    /// OpenCode, into `.agents/skills` and `.opencode/agents`.
    #[serde(rename = "opencode")]
    OpenCode,
    /// Every client pollen knows.
    All,
}

/// Where an entry's skills or agents go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// The directories of the config's top-level `clients`.
    Default,
    /// The directories of these clients: their skill directories for skills,
    /// their agent directories for agents.
    Clients(Vec<Client>),
    /// These directories, as written.
    Targets(Vec<PathBuf>),
}

/// How one run turns destinations into directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// The command line's `--target`, which replaces the destination of
    /// every entry, or its `--client`, which keeps only what the entries send
    /// to those clients; [`Destination::Default`] when neither is given.
    pub cli: Destination,
    /// The directory client skill directories sit in: empty for the working
    /// directory, the home directory with `--global`.
    pub root: PathBuf,
    /// The user's configuration directory, `$XDG_CONFIG_HOME` or
    /// `~/.config`, with `--global`, where a client such as OpenCode reads
    /// its user agents; `None` in a project, where every agent directory
    /// sits in `root`.
    pub user_config: Option<PathBuf>,
}

/// A parsed and validated `pollen.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The format version the file is written in.
    pub version: u32,
    /// The clients an entry naming no destination deploys for, when the
    /// file names them.
    pub clients: Option<Vec<Client>>,
    /// The sources to pull skills from, in the order the file lists them.
    pub repos: Vec<Repo>,
    /// Directory holding the config file; local paths resolve against it.
    pub base_dir: PathBuf,
    /// Absolute path of the config file, recorded as the owner of every
    /// target it deploys into. Empty for a config not read from a file.
    pub file: PathBuf,
}

/// One source of skills and agents, and what to take from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repo {
    /// The directory holding `pollen.yaml`.
    Local {
        /// Where to look for skills.
        paths: Vec<Selection>,
        /// Where to look for agents.
        agents: Vec<AgentSelection>,
    },
    /// A git repository, pinned to a revision.
    Git {
        /// The URL or path passed to `git`.
        url: String,
        /// The tag, branch or commit to check out.
        revision: String,
        /// Where to look for skills.
        paths: Vec<Selection>,
        /// Where to look for agents.
        agents: Vec<AgentSelection>,
    },
    /// An origin publishing an Agent Skills discovery index.
    WellKnown {
        /// The origin, `scheme://host[:port]`, without a trailing slash.
        origin: String,
        /// Which skills of the index to take.
        skills: IndexSelection,
        /// Where they go.
        destination: Destination,
    },
}

/// One place to look for skills inside a git or `local` source, and where
/// the skills it selects go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Where to look.
    pub spec: PathSpec,
    /// Where they go.
    pub destination: Destination,
}

/// One directory of agent files inside a git or `local` source, and where
/// they go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSelection {
    /// The directory, relative to the source root; the agent files directly
    /// under it are taken.
    pub path: PathBuf,
    /// Regular expression rejecting agents by name.
    pub exclude: Option<String>,
    /// Where they go.
    pub destination: Destination,
}

/// A directory an agent goes to, and the client reading it there, whose
/// rules the agent must follow; `None` for a directory of `targets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDir {
    pub dir: PathBuf,
    pub client: Option<Client>,
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

    /// The directories `destination` deploys into: the command line's
    /// `--target` when `scope` has one, else the entry's own `targets`, else
    /// the skill directories of its `clients`, else those of the config's
    /// `clients`, every client by default. A `--client` run keeps only the
    /// clients among those, so an entry sent to `targets` deploys nowhere.
    ///
    /// Duplicates are dropped so a directory named twice is written once.
    #[must_use]
    pub fn directories(&self, destination: &Destination, scope: &Scope) -> Vec<PathBuf> {
        let chosen = match (&scope.cli, destination) {
            (Destination::Default, Destination::Default) => {
                scope.client_dirs(self.clients.as_deref().unwrap_or(&Client::ALL))
            }
            (Destination::Clients(only), destination) => {
                let own: &[Client] = match destination {
                    Destination::Default => self.clients.as_deref().unwrap_or(&Client::ALL),
                    Destination::Clients(clients) => clients,
                    Destination::Targets(_) => &[],
                };
                let kept: Vec<Client> = own
                    .iter()
                    .copied()
                    .filter(|client| only.contains(client))
                    .collect();
                scope.client_dirs(&kept)
            }
            (Destination::Default, Destination::Clients(clients)) => scope.client_dirs(clients),
            (Destination::Targets(targets), _)
            | (Destination::Default, Destination::Targets(targets)) => {
                targets.iter().map(|target| expand_home(target)).collect()
            }
        };
        dedup(chosen)
    }

    /// The directories the agents of `destination` deploy into, each with
    /// its client: none for a run with `--target`, which deploys skills
    /// only; else the agent directories of the command line's clients, else
    /// the entry's own `targets`, which have no client, else the agent
    /// directories of its `clients`, else those of the config's `clients`.
    #[must_use]
    pub fn agent_directories(&self, destination: &Destination, scope: &Scope) -> Vec<AgentDir> {
        let of_clients = |clients: &[Client]| {
            clients
                .iter()
                .map(|client| AgentDir {
                    dir: scope.agents_dir(*client),
                    client: Some(*client),
                })
                .collect()
        };
        let chosen: Vec<AgentDir> = match (&scope.cli, destination) {
            (Destination::Targets(_), _) => Vec::new(),
            (Destination::Default, Destination::Default) => {
                of_clients(self.clients.as_deref().unwrap_or(&Client::ALL))
            }
            (Destination::Clients(clients), _)
            | (Destination::Default, Destination::Clients(clients)) => of_clients(clients),
            (Destination::Default, Destination::Targets(targets)) => targets
                .iter()
                .map(|target| AgentDir {
                    dir: expand_home(target),
                    client: None,
                })
                .collect(),
        };
        let mut seen = BTreeSet::new();
        chosen
            .into_iter()
            .filter(|agents| seen.insert(directory_key(&agents.dir)))
            .collect()
    }

    /// Every directory an entry of this config deploys skills into, in the
    /// order they are first named.
    #[must_use]
    pub fn all_directories(&self, scope: &Scope) -> Vec<PathBuf> {
        dedup(
            self.repos
                .iter()
                .flat_map(Repo::destinations)
                .flat_map(|destination| self.directories(destination, scope))
                .collect(),
        )
    }

    /// Every directory an entry of this config deploys agents into, in the
    /// order they are first named.
    #[must_use]
    pub fn all_agent_directories(&self, scope: &Scope) -> Vec<PathBuf> {
        dedup(
            self.repos
                .iter()
                .flat_map(Repo::agents)
                .flat_map(|selection| self.agent_directories(&selection.destination, scope))
                .map(|agents| agents.dir)
                .collect(),
        )
    }
}

impl Scope {
    /// A run with `targets` from `--target`, else `clients` from
    /// `--client`, else neither, over the working directory or, when
    /// `global`, the home directory.
    #[must_use]
    pub fn new(targets: &[PathBuf], clients: &[Client], global: bool) -> Self {
        let cli = if !targets.is_empty() {
            Destination::Targets(targets.to_vec())
        } else if !clients.is_empty() {
            Destination::Clients(clients.to_vec())
        } else {
            Destination::Default
        };
        let home = dirs::home_dir();
        let root = if global {
            home.clone().unwrap_or_else(|| PathBuf::from("~"))
        } else {
            PathBuf::new()
        };
        let user_config =
            global.then(|| user_config_home(std::env::var_os("XDG_CONFIG_HOME"), home.as_deref()));
        Self {
            cli,
            root,
            user_config,
        }
    }

    /// The agent directory of `client` in this scope.
    #[must_use]
    pub fn agents_dir(&self, client: Client) -> PathBuf {
        let spec = client.spec();
        match (&self.user_config, &spec.user_agents_dir) {
            (None, _) => self.root.join(spec.agents_dir),
            (Some(_), UserDir::Home(dir)) => self.root.join(dir),
            (Some(config), UserDir::XdgConfig(dir)) => config.join(dir),
        }
    }

    /// The skill directories of `clients`, in this scope.
    #[must_use]
    pub fn client_dirs(&self, clients: &[Client]) -> Vec<PathBuf> {
        clients
            .iter()
            .map(|client| self.root.join(client.spec().skills_dir))
            .collect()
    }

    /// Whether the command line replaces every entry's destination.
    #[must_use]
    pub fn overridden(&self) -> bool {
        self.cli != Destination::Default
    }
}

/// Where a client following the XDG layout reads its user configuration, on
/// every platform: a non-empty `$XDG_CONFIG_HOME`, else `~/.config`.
fn user_config_home(xdg: Option<OsString>, home: Option<&Path>) -> PathBuf {
    match xdg.filter(|xdg| !xdg.is_empty()) {
        Some(xdg) => PathBuf::from(xdg),
        None => home.map_or_else(|| PathBuf::from("~/.config"), |home| home.join(".config")),
    }
}

fn dedup(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    dirs.into_iter()
        .filter(|dir| seen.insert(directory_key(dir)))
        .collect()
}

/// What tells two spellings of one directory apart from two directories:
/// the absolute path with its `.` and `..` resolved by reading it, not the
/// filesystem. Two spellings deployed separately would each prune what the
/// other installs.
#[must_use]
pub fn directory_key(dir: &Path) -> PathBuf {
    let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut key = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                key.pop();
            }
            other => key.push(other),
        }
    }
    key
}

impl Client {
    /// Every client pollen knows, in deployment order.
    pub const ALL: [Self; 2] = [Self::ClaudeCode, Self::OpenCode];

    /// Where this client reads what pollen deploys.
    #[must_use]
    pub const fn spec(self) -> &'static ClientSpec {
        const CLAUDE_CODE: ClientSpec = ClientSpec {
            skills_dir: ".claude/skills",
            agents_dir: ".claude/agents",
            user_agents_dir: UserDir::Home(".claude/agents"),
            agent_suffix: crate::agent::SUFFIX,
            identity: Identity::NameField,
            required: &["name", "description"],
        };
        const OPENCODE: ClientSpec = ClientSpec {
            skills_dir: ".agents/skills",
            agents_dir: ".opencode/agents",
            user_agents_dir: UserDir::XdgConfig("opencode/agents"),
            agent_suffix: crate::agent::SUFFIX,
            identity: Identity::FileName,
            required: &["description"],
        };
        match self {
            Self::ClaudeCode => &CLAUDE_CODE,
            Self::OpenCode => &OPENCODE,
        }
    }

    /// The name the config and the command line give this client.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::OpenCode => "opencode",
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
}

impl std::fmt::Display for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

impl ClientChoice {
    /// Every choice, in the order they are documented.
    pub const ALL: [Self; 3] = [Self::ClaudeCode, Self::OpenCode, Self::All];

    /// The name the config and the command line give this choice.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::OpenCode => "opencode",
            Self::All => "all",
        }
    }

    /// The clients this choice names.
    #[must_use]
    pub const fn clients(self) -> &'static [Client] {
        match self {
            Self::ClaudeCode => &[Client::ClaudeCode],
            Self::OpenCode => &[Client::OpenCode],
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
                format!("unknown client `{value}`: expected `claude-code`, `opencode` or `all`")
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
    let clients = latest
        .clients
        .as_ref()
        .map(|clients| Client::expand(clients.list()));
    Ok(Config {
        version,
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
/// one version in the other: version 2's `clients`, `discovery` or a
/// `repo`'s `agents` in version 1, a top-level `targets`, a `repo`'s `paths`
/// or a `well-known` entry in version 2, or a `repo`'s `skills` in version 1.
/// Read from the YAML itself, so it does not depend on how serde words its
/// error.
fn version_hint(text: &str, version: u32) -> Option<&'static str> {
    let document: serde_norway::Value = serde_norway::from_str(text).ok()?;
    let repos = document
        .get("repos")
        .and_then(serde_norway::Value::as_sequence)
        .map_or(&[][..], Vec::as_slice);
    let uses = |key: &str| {
        repos
            .iter()
            .any(|entry| entry.get("repo").is_some() && entry.get(key).is_some())
    };
    if version == 1
        && (document.get("clients").is_some()
            || document.get("discovery").is_some()
            || uses("agents"))
    {
        return Some(
            "`clients`, `discovery` and `agents` need `version: 2`: take them out, run `pollen config migrate`, then put them back",
        );
    }
    if version == 2 && document.get("targets").is_some() {
        return Some(
            "version 2 names `targets` on each item of a `repo`'s `skills` or `agents` and on each `discovery` entry, not at the top level",
        );
    }
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
    pub fn paths(&self) -> &[Selection] {
        match self {
            Self::Local { paths, .. } | Self::Git { paths, .. } => paths,
            Self::WellKnown { .. } => &[],
        }
    }

    /// Where to look for agents inside a git or `local` source; empty for a
    /// `well-known` one, whose index lists skills only.
    #[must_use]
    pub fn agents(&self) -> &[AgentSelection] {
        match self {
            Self::Local { agents, .. } | Self::Git { agents, .. } => agents,
            Self::WellKnown { .. } => &[],
        }
    }

    /// Where the skills of this entry go, one destination per item of a git
    /// or `local` source.
    pub fn destinations(&self) -> impl Iterator<Item = &Destination> {
        let own = match self {
            Self::WellKnown { destination, .. } => Some(destination),
            _ => None,
        };
        self.paths()
            .iter()
            .map(|selection| &selection.destination)
            .chain(own)
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

impl AgentSelection {
    /// The compiled `exclude` pattern.
    ///
    /// # Errors
    ///
    /// When the pattern is not a valid regular expression, which validation
    /// already rules out for a config read by [`Config::load`].
    pub fn exclude_regex(&self) -> Result<Option<Regex>> {
        compile(self.exclude.as_deref())
    }
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
            ..
        } = &config.repos[0]
        else {
            panic!("not a git source: {:?}", config.repos[0]);
        };
        assert_eq!(url, "https://github.com/toto/tata");
        assert_eq!(revision, "1.2.3");
        assert!(paths[0].spec.recurse);
        assert_eq!(paths[0].destination, Destination::Default);
        assert!(
            paths[0]
                .spec
                .exclude_regex()
                .unwrap()
                .unwrap()
                .is_match("toto-skill")
        );
    }

    #[test]
    fn defaults_recurse_to_false_and_exclude_to_none() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: skills\n").unwrap();

        assert!(!config.repos[0].paths()[0].spec.recurse);
        assert!(config.repos[0].paths()[0].spec.exclude.is_none());
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

        assert_eq!(
            config.repos[0].paths()[0].spec.path,
            PathBuf::from("skills")
        );
    }

    #[test]
    fn rejects_an_empty_repo_list() {
        assert!(parse("repos: []\n").is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("repos:\n  - repo: local\n    pathz:\n      - path: .\n").is_err());
    }

    /// A run with no `--target` and no `--client`, in the working directory.
    fn plain() -> Scope {
        Scope::new(&[], &[], false)
    }

    fn strings(dirs: &[PathBuf]) -> Vec<String> {
        dirs.iter()
            .map(|dir| dir.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn targets_default_to_claude_code_and_the_cross_client_convention() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap();

        assert_eq!(
            strings(&config.all_directories(&plain())),
            [".claude/skills", ".agents/skills"]
        );
    }

    #[test]
    fn targets_prefer_the_cli_overrides_then_the_config() {
        let config = parse(
            "targets:\n  - /opt/skills\nrepos:\n  - repo: local\n    paths:\n      - path: .\n",
        )
        .unwrap();

        assert_eq!(
            config.all_directories(&Scope::new(&[PathBuf::from("/tmp/override")], &[], false)),
            [PathBuf::from("/tmp/override")]
        );
        assert_eq!(
            config.all_directories(&plain()),
            [PathBuf::from("/opt/skills")]
        );
    }

    #[test]
    fn global_targets_default_to_the_home_directory() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap();
        let home = dirs::home_dir().unwrap();

        assert_eq!(
            config.all_directories(&Scope::new(&[], &[], true)),
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

        let Repo::WellKnown { origin, skills, .. } = &config.repos[0] else {
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
            config.all_directories(&Scope::new(&repeated, &[], false)),
            [PathBuf::from("skills")]
        );
    }

    #[test]
    fn two_spellings_of_one_directory_are_one_directory() {
        let config = two_entries("        targets: [./.agents/skills, a/../.agents/skills]\n");

        assert_eq!(
            strings(&config.all_directories(&plain())),
            ["./.agents/skills"]
        );
        assert_eq!(
            directory_key(Path::new("./x/../y")),
            directory_key(Path::new("y"))
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
        let v2 = "version: 2\nrepos:\n  - repo: https://x.example/a.git\n    revision: v1.0.0\n    skills:\n      - path: skills\n        recurse: true\n        exclude: ^draft-\n        targets: [skills-out]\n  - repo: local\n    skills:\n      - path: mine\n        targets: [skills-out]\ndiscovery:\n  - url: https://x.example\n    targets: [skills-out]\n    skills:\n      - name: a\n";

        let (v1, v2) = (parse(V1_FILE).unwrap(), parse(v2).unwrap());

        assert_eq!(v2.version, 2);
        assert_eq!(v2.repos, v1.repos);
        assert_eq!(v2.clients, v1.clients);
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
            format!("{discovery_in_v1:#}").contains("`discovery` and `agents` need `version: 2`"),
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
            ("clients: all\n", "at least one source"),
            ("targets: [out]\n", "on each item of a `repo`'s `skills`"),
            (
                "repos:\n  - repo: local\n    skills:\n      - path: .\n        targets: []\n",
                "at least one target",
            ),
            (
                "repos:\n  - repo: local\n    skills:\n      - path: .\n        clients: []\n",
                "at least one client",
            ),
            (
                "repos:\n  - repo: local\n    skills:\n      - path: .\n        clients: cursor\n",
                "clients",
            ),
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
            ("claude-code", vec![".claude/skills"]),
            ("[opencode]", vec![".agents/skills"]),
            (
                "[opencode, claude-code, opencode]",
                vec![".agents/skills", ".claude/skills"],
            ),
            (
                "[claude-code, all]",
                vec![".claude/skills", ".agents/skills"],
            ),
        ] {
            let config = parse_v2(&format!("clients: {clients}\n")).unwrap();

            assert_eq!(
                strings(&config.all_directories(&plain())),
                expected,
                "{clients}"
            );
        }
    }

    #[test]
    fn global_clients_select_the_home_skill_directories() {
        let config = parse_v2("clients: claude-code\n").unwrap();
        let home = dirs::home_dir().unwrap();

        assert_eq!(
            config.all_directories(&Scope::new(&[], &[], true)),
            [home.join(".claude/skills")]
        );
    }

    /// A config whose first entry deploys to `own`, a destination written as
    /// the keys of a skills item, and whose second takes the top-level
    /// `clients: opencode`.
    fn two_entries(own: &str) -> Config {
        parse(&format!(
            "version: 2\nclients: opencode\nrepos:\n  - repo: local\n    skills:\n      - path: one\n{own}      - path: two\n"
        ))
        .unwrap()
    }

    fn per_entry(config: &Config, scope: &Scope) -> Vec<Vec<String>> {
        config
            .repos
            .iter()
            .flat_map(Repo::destinations)
            .map(|destination| strings(&config.directories(destination, scope)))
            .collect()
    }

    #[test]
    fn an_entry_takes_its_own_targets_then_its_own_clients_then_the_top_level_clients() {
        let targets = two_entries("        targets: [out, ~/shared, out]\n");
        let clients = two_entries("        clients: [claude-code, opencode]\n");
        let home = dirs::home_dir().unwrap();

        assert_eq!(
            per_entry(&targets, &plain()),
            [
                vec!["out".to_owned(), strings(&[home.join("shared")])[0].clone()],
                vec![".agents/skills".to_owned()]
            ]
        );
        assert_eq!(
            per_entry(&clients, &plain()),
            [
                vec![".claude/skills", ".agents/skills"],
                vec![".agents/skills"]
            ]
        );
        assert_eq!(
            strings(&clients.all_directories(&plain())),
            [".claude/skills", ".agents/skills"]
        );
    }

    #[test]
    fn cli_targets_win_over_cli_clients_and_every_entry() {
        let config = two_entries("        targets: [out]\n");
        let scope = Scope::new(
            &[PathBuf::from("/tmp/override")],
            &[Client::OpenCode],
            false,
        );

        assert_eq!(
            per_entry(&config, &scope),
            [vec!["/tmp/override"], vec!["/tmp/override"]]
        );
    }

    #[test]
    fn cli_clients_keep_only_what_entries_send_to_them() {
        for (own, expected) in [
            ("        targets: [out]\n", vec![]),
            ("        clients: claude-code\n", vec![]),
            (
                "        clients: [claude-code, opencode]\n",
                vec![".agents/skills"],
            ),
            ("", vec![".agents/skills"]),
        ] {
            let config = two_entries(own);
            let scope = Scope::new(&[], &[Client::OpenCode], false);

            assert_eq!(
                per_entry(&config, &scope),
                [expected, vec![".agents/skills"]],
                "{own}"
            );
            assert!(scope.overridden());
        }
    }

    #[test]
    fn cli_clients_leave_out_a_client_the_config_does_not_deploy_for() {
        let config = parse(&format!("version: 2\nclients: opencode\n{V2_LOCAL}")).unwrap();

        assert!(
            config
                .all_directories(&Scope::new(&[], &[Client::ClaudeCode], false))
                .is_empty()
        );
    }

    #[test]
    fn a_discovery_entry_takes_its_own_destination() {
        let config = parse(
            "version: 2\ndiscovery:\n  - url: https://a.example\n    targets: [out]\n  - url: https://b.example\n    clients: claude-code\n  - url: https://c.example\n",
        )
        .unwrap();

        assert_eq!(
            per_entry(&config, &plain()),
            [
                vec!["out"],
                vec![".claude/skills"],
                vec![".claude/skills", ".agents/skills"]
            ]
        );
    }

    #[test]
    fn refuses_clients_and_targets_on_one_entry() {
        for yaml in [
            "repos:\n  - repo: local\n    skills:\n      - path: .\n        clients: all\n        targets: [out]\n",
            "discovery:\n  - url: https://x.example\n    clients: all\n    targets: [out]\n",
        ] {
            let error = parse(&format!("version: 2\n{yaml}")).unwrap_err();

            assert!(
                format!("{error:#}").contains("either `clients` or `targets`, not both"),
                "{yaml}: {error:#}"
            );
        }
    }

    #[test]
    fn version_one_puts_its_targets_on_every_entry() {
        let config = parse(V1_FILE).unwrap();
        let out = Destination::Targets(vec![PathBuf::from("skills-out")]);

        assert_eq!(config.clients, None);
        assert!(
            config
                .repos
                .iter()
                .flat_map(Repo::destinations)
                .all(|d| *d == out),
            "{config:?}"
        );
        assert_eq!(config.repos.iter().flat_map(Repo::destinations).count(), 3);
    }

    #[test]
    fn a_version_one_file_without_targets_takes_the_default() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: a\n      - path: b\n  - well-known: https://x.example\n").unwrap();

        assert!(
            config
                .repos
                .iter()
                .flat_map(Repo::destinations)
                .all(|destination| *destination == Destination::Default)
        );
    }

    #[test]
    fn rejects_an_empty_or_unknown_client_list() {
        let empty = parse_v2("clients: []\n").unwrap_err();
        assert!(
            format!("{empty:#}").contains("at least one client is required"),
            "{empty:#}"
        );
        for clients in [
            "[cursor]",
            "cursor",
            "claude",
            "agent-skills",
            "{claude-code: true}",
        ] {
            assert!(
                parse_v2(&format!("clients: {clients}\n")).is_err(),
                "{clients}"
            );
        }
    }

    #[test]
    fn points_clients_in_version_one_at_version_two() {
        let error =
            parse("clients: claude-code\nrepos:\n  - repo: local\n    paths:\n      - path: .\n")
                .unwrap_err();

        assert!(
            format!("{error:#}").contains("`clients`, `discovery` and `agents` need `version: 2`"),
            "{error:#}"
        );
    }

    const AGENTS: &str = "version: 2\nrepos:\n  - repo: https://x.example/a.git\n    revision: v1\n    agents:\n      - path: agents\n        exclude: ^README$\n      - path: claude\n        clients: claude-code\n      - path: shared\n        targets: [out, ~/agents]\n  - repo: local\n    skills:\n      - path: skills\n    agents:\n      - path: mine\n";

    #[test]
    fn a_repo_declares_agents_beside_or_instead_of_skills() {
        let config = parse(AGENTS).unwrap();

        let git = &config.repos[0];
        assert!(git.paths().is_empty());
        let destinations: Vec<_> = git
            .agents()
            .iter()
            .map(|agents| agents.destination.clone())
            .collect();
        assert_eq!(
            destinations,
            [
                Destination::Default,
                Destination::Clients(vec![Client::ClaudeCode]),
                Destination::Targets(vec![PathBuf::from("out"), PathBuf::from("~/agents")]),
            ]
        );
        assert!(
            git.agents()[0]
                .exclude_regex()
                .unwrap()
                .unwrap()
                .is_match("README")
        );
        assert_eq!(config.repos[1].paths().len(), 1);
        assert_eq!(config.repos[1].agents()[0].path, PathBuf::from("mine"));
    }

    #[test]
    fn rejects_an_agents_item_breaking_a_rule() {
        for (agents, expected) in [
            (
                "      - path: a\n        client: opencode\n",
                "unknown field `client`",
            ),
            (
                "      - path: a\n        clients: cursor\n",
                "`clients` takes",
            ),
            (
                "      - path: a\n        clients: []\n",
                "at least one client",
            ),
            (
                "      - path: a\n        targets: []\n",
                "at least one target",
            ),
            (
                "      - path: a\n        clients: opencode\n        targets: [out]\n",
                "either `clients` or `targets`",
            ),
            ("      - exclude: ^a\n", "missing field `path`"),
            ("      - path: ../a\n", "climb"),
            (
                "      - path: a\n        exclude: \"[\"\n",
                "regular expression",
            ),
            (
                "      - path: a\n        recurse: true\n",
                "unknown field `recurse`",
            ),
        ] {
            let error = parse(&format!(
                "version: 2\nrepos:\n  - repo: local\n    agents:\n{agents}"
            ))
            .unwrap_err();
            assert!(
                format!("{error:#}").contains(expected),
                "{agents}: {error:#}"
            );
        }
        let empty = parse("version: 2\nrepos:\n  - repo: local\n    agents: []\n").unwrap_err();
        assert!(
            format!("{empty:#}").contains("at least one agent path"),
            "{empty:#}"
        );
        let neither = parse("version: 2\nrepos:\n  - repo: local\n").unwrap_err();
        assert!(
            format!("{neither:#}").contains("needs `skills`, `agents`, or both"),
            "{neither:#}"
        );
    }

    #[test]
    fn a_discovery_origin_takes_no_agents() {
        let error = parse(
            "version: 2\ndiscovery:\n  - url: https://x.example\n    agents:\n      - path: a\n",
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("unknown field `agents`"),
            "{error:#}"
        );
    }

    #[test]
    fn points_agents_in_version_one_at_version_two() {
        let error = parse(
            "repos:\n  - repo: local\n    paths:\n      - path: .\n    agents:\n      - path: a\n",
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("`agents` need `version: 2`"),
            "{error:#}"
        );
    }

    fn agent_dirs(dirs: &[AgentDir]) -> Vec<(String, Option<Client>)> {
        dirs.iter()
            .map(|agents| {
                (
                    agents.dir.to_string_lossy().replace('\\', "/"),
                    agents.client,
                )
            })
            .collect()
    }

    #[test]
    fn agents_go_to_the_agent_directories_of_their_clients_or_to_their_targets() {
        let config = parse(AGENTS).unwrap();
        let [default, claude, shared] = config.repos[0].agents() else {
            panic!("three agents items");
        };
        let home = dirs::home_dir().unwrap();

        assert_eq!(
            agent_dirs(&config.agent_directories(&default.destination, &plain())),
            [
                (".claude/agents".to_owned(), Some(Client::ClaudeCode)),
                (".opencode/agents".to_owned(), Some(Client::OpenCode)),
            ]
        );
        assert_eq!(
            agent_dirs(&config.agent_directories(&claude.destination, &plain())),
            [(".claude/agents".to_owned(), Some(Client::ClaudeCode))]
        );
        assert_eq!(
            agent_dirs(&config.agent_directories(&shared.destination, &plain())),
            [
                ("out".to_owned(), None),
                (
                    home.join("agents").to_string_lossy().replace('\\', "/"),
                    None
                ),
            ]
        );
        assert_eq!(
            strings(&config.all_agent_directories(&plain()))[..3],
            [".claude/agents", ".opencode/agents", "out"]
        );
    }

    #[test]
    fn the_top_level_clients_pick_the_agent_directories_of_an_item_naming_none() {
        let config =
            parse(&AGENTS.replace("version: 2\n", "version: 2\nclients: opencode\n")).unwrap();

        assert_eq!(
            agent_dirs(&config.agent_directories(&Destination::Default, &plain())),
            [(".opencode/agents".to_owned(), Some(Client::OpenCode))]
        );
    }

    #[test]
    fn a_cli_target_sends_no_agent_anywhere_and_a_cli_client_to_its_agent_directory() {
        let config = parse(AGENTS).unwrap();
        let shared = &config.repos[0].agents()[2].destination;

        let target = Scope::new(&[PathBuf::from("out")], &[], false);
        let client = Scope::new(&[], &[Client::OpenCode], false);

        assert!(config.agent_directories(shared, &target).is_empty());
        assert!(config.all_agent_directories(&target).is_empty());
        assert_eq!(
            agent_dirs(&config.agent_directories(shared, &client)),
            [(".opencode/agents".to_owned(), Some(Client::OpenCode))]
        );
    }

    #[test]
    fn user_agent_directories_follow_each_clients_layout() {
        let root = PathBuf::from("/home/me");
        let config = std::env::temp_dir().join("xdg");
        let scope = Scope {
            cli: Destination::Default,
            root: root.clone(),
            user_config: Some(config.clone()),
        };
        let project = Scope {
            cli: Destination::Default,
            root: root.clone(),
            user_config: None,
        };

        assert_eq!(
            scope.agents_dir(Client::ClaudeCode),
            root.join(".claude/agents")
        );
        assert_eq!(
            scope.agents_dir(Client::OpenCode),
            config.join("opencode/agents")
        );
        assert_eq!(
            project.agents_dir(Client::OpenCode),
            root.join(".opencode/agents")
        );
    }

    #[test]
    fn the_user_configuration_follows_xdg_config_home_else_dot_config_on_every_platform() {
        let home = PathBuf::from("/home/me");
        let xdg = std::env::temp_dir().join("xdg");

        assert_eq!(
            user_config_home(Some(xdg.clone().into_os_string()), Some(&home)),
            xdg
        );
        for unset in [None, Some(OsString::new())] {
            assert_eq!(user_config_home(unset, Some(&home)), home.join(".config"));
        }
    }

    #[test]
    fn a_client_parses_from_its_name() {
        for choice in ClientChoice::ALL {
            assert_eq!(choice.name().parse(), Ok(choice));
        }
        assert!("cursor".parse::<ClientChoice>().is_err());
    }
}
