//! The JSON Schema for `pollen.yaml`, published so editors and other tools
//! can validate a config without running pollen.

use anyhow::Result;
use schemars::generate::SchemaSettings;
use serde_json::{Value, json};

use crate::config::raw;

/// Where the published schema is served from.
pub const SCHEMA_ID: &str =
    "https://raw.githubusercontent.com/groupbees/pollen/main/schema/pollen.schema.json";

/// Path of the copy committed to this repository.
pub const SCHEMA_PATH: &str = "schema/pollen.schema.json";

/// Build the JSON Schema document describing `pollen.yaml`.
///
/// One document covers every format version, so the published URL keeps
/// validating files written in an older one: a file declaring `version: 2`
/// is checked against version 2, any other against version 1, the version
/// of a file that sets none.
#[must_use]
pub fn document() -> Value {
    let mut generator = SchemaSettings::default().into_generator();
    let v1 = generator.subschema_for::<raw::V1>();
    let v2 = generator.subschema_for::<raw::V2>();
    let definitions = generator.take_definitions(true);
    let mut document = json!({
        "$schema": generator.settings().meta_schema,
        "$id": SCHEMA_ID,
        "title": "pollen configuration",
        "description": "Declares where Agent Skills come from and which directories they are deployed into.",
        "type": "object",
        "if": {"required": ["version"], "properties": {"version": {"const": 2}}},
        "then": v2,
        "else": v1,
    });
    document["$defs"] = Value::Object(definitions);
    document
}

