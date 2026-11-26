use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use regex::Regex;

use crate::cli::Options;
use crate::command::Context;
use crate::command::outdated::{self, DigestStatus, Entry, Status};
use crate::command::yaml_lines::{entries, entries_of, items, stage};
use crate::config::{IndexSelection, Repo, is_commit_id, normalize_origin};
use crate::version::Version;

/// One `revision:` to rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bump {
    pub repo: String,
    pub from: String,
    pub to: String,
    /// The release `to` stands for, written as a trailing comment when `to`
    /// is a commit id.
    pub release: Option<String>,
}

/// One `digest:` of a skill listed under a `well-known` source to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestBump {
    pub origin: String,
    pub skill: String,
    /// The `digest:` it holds now, `None` when it has none.
    pub from: Option<String>,
    /// The digest the index publishes.
    pub to: String,
}

/// What an autoupdate changed, or would change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub path: PathBuf,
    /// One entry per git source moved, empty when every one is up to date.
    pub bumps: Vec<Bump>,
    /// One entry per `well-known` skill whose digest changes.
    pub digests: Vec<DigestBump>,
    /// Whether the file was rewritten: false on a dry run or with no bump.
    pub written: bool,
}

/// # Errors
///
/// When the config cannot be loaded, a remote cannot be reached, or the
/// rewritten file would not be a valid config.
pub async fn run(
    options: &Options,
    freeze: bool,
    dry_run: bool,
    min_age: Option<Duration>,
) -> Result<()> {
    let outcome = apply(options, freeze, dry_run, min_age).await?;
    let path = outcome.path.display();

    if outcome.bumps.is_empty() && outcome.digests.is_empty() {
        println!("{path}: every source is up to date");
        return Ok(());
    }
    for bump in &outcome.bumps {
        println!("{}  {} -> {}", bump.repo, bump.from, written(bump));
    }
    for bump in &outcome.digests {
        println!(
            "{} {}  {} -> {}",
            bump.origin,
            bump.skill,
            bump.from.as_deref().unwrap_or("(none)"),
            bump.to
        );
    }
    if outcome.written {
        let mut counts = Vec::new();
        if !outcome.bumps.is_empty() {
            counts.push(format!("{} source(s)", outcome.bumps.len()));
        }
        if !outcome.digests.is_empty() {
            counts.push(format!("{} skill digest(s)", outcome.digests.len()));
        }
        println!("{path}: {} updated", counts.join(" and "));
    } else {
        println!("{path}: not written (dry run)");
    }
    Ok(())
}

/// Move every git source of the config `options` name to its newest release
/// at least `min_age` old, and every pinned `well-known` skill to the digest
/// its index publishes, rewriting the file unless `dry_run`.
///
/// With `freeze`, git tags become the commits they point to, and the skills
/// listed without a `digest` get the published one. A `well-known` entry
/// that lists no skills has nothing to pin and is left as it is.
///
/// # Errors
///
/// When `options` ask to stay offline, the config cannot be loaded, a remote
/// cannot be reached, or the rewritten file would not be a valid config.
pub async fn apply(
    options: &Options,
    freeze: bool,
    dry_run: bool,
    min_age: Option<Duration>,
) -> Result<Outcome> {
    if options.offline {
        bail!(
            "`autoupdate` asks every remote for its tags and every origin for its index; drop --offline"
        );
    }
    let path = options.config_path();
    let context = Context::open(options)?;
    if freeze {
        for repo in &context.config.repos {
            if let Repo::WellKnown {
                origin,
                skills: IndexSelection::All { .. },
            } = repo
            {
                tracing::warn!(
                    "{origin}: takes every skill of its index, so there is no digest to pin; list the skills under `skills` to pin them"
                );
            }
        }
    }
    let entries = outdated::check(
        &context.config,
        &options.cache_dir(),
        options.credentials_file(),
        min_age,
    )
    .await?;
    let mut releases = Vec::new();
    let mut skills = Vec::new();
    for entry in entries {
        match entry {
            Entry::Release(status) => releases.push(status),
            Entry::Digest(status) => skills.push(status),
        }
    }
    let bumps = plan(&releases, freeze);
    let digests = plan_digests(&skills, freeze);

    let written = (!bumps.is_empty() || !digests.is_empty()) && !dry_run;
    if written {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let text = rewrite_digests(&rewrite(&text, &bumps)?, &digests)?;
        stage(&path, &text)?.persist(&path)?;
    }
    Ok(Outcome {
        path,
        bumps,
        digests,
        written,
    })
}

