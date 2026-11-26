//! Turning the config into the list of skills that should be deployed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::{Config, Destination, Repo, Scope, directory_key};
use crate::skill;
use crate::source::{Materializer, relative_slug};
use crate::{deploy, discovery};

/// One skill the config selects, ready to be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// The name the skill deploys under.
    pub name: String,
    /// Where its files currently sit.
    pub source_dir: PathBuf,
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
    /// Fingerprint of its tree.
    pub checksum: String,
}

impl Planned {
    /// How the skill's origin reads in output.
    #[must_use]
    pub fn origin(&self) -> String {
        match &self.revision {
            Some(revision) => format!("{}@{revision} {}", self.repo, self.source_path),
            None => format!("{} {}", self.repo, self.source_path),
        }
    }
}

/// The skills planned for one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The directory, as resolved for this run.
    pub dir: PathBuf,
    /// The skills deployed into it, ordered by name.
    pub skills: Vec<Planned>,
}

/// The outcome of walking every source the config declares.
#[derive(Debug, Default)]
pub struct Plan {
    /// Every skill that passed validation, each once however many
    /// directories it goes to, ordered by name.
    pub skills: Vec<Planned>,
    /// The directories something is planned for, in the order they are
    /// first named.
    pub targets: Vec<Target>,
    /// Skills that were found but rejected, one message each.
    pub problems: Vec<String>,
}

impl Plan {
    /// The skills planned for `dir`, empty when none is.
    #[must_use]
    pub fn skills_for(&self, dir: &Path) -> &[Planned] {
        let key = directory_key(dir);
        self.targets
            .iter()
            .find(|target| directory_key(&target.dir) == key)
            .map_or(&[], |target| &target.skills)
    }
}

/// Materialize every source and collect the skills its paths select, each
/// planned for the directories its entry's destination resolves to in
/// `scope`.
///
/// With `allow_hidden_characters`, a skill holding invisible characters is
/// selected, with a warning, rather than reported as a problem.
///
/// # Errors
///
/// When a source cannot be fetched, a declared path is missing, or two
/// sources send the same skill name to one directory.
pub async fn build(
    config: &Config,
    scope: &Scope,
    materializer: &impl Materializer,
    allow_hidden_characters: bool,
) -> Result<Plan> {
    let mut targets: Vec<(PathBuf, BTreeMap<String, Planned>)> = Vec::new();
    let mut skills = Vec::new();
    let mut problems = Vec::new();

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
                    problems.push(format!("{label}: {error}"));
                    continue;
                }
            };

            let planned = Planned {
                name: frontmatter.name,
                source_path,
                checksum: deploy::checksum(&dir)?,
                source_dir: dir,
                repo: repo.location(),
                revision: repo.revision().map(str::to_owned),
                commit: fetched.resolved.clone(),
                digest,
            };

            for target in config.directories(destination, scope) {
                let key = directory_key(&target);
                let index = if let Some(index) = targets
                    .iter()
                    .position(|(dir, _)| directory_key(dir) == key)
                {
                    index
                } else {
                    targets.push((target, BTreeMap::new()));
                    targets.len() - 1
                };
                let (target, planned_there) = &mut targets[index];
                if let Some(existing) = planned_there.get(&planned.name) {
                    bail!(
                        "two sources provide the skill `{}` in {}: {} and {}",
                        planned.name,
                        target.display(),
                        existing.origin(),
                        planned.origin()
                    );
                }
                tracing::debug!(skill = %planned.name, origin = %planned.origin(), target = %target.display(), "selected");
                planned_there.insert(planned.name.clone(), planned.clone());
            }
            skills.push(planned);
        }
    }

    skills.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.origin().cmp(&b.origin()))
    });
    Ok(Plan {
        skills,
        targets: targets
            .into_iter()
            .map(|(dir, skills)| Target {
                dir,
                skills: skills.into_values().collect(),
            })
            .collect(),
        problems,
    })
}
