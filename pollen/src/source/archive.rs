//! Unpacking a `well-known` skill archive, `.tar.gz` or `.zip`, without
//! letting it write outside its directory or fill the disk.

use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use reqwest::Url;

/// The most bytes the files of one archive may unpack to.
pub const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;

/// The most entries one archive may hold.
const MAX_ENTRIES: usize = 10_000;

/// The archive formats the discovery specification requires clients to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A gzip-compressed tar.
    TarGz,
    /// A zip.
    Zip,
}

impl Format {
    /// The format the server's `Content-Type` names, else the one the URL's
    /// extension names, as the specification orders them.
    #[must_use]
    pub fn detect(content_type: Option<&str>, url: &Url) -> Option<Self> {
        let from_header = match content_type {
            Some(
                "application/gzip"
                | "application/x-gzip"
                | "application/tar+gzip"
                | "application/x-tar+gzip"
                | "application/x-compressed-tar",
            ) => Some(Self::TarGz),
            Some("application/zip" | "application/x-zip-compressed") => Some(Self::Zip),
            _ => None,
        };
        from_header.or_else(|| {
            let path = Path::new(url.path());
            let is = |path: &Path, wanted: &str| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
            };
            let tarred = path
                .file_stem()
                .is_some_and(|stem| is(Path::new(stem), "tar"));
            if is(path, "tgz") || (is(path, "gz") && tarred) {
                Some(Self::TarGz)
            } else if is(path, "zip") {
                Some(Self::Zip)
            } else {
                None
            }
        })
    }
}

/// Unpack `bytes` into the empty directory `destination`.
///
/// File modes are kept where the archive records them, on Unix. A link
/// resolving inside the directory is left out, as pollen deploys regular
/// files only; one resolving outside refuses the archive.
///
/// # Errors
///
/// When the archive is corrupt, holds an absolute path, a `..` component, a
/// link leaving the directory or an entry that is not a file, directory or
/// link, has more than 10 000 entries, or unpacks to more than `limit` bytes.
pub fn extract(bytes: &[u8], format: Format, destination: &Path, limit: u64) -> Result<()> {
    let mut budget = Budget { left: limit, limit };
    match format {
        Format::TarGz => extract_tar(bytes, destination, &mut budget),
        Format::Zip => extract_zip(bytes, destination, &mut budget),
    }
}

struct Budget {
    left: u64,
    limit: u64,
}

impl Budget {
    /// Copy `reader` into a new file at `path`, charging its size.
    fn write(&mut self, reader: &mut impl Read, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(path)
            .with_context(|| format!("cannot create {}", path.display()))?;
        let written = std::io::copy(&mut reader.take(self.left + 1), &mut file)
            .with_context(|| format!("cannot unpack {}", path.display()))?;
        if written > self.left {
            bail!("the archive unpacks to more than {} bytes", self.limit);
        }
        self.left -= written;
        Ok(())
    }
}

/// `name` as a path under the archive root, refusing anything that is not a
/// plain relative path.
fn entry_path(name: &Path) -> Result<PathBuf> {
    let mut clean = PathBuf::new();
    for component in name.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("the archive holds an unsafe path: {}", name.display())
            }
        }
    }
    Ok(clean)
}

/// Refuse a link at `entry` whose `target` resolves outside the archive root.
fn check_link(entry: &Path, target: &Path) -> Result<()> {
    let mut depth = entry.components().count().saturating_sub(1);
    let escapes = target.components().any(|component| match component {
        Component::Normal(_) => {
            depth += 1;
            false
        }
        Component::CurDir => false,
        Component::ParentDir => {
            if depth == 0 {
                return true;
            }
            depth -= 1;
            false
        }
        Component::RootDir | Component::Prefix(_) => true,
    });
    if escapes {
        bail!(
            "the archive links {} to {}, outside the skill directory",
            entry.display(),
            target.display()
        );
    }
    tracing::warn!(
        entry = %entry.display(),
        "the archive holds a link; pollen deploys regular files only, so it is left out"
    );
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))
        .with_context(|| format!("cannot set the mode of {}", path.display()))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

fn extract_tar(bytes: &[u8], destination: &Path, budget: &mut Budget) -> Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .context("the archive is not a valid .tar.gz")?;
    for (count, entry) in entries.enumerate() {
        if count >= MAX_ENTRIES {
            bail!("the archive holds more than {MAX_ENTRIES} entries");
        }
        let mut entry = entry.context("the archive is not a valid .tar.gz")?;
        let relative = entry_path(&entry.path().context("the archive holds an invalid path")?)?;
        let path = destination.join(&relative);
        let kind = entry.header().entry_type();
        match kind {
            tar::EntryType::Directory => std::fs::create_dir_all(&path)?,
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                let mode = entry.header().mode().ok();
                budget.write(&mut entry, &path)?;
                if let Some(mode) = mode {
                    set_mode(&path, mode)?;
                }
            }
            tar::EntryType::Symlink | tar::EntryType::Link => {
                let target = entry
                    .link_name()
                    .context("the archive holds an invalid link")?
                    .context("the archive holds a link without a target")?;
                if kind == tar::EntryType::Link {
                    // A hard link names another entry by its path from the root.
                    entry_path(&target)?;
                    tracing::warn!(entry = %relative.display(), "the archive holds a hard link; it is left out");
                } else {
                    check_link(&relative, &target)?;
                }
            }
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => {}
            other => bail!(
                "the archive holds {}, which is neither a file, a directory nor a link ({other:?})",
                relative.display()
            ),
        }
    }
    Ok(())
}

