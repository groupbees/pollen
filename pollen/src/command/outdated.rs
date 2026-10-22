//! `pollen outdated`: which git sources have a newer release than the one
//! they are pinned to.

use std::fmt;

use anyhow::{Result, bail};

use crate::cli::Cli;
use crate::command::Context;
use crate::config::{Config, Source, is_commit_id};
use crate::source::git;
use crate::version::{self, RemoteTag};

/// Where one git source stands against its remote's releases.
#[derive(Debug, Clone)]
pub struct Status {
    /// The `repo:` value.
    pub repo: String,
    /// The `revision:` value.
    pub revision: String,
    /// The release the revision stands for, when it is one.
    pub current: Option<RemoteTag>,
    /// The newest release on the remote.
    pub latest: Option<RemoteTag>,
}

impl Status {
    /// Whether a newer release exists than the one pinned.
    #[must_use]
    pub fn is_outdated(&self) -> bool {
        match (&self.current, &self.latest) {
            (Some(current), Some(latest)) => {
                version::Version::parse(&latest.name) > version::Version::parse(&current.name)
            }
            (None, Some(_)) => is_commit_id(&self.revision),
            _ => false,
        }
    }

    /// The revision as written, with the release it stands for when that
    /// differs (a commit id pinned to a tag).
    fn pinned(&self) -> String {
        match &self.current {
            Some(tag) if tag.name != self.revision => {
                format!("{} ({})", short(&self.revision), tag.name)
            }
            _ => short(&self.revision),
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verdict = match (&self.latest, self.is_outdated()) {
            (None, _) => "no release tag".to_owned(),
            (Some(latest), true) => format!("-> {}", latest.name),
            (Some(_), false) if self.current.is_none() => {
                "not a release; cannot compare".to_owned()
            }
            (Some(_), false) => "up to date".to_owned(),
        };
        write!(formatter, "{}  {}  {verdict}", self.repo, self.pinned())
    }
}

fn short(revision: &str) -> String {
    if is_commit_id(revision) {
        revision[..8].to_owned()
    } else {
        revision.to_owned()
    }
}

/// Run the subcommand.
///
/// # Errors
///
/// When the config cannot be loaded or a remote cannot be reached.
pub async fn run(cli: &Cli) -> Result<()> {
    if cli.offline {
        bail!("`outdated` asks every remote for its tags; drop --offline");
    }
    let context = Context::open(cli)?;
    for status in check(&context.config).await? {
        println!("{status}");
    }
    Ok(())
}

/// Ask each git source's remote for its releases.
///
/// # Errors
///
/// When a remote cannot be reached.
pub async fn check(config: &Config) -> Result<Vec<Status>> {
    let mut statuses = Vec::new();
    for repo in &config.repos {
        let Source::Git { url, revision } = repo.source() else {
            continue;
        };
        let tags = git::remote_tags(&url, &config.base_dir).await?;
        statuses.push(Status {
            current: version::current(&revision, &tags).cloned(),
            latest: version::latest(&tags).cloned(),
            repo: url,
            revision,
        });
    }
    Ok(statuses)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "acad0f52027cf8f8edf7bfa6a55e13c594d8ee71";

    fn tag(name: &str, commit: &str) -> RemoteTag {
        RemoteTag {
            name: name.to_owned(),
            commit: commit.to_owned(),
        }
    }

    fn status(revision: &str, current: Option<RemoteTag>, latest: Option<RemoteTag>) -> Status {
        Status {
            repo: "https://example.invalid/x".to_owned(),
            revision: revision.to_owned(),
            current,
            latest,
        }
    }

    #[test]
    fn a_tag_behind_the_latest_release_is_outdated() {
        let status = status("v0.1.0", Some(tag("v0.1.0", SHA)), Some(tag("v0.2.0", "b")));

        assert!(status.is_outdated());
        assert_eq!(
            status.to_string(),
            "https://example.invalid/x  v0.1.0  -> v0.2.0"
        );
    }

    #[test]
    fn a_commit_shows_the_release_it_stands_for() {
        let status = status(SHA, Some(tag("v0.2.0", SHA)), Some(tag("v0.2.0", SHA)));

        assert!(!status.is_outdated());
        assert_eq!(
            status.to_string(),
            "https://example.invalid/x  acad0f52 (v0.2.0)  up to date"
        );
    }

    #[test]
    fn a_branch_cannot_be_compared() {
        let status = status("main", None, Some(tag("v0.2.0", "b")));

        assert!(!status.is_outdated());
        assert!(status.to_string().ends_with("cannot compare"));
    }
}
