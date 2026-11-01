//! `pollen outdated`: which git sources have a newer release than the one
//! they are pinned to.

use std::fmt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};

use crate::cli::Options;
use crate::command::Context;
use crate::config::{Config, Source, is_commit_id};
use crate::source::git;
use crate::version::{self, RemoteTag, Version};

/// Where one git source stands against its remote's releases.
#[derive(Debug, Clone)]
pub struct Status {
    /// The `repo:` value.
    pub repo: String,
    /// The `revision:` value.
    pub revision: String,
    /// The release the revision stands for, when it is one.
    pub current: Option<RemoteTag>,
    /// The newest release on the remote; with a minimum release age, the
    /// newest one that old.
    pub latest: Option<RemoteTag>,
    /// The newest release left out for being younger than the minimum age.
    pub pending: Option<Pending>,
}

/// A release newer than `latest`, left out for being too young.
#[derive(Debug, Clone)]
pub struct Pending {
    /// The release.
    pub release: RemoteTag,
    /// How old it is, `None` when git records no date for its tag.
    pub age: Option<Duration>,
}

impl fmt::Display for Pending {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const HOUR: u64 = 60 * 60;
        const DAY: u64 = 24 * HOUR;

        let name = &self.release.name;
        let Some(age) = self.age else {
            return write!(formatter, "{name} has no recorded date");
        };
        let (count, unit) = match age.as_secs() {
            seconds if seconds >= DAY => (seconds / DAY, "day"),
            seconds if seconds >= HOUR => (seconds / HOUR, "hour"),
            _ => return write!(formatter, "{name} is under an hour old"),
        };
        let plural = if count == 1 { "" } else { "s" };
        write!(formatter, "{name} is {count} {unit}{plural} old")
    }
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
            (None, _) if self.pending.is_some() => "no release old enough".to_owned(),
            (None, _) => "no release tag".to_owned(),
            (Some(latest), true) => format!("-> {}", latest.name),
            (Some(_), false) if self.current.is_none() => {
                "not a release; cannot compare".to_owned()
            }
            (Some(_), false) => "up to date".to_owned(),
        };
        write!(formatter, "{}  {}  {verdict}", self.repo, self.pinned())?;
        if let Some(pending) = &self.pending {
            write!(formatter, "  ({pending})")?;
        }
        Ok(())
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
pub async fn run(options: &Options, min_age: Option<Duration>) -> Result<()> {
    for status in statuses(options, min_age).await? {
        println!("{status}");
    }
    Ok(())
}

/// Load the config `options` name and check each git source against its
/// remote's releases, leaving out those younger than `min_age`.
///
/// # Errors
///
/// When `options` ask to stay offline, the config cannot be loaded, or a
/// remote cannot be reached.
pub async fn statuses(options: &Options, min_age: Option<Duration>) -> Result<Vec<Status>> {
    if options.offline {
        bail!("`outdated` asks every remote for its tags; drop --offline");
    }
    let context = Context::open(options)?;
    check(&context.config, &options.cache_dir(), min_age).await
}

/// Ask each git source's remote for its releases.
///
/// With a `min_age` (zero means none), a source behind its newest release
/// moves to the newest one at least that old, and the newest one left out is
/// its `pending`. Only the tags of those candidates are fetched, into
/// `cache_dir`, to read their dates.
///
/// # Errors
///
/// When a remote cannot be reached.
pub async fn check(
    config: &Config,
    cache_dir: &Path,
    min_age: Option<Duration>,
) -> Result<Vec<Status>> {
    let min_age = min_age.filter(|age| !age.is_zero());
    let now = SystemTime::now();
    let mut statuses = Vec::new();
    for repo in &config.repos {
        let Source::Git { url, revision } = repo.source() else {
            continue;
        };
        let tags = git::remote_tags(&url, &config.base_dir).await?;
        let mut status = Status {
            current: version::current(&revision, &tags).cloned(),
            latest: version::latest(&tags).cloned(),
            pending: None,
            repo: url,
            revision,
        };
        if let Some(min_age) = min_age
            && status.is_outdated()
        {
            hold_back(&mut status, &tags, config, cache_dir, min_age, now).await?;
        }
        statuses.push(status);
    }
    Ok(statuses)
}

