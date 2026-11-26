//! Turning the config into the skills and agents each directory should hold.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::agent::{self, Details};
use crate::config::{Client, Config, Destination, Repo, Scope, directory_key};
use crate::skill;
use crate::source::{Materializer, relative_slug};
use crate::state::Kind;
use crate::{deploy, discovery};

/// One skill or agent the config selects, ready to be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// The name it deploys under.
    pub name: String,
    /// Where it currently sits: the skill directory, or the agent file.
    pub source: PathBuf,
    /// The `repo:` value that selected it.
    pub repo: String,
    /// The `revision:` that source is pinned to.
    pub revision: Option<String>,
    /// The commit the revision resolved to.
    pub commit: Option<String>,
    /// The digest of the artifact a `well-known` source served it as.
    pub digest: Option<String>,
    /// Its path inside the source.
    pub source_path: String,
    /// Fingerprint of the skill's tree, or of the agent file's bytes.
    pub checksum: String,
    /// What an agent's frontmatter grants; `None` for a skill.
    pub details: Option<Details>,
}

impl Planned {
    /// How the origin reads in output.
    #[must_use]
    pub fn origin(&self) -> String {
        match &self.revision {
            Some(revision) => format!("{}@{revision} {}", self.repo, self.source_path),
            None => format!("{} {}", self.repo, self.source_path),
        }
    }
}

/// What is planned for one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The directory, as resolved for this run.
    pub dir: PathBuf,
    /// What it holds: skills or agents, never both.
    pub kind: Kind,
    /// The skills or agents deployed into it, ordered by name.
    pub entries: Vec<Planned>,
}

/// A skill or an agent the config selects that cannot be deployed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub kind: Kind,
    pub message: String,
}

/// How many skills and agents `problems` hold, as a sentence: `2 skill(s)
/// and 1 agent(s) are invalid`.
#[must_use]
pub fn describe(problems: &[Problem]) -> String {
    let count = |kind| {
        problems
            .iter()
            .filter(|problem| problem.kind == kind)
            .count()
    };
    let parts: Vec<String> = [(Kind::Skills, "skill(s)"), (Kind::Agents, "agent(s)")]
        .into_iter()
        .filter_map(|(kind, noun)| match count(kind) {
            0 => None,
            count => Some(format!("{count} {noun}")),
        })
        .collect();
    format!("{} are invalid", parts.join(" and "))
}

/// The outcome of walking every source the config declares.
#[derive(Debug, Default)]
pub struct Plan {
    /// Every skill that passed validation, each once however many
    /// directories it goes to, ordered by name.
    pub skills: Vec<Planned>,
    /// Every agent that passed validation, each once, ordered by name.
    pub agents: Vec<Planned>,
    /// The directories something is planned for, in the order they are
    /// first named.
    pub targets: Vec<Target>,
    /// Skills and agents that were found but rejected, one message each.
    pub problems: Vec<Problem>,
}

impl Plan {
    /// What is planned for `dir`, `None` when nothing is.
    #[must_use]
    pub fn target_for(&self, dir: &Path) -> Option<&Target> {
        let key = directory_key(dir);
        self.targets
            .iter()
            .find(|target| directory_key(&target.dir) == key)
    }
}

/// The directories planned so far, each with its entries keyed by name.
#[derive(Default)]
struct Targets(Vec<(PathBuf, Kind, BTreeMap<String, Planned>)>);

impl Targets {
    /// Plan `planned` for `dir`, which holds `kind`.
    fn place(&mut self, dir: PathBuf, kind: Kind, planned: &Planned) -> Result<()> {
        let key = directory_key(&dir);
        let index = if let Some(index) = self
            .0
            .iter()
            .position(|(known, ..)| directory_key(known) == key)
        {
            index
        } else {
            self.0.push((dir, kind, BTreeMap::new()));
            self.0.len() - 1
        };
        let (dir, held, planned_there) = &mut self.0[index];
        if *held != kind {
            bail!(
                "{} would receive both skills and agents; a directory holds one kind",
                dir.display()
            );
        }
        if let Some(existing) = planned_there.get(&planned.name) {
            bail!(
                "two sources provide the {} `{}` in {}: {} and {}",
                kind.nouns().0,
                planned.name,
                dir.display(),
                existing.origin(),
                planned.origin()
            );
        }
        tracing::debug!(name = %planned.name, origin = %planned.origin(), target = %dir.display(), "selected");
        planned_there.insert(planned.name.clone(), planned.clone());
        Ok(())
    }
}

