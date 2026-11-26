//! `pollen.yaml` as written: one set of types per format version.
//!
//! These types own the serde shape, the JSON Schema and the validation of a
//! version, each in that version's own vocabulary, so an error names the keys
//! the file actually uses. Nothing outside `config` sees them: a validated
//! file is converted into the model the commands run on, an older version
//! through each newer one, so there is a single model however many versions
//! are read.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use super::{
    AgentSelection, Client, ClientChoice, Destination, IndexSelection, LOCAL_REPO, Repo, Selection,
    is_digest, normalize_origin,
};

/// The newest format version this pollen reads.
pub const LATEST: u32 = 2;

/// Only the `version` key, read before the rest of the file to pick the
/// types that parse it.
#[derive(Deserialize)]
pub struct Probe {
    /// The `version` key as written, whatever its type.
    #[serde(default)]
    pub version: Option<serde_norway::Value>,
}

/// A `pollen.yaml` in format version 1, the format of a file that sets no
/// `version`.
#[derive(Debug, Clone, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "pollen configuration, version 1")]
pub struct V1 {
    /// The format version of this file. 1 is the default when the key is
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("const" = 1))]
    pub version: Option<u32>,
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
    pub repos: Vec<RepoV1>,
}

/// One source of skills in version 1: a git repository, the config file's own
/// directory, or an origin publishing an Agent Skills discovery index.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_source_keys_v1, skip_on_field_errors = true))]
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
pub struct RepoV1 {
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

/// A `pollen.yaml` in format version 2: a `repo` lists where to look under
/// `skills`, and the origins serving a discovery index have a list of their
/// own, `discovery`.
#[derive(Debug, Clone, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_sources_v2))]
#[schemars(
    title = "pollen configuration, version 2",
    extend(
        "anyOf" = serde_json::json!([{"required": ["repos"]}, {"required": ["discovery"]}])
    )
)]
pub struct V2 {
    /// The format version of this file.
    #[schemars(extend("const" = 2))]
    pub version: u32,
    /// The clients an entry naming neither `clients` nor `targets` deploys
    /// for, one or a list: `all`, `claude-code` (skills in `.claude/skills`)
    /// or `opencode` (skills in `.agents/skills`).
    ///
    /// The directories are under the home directory with `--global`.
    /// Defaults to `all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_clients))]
    pub clients: Option<ClientList>,
    /// The git repositories, and the config file's own directory, to pull
    /// skills from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "at least one repo is required"), nested)]
    #[schemars(length(min = 1))]
    pub repos: Option<Vec<RepoV2>>,
    /// The origins serving an Agent Skills discovery index to pull skills
    /// from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "at least one origin is required"), nested)]
    #[schemars(length(min = 1))]
    pub discovery: Option<Vec<DiscoveryV2>>,
}

/// The `clients` of a version 2 file: one choice, or a list of them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(
    untagged,
    expecting = "`clients` takes `all`, `claude-code` or `opencode`, or a list of them"
)]
pub enum ClientList {
    /// A single choice.
    One(ClientChoice),
    /// Several choices.
    Many(#[schemars(length(min = 1))] Vec<ClientChoice>),
}

impl ClientList {
    /// The choices, however they are written.
    #[must_use]
    pub fn list(&self) -> &[ClientChoice] {
        match self {
            Self::One(choice) => std::slice::from_ref(choice),
            Self::Many(choices) => choices,
        }
    }
}

/// A git repository, or the config file's own directory, in version 2.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_repo_v2, skip_on_field_errors = true))]
#[schemars(extend(
    "if" = serde_json::json!({"properties": {"repo": {"const": LOCAL_REPO}}}),
    "then" = serde_json::json!({"properties": {"revision": false}}),
    "else" = serde_json::json!({
        "required": ["revision"],
        "properties": {"revision": {"type": "string", "minLength": 1}}
    }),
    "anyOf" = serde_json::json!([{"required": ["skills"]}, {"required": ["agents"]}])
))]
pub struct RepoV2 {
    /// A git URL, a path to a git repository, or `local` for the directory
    /// holding this config file.
    #[validate(length(min = 1, message = "must not be empty"))]
    #[schemars(length(min = 1))]
    pub repo: String,
    /// The git revision to check out: a tag, a branch or a commit.
    ///
    /// Required for a git source, and forbidden on `local`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Where to look for skills inside the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "at least one skill is required"), nested)]
    #[schemars(length(min = 1))]
    pub skills: Option<Vec<SkillsV2>>,
    /// Where to look for agent files inside the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(
        length(min = 1, message = "at least one agent path is required"),
        nested
    )]
    #[schemars(length(min = 1))]
    pub agents: Option<Vec<AgentsV2>>,
}