/// What to change: every pinned skill whose index publishes another digest,
/// and with `freeze` every listed skill without one.
#[must_use]
pub fn plan_digests(statuses: &[DigestStatus], freeze: bool) -> Vec<DigestBump> {
    let mut bumps = Vec::new();
    for status in statuses {
        let Some(published) = &status.published else {
            tracing::warn!(
                "{}: no longer lists `{}`; left as it is",
                status.origin,
                status.skill
            );
            continue;
        };
        let moves = match &status.pinned {
            Some(_) => status.is_outdated(),
            None => freeze,
        };
        if moves {
            bumps.push(DigestBump {
                origin: status.origin.clone(),
                skill: status.skill.clone(),
                from: status.pinned.clone(),
                to: published.clone(),
            });
        }
    }
    bumps
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
static WELL_KNOWN_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(?:-\s+)?well-known:\s*["']?([^"'#\s]+)["']?"#).expect("valid regex")
});
static URL_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(?:-\s+)?url:\s*["']?([^"'#\s]+)["']?"#).expect("valid regex")
});
static SKILLS_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*(?:-\s+)?)skills:\s*(?:#.*)?$").expect("valid regex"));
static NAME_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\s*(?:-\s+)?)name:\s*["']?([^"'#\s]+)["']?"#).expect("valid regex")
});
static DIGEST_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\s*(?:-\s+)?digest:\s*)["']?[^"'#\s]+["']?(\s*#.*)?$"#).expect("valid regex")
});

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

