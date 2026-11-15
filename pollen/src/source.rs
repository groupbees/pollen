//! Turning a `repos:` entry into a directory on disk that skills can be read from.

pub mod archive;
pub mod git;
pub mod well_known;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::config::{RepoSpec, Source};
use well_known::{CredentialsFile, WellKnown};

/// A source made available on the local filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Materialized {
    /// Directory the source's files are rooted at.
    pub root: PathBuf,
    /// The commit a git revision resolved to.
    pub resolved: Option<String>,
    /// The skills a `well-known` source selected itself; empty for a source
    /// whose `paths:` select them.
    pub skills: Vec<Fetched>,
}

/// One skill a `well-known` source downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// Its name in the index.
    pub name: String,
    /// Where it was unpacked, a directory named after it.
    pub dir: PathBuf,
    /// The digest of the artifact it was unpacked from.
    pub digest: String,
}

/// Makes a `repos:` entry's files available locally.
#[cfg_attr(test, mockall::automock)]
pub trait Materializer {
    /// Fetch or locate `repo`'s files and return where they live.
    fn materialize(&self, repo: &RepoSpec) -> impl Future<Output = Result<Materialized>> + Send;
}

/// The production [`Materializer`]: a git cache, `well-known` origins, and
/// the config's own directory.
#[derive(Debug, Clone)]
pub struct Fetcher {
    base_dir: PathBuf,
    cache_dir: PathBuf,
    offline: bool,
    well_known: Arc<WellKnown>,
}

impl Fetcher {
    /// Build a fetcher rooted at the config file's directory.
    #[must_use]
    pub fn new(
        base_dir: PathBuf,
        cache_dir: PathBuf,
        offline: bool,
        credentials: CredentialsFile,
    ) -> Self {
        let well_known = Arc::new(WellKnown::new(cache_dir.clone(), offline, credentials));
        Self {
            base_dir,
            cache_dir,
            offline,
            well_known,
        }
    }
}

impl Materializer for Fetcher {
    fn materialize(&self, repo: &RepoSpec) -> impl Future<Output = Result<Materialized>> + Send {
        let source = repo.source();
        let base_dir = self.base_dir.clone();
        let cache_dir = self.cache_dir.clone();
        let offline = self.offline;
        let well_known = Arc::clone(&self.well_known);
        let repo = repo.clone();

        async move {
            match source {
                Source::Local => Ok(Materialized {
                    root: base_dir,
                    resolved: None,
                    skills: Vec::new(),
                }),
                Source::Git { url, revision } => {
                    git::checkout(&url, &revision, &cache_dir, offline).await
                }
                Source::WellKnown { .. } => well_known.materialize(&repo).await,
            }
        }
    }
}

/// The default cache root: the platform cache directory through [`dirs`]
/// (`XDG_CACHE_HOME` on Linux, `%LOCALAPPDATA%` on Windows).
#[must_use]
pub fn default_cache_dir() -> PathBuf {
    dirs::cache_dir().map_or_else(
        || PathBuf::from(".pollen-cache"),
        |cache| cache.join("pollen"),
    )
}

/// Resolve `relative` under `root`, refusing anything that escapes it.
///
/// # Errors
///
/// When either path cannot be canonicalised, or the result leaves `root`.
pub fn contained_join(root: &Path, relative: &Path) -> Result<PathBuf> {
    let joined = root.join(relative);
    let canonical_root = root.canonicalize()?;
    let canonical = joined.canonicalize()?;
    anyhow::ensure!(
        canonical.starts_with(&canonical_root),
        "{} escapes the source root {}",
        relative.display(),
        root.display()
    );
    Ok(canonical)
}

/// `path` relative to `base`, joined with `/` whatever the platform.
///
/// This is the form `exclude` patterns match, the state file records, and the
/// checksum hashes, so none of them depends on the machine that produced it.
#[must_use]
pub fn relative_slug(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_slug_uses_forward_slashes_on_every_platform() {
        let root = Path::new("root");

        assert_eq!(
            relative_slug(root, &root.join("nested").join("demo")),
            "nested/demo"
        );
    }
}
