//! `pollen autoupdate`: move every git source to its newest release, in place.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context as _, Result, bail};
use regex::Regex;

use crate::cli::Cli;
use crate::command::Context;
use crate::command::outdated::{self, Status};
use crate::config::{Config, is_commit_id};
use crate::version::Version;

/// One `revision:` to rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bump {
    /// The `repo:` value of the entry.
    pub repo: String,
    /// The `revision:` value it holds now.
    pub from: String,
    /// The value to write.
    pub to: String,
    /// The release `to` stands for, written as a trailing comment when `to`
    /// is a commit id.
    pub release: Option<String>,
}

/// Run the subcommand.
///
/// # Errors
///
/// When the config cannot be loaded, a remote cannot be reached, or the
/// rewritten file would not be a valid config.
pub async fn run(cli: &Cli, freeze: bool, dry_run: bool) -> Result<()> {
    if cli.offline {
        bail!("`autoupdate` asks every remote for its tags; drop --offline");
    }
    let path = cli.config_path();
    let context = Context::open(cli)?;
    let bumps = plan(&outdated::check(&context.config).await?, freeze);

    if bumps.is_empty() {
        println!("{}: every source is up to date", path.display());
        return Ok(());
    }
    for bump in &bumps {
        println!("{}  {} -> {}", bump.repo, bump.from, written(bump));
    }
    if dry_run {
        println!("{}: not written (dry run)", path.display());
        return Ok(());
    }

    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    write_checked(&path, &rewrite(&text, &bumps)?)?;
    println!("{}: {} source(s) updated", path.display(), bumps.len());
    Ok(())
}

/// What to change: every source behind its newest release, and with `freeze`
/// every tag replaced by the commit it points to.
#[must_use]
pub fn plan(statuses: &[Status], freeze: bool) -> Vec<Bump> {
    let mut bumps = Vec::new();
    for status in statuses {
        let Some(latest) = &status.latest else {
            continue;
        };
        if status.current.is_none() && !is_commit_id(&status.revision) {
            tracing::warn!(
                "{}: `{}` is neither a release nor a commit; left as it is",
                status.repo,
                status.revision
            );
            continue;
        }
        let target = if status.is_outdated() {
            latest
        } else {
            status.current.as_ref().unwrap_or(latest)
        };
        let (to, release) = if freeze {
            (target.commit.clone(), Some(target.name.clone()))
        } else {
            (target.name.clone(), None)
        };
        if to != status.revision {
            bumps.push(Bump {
                repo: status.repo.clone(),
                from: status.revision.clone(),
                to,
                release,
            });
        }
    }
    bumps
}

fn written(bump: &Bump) -> String {
    match &bump.release {
        Some(release) => format!("{}  # {release}", bump.to),
        None => bump.to.clone(),
    }
}

static REPO_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(?:-\s+)?repo:\s*["']?([^"'#\s]+)["']?"#).expect("valid regex")
});
static REVISION_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\s*(?:-\s+)?revision:\s*)["']?([^"'#\s]+)["']?\s*(?:#\s*(.*?))?\s*$"#)
        .expect("valid regex")
});
static ITEM_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)-\s").expect("valid regex"));

/// Apply `bumps` to the text of a `pollen.yaml`, line by line, so comments and
/// layout survive.
///
/// Each entry of `repos:` is matched on its `repo:` and current `revision:`.
/// A trailing comment that names a release is replaced (or dropped), any other
/// comment is kept.
///
/// # Errors
///
/// When an entry to bump cannot be found, e.g. in YAML flow style.
pub fn rewrite(text: &str, bumps: &[Bump]) -> Result<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    let mut applied = vec![false; bumps.len()];

    for (start, end) in entries(&lines) {
        let repo = lines[start..end]
            .iter()
            .find_map(|line| REPO_LINE.captures(line).map(|caps| caps[1].to_owned()));
        let Some(repo) = repo else { continue };
        for index in start..end {
            let line = lines[index];
            let Some(caps) = REVISION_LINE.captures(line.trim_end_matches(['\r', '\n'])) else {
                continue;
            };
            let Some((which, bump)) = bumps
                .iter()
                .enumerate()
                .find(|(_, bump)| bump.repo == repo && bump.from == caps[2])
            else {
                continue;
            };
            let kept = caps
                .get(3)
                .map(|comment| comment.as_str())
                .filter(|comment| !comment.is_empty() && Version::parse(comment).is_none());
            let comment = bump.release.as_deref().or(kept);
            let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
            out[index] = match comment {
                Some(comment) => format!("{}{}  # {comment}{ending}", &caps[1], bump.to),
                None => format!("{}{}{ending}", &caps[1], bump.to),
            };
            applied[which] = true;
        }
    }

    if let Some(missing) = bumps.iter().zip(&applied).find(|(_, done)| !**done) {
        bail!(
            "cannot find the `revision: {}` line of {} to rewrite; edit it by hand",
            missing.0.from,
            missing.0.repo
        );
    }
    Ok(out.concat())
}

