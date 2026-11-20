//! The declarative input: `pollen.yaml`, its schema, and its validation.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

/// The sentinel `repo:` value meaning "paths are relative to this config file".
pub const LOCAL_REPO: &str = "local";

/// A parsed and validated `pollen.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_target_keys, skip_on_field_errors = true))]
#[schemars(
    title = "pollen configuration",
    extend("not" = serde_json::json!({"required": ["targets", "clients"]}))
)]
pub struct Config {
    /// Directories the skills are deployed into, each getting a full copy.
    ///
    /// A relative path resolves against the working directory, and a leading
    /// `~` is expanded. Forbidden with `clients`.
    #[validate(length(min = 1, message = "at least one target is required"))]
    #[schemars(length(min = 1))]
    pub targets: Option<Vec<PathBuf>>,
    /// The clients the skills are deployed for, each through its own skill
    /// directory: `all`, or a list of `claude` (`.claude/skills`) and
    /// `agent-skills` (`.agents/skills`).
    ///
    /// The directories are under the home directory with `--global`.
    /// Defaults to `all`. Forbidden with `targets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_clients))]
    pub clients: Option<Clients>,
    /// The sources to pull skills from.
    #[validate(length(min = 1, message = "at least one repo is required"), nested)]
    #[schemars(length(min = 1))]
    pub repos: Vec<RepoSpec>,
    /// Directory holding the config file; local paths resolve against it.
    #[serde(skip)]
    pub base_dir: PathBuf,
    /// Absolute path of the config file, recorded as the owner of every
    /// target it deploys into. Empty for a config not read from a file.
    #[serde(skip)]
    pub file: PathBuf,
}

/// An Agent Skills client, named by the skill directory it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Client {
    /// Claude Code, which reads `.claude/skills`.
    Claude,
    /// The cross-client convention every other Agent Skills client scans:
    /// `.agents/skills`.
    AgentSkills,
}

/// The clients a config deploys for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(
    untagged,
    expecting = "`clients` takes `all`, or a list of `claude` and `agent-skills`"
)]
pub enum Clients {
    /// Every client pollen knows.
    All(AllClients),
    /// Only these clients.
    Only(#[schemars(length(min = 1))] Vec<Client>),
}

/// The `all` keyword of [`Clients`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AllClients {
    /// Every client pollen knows.
    All,
}

/// One source of skills: a git repository, the config file's own directory,
/// or an origin publishing an Agent Skills discovery index.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_source_keys, skip_on_field_errors = true))]
#[schemars(extend("oneOf" = serde_json::json!([
    {
        "required": ["repo", "paths"],
        "properties": {"well-known": false, "exclude": false, "skills": false},
        "if": {"properties": {"repo": {"const": LOCAL_REPO}}},
        "then": {"properties": {"revision": false}},
        "else": {
            "required": ["revision"],
            "properties": {"revision": {"type": "string", "minLength": 1}}
        }
    },
    {
        "required": ["well-known"],
        "properties": {"repo": false, "revision": false, "paths": false},
        "not": {"required": ["exclude", "skills"]}
    }
])))]
pub struct RepoSpec {
    /// A git URL, a path to a git repository, or `local` for the directory
    /// holding this config file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "must not be empty"))]
    #[schemars(with = "String", length(min = 1))]
    pub repo: Option<String>,
    /// An `https://` origin serving `/.well-known/agent-skills/index.json`,
    /// whose skills are downloaded and checked against the index's digests.
    #[serde(
        rename = "well-known",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[validate(custom(function = check_origin))]
    #[schemars(with = "String", extend("format" = "uri"))]
    pub well_known: Option<String>,
    /// The git revision to check out: a tag, a branch or a commit.
    ///
    /// Required for a git source, and forbidden on `local` and `well-known`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Where to look for skills inside a git or `local` source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[validate(nested)]
    #[schemars(length(min = 1))]
    pub paths: Vec<PathSpec>,
    /// Regular expression rejecting the skills of a `well-known` index whose
    /// name matches. Only with every skill of the index, not with `skills`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_regex))]
    #[schemars(extend("format" = "regex"))]
    pub exclude: Option<String>,
    /// The skills to take from a `well-known` index, instead of all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "at least one skill is required"), nested)]
    #[schemars(length(min = 1))]
    pub skills: Option<Vec<SkillPin>>,
}

/// One skill taken from a `well-known` index.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillPin {
    /// The skill's name in the index.
    #[validate(custom(function = crate::skill::validate_name))]
    #[schemars(
        regex(pattern = r"^[a-z0-9]+(-[a-z0-9]+)*$"),
        length(min = 1, max = 64)
    )]
    pub name: String,
    /// The SHA-256 of the artifact to deploy, as the index writes it:
    /// `sha256:` and 64 lowercase hex digits. A run fails rather than deploy
    /// anything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_digest))]
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: Option<String>,
}

