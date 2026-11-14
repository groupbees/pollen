//! The `well-known` side of materialization: an Agent Skills discovery index
//! (<https://github.com/cloudflare/agent-skills-discovery-rfc>, v0.2.0), and
//! the artifacts it lists, verified and unpacked under the cache.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use super::archive::{self, Format};
use super::{Fetched, Materialized};
use crate::config::{RepoSpec, Source, is_digest};
use crate::credentials::Credentials;
use crate::http::Client;
use crate::skill::{self, SKILL_FILE};

/// Where an origin serves its index.
pub const INDEX_PATH: &str = "/.well-known/agent-skills/index.json";

/// The one index version pollen reads.
pub const SCHEMA: &str = "https://schemas.agentskills.io/discovery/0.2.0/schema.json";

const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

/// The distribution type of a single `SKILL.md`.
pub const SKILL_MD: &str = "skill-md";
/// The distribution type of an archive holding the whole skill directory.
pub const ARCHIVE: &str = "archive";

/// One skill an index lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The skill's name.
    pub name: String,
    /// `skill-md` or `archive`; other values are skipped.
    #[serde(rename = "type")]
    pub kind: String,
    /// What the skill does.
    #[serde(default)]
    pub description: String,
    /// Where the artifact is, relative to the index URL.
    pub url: String,
    /// `sha256:` and the hex SHA-256 of the artifact.
    pub digest: String,
}

/// A parsed index, and the URL its relative links resolve against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// The URL the index was read from, after redirects.
    pub url: Url,
    /// The skills it lists, in its order.
    pub skills: Vec<Entry>,
}

#[derive(Deserialize)]
struct RawIndex {
    #[serde(rename = "$schema")]
    schema: Option<String>,
    skills: Vec<Entry>,
}

/// The cached copy of an index: its URL, and its body as served.
#[derive(Serialize, Deserialize)]
struct Cached {
    url: String,
    body: String,
}

impl Index {
    /// Read an index served at `url`.
    ///
    /// # Errors
    ///
    /// When the body is not an index, its `$schema` is missing or not the one
    /// pollen reads, or it lists a name twice.
    pub fn parse(body: &str, url: Url) -> Result<Self> {
        let raw: RawIndex = serde_json::from_str(body)
            .with_context(|| format!("{url} is not an Agent Skills discovery index"))?;
        match raw.schema.as_deref() {
            Some(SCHEMA) => {}
            None => bail!(
                "{url} has no `$schema`, so it is a v0.1.0 index, which pollen does not read; it reads {SCHEMA}"
            ),
            Some(other) => {
                bail!(
                    "{url} uses the index schema {other}, which pollen does not read; it reads {SCHEMA}"
                )
            }
        }
        let mut seen = BTreeSet::new();
        if let Some(duplicate) = raw.skills.iter().find(|entry| !seen.insert(&entry.name)) {
            bail!("{url} lists the skill `{}` twice", duplicate.name);
        }
        Ok(Self {
            url,
            skills: raw.skills,
        })
    }

    /// The entries `repo` selects: the ones its `skills` list names, each
    /// matching its pinned `digest`, else every entry of a known type that
    /// `exclude` does not reject.
    ///
    /// # Errors
    ///
    /// When a listed skill is missing from the index, has a type pollen does
    /// not know, or is published with a digest other than the pinned one.
    pub fn select(&self, repo: &RepoSpec) -> Result<Vec<&Entry>> {
        let url = &self.url;
        let Some(pins) = &repo.skills else {
            let exclude = repo.exclude_regex()?;
            let mut selected = Vec::new();
            for entry in &self.skills {
                if exclude
                    .as_ref()
                    .is_some_and(|pattern| pattern.is_match(&entry.name))
                {
                    tracing::debug!(skill = %entry.name, "excluded");
                } else if known(&entry.kind) {
                    selected.push(entry);
                } else {
                    tracing::warn!(
                        "{url}: `{}` has the type `{}`, which pollen does not know; skipped",
                        entry.name,
                        entry.kind
                    );
                }
            }
            return Ok(selected);
        };

        pins.iter()
            .map(|pin| {
                let entry = self
                    .skills
                    .iter()
                    .find(|entry| entry.name == pin.name)
                    .with_context(|| format!("{url} does not list the skill `{}`", pin.name))?;
                if !known(&entry.kind) {
                    bail!(
                        "{url}: `{}` has the type `{}`, which pollen does not know",
                        entry.name,
                        entry.kind
                    );
                }
                if let Some(digest) = &pin.digest
                    && *digest != entry.digest
                {
                    bail!(
                        "{url}: `{}` is pinned to {digest}, but the index publishes {}. The publisher replaced it, and an index serves only the current version; `pollen outdated` shows the change and `pollen autoupdate` moves the pin",
                        entry.name,
                        entry.digest
                    );
                }
                Ok(entry)
            })
            .collect()
    }
}

