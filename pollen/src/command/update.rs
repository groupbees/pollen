use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::agent::{self, Details};
use crate::cli::Options;
use crate::command::plan::{self, Planned};
use crate::command::{Context, directories};
use crate::config::{Config, Scope};
use crate::deploy;
use crate::source::Materializer;
use crate::state::{InstalledSkill, Kind, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Added,
    /// Reinstalled because its source moved on.
    Updated,
    /// Already installed at this exact content.
    Unchanged,
    /// Dropped because the config no longer declares it.
    Removed,
}

impl Action {
    fn symbol(self) -> char {
        match self {
            Self::Added => '+',
            Self::Updated => '~',
            Self::Unchanged => '=',
            Self::Removed => '-',
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
            Self::Removed => "removed",
        }
    }
}

/// One line of an update's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub action: Action,
    pub name: String,
    /// Where the skill or agent comes from, empty for a removal.
    pub origin: String,
    /// What an agent's frontmatter grants; `None` for a skill and for a
    /// removal.
    pub details: Option<Details>,
}

/// The result of an update in one directory, as printed to stdout.
#[derive(Debug)]
pub struct Summary {
    pub target: PathBuf,
    /// What the directory holds.
    pub kind: Kind,
    /// One entry per skill or agent, ordered by name within each action.
    pub changes: Vec<Change>,
    /// Whether the target directory was left untouched.
    pub dry_run: bool,
}

impl Summary {
    #[must_use]
    pub fn count(&self, action: Action) -> usize {
        self.changes
            .iter()
            .filter(|change| change.action == action)
            .count()
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let width = self
            .changes
            .iter()
            .map(|change| change.name.len())
            .max()
            .unwrap_or(0);

        for change in &self.changes {
            let line = format!("{} {:width$}", change.action.symbol(), change.name);
            if change.origin.is_empty() {
                writeln!(formatter, "{}", line.trim_end())?;
            } else {
                writeln!(formatter, "{line}  {}", change.origin)?;
            }
            if let Some(details) = change
                .details
                .as_ref()
                .filter(|details| !details.is_empty())
            {
                writeln!(formatter, "{:indent$}{details}", "", indent = width + 4)?;
            }
        }

        let counts = [
            Action::Added,
            Action::Updated,
            Action::Unchanged,
            Action::Removed,
        ]
        .into_iter()
        .filter_map(|action| match self.count(action) {
            0 => None,
            count => Some(format!("{count} {}", action.label())),
        })
        .collect::<Vec<_>>()
        .join(", ");

        let installed = self.changes.len() - self.count(Action::Removed);
        let suffix = if self.dry_run { " (dry run)" } else { "" };
        let detail = if counts.is_empty() {
            String::new()
        } else {
            format!(": {counts}")
        };
        let (one, many) = self.kind.nouns();
        let noun = if installed == 1 { one } else { many };
        writeln!(
            formatter,
            "{installed} {noun} in {}{detail}{suffix}",
            self.target.display()
        )
    }
}

/// What one update did, across every directory it visited.
#[derive(Debug)]
pub struct Report {
    /// One summary per directory visited, in the order they were resolved.
    pub summaries: Vec<Summary>,
}

impl Report {
    #[must_use]
    pub fn count(&self, action: Action) -> usize {
        self.summaries
            .iter()
            .map(|summary| summary.count(action))
            .sum()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, summary) in self.summaries.iter().enumerate() {
            if index > 0 {
                writeln!(formatter)?;
            }
            write!(formatter, "{summary}")?;
        }
        Ok(())
    }
}

/// # Errors
///
/// When the config cannot be loaded, or the update itself fails.
pub async fn run(
    options: &Options,
    dry_run: bool,
    force: bool,
    allow_hidden_characters: bool,
) -> Result<()> {
    print!(
        "{}",
        report(options, dry_run, force, allow_hidden_characters).await?
    );
    Ok(())
}

/// Load the config `options` name and update every directory it deploys into.
///
/// # Errors
///
/// When the config cannot be loaded, or the update itself fails.
pub async fn report(
    options: &Options,
    dry_run: bool,
    force: bool,
    allow_hidden_characters: bool,
) -> Result<Report> {
    let context = Context::open(options)?;
    let fetcher = context.fetcher(options);
    update(
        &context.config,
        &context.scope,
        &fetcher,
        dry_run,
        force,
        allow_hidden_characters,
    )
    .await
}

/// Bring every directory `config` deploys into in `scope` in line with it,
/// and prune the known client directories it owns but sends nothing to.
///
/// The sources are fetched and validated once, then applied to each
/// directory. A directory nothing is planned for is left alone unless it has
/// a state file; once pruned, its state file goes, and the directory too
/// when it is empty.
///
/// # Errors
///
/// When a source cannot be fetched, a selected skill or agent is invalid,
/// two sources send the same name to one directory, a directory would
/// receive both kinds or holds the other kind, or a directory holds an
/// unmanaged entry of that name and `force` is not set.
pub async fn update(
    config: &Config,
    scope: &Scope,
    materializer: &impl Materializer,
    dry_run: bool,
    force: bool,
    allow_hidden_characters: bool,
) -> Result<Report> {
    let plan = plan::build(config, scope, materializer, allow_hidden_characters).await?;
    if !plan.problems.is_empty() {
        for problem in &plan.problems {
            tracing::error!("{}", problem.message);
        }
        bail!("{}", plan::describe(&plan.problems));
    }

    let mut summaries = Vec::new();
    for (dir, kind) in directories(config, scope) {
        let target = plan.target_for(&dir);
        let entries = target.map_or(&[][..], |target| &target.entries);
        let kind = target.map_or(kind, |target| target.kind);
        if entries.is_empty() && !State::path(&dir).is_file() {
            tracing::debug!(target = %dir.display(), "nothing planned, nothing to prune");
            continue;
        }
        summaries.push(apply(entries, kind, &config.file, &dir, dry_run, force)?);
    }
    Ok(Report { summaries })
}

