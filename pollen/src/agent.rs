//! An agent definition: one Markdown file whose YAML frontmatter configures
//! a subagent and whose body is its prompt, deployed byte for byte.
//!
//! Every agent passes the checks every client shares; one deployed for a
//! client must also follow that client's rules, read from its row of the
//! client table.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_norway::{Mapping, Value};

use crate::config::{Client, Identity};
use crate::hidden;
use crate::skill::{split_frontmatter, validate_name};

/// What an agent file's name ends with, for every client pollen knows.
pub const SUFFIX: &str = ".md";

/// Why an agent file cannot be deployed.
#[derive(Debug, thiserror::Error)]
#[error("{path}: {problem}")]
pub struct AgentError {
    pub path: PathBuf,
    pub problem: String,
}

/// An agent file that passed the checks every client shares.
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    /// Where it was read.
    pub path: PathBuf,
    /// The name it deploys under: its file name without `.md`.
    pub name: String,
    /// Its `description`, when it sets one.
    pub description: Option<String>,
    /// What its frontmatter grants.
    pub details: Details,
    frontmatter: Mapping,
}

/// What an agent's frontmatter grants, as a reviewer reads it: each value is
/// the frontmatter's own, on one line, and `None` when the key is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Details {
    /// The tools it may use: Claude Code's allowlist, OpenCode's switches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Claude Code's `permissionMode`, else OpenCode's `permission` rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// Whether it declares `hooks`, which run shell commands.
    pub hooks: bool,
    /// Whether it declares `mcpServers`.
    pub mcp_servers: bool,
}

impl Details {
    fn read(frontmatter: &Mapping) -> Self {
        let text = |key: &str| frontmatter.get(key).map(one_line);
        Self {
            tools: text("tools"),
            model: text("model"),
            permission: text("permissionMode").or_else(|| text("permission")),
            hooks: frontmatter.contains_key("hooks"),
            mcp_servers: frontmatter.contains_key("mcpServers"),
        }
    }

    /// Whether the frontmatter sets none of what this records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl fmt::Display for Details {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        for (label, value) in [
            ("tools", &self.tools),
            ("model", &self.model),
            ("permission", &self.permission),
        ] {
            if let Some(value) = value {
                parts.push(format!("{label}: {value}"));
            }
        }
        if self.hooks {
            parts.push("hooks".to_owned());
        }
        if self.mcp_servers {
            parts.push("MCP servers".to_owned());
        }
        formatter.write_str(&parts.join("; "))
    }
}

/// A YAML value on one line: a list joined with commas, a mapping as
/// `key: value` pairs, nested mappings in braces.
fn one_line(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
        Value::Sequence(items) => items.iter().map(one_line).collect::<Vec<_>>().join(", "),
        Value::Mapping(mapping) => mapping
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    Value::Mapping(_) => format!("{{{}}}", one_line(value)),
                    _ => one_line(value),
                };
                format!("{}: {value}", one_line(key))
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Tagged(tagged) => one_line(&tagged.value),
    }
}

/// The agent name `file` deploys under, when its name ends with `.md`.
#[must_use]
pub fn name_of(file: &Path) -> Option<&str> {
    file.file_name()?
        .to_str()?
        .strip_suffix(SUFFIX)
        .filter(|name| !name.is_empty())
}

