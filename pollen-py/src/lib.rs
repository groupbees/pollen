//! The `pollen._pollen` extension module: each function runs one pollen
//! command and hands back the dataclasses `pollen._types` defines.

use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use pollen::agent::Details;
use pollen::cli::Options;
use pollen::command;
use pollen::command::autoupdate::DigestBump;
use pollen::command::outdated::{Entry, Pending};
use pollen::command::show::AgentDirectory;
use pollen::config::{Client, ClientChoice};
use pollen::state::Kind;
use pollen::version::RemoteTag;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

create_exception!(pollen, PollenError, PyException, "A pollen command failed.");

/// How often a running command checks for a pending `KeyboardInterrupt`.
const SIGNAL_POLL: Duration = Duration::from_millis(100);

/// Build an instance of the `pollen._types` dataclass `$class`.
macro_rules! record {
    ($py:expr, $class:literal { $($field:ident: $value:expr),* $(,)? }) => {{
        let kwargs = PyDict::new($py);
        $(kwargs.set_item(stringify!($field), $value)?;)*
        $py.import("pollen._types")?
            .getattr($class)?
            .call((), Some(&kwargs))
    }};
}

fn options(
    config: Option<PathBuf>,
    global: bool,
    targets: Option<Vec<PathBuf>>,
    clients: Option<Vec<String>>,
    cache_dir: Option<PathBuf>,
    offline: bool,
    credentials_file: Option<PathBuf>,
) -> PyResult<Options> {
    if global && config.is_some() {
        return Err(PyValueError::new_err(
            "`config` and `global_` are mutually exclusive",
        ));
    }
    if targets.is_some() && clients.is_some() {
        return Err(PyValueError::new_err(
            "`targets` and `clients` are mutually exclusive",
        ));
    }
    let clients = clients
        .unwrap_or_default()
        .iter()
        .map(|client| {
            client
                .parse::<ClientChoice>()
                .map_err(PyValueError::new_err)
        })
        .collect::<PyResult<_>>()?;
    let defaults = Options::default();
    Ok(Options {
        config: config.unwrap_or(defaults.config),
        global,
        targets: targets.unwrap_or_default(),
        clients,
        cache_dir,
        offline,
        credentials_file,
    })
}

/// A `PollenError` carrying the whole error chain.
// `map_err` hands the error over by value.
#[allow(clippy::needless_pass_by_value)]
fn pollen_error(error: anyhow::Error) -> PyErr {
    PollenError::new_err(format!("{error:#}"))
}

/// Run `future` to completion with the GIL released, as the CLI would.
///
/// The future is polled on the calling thread, the only one where `CPython` runs
/// signal handlers, so Ctrl-C raises `KeyboardInterrupt` there. Dropping the
/// future then leaves the target directories as they were found, as the CLI
/// does on `SIGINT`.
fn block_on<T: Send>(
    py: Python<'_>,
    future: impl Future<Output = anyhow::Result<T>> + Send,
) -> PyResult<T> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    py.detach(|| {
        runtime.block_on(async {
            tokio::select! {
                result = future => result.map_err(pollen_error),
                interrupt = interrupted() => Err(interrupt),
            }
        })
    })
}

// The method path clippy suggests does not satisfy `attach`'s higher-ranked
// lifetime bound.
#[allow(clippy::redundant_closure_for_method_calls)]
async fn interrupted() -> PyErr {
    loop {
        tokio::time::sleep(SIGNAL_POLL).await;
        if let Err(error) = Python::attach(|py| py.check_signals()) {
            return error;
        }
    }
}