fn known(kind: &str) -> bool {
    matches!(kind, SKILL_MD | ARCHIVE)
}

/// Which credentials file to read, and whether it must exist.
#[derive(Debug, Clone)]
pub struct CredentialsFile {
    /// The file.
    pub path: PathBuf,
    /// Whether a missing file is an error: it is when named explicitly.
    pub required: bool,
}

/// Reads indexes and artifacts, through a cache, with the credentials of a
/// user-level file.
///
/// The HTTP client, and with it the credentials file, is only set up on the
/// first download, so an offline run from the cache never reads it.
#[derive(Debug)]
pub struct WellKnown {
    cache_dir: PathBuf,
    offline: bool,
    credentials: CredentialsFile,
    client: OnceCell<Client>,
}

impl WellKnown {
    /// Read through `cache_dir`, only from it when `offline`.
    #[must_use]
    pub fn new(cache_dir: PathBuf, offline: bool, credentials: CredentialsFile) -> Self {
        Self {
            cache_dir,
            offline,
            credentials,
            client: OnceCell::new(),
        }
    }

    async fn client(&self) -> Result<&Client> {
        self.client
            .get_or_try_init(|| async {
                let credentials =
                    Credentials::load(&self.credentials.path, self.credentials.required)?;
                Client::new(credentials)
            })
            .await
    }

    fn root(&self) -> PathBuf {
        self.cache_dir.join("well-known")
    }

    /// The index `origin` publishes; with `offline`, the copy cached by the
    /// last run that read it.
    ///
    /// # Errors
    ///
    /// When the index cannot be downloaded or parsed, or `offline` is set and
    /// no copy is cached.
    pub async fn index(&self, origin: &str) -> Result<Index> {
        let cache = self
            .root()
            .join("indexes")
            .join(format!("{}.json", cache_key(origin)));
        if self.offline {
            let text = std::fs::read_to_string(&cache).with_context(|| {
                format!("no index of {origin} is cached; drop --offline to fetch it")
            })?;
            let cached: Cached = serde_json::from_str(&text)
                .with_context(|| format!("{} is corrupt; drop --offline", cache.display()))?;
            let url = Url::parse(&cached.url)
                .with_context(|| format!("{} is corrupt; drop --offline", cache.display()))?;
            return Index::parse(&cached.body, url);
        }

        let url = Url::parse(&format!("{origin}{INDEX_PATH}"))
            .with_context(|| format!("`{origin}` is not an origin"))?;
        let download = self.client().await?.get(&url, MAX_INDEX_BYTES).await?;
        let body = String::from_utf8(download.bytes)
            .with_context(|| format!("{} is not UTF-8", download.url))?;
        let index = Index::parse(&body, download.url.clone())?;
        write_atomic(
            &cache,
            &serde_json::to_vec(&Cached {
                url: download.url.to_string(),
                body,
            })?,
        )?;
        tracing::debug!(%origin, count = index.skills.len(), "read the index");
        Ok(index)
    }

    /// Make the skills `repo` selects available under the cache, one
    /// directory per skill, named after it.
    ///
    /// # Errors
    ///
    /// When `repo` is not a `well-known` source, its index cannot be read,
    /// the selection fails, or an artifact cannot be downloaded, verified or
    /// unpacked.
    pub async fn materialize(&self, repo: &RepoSpec) -> Result<Materialized> {
        let Source::WellKnown { origin } = repo.source() else {
            bail!("{} is not a well-known source", repo.label());
        };
        let index = self.index(&origin).await?;
        let root = self.root().join("artifacts");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("cannot create the cache directory {}", root.display()))?;