/// One directory of agent files inside a git or `local` source, in version
/// 2, and where its agents go.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_destination_of_agents, skip_on_field_errors = true))]
#[schemars(extend("not" = serde_json::json!({"required": ["clients", "targets"]})))]
pub struct AgentsV2 {
    /// Directory holding the agent files, relative to the source root.
    ///
    /// Every file directly under it ending in `.md` is an agent, named after
    /// the file; subdirectories are not searched.
    #[validate(custom(function = check_contained_path))]
    pub path: PathBuf,
    /// Regular expression rejecting agents whose name, the file name without
    /// `.md`, matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_regex))]
    #[schemars(extend("format" = "regex"))]
    pub exclude: Option<String>,
    /// The clients these agents are deployed for, one or a list, instead of
    /// the top-level `clients`: into each one's agent directory, checked
    /// against each one's rules. Not with `targets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_clients))]
    pub clients: Option<ClientList>,
    /// Directories these agents are deployed into, instead of the agent
    /// directories of any client. Not with `clients`.
    ///
    /// A relative path resolves against the working directory, and a leading
    /// `~` is expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "at least one target is required"))]
    #[schemars(length(min = 1))]
    pub targets: Option<Vec<PathBuf>>,
}

/// One place to look for skills inside a git or `local` source, in version
/// 2, and where the skills it selects go.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_destination_of_skills, skip_on_field_errors = true))]
#[schemars(extend("not" = serde_json::json!({"required": ["clients", "targets"]})))]
pub struct SkillsV2 {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_regex))]
    #[schemars(extend("format" = "regex"))]
    pub exclude: Option<String>,
    /// The clients these skills are deployed for, one or a list, instead of
    /// the top-level `clients`. Not with `targets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_clients))]
    pub clients: Option<ClientList>,
    /// Directories these skills are deployed into, each getting a full copy,
    /// instead of the skill directories of any client. Not with `clients`.
    ///
    /// A relative path resolves against the working directory, and a leading
    /// `~` is expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "at least one target is required"))]
    #[schemars(length(min = 1))]
    pub targets: Option<Vec<PathBuf>>,
}

/// An origin serving an Agent Skills discovery index, in version 2.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = check_discovery_v2, skip_on_field_errors = true))]
#[schemars(extend("allOf" = serde_json::json!([
    {"not": {"required": ["exclude", "skills"]}},
    {"not": {"required": ["clients", "targets"]}}
])))]
pub struct DiscoveryV2 {
    /// An `https://` origin serving `/.well-known/agent-skills/index.json`,
    /// whose skills are downloaded and checked against the index's digests.
    #[validate(custom(function = check_url))]
    #[schemars(extend("format" = "uri"))]
    pub url: String,
    /// Regular expression rejecting the skills of the index whose name
    /// matches. Only with every skill of the index, not with `skills`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_regex))]
    #[schemars(extend("format" = "regex"))]
    pub exclude: Option<String>,
    /// The skills to take from the index, instead of all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "at least one skill is required"), nested)]
    #[schemars(length(min = 1))]
    pub skills: Option<Vec<SkillPin>>,
    /// The clients these skills are deployed for, one or a list, instead of
    /// the top-level `clients`. Not with `targets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_clients))]
    pub clients: Option<ClientList>,
    /// Directories these skills are deployed into, each getting a full copy,
    /// instead of the skill directories of any client. Not with `clients`.
    ///
    /// A relative path resolves against the working directory, and a leading
    /// `~` is expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(length(min = 1, message = "at least one target is required"))]
    #[schemars(length(min = 1))]
    pub targets: Option<Vec<PathBuf>>,
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Validate, JsonSchema)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = check_regex))]
    #[schemars(extend("format" = "regex"))]
    pub exclude: Option<String>,
}