/// Read the agent `file` and run the checks every client shares.
///
/// With `allow_hidden_characters`, an invisible character is logged as a
/// warning instead of refusing the agent.
///
/// # Errors
///
/// When the file name is not a valid name, the file cannot be read or holds
/// an invisible character, or its frontmatter is missing or not a mapping.
pub fn load(file: &Path, allow_hidden_characters: bool) -> Result<Agent, AgentError> {
    let fail = |problem: String| AgentError {
        path: file.to_path_buf(),
        problem,
    };
    let name = name_of(file)
        .ok_or_else(|| {
            fail(format!(
                "not an agent file: its name must end with {SUFFIX}"
            ))
        })?
        .to_owned();
    validate_name(&name)
        .map_err(|error| fail(format!("`{name}` is not a valid agent name: {error}")))?;

    let text =
        std::fs::read_to_string(file).map_err(|error| fail(format!("cannot read: {error}")))?;
    let found = hidden::find(&text);
    if let Some(first) = found.first() {
        let problem = format!(
            "holds {} invisible character(s), the first at {first}",
            found.len()
        );
        if !allow_hidden_characters {
            return Err(fail(problem));
        }
        tracing::warn!(path = %file.display(), "{problem}; kept, since invisible characters are allowed");
    }

    let block = split_frontmatter(&text).ok_or_else(|| {
        fail("has no YAML frontmatter (it must open with a `---` line)".to_owned())
    })?;
    let frontmatter: Mapping = serde_norway::from_str(block)
        .map_err(|error| fail(format!("frontmatter is not a YAML mapping: {error}")))?;

    Ok(Agent {
        path: file.to_path_buf(),
        description: field(&frontmatter, "description").map(str::to_owned),
        details: Details::read(&frontmatter),
        name,
        frontmatter,
    })
}

fn field<'a>(frontmatter: &'a Mapping, key: &str) -> Option<&'a str> {
    frontmatter
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

impl Agent {
    /// Check the agent against `client`'s rules: the fields it requires, and
    /// its identity rule.
    ///
    /// # Errors
    ///
    /// When a field the client requires is missing or empty, or, for a client
    /// naming agents by their `name`, that name is not the file name.
    pub fn check(&self, client: Client) -> Result<(), AgentError> {
        let fail = |problem: String| AgentError {
            path: self.path.clone(),
            problem: format!("for {client}, {problem}"),
        };
        let spec = client.spec();
        for key in spec.required {
            if field(&self.frontmatter, key).is_none() {
                return Err(fail(format!(
                    "the frontmatter needs `{key}`, a non-empty string"
                )));
            }
        }
        if spec.identity == Identity::NameField
            && field(&self.frontmatter, "name") != Some(self.name.as_str())
        {
            return Err(fail(format!(
                "the frontmatter `name` must be `{}`, the file name, since {client} tells agents apart by it",
                self.name
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, file: &str, text: &str) -> PathBuf {
        let path = dir.join(file);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn refusal(file: &Path, client: Client) -> String {
        load(file, false)
            .unwrap()
            .check(client)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn loads_a_claude_code_agent_and_what_it_grants() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "reviewer.md",
            "---\nname: reviewer\ndescription: Reviews code.\ntools: Read, Grep, Glob\nmodel: opus\npermissionMode: acceptEdits\nhooks:\n  PreToolUse: []\nmcpServers:\n  - github\ncolor: red\n---\n\nYou review code.\n",
        );

        let agent = load(&file, false).unwrap();

        assert_eq!(agent.name, "reviewer");
        assert_eq!(agent.description.as_deref(), Some("Reviews code."));
        assert!(agent.check(Client::ClaudeCode).is_ok());
        assert!(agent.check(Client::OpenCode).is_ok());
        assert_eq!(
            agent.details.to_string(),
            "tools: Read, Grep, Glob; model: opus; permission: acceptEdits; hooks; MCP servers"
        );
    }

    #[test]
    fn loads_an_opencode_agent_named_by_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "docs-writer.md",
            "---\ndescription: Writes docs.\nmode: subagent\nmodel: anthropic/claude-sonnet-4-5\ntools:\n  write: true\n  bash: false\npermission:\n  edit: ask\n  bash:\n    \"*\": deny\n    git status: allow\n---\nWrite.\n",
        );

        let agent = load(&file, false).unwrap();

        assert_eq!(agent.name, "docs-writer");
        assert!(agent.check(Client::OpenCode).is_ok());
        assert_eq!(
            agent.details.to_string(),
            "tools: write: true, bash: false; model: anthropic/claude-sonnet-4-5; permission: edit: ask, bash: {*: deny, git status: allow}"
        );
        assert!(!agent.details.hooks);
    }

    #[test]
    fn an_agent_setting_nothing_reviewable_has_empty_details() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "plain.md", "---\ndescription: Plain.\n---\n");

        assert!(load(&file, false).unwrap().details.is_empty());
    }

