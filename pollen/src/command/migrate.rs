use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use regex::Regex;

use crate::cli::Options;
use crate::command::yaml_lines::{entries, items, stage};
use crate::config::{self, Config, LATEST};

/// What a migration changed, or would change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub path: PathBuf,
    pub from_version: u32,
    pub to_version: u32,
    /// Whether the file was rewritten: false on a dry run, or when it was
    /// already in the newest version.
    pub written: bool,
}

/// # Errors
///
/// See [`apply`].
pub fn run(options: &Options, dry_run: bool) -> Result<()> {
    let migration = apply(options, dry_run)?;
    let path = migration.path.display();
    let (from, to) = (migration.from_version, migration.to_version);
    if from == to {
        println!("{path}: already in version {to}");
    } else if migration.written {
        println!("{path}: migrated from version {from} to version {to}");
    } else {
        println!("{path}: would migrate from version {from} to version {to} (dry run)");
    }
    Ok(())
}

/// Rewrite the config `options` name in the newest format version, unless
/// `dry_run`.
///
/// The file is edited line by line, so its comments and layout survive, and
/// written only if the result declares the same sources and destinations as
/// the original.
///
/// # Errors
///
/// When the config cannot be read or is invalid, or a line to rewrite cannot
/// be found, e.g. in YAML flow style.
pub fn apply(options: &Options, dry_run: bool) -> Result<Migration> {
    let path = options.config_path();
    let original = Config::load(&path).context("fix the config before migrating it")?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let from_version = config::version_of(&text)?;
    if from_version == LATEST {
        return Ok(Migration {
            path,
            from_version,
            to_version: LATEST,
            written: false,
        });
    }

    let migrated = to_v2(&text)?;
    let staged = stage(&path, &migrated)?;
    ensure!(
        staged.config.repos == original.repos && staged.config.clients == original.clients,
        "the migrated config would declare other sources or destinations than {}; nothing was written",
        path.display()
    );
    if !dry_run {
        staged.persist(&path)?;
    }
    Ok(Migration {
        path,
        from_version,
        to_version: LATEST,
        written: !dry_run,
    })
}

static PATHS_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*(?:-\s+)?)paths:").expect("valid regex"));
static REPO_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:-\s+)?repo:").expect("valid regex"));
static WELL_KNOWN_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*(?:-\s+)?)well-known:").expect("valid regex"));
static REPOS_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^repos:").expect("valid regex"));
static REPOS_KEY_IN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^repos:").expect("valid regex"));
static VERSION_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^version:\s*[^#\s]+(.*)$").expect("valid regex"));
static TARGETS_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^targets:").expect("valid regex"));

/// The text of a version 1 `pollen.yaml`, said in version 2: the top-level
/// `targets:` block, with the comments right above it, goes on every item of
/// a `repo`'s `paths:` and on every `well-known` entry, `paths:` becomes
/// `skills:` in every `repo` entry, every `well-known` entry moves, with the
/// comments right above it, to a `discovery:` list after `repos:` and calls
/// its origin `url:`, and `version: 2` is set.
///
/// # Errors
///
/// When an entry's keys or the place for `version` cannot be found.
pub fn to_v2(text: &str) -> Result<String> {
    let text = &move_targets(text)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    let by_hand = || {
        anyhow!(
            "cannot find the keys of every `repos` entry, e.g. in YAML flow style; migrate by hand: {BY_HAND}"
        )
    };

    let entries = entries(&lines);
    let mut moved = vec![false; entries.len()];
    for (which, &(start, end)) in entries.iter().enumerate() {
        if lines[start..end].iter().any(|line| REPO_KEY.is_match(line)) {
            let index = (start..end)
                .find(|&index| PATHS_LINE.is_match(lines[index]))
                .ok_or_else(by_hand)?;
            out[index] = PATHS_LINE.replace(lines[index], "${1}skills:").into_owned();
        } else {
            let index = (start..end)
                .find(|&index| WELL_KNOWN_KEY.is_match(lines[index]))
                .ok_or_else(by_hand)?;
            out[index] = WELL_KNOWN_KEY
                .replace(lines[index], "${1}url:")
                .into_owned();
            moved[which] = true;
        }
    }
    if entries.is_empty() {
        return Err(by_hand());
    }

    set_version(&lines, &mut out)?;
    if moved.iter().any(|moved| *moved) {
        out = to_discovery(&lines, out, &entries, &moved)?;
    }
    Ok(out.concat())
}

/// The steps of a migration, for an error asking to do it by hand.
const BY_HAND: &str = "copy the top-level `targets:` into each item of `paths:` and each `well-known:` entry, rename each `paths:` to `skills:`, move each `well-known:` entry under `discovery:` as `url:`, and add `version: 2`";

/// Remove the top-level `targets:` block, with the comments right above it
/// unless they open the file, and write a copy of it as the last key of every
/// item of a `repo`'s `paths:` and of every `well-known` entry, since version
/// 2 names the destination on each entry.
fn move_targets(text: &str) -> Result<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some((start, end)) = targets_block(&lines) else {
        return Ok(text.to_owned());
    };
    let ending = if lines[0].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let block: Vec<String> = lines[start..end]
        .iter()
        .map(|line| {
            let line = line.trim_start_matches('\u{feff}');
            if line.ends_with('\n') {
                line.to_owned()
            } else {
                format!("{line}{ending}")
            }
        })
        .collect();
    let indented = |column: usize| -> String {
        block
            .iter()
            .map(|line| {
                if line.trim().is_empty() {
                    line.clone()
                } else {
                    format!("{}{line}", " ".repeat(column))
                }
            })
            .collect()
    };

    let places = destination_places(&lines)?;
    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    for line in &mut out[start..end] {
        line.clear();
    }
    if lines[start].starts_with('\u{feff}') {
        out[start].push('\u{feff}');
    }
    // The blank line that set the block apart goes with it, unless the
    // block was between two paragraphs.
    let blank = |index: usize| lines.get(index).is_none_or(|line| line.trim().is_empty());
    if start > 0 && blank(start - 1) && blank(end) {
        out[start - 1].clear();
    }
    for (at, column) in places {
        if !out[at].ends_with('\n') {
            out[at].push_str(ending);
        }
        out[at].push_str(&indented(column));
    }
    Ok(out.concat())
}

