//! `pollen show`: list the skills a git source provides, before adding it to
//! a config.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::cli::{Format, Options};
use crate::config::PathSpec;
use crate::source::{git, relative_slug};
use crate::version::{self, RemoteTag};
use crate::{discovery, skill};

/// Descriptions longer than this are cut in the text listing.
const DESCRIPTION_WIDTH: usize = 90;

/// What one source provides at one revision.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Listing {
    /// The URL or path that was inspected.
    pub repo: String,
    /// The revision that was checked out: the one asked for, the newest
    /// release, or `HEAD` when the source has none.
    pub revision: String,
    /// The release that revision stands for, when it is one.
    pub release: Option<String>,
    /// The commit the revision resolved to.
    pub commit: String,
    /// Every skill directory found, ordered by path.
    pub skills: Vec<Entry>,
}

/// One skill directory found in the source.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Entry {
    /// Its path inside the source, the form `paths:` takes.
    pub path: String,
    /// The `name` of a valid skill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The `description` of a valid skill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Why the skill breaks the Agent Skills specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Run the subcommand.
///
/// # Errors
///
/// When the source cannot be fetched, the revision names nothing, or `path`
/// is missing from the source.
pub async fn run(
    options: &Options,
    repo: &str,
    revision: Option<&str>,
    path: Option<&Path>,
    format: Format,
) -> Result<()> {
    let listing = inspect(repo, revision, path, &options.cache_dir(), options.offline).await?;
    match format {
        Format::Text => print!("{}", render(&listing)),
        Format::Json => println!("{}", serde_json::to_string_pretty(&listing)?),
    }
    Ok(())
}

/// Check `repo` out at `revision` — its newest release when `None` — and list
/// the skills under `path`, the whole source when `None`.
///
/// A skill that breaks the specification is listed with its error rather than
/// failing the run: `show` is how a catalog in progress gets inspected too.
///
/// # Errors
///
/// When the source cannot be fetched, the revision names nothing, or `path`
/// is missing from the source.
pub async fn inspect(
    repo: &str,
    revision: Option<&str>,
    path: Option<&Path>,
    cache_dir: &Path,
    offline: bool,
) -> Result<Listing> {
    // Run `git ls-remote` from the working directory, so a relative path to a
    // local repository resolves the way it was typed.
    let tags = if offline {
        Vec::new()
    } else {
        git::remote_tags(repo, Path::new(".")).await?
    };
    let (revision, release) = pick_revision(revision, &tags, offline)?;

    let checkout = git::checkout(repo, &revision, cache_dir, offline).await?;
    let root = checkout.root.canonicalize()?;
    let spec = PathSpec {
        path: path.map_or_else(|| PathBuf::from("."), Path::to_path_buf),
        recurse: true,
        exclude: None,
    };

    let skills = discovery::discover(&root, &spec)?
        .into_iter()
        .map(|dir| {
            let path = relative_slug(&root, &dir);
            match skill::load(&dir) {
                Ok(frontmatter) => Entry {
                    path,
                    name: Some(frontmatter.name),
                    description: Some(frontmatter.description),
                    error: None,
                },
                Err(error) => Entry {
                    path,
                    name: None,
                    description: None,
                    error: Some(error.to_string()),
                },
            }
        })
        .collect();

    Ok(Listing {
        repo: repo.to_owned(),
        revision,
        release,
        commit: checkout.resolved.unwrap_or_default(),
        skills,
    })
}

/// The revision to check out, and the release it stands for.
fn pick_revision(
    asked: Option<&str>,
    tags: &[RemoteTag],
    offline: bool,
) -> Result<(String, Option<String>)> {
    if let Some(revision) = asked {
        let release = version::current(revision, tags).map(|tag| tag.name.clone());
        return Ok((revision.to_owned(), release));
    }
    if offline {
        bail!("--offline cannot look up the newest release; pass --revision");
    }
    Ok(version::latest(tags).map_or_else(
        || ("HEAD".to_owned(), None),
        |tag| (tag.name.clone(), Some(tag.name.clone())),
    ))
}