/// Install, refresh and prune the skills the config declares.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, targets=None, clients=None, cache_dir=None, offline=false, credentials_file=None, dry_run=false, force=false, allow_hidden_characters=false))]
// One parameter per Python keyword argument, mirroring the CLI's flags.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn update(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    clients: Option<Vec<String>>,
    cache_dir: Option<PathBuf>,
    offline: bool,
    credentials_file: Option<PathBuf>,
    dry_run: bool,
    force: bool,
    allow_hidden_characters: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let options = options(
        config,
        global_,
        targets,
        clients,
        cache_dir,
        offline,
        credentials_file,
    )?;
    let report = block_on(
        py,
        command::update::report(&options, dry_run, force, allow_hidden_characters),
    )?;

    let summaries = report
        .summaries
        .iter()
        .map(|summary| {
            let changes = summary
                .changes
                .iter()
                .map(|change| {
                    record!(py, "Change" {
                        action: action(py, change.action)?,
                        name: &change.name,
                        origin: &change.origin,
                        details: details(py, change.details.as_ref())?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()?;
            record!(py, "Summary" {
                target: &summary.target,
                kind: kind(py, summary.kind)?,
                changes: changes,
                dry_run: summary.dry_run,
            })
        })
        .collect::<PyResult<Vec<_>>>()?;
    record!(py, "Report" { summaries: summaries })
}

fn kind(py: Python<'_>, kind: Kind) -> PyResult<Bound<'_, PyAny>> {
    let value = match kind {
        Kind::Skills => "skills",
        Kind::Agents => "agents",
    };
    py.import("pollen._types")?.getattr("Kind")?.call1((value,))
}

fn details<'py>(py: Python<'py>, details: Option<&Details>) -> PyResult<Option<Bound<'py, PyAny>>> {
    details
        .map(|details| {
            record!(py, "AgentDetails" {
                tools: &details.tools,
                model: &details.model,
                permission: &details.permission,
                hooks: details.hooks,
                mcp_servers: details.mcp_servers,
            })
        })
        .transpose()
}

fn action(py: Python<'_>, action: command::update::Action) -> PyResult<Bound<'_, PyAny>> {
    let value = match action {
        command::update::Action::Added => "added",
        command::update::Action::Updated => "updated",
        command::update::Action::Unchanged => "unchanged",
        command::update::Action::Removed => "removed",
    };
    py.import("pollen._types")?
        .getattr("Action")?
        .call1((value,))
}

/// The skills pollen manages in each target directory.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, targets=None, clients=None, cache_dir=None, offline=false))]
fn list_installed(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    clients: Option<Vec<String>>,
    cache_dir: Option<PathBuf>,
    offline: bool,
) -> PyResult<Vec<Bound<'_, PyAny>>> {
    let options = options(config, global_, targets, clients, cache_dir, offline, None)?;
    let deployed = py
        .detach(|| command::list::installed(&options))
        .map_err(pollen_error)?;

    deployed
        .iter()
        .map(|deployed| {
            let mut skills = Vec::new();
            let mut agents = Vec::new();
            for (name, entry) in &deployed.entries {
                match deployed.kind {
                    Kind::Skills => skills.push(record!(py, "InstalledSkill" {
                        name: name,
                        repo: &entry.repo,
                        revision: &entry.revision,
                        commit: &entry.commit,
                        digest: &entry.digest,
                        source_path: &entry.source_path,
                        checksum: &entry.checksum,
                    })?),
                    Kind::Agents => agents.push(record!(py, "InstalledAgent" {
                        name: name,
                        repo: &entry.repo,
                        revision: &entry.revision,
                        commit: &entry.commit,
                        source_path: &entry.source_path,
                        checksum: &entry.checksum,
                    })?),
                }
            }
            record!(py, "Deployed" {
                target: &deployed.target,
                kind: kind(py, deployed.kind)?,
                skills: skills,
                agents: agents,
            })
        })
        .collect()
}

/// Check the config and, unless `config_only`, every skill and agent it
/// selects.
///
/// Raises `PollenError` when the config is invalid or a selected skill or
/// agent is.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, targets=None, clients=None, cache_dir=None, offline=false, credentials_file=None, config_only=false, pinned=false, allow_hidden_characters=false))]
// One parameter per Python keyword argument, mirroring the CLI's flags.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn validate(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    clients: Option<Vec<String>>,
    cache_dir: Option<PathBuf>,
    offline: bool,
    credentials_file: Option<PathBuf>,
    config_only: bool,
    pinned: bool,
    allow_hidden_characters: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let options = options(
        config,
        global_,
        targets,
        clients,
        cache_dir,
        offline,
        credentials_file,
    )?;
    let path = options.config_path();
    let validation = block_on(
        py,
        command::validate::check(
            &options,
            &path,
            config_only,
            pinned,
            allow_hidden_characters,
        ),
    )?;

    if !validation.problems.is_empty() {
        let messages: Vec<&str> = validation
            .problems
            .iter()
            .map(|problem| problem.message.as_str())
            .collect();
        return Err(PollenError::new_err(format!(
            "{}: {}:\n{}",
            validation.path.display(),
            command::plan::describe(&validation.problems),
            messages.join("\n"),
        )));
    }
    let skills = validation
        .skills
        .map(|skills| {
            skills
                .iter()
                .map(|skill| {
                    record!(py, "PlannedSkill" {
                        name: &skill.name,
                        repo: &skill.repo,
                        revision: &skill.revision,
                        commit: &skill.commit,
                        digest: &skill.digest,
                        source_path: &skill.source_path,
                    })
                })
                .collect::<PyResult<Vec<_>>>()
        })
        .transpose()?;
    let agents = validation
        .agents
        .map(|agents| {
            agents
                .iter()
                .map(|agent| {
                    record!(py, "PlannedAgent" {
                        name: &agent.name,
                        repo: &agent.repo,
                        revision: &agent.revision,
                        commit: &agent.commit,
                        source_path: &agent.source_path,
                        details: details(py, agent.details.as_ref())?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()
        })
        .transpose()?;
    record!(py, "Validation" {
        path: &validation.path,
        sources: validation.sources,
        skills: skills,
        agents: agents,
    })
}

/// Where each git source stands against its remote's releases, leaving out
/// those younger than `min_release_age`, and each skill listed under a
/// `well-known` source against its index.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, targets=None, clients=None, cache_dir=None, credentials_file=None, min_release_age=None))]
// One parameter per Python keyword argument, mirroring the CLI's flags.
#[allow(clippy::too_many_arguments)]
fn outdated(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    clients: Option<Vec<String>>,
    cache_dir: Option<PathBuf>,
    credentials_file: Option<PathBuf>,
    min_release_age: Option<Duration>,
) -> PyResult<Vec<Bound<'_, PyAny>>> {
    let options = options(
        config,
        global_,
        targets,
        clients,
        cache_dir,
        false,
        credentials_file,
    )?;
    let statuses = block_on(py, command::outdated::statuses(&options, min_release_age))?;

    statuses
        .iter()
        .map(|entry| match entry {
            Entry::Release(status) => record!(py, "SourceStatus" {
                repo: &status.repo,
                revision: &status.revision,
                current: release(py, status.current.as_ref())?,
                latest: release(py, status.latest.as_ref())?,
                pending: pending(py, status.pending.as_ref())?,
                is_outdated: status.is_outdated(),
            }),
            Entry::Digest(status) => record!(py, "DigestStatus" {
                origin: &status.origin,
                skill: &status.skill,
                pinned: &status.pinned,
                published: &status.published,
                held_back: status.held_back,
                is_outdated: status.is_outdated(),
            }),
        })
        .collect()
}

fn release<'py>(py: Python<'py>, tag: Option<&RemoteTag>) -> PyResult<Option<Bound<'py, PyAny>>> {
    tag.map(|tag| record!(py, "Release" { name: &tag.name, commit: &tag.commit }))
        .transpose()
}

fn pending<'py>(py: Python<'py>, pending: Option<&Pending>) -> PyResult<Option<Bound<'py, PyAny>>> {
    pending
        .map(|pending| {
            record!(py, "PendingRelease" {
                name: &pending.release.name,
                commit: &pending.release.commit,
                age: pending.age,
            })
        })
        .transpose()
}

