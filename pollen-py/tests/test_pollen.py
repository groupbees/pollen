import _thread
import dataclasses
import json
import logging
import os
import re
import sys
import threading
import time
from pathlib import Path

import pytest
from conftest import commit_and_tag, write_skill

import pollen


def test_update_deploys_every_selected_skill(workspace: Path, cache_dir: Path) -> None:
    report = pollen.update(cache_dir=cache_dir)

    [summary] = report.summaries
    assert summary.target == Path("deployed")
    assert [(change.action, change.name) for change in summary.changes] == [
        (pollen.Action.ADDED, "alpha"),
        (pollen.Action.ADDED, "beta"),
        (pollen.Action.ADDED, "gamma"),
    ]
    assert not summary.dry_run
    assert (workspace / "deployed" / "alpha" / "SKILL.md").is_file()


def test_a_second_update_offline_leaves_everything_unchanged(
    workspace: Path, cache_dir: Path
) -> None:
    pollen.update(cache_dir=cache_dir)

    report = pollen.update(cache_dir=cache_dir, offline=True)

    assert report.count(pollen.Action.UNCHANGED) == 3


def test_a_dry_run_writes_nothing(workspace: Path, cache_dir: Path) -> None:
    report = pollen.update(cache_dir=cache_dir, dry_run=True)

    assert report.summaries[0].dry_run
    assert report.count(pollen.Action.ADDED) == 3
    assert not (workspace / "deployed").exists()


def test_targets_override_the_config(workspace: Path, tmp_path: Path, cache_dir: Path) -> None:
    elsewhere = tmp_path / "elsewhere"

    report = pollen.update(cache_dir=cache_dir, targets=[elsewhere])

    assert [summary.target for summary in report.summaries] == [elsewhere]
    assert (elsewhere / "gamma" / "SKILL.md").is_file()


def test_list_installed_reads_what_update_recorded(workspace: Path, cache_dir: Path) -> None:
    pollen.update(cache_dir=cache_dir)

    [deployed] = pollen.list_installed()

    assert deployed.target == Path("deployed")
    by_name = {skill.name: skill for skill in deployed.skills}
    assert sorted(by_name) == ["alpha", "beta", "gamma"]
    assert by_name["alpha"].revision == "v1.0.0"
    assert by_name["alpha"].commit is not None
    assert len(by_name["alpha"].commit) == 40
    assert by_name["gamma"].repo == "local"


def test_validate_lists_the_selected_skills(workspace: Path, cache_dir: Path) -> None:
    validation = pollen.validate(cache_dir=cache_dir)

    assert validation.path == Path("pollen.yaml")
    assert validation.sources == 2
    assert validation.skills is not None
    assert [skill.name for skill in validation.skills] == ["alpha", "beta", "gamma"]


def test_validate_config_only_fetches_nothing(workspace: Path, cache_dir: Path) -> None:
    validation = pollen.validate(cache_dir=cache_dir, config_only=True)

    assert validation.skills is None
    assert not cache_dir.exists()


def test_an_invalid_skill_raises_with_the_problem(workspace: Path, cache_dir: Path) -> None:
    write_skill(workspace / "local-skills" / "Bad-Name", "Bad-Name")

    with pytest.raises(pollen.PollenError, match="Bad-Name"):
        pollen.validate(cache_dir=cache_dir)


def test_validate_pinned_refuses_a_tag(workspace: Path) -> None:
    with pytest.raises(pollen.PollenError, match="not pinned to a commit"):
        pollen.validate(config_only=True, pinned=True)


def test_a_missing_config_raises(tmp_path: Path) -> None:
    with pytest.raises(pollen.PollenError, match=re.escape("missing.yaml")):
        pollen.validate(config=tmp_path / "missing.yaml", config_only=True)


def test_config_and_global_are_mutually_exclusive() -> None:
    with pytest.raises(ValueError, match="mutually exclusive"):
        pollen.list_installed(config="pollen.yaml", global_=True)