/// The text listing: a header, one line per skill, and the line to pin it.
#[must_use]
pub fn render(listing: &Listing) -> String {
    let short = listing.commit.get(..8).unwrap_or(&listing.commit);
    let mut out = String::new();
    let _ = writeln!(out, "{} @ {} ({short})", listing.repo, listing.revision);

    let width = listing
        .skills
        .iter()
        .map(|entry| entry.path.chars().count())
        .max()
        .unwrap_or(0);
    for entry in &listing.skills {
        let detail = match (&entry.description, &entry.error) {
            (Some(description), _) => truncate(description, DESCRIPTION_WIDTH),
            (None, Some(error)) => format!("invalid: {error}"),
            (None, None) => String::new(),
        };
        let _ = writeln!(out, "  {:width$}  {detail}", entry.path);
    }

    let invalid = listing
        .skills
        .iter()
        .filter(|entry| entry.error.is_some())
        .count();
    let valid = listing.skills.len() - invalid;
    let _ = write!(out, "{valid} skill(s)");
    if invalid > 0 {
        let _ = write!(out, ", {invalid} invalid");
    }
    let _ = writeln!(out);

    if !listing.commit.is_empty() {
        let comment = listing
            .release
            .as_ref()
            .map_or_else(String::new, |release| format!("  # {release}"));
        let _ = writeln!(out, "pin with: revision: {}{comment}", listing.commit);
    }
    out
}

