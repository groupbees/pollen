from os import PathLike
from typing import TypeAlias

from pollen._types import AutoupdateOutcome, Deployed, Listing, Report, SourceStatus, Validation

StrPath: TypeAlias = str | PathLike[str]

class PollenError(Exception):
    """A pollen command failed."""

def update(
    *,
    config: StrPath | None = None,
    global_: bool = False,
    targets: list[StrPath] | None = None,
    cache_dir: StrPath | None = None,
    offline: bool = False,
    dry_run: bool = False,
    force: bool = False,
) -> Report:
    """Install, refresh and prune the skills the config declares."""

def list_installed(
    *,
    config: StrPath | None = None,
    global_: bool = False,
    targets: list[StrPath] | None = None,
    cache_dir: StrPath | None = None,
    offline: bool = False,
) -> list[Deployed]:
    """The skills pollen manages in each target directory."""

def validate(
    *,
    config: StrPath | None = None,
    global_: bool = False,
    targets: list[StrPath] | None = None,
    cache_dir: StrPath | None = None,
    offline: bool = False,
    config_only: bool = False,
    pinned: bool = False,
) -> Validation:
    """Check the config and, unless `config_only`, every skill it selects."""

def outdated(
    *,
    config: StrPath | None = None,
    global_: bool = False,
    targets: list[StrPath] | None = None,
    cache_dir: StrPath | None = None,
) -> list[SourceStatus]:
    """Where each git source stands against its remote's releases."""

def autoupdate(
    *,
    config: StrPath | None = None,
    global_: bool = False,
    targets: list[StrPath] | None = None,
    cache_dir: StrPath | None = None,
    freeze: bool = False,
    dry_run: bool = False,
) -> AutoupdateOutcome:
    """Move every git source to its newest release."""

def show(
    repo: str,
    *,
    revision: str | None = None,
    path: StrPath | None = None,
    cache_dir: StrPath | None = None,
    offline: bool = False,
) -> Listing:
    """List the skills a git source provides."""

def schema() -> str:
    """The JSON Schema for `pollen.yaml`, as a JSON document."""