/// One place to look for skills inside a source.
#[derive(Debug, Clone, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathSpec {
    /// Directory to search, relative to the source root.
    ///
    /// A directory holding a `SKILL.md` is itself the skill; otherwise its
    /// subdirectories are searched.
    #[validate(custom(function = check_contained_path))]
    pub path: PathBuf,
    /// Search the whole subtree rather than only the immediate children.
    #[serde(default)]
    pub recurse: bool,
    /// Regular expression rejecting skills whose path under `path` matches.
    #[validate(custom(function = check_regex))]
    #[schemars(extend("format" = "regex"))]
    pub exclude: Option<String>,
}

/// Where a [`RepoSpec`]'s files come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The directory holding `pollen.yaml`.
    Local,
    /// A git repository, pinned to a revision.
    Git {
        /// The URL or path passed to `git`.
        url: String,
        /// The tag, branch or commit to check out.
        revision: String,
    },
    /// An origin publishing an Agent Skills discovery index.
    WellKnown {
        /// The origin, `scheme://host[:port]`, without a trailing slash.
        origin: String,
    },
}

impl Config {
    /// Read, parse and validate the config file at `path`.
    ///
    /// # Errors
    ///
    /// When the file cannot be read, is not valid YAML, or breaks a rule.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read the config file {}", path.display()))?;
        let mut config: Self = serde_norway::from_str(&text)
            .with_context(|| format!("{} is not a valid pollen config", path.display()))?;
        // `validator` files a struct-level failure under the field name
        // `__all__`, which means nothing to whoever is reading the error.
        config.validate().map_err(|errors| {
            anyhow!(
                "{} is invalid: {}",
                path.display(),
                errors
                    .to_string()
                    .replace(".__all__", "")
                    .replace("__all__: ", "")
            )
        })?;
        config.base_dir = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        // Canonical, so `../pollen.yaml` from a subdirectory and a symlinked
        // path name the same owner as the plain path.
        config.file = path
            .canonicalize()
            .with_context(|| format!("cannot resolve {}", path.display()))?;
        Ok(config)
    }

    /// The git sources whose `revision` is not a full commit id, and the
    /// `well-known` sources not listing every skill with its `digest`.
    #[must_use]
    pub fn unpinned(&self) -> Vec<&RepoSpec> {
        self.repos
            .iter()
            .filter(|repo| match repo.source() {
                Source::Local => false,
                Source::Git { revision, .. } => !is_commit_id(&revision),
                Source::WellKnown { .. } => repo
                    .skills
                    .as_ref()
                    .is_none_or(|skills| skills.iter().any(|skill| skill.digest.is_none())),
            })
            .collect()
    }

    /// The directories to deploy into: the CLI overrides, else the config's
    /// `targets`, else the skill directories of its `clients` — in the
    /// working directory, or in the home directory when `global`.
    ///
    /// Duplicates are dropped so a directory named twice is written once.
    #[must_use]
    pub fn targets(&self, overrides: &[PathBuf], global: bool) -> Vec<PathBuf> {
        let chosen = if !overrides.is_empty() {
            overrides.to_vec()
        } else if let Some(targets) = &self.targets {
            targets.clone()
        } else {
            self.clients
                .as_ref()
                .map_or(&Client::ALL[..], Clients::list)
                .iter()
                .map(|client| client.dir(global))
                .collect()
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

    fn dir(self, global: bool) -> PathBuf {
        let dir = match self {
            Self::Claude => ".claude/skills",
            Self::AgentSkills => ".agents/skills",
        };
        if global {
            PathBuf::from(format!("~/{dir}"))
        } else {
            PathBuf::from(dir)
        }
    }
}

impl Clients {
    /// The clients this value names.
    #[must_use]
    pub fn list(&self) -> &[Client] {
        match self {
            Self::All(_) => &Client::ALL,
            Self::Only(clients) => clients,
        }
    }
}

impl RepoSpec {
    /// Where this entry's files come from.
    #[must_use]
    pub fn source(&self) -> Source {
        if let Some(origin) = &self.well_known {
            return Source::WellKnown {
                origin: normalize_origin(origin).unwrap_or_else(|| origin.clone()),
            };
        }
        match self.repo.as_deref() {
            Some(LOCAL_REPO) => Source::Local,
            repo => Source::Git {
                url: repo.unwrap_or_default().to_owned(),
                revision: self.revision.clone().unwrap_or_default(),
            },
        }
    }

    /// The `repo:` value, or the origin of a `well-known` source: what the
    /// state file records as the skill's repo.
    #[must_use]
    pub fn location(&self) -> String {
        match self.source() {
            Source::Local => LOCAL_REPO.to_owned(),
            Source::Git { url, .. } => url,
            Source::WellKnown { origin } => origin,
        }
    }

    /// How this entry is named in output.
    #[must_use]
    pub fn label(&self) -> String {
        match self.source() {
            Source::Local => LOCAL_REPO.to_owned(),
            Source::Git { url, revision } => format!("{url}@{revision}"),
            Source::WellKnown { origin } => origin,
        }
    }

    /// The compiled `exclude` pattern of a `well-known` source.
    ///
    /// # Errors
    ///
    /// When the pattern is not a valid regular expression, which validation
    /// already rules out for a config read by [`Config::load`].
    pub fn exclude_regex(&self) -> Result<Option<Regex>> {
        compile(self.exclude.as_deref())
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

fn check_target_keys(config: &Config) -> Result<(), ValidationError> {
    if config.targets.is_some() && config.clients.is_some() {
        return Err(named_error(
            "clients",
            "set either `targets` (directories) or `clients` (their default skill directories), not both",
        ));
    }
    Ok(())
}

fn check_clients(clients: &Clients) -> Result<(), ValidationError> {
    if clients.list().is_empty() {
        return Err(named_error("clients", "at least one client is required"));
    }
    Ok(())
}

fn check_source_keys(repo: &RepoSpec) -> Result<(), ValidationError> {
    match (&repo.repo, &repo.well_known) {
        (Some(_), Some(_)) => Err(named_error(
            "repo",
            "an entry takes either `repo` or `well-known`, not both",
        )),
        (None, None) => Err(named_error(
            "repo",
            "an entry needs `repo` (a git repository or `local`) or `well-known` (an origin)",
        )),
        (Some(_), None) => check_repo_keys(repo),
        (None, Some(_)) => check_well_known_keys(repo),
    }
}

fn check_well_known_keys(repo: &RepoSpec) -> Result<(), ValidationError> {
    if repo.revision.is_some() {
        return Err(named_error(
            "revision",
            "a `well-known` source takes no revision; pin each skill with a `digest` under `skills`",
        ));
    }
    if !repo.paths.is_empty() {
        return Err(named_error(
            "paths",
            "a `well-known` source takes no paths; select skills by name under `skills`, or leave some out with `exclude`",
        ));
    }
    if repo.exclude.is_some() && repo.skills.is_some() {
        return Err(named_error(
            "exclude",
            "`exclude` filters every skill of the index, so it does not combine with a `skills` list",
        ));
    }
    let mut seen = BTreeSet::new();
    if let Some(duplicate) = repo
        .skills
        .iter()
        .flatten()
        .find(|skill| !seen.insert(&skill.name))
    {
        let mut error = named_error("skills", "a skill is listed twice");
        error.add_param("name".into(), &duplicate.name);
        return Err(error);
    }
    Ok(())
}

fn check_repo_keys(repo: &RepoSpec) -> Result<(), ValidationError> {
    if repo.exclude.is_some() || repo.skills.is_some() {
        return Err(named_error(
            "skills",
            "`skills` and a top-level `exclude` belong to a `well-known` source; a `repo` selects skills under `paths`",
        ));
    }
    if repo.paths.is_empty() {
        return Err(named_error("paths", "at least one path is required"));
    }
    check_revision_matches_repo(repo)
}

fn check_origin(origin: &str) -> Result<(), ValidationError> {
    normalize_origin(origin).map(|_| ()).ok_or_else(|| {
        named_error(
            "well-known",
            "must be an origin, such as `https://skills.example.com`, with no path, query or credentials",
        )
    })
}

fn check_digest(digest: &str) -> Result<(), ValidationError> {
    if is_digest(digest) {
        Ok(())
    } else {
        Err(named_error(
            "digest",
            "must be `sha256:` followed by 64 lowercase hex digits",
        ))
    }
}

fn check_revision_matches_repo(repo: &RepoSpec) -> Result<(), ValidationError> {
    let is_local = repo.repo.as_deref() == Some(LOCAL_REPO);
    let has_revision = repo.revision.as_ref().is_some_and(|rev| !rev.is_empty());

    if is_local && has_revision {
        return Err(named_error(
            "revision",
            "`repo: local` reads the working tree, so it takes no revision",
        ));
    }
    if !is_local && !has_revision {
        return Err(named_error(
            "revision",
            "a git repo must be pinned with a revision (a tag, a branch or a commit)",
        ));
    }
    Ok(())
}

/// Judge `path` the same way on every platform, `/` and `\` alike, so a
/// shared config that passes on Linux cannot escape the root on Windows.
fn check_contained_path(path: &Path) -> Result<(), ValidationError> {
    let text = path.to_string_lossy();
    let has_drive = matches!(text.as_bytes(), [letter, b':', ..] if letter.is_ascii_alphabetic());
    if text.starts_with(['/', '\\']) || has_drive {
        return Err(named_error("path", "must be relative to the source root"));
    }
    if text.split(['/', '\\']).any(|part| part == "..") {
        return Err(named_error("path", "must not climb out of the source root"));
    }
    Ok(())
}

fn check_regex(pattern: &str) -> Result<(), ValidationError> {
    Regex::new(pattern)
        .map(|_| ())
        .map_err(|_| named_error("exclude", "is not a valid regular expression"))
}

fn named_error(code: &'static str, message: &'static str) -> ValidationError {
    let mut error = ValidationError::new(code);
    error.message = Some(message.into());
    error
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

        let repo = &config.repos[0];
        assert_eq!(
            repo.source(),
            Source::Git {
                url: "https://github.com/toto/tata".to_owned(),
                revision: "1.2.3".to_owned(),
            }
        );
        assert!(repo.paths[0].recurse);
        assert!(
            repo.paths[0]
                .exclude_regex()
                .unwrap()
                .unwrap()
                .is_match("toto-skill")
        );
    }

    #[test]
    fn defaults_recurse_to_false_and_exclude_to_none() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: skills\n").unwrap();

        assert!(!config.repos[0].paths[0].recurse);
        assert!(config.repos[0].paths[0].exclude.is_none());
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

        assert_eq!(config.repos[0].paths[0].path, PathBuf::from("skills"));
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
            config.targets(&[], false),
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
            config.targets(&[PathBuf::from("/tmp/override")], false),
            [PathBuf::from("/tmp/override")]
        );
        assert_eq!(config.targets(&[], false), [PathBuf::from("/opt/skills")]);
    }

    #[test]
    fn global_targets_default_to_the_home_directory() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap();
        let home = dirs::home_dir().unwrap();

        assert_eq!(
            config.targets(&[], true),
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

        assert_eq!(
            config.repos[0].source(),
            Source::WellKnown {
                origin: "https://gateway.example.com".to_owned()
            }
        );
        assert!(
            config.repos[0]
                .exclude_regex()
                .unwrap()
                .unwrap()
                .is_match("experimental-x")
        );
        assert_eq!(config.repos[1].label(), "https://gateway.example.com:8443");
        let skills = config.repos[1].skills.as_ref().unwrap();
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

        assert_eq!(config.targets(&repeated, false), [PathBuf::from("skills")]);
    }

    #[test]
    fn clients_select_their_skill_directories() {
        for (clients, expected) in [
            ("all", vec![".claude/skills", ".agents/skills"]),
            ("[claude]", vec![".claude/skills"]),
            ("[agent-skills]", vec![".agents/skills"]),
            (
                "[agent-skills, claude, agent-skills]",
                vec![".agents/skills", ".claude/skills"],
            ),
        ] {
            let config = parse(&format!(
                "clients: {clients}\nrepos:\n  - repo: local\n    paths:\n      - path: .\n"
            ))
            .unwrap();

            assert_eq!(
                config.targets(&[], false),
                expected.iter().map(PathBuf::from).collect::<Vec<_>>(),
                "{clients}"
            );
        }
    }

    #[test]
    fn global_clients_select_the_home_skill_directories() {
        let config =
            parse("clients: [claude]\nrepos:\n  - repo: local\n    paths:\n      - path: .\n")
                .unwrap();
        let home = dirs::home_dir().unwrap();

        assert_eq!(config.targets(&[], true), [home.join(".claude/skills")]);
    }

    #[test]
    fn the_cli_overrides_win_over_clients() {
        let config =
            parse("clients: [claude]\nrepos:\n  - repo: local\n    paths:\n      - path: .\n")
                .unwrap();

        assert_eq!(
            config.targets(&[PathBuf::from("/tmp/override")], false),
            [PathBuf::from("/tmp/override")]
        );
    }

    #[test]
    fn rejects_targets_with_clients() {
        let error = parse(
            "targets: [skills]\nclients: all\nrepos:\n  - repo: local\n    paths:\n      - path: .\n",
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("not both"), "{error:#}");
    }

    #[test]
    fn rejects_an_empty_or_unknown_client_list() {
        for clients in ["[]", "[cursor]", "claude", "everything"] {
            assert!(
                parse(&format!(
                    "clients: {clients}\nrepos:\n  - repo: local\n    paths:\n      - path: .\n"
                ))
                .is_err(),
                "{clients}"
            );
        }
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
}
