import subprocess
from pathlib import Path

import pytest


def write_skill(directory: Path, name: str, description: str | None = None) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    description = description or f"The {name} skill, for tests."
    (directory / "SKILL.md").write_text(
        f"---\nname: {name}\ndescription: {description}\n---\n\nBody.\n"
    )


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, capture_output=True, text=True
    ).stdout.strip()


def commit_and_tag(root: Path, tag: str) -> str:
    if not (root / ".git").exists():
        git(root, "init", "--quiet", "--initial-branch", "main")
        git(root, "config", "user.email", "test@example.invalid")
        git(root, "config", "user.name", "pollen tests")
        git(root, "config", "commit.gpgsign", "false")
        git(root, "config", "tag.gpgsign", "false")
    git(root, "add", ".")
    git(root, "commit", "--quiet", "--allow-empty", "--message", tag)
    git(root, "tag", tag)
    return git(root, "rev-parse", "HEAD")


@pytest.fixture
def origin(tmp_path: Path) -> Path:
    """A git repository with two skills, tagged `v1.0.0`."""
    root = tmp_path / "origin"
    write_skill(root / "skills" / "alpha", "alpha")
    write_skill(root / "skills" / "beta", "beta")
    commit_and_tag(root, "v1.0.0")
    return root


@pytest.fixture
def cache_dir(tmp_path: Path) -> Path:
    return tmp_path / "cache"


@pytest.fixture
def workspace(tmp_path: Path, origin: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """The working directory, holding a `pollen.yaml` that pins `origin` at `v1.0.0`."""
    root = tmp_path / "workspace"
    root.mkdir()
    write_skill(root / "local-skills" / "gamma", "gamma")
    (root / "pollen.yaml").write_text(
        "targets:\n"
        "  - deployed\n"
        "repos:\n"
        f"  - repo: {origin.as_posix()}\n"
        "    revision: v1.0.0\n"
        "    paths:\n"
        "      - path: skills\n"
        "  - repo: local\n"
        "    paths:\n"
        "      - path: local-skills\n"
    )
    monkeypatch.chdir(root)
    return root