impl From<V1> for V2 {
    /// A validated version 1 file, said in version 2: a `repo`'s `paths`
    /// move under `skills`, each `well-known` entry moves to `discovery`, and
    /// the top-level `targets` goes on every skills item and every origin.
    fn from(v1: V1) -> Self {
        let mut repos = Vec::new();
        let mut discovery = Vec::new();
        for entry in v1.repos {
            match (entry.repo, entry.well_known) {
                (Some(repo), _) => repos.push(RepoV2 {
                    repo,
                    revision: entry.revision,
                    skills: Some(
                        entry
                            .paths
                            .into_iter()
                            .map(|spec| SkillsV2 {
                                path: spec.path,
                                recurse: spec.recurse,
                                exclude: spec.exclude,
                                clients: None,
                                targets: v1.targets.clone(),
                            })
                            .collect(),
                    ),
                    agents: None,
                }),
                (None, Some(url)) => discovery.push(DiscoveryV2 {
                    url,
                    exclude: entry.exclude,
                    skills: entry.skills,
                    clients: None,
                    targets: v1.targets.clone(),
                }),
                (None, None) => {}
            }
        }
        Self {
            version: 2,
            clients: None,
            repos: (!repos.is_empty()).then_some(repos),
            discovery: (!discovery.is_empty()).then_some(discovery),
        }
    }
}

impl V2 {
    /// The sources of a validated file, in the model: the `repos` first,
    /// then the `discovery` origins, each list in the order it is written.
    #[must_use]
    pub fn into_sources(self) -> Vec<Repo> {
        let repos = self.repos.into_iter().flatten().map(RepoV2::into_model);
        let discovery = self
            .discovery
            .into_iter()
            .flatten()
            .map(DiscoveryV2::into_model);
        repos.chain(discovery).collect()
    }
}

impl RepoV2 {
    /// The model of a validated entry.
    #[must_use]
    pub fn into_model(self) -> Repo {
        let paths = self
            .skills
            .into_iter()
            .flatten()
            .map(SkillsV2::into_model)
            .collect();
        let agents = self
            .agents
            .into_iter()
            .flatten()
            .map(AgentsV2::into_model)
            .collect();
        if self.repo == LOCAL_REPO {
            Repo::Local { paths, agents }
        } else {
            Repo::Git {
                url: self.repo,
                revision: self.revision.unwrap_or_default(),
                paths,
                agents,
            }
        }
    }
}

impl AgentsV2 {
    /// The model of a validated item.
    #[must_use]
    pub fn into_model(self) -> AgentSelection {
        AgentSelection {
            path: self.path,
            exclude: self.exclude,
            destination: destination(self.clients.as_ref(), self.targets),
        }
    }
}

impl SkillsV2 {
    /// The model of a validated item.
    #[must_use]
    pub fn into_model(self) -> Selection {
        Selection {
            spec: PathSpec {
                path: self.path,
                recurse: self.recurse,
                exclude: self.exclude,
            },
            destination: destination(self.clients.as_ref(), self.targets),
        }
    }
}

impl DiscoveryV2 {
    /// The model of a validated entry.
    #[must_use]
    pub fn into_model(self) -> Repo {
        let skills = match self.skills {
            Some(pins) => IndexSelection::Only(pins),
            None => IndexSelection::All {
                exclude: self.exclude,
            },
        };
        Repo::WellKnown {
            origin: normalize_origin(&self.url).unwrap_or(self.url),
            skills,
            destination: destination(self.clients.as_ref(), self.targets),
        }
    }
}

/// The destination an entry's `clients` and `targets` name, which validation
/// made mutually exclusive.
fn destination(clients: Option<&ClientList>, targets: Option<Vec<PathBuf>>) -> Destination {
    match (clients, targets) {
        (_, Some(targets)) => Destination::Targets(targets),
        (Some(clients), None) => Destination::Clients(Client::expand(clients.list())),
        (None, None) => Destination::Default,
    }
}

fn check_clients(clients: &ClientList) -> Result<(), ValidationError> {
    if clients.list().is_empty() {
        return Err(named_error("clients", "at least one client is required"));
    }
    Ok(())
}