        let mut skills = Vec::new();
        for entry in index.select(repo)? {
            let dir = self
                .artifact(&index, entry, &root)
                .await
                .with_context(|| format!("cannot install `{}` from {origin}", entry.name))?;
            skills.push(Fetched {
                name: entry.name.clone(),
                dir,
                digest: entry.digest.clone(),
            });
        }
        Ok(Materialized {
            root,
            resolved: None,
            skills,
        })
    }

    /// The skill directory of `entry`, downloaded and unpacked unless a
    /// previous run left it in the cache.
    async fn artifact(&self, index: &Index, entry: &Entry, root: &Path) -> Result<PathBuf> {
        skill::validate_name(&entry.name).map_err(|error| {
            anyhow::anyhow!("`{}` is not a valid skill name: {error}", entry.name)
        })?;
        if !is_digest(&entry.digest) {
            bail!(
                "the index gives `{}` the digest `{}`, not `sha256:` and 64 lowercase hex digits",
                entry.name,
                entry.digest
            );
        }
        let hex = &entry.digest["sha256:".len()..];
        let unpacked = root.join(hex);
        let dir = unpacked.join(&entry.name);
        if dir.join(SKILL_FILE).is_file() {
            tracing::debug!(skill = %entry.name, digest = %entry.digest, "cached");
            return Ok(dir);
        }
        if self.offline {
            bail!("it is not in the cache; drop --offline to download it");
        }

        let url = index
            .url
            .join(&entry.url)
            .with_context(|| format!("`{}` is not a valid URL", entry.url))?;
        let download = self.client().await?.get(&url, MAX_ARTIFACT_BYTES).await?;
        let actual = hex::encode(Sha256::digest(&download.bytes));
        if actual != hex {
            bail!(
                "{} does not match its digest: the index says {}, the download is sha256:{actual}",
                download.url,
                entry.digest
            );
        }

        let staging = tempfile::Builder::new()
            .prefix(".pollen-staging-")
            .tempdir_in(root)
            .with_context(|| format!("cannot write into {}", root.display()))?;
        let staged = staging.path().join(&entry.name);
        std::fs::create_dir(&staged)?;
        if entry.kind == SKILL_MD {
            std::fs::write(staged.join(SKILL_FILE), &download.bytes)?;
        } else {
            let format = Format::detect(download.content_type.as_deref(), &download.url)
                .with_context(|| {
                    format!(
                        "cannot tell the format of {}: neither its Content-Type nor its extension names .tar.gz or .zip",
                        download.url
                    )
                })?;
            archive::extract(
                &download.bytes,
                format,
                &staged,
                archive::MAX_UNPACKED_BYTES,
            )?;
            if !staged.join(SKILL_FILE).is_file() {
                bail!(
                    "the archive at {} has no SKILL.md at its root",
                    download.url
                );
            }
        }

        if let Err(error) = std::fs::rename(staging.path(), &unpacked) {
            // Another run unpacked the same digest in the meantime.
            if !dir.join(SKILL_FILE).is_file() {
                return Err(error)
                    .with_context(|| format!("cannot move {} into the cache", entry.name));
            }
        }
        tracing::debug!(skill = %entry.name, digest = %entry.digest, path = %dir.display(), "unpacked");
        Ok(dir)
    }
}