/// Render the document exactly as the committed copy is stored.
///
/// # Errors
///
/// When the document cannot be serialised, which would mean a schemars bug.
pub fn rendered() -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(&document())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validator() -> jsonschema::Validator {
        jsonschema::options()
            .should_validate_formats(true)
            .build(&document())
            .expect("the generated document is a valid JSON Schema")
    }

    fn yaml(text: &str) -> Value {
        serde_norway::from_str(text).expect("the fixture is valid YAML")
    }

    #[test]
    fn the_committed_copy_is_up_to_date() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(SCHEMA_PATH);
        let committed = std::fs::read_to_string(&path).expect("the schema is committed");

        assert_eq!(
            committed,
            rendered().unwrap(),
            "{SCHEMA_PATH} is stale; regenerate it with `scripts/generate-schema.sh`"
        );
    }

    #[test]
    fn accepts_the_documented_example() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/pollen.yaml");
        let example = yaml(&std::fs::read_to_string(path).unwrap());

        let errors: Vec<String> = validator()
            .iter_errors(&example)
            .map(|error| error.to_string())
            .collect();

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn accepts_a_minimal_config() {
        let config = yaml("repos:\n  - repo: local\n    paths:\n      - path: skills\n");

        assert!(validator().is_valid(&config));
    }

    #[test]
    fn rejects_a_git_repo_without_a_revision() {
        let config =
            yaml("repos:\n  - repo: https://example.com/x.git\n    paths:\n      - path: .\n");

        assert!(!validator().is_valid(&config));
    }

    #[test]
    fn rejects_a_local_repo_carrying_a_revision() {
        let config =
            yaml("repos:\n  - repo: local\n    revision: 1.0.0\n    paths:\n      - path: .\n");

        assert!(!validator().is_valid(&config));
    }

    #[test]
    fn accepts_a_well_known_source() {
        for config in [
            "repos:\n  - well-known: https://x.example\n",
            "repos:\n  - well-known: https://x.example\n    exclude: ^a\n",
            "repos:\n  - well-known: https://x.example\n    skills:\n      - name: a\n        digest: sha256:0000000000000000000000000000000000000000000000000000000000000000\n",
        ] {
            assert!(validator().is_valid(&yaml(config)), "{config}");
        }
    }

    #[test]
    fn rejects_keys_that_do_not_belong_to_the_kind_of_source() {
        for config in [
            "repos:\n  - repo: local\n    well-known: https://x.example\n    paths:\n      - path: .\n",
            "repos:\n  - well-known: https://x.example\n    revision: v1\n",
            "repos:\n  - well-known: https://x.example\n    paths:\n      - path: .\n",
            "repos:\n  - well-known: https://x.example\n    exclude: ^a\n    skills:\n      - name: a\n",
            "repos:\n  - repo: local\n    exclude: ^a\n    paths:\n      - path: .\n",
            "repos:\n  - well-known: https://x.example\n    skills:\n      - name: a\n        digest: sha256:00\n",
        ] {
            assert!(!validator().is_valid(&yaml(config)), "{config}");
        }
    }

    #[test]
    fn accepts_both_versions_of_the_format() {
        for config in [
            "version: 1\nrepos:\n  - repo: local\n    paths:\n      - path: skills\n",
            "version: 2\nrepos:\n  - repo: local\n    skills:\n      - path: skills\n        recurse: true\n",
            "version: 2\nrepos:\n  - repo: https://x.example/a.git\n    revision: v1\n    skills:\n      - path: .\n",
            "version: 2\ndiscovery:\n  - url: https://x.example\n    skills:\n      - name: a\n",
            "version: 2\ndiscovery:\n  - url: https://x.example\n    exclude: ^a\n",
            "version: 2\nrepos:\n  - repo: local\n    skills:\n      - path: .\ndiscovery:\n  - url: https://x.example\n",
        ] {
            assert!(validator().is_valid(&yaml(config)), "{config}");
        }
    }

    #[test]
    fn checks_each_version_against_its_own_keys() {
        for config in [
            "version: 2\nrepos:\n  - repo: local\n    paths:\n      - path: .\n",
            "repos:\n  - repo: local\n    skills:\n      - path: .\n",
            "version: 2\nrepos:\n  - repo: local\n    skills:\n      - name: a\n",
            "version: 2\nrepos:\n  - well-known: https://x.example\n",
            "version: 2\ndiscovery:\n  - url: https://x.example\n    skills:\n      - path: .\n",
            "version: 2\ndiscovery:\n  - url: https://x.example\n    revision: v1\n",
            "version: 2\ndiscovery:\n  - url: https://x.example\n    exclude: ^a\n    skills:\n      - name: a\n",
            "version: 2\ndiscovery: []\n",
            "version: 2\ntargets: [out]\n",
            "discovery:\n  - url: https://x.example\n",
            "version: 2\nrepos:\n  - repo: local\n",
            "version: 2\nrepos:\n  - repo: local\n    revision: v1\n    skills:\n      - path: .\n",
            "version: 2\nrepos:\n  - repo: https://x.example/a.git\n    skills:\n      - path: .\n",
            "version: 3\nrepos:\n  - repo: local\n    skills:\n      - path: .\n",
            "version: \"2\"\nrepos:\n  - repo: local\n    skills:\n      - path: .\n",
        ] {
            assert!(!validator().is_valid(&yaml(config)), "{config}");
        }
    }

    #[test]
    fn rejects_the_singular_target_key() {
        let config =
            yaml("target: .claude/skills\nrepos:\n  - repo: local\n    paths:\n      - path: .\n");

        assert!(!validator().is_valid(&config));
    }

    #[test]
    fn accepts_clients() {
        for clients in ["all", "claude", "[claude]", "[claude, agent-skills]"] {
            let config = yaml(&format!(
                "version: 2\nclients: {clients}\nrepos:\n  - repo: local\n    skills:\n      - path: .\n"
            ));

            assert!(validator().is_valid(&config), "{clients}");
        }
    }

    #[test]
    fn rejects_unknown_clients_clients_with_targets_and_clients_in_version_one() {
        for prefix in [
            "clients: []\n",
            "clients: cursor\n",
            "clients: [cursor]\n",
            "clients: all\ntargets: [skills]\n",
        ] {
            let config = yaml(&format!(
                "version: 2\n{prefix}repos:\n  - repo: local\n    skills:\n      - path: .\n"
            ));

            assert!(!validator().is_valid(&config), "{prefix}");
        }
        let v1 = yaml("clients: claude\nrepos:\n  - repo: local\n    paths:\n      - path: .\n");
        assert!(!validator().is_valid(&v1));
    }

    #[test]
    fn rejects_empty_lists() {
        assert!(!validator().is_valid(&yaml("repos: []\n")));
        assert!(!validator().is_valid(&yaml(
            "targets: []\nrepos:\n  - repo: local\n    paths:\n      - path: .\n"
        )));
        assert!(!validator().is_valid(&yaml("repos:\n  - repo: local\n    paths: []\n")));
    }

    #[test]
    fn rejects_a_misspelled_path_key() {
        let config = yaml("repos:\n  - repo: local\n    paths:\n      - pathz: skills\n");

        assert!(!validator().is_valid(&config));
    }

    #[test]
    fn does_not_leak_the_internal_base_directory() {
        let text = rendered().unwrap();

        assert!(!text.contains("base_dir"), "{text}");
    }
}
