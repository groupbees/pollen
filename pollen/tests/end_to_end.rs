//! The whole pipeline against a real git repository and a real target
//! directory: config, fetch, discovery, install, prune.

use std::path::{Path, PathBuf};
use std::process::Command;

use pollen::command::update::{Action, Report, update};
use pollen::config::{Config, Destination, Scope};
use pollen::source::Fetcher;
use pollen::source::well_known::CredentialsFile;
use pollen::state::State;

fn fetcher_for(config: &Config, cache: &Path, offline: bool) -> Fetcher {
    Fetcher::new(
        config.base_dir.clone(),
        cache.to_path_buf(),
        offline,
        CredentialsFile {
            path: cache.join("credentials.yaml"),
            required: false,
        },
    )
}

/// A run with `--target` set to `target`.
fn into(target: &Path) -> Scope {
    Scope {
        cli: Destination::Targets(vec![target.to_path_buf()]),
        root: PathBuf::new(),
        user_config: None,
    }
}

/// A run with neither `--target` nor `--client`, with the client
/// directories under `root` as if it were the working directory.
fn plain(root: &Path) -> Scope {
    Scope {
        cli: Destination::Default,
        root: root.to_path_buf(),
        user_config: None,
    }
}

#[tokio::test]
async fn deploys_skills_from_a_git_tag_and_from_the_config_directory() {
    let origin = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    write_skill(&origin.path().join("skills/from-git"), "from-git");
    write_skill(&origin.path().join("skills/experimental"), "experimental");
    commit_and_tag(origin.path(), "v1.0.0");

    write_skill(
        &workspace.path().join("local-skills/from-local"),
        "from-local",
    );
    write_skill(&workspace.path().join("local-skills/draft/wip"), "wip");

    let config_path = workspace.path().join("pollen.yaml");
    std::fs::write(
        &config_path,
        format!(
            "targets:\n\
             \x20 - deployed\n\
             repos:\n\
             \x20 - repo: {origin}\n\
             \x20   revision: v1.0.0\n\
             \x20   paths:\n\
             \x20     - path: skills\n\
             \x20       exclude: ^experimental$\n\
             \x20 - repo: local\n\
             \x20   paths:\n\
             \x20     - path: local-skills\n\
             \x20       recurse: true\n\
             \x20       exclude: ^draft/\n",
            origin = origin.path().display()
        ),
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();
    let target = workspace.path().join("deployed");
    let fetcher = fetcher_for(&config, cache.path(), false);

    let report = update(&config, &into(&target), &fetcher, false, false, false)
        .await
        .unwrap();

    let mut installed: Vec<_> = report.summaries[0]
        .changes
        .iter()
        .map(|change| (change.action, change.name.as_str()))
        .collect();
    installed.sort_by_key(|(_, name)| *name);
    assert_eq!(
        installed,
        [(Action::Added, "from-git"), (Action::Added, "from-local")]
    );
    assert!(target.join("from-git/SKILL.md").is_file());
    assert!(target.join("from-local/SKILL.md").is_file());
    assert!(!target.join("experimental").exists());
    assert!(!target.join("wip").exists());

    let state = State::load(&target).unwrap();
    assert_eq!(
        state.entries["from-git"].revision.as_deref(),
        Some("v1.0.0")
    );
    assert_eq!(state.entries["from-git"].commit.as_ref().unwrap().len(), 40);
    assert_eq!(state.entries["from-local"].repo, "local");

    let offline = fetcher_for(&config, cache.path(), true);
    let second: Report = update(&config, &into(&target), &offline, false, false, false)
        .await
        .unwrap();
    assert_eq!(second.count(Action::Unchanged), 2);
}

#[tokio::test]
async fn a_migrated_config_deploys_what_its_version_one_did() {
    let origin = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    write_skill(&origin.path().join("skills/from-git"), "from-git");
    commit_and_tag(origin.path(), "v1.0.0");
    write_skill(&workspace.path().join("mine/from-local"), "from-local");

    let config_path = workspace.path().join("pollen.yaml");
    std::fs::write(
        &config_path,
        format!(
            "# Our skills.\n\
             targets:\n\
             \x20 - deployed\n\
             repos:\n\
             \x20 - repo: {origin}\n\
             \x20   revision: v1.0.0  # pinned\n\
             \x20   paths:\n\
             \x20     - path: skills\n\
             \x20 - repo: local\n\
             \x20   paths:\n\
             \x20     - path: mine\n",
            origin = origin.path().display()
        ),
    )
    .unwrap();
    let target = workspace.path().join("deployed");
    let v1 = Config::load(&config_path).unwrap();
    let first = update(
        &v1,
        &into(&target),
        &fetcher_for(&v1, cache.path(), false),
        false,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(first.count(Action::Added), 2);

    let options = pollen::cli::Options {
        config: config_path.clone(),
        ..pollen::cli::Options::default()
    };
    let migration = pollen::command::migrate::apply(&options, false).unwrap();
    assert_eq!((migration.from_version, migration.to_version), (1, 2));
    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(text.starts_with("# Our skills.\nversion: 2\n"), "{text}");
    assert!(text.contains("revision: v1.0.0  # pinned"), "{text}");
    assert!(!text.contains("paths:"), "{text}");

    let v2 = Config::load(&config_path).unwrap();
    let second = update(
        &v2,
        &into(&target),
        &fetcher_for(&v2, cache.path(), true),
        false,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(second.count(Action::Unchanged), 2);
}

#[tokio::test]
async fn a_moved_tag_reinstalls_the_skill() {
    let origin = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    write_skill(&origin.path().join("skills/demo"), "demo");
    commit_and_tag(origin.path(), "v1.0.0");

    let config_path = workspace.path().join("pollen.yaml");
    std::fs::write(
        &config_path,
        format!(
            "repos:\n\
             \x20 - repo: {origin}\n\
             \x20   revision: main\n\
             \x20   paths:\n\
             \x20     - path: skills\n",
            origin = origin.path().display()
        ),
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();
    let target = workspace.path().join("deployed");
    let fetcher = fetcher_for(&config, cache.path(), false);

    update(&config, &into(&target), &fetcher, false, false, false)
        .await
        .unwrap();

    std::fs::write(
        origin.path().join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: Now with more demo.\n---\n",
    )
    .unwrap();
    git(origin.path(), &["add", "."]);
    git(origin.path(), &["commit", "--quiet", "--message", "update"]);

    let report = update(&config, &into(&target), &fetcher, false, false, false)
        .await
        .unwrap();

    assert_eq!(report.count(Action::Updated), 1);
    assert!(
        std::fs::read_to_string(target.join("demo/SKILL.md"))
            .unwrap()
            .contains("Now with more demo.")
    );
}

mod destinations {
    use std::path::{Path, PathBuf};

    use pollen::command::update::{Action, Report, update};
    use pollen::config::Config;
    use pollen::source::Fetcher;
    use pollen::state::State;

    use super::{fetcher_for, into, plain, write_skill};

    /// A workspace holding the skills `alpha` and `beta` under `skills/`, and
    /// a cache.
    struct Workspace {
        root: tempfile::TempDir,
        cache: tempfile::TempDir,
    }

    impl Workspace {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            for name in ["alpha", "beta"] {
                write_skill(&root.path().join("skills").join(name), name);
            }
            Self {
                root,
                cache: tempfile::tempdir().unwrap(),
            }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.path().join(relative)
        }

        /// Write `pollen.yaml` in version 2 with `body` after `version: 2`.
        fn config(&self, body: &str) -> Config {
            let path = self.path("pollen.yaml");
            std::fs::write(&path, format!("version: 2\n{body}")).unwrap();
            Config::load(&path).unwrap()
        }

        fn fetcher(&self, config: &Config) -> Fetcher {
            fetcher_for(config, self.cache.path(), true)
        }

        async fn update(&self, config: &Config) -> Report {
            update(
                config,
                &plain(self.root.path()),
                &self.fetcher(config),
                false,
                false,
                false,
            )
            .await
            .unwrap()
        }

        fn names(&self, dir: &str) -> Vec<String> {
            State::load(&self.path(dir))
                .unwrap()
                .entries
                .into_keys()
                .collect()
        }
    }

    fn visited(report: &Report) -> Vec<PathBuf> {
        report
            .summaries
            .iter()
            .map(|summary| summary.target.clone())
            .collect()
    }

    fn skills(path: &str, destination: &str) -> String {
        format!("repos:\n  - repo: local\n    skills:\n      - path: {path}\n{destination}")
    }

    #[tokio::test]
    async fn an_entry_with_its_own_targets_deploys_only_there() {
        let workspace = Workspace::new();
        let out = workspace.path("out");
        let config = workspace.config(&format!(
            "{}      - path: skills/beta\n",
            skills(
                "skills/alpha",
                &format!("        targets: ['{}']\n", out.display())
            )
        ));

        let report = workspace.update(&config).await;

        assert_eq!(
            visited(&report),
            [
                out.clone(),
                workspace.path(".claude/skills"),
                workspace.path(".agents/skills")
            ]
        );
        assert_eq!(workspace.names("out"), ["alpha"]);
        assert_eq!(workspace.names(".claude/skills"), ["beta"]);
        assert_eq!(workspace.names(".agents/skills"), ["beta"]);
        assert!(out.join("alpha/SKILL.md").is_file());
    }

    #[tokio::test]
    async fn an_entry_with_its_own_clients_deploys_into_their_directories() {
        let workspace = Workspace::new();
        let config = workspace.config(&format!(
            "clients: claude-code\n{}      - path: skills/beta\n",
            skills("skills/alpha", "        clients: [opencode]\n")
        ));

        let report = workspace.update(&config).await;

        assert_eq!(
            visited(&report),
            [
                workspace.path(".agents/skills"),
                workspace.path(".claude/skills")
            ]
        );
        assert_eq!(workspace.names(".agents/skills"), ["alpha"]);
        assert_eq!(workspace.names(".claude/skills"), ["beta"]);
    }

    #[tokio::test]
    async fn moving_an_entry_to_targets_removes_it_from_the_client_directories() {
        let workspace = Workspace::new();
        let out = workspace.path("out");
        let before = workspace.config(&format!(
            "{}      - path: skills/beta\n",
            skills("skills/alpha", "")
        ));
        workspace.update(&before).await;
        assert_eq!(workspace.names(".claude/skills"), ["alpha", "beta"]);

        let after = workspace.config(&format!(
            "{}      - path: skills/beta\n",
            skills(
                "skills/alpha",
                &format!("        targets: ['{}']\n", out.display())
            )
        ));
        let report = workspace.update(&after).await;

        assert_eq!(report.count(Action::Removed), 2);
        assert_eq!(workspace.names(".claude/skills"), ["beta"]);
        assert_eq!(workspace.names(".agents/skills"), ["beta"]);
        assert!(!workspace.path(".claude/skills/alpha").exists());
        assert!(out.join("alpha/SKILL.md").is_file());
    }

    #[tokio::test]
    async fn dropping_a_client_prunes_its_directory() {
        let workspace = Workspace::new();
        let body = skills("skills", "");
        workspace.update(&workspace.config(&body)).await;
        assert_eq!(workspace.names(".agents/skills"), ["alpha", "beta"]);

        let report = workspace
            .update(&workspace.config(&format!("clients: claude-code\n{body}")))
            .await;

        assert_eq!(
            visited(&report),
            [
                workspace.path(".claude/skills"),
                workspace.path(".agents/skills")
            ]
        );
        assert_eq!(report.summaries[1].count(Action::Removed), 2);
        assert!(!workspace.path(".agents/skills").exists());
        assert_eq!(workspace.names(".claude/skills"), ["alpha", "beta"]);
    }

    #[tokio::test]
    async fn a_target_run_visits_nothing_else() {
        let workspace = Workspace::new();
        let config = workspace.config(&skills("skills", ""));
        workspace.update(&config).await;
        let scratch = workspace.path("scratch");

        let report = update(
            &config,
            &into(&scratch),
            &workspace.fetcher(&config),
            false,
            false,
            false,
        )
        .await
        .unwrap();

        assert_eq!(visited(&report), [scratch]);
        assert_eq!(report.count(Action::Removed), 0);
        assert_eq!(workspace.names(".claude/skills"), ["alpha", "beta"]);
        assert_eq!(workspace.names(".agents/skills"), ["alpha", "beta"]);
        assert!(Path::new(&workspace.path("scratch/alpha/SKILL.md")).is_file());
    }
}

fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: The {name} skill, for tests.\n---\n\nBody.\n"),
    )
    .unwrap();
}

fn commit_and_tag(root: &Path, tag: &str) {
    for args in [
        vec!["init", "--quiet", "--initial-branch", "main"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "user.name", "pollen tests"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "tag.gpgsign", "false"],
        vec!["add", "."],
        vec!["commit", "--quiet", "--message", "seed"],
        vec!["tag", tag],
    ] {
        git(root, &args);
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Drive `pollen validate` exactly as the command line would.
async fn run_validate(args: &[&str]) -> anyhow::Result<()> {
    use clap::Parser as _;

    let cli = pollen::cli::Cli::parse_from(std::iter::once("pollen").chain(args.iter().copied()));
    let pollen::cli::Command::Validate {
        configs,
        config_only,
        pinned,
        allow_hidden_characters,
    } = &cli.command
    else {
        panic!("not a validate invocation");
    };
    pollen::command::validate::run(
        &cli.options,
        configs,
        *config_only,
        *pinned,
        *allow_hidden_characters,
    )
    .await
}

fn write_config(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn config_only_validation_checks_every_file_it_is_given() {
    let workspace = tempfile::tempdir().unwrap();
    let good = write_config(
        workspace.path(),
        "pollen.yaml",
        "repos:\n  - repo: local\n    paths:\n      - path: skills\n",
    );
    let bad = write_config(
        workspace.path(),
        "broken.yaml",
        "repos:\n  - repo: https://example.invalid/x.git\n    paths:\n      - path: .\n",
    );

    assert!(
        run_validate(&["validate", "--config-only", &good])
            .await
            .is_ok()
    );

    let error = run_validate(&["validate", "--config-only", &good, &bad])
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("1 of 2"), "{error}");
}

#[tokio::test]
async fn config_only_validation_falls_back_to_the_config_flag() {
    let workspace = tempfile::tempdir().unwrap();
    let path = write_config(
        workspace.path(),
        "pollen.yaml",
        "repos:\n  - repo: local\n    paths:\n      - path: skills\n",
    );

    assert!(
        run_validate(&["validate", "--config-only", "--config", &path])
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn config_only_validation_does_not_look_at_the_skills() {
    let workspace = tempfile::tempdir().unwrap();
    let broken = workspace.path().join("skills/Bad-Name");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(
        broken.join("SKILL.md"),
        "---\nname: Bad-Name\ndescription: outside the specification\n---\n",
    )
    .unwrap();
    let path = write_config(
        workspace.path(),
        "pollen.yaml",
        "repos:\n  - repo: local\n    paths:\n      - path: skills\n",
    );

    assert!(
        run_validate(&["validate", "--config-only", &path])
            .await
            .is_ok(),
        "the config itself is fine, and --config-only claims nothing more"
    );

    let error = run_validate(&["validate", &path])
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("skill(s) are invalid"), "{error}");
}

#[tokio::test]
async fn validation_reports_a_config_that_does_not_exist() {
    let workspace = tempfile::tempdir().unwrap();
    let missing = workspace.path().join("absent.yaml");

    assert!(
        run_validate(&["validate", "--config-only", missing.to_str().unwrap()])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pinned_validation_requires_a_commit_on_every_git_source() {
    let workspace = tempfile::tempdir().unwrap();
    let pinned = write_config(
        workspace.path(),
        "pinned.yaml",
        "repos:\n  - repo: https://example.invalid/x.git\n    revision: acad0f52027cf8f8edf7bfa6a55e13c594d8ee71  # v0.1.0\n    paths:\n      - path: .\n  - repo: local\n    paths:\n      - path: skills\n",
    );
    let tagged = write_config(
        workspace.path(),
        "tagged.yaml",
        "repos:\n  - repo: https://example.invalid/x.git\n    revision: v0.1.0\n    paths:\n      - path: .\n",
    );

    assert!(
        run_validate(&["validate", "--config-only", "--pinned", &pinned])
            .await
            .is_ok()
    );
    assert!(
        run_validate(&["validate", "--config-only", &tagged])
            .await
            .is_ok()
    );
    let error = run_validate(&["validate", "--config-only", "--pinned", &tagged])
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("not pinned: https://example.invalid/x.git@v0.1.0"),
        "{error}"
    );
}

/// Commit and tag `tag` in `root`, dated `days_ago` days back.
fn release(root: &Path, tag: &str, days_ago: u64) {
    if !root.join(".git").exists() {
        git(root, &["init", "--quiet", "--initial-branch", "main"]);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let date = format!("{} +0000", now - days_ago * 24 * 60 * 60);
    for args in [
        vec!["commit", "--quiet", "--allow-empty", "--message", tag],
        vec!["tag", tag],
    ] {
        let output = Command::new("git")
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(&args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "pollen tests")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "pollen tests")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Drive `pollen autoupdate` exactly as the command line would.
async fn run_autoupdate(args: &[&str]) -> anyhow::Result<()> {
    use clap::Parser as _;

    let cli = pollen::cli::Cli::parse_from(std::iter::once("pollen").chain(args.iter().copied()));
    let pollen::cli::Command::Autoupdate {
        freeze,
        dry_run,
        min_release_age,
    } = &cli.command
    else {
        panic!("not an autoupdate invocation");
    };
    pollen::command::autoupdate::run(&cli.options, *freeze, *dry_run, *min_release_age).await
}

#[tokio::test]
async fn autoupdate_passes_over_a_release_younger_than_the_minimum_age() {
    let origin = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    release(origin.path(), "v1.0.0", 60);
    release(origin.path(), "v1.1.0", 30);
    release(origin.path(), "v1.2.0", 1);
    let config = write_config(
        workspace.path(),
        "pollen.yaml",
        &format!(
            "repos:\n  - repo: {}\n    revision: v1.0.0\n    paths:\n      - path: .\n",
            origin.path().display()
        ),
    );
    let online = [
        "--config",
        &config,
        "--cache-dir",
        cache.path().to_str().unwrap(),
        "autoupdate",
        "--min-release-age",
        "7d",
    ];
    let offline: Vec<&str> = std::iter::once("--offline").chain(online).collect();

    let error = run_autoupdate(&offline).await.unwrap_err().to_string();
    assert!(error.contains("drop --offline"), "{error}");

    run_autoupdate(&online).await.unwrap();

    let written = std::fs::read_to_string(&config).unwrap();
    assert!(written.contains("revision: v1.1.0\n"), "{written}");
}

mod well_known {
    use std::path::{Path, PathBuf};

    use pollen::cli::Options;
    use pollen::command::{autoupdate, outdated, update, validate};
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SCHEMA: &str = "https://schemas.agentskills.io/discovery/0.2.0/schema.json";

    fn skill_md(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n")
    }

    fn digest(body: &str) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(body.as_bytes())))
    }

    fn entry(name: &str, digest: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "type": "skill-md",
            "description": "d",
            "url": format!("{name}/SKILL.md"),
            "digest": digest,
        })
    }

    /// Serve an index listing each `(name, SKILL.md)` as a `skill-md` entry,
    /// then the `extra` entries, which have no artifact.
    async fn publish_with(
        server: &MockServer,
        skills: &[(&str, &str)],
        extra: Vec<serde_json::Value>,
    ) {
        server.reset().await;
        let mut entries: Vec<_> = skills
            .iter()
            .map(|(name, body)| entry(name, &digest(body)))
            .collect();
        entries.extend(extra);
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-skills/index.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"$schema": SCHEMA, "skills": entries})),
            )
            .mount(server)
            .await;
        for (name, body) in skills {
            Mock::given(method("GET"))
                .and(path(format!("/.well-known/agent-skills/{name}/SKILL.md")))
                .respond_with(ResponseTemplate::new(200).set_body_string(*body))
                .mount(server)
                .await;
        }
    }

    async fn publish(server: &MockServer, skills: &[(&str, &str)]) {
        publish_with(server, skills, Vec::new()).await;
    }

    fn options(config: &Path, cache: &Path, credentials: PathBuf) -> Options {
        Options {
            config: config.to_owned(),
            cache_dir: Some(cache.to_owned()),
            credentials_file: Some(credentials),
            ..Options::default()
        }
    }

    #[tokio::test]
    async fn deploys_freezes_detects_and_moves_a_pinned_skill() {
        let server = MockServer::start().await;
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let first = skill_md("alpha", "first");
        publish(
            &server,
            &[("alpha", &first), ("beta", &skill_md("beta", "b"))],
        )
        .await;
        let credentials = workspace.path().join("credentials.yaml");
        std::fs::write(&credentials, "credentials: []\n").unwrap();
        let config = workspace.path().join("pollen.yaml");
        std::fs::write(
            &config,
            format!(
                "version: 2\ndiscovery:\n  - url: {}\n    skills:\n      - name: alpha\n",
                server.uri()
            ),
        )
        .unwrap();
        let mut options = options(&config, cache.path(), credentials);
        options.targets = vec![workspace.path().join("deployed")];

        let report = update::report(&options, false, false, false).await.unwrap();
        assert_eq!(report.count(update::Action::Added), 1);
        let state = pollen::state::State::load(&workspace.path().join("deployed")).unwrap();
        assert_eq!(state.entries["alpha"].digest, Some(digest(&first)));
        assert_eq!(state.entries["alpha"].repo, server.uri());

        assert!(
            validate::check(&options, &config, true, true, false)
                .await
                .is_err()
        );
        let frozen = autoupdate::apply(&options, true, false, None)
            .await
            .unwrap();
        assert_eq!(frozen.digests.len(), 1);
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(
            text.contains(&format!(
                "      - name: alpha\n        digest: {}\n",
                digest(&first)
            )),
            "{text}"
        );
        validate::check(&options, &config, true, true, false)
            .await
            .unwrap();

        let second = skill_md("alpha", "second");
        publish(&server, &[("alpha", &second)]).await;
        let error = format!(
            "{:#}",
            update::report(&options, false, false, false)
                .await
                .unwrap_err()
        );
        assert!(error.contains("pinned to"), "{error}");
        let statuses = outdated::statuses(&options, None).await.unwrap();
        assert!(statuses[0].is_outdated(), "{}", statuses[0]);

        autoupdate::apply(&options, false, false, None)
            .await
            .unwrap();
        let report = update::report(&options, false, false, false).await.unwrap();
        assert_eq!(report.count(update::Action::Updated), 1);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("deployed/alpha/SKILL.md")).unwrap(),
            second
        );
    }

    #[tokio::test]
    async fn a_minimum_release_age_holds_back_every_changed_digest() {
        let server = MockServer::start().await;
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        publish(&server, &[("alpha", &skill_md("alpha", "new"))]).await;
        let credentials = workspace.path().join("credentials.yaml");
        std::fs::write(&credentials, "credentials: []\n").unwrap();
        let config = workspace.path().join("pollen.yaml");
        let old = digest("old");
        std::fs::write(
            &config,
            format!(
                "repos:\n  - well-known: {}\n    skills:\n      - name: alpha\n        digest: {old}\n",
                server.uri()
            ),
        )
        .unwrap();
        let options = options(&config, cache.path(), credentials);
        let week = Some(std::time::Duration::from_secs(7 * 24 * 60 * 60));

        let outcome = autoupdate::apply(&options, false, false, week)
            .await
            .unwrap();

        assert!(outcome.digests.is_empty());
        assert!(std::fs::read_to_string(&config).unwrap().contains(&old));
    }

    #[tokio::test]
    async fn every_command_skips_an_invalid_entry_unless_it_is_listed() {
        let server = MockServer::start().await;
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let alpha = skill_md("alpha", "a");
        publish_with(
            &server,
            &[("alpha", &alpha)],
            vec![
                entry("PDF_Summarizer", &digest(&alpha)),
                entry("broken", "sha256:not-a-digest"),
            ],
        )
        .await;
        let credentials = workspace.path().join("credentials.yaml");
        std::fs::write(&credentials, "credentials: []\n").unwrap();
        let config = workspace.path().join("pollen.yaml");
        let follow = format!("repos:\n  - well-known: {}\n", server.uri());
        std::fs::write(&config, &follow).unwrap();
        let mut options = options(&config, cache.path(), credentials);
        options.targets = vec![workspace.path().join("deployed")];

        let report = update::report(&options, false, false, false).await.unwrap();
        assert_eq!(report.count(update::Action::Added), 1);
        assert!(workspace.path().join("deployed/alpha/SKILL.md").is_file());
        let validation = validate::check(&options, &config, false, false, false)
            .await
            .unwrap();
        assert_eq!(validation.skills.unwrap().len(), 1);
        assert!(outdated::statuses(&options, None).await.unwrap().is_empty());
        let frozen = autoupdate::apply(&options, true, false, None)
            .await
            .unwrap();
        assert!(frozen.digests.is_empty());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), follow);

        std::fs::write(
            &config,
            format!("{follow}    skills:\n      - name: alpha\n      - name: broken\n"),
        )
        .unwrap();
        let errors = [
            format!(
                "{:#}",
                update::report(&options, false, false, false)
                    .await
                    .unwrap_err()
            ),
            format!(
                "{:#}",
                validate::check(&options, &config, false, false, false)
                    .await
                    .unwrap_err()
            ),
            format!(
                "{:#}",
                outdated::statuses(&options, None).await.unwrap_err()
            ),
            format!(
                "{:#}",
                autoupdate::apply(&options, true, false, None)
                    .await
                    .unwrap_err()
            ),
        ];
        for error in errors {
            assert!(error.contains("`broken` has the digest"), "{error}");
        }
    }
}