/// Move every git source to its newest release at least `min_release_age`
/// old, rewriting the config file unless `dry_run`.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, targets=None, clients=None, cache_dir=None, credentials_file=None, freeze=false, dry_run=false, min_release_age=None))]
// One parameter per Python keyword argument, mirroring the CLI's flags.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn autoupdate(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    clients: Option<Vec<String>>,
    cache_dir: Option<PathBuf>,
    credentials_file: Option<PathBuf>,
    freeze: bool,
    dry_run: bool,
    min_release_age: Option<Duration>,
) -> PyResult<Bound<'_, PyAny>> {
    let options = options(
        config,
        global_,
        targets,
        clients,
        cache_dir,
        false,
        credentials_file,
    )?;
    let outcome = block_on(
        py,
        command::autoupdate::apply(&options, freeze, dry_run, min_release_age),
    )?;

    let bumps = outcome
        .bumps
        .iter()
        .map(|bump| {
            record!(py, "Bump" {
                repo: &bump.repo,
                from_revision: &bump.from,
                to_revision: &bump.to,
                release: &bump.release,
            })
        })
        .collect::<PyResult<Vec<_>>>()?;
    let digests = outcome
        .digests
        .iter()
        .map(|bump| digest_bump(py, bump))
        .collect::<PyResult<Vec<_>>>()?;
    record!(py, "AutoupdateOutcome" {
        path: &outcome.path,
        bumps: bumps,
        digests: digests,
        written: outcome.written,
    })
}

fn digest_bump<'py>(py: Python<'py>, bump: &DigestBump) -> PyResult<Bound<'py, PyAny>> {
    record!(py, "DigestBump" {
        origin: &bump.origin,
        skill: &bump.skill,
        from_digest: &bump.from,
        to_digest: &bump.to,
    })
}

/// Rewrite the config file in the newest format version, unless `dry_run`.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, dry_run=false))]
fn migrate(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    dry_run: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let options = options(config, global_, None, None, None, false, None)?;
    let migration = py
        .detach(|| command::migrate::apply(&options, dry_run))
        .map_err(pollen_error)?;
    record!(py, "Migration" {
        path: &migration.path,
        from_version: migration.from_version,
        to_version: migration.to_version,
        written: migration.written,
    })
}