/// Deploy `entries`, which are of `kind`, into `target`, and prune what its
/// state records beyond them. With nothing to deploy, the state's own kind
/// says how its entries are laid out.
fn apply(
    entries: &[Planned],
    kind: Kind,
    config_file: &Path,
    target: &Path,
    dry_run: bool,
    force: bool,
) -> Result<Summary> {
    let mut state = State::read(target)?.unwrap_or_else(|| State::new(kind));
    if !entries.is_empty() {
        check_kind(&state, kind, target)?;
    }
    check_owner(&state, config_file, target, force)?;
    if !config_file.as_os_str().is_empty() {
        state.config = Some(config_file.to_path_buf());
    }
    if !dry_run {
        std::fs::create_dir_all(target)
            .with_context(|| format!("cannot create {}", target.display()))?;
        deploy::sweep_staging(target)?;
    }

    let mut changes = Vec::new();
    let reconciled = reconcile(entries, target, &mut state, &mut changes, dry_run, force);

    // Saved whatever happened: a skill installed before a failure is on disk,
    // and an unrecorded one would be refused as unmanaged by the next run.
    // When both fail, the reconcile error is the one returned, and the save
    // error is logged rather than lost.
    if !dry_run {
        let written = if state.entries.is_empty() && entries.is_empty() {
            forget(target)
        } else {
            state.save(target)
        };
        if let Err(error) = written {
            if reconciled.is_ok() {
                return Err(error);
            }
            tracing::error!("{error:#}");
        }
    }
    reconciled?;

    Ok(Summary {
        target: target.to_path_buf(),
        kind: state.kind,
        changes,
        dry_run,
    })
}

/// Remove the state file of a directory pollen deploys nothing into, then
/// the directory itself when nothing else is left in it.
fn forget(target: &Path) -> Result<()> {
    let path = State::path(target);
    if path.is_file() {
        tracing::info!(path = %path.display(), "removing");
        std::fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
    }
    let empty = std::fs::read_dir(target)
        .with_context(|| format!("cannot read {}", target.display()))?
        .next()
        .is_none();
    if empty {
        tracing::info!(path = %target.display(), "removing the empty directory");
        std::fs::remove_dir(target)
            .with_context(|| format!("cannot remove {}", target.display()))?;
    }
    Ok(())
}

/// Refuse to deploy one kind into a directory recorded as holding the other.
fn check_kind(state: &State, kind: Kind, target: &Path) -> Result<()> {
    if state.kind == kind {
        return Ok(());
    }
    bail!(
        "{} holds {} pollen deployed, so pollen does not deploy {} into it; choose another directory, or remove {} if pollen should forget what it deployed there",
        target.display(),
        state.kind.nouns().1,
        kind.nouns().1,
        State::path(target).display()
    )
}

/// Refuse a target another config still manages, unless `force` hands it over.
///
/// A recorded config that no longer exists is not an owner any more — the
/// project was moved or renamed — and a state written before configs were
/// recorded has none: both are adopted silently.
fn check_owner(state: &State, config_file: &Path, target: &Path, force: bool) -> Result<()> {
    let Some(owner) = &state.config else {
        return Ok(());
    };
    if force
        || config_file.as_os_str().is_empty()
        || owner == config_file
        || state.entries.is_empty()
        || !owner.is_file()
    {
        return Ok(());
    }
    bail!(
        "{} is managed by another config, {}; pollen keeps one config per target directory, since an update removes what its config does not declare. Declare these sources in that config, or pass --force to hand the directory over to {}",
        target.display(),
        owner.display(),
        config_file.display()
    )
}

/// Where `name` is installed in `target`, which holds `kind`: a skill
/// directory, or an agent file.
fn entry_path(target: &Path, kind: Kind, name: &str) -> PathBuf {
    match kind {
        Kind::Skills => target.join(name),
        Kind::Agents => target.join(format!("{name}{}", agent::SUFFIX)),
    }
}

/// Install every planned entry into `target`, then remove the ones the config
/// no longer selects, recording each step in `state` and `changes` as it lands.
fn reconcile(
    entries: &[Planned],
    target: &Path,
    state: &mut State,
    changes: &mut Vec<Change>,
    dry_run: bool,
    force: bool,
) -> Result<()> {
    for planned in entries {
        let action = install(target, state, planned, dry_run, force)?;
        changes.push(Change {
            action,
            name: planned.name.clone(),
            origin: planned.origin(),
            details: planned.details.clone(),
        });
    }

    let declared: BTreeSet<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    let orphans: Vec<String> = state
        .entries
        .keys()
        .filter(|name| !declared.contains(name.as_str()))
        .cloned()
        .collect();
    for name in orphans {
        let path = entry_path(target, state.kind, &name);
        if dry_run {
            tracing::info!(name = %name, path = %path.display(), "would remove");
        } else {
            tracing::info!(name = %name, path = %path.display(), "removing");
            match state.kind {
                Kind::Skills => deploy::remove(&path)?,
                Kind::Agents => deploy::remove_file(&path)?,
            }
        }

        state.entries.remove(&name);
        changes.push(Change {
            action: Action::Removed,
            name,
            origin: String::new(),
            details: None,
        });
    }
    Ok(())
}