/// Agents deployed by the `pollen` binary, run from a workspace as a user
/// would, so the client directories resolve against its working directory.
mod agents {
    use std::fmt::Write as _;
    use std::path::Path;
    use std::process::{Command, Output};

    use pollen::state::{Kind, State};

    use super::{commit_and_tag, write_skill};

    const CLAUDE_REVIEWER: &str = "---\nname: reviewer\ndescription: Reviews code.\ntools: Read, Grep\nmodel: opus\npermissionMode: plan\nhooks:\n  Stop: []\n---\nYou review code.\n";
    const OPENCODE_REVIEWER: &str = "---\ndescription: Reviews code.\nmodel: anthropic/claude-sonnet-4-5\npermission:\n  edit: deny\n---\nYou review code.\n";

    /// A git repository holding a `reviewer` skill, and a `reviewer` agent
    /// written for each client beside a README the config excludes.
    fn catalog() -> tempfile::TempDir {
        let origin = tempfile::tempdir().unwrap();
        write_skill(&origin.path().join("skills/reviewer"), "reviewer");
        for (dir, text) in [("claude", CLAUDE_REVIEWER), ("opencode", OPENCODE_REVIEWER)] {
            let agents = origin.path().join("agents").join(dir);
            std::fs::create_dir_all(&agents).unwrap();
            std::fs::write(agents.join("reviewer.md"), text).unwrap();
            std::fs::write(agents.join("README.md"), "# Agents\n").unwrap();
        }
        commit_and_tag(origin.path(), "v1.0.0");
        origin
    }