/// One file per origin: a readable prefix, and a hash so two origins never
/// share one.
fn cache_key(origin: &str) -> String {
    let digest = Sha256::digest(origin.as_bytes());
    let slug: String = origin
        .split("://")
        .last()
        .unwrap_or(origin)
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '.' {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("{slug}-{}", hex::encode(&digest[..8]))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path.parent().context("the cache file has no parent")?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("cannot create {}", directory.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".pollen-")
        .tempfile_in(directory)
        .with_context(|| format!("cannot write into {}", directory.display()))?;
    std::fs::write(staging.path(), bytes)?;
    staging
        .persist(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SkillPin;
    use std::io::Write;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn digest(bytes: &[u8]) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
    }

    fn skill_md(name: &str) -> Vec<u8> {
        format!("---\nname: {name}\ndescription: {name} things\n---\n").into_bytes()
    }

    fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *body).unwrap();
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&builder.into_inner().unwrap()).unwrap();
        encoder.finish().unwrap()
    }

    fn entry(name: &str, kind: &str, url: &str, digest: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name, "type": kind, "description": "d", "url": url, "digest": digest,
        })
    }

    fn index_body(entries: &[serde_json::Value]) -> String {
        serde_json::json!({"$schema": SCHEMA, "skills": entries}).to_string()
    }

    fn index(entries: &[serde_json::Value]) -> Index {
        Index::parse(
            &index_body(entries),
            Url::parse("https://x.example/.well-known/agent-skills/index.json").unwrap(),
        )
        .unwrap()
    }

    fn repo(skills: Option<Vec<(&str, Option<&str>)>>, exclude: Option<&str>) -> RepoSpec {
        RepoSpec {
            well_known: Some("https://x.example".to_owned()),
            exclude: exclude.map(str::to_owned),
            skills: skills.map(|skills| {
                skills
                    .into_iter()
                    .map(|(name, digest)| SkillPin {
                        name: name.to_owned(),
                        digest: digest.map(str::to_owned),
                    })
                    .collect()
            }),
            ..RepoSpec::default()
        }
    }

    fn names(entries: &[&Entry]) -> Vec<String> {
        entries.iter().map(|entry| entry.name.clone()).collect()
    }

    const A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn reads_only_the_schema_it_knows() {
        let url = Url::parse("https://x.example/index.json").unwrap();

        assert!(Index::parse(r#"{"skills": []}"#, url.clone()).is_err());
        assert!(
            Index::parse(
                r#"{"$schema": "https://example.invalid/9", "skills": []}"#,
                url.clone()
            )
            .is_err()
        );
        let index = Index::parse(
            &format!(r#"{{"$schema": "{SCHEMA}", "skills": [], "extra": 1}}"#),
            url,
        )
        .unwrap();
        assert!(index.skills.is_empty());
    }

    #[test]
    fn refuses_an_index_listing_a_name_twice() {
        let body = index_body(&[entry("a", SKILL_MD, "a", A), entry("a", SKILL_MD, "b", B)]);

        assert!(Index::parse(&body, Url::parse("https://x.example/i").unwrap()).is_err());
    }

    #[test]
    fn selects_every_known_skill_but_the_excluded_ones() {
        let index = index(&[
            entry("keep", SKILL_MD, "k", A),
            entry("experimental-x", ARCHIVE, "e", A),
            entry("future", "plugin", "f", A),
        ]);

        assert_eq!(
            names(&index.select(&repo(None, None)).unwrap()),
            ["keep", "experimental-x"]
        );
        assert_eq!(
            names(&index.select(&repo(None, Some("^experimental-"))).unwrap()),
            ["keep"]
        );
    }

    #[test]
    fn selects_the_listed_skills_at_their_pinned_digest() {
        let index = index(&[entry("a", SKILL_MD, "a", A), entry("b", SKILL_MD, "b", B)]);

        let selected = index
            .select(&repo(Some(vec![("b", Some(B)), ("a", None)]), None))
            .unwrap();
        assert_eq!(names(&selected), ["b", "a"]);

        let error = index
            .select(&repo(Some(vec![("a", Some(B))]), None))
            .unwrap_err()
            .to_string();
        assert!(error.contains("pinned to") && error.contains(A), "{error}");

        assert!(
            index
                .select(&repo(Some(vec![("missing", None)]), None))
                .is_err()
        );
    }

    fn well_known(cache: &Path, offline: bool) -> WellKnown {
        WellKnown::new(
            cache.to_path_buf(),
            offline,
            CredentialsFile {
                path: cache.join("credentials.yaml"),
                required: false,
            },
        )
    }

    async fn serve(server: &MockServer, route: &str, body: Vec<u8>, content_type: &str) {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, content_type))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn downloads_verifies_and_unpacks_then_serves_the_cache_offline() {
        let server = MockServer::start().await;
        let markdown = skill_md("single");
        let archive = tar_gz(&[
            ("SKILL.md", &skill_md("bundle")),
            ("references/more.md", b"more"),
        ]);
        serve(
            &server,
            INDEX_PATH,
            index_body(&[
                entry("single", SKILL_MD, "single/SKILL.md", &digest(&markdown)),
                entry("bundle", ARCHIVE, "/files/bundle.tar.gz", &digest(&archive)),
            ])
            .into_bytes(),
            "application/json",
        )
        .await;
        serve(
            &server,
            "/.well-known/agent-skills/single/SKILL.md",
            markdown.clone(),
            "text/markdown",
        )
        .await;
        serve(
            &server,
            "/files/bundle.tar.gz",
            archive,
            "application/octet-stream",
        )
        .await;
        let cache = tempfile::tempdir().unwrap();
        let mut source = repo(None, None);
        source.well_known = Some(server.uri());

        let online = well_known(cache.path(), false)
            .materialize(&source)
            .await
            .unwrap();

        let dirs: Vec<_> = online
            .skills
            .iter()
            .map(|skill| skill.dir.clone())
            .collect();
        assert_eq!(std::fs::read(dirs[0].join(SKILL_FILE)).unwrap(), markdown);
        assert_eq!(
            std::fs::read(dirs[1].join("references/more.md")).unwrap(),
            b"more"
        );
        assert!(dirs[1].ends_with("bundle"));

        drop(server);
        let offline = well_known(cache.path(), true)
            .materialize(&source)
            .await
            .unwrap();
        assert_eq!(offline, online);
    }

    #[tokio::test]
    async fn refuses_an_artifact_that_does_not_match_its_digest() {
        let server = MockServer::start().await;
        serve(
            &server,
            INDEX_PATH,
            index_body(&[entry("single", SKILL_MD, "/single.md", A)]).into_bytes(),
            "application/json",
        )
        .await;
        serve(&server, "/single.md", skill_md("single"), "text/markdown").await;
        let cache = tempfile::tempdir().unwrap();
        let mut source = repo(None, None);
        source.well_known = Some(server.uri());

        let error = format!(
            "{:#}",
            well_known(cache.path(), false)
                .materialize(&source)
                .await
                .unwrap_err()
        );

        assert!(error.contains("does not match its digest"), "{error}");
        let artifacts = cache.path().join("well-known/artifacts");
        assert_eq!(std::fs::read_dir(artifacts).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn offline_without_a_cached_index_says_so() {
        let cache = tempfile::tempdir().unwrap();

        let error = well_known(cache.path(), true)
            .materialize(&repo(None, None))
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("--offline"), "{error}");
    }
}
