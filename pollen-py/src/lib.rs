//! The `pollen._pollen` extension module: each function runs one pollen
//! command and hands back the dataclasses `pollen._types` defines.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use pollen::cli::Options;
use pollen::command;
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
    cache_dir: Option<PathBuf>,
    offline: bool,
) -> PyResult<Options> {
    if global && config.is_some() {
        return Err(PyValueError::new_err(
            "`config` and `global_` are mutually exclusive",
        ));
    }
    let defaults = Options::default();
    Ok(Options {
        config: config.unwrap_or(defaults.config),
        global,
        targets: targets.unwrap_or_default(),
        cache_dir,
        offline,
    })
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
                result = future => result.map_err(|error| PollenError::new_err(format!("{error:#}"))),
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
#[pyo3(signature = (*, config=None, global_=false, targets=None, cache_dir=None, offline=false, dry_run=false, force=false))]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn update(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    cache_dir: Option<PathBuf>,
    offline: bool,
    dry_run: bool,
    force: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let options = options(config, global_, targets, cache_dir, offline)?;
    let report = block_on(py, command::update::report(&options, dry_run, force))?;

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
                    })
                })
                .collect::<PyResult<Vec<_>>>()?;
            record!(py, "Summary" {
                target: &summary.target,
                changes: changes,
                dry_run: summary.dry_run,
            })
        })
        .collect::<PyResult<Vec<_>>>()?;
    record!(py, "Report" { summaries: summaries })
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
#[pyo3(signature = (*, config=None, global_=false, targets=None, cache_dir=None, offline=false))]
fn list_installed(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    cache_dir: Option<PathBuf>,
    offline: bool,
) -> PyResult<Vec<Bound<'_, PyAny>>> {
    let options = options(config, global_, targets, cache_dir, offline)?;
    let deployed = py
        .detach(|| command::list::installed(&options))
        .map_err(|error| PollenError::new_err(format!("{error:#}")))?;

    deployed
        .iter()
        .map(|deployed| {
            let skills = deployed
                .skills
                .iter()
                .map(|(name, skill)| {
                    record!(py, "InstalledSkill" {
                        name: name,
                        repo: &skill.repo,
                        revision: &skill.revision,
                        commit: &skill.commit,
                        source_path: &skill.source_path,
                        checksum: &skill.checksum,
                    })
                })
                .collect::<PyResult<Vec<_>>>()?;
            record!(py, "Deployed" {
                target: &deployed.target,
                skills: skills,
            })
        })
        .collect()
}

/// Check the config and, unless `config_only`, every skill it selects.
///
/// Raises `PollenError` when the config is invalid or a selected skill breaks
/// the Agent Skills specification.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, targets=None, cache_dir=None, offline=false, config_only=false, pinned=false))]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn validate(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    cache_dir: Option<PathBuf>,
    offline: bool,
    config_only: bool,
    pinned: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let options = options(config, global_, targets, cache_dir, offline)?;
    let path = options.config_path();
    let validation = block_on(
        py,
        command::validate::check(&options, &path, config_only, pinned),
    )?;

    if !validation.problems.is_empty() {
        return Err(PollenError::new_err(format!(
            "{}: {} skill(s) do not satisfy the Agent Skills specification:\n{}",
            validation.path.display(),
            validation.problems.len(),
            validation.problems.join("\n"),
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
                        source_path: &skill.source_path,
                    })
                })
                .collect::<PyResult<Vec<_>>>()
        })
        .transpose()?;
    record!(py, "Validation" {
        path: &validation.path,
        sources: validation.sources,
        skills: skills,
    })
}

/// Where each git source stands against its remote's releases.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, targets=None, cache_dir=None))]
fn outdated(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    cache_dir: Option<PathBuf>,
) -> PyResult<Vec<Bound<'_, PyAny>>> {
    let options = options(config, global_, targets, cache_dir, false)?;
    let statuses = block_on(py, command::outdated::statuses(&options))?;

    statuses
        .iter()
        .map(|status| {
            record!(py, "SourceStatus" {
                repo: &status.repo,
                revision: &status.revision,
                current: release(py, status.current.as_ref())?,
                latest: release(py, status.latest.as_ref())?,
                is_outdated: status.is_outdated(),
            })
        })
        .collect()
}

fn release<'py>(py: Python<'py>, tag: Option<&RemoteTag>) -> PyResult<Option<Bound<'py, PyAny>>> {
    tag.map(|tag| record!(py, "Release" { name: &tag.name, commit: &tag.commit }))
        .transpose()
}

/// Move every git source to its newest release, rewriting the config file
/// unless `dry_run`.
#[pyfunction]
#[pyo3(signature = (*, config=None, global_=false, targets=None, cache_dir=None, freeze=false, dry_run=false))]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn autoupdate(
    py: Python<'_>,
    config: Option<PathBuf>,
    global_: bool,
    targets: Option<Vec<PathBuf>>,
    cache_dir: Option<PathBuf>,
    freeze: bool,
    dry_run: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let options = options(config, global_, targets, cache_dir, false)?;
    let outcome = block_on(py, command::autoupdate::apply(&options, freeze, dry_run))?;

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
    record!(py, "AutoupdateOutcome" {
        path: &outcome.path,
        bumps: bumps,
        written: outcome.written,
    })
}

/// List the skills a git source provides, at its newest release unless
/// `revision` names another, under `path` when given.
#[pyfunction]
#[pyo3(signature = (repo, *, revision=None, path=None, cache_dir=None, offline=false))]
// pyo3 extracts a path only as an owned `PathBuf`.
#[allow(clippy::needless_pass_by_value)]
fn show<'py>(
    py: Python<'py>,
    repo: &str,
    revision: Option<&str>,
    path: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    offline: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let options = options(None, false, None, cache_dir, offline)?;
    let cache_dir = options.cache_dir();
    let listing = block_on(
        py,
        command::show::inspect(repo, revision, path.as_deref(), &cache_dir, offline),
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
    record!(py, "Listing" {
        repo: &listing.repo,
        revision: &listing.revision,
        release: &listing.release,
        commit: &listing.commit,
        skills: skills,
    })
}

/// The JSON Schema for `pollen.yaml`, as a JSON document.
#[pyfunction]
fn schema() -> PyResult<String> {
    pollen::schema::rendered().map_err(|error| PollenError::new_err(format!("{error:#}")))
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
    module.add_function(wrap_pyfunction!(show, module)?)?;
    module.add_function(wrap_pyfunction!(schema, module)?)?;
    Ok(())
}