/// The `[start, end)` lines of the top-level `targets:` block, with the
/// comments right above it unless they open the file.
fn targets_block(lines: &[&str]) -> Option<(usize, usize)> {
    let key = lines
        .iter()
        .position(|line| TARGETS_LINE.is_match(line.trim_start_matches('\u{feff}')))?;
    let mut end = key + 1;
    for (index, line) in lines.iter().enumerate().skip(key + 1) {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t', '-']) {
            break;
        }
        end = index + 1;
    }
    let mut start = key;
    while start > 0 && lines[start - 1].starts_with('#') {
        start -= 1;
    }
    Some((if start == 0 { key } else { start }, end))
}

/// Where each entry's destination goes, as a line to write after and a
/// column: after the last line of each item of a `repo`'s `paths:` and of
/// each `well-known` entry that is neither blank nor a comment, at the
/// column of that item's keys.
fn destination_places(lines: &[&str]) -> Result<Vec<(usize, usize)>> {
    let by_hand = || {
        anyhow!(
            "cannot find where each entry's keys go, e.g. in YAML flow style; migrate by hand: {BY_HAND}"
        )
    };
    let last_content = |from: usize, to: usize| {
        (from..to).rev().find(|&index| {
            let trimmed = lines[index].trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
    };
    let entries = entries(lines);
    if entries.is_empty() {
        return Err(by_hand());
    }
    let mut places = Vec::new();
    for (first, last) in entries {
        if !lines[first..last]
            .iter()
            .any(|line| REPO_KEY.is_match(line))
        {
            let at = last_content(first, last).ok_or_else(by_hand)?;
            places.push((at, key_column(lines[first])));
            continue;
        }
        let (header, column) = (first..last)
            .find_map(|index| {
                PATHS_LINE
                    .captures(lines[index])
                    .map(|caps| (index, caps[1].len()))
            })
            .ok_or_else(by_hand)?;
        let paths = items(lines, header, last, column);
        if paths.is_empty() {
            return Err(by_hand());
        }
        for (from, to) in paths {
            let at = last_content(from, to).ok_or_else(by_hand)?;
            places.push((at, key_column(lines[from])));
        }
    }
    Ok(places)
}

/// The column of the keys of the list item starting on `line`: just after
/// its `- `.
fn key_column(line: &str) -> usize {
    let dash = line.find('-').unwrap_or(0);
    let after = &line[dash + 1..];
    dash + 1 + after.len() - after.trim_start().len()
}

/// Move the `repos` entries `moved` marks under a `discovery:` key after
/// `repos:`, or rename `repos:` itself when every entry moves.
fn to_discovery(
    lines: &[&str],
    mut out: Vec<String>,
    entries: &[(usize, usize)],
    moved: &[bool],
) -> Result<Vec<String>> {
    let header = lines
        .iter()
        .position(|line| REPOS_LINE.is_match(line))
        .ok_or_else(|| anyhow!("cannot find the `repos:` key; migrate by hand"))?;
    let ending = if lines[header].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if moved.iter().all(|moved| *moved) {
        // `version: 2` may have gone into the same line, above the key.
        out[header] = REPOS_KEY_IN
            .replace(&out[header], "discovery:")
            .into_owned();
        return Ok(out);
    }

    // Each entry with the comments right above it, which describe it.
    let leads: Vec<usize> = entries
        .iter()
        .map(|&(start, _)| {
            let mut lead = start;
            while lead > header + 1 && lines[lead - 1].trim_start().starts_with('#') {
                lead -= 1;
            }
            lead
        })
        .collect();
    // Blank lines closing the list stay after it, after `discovery:` now.
    let mut region_end = entries.last().map_or(header + 1, |&(_, end)| end);
    while region_end > leads[leads.len() - 1] + 1 && lines[region_end - 1].trim().is_empty() {
        region_end -= 1;
    }
    let mut kept = Vec::new();
    let mut discovery = Vec::new();
    for (which, &lead) in leads.iter().enumerate() {
        let end = leads.get(which + 1).copied().unwrap_or(region_end);
        let block = if moved[which] {
            &mut discovery
        } else {
            &mut kept
        };
        block.extend(out[lead..end].iter().cloned());
    }
    for line in kept.iter_mut().chain(discovery.iter_mut()) {
        if !line.ends_with('\n') {
            line.push_str(ending);
        }
    }

    let mut rewritten: Vec<String> = out[..leads[0]].to_vec();
    rewritten.extend(kept);
    rewritten.push(format!("discovery:{ending}"));
    rewritten.extend(discovery);
    rewritten.extend(out[region_end..].iter().cloned());
    Ok(rewritten)
}

/// Replace a top-level `version:` line, or insert one before the first
/// top-level key, after the leading comments and document marker.
fn set_version<'a>(lines: &[&'a str], out: &mut [String]) -> Result<()> {
    // Each line split into its byte order mark, its body and its ending.
    let parts = |line: &'a str| -> (&'a str, &'a str, &'a str) {
        let content = line.trim_end_matches(['\r', '\n']);
        let body = content.trim_start_matches('\u{feff}');
        (
            &line[..content.len() - body.len()],
            body,
            &line[content.len()..],
        )
    };
    if let Some(index) = lines
        .iter()
        .position(|line| VERSION_LINE.is_match(parts(line).1))
    {
        let (bom, body, ending) = parts(lines[index]);
        let comment = VERSION_LINE
            .captures(body)
            .map_or("", |caps| caps.get(1).map_or("", |m| m.as_str()));
        out[index] = format!("{bom}version: {LATEST}{comment}{ending}");
        return Ok(());
    }
    let Some(index) = lines.iter().position(|line| {
        let body = parts(line).1;
        !body.trim().is_empty()
            && !body.starts_with('#')
            && body.trim_end() != "---"
            && !body.starts_with([' ', '\t'])
    }) else {
        bail!("cannot find where to add `version: {LATEST}`; add it by hand");
    };
    let ending = if lines[index].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    // The comments right above the first key describe it: go above them,
    // unless they open the file.
    let mut first = index;
    while first > 0 && parts(lines[first - 1]).1.starts_with('#') {
        first -= 1;
    }
    let (at, gap) = if first > 0 && parts(lines[first - 1]).1.trim().is_empty() {
        (first, ending)
    } else {
        (index, "")
    };
    let (bom, _, _) = parts(lines[at]);
    out[at] = format!(
        "{bom}version: {LATEST}{ending}{gap}{}",
        &lines[at][bom.len()..]
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(path: &std::path::Path) -> Options {
        Options {
            config: path.to_path_buf(),
            ..Options::default()
        }
    }

    #[test]
    fn renames_paths_and_sets_the_version_keeping_comments() {
        let text = "# yaml-language-server: $schema=./schema.json\n# Skills.\ntargets:\n  - .claude/skills\nrepos:\n  # The team's.\n  - repo: https://x.example/a.git\n    revision: v1.0.0  # pinned\n    paths:  # where\n      - path: skills\n        recurse: true\n  - well-known: https://x.example\n    skills:\n      - name: a\n  - paths:\n      - path: local-skills\n    repo: local\n";

        assert_eq!(
            to_v2(text).unwrap(),
            "# yaml-language-server: $schema=./schema.json\n# Skills.\nversion: 2\nrepos:\n  # The team's.\n  - repo: https://x.example/a.git\n    revision: v1.0.0  # pinned\n    skills:  # where\n      - path: skills\n        recurse: true\n        targets:\n          - .claude/skills\n  - skills:\n      - path: local-skills\n        targets:\n          - .claude/skills\n    repo: local\ndiscovery:\n  - url: https://x.example\n    skills:\n      - name: a\n    targets:\n      - .claude/skills\n"
        );
    }

    #[test]
    fn moves_well_known_entries_with_their_comments_to_discovery() {
        let text = "repos:  # sources\n  # A gateway.\n  - well-known: https://g.example  # SSO\n    exclude: ^beta-\n\n  # Ours.\n  - repo: local\n    paths:\n      - path: .\n\n  - skills:\n      - name: a\n    well-known: https://h.example\n\ntargets:\n  - out\n";

        assert_eq!(
            to_v2(text).unwrap(),
            "version: 2\nrepos:  # sources\n  # Ours.\n  - repo: local\n    skills:\n      - path: .\n        targets:\n          - out\n\ndiscovery:\n  # A gateway.\n  - url: https://g.example  # SSO\n    exclude: ^beta-\n    targets:\n      - out\n\n  - skills:\n      - name: a\n    url: https://h.example\n    targets:\n      - out\n"
        );
    }

    #[test]
    fn renames_repos_when_every_entry_is_well_known() {
        assert_eq!(
            to_v2("repos:\r\n  - well-known: https://g.example\r\n").unwrap(),
            "version: 2\r\ndiscovery:\r\n  - url: https://g.example\r\n"
        );
    }

    #[test]
    fn migrates_a_well_known_file_to_an_equal_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pollen.yaml");
        std::fs::write(
            &path,
            "repos:\n  - well-known: https://g.example/\n    skills:\n      - name: a\n  - repo: local\n    paths:\n      - path: .\n",
        )
        .unwrap();
        let before = Config::load(&path).unwrap();

        apply(&options(&path), false).unwrap();

        assert_eq!(Config::load(&path).unwrap().repos, before.repos);
    }

    #[test]
    fn goes_above_the_comments_that_describe_the_first_key() {
        assert_eq!(
            to_v2("# A config.\n\n# Where they come from.\nrepos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap(),
            "# A config.\n\nversion: 2\n\n# Where they come from.\nrepos:\n  - repo: local\n    skills:\n      - path: .\n"
        );
    }

    #[test]
    fn moves_the_targets_with_their_comments_onto_every_entry() {
        assert_eq!(
            to_v2("# A config.\n\n# Where they go.\ntargets:  # both\n- out  # ours\n# and theirs\n- ~/shared\n\nrepos:\n- repo: local\n  paths:\n  - path: a\n    recurse: true\n  # The second.\n  - path: b\n- well-known: https://g.example\n").unwrap(),
            "# A config.\n\nversion: 2\n\nrepos:\n- repo: local\n  skills:\n  - path: a\n    recurse: true\n    # Where they go.\n    targets:  # both\n    - out  # ours\n    # and theirs\n    - ~/shared\n  # The second.\n  - path: b\n    # Where they go.\n    targets:  # both\n    - out  # ours\n    # and theirs\n    - ~/shared\ndiscovery:\n- url: https://g.example\n  # Where they go.\n  targets:  # both\n  - out  # ours\n  # and theirs\n  - ~/shared\n"
        );
    }

    #[test]
    fn moves_flow_style_targets_and_keeps_line_endings() {
        assert_eq!(
            to_v2("repos:\r\n  - repo: local\r\n    paths:\r\n      - path: .\r\n\r\ntargets: [out, other]\r\n").unwrap(),
            "version: 2\r\nrepos:\r\n  - repo: local\r\n    skills:\r\n      - path: .\r\n        targets: [out, other]\r\n"
        );
        assert_eq!(
            to_v2("\u{feff}targets: [out]\nrepos:\n  - repo: local\n    paths:\n      - path: .")
                .unwrap(),
            "\u{feff}version: 2\nrepos:\n  - repo: local\n    skills:\n      - path: .\n        targets: [out]\n"
        );
    }

    #[test]
    fn refuses_targets_it_cannot_place() {
        let error = to_v2("targets: [out]\nrepos:\n  - repo: local\n    paths: [{path: .}]\n")
            .unwrap_err()
            .to_string();

        assert!(error.contains("by hand"), "{error}");
    }

    #[test]
    fn migrates_targets_to_a_config_with_the_same_destinations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pollen.yaml");
        std::fs::write(
            &path,
            "targets:\n  - out\n  - ~/shared\nrepos:\n  - repo: local\n    paths:\n      - path: a\n      - path: b\n  - well-known: https://g.example\n    skills:\n      - name: x\n",
        )
        .unwrap();
        let before = Config::load(&path).unwrap();

        apply(&options(&path), false).unwrap();
        let after = Config::load(&path).unwrap();

        assert_eq!(after.version, 2);
        assert_eq!(after.repos, before.repos);
        assert!(
            after
                .repos
                .iter()
                .flat_map(crate::config::Repo::destinations)
                .all(|destination| matches!(destination, crate::config::Destination::Targets(dirs) if dirs.len() == 2))
        );
    }

    #[test]
    fn replaces_an_explicit_version_one() {
        assert_eq!(
            to_v2(
                "version: 1  # the default\nrepos:\n  - repo: local\n    paths:\n      - path: .\n"
            )
            .unwrap(),
            "version: 2  # the default\nrepos:\n  - repo: local\n    skills:\n      - path: .\n"
        );
    }

    #[test]
    fn keeps_windows_line_endings_a_byte_order_mark_and_a_document_marker() {
        assert_eq!(
            to_v2("\u{feff}---\r\nrepos:\r\n  - repo: local\r\n    paths:\r\n      - path: .\r\n")
                .unwrap(),
            "\u{feff}---\r\nversion: 2\r\nrepos:\r\n  - repo: local\r\n    skills:\r\n      - path: .\r\n"
        );
    }

    #[test]
    fn refuses_a_flow_style_entry_it_cannot_rewrite() {
        let error = to_v2("repos: [{repo: local, paths: [{path: .}]}]\n")
            .unwrap_err()
            .to_string();

        assert!(error.contains("by hand"), "{error}");
    }

    #[test]
    fn migrates_a_file_to_an_equal_config_and_leaves_a_migrated_one_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pollen.yaml");
        std::fs::write(
            &path,
            "repos:\n  - repo: local\n    paths:\n      - path: skills\n        exclude: ^draft-\n",
        )
        .unwrap();
        let before = Config::load(&path).unwrap();

        let first = apply(&options(&path), false).unwrap();
        let after = Config::load(&path).unwrap();
        let second = apply(&options(&path), false).unwrap();

        assert_eq!(
            (first.from_version, first.to_version, first.written),
            (1, 2, true)
        );
        assert_eq!(after.version, 2);
        assert_eq!(after.repos, before.repos);
        assert_eq!((second.from_version, second.written), (2, false));
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pollen.yaml");
        let text = "repos:\n  - repo: local\n    paths:\n      - path: .\n";
        std::fs::write(&path, text).unwrap();

        let migration = apply(&options(&path), true).unwrap();

        assert_eq!((migration.to_version, migration.written), (2, false));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn refuses_an_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pollen.yaml");
        std::fs::write(&path, "repos: []\n").unwrap();

        assert!(apply(&options(&path), false).is_err());
    }
}
