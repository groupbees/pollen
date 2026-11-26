import json
import subprocess
import sys
from importlib.metadata import entry_points, version
from pathlib import Path


def run_cli(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "pollen", *args], capture_output=True, text=True, check=False
    )


def test_the_pollen_script_runs_the_cli() -> None:
    [script] = entry_points(group="console_scripts", name="pollen")

    assert script.value == "pollen._pollen:main"


def test_version_names_the_program_pollen() -> None:
    result = run_cli("--version")

    assert result.returncode == 0
    assert result.stdout.split() == ["pollen", version("pollen-skills")]


def test_update_deploys_from_the_command_line(workspace: Path, cache_dir: Path) -> None:
    result = run_cli("update", "--cache-dir", str(cache_dir))

    assert result.returncode == 0, result.stderr
    assert (workspace / "deployed" / "alpha" / "SKILL.md").is_file()


def test_a_failure_exits_non_zero_with_the_error(tmp_path: Path) -> None:
    result = run_cli("list", "--config", str(tmp_path / "missing.yaml"))

    assert result.returncode == 1
    assert result.stderr.startswith("Error: ")


def test_a_usage_error_exits_with_clap_status() -> None:
    result = run_cli("no-such-command")

    assert result.returncode == 2
    assert "Usage: pollen" in result.stderr


def test_a_log_filter_that_does_not_parse_exits_with_clap_status() -> None:
    result = run_cli("schema", "--log-filter", "pollen=loud")

    assert result.returncode == 2
    assert "invalid log filter `pollen=loud`" in result.stderr


def test_logs_can_be_json(workspace: Path, cache_dir: Path) -> None:
    result = run_cli(
        "update", "--cache-dir", str(cache_dir), "--log-filter", "info", "--log-format", "json"
    )

    assert result.returncode == 0, result.stderr
    lines = result.stderr.splitlines()
    assert lines
    assert all(json.loads(line)["level"] for line in lines)