fn extract_zip(bytes: &[u8], destination: &Path, budget: &mut Budget) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("the archive is not a valid .zip")?;
    if archive.len() > MAX_ENTRIES {
        bail!("the archive holds more than {MAX_ENTRIES} entries");
    }
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .context("the archive is not a valid .zip")?;
        let name = file.name().context("the archive holds an invalid path")?;
        let relative = entry_path(Path::new(&name.replace('\\', "/")))?;
        let path = destination.join(&relative);
        if file.is_dir() {
            std::fs::create_dir_all(&path)?;
        } else if file.is_symlink() {
            let mut target = String::new();
            file.by_ref()
                .take(4096)
                .read_to_string(&mut target)
                .context("the archive holds an invalid link")?;
            check_link(&relative, Path::new(&target))?;
        } else {
            let mode = file.unix_mode();
            budget.write(&mut file, &path)?;
            if let Some(mode) = mode {
                set_mode(&path, mode)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tar_gz(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        build(&mut builder);
        let tar = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap()
    }

    fn file(builder: &mut tar::Builder<Vec<u8>>, path: &str, mode: u32, body: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(mode);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append_data(&mut header, path, body).unwrap();
    }

    /// A tar header whose name is written raw, as a hostile archive would:
    /// the builder itself refuses `..` and absolute paths.
    fn raw(builder: &mut tar::Builder<Vec<u8>>, name: &str, kind: tar::EntryType, link: &str) {
        let mut header = tar::Header::new_old();
        header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        header.as_old_mut().linkname[..link.len()].copy_from_slice(link.as_bytes());
        header.set_entry_type(kind);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
    }

    fn zip(entries: &[(&str, Option<u32>, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, mode, body) in entries {
            let mut options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            if let Some(mode) = mode {
                options = options.unix_permissions(*mode);
            }
            writer.start_file(*name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn unpack(bytes: &[u8], format: Format) -> Result<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        extract(bytes, format, dir.path(), 1024).map(|()| dir)
    }

    #[test]
    fn unpacks_a_tar_gz_and_keeps_the_executable_bit() {
        let bytes = tar_gz(|builder| {
            file(builder, "SKILL.md", 0o644, b"---\n");
            file(builder, "scripts/run.sh", 0o755, b"#!/bin/sh\n");
        });

        let dir = unpack(&bytes, Format::TarGz).unwrap();

        assert_eq!(
            std::fs::read(dir.path().join("SKILL.md")).unwrap(),
            b"---\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("scripts/run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn unpacks_a_zip() {
        let bytes = zip(&[
            ("SKILL.md", None, b"---\n"),
            ("references/a.md", Some(0o644), b"a"),
        ]);

        let dir = unpack(&bytes, Format::Zip).unwrap();

        assert_eq!(
            std::fs::read(dir.path().join("references/a.md")).unwrap(),
            b"a"
        );
    }

    #[test]
    fn refuses_path_traversal_and_absolute_paths() {
        for name in ["../escape", "a/../../escape", "/etc/escape"] {
            let bytes = tar_gz(|builder| raw(builder, name, tar::EntryType::Regular, ""));
            assert!(unpack(&bytes, Format::TarGz).is_err(), "tar {name}");

            let bytes = zip(&[(name, None, b"x")]);
            assert!(unpack(&bytes, Format::Zip).is_err(), "zip {name}");
        }
    }

    #[test]
    fn refuses_a_link_leaving_the_directory_and_leaves_out_one_inside() {
        for target in ["../../outside", "/etc/passwd"] {
            let bytes = tar_gz(|builder| raw(builder, "sub/link", tar::EntryType::Symlink, target));
            assert!(unpack(&bytes, Format::TarGz).is_err(), "{target}");
        }

        let bytes = tar_gz(|builder| {
            file(builder, "SKILL.md", 0o644, b"x");
            raw(builder, "sub/link", tar::EntryType::Symlink, "../SKILL.md");
        });
        let dir = unpack(&bytes, Format::TarGz).unwrap();
        assert!(!dir.path().join("sub/link").exists());
    }

    #[test]
    fn refuses_an_archive_unpacking_past_the_limit() {
        let big = vec![0; 2048];
        let bytes = tar_gz(|builder| file(builder, "big", 0o644, &big));
        assert!(unpack(&bytes, Format::TarGz).is_err());

        let bytes = zip(&[("big", None, &big)]);
        assert!(unpack(&bytes, Format::Zip).is_err());
    }

    #[test]
    fn refuses_a_device_entry() {
        let bytes = tar_gz(|builder| raw(builder, "device", tar::EntryType::Char, ""));

        assert!(unpack(&bytes, Format::TarGz).is_err());
    }

    #[test]
    fn detects_the_format_from_the_header_then_the_extension() {
        let url = |path: &str| Url::parse(&format!("https://x.example{path}")).unwrap();

        assert_eq!(
            Format::detect(Some("application/zip"), &url("/a.tar.gz")),
            Some(Format::Zip)
        );
        assert_eq!(
            Format::detect(Some("application/octet-stream"), &url("/a.tar.gz")),
            Some(Format::TarGz)
        );
        assert_eq!(Format::detect(None, &url("/a.TGZ")), Some(Format::TarGz));
        assert_eq!(Format::detect(None, &url("/a.zip?v=2")), Some(Format::Zip));
        assert_eq!(Format::detect(None, &url("/a")), None);
    }
}