fn check_source_keys_v1(repo: &RepoV1) -> Result<(), ValidationError> {
    check_one_kind(repo.repo.as_ref(), repo.well_known.as_ref())?;
    if repo.well_known.is_some() {
        if repo.revision.is_some() {
            return Err(well_known_revision());
        }
        if !repo.paths.is_empty() {
            return Err(named_error(
                "paths",
                "a `well-known` source takes no paths; select skills by name under `skills`, or leave some out with `exclude`",
            ));
        }
        if repo.exclude.is_some() && repo.skills.is_some() {
            return Err(exclude_with_skills());
        }
        return check_unique(repo.skills.iter().flatten().map(|skill| &skill.name));
    }
    if repo.exclude.is_some() || repo.skills.is_some() {
        return Err(named_error(
            "skills",
            "`skills` and a top-level `exclude` belong to a `well-known` source; a `repo` selects skills under `paths`, or under `skills` with `version: 2`",
        ));
    }
    if repo.paths.is_empty() {
        return Err(named_error("paths", "at least one path is required"));
    }
    check_revision_matches_repo(repo.repo.as_deref(), repo.revision.as_deref())
}

fn check_sources_v2(v2: &V2) -> Result<(), ValidationError> {
    if v2.repos.is_none() && v2.discovery.is_none() {
        return Err(named_error(
            "repos",
            "at least one source is required, under `repos` or `discovery`",
        ));
    }
    Ok(())
}

fn check_repo_v2(repo: &RepoV2) -> Result<(), ValidationError> {
    if repo.skills.is_none() && repo.agents.is_none() {
        return Err(named_error(
            "skills",
            "a `repo` needs `skills`, `agents`, or both",
        ));
    }
    check_revision_matches_repo(Some(&repo.repo), repo.revision.as_deref())
}

fn check_discovery_v2(origin: &DiscoveryV2) -> Result<(), ValidationError> {
    if origin.exclude.is_some() && origin.skills.is_some() {
        return Err(exclude_with_skills());
    }
    check_one_destination(origin.clients.as_ref(), origin.targets.as_ref())?;
    check_unique(origin.skills.iter().flatten().map(|skill| &skill.name))
}

fn check_destination_of_skills(item: &SkillsV2) -> Result<(), ValidationError> {
    check_one_destination(item.clients.as_ref(), item.targets.as_ref())
}

fn check_destination_of_agents(item: &AgentsV2) -> Result<(), ValidationError> {
    check_one_destination(item.clients.as_ref(), item.targets.as_ref())
}

fn check_one_destination(
    clients: Option<&ClientList>,
    targets: Option<&Vec<PathBuf>>,
) -> Result<(), ValidationError> {
    if clients.is_some() && targets.is_some() {
        return Err(named_error(
            "targets",
            "an entry takes either `clients` or `targets`, not both: `targets` names the directories themselves",
        ));
    }
    Ok(())
}

fn check_one_kind(
    repo: Option<&String>,
    well_known: Option<&String>,
) -> Result<(), ValidationError> {
    match (repo, well_known) {
        (Some(_), Some(_)) => Err(named_error(
            "repo",
            "an entry takes either `repo` or `well-known`, not both",
        )),
        (None, None) => Err(named_error(
            "repo",
            "an entry needs `repo` (a git repository or `local`) or `well-known` (an origin)",
        )),
        _ => Ok(()),
    }
}

fn well_known_revision() -> ValidationError {
    named_error(
        "revision",
        "a `well-known` source takes no revision; pin each skill with a `digest` under `skills`",
    )
}

fn exclude_with_skills() -> ValidationError {
    named_error(
        "exclude",
        "`exclude` filters every skill of the index, so it does not combine with a `skills` list",
    )
}

fn check_unique<'a>(names: impl Iterator<Item = &'a String>) -> Result<(), ValidationError> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            let mut error = named_error("skills", "a skill is listed twice");
            error.add_param("name".into(), name);
            return Err(error);
        }
    }
    Ok(())
}

fn check_origin(origin: &str) -> Result<(), ValidationError> {
    normalize_origin(origin).map(|_| ()).ok_or_else(|| {
        named_error(
            "well-known",
            "must be an origin, such as `https://skills.example.com`, with no path, query or credentials",
        )
    })
}

fn check_url(url: &str) -> Result<(), ValidationError> {
    normalize_origin(url).map(|_| ()).ok_or_else(|| {
        named_error(
            "url",
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

fn check_revision_matches_repo(
    repo: Option<&str>,
    revision: Option<&str>,
) -> Result<(), ValidationError> {
    let is_local = repo == Some(LOCAL_REPO);
    let has_revision = revision.is_some_and(|rev| !rev.is_empty());

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
