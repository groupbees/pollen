//! The unit pollen deploys: a directory holding a `SKILL.md` whose YAML
//! frontmatter follows the Agent Skills specification.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;
use validator::{Validate, ValidationError};
use walkdir::WalkDir;

use crate::hidden::{self, Hidden};
use crate::source::relative_slug;

/// The file that makes a directory a skill.
pub const SKILL_FILE: &str = "SKILL.md";

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("{path}: no {SKILL_FILE}")]
    NotASkill { path: PathBuf },
    #[error("{path}: {SKILL_FILE} has no YAML frontmatter (it must open with a `---` line)")]
    NoFrontmatter { path: PathBuf },
    #[error("{path}: {SKILL_FILE} frontmatter is not valid YAML: {source}")]
    Yaml {
        path: PathBuf,
        source: serde_norway::Error,
    },
    #[error("{path}: {SKILL_FILE} frontmatter is invalid: {source}")]
    Invalid {
        path: PathBuf,
        source: validator::ValidationErrors,
    },
    #[error("{path}: frontmatter name `{name}` does not match the directory name")]
    NameMismatch {
        path: PathBuf,
        /// The name the frontmatter claims.
        name: String,
    },
    #[error("{path}: {file} holds {count} invisible character(s), the first at {first}")]
    HiddenCharacters {
        path: PathBuf,
        /// The file, relative to the skill directory.
        file: String,
        /// How many the file holds.
        count: usize,
        /// The first of them.
        first: Hidden,
    },
    #[error("{path}: cannot read {file}: {source}")]
    Io {
        path: PathBuf,
        /// The file, relative to the skill directory.
        file: String,
        source: std::io::Error,
    },
}

/// The frontmatter fields defined by the Agent Skills specification.
///
/// Unknown keys are kept out of this struct on purpose: agents extend the
/// frontmatter with their own fields (`model`, `argument-hint`, `paths`, …)
/// and rejecting those would make pollen refuse perfectly good skills.
#[derive(Debug, Clone, Deserialize, Validate)]
pub struct Frontmatter {
    /// Skill identifier, and the directory name it must be stored under.
    #[validate(length(min = 1, max = 64), custom(function = validate_name))]
    pub name: String,
    /// What the skill does and when an agent should reach for it.
    #[validate(length(min = 1, max = 1024))]
    pub description: String,
    /// Environment requirements, when the skill has any.
    #[validate(length(min = 1, max = 500))]
    pub compatibility: Option<String>,
    /// Client-defined extra properties.
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_norway::Value>,
}

/// Read and validate the skill stored in `dir`.
///
/// With `allow_hidden_characters`, a file holding an invisible character is
/// logged as a warning instead of refusing the skill.
///
/// # Errors
///
/// When the directory holds no `SKILL.md`, its frontmatter is missing,
/// unparseable, or outside the Agent Skills specification, or one of its
/// text files holds an invisible character and `allow_hidden_characters` is
/// not set.
pub fn load(dir: &Path, allow_hidden_characters: bool) -> Result<Frontmatter, SkillError> {
    let manifest = dir.join(SKILL_FILE);
    if !manifest.is_file() {
        return Err(SkillError::NotASkill {
            path: dir.to_path_buf(),
        });
    }

    let text = std::fs::read_to_string(&manifest).map_err(|source| SkillError::Io {
        path: dir.to_path_buf(),
        file: SKILL_FILE.to_owned(),
        source,
    })?;
    let block = split_frontmatter(&text).ok_or_else(|| SkillError::NoFrontmatter {
        path: dir.to_path_buf(),
    })?;
    let frontmatter: Frontmatter =
        serde_norway::from_str(block).map_err(|source| SkillError::Yaml {
            path: dir.to_path_buf(),
            source,
        })?;
    frontmatter
        .validate()
        .map_err(|source| SkillError::Invalid {
            path: dir.to_path_buf(),
            source,
        })?;

    let dir_name = dir.file_name().and_then(|name| name.to_str());
    if dir_name != Some(frontmatter.name.as_str()) {
        return Err(SkillError::NameMismatch {
            path: dir.to_path_buf(),
            name: frontmatter.name,
        });
    }

    check_hidden_characters(dir, allow_hidden_characters)?;

    for (key, value) in &frontmatter.metadata {
        if !value.is_string() {
            tracing::warn!(
                skill = %frontmatter.name,
                %key,
                "metadata value is not a string; the specification maps string keys to string values"
            );
        }
    }

    Ok(frontmatter)
}

/// Refuse the first file whose text holds an invisible character, or warn
/// about each one when they are allowed. A file that is not UTF-8 is binary
/// to an agent too, and is left alone.
fn check_hidden_characters(dir: &Path, allow: bool) -> Result<(), SkillError> {
    let walk = WalkDir::new(dir)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".git");
    for entry in walk {
        let entry = entry.map_err(|error| SkillError::Io {
            path: dir.to_path_buf(),
            file: error
                .path()
                .map_or_else(|| ".".to_owned(), |path| relative_slug(dir, path)),
            source: error.into(),
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let file = relative_slug(dir, entry.path());
        let bytes = std::fs::read(entry.path()).map_err(|source| SkillError::Io {
            path: dir.to_path_buf(),
            file: file.clone(),
            source,
        })?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            tracing::debug!(%file, "not UTF-8, not checked for invisible characters");
            continue;
        };
        let found = hidden::find(text);
        let Some(&first) = found.first() else {
            continue;
        };
        let error = SkillError::HiddenCharacters {
            path: dir.to_path_buf(),
            file,
            count: found.len(),
            first,
        };
        if !allow {
            return Err(error);
        }
        tracing::warn!("{error}; kept, since invisible characters are allowed");
    }
    Ok(())
}

