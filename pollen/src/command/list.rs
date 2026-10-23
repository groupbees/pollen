//! `pollen list`: what is deployed in the target directories.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;

use crate::cli::Options;
use crate::command::Context;
use crate::state::{InstalledSkill, State};

/// The skills pollen manages in one target directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployed {
    /// The target directory.
    pub target: PathBuf,
    /// Its managed skills, keyed by the name they are deployed under.
    pub skills: BTreeMap<String, InstalledSkill>,
}

/// Run the subcommand.
///
/// # Errors
///
/// When the config or a target directory's state cannot be read.
pub fn run(options: &Options) -> Result<()> {
    for (index, deployed) in installed(options)?.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("{}:", deployed.target.display());

        let width = deployed.skills.keys().map(String::len).max().unwrap_or(0);
        for (name, skill) in &deployed.skills {
            println!("  {name:width$}  {}  {}", origin(skill), skill.source_path);
        }
    }
    Ok(())
}

/// Read what pollen installed in every target directory `options` resolve to.
///
/// # Errors
///
/// When the config or a target directory's state cannot be read.
pub fn installed(options: &Options) -> Result<Vec<Deployed>> {
    let context = Context::open(options)?;
    context
        .targets
        .into_iter()
        .map(|target| {
            let state = State::load(&target)?;
            tracing::debug!(target = %target.display(), count = state.skills.len(), "installed skills");
            Ok(Deployed {
                target,
                skills: state.skills,
            })
        })
        .collect()
}

fn origin(skill: &InstalledSkill) -> String {
    match (&skill.revision, &skill.commit) {
        (Some(revision), Some(commit)) => format!(
            "{}@{revision} ({})",
            skill.repo,
            &commit[..commit.len().min(8)]
        ),
        (Some(revision), None) => format!("{}@{revision}", skill.repo),
        _ => skill.repo.clone(),
    }
}