    fn config(origin: &Path, clients: &str, agents: &[&str]) -> String {
        let mut text = format!(
            "version: 2\nclients: {clients}\nrepos:\n  - repo: {}\n    revision: v1.0.0\n    skills:\n      - path: skills\n    agents:\n",
            origin.display()
        );
        for client in agents {
            let dir = if *client == "claude-code" {
                "claude"
            } else {
                "opencode"
            };
            let _ = write!(
                text,
                "      - path: agents/{dir}\n        clients: {client}\n        exclude: ^README$\n"
            );
        }
        text
    }

    /// Run `pollen` in `workspace` with nothing inherited from the caller's
    /// `POLLEN_*` variables.
    fn pollen(
        workspace: &Path,
        cache: &Path,
        args: &[&str],
        env: &[(&str, Option<&Path>)],
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pollen"));
        command
            .current_dir(workspace)
            .arg("--cache-dir")
            .arg(cache)
            .args(args)
            // GitHub's Windows runners set `core.autocrlf=true`, so the
            // checkout pollen copies would hold CRLF and never equal the LF
            // bytes these tests wrote.
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "core.autocrlf")
            .env("GIT_CONFIG_VALUE_0", "false");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("POLLEN_") {
                command.env_remove(name);
            }
        }
        for (name, value) in env {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        command.output().unwrap()
    }

    fn succeeds(output: &Output) -> String {
        assert!(
            output.status.success(),
            "pollen failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout.clone()).unwrap()
    }

    fn fails(output: &Output) -> String {
        assert!(!output.status.success(), "pollen succeeded");
        String::from_utf8(output.stderr.clone()).unwrap()
    }

    #[test]
    fn deploys_agents_for_each_client_and_prunes_what_the_config_drops() {
        let origin = catalog();
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        let yaml = ws.join("pollen.yaml");
        std::fs::write(
            &yaml,
            config(origin.path(), "all", &["claude-code", "opencode"]),
        )
        .unwrap();

        let first = succeeds(&pollen(ws, cache.path(), &["update"], &[]));
        let claude = ws.join(".claude/agents");
        let opencode = ws.join(".opencode/agents");
        assert_eq!(
            std::fs::read_to_string(claude.join("reviewer.md")).unwrap(),
            CLAUDE_REVIEWER
        );
        assert_eq!(
            std::fs::read_to_string(opencode.join("reviewer.md")).unwrap(),
            OPENCODE_REVIEWER
        );
        assert!(ws.join(".claude/skills/reviewer/SKILL.md").is_file());
        assert!(ws.join(".agents/skills/reviewer/SKILL.md").is_file());
        assert!(!claude.join("README.md").exists());
        assert_eq!(State::load(&claude).unwrap().kind, Kind::Agents);
        assert!(
            first.contains("tools: Read, Grep; model: opus; permission: plan; hooks\n"),
            "{first}"
        );
        assert!(
            first.contains("model: anthropic/claude-sonnet-4-5; permission: edit: deny\n"),
            "{first}"
        );
        assert!(
            first.contains("1 agent in .claude/agents: 1 added"),
            "{first}"
        );

        let second = succeeds(&pollen(ws, cache.path(), &["update", "--offline"], &[]));
        assert!(
            second.contains("1 agent in .opencode/agents: 1 unchanged"),
            "{second}"
        );
        assert!(
            second.contains("1 skill in .claude/skills: 1 unchanged"),
            "{second}"
        );

        let listed = succeeds(&pollen(ws, cache.path(), &["list"], &[]));
        assert!(
            listed.contains(".claude/agents (agents):\n  reviewer"),
            "{listed}"
        );

        std::fs::write(&yaml, config(origin.path(), "all", &["claude-code"])).unwrap();
        let dropped_agents = succeeds(&pollen(ws, cache.path(), &["update", "--offline"], &[]));
        assert!(
            dropped_agents.contains("0 agents in .opencode/agents: 1 removed"),
            "{dropped_agents}"
        );
        assert!(!opencode.exists());
        assert!(claude.join("reviewer.md").is_file());

        std::fs::write(
            &yaml,
            config(origin.path(), "opencode", &["claude-code", "opencode"]),
        )
        .unwrap();
        let dropped_client = succeeds(&pollen(ws, cache.path(), &["update", "--offline"], &[]));
        assert!(
            dropped_client.contains("0 skills in .claude/skills: 1 removed"),
            "{dropped_client}"
        );
        assert!(!ws.join(".claude/skills").exists());
        assert!(
            claude.join("reviewer.md").is_file(),
            "an item naming its clients keeps them"
        );
        assert!(opencode.join("reviewer.md").is_file());
        assert!(ws.join(".agents/skills/reviewer").is_dir());
    }

    #[test]
    fn a_run_with_a_target_deploys_skills_only_and_a_client_run_keeps_what_the_config_sends_it() {
        let origin = catalog();
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        std::fs::write(
            ws.join("pollen.yaml"),
            config(origin.path(), "all", &["claude-code", "opencode"]),
        )
        .unwrap();

        let output = succeeds(&pollen(
            ws,
            cache.path(),
            &["update", "--target", "out"],
            &[],
        ));
        assert!(ws.join("out/reviewer/SKILL.md").is_file());
        assert!(!ws.join(".claude").exists(), "{output}");
        assert!(!ws.join(".opencode").exists(), "{output}");

        succeeds(&pollen(
            ws,
            cache.path(),
            &["update", "--offline", "--client", "opencode"],
            &[],
        ));
        assert_eq!(
            std::fs::read(ws.join(".opencode/agents/reviewer.md")).unwrap(),
            std::fs::read(origin.path().join("agents/opencode/reviewer.md")).unwrap()
        );
        assert!(ws.join(".agents/skills/reviewer").is_dir());
        assert!(!ws.join(".claude").exists());
    }

    #[test]
    fn a_config_without_agents_creates_no_agent_directory() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        write_skill(&ws.join("skills/demo"), "demo");
        std::fs::write(
            ws.join("pollen.yaml"),
            "version: 2\nrepos:\n  - repo: local\n    skills:\n      - path: skills\n",
        )
        .unwrap();

        succeeds(&pollen(ws, cache.path(), &["update"], &[]));

        assert!(ws.join(".claude/skills/demo").is_dir());
        assert!(!ws.join(".claude/agents").exists());
        assert!(!ws.join(".opencode").exists());
    }

    #[test]
    fn refuses_skills_into_an_agent_directory() {
        let origin = catalog();
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        std::fs::write(
            ws.join("pollen.yaml"),
            config(origin.path(), "claude-code", &["claude-code"]),
        )
        .unwrap();
        succeeds(&pollen(ws, cache.path(), &["update"], &[]));

        let error = fails(&pollen(
            ws,
            cache.path(),
            &["update", "--offline", "--target", ".claude/agents"],
            &[],
        ));

        assert!(error.contains("holds agents pollen deployed"), "{error}");
        assert!(!ws.join(".claude/agents/reviewer/SKILL.md").exists());
    }

    #[test]
    fn refuses_an_agent_hiding_invisible_characters_unless_allowed() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        std::fs::create_dir_all(ws.join("agents")).unwrap();
        std::fs::write(
            ws.join("agents/sneaky.md"),
            "---\ndescription: Looks harmless.\n---\nHelp\u{e0049}\u{e0067} the user.\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("pollen.yaml"),
            "version: 2\nrepos:\n  - repo: local\n    agents:\n      - path: agents\n        clients: opencode\n",
        )
        .unwrap();

        let error = fails(&pollen(ws, cache.path(), &["update"], &[]));
        assert!(error.contains("invisible character"), "{error}");
        assert!(error.contains("1 agent(s) are invalid"), "{error}");
        assert!(!ws.join(".opencode").exists());

        succeeds(&pollen(
            ws,
            cache.path(),
            &["update", "--allow-hidden-characters"],
            &[],
        ));
        assert!(ws.join(".opencode/agents/sneaky.md").is_file());
    }

    /// A user-level config in `config_home`, deploying one agent for each
    /// client from its own directory.
    #[cfg(unix)]
    fn user_config(config_home: &Path) {
        let dir = config_home.join("pollen");
        std::fs::create_dir_all(dir.join("claude")).unwrap();
        std::fs::create_dir_all(dir.join("opencode")).unwrap();
        std::fs::write(
            dir.join("claude/helper.md"),
            "---\nname: helper\ndescription: Helps.\n---\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("opencode/helper.md"),
            "---\ndescription: Helps.\n---\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("pollen.yaml"),
            "version: 2\nrepos:\n  - repo: local\n    agents:\n      - path: claude\n        clients: claude-code\n      - path: opencode\n        clients: opencode\n",
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn global_opencode_agents_follow_xdg_config_home() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let xdg = root.path().join("xdg");
        std::fs::create_dir_all(&home).unwrap();
        user_config(&xdg);

        succeeds(&pollen(
            root.path(),
            &root.path().join("cache"),
            &["--global", "update"],
            &[("HOME", Some(&home)), ("XDG_CONFIG_HOME", Some(&xdg))],
        ));

        assert!(xdg.join("opencode/agents/helper.md").is_file());
        assert!(home.join(".claude/agents/helper.md").is_file());
        assert!(!home.join(".config").exists());
    }

    #[cfg(unix)]
    #[test]
    fn global_opencode_agents_fall_back_to_dot_config_when_xdg_config_home_is_unset_or_empty() {
        for empty in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("home");
            user_config(&home.join(".config"));
            let blank = std::path::PathBuf::new();
            let xdg = empty.then_some(blank.as_path());

            succeeds(&pollen(
                root.path(),
                &root.path().join("cache"),
                &["--global", "update"],
                &[("HOME", Some(&home)), ("XDG_CONFIG_HOME", xdg)],
            ));

            assert!(
                home.join(".config/opencode/agents/helper.md").is_file(),
                "empty: {empty}"
            );
            assert!(
                home.join(".claude/agents/helper.md").is_file(),
                "empty: {empty}"
            );
        }
    }
}