/// Return the YAML between the opening `---` line and the closing one.
///
/// A leading byte order mark is skipped: Windows editors and PowerShell's
/// `Out-File -Encoding utf8` write one, and it would hide the opening `---`.
fn split_frontmatter(text: &str) -> Option<&str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let body = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some(&body[..offset]);
        }
        offset += line.len();
    }
    None
}

/// Validate an authored or persisted skill identifier before using it as a path.
pub(crate) fn validate_name(name: &str) -> Result<(), ValidationError> {
    let error = |message: &'static str| {
        let mut failure = ValidationError::new("skill_name");
        failure.message = Some(message.into());
        failure
    };

    if name.is_empty() || name.len() > 64 {
        return Err(error("must contain between 1 and 64 characters"));
    }
    if !name.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err(error(
            "must contain only lowercase letters, digits and hyphens",
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(error("must not start or end with a hyphen"));
    }
    if name.contains("--") {
        return Err(error("must not contain consecutive hyphens"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &Path, name: &str, body: &str) -> PathBuf {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join(SKILL_FILE), body).unwrap();
        skill_dir
    }

    #[test]
    fn the_skill_pollen_ships_satisfies_the_specification() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills/pollen");

        assert_eq!(load(&dir, false).unwrap().name, "pollen");
    }

    #[test]
    fn loads_a_minimal_skill() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "pdf-processing",
            "---\nname: pdf-processing\ndescription: Extract text from PDFs.\n---\n\nBody.\n",
        );

        let frontmatter = load(&dir, false).unwrap();

        assert_eq!(frontmatter.name, "pdf-processing");
        assert_eq!(frontmatter.description, "Extract text from PDFs.");
    }

    #[test]
    fn keeps_agent_specific_frontmatter_extensions() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "deploy",
            "---\nname: deploy\ndescription: Ship it.\nmodel: opus\ndisable-model-invocation: true\n---\n",
        );

        assert!(load(&dir, false).is_ok());
    }

    #[test]
    fn rejects_a_name_that_disagrees_with_the_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "deploy",
            "---\nname: ship\ndescription: Ship it.\n---\n",
        );

        assert!(matches!(
            load(&dir, false),
            Err(SkillError::NameMismatch { .. })
        ));
    }

    #[test]
    fn rejects_a_name_outside_the_specification() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "PDF--Processing",
            "---\nname: PDF--Processing\ndescription: Nope.\n---\n",
        );

        assert!(matches!(load(&dir, false), Err(SkillError::Invalid { .. })));
    }

    #[test]
    fn rejects_an_empty_description() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "deploy",
            "---\nname: deploy\ndescription: \"\"\n---\n",
        );

        assert!(matches!(load(&dir, false), Err(SkillError::Invalid { .. })));
    }

    #[test]
    fn rejects_a_directory_without_frontmatter() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(root.path(), "deploy", "# Just markdown\n");

        assert!(matches!(
            load(&dir, false),
            Err(SkillError::NoFrontmatter { .. })
        ));
    }

    #[test]
    fn rejects_a_directory_without_a_skill_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("empty")).unwrap();

        assert!(matches!(
            load(&root.path().join("empty"), false),
            Err(SkillError::NotASkill { .. })
        ));
    }

    #[test]
    fn rejects_a_skill_file_hiding_an_instruction() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "deploy",
            "---\nname: deploy\ndescription: Ship it.\n---\n\nShip\u{200b}\u{200b} it.\n",
        );

        let error = load(&dir, false).unwrap_err();

        assert!(
            matches!(
                &error,
                SkillError::HiddenCharacters { file, count: 2, first, .. }
                    if file == SKILL_FILE && first.line == 6 && first.column == 5
            ),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_bundled_file_hiding_an_instruction() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "deploy",
            "---\nname: deploy\ndescription: Ship it.\n---\n",
        );
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(dir.join("references/guide.md"), "Read \u{202e}me\n").unwrap();

        let error = load(&dir, false).unwrap_err().to_string();

        assert!(
            error.ends_with(
                "references/guide.md holds 1 invisible character(s), the first at line 1, column 6 (U+202E)"
            ),
            "{error}"
        );
    }

    #[test]
    fn keeps_a_skill_hiding_characters_when_they_are_allowed() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "deploy",
            "---\nname: deploy\ndescription: Ship it.\n---\n\nShip\u{200b} it.\n",
        );

        assert_eq!(load(&dir, true).unwrap().name, "deploy");
    }

    #[test]
    fn leaves_binary_files_alone() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "deploy",
            "---\nname: deploy\ndescription: Ship it.\n---\n",
        );
        std::fs::write(
            dir.join("logo.png"),
            [0x89, b'P', b'N', b'G', 0xff, 0x1b, 0x00],
        )
        .unwrap();

        assert!(load(&dir, false).is_ok());
    }

    #[test]
    fn splits_frontmatter_at_the_closing_marker() {
        assert_eq!(split_frontmatter("---\na: 1\n---\nbody\n"), Some("a: 1\n"));
        assert_eq!(split_frontmatter("no marker\n"), None);
        assert_eq!(split_frontmatter("---\nunterminated\n"), None);
    }

    #[test]
    fn loads_a_skill_saved_with_windows_line_endings_and_a_byte_order_mark() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(
            root.path(),
            "deploy",
            "\u{feff}---\r\nname: deploy\r\ndescription: Ship it.\r\n---\r\n\r\nBody.\r\n",
        );

        let frontmatter = load(&dir, false).unwrap();

        assert_eq!(frontmatter.name, "deploy");
        assert_eq!(frontmatter.description, "Ship it.");
    }
}
