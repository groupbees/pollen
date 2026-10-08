//! Release tags: which of a remote's tags are versions, and which is newest.

use std::collections::BTreeMap;

/// A stable `X.Y.Z` version, read from a tag with an optional `v` prefix.
///
/// Pre-releases (`v1.2.0-rc1`) are not versions here: `outdated` and
/// `autoupdate` only ever propose a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    /// Incremented on breaking changes.
    pub major: u64,
    /// Incremented on additions.
    pub minor: u64,
    /// Incremented on fixes.
    pub patch: u64,
}

impl Version {
    /// Read `v1.2.3` or `1.2.3`; anything else is not a version.
    #[must_use]
    pub fn parse(tag: &str) -> Option<Self> {
        let digits = tag.strip_prefix('v').unwrap_or(tag);
        let mut parts = digits.split('.');
        let (major, minor, patch) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major: number(major)?,
            minor: number(minor)?,
            patch: number(patch)?,
        })
    }
}

fn number(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// A tag on a remote, and the commit it points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTag {
    /// The tag name, without `refs/tags/`.
    pub name: String,
    /// The commit the tag points to, annotated tags peeled.
    pub commit: String,
}

/// Read the output of `git ls-remote --tags`.
///
/// An annotated tag is listed twice, the tag object then the commit as
/// `name^{}`; the peeled line wins, so `commit` is always a commit.
#[must_use]
pub fn parse_ls_remote(output: &str) -> Vec<RemoteTag> {
    let mut tags: BTreeMap<String, (String, bool)> = BTreeMap::new();
    for line in output.lines() {
        let Some((sha, reference)) = line.split_once('\t') else {
            continue;
        };
        let Some(name) = reference.strip_prefix("refs/tags/") else {
            continue;
        };
        match name.strip_suffix("^{}") {
            Some(name) => {
                tags.insert(name.to_owned(), (sha.to_owned(), true));
            }
            None => {
                tags.entry(name.to_owned())
                    .or_insert_with(|| (sha.to_owned(), false));
            }
        }
    }
    tags.into_iter()
        .map(|(name, (commit, _))| RemoteTag { name, commit })
        .collect()
}

/// The newest stable version among `tags`.
#[must_use]
pub fn latest(tags: &[RemoteTag]) -> Option<&RemoteTag> {
    newest(tags.iter())
}

/// The tag `revision` stands for: itself when it is a tag, else the newest
/// version tag on that commit.
#[must_use]
pub fn current<'a>(revision: &str, tags: &'a [RemoteTag]) -> Option<&'a RemoteTag> {
    if let Some(tag) = tags.iter().find(|tag| tag.name == revision) {
        return Some(tag);
    }
    newest(
        tags.iter()
            .filter(|tag| tag.commit.eq_ignore_ascii_case(revision)),
    )
}

fn newest<'a>(tags: impl Iterator<Item = &'a RemoteTag>) -> Option<&'a RemoteTag> {
    tags.filter_map(|tag| Version::parse(&tag.name).map(|version| (version, tag)))
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, commit: &str) -> RemoteTag {
        RemoteTag {
            name: name.to_owned(),
            commit: commit.to_owned(),
        }
    }

    #[test]
    fn reads_a_version_with_or_without_the_v_prefix() {
        let expected = Some(Version {
            major: 1,
            minor: 12,
            patch: 3,
        });

        assert_eq!(Version::parse("v1.12.3"), expected);
        assert_eq!(Version::parse("1.12.3"), expected);
    }

    #[test]
    fn a_pre_release_or_a_name_is_not_a_version() {
        for tag in [
            "v1.2.0-rc1",
            "v1.2",
            "v1.2.3.4",
            "latest",
            "v1..3",
            "v+1.2.3",
        ] {
            assert_eq!(Version::parse(tag), None, "{tag}");
        }
    }

    #[test]
    fn versions_order_numerically() {
        assert!(Version::parse("v0.10.0") > Version::parse("v0.9.9"));
    }

    #[test]
    fn peels_annotated_tags_to_their_commit() {
        let output = "aaaa\trefs/tags/v1.0.0\nbbbb\trefs/tags/v1.0.0^{}\ncccc\trefs/tags/v1.1.0\n";

        assert_eq!(
            parse_ls_remote(output),
            [tag("v1.0.0", "bbbb"), tag("v1.1.0", "cccc")]
        );
    }

    #[test]
    fn the_latest_is_the_highest_stable_version() {
        let tags = [
            tag("v0.9.0", "a"),
            tag("v0.10.0", "b"),
            tag("v0.11.0-rc1", "c"),
            tag("nightly", "d"),
        ];

        assert_eq!(latest(&tags).unwrap().name, "v0.10.0");
    }

    #[test]
    fn the_current_tag_of_a_commit_is_its_newest_version() {
        let tags = [
            tag("v1.0.0", "abc"),
            tag("v1.0.1", "abc"),
            tag("v1.1.0", "def"),
        ];

        assert_eq!(current("v1.0.0", &tags).unwrap().name, "v1.0.0");
        assert_eq!(current("ABC", &tags).unwrap().name, "v1.0.1");
        assert_eq!(current("main", &tags), None);
    }
}