fn install(
    target: &Path,
    state: &mut State,
    planned: &Planned,
    dry_run: bool,
    force: bool,
) -> Result<Action> {
    let _span = tracing::info_span!("target", path = %target.display()).entered();
    let path = entry_path(target, state.kind, &planned.name);
    let known = state.entries.get(&planned.name);

    if known.is_none() && path.exists() && !force {
        bail!(
            "{} already exists and pollen did not install it; move it aside, or pass --force to take it over",
            path.display()
        );
    }

    let installed = match state.kind {
        Kind::Skills if path.is_dir() => Some(deploy::checksum(&path)?),
        Kind::Agents if path.is_file() => Some(deploy::checksum_file(&path)?),
        _ => None,
    };
    let action = if known.is_some_and(|record| record.checksum == planned.checksum)
        && installed.as_ref() == Some(&planned.checksum)
    {
        Action::Unchanged
    } else if path.exists() {
        Action::Updated
    } else {
        Action::Added
    };

    if action == Action::Unchanged {
        tracing::debug!(name = %planned.name, "unchanged");
    } else if dry_run {
        tracing::info!(name = %planned.name, origin = %planned.origin(), action = action.label(), "would install");
    } else {
        tracing::info!(name = %planned.name, origin = %planned.origin(), action = action.label(), "installing");
        match state.kind {
            Kind::Skills => deploy::install(&planned.source, &path)?,
            Kind::Agents => deploy::install_file(&planned.source, &path)?,
        }
    }

    state.entries.insert(
        planned.name.clone(),
        InstalledSkill {
            repo: planned.repo.clone(),
            revision: planned.revision.clone(),
            commit: planned.commit.clone(),
            digest: planned.digest.clone(),
            source_path: planned.source_path.clone(),
            checksum: planned.checksum.clone(),
        },
    );
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Details;
    use crate::config::{AgentSelection, Client, Destination, PathSpec, Repo, Selection};
    use crate::source::{Materialized, MockMaterializer};
    use crate::state::State;

    fn write_skill(root: &Path, name: &str, description: &str) {
        let dir = root.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n"),
        )
        .unwrap();
    }

    fn config_with(repo_count: usize) -> Config {
        Config {
            version: 2,
            clients: None,
            repos: (0..repo_count)
                .map(|index| Repo::Git {
                    url: format!("https://example.invalid/{index}.git"),
                    revision: "1.0.0".to_owned(),
                    paths: vec![Selection {
                        spec: PathSpec {
                            path: PathBuf::from("skills"),
                            recurse: false,
                            exclude: None,
                        },
                        destination: Destination::Default,
                    }],
                    agents: Vec::new(),
                })
                .collect(),
            base_dir: PathBuf::new(),
            file: PathBuf::new(),
        }
    }

    fn materializer(root: &Path) -> MockMaterializer {
        let root = root.to_path_buf();
        let mut mock = MockMaterializer::new();
        mock.expect_materialize().returning(move |_| {
            let materialized = Materialized {
                root: root.clone(),
                resolved: Some("c0ffee".to_owned()),
                skills: Vec::new(),
            };
            Box::pin(async move { Ok(materialized) })
        });
        mock
    }

    /// A run with `--target` set to `dirs`.
    fn targets(dirs: &[PathBuf]) -> Scope {
        Scope {
            cli: Destination::Targets(dirs.to_vec()),
            root: PathBuf::new(),
            user_config: None,
        }
    }

    fn one(target: &Path) -> Scope {
        targets(&[target.to_path_buf()])
    }

    /// A run with neither `--target` nor `--client`, with the client
    /// directories under `root`.
    fn plain(root: &Path) -> Scope {
        Scope {
            cli: Destination::Default,
            root: root.to_path_buf(),
            user_config: None,
        }
    }

    fn actions(report: &Report) -> Vec<(Action, &str)> {
        report.summaries[0]
            .changes
            .iter()
            .map(|change| (change.action, change.name.as_str()))
            .collect()
    }

    #[tokio::test]
    async fn installs_a_skill_and_then_reports_it_unchanged() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_with(1);
        let mock = materializer(source.path());

        let first = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();
        let second = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(actions(&first), [(Action::Added, "demo")]);
        assert_eq!(actions(&second), [(Action::Unchanged, "demo")]);
        assert!(target.path().join("demo/SKILL.md").is_file());
        assert!(
            State::load(target.path())
                .unwrap()
                .entries
                .contains_key("demo")
        );
    }

    #[tokio::test]
    async fn reinstalls_when_the_source_moves_on() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_with(1);
        let mock = materializer(source.path());

        update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();
        write_skill(source.path(), "demo", "A better demo skill.");
        let summary = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(actions(&summary), [(Action::Updated, "demo")]);
        assert!(
            std::fs::read_to_string(target.path().join("demo/SKILL.md"))
                .unwrap()
                .contains("A better demo skill.")
        );
    }

    #[tokio::test]
    async fn reinstalls_when_the_installed_copy_was_edited_by_hand() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_with(1);
        let mock = materializer(source.path());

        update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();
        std::fs::write(target.path().join("demo/SKILL.md"), "tampered").unwrap();
        let summary = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(actions(&summary), [(Action::Updated, "demo")]);
    }

    #[tokio::test]
    async fn prunes_a_skill_the_config_no_longer_selects() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        write_skill(source.path(), "gone", "On its way out.");
        let config = config_with(1);
        let mock = materializer(source.path());

        update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();
        std::fs::remove_dir_all(source.path().join("skills/gone")).unwrap();
        let summary = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(
            actions(&summary),
            [(Action::Unchanged, "demo"), (Action::Removed, "gone")]
        );
        assert!(!target.path().join("gone").exists());
        assert!(target.path().join("demo").exists());
    }

    #[tokio::test]
    async fn refuses_to_take_over_a_directory_it_does_not_own() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        std::fs::create_dir_all(target.path().join("demo")).unwrap();
        std::fs::write(target.path().join("demo/SKILL.md"), "handwritten").unwrap();
        let config = config_with(1);
        let mock = materializer(source.path());

        let error = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("--force"), "{error}");
        assert_eq!(
            std::fs::read_to_string(target.path().join("demo/SKILL.md")).unwrap(),
            "handwritten"
        );
    }

    #[tokio::test]
    async fn refuses_unsafe_state_before_installing_pruning_or_sweeping() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let outside = root.path().join("outside");
        write_skill(source.path(), "alpha", "Ordinary declared skill.");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let sentinel = outside.join("retained.txt");
        std::fs::write(&sentinel, "preserve outside bytes").unwrap();
        let leftovers: Vec<_> = [deploy::STAGING_PREFIX, deploy::REPLACED_PREFIX]
            .into_iter()
            .map(|prefix| {
                let directory = target.join(format!("{prefix}retained"));
                std::fs::create_dir(&directory).unwrap();
                let sentinel = directory.join("retained.txt");
                std::fs::write(&sentinel, "preserve interrupted update bytes").unwrap();
                sentinel
            })
            .collect();
        let original_state = serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "skills": {
                "../outside": {
                    "repo": "local",
                    "revision": null,
                    "commit": null,
                    "sourcePath": "skills/retired",
                    "checksum": "sha256:00"
                }
            }
        }))
        .unwrap();
        std::fs::write(State::path(&target), &original_state).unwrap();
        let config = config_with(1);
        let mock = materializer(source.path());

        for dry_run in [false, true] {
            for force in [false, true] {
                let result = update(&config, &one(&target), &mock, dry_run, force, false).await;

                assert!(result.is_err(), "unsafe state must refuse before mutation");
                assert_eq!(
                    std::fs::read_to_string(&sentinel).unwrap(),
                    "preserve outside bytes"
                );
                assert_eq!(std::fs::read(State::path(&target)).unwrap(), original_state);
                assert!(!target.join("alpha").exists());
                for leftover in &leftovers {
                    assert_eq!(
                        std::fs::read_to_string(leftover).unwrap(),
                        "preserve interrupted update bytes"
                    );
                }
            }
        }
    }

    /// A config read from `name`, a real file in `dir`.
    fn config_owned_by(dir: &Path, name: &str) -> Config {
        let file = dir.join(name);
        std::fs::write(&file, "").unwrap();
        Config {
            file: file.canonicalize().unwrap(),
            ..config_with(1)
        }
    }

    #[tokio::test]
    async fn records_the_config_that_owns_the_target() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_owned_by(configs.path(), "a.yaml");
        let mock = materializer(source.path());

        update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(
            State::load(target.path()).unwrap().config,
            Some(config.file)
        );
    }

    #[tokio::test]
    async fn refuses_a_target_another_config_manages() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let first = config_owned_by(configs.path(), "a.yaml");
        let second = config_owned_by(configs.path(), "b.yaml");
        let mock = materializer(source.path());

        update(&first, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();
        let error = update(&second, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("managed by another config"), "{error}");
        assert_eq!(State::load(target.path()).unwrap().config, Some(first.file));
    }

    #[tokio::test]
    async fn hands_a_target_over_to_another_config_when_forced() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let first = config_owned_by(configs.path(), "a.yaml");
        let second = config_owned_by(configs.path(), "b.yaml");
        let mock = materializer(source.path());

        update(&first, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();
        update(&second, &one(target.path()), &mock, false, true, false)
            .await
            .unwrap();

        assert_eq!(
            State::load(target.path()).unwrap().config,
            Some(second.file)
        );
    }

    #[tokio::test]
    async fn adopts_a_target_whose_config_no_longer_exists() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let moved = config_owned_by(configs.path(), "old.yaml");
        let current = config_owned_by(configs.path(), "new.yaml");
        let mock = materializer(source.path());

        update(&moved, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();
        std::fs::remove_file(&moved.file).unwrap();
        let summary = update(&current, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(actions(&summary), [(Action::Unchanged, "demo")]);
        assert_eq!(
            State::load(target.path()).unwrap().config,
            Some(current.file)
        );
    }

    #[tokio::test]
    async fn adopts_a_state_written_before_owners_were_recorded() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_owned_by(configs.path(), "a.yaml");
        let mock = materializer(source.path());
        update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();
        let mut legacy = State::load(target.path()).unwrap();
        legacy.config = None;
        legacy.save(target.path()).unwrap();

        let summary = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(actions(&summary), [(Action::Unchanged, "demo")]);
        assert_eq!(
            State::load(target.path()).unwrap().config,
            Some(config.file)
        );
    }

    #[tokio::test]
    async fn a_failure_part_way_keeps_what_was_installed_recorded() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "alpha", "Installed before the failure.");
        write_skill(source.path(), "beta", "Blocked by a handwritten directory.");
        std::fs::create_dir_all(target.path().join("beta")).unwrap();
        std::fs::write(target.path().join("beta/SKILL.md"), "handwritten").unwrap();
        let config = config_with(1);
        let mock = materializer(source.path());

        update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap_err();
        let recorded = State::load(target.path()).unwrap();
        std::fs::remove_dir_all(target.path().join("beta")).unwrap();
        let retry = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert!(recorded.entries.contains_key("alpha"));
        assert!(!recorded.entries.contains_key("beta"));
        assert_eq!(
            actions(&retry),
            [(Action::Unchanged, "alpha"), (Action::Added, "beta")]
        );
    }

    #[tokio::test]
    async fn takes_over_an_unmanaged_directory_when_forced() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        std::fs::create_dir_all(target.path().join("demo")).unwrap();
        std::fs::write(target.path().join("demo/SKILL.md"), "handwritten").unwrap();
        let config = config_with(1);
        let mock = materializer(source.path());

        let summary = update(&config, &one(target.path()), &mock, false, true, false)
            .await
            .unwrap();

        assert_eq!(actions(&summary), [(Action::Updated, "demo")]);
        assert!(
            std::fs::read_to_string(target.path().join("demo/SKILL.md"))
                .unwrap()
                .contains("A demo skill.")
        );
    }

    #[tokio::test]
    async fn a_dry_run_reports_without_writing() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_with(1);
        let mock = materializer(source.path());

        let summary = update(&config, &one(target.path()), &mock, true, false, false)
            .await
            .unwrap();

        assert_eq!(actions(&summary), [(Action::Added, "demo")]);
        assert!(summary.summaries[0].dry_run);
        assert!(!target.path().join("demo").exists());
        assert!(!State::path(target.path()).exists());
    }

    #[tokio::test]
    async fn refuses_two_sources_providing_the_same_skill() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_with(2);
        let mock = materializer(source.path());

        let error = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("two sources provide the skill `demo` in"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn refuses_a_skill_that_breaks_the_specification() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let dir = source.path().join("skills/Bad-Name");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: Bad-Name\ndescription: x\n---\n",
        )
        .unwrap();
        let config = config_with(1);
        let mock = materializer(source.path());

        let error = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("skill(s) are invalid"), "{error}");
        assert!(!target.path().join("Bad-Name").exists());
    }

    #[tokio::test]
    async fn refuses_a_skill_hiding_invisible_characters() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let dir = source.path().join("skills/demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: demo\ndescription: x\n---\n\u{e0049}\u{e0067}\u{e006e}\n",
        )
        .unwrap();
        let config = config_with(1);
        let mock = materializer(source.path());

        let error = update(&config, &one(target.path()), &mock, false, false, false)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("skill(s) are invalid"), "{error}");
        assert!(!target.path().join("demo").exists());

        update(&config, &one(target.path()), &mock, false, false, true)
            .await
            .unwrap();
        assert!(target.path().join("demo/SKILL.md").is_file());
    }

    #[tokio::test]
    async fn deploys_the_same_skill_into_every_target() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let claude = root.path().join(".claude/skills");
        let agents = root.path().join(".agents/skills");
        let config = config_with(1);
        let mock = materializer(source.path());

        let report = update(
            &config,
            &targets(&[claude.clone(), agents.clone()]),
            &mock,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        assert_eq!(report.summaries.len(), 2);
        assert_eq!(report.count(Action::Added), 2);
        assert!(claude.join("demo/SKILL.md").is_file());
        assert!(agents.join("demo/SKILL.md").is_file());
        assert!(State::load(&claude).unwrap().entries.contains_key("demo"));
        assert!(State::load(&agents).unwrap().entries.contains_key("demo"));
    }

    #[tokio::test]
    async fn a_second_target_added_later_catches_up_on_its_own() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let claude = root.path().join(".claude/skills");
        let agents = root.path().join(".agents/skills");
        let config = config_with(1);
        let mock = materializer(source.path());

        update(&config, &one(&claude), &mock, false, false, false)
            .await
            .unwrap();
        let report = update(
            &config,
            &targets(&[claude, agents]),
            &mock,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        assert_eq!(report.count(Action::Unchanged), 1);
        assert_eq!(report.count(Action::Added), 1);
    }

    #[tokio::test]
    async fn deploys_into_the_skill_directories_of_every_client_by_default() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_owned_by(configs.path(), "a.yaml");
        let mock = materializer(source.path());

        let report = update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        let visited: Vec<_> = report.summaries.iter().map(|s| s.target.clone()).collect();
        assert_eq!(
            visited,
            [
                root.path().join(".claude/skills"),
                root.path().join(".agents/skills")
            ]
        );
        assert!(root.path().join(".agents/skills/demo/SKILL.md").is_file());
    }

    /// `config` sending its one entry to `destination`.
    fn sending(config: Config, destination: Destination) -> Config {
        let mut config = config;
        let Repo::Git { paths, .. } = &mut config.repos[0] else {
            unreachable!()
        };
        paths[0].destination = destination;
        config
    }

    #[tokio::test]
    async fn prunes_a_client_directory_the_config_no_longer_sends_anything_to() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let both = config_owned_by(configs.path(), "a.yaml");
        let claude_only = Config {
            clients: Some(vec![Client::ClaudeCode]),
            ..both.clone()
        };
        let mock = materializer(source.path());
        let agents = root.path().join(".agents/skills");
        update(&both, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        let dry = update(&claude_only, &plain(root.path()), &mock, true, false, false)
            .await
            .unwrap();
        assert_eq!(dry.summaries[1].target, agents);
        assert!(State::path(&agents).is_file());
        let report = update(
            &claude_only,
            &plain(root.path()),
            &mock,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        assert_eq!(report.summaries.len(), 2);
        assert_eq!(report.summaries[1].target, agents);
        assert_eq!(
            report.summaries[1].changes,
            [Change {
                action: Action::Removed,
                name: "demo".to_owned(),
                origin: String::new(),
                details: None,
            }]
        );
        assert!(!agents.exists());
        assert!(root.path().join(".claude/skills/demo").is_dir());
        let again = update(
            &claude_only,
            &plain(root.path()),
            &mock,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(again.summaries.len(), 1);
    }

    #[tokio::test]
    async fn keeps_a_pruned_directory_that_holds_something_else() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_owned_by(configs.path(), "a.yaml");
        let mock = materializer(source.path());
        let claude = root.path().join(".claude/skills");
        update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();
        std::fs::create_dir_all(claude.join("handwritten")).unwrap();

        let moved = sending(config, Destination::Clients(vec![Client::OpenCode]));
        update(&moved, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert!(!claude.join("demo").exists());
        assert!(!State::path(&claude).exists());
        assert!(claude.join("handwritten").is_dir());
    }

    #[tokio::test]
    async fn moving_an_entry_to_targets_prunes_the_client_directories() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_owned_by(configs.path(), "a.yaml");
        let mock = materializer(source.path());
        update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        let out = root.path().join("out");
        let moved = sending(config, Destination::Targets(vec![out.clone()]));
        let report = update(&moved, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(report.summaries.len(), 3);
        assert_eq!(report.count(Action::Added), 1);
        assert_eq!(report.count(Action::Removed), 2);
        assert!(out.join("demo").is_dir());
        assert!(!root.path().join(".claude/skills").exists());
        assert!(!root.path().join(".agents/skills").exists());
    }

    #[tokio::test]
    async fn leaves_a_client_directory_another_config_owns_alone() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let other = config_owned_by(configs.path(), "b.yaml");
        let mine = sending(
            config_owned_by(configs.path(), "a.yaml"),
            Destination::Clients(vec![Client::ClaudeCode]),
        );
        let mock = materializer(source.path());
        let agents = root.path().join(".agents/skills");
        update(
            &other,
            &targets(std::slice::from_ref(&agents)),
            &mock,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let report = update(&mine, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(report.summaries.len(), 1);
        assert!(agents.join("demo").is_dir());
    }

    #[tokio::test]
    async fn an_overridden_run_visits_only_the_directories_it_names() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let config = config_owned_by(configs.path(), "a.yaml");
        let mock = materializer(source.path());
        update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();
        let elsewhere = root.path().join("elsewhere");
        let claude_only = Scope {
            cli: Destination::Clients(vec![Client::ClaudeCode]),
            root: root.path().to_path_buf(),
            user_config: None,
        };

        for scope in [one(&elsewhere), claude_only] {
            let report = update(&config, &scope, &mock, false, false, false)
                .await
                .unwrap();

            assert_eq!(report.summaries.len(), 1);
            assert_eq!(report.count(Action::Removed), 0);
        }
        assert!(root.path().join(".agents/skills/demo").is_dir());
    }

    #[tokio::test]
    async fn one_name_may_go_to_two_directories_from_two_entries() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let mut config = config_with(2);
        for (repo, client) in config.repos.iter_mut().zip(Client::ALL) {
            let Repo::Git { paths, .. } = repo else {
                unreachable!()
            };
            paths[0].destination = Destination::Clients(vec![client]);
        }
        let mock = materializer(source.path());

        let report = update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(report.count(Action::Added), 2);
        let claude = State::load(&root.path().join(".claude/skills")).unwrap();
        let agents = State::load(&root.path().join(".agents/skills")).unwrap();
        assert_eq!(claude.entries["demo"].repo, "https://example.invalid/0.git");
        assert_eq!(agents.entries["demo"].repo, "https://example.invalid/1.git");
    }

    #[tokio::test]
    async fn two_spellings_of_one_directory_share_one_plan() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        let mut config = config_with(2);
        let Repo::Git { paths, .. } = &mut config.repos[0] else {
            unreachable!()
        };
        paths[0].destination = Destination::Targets(vec![root.path().join("./.claude/skills")]);
        config.clients = Some(vec![Client::ClaudeCode]);
        let mock = materializer(source.path());

        let error = update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("two sources provide the skill `demo` in"),
            "{error}"
        );
    }

    fn write_agent(root: &Path, dir: &str, file: &str, text: &str) {
        let dir = root.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), text).unwrap();
    }

    /// `config` with a `local` source taking skills under `skills` for its
    /// default clients, and agents under `agents`, sent to `destination`.
    fn with_agents(config: Config, destination: Destination) -> Config {
        Config {
            repos: vec![Repo::Local {
                paths: vec![Selection {
                    spec: PathSpec {
                        path: PathBuf::from("skills"),
                        recurse: false,
                        exclude: None,
                    },
                    destination: Destination::Default,
                }],
                agents: vec![AgentSelection {
                    path: PathBuf::from("agents"),
                    exclude: None,
                    destination,
                }],
            }],
            ..config
        }
    }

    const REVIEWER: &str = "---\nname: reviewer\ndescription: Reviews.\ntools: Read\n---\n";

    #[tokio::test]
    async fn deploys_one_name_as_a_skill_and_as_an_agent_of_each_client() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_skill(source.path(), "reviewer", "A skill.");
        write_agent(source.path(), "agents", "reviewer.md", REVIEWER);
        let config = with_agents(config_with(0), Destination::Default);
        let mock = materializer(source.path());

        let first = update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();
        let second = update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(first.count(Action::Added), 4);
        assert_eq!(second.count(Action::Unchanged), 4);
        assert!(
            root.path()
                .join(".claude/skills/reviewer/SKILL.md")
                .is_file()
        );
        for dir in [".claude/agents", ".opencode/agents"] {
            let agents = root.path().join(dir);
            assert_eq!(
                std::fs::read_to_string(agents.join("reviewer.md")).unwrap(),
                REVIEWER
            );
            let state = State::load(&agents).unwrap();
            assert_eq!(state.kind, Kind::Agents);
            assert_eq!(state.entries["reviewer"].source_path, "agents/reviewer.md");
        }
        let agents = &first.summaries[2];
        assert_eq!(agents.kind, Kind::Agents);
        assert_eq!(
            agents.changes[0].details.as_ref().unwrap().tools.as_deref(),
            Some("Read")
        );
    }

    #[tokio::test]
    async fn checks_an_agent_against_every_client_it_goes_to_and_says_which_refused() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("skills")).unwrap();
        write_agent(
            source.path(),
            "agents",
            "helper.md",
            "---\ndescription: Helps.\n---\n",
        );
        let mock = materializer(source.path());

        let both = with_agents(config_with(0), Destination::Default);
        let error = update(&both, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "1 agent(s) are invalid");
        assert!(!root.path().join(".opencode").exists());

        let opencode = with_agents(config_with(0), Destination::Clients(vec![Client::OpenCode]));
        update(&opencode, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();
        assert!(root.path().join(".opencode/agents/helper.md").is_file());
    }

    #[tokio::test]
    async fn an_agent_sent_to_targets_passes_only_the_shared_checks() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("skills")).unwrap();
        write_agent(source.path(), "agents", "bare.md", "---\nmodel: x\n---\n");
        let out = root.path().join("out");
        let config = with_agents(config_with(0), Destination::Targets(vec![out.clone()]));

        update(
            &config,
            &plain(root.path()),
            &materializer(source.path()),
            false,
            false,
            false,
        )
        .await
        .unwrap();

        assert!(out.join("bare.md").is_file());
        assert_eq!(State::load(&out).unwrap().kind, Kind::Agents);
    }

    #[tokio::test]
    async fn reinstalls_an_agent_edited_by_hand_and_prunes_one_no_longer_declared() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("skills")).unwrap();
        for name in ["kept", "gone"] {
            write_agent(
                source.path(),
                "agents",
                &format!("{name}.md"),
                "---\ndescription: x\n---\n",
            );
        }
        let agents = root.path().join(".opencode/agents");
        let config = with_agents(config_with(0), Destination::Clients(vec![Client::OpenCode]));
        let mock = materializer(source.path());

        update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();
        std::fs::write(agents.join("kept.md"), "tampered").unwrap();
        std::fs::remove_file(source.path().join("agents/gone.md")).unwrap();
        let report = update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        assert_eq!(
            actions(&report),
            [(Action::Updated, "kept"), (Action::Removed, "gone")]
        );
        assert!(!agents.join("gone.md").exists());
        assert_eq!(
            std::fs::read_to_string(agents.join("kept.md")).unwrap(),
            "---\ndescription: x\n---\n"
        );
    }

    #[tokio::test]
    async fn refuses_an_agent_file_it_did_not_install_unless_forced() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("skills")).unwrap();
        write_agent(
            source.path(),
            "agents",
            "mine.md",
            "---\ndescription: x\n---\n",
        );
        write_agent(root.path(), ".opencode/agents", "mine.md", "handwritten");
        let config = with_agents(config_with(0), Destination::Clients(vec![Client::OpenCode]));
        let mock = materializer(source.path());

        let error = update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("--force"), "{error}");
        update(&config, &plain(root.path()), &mock, false, true, false)
            .await
            .unwrap();

        assert_ne!(
            std::fs::read_to_string(root.path().join(".opencode/agents/mine.md")).unwrap(),
            "handwritten"
        );
    }

    #[tokio::test]
    async fn a_cli_target_deploys_skills_only() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        write_agent(source.path(), "agents", "reviewer.md", REVIEWER);
        let out = root.path().join("out");
        let config = with_agents(config_with(0), Destination::Default);

        let report = update(
            &config,
            &one(&out),
            &materializer(source.path()),
            false,
            false,
            false,
        )
        .await
        .unwrap();

        assert_eq!(actions(&report), [(Action::Added, "demo")]);
        assert!(!out.join("reviewer.md").exists());
        assert!(!root.path().join(".claude").exists());
    }

    #[tokio::test]
    async fn refuses_to_deploy_one_kind_where_the_state_records_the_other() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        write_agent(source.path(), "agents", "reviewer.md", REVIEWER);
        let agents = root.path().join(".claude/agents");
        let config = with_agents(
            config_with(0),
            Destination::Clients(vec![Client::ClaudeCode]),
        );
        let mock = materializer(source.path());
        update(&config, &plain(root.path()), &mock, false, false, false)
            .await
            .unwrap();

        for force in [false, true] {
            let error = update(&config, &one(&agents), &mock, false, force, false)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("holds agents pollen deployed"), "{error}");
        }
        let skills = root.path().join("skills-out");
        update(&config, &one(&skills), &mock, false, false, false)
            .await
            .unwrap();
        let into_skills = with_agents(config_with(0), Destination::Targets(vec![skills]));
        let error = update(
            &into_skills,
            &plain(root.path()),
            &mock,
            false,
            false,
            false,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("holds skills pollen deployed"), "{error}");
    }

    #[tokio::test]
    async fn refuses_one_directory_for_skills_and_agents() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        write_agent(source.path(), "agents", "reviewer.md", REVIEWER);
        let shared = root.path().join("shared");
        let mut config = with_agents(config_with(0), Destination::Targets(vec![shared.clone()]));
        if let Repo::Local { paths, .. } = &mut config.repos[0] {
            paths[0].destination = Destination::Targets(vec![shared.clone()]);
        }

        let error = update(
            &config,
            &plain(root.path()),
            &materializer(source.path()),
            false,
            false,
            false,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("both skills and agents"), "{error}");
        assert!(!shared.exists());
    }

    #[tokio::test]
    async fn refuses_two_sources_sending_one_agent_to_one_directory() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("skills")).unwrap();
        write_agent(
            source.path(),
            "agents",
            "twice.md",
            "---\ndescription: x\n---\n",
        );
        let mut config = with_agents(config_with(0), Destination::Clients(vec![Client::OpenCode]));
        config.repos.push(config.repos[0].clone());

        let error = update(
            &config,
            &plain(root.path()),
            &materializer(source.path()),
            false,
            false,
            false,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("two sources provide the agent `twice`"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn prunes_the_agent_directory_of_a_client_the_config_dropped() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");
        write_agent(source.path(), "agents", "reviewer.md", REVIEWER);
        let owned = config_owned_by(configs.path(), "a.yaml");
        let mock = materializer(source.path());
        update(
            &with_agents(owned.clone(), Destination::Default),
            &plain(root.path()),
            &mock,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let claude_only = with_agents(owned, Destination::Clients(vec![Client::ClaudeCode]));
        let report = update(
            &claude_only,
            &plain(root.path()),
            &mock,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let opencode = root.path().join(".opencode/agents");
        let pruned = report
            .summaries
            .iter()
            .find(|summary| summary.target == opencode)
            .unwrap();
        assert_eq!(pruned.kind, Kind::Agents);
        assert_eq!(pruned.count(Action::Removed), 1);
        assert!(!opencode.exists());
        assert!(root.path().join(".claude/agents/reviewer.md").is_file());
    }

    #[tokio::test]
    async fn a_config_without_agents_creates_no_agent_directory() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let configs = tempfile::tempdir().unwrap();
        write_skill(source.path(), "demo", "A demo skill.");

        update(
            &config_owned_by(configs.path(), "a.yaml"),
            &plain(root.path()),
            &materializer(source.path()),
            false,
            false,
            false,
        )
        .await
        .unwrap();

        assert!(root.path().join(".claude/skills/demo").is_dir());
        assert!(!root.path().join(".claude/agents").exists());
        assert!(!root.path().join(".opencode").exists());
    }

    #[test]
    fn the_summary_shows_what_each_agent_grants() {
        let summary = Summary {
            target: PathBuf::from(".claude/agents"),
            kind: Kind::Agents,
            changes: vec![Change {
                action: Action::Added,
                name: "reviewer".to_owned(),
                origin: "local agents/reviewer.md".to_owned(),
                details: Some(Details {
                    tools: Some("Read, Grep".to_owned()),
                    model: Some("opus".to_owned()),
                    permission: None,
                    hooks: false,
                    mcp_servers: true,
                }),
            }],
            dry_run: false,
        };

        assert_eq!(
            summary.to_string(),
            "+ reviewer  local agents/reviewer.md\n\
             \x20           tools: Read, Grep; model: opus; MCP servers\n\
             1 agent in .claude/agents: 1 added\n"
        );
    }

    #[test]
    fn the_summary_counts_every_action() {
        let summary = Summary {
            target: PathBuf::from(".claude/skills"),
            kind: Kind::Skills,
            changes: vec![
                Change {
                    action: Action::Added,
                    name: "alpha".to_owned(),
                    origin: "local skills/alpha".to_owned(),
                    details: None,
                },
                Change {
                    action: Action::Removed,
                    name: "beta".to_owned(),
                    origin: String::new(),
                    details: None,
                },
            ],
            dry_run: false,
        };

        let rendered = summary.to_string();

        assert!(rendered.contains("+ alpha"), "{rendered}");
        assert!(rendered.contains("- beta"), "{rendered}");
        assert!(
            rendered.contains("1 skill in .claude/skills: 1 added, 1 removed"),
            "{rendered}"
        );
    }
}