/// Move `status.latest` back to the newest release at least `min_age` old,
/// newer than the current one, and record the newest release passed over.
///
/// A release whose tag carries no date is passed over too: its age cannot be
/// shown to be enough.
async fn hold_back(
    status: &mut Status,
    tags: &[RemoteTag],
    config: &Config,
    cache_dir: &Path,
    min_age: Duration,
    now: SystemTime,
) -> Result<()> {
    let floor = status
        .current
        .as_ref()
        .and_then(|tag| Version::parse(&tag.name));
    for release in version::newer_than(tags, floor) {
        let date = git::tag_date(&status.repo, release, &config.base_dir, cache_dir).await?;
        // A date in the future counts as no age at all.
        let age = date.map(|date| {
            now.duration_since(date).unwrap_or_else(|_| {
                tracing::warn!(
                    "{}: `{}` is dated in the future, so it is not old enough yet",
                    status.repo,
                    release.name
                );
                Duration::ZERO
            })
        });
        if age.is_some_and(|age| age >= min_age) {
            status.latest = Some(release.clone());
            return Ok(());
        }
        if age.is_none() {
            tracing::warn!(
                "{}: git records no date for `{}`, so it is never old enough",
                status.repo,
                release.name
            );
        }
        status.pending.get_or_insert_with(|| Pending {
            release: release.clone(),
            age,
        });
    }
    status.latest = status
        .current
        .clone()
        .filter(|tag| Version::parse(&tag.name).is_some());
    Ok(())
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
            pending: None,
        }
    }

    fn pending(name: &str, age: Option<Duration>) -> Pending {
        Pending {
            release: tag(name, "c"),
            age,
        }
    }

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

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

    #[test]
    fn a_held_back_release_is_named_with_its_age() {
        let mut moved = status("v0.1.0", Some(tag("v0.1.0", SHA)), Some(tag("v0.1.1", "b")));
        moved.pending = Some(pending("v0.2.0", Some(2 * DAY + DAY / 2)));
        let mut kept = status("v0.1.0", Some(tag("v0.1.0", SHA)), Some(tag("v0.1.0", SHA)));
        kept.pending = Some(pending("v0.1.1", Some(DAY / 24)));
        let mut untagged = status(SHA, None, None);
        untagged.pending = Some(pending("v0.1.0", None));

        assert_eq!(
            moved.to_string(),
            "https://example.invalid/x  v0.1.0  -> v0.1.1  (v0.2.0 is 2 days old)"
        );
        assert_eq!(
            kept.to_string(),
            "https://example.invalid/x  v0.1.0  up to date  (v0.1.1 is 1 hour old)"
        );
        assert_eq!(
            untagged.to_string(),
            "https://example.invalid/x  acad0f52  no release old enough  (v0.1.0 has no recorded date)"
        );
    }

    #[test]
    fn an_age_under_an_hour_or_in_the_future_reads_as_such() {
        for age in [Duration::ZERO, Duration::from_secs(59 * 60)] {
            assert_eq!(
                pending("v1.0.0", Some(age)).to_string(),
                "v1.0.0 is under an hour old"
            );
        }
        assert_eq!(
            pending("v1.0.0", Some(DAY)).to_string(),
            "v1.0.0 is 1 day old"
        );
    }

    /// A repository with one commit per release, each `days` old, tagged
    /// lightweight; returns the commit of each.
    fn releases(root: &Path, releases: &[(&str, u64)]) -> Vec<String> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        git(root, "", &["init", "--quiet", "--initial-branch", "main"]);
        releases
            .iter()
            .map(|(name, days)| {
                let date = format!("{} +0000", now - days * DAY.as_secs());
                git(
                    root,
                    &date,
                    &["commit", "--quiet", "--allow-empty", "--message", name],
                );
                git(root, &date, &["tag", name]);
                git(root, &date, &["rev-parse", "HEAD"])
            })
            .collect()
    }

    fn git(root: &Path, date: &str, args: &[&str]) -> String {
        let mut command = std::process::Command::new("git");
        command
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "pollen tests")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "pollen tests")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
        if !date.is_empty() {
            command
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    async fn check_one(origin: &Path, revision: &str, cache: &Path, days: u32) -> Status {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("pollen.yaml");
        std::fs::write(
            &path,
            format!(
                "repos:\n  - repo: {}\n    revision: {revision}\n    paths:\n      - path: .\n",
                origin.display()
            ),
        )
        .unwrap();
        let config = Config::load(&path).unwrap();

        check(&config, cache, Some(days * DAY))
            .await
            .unwrap()
            .remove(0)
    }

    #[tokio::test]
    async fn moves_to_the_newest_release_old_enough() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let commits = releases(
            origin.path(),
            &[("v1.0.0", 60), ("v1.1.0", 30), ("v1.2.0", 3), ("v1.3.0", 1)],
        );

        for revision in ["v1.0.0", commits[0].as_str()] {
            let status = check_one(origin.path(), revision, cache.path(), 7).await;

            assert!(status.is_outdated(), "{revision}");
            assert_eq!(status.latest.unwrap().name, "v1.1.0", "{revision}");
            let pending = status.pending.unwrap();
            assert_eq!(pending.release.name, "v1.3.0", "{revision}");
            assert_eq!(pending.age.unwrap().as_secs() / DAY.as_secs(), 1);
        }
    }

    #[tokio::test]
    async fn a_source_with_only_younger_releases_stays_where_it_is() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        releases(origin.path(), &[("v1.0.0", 60), ("v1.1.0", 2)]);

        let status = check_one(origin.path(), "v1.0.0", cache.path(), 7).await;

        assert!(!status.is_outdated());
        assert_eq!(status.latest.unwrap().name, "v1.0.0");
        assert_eq!(status.pending.unwrap().release.name, "v1.1.0");
        let fetched = std::fs::read_dir(cache.path().join("tags"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            git(
                &fetched,
                "",
                &["for-each-ref", "--format=%(refname:strip=2)"]
            ),
            "v1.1.0"
        );
    }

    #[tokio::test]
    async fn a_release_dated_in_the_future_is_never_old_enough() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        releases(origin.path(), &[("v1.0.0", 60)]);
        let future = SystemTime::now() + 400 * DAY;
        let date = format!(
            "{} +0000",
            future
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        git(
            origin.path(),
            &date,
            &["commit", "--quiet", "--allow-empty", "--message", "v1.1.0"],
        );
        git(origin.path(), &date, &["tag", "v1.1.0"]);

        let status = check_one(origin.path(), "v1.0.0", cache.path(), 7).await;

        assert!(!status.is_outdated());
        let pending = status.pending.unwrap();
        assert_eq!(pending.release.name, "v1.1.0");
        assert_eq!(pending.age, Some(Duration::ZERO));
    }

    #[tokio::test]
    async fn a_release_without_a_date_is_never_old_enough() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let commits = releases(origin.path(), &[("v1.0.0", 60), ("v1.1.0", 30)]);
        std::fs::write(
            origin.path().join("tag"),
            format!(
                "object {}\ntype commit\ntag v1.2.0\n\nNo tagger line.\n",
                commits[1]
            ),
        )
        .unwrap();
        let object = git(
            origin.path(),
            "",
            &["hash-object", "-t", "tag", "-w", "--literally", "tag"],
        );
        git(
            origin.path(),
            "",
            &["update-ref", "refs/tags/v1.2.0", &object],
        );

        let status = check_one(origin.path(), "v1.0.0", cache.path(), 7).await;

        assert_eq!(status.latest.unwrap().name, "v1.1.0");
        let pending = status.pending.unwrap();
        assert_eq!(
            (pending.release.name.as_str(), pending.age),
            ("v1.2.0", None)
        );
    }

    #[tokio::test]
    async fn an_untagged_commit_with_only_younger_releases_has_none_to_move_to() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        releases(origin.path(), &[("v1.0.0", 1)]);
        git(
            origin.path(),
            "",
            &["commit", "--quiet", "--allow-empty", "--message", "next"],
        );
        let untagged = git(origin.path(), "", &["rev-parse", "HEAD"]);

        let status = check_one(origin.path(), &untagged, cache.path(), 7).await;

        assert!(!status.is_outdated());
        assert!(status.latest.is_none());
        assert_eq!(status.pending.unwrap().release.name, "v1.0.0");
    }

    #[tokio::test]
    async fn only_a_source_behind_its_newest_release_fetches_dates() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        releases(origin.path(), &[("v1.0.0", 60), ("v1.1.0", 1)]);

        for revision in ["v1.1.0", "main"] {
            let status = check_one(origin.path(), revision, cache.path(), 7).await;

            assert!(status.pending.is_none(), "{revision}");
        }
        let status = check_one(origin.path(), "v1.0.0", cache.path(), 0).await;

        assert_eq!(status.latest.unwrap().name, "v1.1.0");
        assert!(!cache.path().join("tags").exists());
    }
}