/// Materialize every source and collect the skills and agents it selects,
/// each planned for the directories its entry's destination resolves to in
/// `scope`. An agent is checked against the rules of every client it is
/// deployed for.
///
/// With `allow_hidden_characters`, a skill or an agent holding invisible
/// characters is selected, with a warning, rather than reported as a
/// problem.
///
/// # Errors
///
/// When a source cannot be fetched, a declared path is missing, two sources
/// send the same name to one directory, or one directory would receive both
/// skills and agents.
pub async fn build(
    config: &Config,
    scope: &Scope,
    materializer: &impl Materializer,
    allow_hidden_characters: bool,
) -> Result<Plan> {
    let mut targets = Targets::default();
    let mut plan = Plan::default();

    for repo in &config.repos {
        let label = repo.label();
        let fetched = materializer
            .materialize(repo)
            .await
            .with_context(|| format!("cannot make {label} available"))?;
        let root = fetched
            .root
            .canonicalize()
            .with_context(|| format!("{} does not exist", fetched.root.display()))?;
        let origin = Origin {
            repo,
            label: &label,
            root: &root,
            commit: fetched.resolved.as_ref(),
        };

        // A `well-known` source selects its skills itself, by name; the
        // others select them under `paths:`.
        let mut candidates: Vec<(PathBuf, String, Option<String>, &Destination)> = Vec::new();
        if let Repo::WellKnown { destination, .. } = repo {
            candidates.extend(fetched.skills.iter().map(|skill| {
                (
                    skill.dir.clone(),
                    skill.name.clone(),
                    Some(skill.digest.clone()),
                    destination,
                )
            }));
        }
        for selection in repo.paths() {
            for dir in discovery::discover(&root, &selection.spec)? {
                let source_path = relative_slug(&root, &dir);
                candidates.push((dir, source_path, None, &selection.destination));
            }
        }

        for (dir, source_path, digest, destination) in candidates {
            let frontmatter = match skill::load(&dir, allow_hidden_characters) {
                Ok(frontmatter) => frontmatter,
                Err(error) => {
                    plan.problems.push(Problem {
                        kind: Kind::Skills,
                        message: format!("{label}: {error}"),
                    });
                    continue;
                }
            };

            let planned = Planned {
                name: frontmatter.name,
                source_path,
                checksum: deploy::checksum(&dir)?,
                source: dir,
                repo: repo.location(),
                revision: repo.revision().map(str::to_owned),
                commit: fetched.resolved.clone(),
                digest,
                details: None,
            };
            for target in config.directories(destination, scope) {
                targets.place(target, Kind::Skills, &planned)?;
            }
            plan.skills.push(planned);
        }

        select_agents(
            config,
            scope,
            &origin,
            allow_hidden_characters,
            &mut targets,
            &mut plan,
        )?;
    }

    for list in [&mut plan.skills, &mut plan.agents] {
        list.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.origin().cmp(&b.origin()))
        });
    }
    plan.targets = targets
        .0
        .into_iter()
        .map(|(dir, kind, entries)| Target {
            dir,
            kind,
            entries: entries.into_values().collect(),
        })
        .collect();
    Ok(plan)
}

/// A materialized source, as its agents are planned from it.
struct Origin<'a> {
    repo: &'a Repo,
    label: &'a str,
    root: &'a Path,
    commit: Option<&'a String>,
}

/// Plan the agents the `agents:` items of a source select, each checked
/// against the rules of the clients it goes to; an agent breaking one goes
/// to `plan.problems` instead.
fn select_agents(
    config: &Config,
    scope: &Scope,
    origin: &Origin<'_>,
    allow_hidden_characters: bool,
    targets: &mut Targets,
    plan: &mut Plan,
) -> Result<()> {
    for selection in origin.repo.agents() {
        let dirs = config.agent_directories(&selection.destination, scope);
        let mut clients: Vec<Client> = dirs.iter().filter_map(|dir| dir.client).collect();
        clients.sort();
        clients.dedup();

        for file in discovery::agent_files(origin.root, selection)? {
            let refusals: Vec<String> = match agent::load(&file, allow_hidden_characters) {
                Ok(found) => {
                    let refusals: Vec<String> = clients
                        .iter()
                        .filter_map(|client| found.check(*client).err())
                        .map(|error| error.to_string())
                        .collect();
                    if refusals.is_empty() {
                        let planned = Planned {
                            name: found.name,
                            source_path: relative_slug(origin.root, &file),
                            checksum: deploy::checksum_file(&file)?,
                            source: file,
                            repo: origin.repo.location(),
                            revision: origin.repo.revision().map(str::to_owned),
                            commit: origin.commit.cloned(),
                            digest: None,
                            details: Some(found.details),
                        };
                        for dir in &dirs {
                            targets.place(dir.dir.clone(), Kind::Agents, &planned)?;
                        }
                        plan.agents.push(planned);
                    }
                    refusals
                }
                Err(error) => vec![error.to_string()],
            };
            plan.problems
                .extend(refusals.into_iter().map(|refusal| Problem {
                    kind: Kind::Agents,
                    message: format!("{}: {refusal}", origin.label),
                }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_the_problems_of_each_kind() {
        let problem = |kind| Problem {
            kind,
            message: String::new(),
        };

        assert_eq!(describe(&[problem(Kind::Skills)]), "1 skill(s) are invalid");
        assert_eq!(describe(&[problem(Kind::Agents)]), "1 agent(s) are invalid");
        assert_eq!(
            describe(&[
                problem(Kind::Agents),
                problem(Kind::Skills),
                problem(Kind::Skills)
            ]),
            "2 skill(s) and 1 agent(s) are invalid"
        );
    }
}