/// The `[start, end)` line ranges of the items under `repos:`.
fn entries(lines: &[&str]) -> Vec<(usize, usize)> {
    let Some(repos) = lines
        .iter()
        .position(|line| line.trim_end().starts_with("repos:"))
    else {
        return Vec::new();
    };
    let mut item_indent = None;
    let mut starts = Vec::new();
    let mut end = lines.len();
    for (index, line) in lines.iter().enumerate().skip(repos + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if let Some(caps) = ITEM_START.captures(line) {
            let this = caps[1].len();
            if item_indent.is_none_or(|known| known == this) {
                item_indent = Some(this);
                starts.push(index);
                continue;
            }
        }
        if item_indent.is_some_and(|known| indent <= known && !trimmed.starts_with('-'))
            || indent == 0
        {
            end = index;
            break;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(position, &start)| (start, starts.get(position + 1).copied().unwrap_or(end)))
        .collect()
}

/// Write `text` to `path` only if it still parses as a valid config.
fn write_checked(path: &Path, text: &str) -> Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staging = tempfile::Builder::new()
        .prefix(".pollen-config-")
        .suffix(".yaml")
        .tempfile_in(directory)
        .with_context(|| format!("cannot write into {}", directory.display()))?;
    std::fs::write(staging.path(), text)?;
    Config::load(staging.path()).context("the rewritten config is invalid; nothing was written")?;
    staging
        .persist(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "acad0f52027cf8f8edf7bfa6a55e13c594d8ee71";

    fn bump(repo: &str, from: &str, to: &str, release: Option<&str>) -> Bump {
        Bump {
            repo: repo.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
            release: release.map(str::to_owned),
        }
    }

    const CONFIG: &str = "\
# my skills
targets:
  - ~/.claude/skills
repos:
  - repo: git@github.com:groupbees/skills-core.git   # private
    revision: v0.1.0
    paths:
      - path: skills
        recurse: true
  - revision: \"v1.0.0\"  # keep me
    repo: https://github.com/groupbees/skills-opensource
    paths:
      - path: skills/git
  - repo: local
    paths:
      - path: .skills
";

    #[test]
    fn bumps_a_tag_and_keeps_every_other_line() {
        let out = rewrite(
            CONFIG,
            &[bump(
                "git@github.com:groupbees/skills-core.git",
                "v0.1.0",
                "v0.2.0",
                None,
            )],
        )
        .unwrap();

        assert_eq!(out, CONFIG.replace("revision: v0.1.0", "revision: v0.2.0"));
    }

    #[test]
    fn freezes_to_a_commit_with_the_release_in_a_comment() {
        let out = rewrite(
            CONFIG,
            &[bump(
                "git@github.com:groupbees/skills-core.git",
                "v0.1.0",
                SHA,
                Some("v0.2.0"),
            )],
        )
        .unwrap();

        assert!(
            out.contains(&format!("    revision: {SHA}  # v0.2.0\n")),
            "{out}"
        );
    }

    #[test]
    fn matches_an_entry_whose_revision_comes_before_its_repo_and_keeps_its_comment() {
        let out = rewrite(
            CONFIG,
            &[bump(
                "https://github.com/groupbees/skills-opensource",
                "v1.0.0",
                "v1.1.0",
                None,
            )],
        )
        .unwrap();

        assert!(out.contains("  - revision: v1.1.0  # keep me\n"), "{out}");
    }

    #[test]
    fn replaces_a_stale_release_comment() {
        let text = format!(
            "repos:\n  - repo: https://example.invalid/x\n    revision: {SHA}  # v0.1.0\n    paths:\n      - path: .\n"
        );

        let out = rewrite(
            &text,
            &[bump("https://example.invalid/x", SHA, "v0.2.0", None)],
        )
        .unwrap();

        assert!(out.contains("    revision: v0.2.0\n"), "{out}");
    }

    #[test]
    fn keeps_windows_line_endings() {
        let text = "repos:\r\n  - repo: https://example.invalid/x\r\n    revision: v1.0.0\r\n    paths:\r\n      - path: .\r\n";

        let out = rewrite(
            text,
            &[bump("https://example.invalid/x", "v1.0.0", "v1.1.0", None)],
        )
        .unwrap();

        assert_eq!(out, text.replace("v1.0.0", "v1.1.0"));
    }

    fn status(revision: &str, current: Option<&str>, latest: &str) -> Status {
        let tag = |name: &str| crate::version::RemoteTag {
            name: name.to_owned(),
            commit: if name == latest {
                "f".repeat(40)
            } else {
                SHA.to_owned()
            },
        };
        Status {
            repo: "https://example.invalid/x".to_owned(),
            revision: revision.to_owned(),
            current: current.map(tag),
            latest: Some(tag(latest)),
        }
    }

    #[test]
    fn plans_the_newest_release_for_an_outdated_tag() {
        let bumps = plan(&[status("v0.1.0", Some("v0.1.0"), "v0.2.0")], false);

        assert_eq!(
            bumps,
            [bump("https://example.invalid/x", "v0.1.0", "v0.2.0", None)]
        );
    }

    #[test]
    fn freezing_an_up_to_date_tag_pins_its_commit() {
        let bumps = plan(&[status("v0.2.0", Some("v0.2.0"), "v0.2.0")], true);

        assert_eq!(
            bumps,
            [bump(
                "https://example.invalid/x",
                "v0.2.0",
                &"f".repeat(40),
                Some("v0.2.0")
            )]
        );
    }

    #[test]
    fn leaves_a_branch_and_an_up_to_date_source_alone() {
        let statuses = [
            status("main", None, "v0.2.0"),
            status("v0.2.0", Some("v0.2.0"), "v0.2.0"),
        ];

        assert!(plan(&statuses, false).is_empty());
    }

    #[test]
    fn refuses_an_entry_it_cannot_find() {
        let error = rewrite(
            "repos: [{repo: https://example.invalid/x, revision: v1.0.0, paths: [{path: .}]}]\n",
            &[bump("https://example.invalid/x", "v1.0.0", "v1.1.0", None)],
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("edit it by hand"), "{error}");
    }
}
