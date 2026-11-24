//! `pollen migrate`: rewrite `pollen.yaml` in the newest format version, in
//! place.

use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::{Context as _, Result, bail, ensure};
use regex::Regex;

use crate::cli::Options;
use crate::command::yaml_lines::{entries, write_checked};
use crate::config::{self, Config, LATEST};

/// What a migration changed, or would change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// The config file.
    pub path: PathBuf,
    /// The format version the file was written in.
    pub from_version: u32,
    /// The format version it is in after the migration.
    pub to_version: u32,
    /// Whether the file was rewritten: false on a dry run, or when it was
    /// already in the newest version.
    pub written: bool,
}

/// Run the subcommand.
///
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
/// written only if the result declares the same sources and targets as the
/// original.
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
    write_checked(&path, &migrated, dry_run, |config| {
        ensure!(
            config.repos == original.repos && config.targets == original.targets,
            "the migrated config would declare other sources than {}; nothing was written",
            path.display()
        );
        Ok(())
    })?;
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
static VERSION_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^version:\s*[^#\s]+(.*)$").expect("valid regex"));

/// The text of a version 1 `pollen.yaml`, said in version 2: `paths:` becomes
/// `skills:` in every `repo` entry, and `version: 2` is set.
///
/// # Errors
///
/// When an entry's `paths:` key or the place for `version` cannot be found.
pub fn to_v2(text: &str) -> Result<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();

    let mut repo_entries = 0;
    let mut renamed = 0;
    for (start, end) in entries(&lines) {
        if !lines[start..end].iter().any(|line| REPO_KEY.is_match(line)) {
            continue;
        }
        repo_entries += 1;
        if let Some(index) = (start..end).find(|&index| PATHS_LINE.is_match(lines[index])) {
            out[index] = PATHS_LINE.replace(lines[index], "${1}skills:").into_owned();
            renamed += 1;
        }
    }
    if repo_entries == 0 || renamed != repo_entries {
        bail!(
            "cannot find the `paths:` key of every `repo` entry, e.g. in YAML flow style; migrate by hand: rename each `paths:` to `skills:` and add `version: {LATEST}`"
        );
    }

    set_version(&lines, &mut out)?;
    Ok(out.concat())
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
            "# yaml-language-server: $schema=./schema.json\n# Skills.\nversion: 2\ntargets:\n  - .claude/skills\nrepos:\n  # The team's.\n  - repo: https://x.example/a.git\n    revision: v1.0.0  # pinned\n    skills:  # where\n      - path: skills\n        recurse: true\n  - well-known: https://x.example\n    skills:\n      - name: a\n  - skills:\n      - path: local-skills\n    repo: local\n"
        );
    }

    #[test]
    fn goes_above_the_comments_that_describe_the_first_key() {
        assert_eq!(
            to_v2("# A config.\n\n# Where they go.\ntargets:\n  - out\nrepos:\n  - repo: local\n    paths:\n      - path: .\n").unwrap(),
            "# A config.\n\nversion: 2\n\n# Where they go.\ntargets:\n  - out\nrepos:\n  - repo: local\n    skills:\n      - path: .\n"
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