/// List the skills a git source provides, at its newest release unless
/// `revision` names another, under `path` when given, and the agent files
/// of each `(path, client)` of `agents`, checked against that client.
#[pyfunction]
#[pyo3(signature = (repo, *, revision=None, path=None, agents=None, cache_dir=None, offline=false))]
// pyo3 extracts a path only as an owned `PathBuf`.
#[allow(clippy::needless_pass_by_value)]
fn show<'py>(
    py: Python<'py>,
    repo: &str,
    revision: Option<&str>,
    path: Option<PathBuf>,
    agents: Option<Vec<(PathBuf, String)>>,
    cache_dir: Option<PathBuf>,
    offline: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let options = options(None, false, None, None, cache_dir, offline, None)?;
    let cache_dir = options.cache_dir();
    let agents = agents
        .unwrap_or_default()
        .into_iter()
        .map(|(path, client)| {
            let client = Client::ALL
                .into_iter()
                .find(|known| known.name() == client)
                .ok_or_else(|| {
                    PyValueError::new_err(format!(
                        "unknown client `{client}`: expected `claude-code` or `opencode`"
                    ))
                })?;
            Ok(AgentDirectory { path, client })
        })
        .collect::<PyResult<Vec<_>>>()?;
    let listing = block_on(
        py,
        command::show::inspect(
            repo,
            revision,
            path.as_deref(),
            &agents,
            &cache_dir,
            offline,
        ),
    )?;

    let skills = listing
        .skills
        .iter()
        .map(|entry| {
            record!(py, "ListedSkill" {
                path: &entry.path,
                name: &entry.name,
                description: &entry.description,
                error: &entry.error,
            })
        })
        .collect::<PyResult<Vec<_>>>()?;
    let agents = listing
        .agents
        .iter()
        .map(|entry| {
            record!(py, "ListedAgent" {
                path: &entry.path,
                client: entry.client.name(),
                name: &entry.name,
                description: &entry.description,
                details: details(py, entry.details.as_ref())?,
                error: &entry.error,
            })
        })
        .collect::<PyResult<Vec<_>>>()?;
    record!(py, "Listing" {
        repo: &listing.repo,
        revision: &listing.revision,
        release: &listing.release,
        commit: &listing.commit,
        skills: skills,
        agents: agents,
    })
}

/// The JSON Schema for `pollen.yaml`, as a JSON document.
#[pyfunction]
fn schema() -> PyResult<String> {
    pollen::schema::rendered().map_err(pollen_error)
}

/// Run the `pollen` command line on `sys.argv` and return its exit status:
/// what the `pollen` script and `python -m pollen` call.
#[pyfunction]
fn main(py: Python<'_>) -> PyResult<u8> {
    let sys_argv: Vec<OsString> = py.import("sys")?.getattr("argv")?.extract()?;
    // `argv[0]` is the script's path, or `__main__.py` under `-m`: clap would
    // show it in the usage line.
    let args: Vec<OsString> = std::iter::once("pollen".into())
        .chain(sys_argv.into_iter().skip(1))
        .collect();
    let status = py.detach(|| pollen::cli::main(args));
    // Ctrl-C also tripped Python's handler; the CLI already reported it, so
    // the pending `KeyboardInterrupt` is dropped rather than raised.
    let _ = py.check_signals();
    Ok(status)
}

/// The extension module behind the `pollen` package.
#[pymodule]
fn _pollen(module: &Bound<'_, PyModule>) -> PyResult<()> {
    // Levels are read from Python on every record: the default cache would
    // freeze them at import, before the application configures `logging`.
    pyo3_log::Logger::new(module.py(), pyo3_log::Caching::Loggers)?
        .install()
        .map_err(|error| PyException::new_err(error.to_string()))?;
    module.add("PollenError", module.py().get_type::<PollenError>())?;
    module.add_function(wrap_pyfunction!(update, module)?)?;
    module.add_function(wrap_pyfunction!(list_installed, module)?)?;
    module.add_function(wrap_pyfunction!(validate, module)?)?;
    module.add_function(wrap_pyfunction!(outdated, module)?)?;
    module.add_function(wrap_pyfunction!(autoupdate, module)?)?;
    module.add_function(wrap_pyfunction!(migrate, module)?)?;
    module.add_function(wrap_pyfunction!(show, module)?)?;
    module.add_function(wrap_pyfunction!(schema, module)?)?;
    module.add_function(wrap_pyfunction!(main, module)?)?;
    Ok(())
}
