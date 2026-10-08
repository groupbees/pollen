//! The git side of materialization: a per-revision checkout under the cache.

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use super::Materialized;
use crate::config::is_commit_id;

/// Check `revision` of `url` out into the cache, and say where it landed.
///
/// # Errors
///
/// When `git` is missing, the fetch fails, or the revision names nothing.
pub async fn checkout(
    url: &str,
    revision: &str,
    cache_dir: &Path,
    offline: bool,
) -> Result<Materialized> {
    let repo_dir = cache_dir.join("repos").join(cache_key(url, revision));
    tokio::fs::create_dir_all(&repo_dir)
        .await
        .with_context(|| format!("cannot create the cache directory {}", repo_dir.display()))?;

    if !repo_dir.join(".git").exists() {
        git(&repo_dir, &["init", "--quiet"]).await?;
    }
    if git(&repo_dir, &["remote", "set-url", "origin", url])
        .await
        .is_err()
    {
        git(&repo_dir, &["remote", "add", "origin", url]).await?;
    }

    let fetched = if offline {
        tracing::debug!(%url, %revision, "offline, using the cached checkout");
        Fetch::Skipped
    } else {
        fetch(&repo_dir, revision).await?
    };

    let commit = resolve(&repo_dir, revision, fetched)
        .await
        .with_context(|| {
            if offline {
                format!("`{revision}` is not in the cache for {url}; drop --offline to fetch it")
            } else {
                format!("`{revision}` does not name a commit, tag or branch in {url}")
            }
        })?;

    if !offline && !is_commit_id(revision) && names_a_branch(&repo_dir, revision).await {
        tracing::warn!(
            "{url}: `{revision}` is a branch, so each update deploys whatever it points to now; pin a tag or a commit"
        );
    }

    git(
        &repo_dir,
        &["checkout", "--quiet", "--force", "--detach", &commit],
    )
    .await?;
    git(&repo_dir, &["clean", "--quiet", "-ffd"]).await?;
    tracing::debug!(%url, %revision, %commit, path = %repo_dir.display(), "checked out");

    Ok(Materialized {
        root: repo_dir,
        resolved: Some(commit),
    })
}

/// One cache directory per `(url, revision)`, so two pins never clobber each other.
fn cache_key(url: &str, revision: &str) -> String {
    let digest = Sha256::digest(format!("{url}\n{revision}").as_bytes());
    let slug: String = url
        .trim_end_matches(['/', '\\'])
        .trim_end_matches(".git")
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or("repo")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("{slug}-{}", hex::encode(&digest[..8]))
}

/// How the revision was brought into the cache, which decides what
/// `resolve` is allowed to trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fetch {
    /// The revision itself was fetched, so `FETCH_HEAD` is exactly it.
    Pinned,
    /// Every branch and tag was fetched; `FETCH_HEAD` means nothing here.
    Full,
    /// Nothing was fetched; the previous run left the revision at `HEAD`.
    Skipped,
}

