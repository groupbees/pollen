from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path


class Action(StrEnum):
    """What an update did to one skill."""

    ADDED = "added"
    UPDATED = "updated"
    UNCHANGED = "unchanged"
    REMOVED = "removed"


@dataclass(frozen=True, slots=True, kw_only=True)
class Change:
    """What happened to one skill in one target directory."""

    action: Action
    name: str
    origin: str
    """Where the skill comes from, empty for a removal."""


@dataclass(frozen=True, slots=True, kw_only=True)
class Summary:
    """The result of an update in one target directory."""

    target: Path
    changes: list[Change]
    """One entry per skill, ordered by name within each action."""
    dry_run: bool


@dataclass(frozen=True, slots=True, kw_only=True)
class Report:
    """The result of an update, one summary per target directory."""

    summaries: list[Summary]

    def count(self, action: Action) -> int:
        """How many skills, across every target, the update put in `action`."""
        return sum(
            change.action is action for summary in self.summaries for change in summary.changes
        )


@dataclass(frozen=True, slots=True, kw_only=True)
class InstalledSkill:
    """One skill pollen deployed, as its target directory records it."""

    name: str
    repo: str
    revision: str | None
    commit: str | None
    source_path: str
    checksum: str


@dataclass(frozen=True, slots=True, kw_only=True)
class Deployed:
    """The skills pollen manages in one target directory."""

    target: Path
    skills: list[InstalledSkill]


@dataclass(frozen=True, slots=True, kw_only=True)
class PlannedSkill:
    """One skill a config selects."""

    name: str
    repo: str
    revision: str | None
    commit: str | None
    source_path: str


@dataclass(frozen=True, slots=True, kw_only=True)
class Validation:
    """What checking a config found."""

    path: Path
    sources: int
    skills: list[PlannedSkill] | None
    """The skills the sources yield, `None` when only the file was checked."""


@dataclass(frozen=True, slots=True, kw_only=True)
class Release:
    """A release tag on a git remote."""

    name: str
    commit: str


@dataclass(frozen=True, slots=True, kw_only=True)
class SourceStatus:
    """Where one git source stands against its remote's releases."""

    repo: str
    revision: str
    current: Release | None
    """The release the pinned revision stands for, when it is one."""
    latest: Release | None
    is_outdated: bool


@dataclass(frozen=True, slots=True, kw_only=True)
class Bump:
    """One `revision:` an autoupdate moves."""

    repo: str
    from_revision: str
    to_revision: str
    release: str | None
    """The release `to_revision` stands for when it is a commit id."""


@dataclass(frozen=True, slots=True, kw_only=True)
class AutoupdateOutcome:
    """What an autoupdate changed, or would change."""

    path: Path
    bumps: list[Bump]
    written: bool
    """Whether the config file was rewritten."""


@dataclass(frozen=True, slots=True, kw_only=True)
class ListedSkill:
    """One skill directory found in a source; `error` is set when it is invalid."""

    path: str
    name: str | None
    description: str | None
    error: str | None


@dataclass(frozen=True, slots=True, kw_only=True)
class Listing:
    """What one git source provides at one revision."""

    repo: str
    revision: str
    release: str | None
    commit: str
    skills: list[ListedSkill]
