//! The declarative input: `pollen.yaml`, its schema, and its validation.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

/// Where skills land when neither the CLI nor the config says otherwise.
///
/// Claude Code reads the first; the second is the cross-client convention
/// every other Agent Skills client scans, so one deployment serves both.
pub const DEFAULT_TARGETS: [&str; 2] = [".claude/skills", ".agents/skills"];

/// Where skills land with `--global` when neither the CLI nor the config says
/// otherwise: the same pair, in the user's home.
pub const GLOBAL_DEFAULT_TARGETS: [&str; 2] = ["~/.claude/skills", "~/.agents/skills"];

/// The sentinel `repo:` value meaning "paths are relative to this config file".
pub const LOCAL_REPO: &str = "local";

/// A parsed and validated `pollen.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "pollen configuration")]
pub struct Config {
    /// Directories the skills are deployed into, each getting a full copy.
    ///
    /// A relative path resolves against the working directory, and a leading
    /// `~` is expanded. Defaults to `.claude/skills` and `.agents/skills`, or
    /// to `~/.claude/skills` and `~/.agents/skills` with `--global`.
    #[validate(length(min = 1, message = "at least one target is required"))]
    #[schemars(length(min = 1))]
    pub targets: Option<Vec<PathBuf>>,
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

/// One source of skills: a git repository, or the config file's own directory.
#[derive(Debug, Clone, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_revision_matches_repo, skip_on_field_errors = true))]
#[schemars(extend(
    "if" = serde_json::json!({"properties": {"repo": {"const": LOCAL_REPO}}, "required": ["repo"]}),
    "then" = serde_json::json!({"properties": {"revision": false}}),
    "else" = serde_json::json!({
        "required": ["revision"],
        "properties": {"revision": {"type": "string", "minLength": 1}}
    }),
))]
pub struct RepoSpec {
    /// A git URL, a path to a git repository, or `local` for the directory
    /// holding this config file.
    #[validate(length(min = 1, message = "must not be empty"))]
    #[schemars(length(min = 1))]
    pub repo: String,
    /// The git revision to check out: a tag, a branch or a commit.
    ///
    /// Required for a git source, and forbidden on `local`.
    pub revision: Option<String>,
    /// Where to look for skills inside the source.
    #[validate(length(min = 1, message = "at least one path is required"), nested)]
    #[schemars(length(min = 1))]
    pub paths: Vec<PathSpec>,
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
                errors.to_string().replace(".__all__", "")
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

    /// The git sources whose `revision` is not a full commit id.
    #[must_use]
    pub fn unpinned(&self) -> Vec<&RepoSpec> {
        self.repos
            .iter()
            .filter(|repo| {
                matches!(repo.source(), Source::Git { revision, .. } if !is_commit_id(&revision))
            })
            .collect()
    }

    /// The directories to deploy into: the CLI overrides, else the config's
    /// `targets`, else `defaults`.
    ///
    /// Duplicates are dropped so a directory named twice is written once.
    #[must_use]
    pub fn targets(&self, overrides: &[PathBuf], defaults: &[&str]) -> Vec<PathBuf> {
        let chosen = if overrides.is_empty() {
            self.targets
                .clone()
                .unwrap_or_else(|| defaults.iter().map(PathBuf::from).collect())
        } else {
            overrides.to_vec()
        };

        let mut seen = BTreeSet::new();
        chosen
            .iter()
            .map(|target| expand_home(target))
            .filter(|target| seen.insert(target.clone()))
            .collect()
    }
}

impl RepoSpec {
    /// Where this entry's files come from.
    #[must_use]
    pub fn source(&self) -> Source {
        if self.repo == LOCAL_REPO {
            Source::Local
        } else {
            Source::Git {
                url: self.repo.clone(),
                revision: self.revision.clone().unwrap_or_default(),
            }
        }
    }

    /// How this entry is named in output and in the state file.
    #[must_use]
    pub fn label(&self) -> String {
        match self.source() {
            Source::Local => LOCAL_REPO.to_owned(),
            Source::Git { url, revision } => format!("{url}@{revision}"),
        }
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
        self.exclude
            .as_deref()
            .map(|pattern| {
                Regex::new(pattern)
                    .with_context(|| format!("`{pattern}` is not a valid regular expression"))
            })
            .transpose()
    }
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

fn check_revision_matches_repo(repo: &RepoSpec) -> Result<(), ValidationError> {
    let is_local = repo.repo == LOCAL_REPO;
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
            config.targets(&[], &DEFAULT_TARGETS),
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
            config.targets(&[PathBuf::from("/tmp/override")], &DEFAULT_TARGETS),
            [PathBuf::from("/tmp/override")]
        );
        assert_eq!(
            config.targets(&[], &DEFAULT_TARGETS),
            [PathBuf::from("/opt/skills")]
        );
    }

    #[test]
    fn global_targets_default_to_the_home_directory() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap();
        let home = dirs::home_dir().unwrap();

        assert_eq!(
            config.targets(&[], &GLOBAL_DEFAULT_TARGETS),
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

    #[test]
    fn targets_are_deduplicated() {
        let config = parse("repos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap();
        let repeated = vec![PathBuf::from("skills"), PathBuf::from("skills")];

        assert_eq!(
            config.targets(&repeated, &DEFAULT_TARGETS),
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
}
