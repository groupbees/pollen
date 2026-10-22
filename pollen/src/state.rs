//! What pollen installed, so it knows what it may replace and remove.
//!
//! The file lives in the target directory rather than next to the config: the
//! config is shared and version-controlled, whereas what is on disk is a
//! property of this machine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Name of the state file inside the target directory.
pub const STATE_FILE: &str = ".pollen.json";

const CURRENT_VERSION: u32 = 1;

/// The record of every skill pollen owns in one target directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Schema version of this file.
    pub version: u32,
    /// The config file that deploys into this directory. One target has one
    /// owner: an update prunes whatever its config does not declare, so a
    /// second config would remove the first one's skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<PathBuf>,
    /// Installed skills, keyed by the name they are deployed under.
    pub skills: BTreeMap<String, InstalledSkill>,
}

/// One deployed skill, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    /// The `repo:` value it was declared under.
    pub repo: String,
    /// The `revision:` it was pinned to.
    pub revision: Option<String>,
    /// The commit that revision resolved to.
    pub commit: Option<String>,
    /// Where the skill sits inside its source.
    pub source_path: String,
    /// Fingerprint of the installed tree.
    pub checksum: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            config: None,
            skills: BTreeMap::new(),
        }
    }
}

impl State {
    /// Read the state of `target`, defaulting to empty when there is none.
    ///
    /// # Errors
    ///
    /// When the file cannot be read, is corrupt, was written by a newer
    /// pollen, or contains an invalid managed skill name.
    pub fn load(target: &Path) -> Result<Self> {
        let path = Self::path(target);
        if !path.is_file() {
            return Ok(Self::default());
        }

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let state: Self = serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is corrupt; remove it to start from scratch",
                path.display()
            )
        })?;
        anyhow::ensure!(
            state.version == CURRENT_VERSION,
            "{} was written by a newer pollen (state version {}); upgrade pollen",
            path.display(),
            state.version
        );
        state
            .validate_skill_names()
            .with_context(|| format!("{} contains unsafe managed skill names", path.display()))?;
        Ok(state)
    }

    /// Write the state of `target`, atomically.
    ///
    /// # Errors
    ///
    /// When a managed skill name is invalid or the target directory cannot
    /// be created or written to.
    pub fn save(&self, target: &Path) -> Result<()> {
        let path = Self::path(target);
        self.validate_skill_names()
            .with_context(|| format!("{} contains unsafe managed skill names", path.display()))?;
        std::fs::create_dir_all(target)
            .with_context(|| format!("cannot create {}", target.display()))?;
        let text = serde_json::to_string_pretty(self)?;

        let mut staging = tempfile::Builder::new()
            .prefix(".pollen-state-")
            .tempfile_in(target)
            .with_context(|| format!("cannot write into {}", target.display()))?;
        std::io::Write::write_all(&mut staging, text.as_bytes())?;
        std::io::Write::write_all(&mut staging, b"\n")?;
        staging
            .persist(&path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }

    /// Path of the state file inside `target`.
    #[must_use]
    pub fn path(target: &Path) -> PathBuf {
        target.join(STATE_FILE)
    }

    /// Refuse unsafe identities before they reach target writes or pruning.
    fn validate_skill_names(&self) -> Result<()> {
        for name in self.skills.keys() {
            crate::skill::validate_name(name)
                .map_err(|error| anyhow::anyhow!("unsafe managed skill name {name:?}: {error}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> InstalledSkill {
        InstalledSkill {
            repo: "https://github.com/toto/tata".to_owned(),
            revision: Some("1.2.3".to_owned()),
            commit: Some("deadbeef".to_owned()),
            source_path: "mydir/demo".to_owned(),
            checksum: "sha256:00".to_owned(),
        }
    }

    #[test]
    fn an_absent_state_file_reads_as_empty() {
        let target = tempfile::tempdir().unwrap();

        assert!(State::load(target.path()).unwrap().skills.is_empty());
    }

    #[test]
    fn round_trips_through_the_target_directory() {
        let target = tempfile::tempdir().unwrap();
        let mut state = State::default();
        state.skills.insert("demo".to_owned(), entry());

        state.save(target.path()).unwrap();
        let reloaded = State::load(target.path()).unwrap();

        assert_eq!(reloaded.skills["demo"], entry());
    }

    #[test]
    fn reads_a_state_file_without_an_owner() {
        let target = tempfile::tempdir().unwrap();
        std::fs::write(State::path(target.path()), r#"{"version":1,"skills":{}}"#).unwrap();

        assert_eq!(State::load(target.path()).unwrap().config, None);
    }

    #[test]
    fn refuses_a_state_file_from_a_newer_version() {
        let target = tempfile::tempdir().unwrap();
        std::fs::write(State::path(target.path()), r#"{"version":99,"skills":{}}"#).unwrap();

        let error = State::load(target.path()).unwrap_err().to_string();

        assert!(error.contains("newer pollen"), "{error}");
    }

    #[test]
    fn refuses_a_corrupt_state_file() {
        let target = tempfile::tempdir().unwrap();
        std::fs::write(State::path(target.path()), "not json").unwrap();

        assert!(State::load(target.path()).is_err());
    }

    #[test]
    fn refuses_unsafe_managed_skill_names_when_loading() {
        let target = tempfile::tempdir().unwrap();
        let unsafe_names = [
            String::new(),
            "../outside".to_owned(),
            "..\\outside".to_owned(),
            "/outside".to_owned(),
            "C:\\outside".to_owned(),
            "Uppercase".to_owned(),
            "double--hyphen".to_owned(),
            "x".repeat(65),
        ];
        for name in unsafe_names {
            let mut skills = serde_json::Map::new();
            skills.insert(name.clone(), serde_json::to_value(entry()).unwrap());
            let original = serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "skills": skills,
            }))
            .unwrap();
            std::fs::write(State::path(target.path()), &original).unwrap();

            let result = State::load(target.path());

            assert!(
                result.is_err(),
                "unsafe managed identity accepted: {name:?}"
            );
            assert_eq!(std::fs::read(State::path(target.path())).unwrap(), original);
        }
    }

    #[test]
    fn refuses_to_write_unsafe_managed_skill_names() {
        let target = tempfile::tempdir().unwrap();
        let mut state = State::default();
        state.skills.insert("../outside".to_owned(), entry());

        let result = state.save(target.path());

        assert!(result.is_err());
        assert!(!State::path(target.path()).exists());
    }
}