def test_outdated_reports_a_newer_release(workspace: Path, origin: Path, cache_dir: Path) -> None:
    commit = commit_and_tag(origin, "v1.1.0")

    [status] = pollen.outdated(cache_dir=cache_dir)

    assert status.revision == "v1.0.0"
    assert status.current is not None
    assert status.current.name == "v1.0.0"
    assert status.latest == pollen.Release(name="v1.1.0", commit=commit)
    assert status.is_outdated


def test_autoupdate_rewrites_the_revision(workspace: Path, origin: Path, cache_dir: Path) -> None:
    commit = commit_and_tag(origin, "v1.1.0")

    outcome = pollen.autoupdate(cache_dir=cache_dir, freeze=True)

    [bump] = outcome.bumps
    assert (bump.from_revision, bump.to_revision, bump.release) == ("v1.0.0", commit, "v1.1.0")
    assert outcome.written
    assert f"revision: {commit}  # v1.1.0" in (workspace / "pollen.yaml").read_text()


def test_autoupdate_dry_run_leaves_the_file_alone(
    workspace: Path, origin: Path, cache_dir: Path
) -> None:
    commit_and_tag(origin, "v1.1.0")
    before = (workspace / "pollen.yaml").read_text()

    outcome = pollen.autoupdate(cache_dir=cache_dir, dry_run=True)

    assert [bump.to_revision for bump in outcome.bumps] == ["v1.1.0"]
    assert not outcome.written
    assert (workspace / "pollen.yaml").read_text() == before


def test_show_lists_the_newest_release(origin: Path, cache_dir: Path) -> None:
    write_skill(origin / "skills" / "Broken", "Broken")
    commit = commit_and_tag(origin, "v2.0.0")

    listing = pollen.show(origin.as_posix(), cache_dir=cache_dir)

    assert (listing.revision, listing.release, listing.commit) == ("v2.0.0", "v2.0.0", commit)
    by_path = {skill.path: skill for skill in listing.skills}
    assert by_path["skills/alpha"].description == "The alpha skill, for tests."
    assert by_path["skills/Broken"].error is not None


def test_show_restricts_to_a_path_at_a_revision(origin: Path, cache_dir: Path) -> None:
    listing = pollen.show(
        origin.as_posix(), revision="v1.0.0", path="skills/beta", cache_dir=cache_dir
    )

    assert [skill.name for skill in listing.skills] == ["beta"]


def test_schema_is_the_json_schema_of_the_config() -> None:
    document = json.loads(pollen.schema())

    assert document["$id"].endswith("/schema/pollen.schema.json")
    assert "repos" in document["properties"]


def test_logs_reach_python_logging(
    workspace: Path, cache_dir: Path, caplog: pytest.LogCaptureFixture
) -> None:
    with caplog.at_level(logging.DEBUG, logger="pollen"):
        pollen.update(cache_dir=cache_dir)

    assert any(record.name.startswith("pollen.") for record in caplog.records)


def test_results_are_immutable(workspace: Path, cache_dir: Path) -> None:
    report = pollen.update(cache_dir=cache_dir)

    with pytest.raises(dataclasses.FrozenInstanceError):
        report.summaries[0].changes[0].name = "renamed"  # pyright: ignore[reportAttributeAccessIssue]  # the assignment is the point


@pytest.mark.skipif(sys.platform == "win32", reason="the stand-in git is a shell script")
def test_ctrl_c_interrupts_a_running_command(
    tmp_path: Path, cache_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    hanging = tmp_path / "bin"
    hanging.mkdir()
    (hanging / "git").write_text("#!/bin/sh\nexec sleep 30\n")
    (hanging / "git").chmod(0o755)
    monkeypatch.setenv("PATH", f"{hanging}{os.pathsep}{os.environ['PATH']}")
    threading.Timer(0.5, _thread.interrupt_main).start()
    started = time.monotonic()

    with pytest.raises(KeyboardInterrupt):
        pollen.show("https://example.invalid/skills.git", cache_dir=cache_dir)

    assert time.monotonic() - started < 5