/// Apply `bumps` to the text of a `pollen.yaml`, line by line, as
/// [`rewrite`] does: a `digest:` line is replaced, keeping its comment, or
/// inserted under the skill's `name:` when it has none.
///
/// # Errors
///
/// When a skill to bump cannot be found, e.g. in YAML flow style.
pub fn rewrite_digests(text: &str, bumps: &[DigestBump]) -> Result<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    let mut applied = vec![false; bumps.len()];

    // A `well-known` entry of `repos` in version 1, a `discovery` one in
    // version 2.
    let origins = entries(&lines)
        .into_iter()
        .map(|range| (range, &*WELL_KNOWN_LINE))
        .chain(
            entries_of(&lines, "discovery:")
                .into_iter()
                .map(|range| (range, &*URL_LINE)),
        );
    for ((start, end), key) in origins {
        let origin = lines[start..end].iter().find_map(|line| {
            key.captures(line)
                .and_then(|caps| normalize_origin(&caps[1]))
        });
        let Some(origin) = origin else { continue };
        let Some((header, key)) = (start..end).find_map(|index| {
            SKILLS_LINE
                .captures(lines[index])
                .map(|caps| (index, caps[1].len()))
        }) else {
            continue;
        };
        for (first, last) in items(&lines, header, end, key) {
            let Some((name_index, name, column)) = (first..last).find_map(|index| {
                NAME_LINE
                    .captures(lines[index])
                    .map(|caps| (index, caps[2].to_owned(), caps[1].len()))
            }) else {
                continue;
            };
            let Some((which, bump)) = bumps
                .iter()
                .enumerate()
                .find(|(_, bump)| bump.origin == origin && bump.skill == name)
            else {
                continue;
            };
            let digest = (first..last).find_map(|index| {
                let body = lines[index].trim_end_matches(['\r', '\n']);
                DIGEST_LINE
                    .captures(body)
                    .map(|caps| (index, body.len(), caps))
            });
            if let Some((index, length, caps)) = digest {
                let line = lines[index];
                let comment = caps.get(2).map_or("", |comment| comment.as_str());
                out[index] = format!("{}{}{comment}{}", &caps[1], bump.to, &line[length..]);
            } else {
                let line = lines[name_index];
                let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
                let ending = if ending.is_empty() { "\n" } else { ending };
                out[name_index] = format!(
                    "{}{ending}{}digest: {}{ending}",
                    line.trim_end_matches(['\r', '\n']),
                    " ".repeat(column),
                    bump.to
                );
            }
            applied[which] = true;
        }
    }

    if let Some(missing) = bumps.iter().zip(&applied).find(|(_, done)| !**done) {
        bail!(
            "cannot find the skill `{}` of {} to rewrite its digest; edit it by hand",
            missing.0.skill,
            missing.0.origin
        );
    }
    Ok(out.concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

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
    fn bumps_a_revision_and_a_digest_in_a_version_two_file() {
        let text = format!(
            "version: 2\nrepos:\n  - repo: https://x.example/a.git\n    revision: v0.1.0\n    skills:\n      - path: skills\ndiscovery:\n  - url: https://gateway.example.com\n    skills:\n      - name: pinned\n        digest: {OLD}\n"
        );

        let out = rewrite_digests(
            &rewrite(
                &text,
                &[bump("https://x.example/a.git", "v0.1.0", "v0.2.0", None)],
            )
            .unwrap(),
            &[digest_bump("pinned", Some(OLD))],
        )
        .unwrap();

        assert_eq!(out, text.replace("v0.1.0", "v0.2.0").replace(OLD, NEW));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pollen.yaml");
        std::fs::write(&path, &out).unwrap();
        assert_eq!(Config::load(&path).unwrap().version, 2);
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

    const OLD: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    const NEW: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";

    fn digest_bump(skill: &str, from: Option<&str>) -> DigestBump {
        DigestBump {
            origin: "https://gateway.example.com".to_owned(),
            skill: skill.to_owned(),
            from: from.map(str::to_owned),
            to: NEW.to_owned(),
        }
    }

    const WELL_KNOWN: &str = "\
repos:
  - repo: local
    paths:
      - path: skills
  - well-known: https://gateway.example.com/  # the gateway
    skills:
      - name: pinned
        digest: \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"  # keep
      - name: loose
      - digest: sha256:0000000000000000000000000000000000000000000000000000000000000000
        name: reversed
";

    #[test]
    fn replaces_a_digest_and_inserts_a_missing_one() {
        let out = rewrite_digests(
            WELL_KNOWN,
            &[
                digest_bump("pinned", Some(OLD)),
                digest_bump("loose", None),
                digest_bump("reversed", Some(OLD)),
            ],
        )
        .unwrap();

        assert_eq!(
            out,
            WELL_KNOWN
                .replace(&format!("\"{OLD}\"  # keep"), &format!("{NEW}  # keep"))
                .replace(
                    "      - name: loose\n",
                    &format!("      - name: loose\n        digest: {NEW}\n")
                )
                .replace(
                    &format!("      - digest: {OLD}"),
                    &format!("      - digest: {NEW}")
                )
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pollen.yaml");
        std::fs::write(&path, &out).unwrap();
        Config::load(&path).unwrap();
    }

    #[test]
    fn inserts_a_digest_with_windows_line_endings() {
        let text = "repos:\r\n  - well-known: https://gateway.example.com\r\n    skills:\r\n      - name: loose\r\n";

        let out = rewrite_digests(text, &[digest_bump("loose", None)]).unwrap();

        assert_eq!(
            out,
            format!(
                "repos:\r\n  - well-known: https://gateway.example.com\r\n    skills:\r\n      - name: loose\r\n        digest: {NEW}\r\n"
            )
        );
    }

    #[test]
    fn refuses_a_skill_it_cannot_find() {
        let error = rewrite_digests(
            "repos: [{well-known: https://gateway.example.com, skills: [{name: a}]}]\n",
            &[digest_bump("a", None)],
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("edit it by hand"), "{error}");
    }

    fn digest_status(
        pinned: Option<&str>,
        published: Option<&str>,
        held_back: bool,
    ) -> DigestStatus {
        DigestStatus {
            origin: "https://gateway.example.com".to_owned(),
            skill: "a".to_owned(),
            pinned: pinned.map(str::to_owned),
            published: published.map(str::to_owned),
            held_back,
        }
    }

    #[test]
    fn plans_a_changed_digest_and_with_freeze_a_missing_one() {
        let statuses = [
            digest_status(Some(OLD), Some(NEW), false),
            digest_status(Some(NEW), Some(NEW), false),
            digest_status(Some(OLD), Some(NEW), true),
            digest_status(Some(OLD), None, false),
            digest_status(None, Some(NEW), false),
        ];

        assert_eq!(
            plan_digests(&statuses, false),
            [digest_bump("a", Some(OLD))]
        );
        assert_eq!(
            plan_digests(&statuses, true),
            [digest_bump("a", Some(OLD)), digest_bump("a", None)]
        );
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
            pending: None,
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