/// A shallow fetch of the single revision, falling back to a full one.
///
/// `--depth 1 <revision>` is refused by servers that disallow fetching an
/// arbitrary commit, and by older ones for tags, hence the fallback rather
/// than a hard failure.
async fn fetch(repo_dir: &Path, revision: &str) -> Result<Fetch> {
    if git(
        repo_dir,
        &["fetch", "--quiet", "--depth", "1", "origin", revision],
    )
    .await
    .is_ok()
    {
        return Ok(Fetch::Pinned);
    }

    tracing::debug!(%revision, "shallow fetch refused, falling back to a full fetch");
    git(
        repo_dir,
        &[
            "fetch",
            "--quiet",
            "--force",
            "--tags",
            "origin",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    )
    .await
    .map(|_| Fetch::Full)
}

/// Turn a revision into a commit, without ever guessing.
///
/// A full fetch leaves `FETCH_HEAD` pointing at the remote's branches, so
/// trusting it there would silently resolve a mistyped tag to the default
/// branch. It only names the requested revision after a pinned fetch.
async fn resolve(repo_dir: &Path, revision: &str, fetched: Fetch) -> Result<String> {
    let mut candidates = vec![
        revision.to_owned(),
        format!("refs/tags/{revision}"),
        format!("refs/remotes/origin/{revision}"),
    ];
    match fetched {
        Fetch::Pinned => candidates.push("FETCH_HEAD".to_owned()),
        Fetch::Skipped => candidates.push("HEAD".to_owned()),
        Fetch::Full => {}
    }

    for candidate in &candidates {
        let spec = format!("{candidate}^{{commit}}");
        if let Ok(output) = git(repo_dir, &["rev-parse", "--verify", "--quiet", &spec]).await {
            let commit = output.trim().to_owned();
            if !commit.is_empty() {
                return Ok(commit);
            }
        }
    }

    bail!("no local ref matches `{revision}`")
}

/// Whether the remote has a branch named `revision` and no tag of that name.
///
/// Asked of the remote because a pinned fetch leaves no local ref behind to
/// tell the two apart. A failed query is not worth failing the run over.
async fn names_a_branch(repo_dir: &Path, revision: &str) -> bool {
    let refs = match git(
        repo_dir,
        &["ls-remote", "--heads", "--tags", "origin", revision],
    )
    .await
    {
        Ok(refs) => refs,
        Err(error) => {
            tracing::debug!(%revision, "cannot list the remote refs: {error:#}");
            return false;
        }
    };
    let names: Vec<&str> = refs
        .lines()
        .filter_map(|line| line.split('\t').nth(1))
        .collect();
    let branch = format!("refs/heads/{revision}");
    let tag = format!("refs/tags/{revision}");
    names.contains(&branch.as_str())
        && !names.iter().any(|name| name.trim_end_matches("^{}") == tag)
}

fn command(repo_dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    // Git for Windows stops at 260-character paths without this, and a skill
    // nested in the cache easily passes that. Other platforms ignore it.
    command
        .args(["-c", "core.longpaths=true"])
        .args(args)
        .current_dir(repo_dir)
        .stdin(Stdio::null());
    command
}

async fn git(repo_dir: &Path, args: &[&str]) -> Result<String> {
    tracing::trace!(dir = %repo_dir.display(), ?args, "running git");
    let output = command(repo_dir, args)
        .output()
        .await
        .context("cannot run `git`; pollen needs it on PATH")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_separate_revisions_of_one_repository() {
        let one = cache_key("https://github.com/toto/tata.git", "1.2.3");
        let two = cache_key("https://github.com/toto/tata.git", "1.2.4");

        assert!(one.starts_with("tata-"), "{one}");
        assert_ne!(one, two);
    }

    #[test]
    fn cache_keys_survive_an_ssh_url() {
        let key = cache_key("git@github.com:toto/tata.git", "main");

        assert!(key.starts_with("tata-"), "{key}");
    }

    #[test]
    fn cache_keys_name_a_local_path_after_its_last_directory() {
        for url in [r"C:\repos\tata", "C:/repos/tata/", "/srv/git/tata.git/"] {
            let key = cache_key(url, "main");

            assert!(key.starts_with("tata-"), "{url} gave {key}");
        }
    }

    #[test]
    fn lifts_the_windows_path_length_limit_on_every_call() {
        let command = command(Path::new("."), &["fetch", "origin"]);
        let args: Vec<_> = command.as_std().get_args().collect();

        assert_eq!(args, ["-c", "core.longpaths=true", "fetch", "origin"]);
    }

    #[tokio::test]
    async fn checks_a_tag_out_of_a_local_repository() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        seed_repository(origin.path());

        let materialized = checkout(
            origin.path().to_str().unwrap(),
            "v1.0.0",
            cache.path(),
            false,
        )
        .await
        .unwrap();

        assert!(materialized.root.join("skills/demo/SKILL.md").is_file());
        assert_eq!(materialized.resolved.as_ref().unwrap().len(), 40);
    }

    #[tokio::test]
    async fn refuses_an_unknown_revision() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        seed_repository(origin.path());

        let error = checkout(
            origin.path().to_str().unwrap(),
            "v9.9.9",
            cache.path(),
            false,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("v9.9.9"), "{error}");
    }

    #[tokio::test]
    async fn serves_a_cached_revision_offline() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        seed_repository(origin.path());
        let url = origin.path().to_str().unwrap();

        checkout(url, "v1.0.0", cache.path(), false).await.unwrap();
        let offline = checkout(url, "v1.0.0", cache.path(), true).await.unwrap();

        assert!(offline.root.join("skills/demo/SKILL.md").is_file());
    }

    #[tokio::test]
    async fn tells_a_branch_from_a_tag() {
        let origin = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        seed_repository(origin.path());
        let url = origin.path().to_str().unwrap();

        let branch = checkout(url, "main", cache.path(), false).await.unwrap();
        let tag = checkout(url, "v1.0.0", cache.path(), false).await.unwrap();

        assert!(names_a_branch(&branch.root, "main").await);
        assert!(!names_a_branch(&tag.root, "v1.0.0").await);
        assert!(!names_a_branch(&tag.root, "nothing").await);
    }

    fn seed_repository(root: &Path) {
        let skill = root.join("skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: A demo skill.\n---\n",
        )
        .unwrap();

        for args in [
            vec!["init", "--quiet", "--initial-branch", "main"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["config", "user.name", "pollen tests"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "tag.gpgsign", "false"],
            vec!["add", "."],
            vec!["commit", "--quiet", "--message", "seed"],
            vec!["tag", "v1.0.0"],
        ] {
            let output = std::process::Command::new("git")
                .args(&args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
