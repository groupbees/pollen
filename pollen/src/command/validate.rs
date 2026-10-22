//! `pollen validate`: check one or more config files, and optionally the
//! skills their sources yield.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::cli::Cli;
use crate::command::Context;
use crate::command::plan;

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
pub async fn run(cli: &Cli, configs: &[PathBuf], config_only: bool, pinned: bool) -> Result<()> {
    let paths: Vec<PathBuf> = if configs.is_empty() {
        vec![cli.config_path()]
    } else {
        configs.to_vec()
    };

    if let [only] = paths.as_slice() {
        return check(cli, only, config_only, pinned).await;
    }

    let mut failures = 0_usize;
    for path in &paths {
        if let Err(error) = check(cli, path, config_only, pinned).await {
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

async fn check(cli: &Cli, path: &Path, config_only: bool, pinned: bool) -> Result<()> {
    let context = Context::open_at(cli, path)?;

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

    if config_only {
        println!(
            "{}: ok, {} source(s)",
            path.display(),
            context.config.repos.len()
        );
        return Ok(());
    }

    let plan = plan::build(&context.config, &context.fetcher(cli))
        .await
        .with_context(|| path.display().to_string())?;
    println!("{}:", path.display());
    for skill in &plan.skills {
        println!("  {}  {}", skill.name, skill.origin());
    }
    for problem in &plan.problems {
        tracing::error!("{problem}");
    }

    if plan.problems.is_empty() {
        tracing::debug!(count = plan.skills.len(), "every selected skill is valid");
        Ok(())
    } else {
        bail!(
            "{}: {} skill(s) do not satisfy the Agent Skills specification",
            path.display(),
            plan.problems.len()
        )
    }
}
