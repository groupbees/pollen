//! Bearer tokens for `well-known` origins, read from a user-level file that a
//! project's `pollen.yaml` cannot reach.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::config::normalize_origin;

/// The parsed credentials file: how to obtain a token for each origin.
#[derive(Debug, Default)]
pub struct Credentials {
    path: PathBuf,
    origins: BTreeMap<String, Entry>,
}

#[derive(Debug)]
struct Entry {
    method: Method,
    token: OnceCell<Token>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Method {
    Command(Vec<String>),
    Env(String),
}

/// A bearer token. Its `Debug` output is redacted, so a token never lands in
/// a log line or an error by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// The value, for the `Authorization` header and nothing else.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(<redacted>)")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    credentials: Vec<FileEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEntry {
    origin: String,
    command: Option<Vec<String>>,
    env: Option<String>,
}

impl Credentials {
    /// Read the credentials file at `path`.
    ///
    /// A missing file means no credentials when `required` is false, which is
    /// the case for the default location.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or parsed, an entry names no valid origin,
    /// takes both or neither of `command` and `env`, has an empty command, or
    /// repeats an origin.
    pub fn load(path: &Path, required: bool) -> Result<Self> {
        if !required && !path.exists() {
            tracing::debug!(path = %path.display(), "no credentials file");
            return Ok(Self {
                path: path.to_owned(),
                origins: BTreeMap::new(),
            });
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read the credentials file {}", path.display()))?;
        Self::parse(path, &text)
    }

    fn parse(path: &Path, text: &str) -> Result<Self> {
        let file: File = serde_norway::from_str(text)
            .with_context(|| format!("{} is not a valid credentials file", path.display()))?;
        let mut origins = BTreeMap::new();
        for entry in file.credentials {
            let Some(origin) = normalize_origin(&entry.origin) else {
                bail!(
                    "{}: `{}` is not an origin, such as `https://skills.example.com`",
                    path.display(),
                    entry.origin
                );
            };
            let method = match (entry.command, entry.env) {
                (Some(command), None)
                    if command.first().is_some_and(|program| !program.is_empty()) =>
                {
                    Method::Command(command)
                }
                (Some(_), None) => {
                    bail!("{}: the command for {origin} is empty", path.display())
                }
                (None, Some(env)) if !env.is_empty() => Method::Env(env),
                _ => bail!(
                    "{}: {origin} takes either `command` or `env`",
                    path.display()
                ),
            };
            let entry = Entry {
                method,
                token: OnceCell::new(),
            };
            if origins.insert(origin.clone(), entry).is_some() {
                bail!("{}: {origin} is listed twice", path.display());
            }
        }
        Ok(Self {
            path: path.to_owned(),
            origins,
        })
    }

    /// The token for `origin`, `None` when the file names no credentials
    /// for it.
    ///
    /// A command runs at most once per [`Credentials`], on the first call
    /// for its origin.
    ///
    /// # Errors
    ///
    /// When the command cannot start, fails or prints nothing, or the
    /// variable is unset or empty.
    pub async fn token(&self, origin: &str) -> Result<Option<Token>> {
        let Some(entry) = self.origins.get(origin) else {
            return Ok(None);
        };
        let token = entry
            .token
            .get_or_try_init(|| obtain(origin, &entry.method))
            .await
            .with_context(|| {
                format!(
                    "cannot obtain a token for {origin}, as {} configures it",
                    self.path.display()
                )
            })?;
        Ok(Some(token.clone()))
    }
}

async fn obtain(origin: &str, method: &Method) -> Result<Token> {
    let token = match method {
        Method::Env(name) => {
            tracing::debug!(%origin, variable = %name, "reading a token from the environment");
            match std::env::var(name) {
                Ok(value) => value,
                Err(_) => bail!("`{name}` is unset or not valid UTF-8"),
            }
        }
        Method::Command(command) => {
            tracing::debug!(%origin, program = %command[0], "running the credential helper");
            run_helper(command).await?
        }
    };
    let token = token.trim();
    if token.is_empty() {
        bail!("the token is empty");
    }
    Ok(Token(token.to_owned()))
}

/// Run `command` without a shell, its standard input and error left on the
/// terminal so it can sign in interactively, and read the token it prints.
async fn run_helper(command: &[String]) -> Result<String> {
    let output = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("cannot run `{}`", command[0]))?;
    if !output.status.success() {
        bail!("`{}` failed with {}", command[0], output.status);
    }
    String::from_utf8(output.stdout)
        .map_err(|_| anyhow::anyhow!("`{}` printed a token that is not UTF-8", command[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Credentials> {
        Credentials::parse(Path::new("credentials.yaml"), text)
    }

    #[test]
    fn reads_a_command_and_an_environment_variable() {
        let credentials = parse(
            "credentials:\n  - origin: https://gateway.example.com/\n    command: [my-sso-cli, token]\n  - origin: https://skills.example.org\n    env: SKILLS_TOKEN\n",
        )
        .unwrap();

        assert_eq!(
            credentials.origins["https://gateway.example.com"].method,
            Method::Command(vec!["my-sso-cli".to_owned(), "token".to_owned()])
        );
        assert_eq!(
            credentials.origins["https://skills.example.org"].method,
            Method::Env("SKILLS_TOKEN".to_owned())
        );
    }

    #[test]
    fn refuses_an_entry_with_both_or_neither_method() {
        for entry in [
            "    command: [a]\n    env: B\n",
            "    env: ''\n",
            "    command: []\n",
            "    command: ['']\n",
            "",
        ] {
            let text = format!("credentials:\n  - origin: https://x.example\n{entry}");
            assert!(parse(&text).is_err(), "{entry}");
        }
    }

    #[test]
    fn refuses_a_url_that_is_not_an_origin_and_a_repeated_origin() {
        assert!(parse("credentials:\n  - origin: https://x.example/path\n    env: A\n").is_err());
        assert!(
            parse("credentials:\n  - origin: https://x.example\n    env: A\n  - origin: https://x.example/\n    env: B\n")
                .is_err()
        );
    }

    #[test]
    fn a_missing_default_file_means_no_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.yaml");

        assert!(Credentials::load(&path, false).unwrap().origins.is_empty());
        assert!(Credentials::load(&path, true).is_err());
    }

    #[tokio::test]
    async fn an_origin_without_credentials_has_no_token() {
        let credentials = parse("credentials: []\n").unwrap();

        assert_eq!(credentials.token("https://x.example").await.unwrap(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_a_helper_once_and_trims_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let counter = dir.path().join("runs");
        let script = format!("echo run >> '{}'; printf ' s3cret\\n'", counter.display());
        let credentials = Credentials {
            path: PathBuf::from("credentials.yaml"),
            origins: BTreeMap::from([(
                "https://x.example".to_owned(),
                Entry {
                    method: Method::Command(vec!["sh".to_owned(), "-c".to_owned(), script]),
                    token: OnceCell::new(),
                },
            )]),
        };

        for _ in 0..2 {
            let token = credentials
                .token("https://x.example")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(token.expose(), "s3cret");
        }
        assert_eq!(std::fs::read_to_string(counter).unwrap(), "run\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_or_silent_helper_fails_naming_the_origin() {
        for script in ["exit 3", "true"] {
            let text = format!(
                "credentials:\n  - origin: https://x.example\n    command: [sh, -c, '{script}']\n"
            );
            let credentials = parse(&text).unwrap();

            let error = format!(
                "{:#}",
                credentials.token("https://x.example").await.unwrap_err()
            );

            assert!(error.contains("https://x.example"), "{error}");
        }
    }

    #[tokio::test]
    async fn an_unset_variable_fails_without_naming_a_value() {
        let credentials =
            parse("credentials:\n  - origin: https://x.example\n    env: POLLEN_TEST_UNSET_TOKEN_VARIABLE\n")
                .unwrap();

        let error = format!(
            "{:#}",
            credentials.token("https://x.example").await.unwrap_err()
        );

        assert!(
            error.contains("POLLEN_TEST_UNSET_TOKEN_VARIABLE"),
            "{error}"
        );
    }

    #[test]
    fn a_token_never_prints() {
        assert_eq!(
            format!("{:?}", Token("s3cret".to_owned())),
            "Token(<redacted>)"
        );
    }
}
