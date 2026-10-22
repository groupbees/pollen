//! `pollen validate`: check one or more config files, and optionally the
//! skills their sources yield.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::cli::Options;
use crate::command::Context;
use crate::command::plan::{self, Planned};

/// What checking one config file found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    /// The config file.
    pub path: PathBuf,
    /// How many sources it declares.
    pub sources: usize,
    /// The skills its sources yield, `None` when only the file was checked.
    pub skills: Option<Vec<Planned>>,
    /// The selected skills that break the Agent Skills specification, one
    /// message each.
    pub problems: Vec<String>,
}

/// Run the subcommand.
///
/// With several files, every one is checked before the run fails, so a single
/// invocation reports every problem rather than only the first. With one, its
/// error is returned as it is rather than wrapped in a count.
///
/// # Errors
///
/// When a config file is invalid, a source cannot be fetched, or a selected
/// skill breaks the Agent Skills specification.
pub async fn run(
    options: &Options,
    configs: &[PathBuf],
    config_only: bool,
    pinned: bool,
) -> Result<()> {
    let paths: Vec<PathBuf> = if configs.is_empty() {
        vec![options.config_path()]
    } else {
        configs.to_vec()
    };

    if let [only] = paths.as_slice() {
        return report(&check(options, only, config_only, pinned).await?);
    }

    let mut failures = 0_usize;
    for path in &paths {
        let outcome = check(options, path, config_only, pinned).await;
        if let Err(error) = outcome.and_then(|validation| report(&validation)) {
            tracing::error!("{error:#}");
            failures += 1;
        }
    }

    if failures == 0 {
        Ok(())
    } else {
        bail!("{failures} of {} config file(s) are invalid", paths.len())
    }
}

/// Check the config file at `path`, and unless `config_only` every skill its
/// sources select.
///
/// A skill that breaks the specification lands in `problems` rather than
/// failing the check, so every one of them is reported.
///
/// # Errors
///
/// When the file is invalid, `pinned` is set and a git source is not pinned
/// to a commit, or a source cannot be fetched.
pub async fn check(
    options: &Options,
    path: &Path,
    config_only: bool,
    pinned: bool,
) -> Result<Validation> {
    let context = Context::open_at(options, path)?;

    if pinned {
        let loose = context.config.unpinned();
        if !loose.is_empty() {
            let list = loose
                .iter()
                .map(|repo| repo.label())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "{}: {} source(s) not pinned to a commit: {list}. Pin each `revision` to a full commit SHA; keep the tag in a comment: `revision: <sha>  # v1.2.3`",
                path.display(),
                loose.len()
            );
        }
    }

    let sources = context.config.repos.len();
    if config_only {
        return Ok(Validation {
            path: path.to_owned(),
            sources,
            skills: None,
            problems: Vec::new(),
        });
    }

    let plan = plan::build(&context.config, &context.fetcher(options))
        .await
        .with_context(|| path.display().to_string())?;
    Ok(Validation {
        path: path.to_owned(),
        sources,
        skills: Some(plan.skills),
        problems: plan.problems,
    })
}

fn report(validation: &Validation) -> Result<()> {
    let path = validation.path.display();
    let Some(skills) = &validation.skills else {
        println!("{path}: ok, {} source(s)", validation.sources);
        return Ok(());
    };

    println!("{path}:");
    for skill in skills {
        println!("  {}  {}", skill.name, skill.origin());
    }
    for problem in &validation.problems {
        tracing::error!("{problem}");
    }

    if validation.problems.is_empty() {
        tracing::debug!(count = skills.len(), "every selected skill is valid");
        Ok(())
    } else {
        bail!(
            "{path}: {} skill(s) do not satisfy the Agent Skills specification",
            validation.problems.len()
        )
    }
}