    #[test]
    fn claude_code_requires_a_name_equal_to_the_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let other = write(
            dir.path(),
            "reviewer.md",
            "---\nname: auditor\ndescription: x\n---\n",
        );
        let missing = write(dir.path(), "auditor.md", "---\ndescription: x\n---\n");

        let renamed = refusal(&other, Client::ClaudeCode);
        assert!(
            renamed.ends_with(
                "for claude-code, the frontmatter `name` must be `reviewer`, the file name, since claude-code tells agents apart by it"
            ),
            "{renamed}"
        );
        assert!(
            refusal(&missing, Client::ClaudeCode)
                .contains("for claude-code, the frontmatter needs `name`")
        );
        assert!(load(&other, false).unwrap().check(Client::OpenCode).is_ok());
    }

    #[test]
    fn every_client_requires_a_description_but_the_shared_checks_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let empty = write(
            dir.path(),
            "a.md",
            "---\nname: a\ndescription: \"  \"\n---\n",
        );
        let listed = write(dir.path(), "b.md", "---\nname: b\ndescription: [x]\n---\n");

        for file in [&empty, &listed] {
            assert!(load(file, false).is_ok());
            for client in Client::ALL {
                let refused = refusal(file, client);
                assert!(
                    refused.contains(&format!(
                        "for {client}, the frontmatter needs `description`"
                    )),
                    "{refused}"
                );
            }
        }
    }

    #[test]
    fn refuses_a_file_without_frontmatter_or_with_a_scalar_one() {
        let dir = tempfile::tempdir().unwrap();
        let readme = write(dir.path(), "readme.md", "# Agents\n");
        let scalar = write(dir.path(), "scalar.md", "---\njust text\n---\n");

        assert!(
            load(&readme, false)
                .unwrap_err()
                .problem
                .contains("no YAML frontmatter")
        );
        assert!(
            load(&scalar, false)
                .unwrap_err()
                .problem
                .contains("not a YAML mapping")
        );
    }

    #[test]
    fn refuses_a_file_name_outside_the_name_rules() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["Reviewer.md", "code_review.md", "a--b.md"] {
            let path = write(dir.path(), file, "---\nname: x\ndescription: x\n---\n");

            assert!(
                load(&path, false)
                    .unwrap_err()
                    .problem
                    .contains("not a valid agent name"),
                "{file}"
            );
        }
    }

    #[test]
    fn refuses_invisible_characters_unless_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "sneaky.md",
            "---\ndescription: x\n---\nRun\u{200b} it.\n",
        );

        assert!(
            load(&file, false).unwrap_err().problem.ends_with(
                "holds 1 invisible character(s), the first at line 4, column 4 (U+200B)"
            )
        );
        assert!(load(&file, true).is_ok());
    }

    #[test]
    fn reads_a_file_saved_with_windows_line_endings_and_a_byte_order_mark() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            "reviewer.md",
            "\u{feff}---\r\nname: reviewer\r\ndescription: Reviews.\r\n---\r\nBody.\r\n",
        );

        let agent = load(&file, false).unwrap();

        assert_eq!(agent.description.as_deref(), Some("Reviews."));
        assert!(agent.check(Client::ClaudeCode).is_ok());
    }

    #[test]
    fn names_only_files_ending_in_md() {
        assert_eq!(name_of(Path::new("a/reviewer.md")), Some("reviewer"));
        assert_eq!(name_of(Path::new("a/reviewer.txt")), None);
        assert_eq!(name_of(Path::new("a/.md")), None);
    }
}