/// `text` cut to `width` characters, with an ellipsis when something was cut.
fn truncate(text: &str, width: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= width {
        return text;
    }
    let cut: String = text.chars().take(width - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command;

    fn git_command(root: &Path, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
        command
    }

    fn git(root: &Path, args: &[&str]) {
        let status = git_command(root, args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn fixtures_commit_and_tag_under_a_global_signing_setup() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("gitconfig");
        std::fs::write(
            &config,
            "[commit]\n\tgpgsign = true\n[tag]\n\tgpgsign = true\n[gpg]\n\tprogram = false\n",
        )
        .unwrap();
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "--quiet", "-b", "main"]);
        write(repo.path(), "README", "x");
        git(repo.path(), &["add", "-A"]);

        for args in [
            &["commit", "--quiet", "-m", "signed?"][..],
            &["tag", "v1.0.0"],
        ] {
            let status = git_command(repo.path(), args)
                .env("GIT_CONFIG_GLOBAL", &config)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} tried to sign");
        }
    }

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn skill(root: &Path, dir: &str, description: &str) {
        let name = dir.rsplit('/').next().unwrap();
        write(
            root,
            &format!("{dir}/SKILL.md"),
            &format!("---\nname: {name}\ndescription: {description}\n---\n\nBody.\n"),
        );
    }

    fn commit(root: &Path, message: &str) {
        git(root, &["add", "-A"]);
        git(root, &["commit", "--quiet", "-m", message]);
    }

    /// A catalog with two releases: v1.0.0 has one skill, v1.1.0 adds a
    /// second one and a broken one.
    fn catalog() -> tempfile::TempDir {
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "--quiet", "-b", "main"]);
        skill(
            origin.path(),
            "skills/docker/dockerfile",
            "Write Dockerfiles.",
        );
        commit(origin.path(), "first");
        git(origin.path(), &["tag", "v1.0.0"]);
        skill(origin.path(), "skills/git/commit-style", "Write commits.");
        write(
            origin.path(),
            "skills/git/broken/SKILL.md",
            "---\nname: Not-Valid\ndescription: x\n---\n",
        );
        commit(origin.path(), "second");
        git(origin.path(), &["tag", "v1.1.0"]);
        origin
    }

    fn url(origin: &tempfile::TempDir) -> String {
        origin.path().display().to_string()
    }

    fn paths(listing: &Listing) -> Vec<&str> {
        listing
            .skills
            .iter()
            .map(|entry| entry.path.as_str())
            .collect()
    }

    #[tokio::test]
    async fn lists_the_newest_release_by_default_with_invalid_skills_flagged() {
        let origin = catalog();
        let cache = tempfile::tempdir().unwrap();

        let listing = inspect(&url(&origin), None, None, cache.path(), false)
            .await
            .unwrap();

        assert_eq!(listing.revision, "v1.1.0");
        assert_eq!(listing.release.as_deref(), Some("v1.1.0"));
        assert_eq!(listing.commit.len(), 40);
        assert_eq!(
            paths(&listing),
            [
                "skills/docker/dockerfile",
                "skills/git/broken",
                "skills/git/commit-style"
            ]
        );
        let broken = &listing.skills[1];
        assert!(broken.name.is_none());
        assert!(broken.error.is_some());
        assert_eq!(
            listing.skills[2].description.as_deref(),
            Some("Write commits.")
        );
    }

    #[tokio::test]
    async fn shows_the_revision_asked_for_and_the_release_it_stands_for() {
        let origin = catalog();
        let cache = tempfile::tempdir().unwrap();

        let listing = inspect(&url(&origin), Some("v1.0.0"), None, cache.path(), false)
            .await
            .unwrap();

        assert_eq!(listing.revision, "v1.0.0");
        assert_eq!(listing.release.as_deref(), Some("v1.0.0"));
        assert_eq!(paths(&listing), ["skills/docker/dockerfile"]);
    }

    #[tokio::test]
    async fn restricts_the_listing_to_a_path() {
        let origin = catalog();
        let cache = tempfile::tempdir().unwrap();

        let listing = inspect(
            &url(&origin),
            None,
            Some(Path::new("skills/docker")),
            cache.path(),
            false,
        )
        .await
        .unwrap();

        assert_eq!(paths(&listing), ["skills/docker/dockerfile"]);
    }

    #[tokio::test]
    async fn falls_back_to_head_when_the_source_has_no_release() {
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "--quiet", "-b", "main"]);
        skill(origin.path(), "skills/only", "The only skill.");
        commit(origin.path(), "first");
        let cache = tempfile::tempdir().unwrap();

        let listing = inspect(&url(&origin), None, None, cache.path(), false)
            .await
            .unwrap();

        assert_eq!(listing.revision, "HEAD");
        assert_eq!(listing.release, None);
        assert_eq!(paths(&listing), ["skills/only"]);
    }

    #[tokio::test]
    async fn offline_needs_an_explicit_revision() {
        let cache = tempfile::tempdir().unwrap();

        let result = inspect("https://example.invalid/x", None, None, cache.path(), true).await;

        assert!(result.is_err());
    }

    fn listing(skills: Vec<Entry>) -> Listing {
        Listing {
            repo: "https://example.invalid/skills".to_owned(),
            revision: "v1.1.0".to_owned(),
            release: Some("v1.1.0".to_owned()),
            commit: "5984faabb700c59271614f35fe5f2e29939ef89f".to_owned(),
            skills,
        }
    }

    fn valid(path: &str, description: &str) -> Entry {
        Entry {
            path: path.to_owned(),
            name: Some(path.rsplit('/').next().unwrap().to_owned()),
            description: Some(description.to_owned()),
            error: None,
        }
    }

    #[test]
    fn renders_one_aligned_line_per_skill_and_the_pin() {
        let text = render(&listing(vec![
            valid("skills/a", "First."),
            valid("skills/longer-name", "Second."),
            Entry {
                path: "skills/bad".to_owned(),
                name: None,
                description: None,
                error: Some("no SKILL.md".to_owned()),
            },
        ]));

        assert_eq!(
            text,
            "https://example.invalid/skills @ v1.1.0 (5984faab)\n\
             \x20 skills/a            First.\n\
             \x20 skills/longer-name  Second.\n\
             \x20 skills/bad          invalid: no SKILL.md\n\
             2 skill(s), 1 invalid\n\
             pin with: revision: 5984faabb700c59271614f35fe5f2e29939ef89f  # v1.1.0\n"
        );
    }

    #[test]
    fn cuts_long_descriptions_on_one_line() {
        let long = format!("{}\n  continued", "word ".repeat(40));

        let cut = truncate(&long, DESCRIPTION_WIDTH);

        assert_eq!(cut.chars().count(), DESCRIPTION_WIDTH);
        assert!(cut.ends_with('…'));
        assert!(!cut.contains('\n'));
    }

    #[test]
    fn serializes_only_the_fields_an_entry_has() {
        let json = serde_json::to_value(listing(vec![valid("skills/a", "First.")])).unwrap();

        assert_eq!(
            json["skills"][0],
            serde_json::json!({"path": "skills/a", "name": "a", "description": "First."})
        );
    }
}
