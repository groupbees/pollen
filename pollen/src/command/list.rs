use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;

use crate::cli::Options;
use crate::command::Context;
use crate::state::{InstalledSkill, Kind, State};

/// The skills or agents pollen manages in one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployed {
    pub target: PathBuf,
    /// What the directory holds: what its state file records, else what
    /// the run would deploy into it.
    pub kind: Kind,
    /// Its managed entries, keyed by the name they are deployed under.
    pub entries: BTreeMap<String, InstalledSkill>,
}

/// # Errors
///
/// When the config or a directory's state cannot be read.
pub fn run(options: &Options) -> Result<()> {
    for (index, deployed) in installed(options)?.iter().enumerate() {
        if index > 0 {
            println!();
        }
        match deployed.kind {
            Kind::Skills => println!("{}:", deployed.target.display()),
            Kind::Agents => println!("{} (agents):", deployed.target.display()),
        }

        let width = deployed.entries.keys().map(String::len).max().unwrap_or(0);
        for (name, entry) in &deployed.entries {
            println!("  {name:width$}  {}  {}", origin(entry), entry.source_path);
        }
    }
    Ok(())
}

/// Read what pollen installed in every directory an update with `options`
/// visits: those the config's entries deploy skills or agents into, and the
/// known client directories whose state file names the config.
///
/// # Errors
///
/// When the config or a directory's state cannot be read.
pub fn installed(options: &Options) -> Result<Vec<Deployed>> {
    let context = Context::open(options)?;
    context
        .directories()
        .into_iter()
        .map(|(target, kind)| {
            let state = State::read(&target)?.unwrap_or_else(|| State::new(kind));
            tracing::debug!(target = %target.display(), count = state.entries.len(), "installed");
            Ok(Deployed {
                target,
                kind: state.kind,
                entries: state.entries,
            })
        })
        .collect()
}

fn origin(entry: &InstalledSkill) -> String {
    match (&entry.revision, &entry.commit) {
        (Some(revision), Some(commit)) => format!(
            "{}@{revision} ({})",
            entry.repo,
            &commit[..commit.len().min(8)]
        ),
        (Some(revision), None) => format!("{}@{revision}", entry.repo),
        _ => match &entry.digest {
            Some(digest) => format!("{} ({})", entry.repo, &digest[..digest.len().min(15)]),
            None => entry.repo.clone(),
        },
    }
}
