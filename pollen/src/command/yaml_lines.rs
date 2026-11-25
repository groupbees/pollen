//! Editing `pollen.yaml` line by line, so comments and layout survive what
//! re-serialising the YAML would lose. `autoupdate` and `migrate` share it.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context as _, Result};
use regex::Regex;

use crate::config::Config;

static ITEM_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)-\s").expect("valid regex"));

/// The `[start, end)` line ranges of the items under `repos:`.
pub(crate) fn entries(lines: &[&str]) -> Vec<(usize, usize)> {
    lines
        .iter()
        .position(|line| line.trim_end().starts_with("repos:"))
        .map_or_else(Vec::new, |repos| items(lines, repos, lines.len(), 0))
}

/// The `[start, end)` line ranges of the items of the block list whose key
/// is on line `header`, at column `key`, looking no further than `limit`.
pub(crate) fn items(
    lines: &[&str],
    header: usize,
    limit: usize,
    key: usize,
) -> Vec<(usize, usize)> {
    let mut item_indent = None;
    let mut starts = Vec::new();
    let mut end = limit;
    for (index, line) in lines.iter().enumerate().take(limit).skip(header + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if let Some(caps) = ITEM_START.captures(line) {
            let this = caps[1].len();
            if item_indent.map_or(this >= key, |known| known == this) {
                item_indent = Some(this);
                starts.push(index);
                continue;
            }
        }
        if item_indent.is_none_or(|known| indent <= known) {
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

/// A rewritten config, written beside the file it replaces and loaded, but
/// not in place yet.
pub(crate) struct Staged {
    file: tempfile::NamedTempFile,
    /// The rewritten file, as pollen reads it.
    pub(crate) config: Config,
}

/// Write `text` beside `path` and load it, so a relative `local` path
/// resolves as it will once the file is in place.
///
/// # Errors
///
/// When the copy cannot be written or is not a valid config.
pub(crate) fn stage(path: &Path, text: &str) -> Result<Staged> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file = tempfile::Builder::new()
        .prefix(".pollen-config-")
        .suffix(".yaml")
        .tempfile_in(directory)
        .with_context(|| format!("cannot write into {}", directory.display()))?;
    std::fs::write(file.path(), text)?;
    let config = Config::load(file.path())
        .context("the rewritten config is invalid; nothing was written")?;
    Ok(Staged { file, config })
}

impl Staged {
    /// Move the rewritten file over `path`, in one rename. Dropping a
    /// `Staged` instead deletes the copy and leaves `path` alone.
    ///
    /// # Errors
    ///
    /// When the rename fails.
    pub(crate) fn persist(self, path: &Path) -> Result<()> {
        self.file
            .persist(path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }
}
