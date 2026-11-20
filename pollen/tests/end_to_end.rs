//! The whole pipeline against a real git repository and a real target
//! directory: config, fetch, discovery, install, prune.

use std::path::Path;
use std::process::Command;

use pollen::command::update::{Action, Report, update};
use pollen::config::Config;
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

    let report = update(
        &config,
        std::slice::from_ref(&target),
        &fetcher,
        false,
        false,
    )
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
    assert_eq!(state.skills["from-git"].revision.as_deref(), Some("v1.0.0"));
    assert_eq!(state.skills["from-git"].commit.as_ref().unwrap().len(), 40);
    assert_eq!(state.skills["from-local"].repo, "local");

    let offline = fetcher_for(&config, cache.path(), true);
    let second: Report = update(
        &config,
        std::slice::from_ref(&target),
        &offline,
        false,
        false,
    )
    .await
    .unwrap();
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

    update(
        &config,
        std::slice::from_ref(&target),
        &fetcher,
        false,
        false,
    )
    .await
    .unwrap();

    std::fs::write(
        origin.path().join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: Now with more demo.\n---\n",
    )
    .unwrap();
    git(origin.path(), &["add", "."]);
    git(origin.path(), &["commit", "--quiet", "--message", "update"]);

    let report = update(
        &config,
        std::slice::from_ref(&target),
        &fetcher,
        false,
        false,
    )
    .await
    .unwrap();

    assert_eq!(report.count(Action::Updated), 1);
    assert!(
        std::fs::read_to_string(target.join("demo/SKILL.md"))
            .unwrap()
            .contains("Now with more demo.")
    );
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
    } = &cli.command
    else {
        panic!("not a validate invocation");
    };
    let (configs, config_only, pinned) = (configs.clone(), *config_only, *pinned);
    pollen::command::validate::run(&cli.options, &configs, config_only, pinned).await
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
                "repos:\n  - well-known: {}\n    skills:\n      - name: alpha\n",
                server.uri()
            ),
        )
        .unwrap();
        let mut options = options(&config, cache.path(), credentials);
        options.targets = vec![workspace.path().join("deployed")];

        let report = update::report(&options, false, false).await.unwrap();
        assert_eq!(report.count(update::Action::Added), 1);
        let state = pollen::state::State::load(&workspace.path().join("deployed")).unwrap();
        assert_eq!(state.skills["alpha"].digest, Some(digest(&first)));
        assert_eq!(state.skills["alpha"].repo, server.uri());

        assert!(
            validate::check(&options, &config, true, true)
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
        validate::check(&options, &config, true, true)
            .await
            .unwrap();

        let second = skill_md("alpha", "second");
        publish(&server, &[("alpha", &second)]).await;
        let error = format!(
            "{:#}",
            update::report(&options, false, false).await.unwrap_err()
        );
        assert!(error.contains("pinned to"), "{error}");
        let statuses = outdated::statuses(&options, None).await.unwrap();
        assert!(statuses[0].is_outdated(), "{}", statuses[0]);

        autoupdate::apply(&options, false, false, None)
            .await
            .unwrap();
        let report = update::report(&options, false, false).await.unwrap();
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

        let report = update::report(&options, false, false).await.unwrap();
        assert_eq!(report.count(update::Action::Added), 1);
        assert!(workspace.path().join("deployed/alpha/SKILL.md").is_file());
        let validation = validate::check(&options, &config, false, false)
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
                update::report(&options, false, false).await.unwrap_err()
            ),
            format!(
                "{:#}",
                validate::check(&options, &config, false, false)
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
