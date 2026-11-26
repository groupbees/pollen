from dataclasses import dataclass, field
from datetime import timedelta
from enum import StrEnum
from pathlib import Path


class Action(StrEnum):
    """What an update did to one skill."""

    ADDED = "added"
    UPDATED = "updated"
    UNCHANGED = "unchanged"
    REMOVED = "removed"


class Kind(StrEnum):
    """What a directory pollen deploys into holds."""

    SKILLS = "skills"
    AGENTS = "agents"


@dataclass(frozen=True, slots=True, kw_only=True)
class AgentDetails:
    """What an agent's frontmatter grants, each value on one line as written."""

    tools: str | None
    model: str | None
    permission: str | None
    """Claude Code's `permissionMode`, else OpenCode's `permission` rules."""
    hooks: bool
    """Whether it declares `hooks`, which run shell commands."""
    mcp_servers: bool
    """Whether it declares `mcpServers`."""


@dataclass(frozen=True, slots=True, kw_only=True)
class Change:
    """What happened to one skill or agent in one target directory."""

    action: Action
    name: str
    origin: str
    """Where the skill or agent comes from, empty for a removal."""
    details: AgentDetails | None = None
    """What an agent's frontmatter grants; `None` for a skill and for a removal."""


@dataclass(frozen=True, slots=True, kw_only=True)
class Summary:
    """The result of an update in one target directory."""

    target: Path
    kind: Kind
    changes: list[Change]
    """One entry per skill or agent, ordered by name within each action."""
    dry_run: bool


@dataclass(frozen=True, slots=True, kw_only=True)
class Report:
    """The result of an update, one summary per directory it visited."""

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
    digest: str | None
    """The digest of the artifact a `well-known` source served."""
    source_path: str
    checksum: str


@dataclass(frozen=True, slots=True, kw_only=True)
class InstalledAgent:
    """One agent pollen deployed, as its directory records it."""

    name: str
    repo: str
    revision: str | None
    commit: str | None
    source_path: str
    checksum: str


@dataclass(frozen=True, slots=True, kw_only=True)
class Deployed:
    """The skills or agents pollen manages in one directory."""

    target: Path
    kind: Kind
    skills: list[InstalledSkill]
    """The skills of a directory of skills, empty for one of agents."""
    agents: list[InstalledAgent] = field(default_factory=list[InstalledAgent])
    """The agents of a directory of agents, empty for one of skills."""


@dataclass(frozen=True, slots=True, kw_only=True)
class PlannedSkill:
    """One skill a config selects."""

    name: str
    repo: str
    revision: str | None
    commit: str | None
    digest: str | None
    """The digest of the artifact a `well-known` source serves."""
    source_path: str


@dataclass(frozen=True, slots=True, kw_only=True)
class PlannedAgent:
    """One agent a config selects."""

    name: str
    repo: str
    revision: str | None
    commit: str | None
    source_path: str
    details: AgentDetails


@dataclass(frozen=True, slots=True, kw_only=True)
class Validation:
    """What checking a config found."""

    path: Path
    sources: int
    skills: list[PlannedSkill] | None
    """The skills the sources yield, `None` when only the file was checked."""
    agents: list[PlannedAgent] | None = None
    """The agents the sources yield, `None` when only the file was checked."""


@dataclass(frozen=True, slots=True, kw_only=True)
class Release:
    """A release tag on a git remote."""

    name: str
    commit: str


@dataclass(frozen=True, slots=True, kw_only=True)
class PendingRelease:
    """A release left out for being younger than the minimum release age."""

    name: str
    commit: str
    age: timedelta | None
    """How old its tag is, `None` when git records no date for it."""


@dataclass(frozen=True, slots=True, kw_only=True)
class SourceStatus:
    """Where one git source stands against its remote's releases."""

    repo: str
    revision: str
    current: Release | None
    """The release the pinned revision stands for, when it is one."""
    latest: Release | None
    """The newest release; with `min_release_age`, the newest one that old."""
    pending: PendingRelease | None
    """The newest release `latest` leaves out for being too young."""
    is_outdated: bool


@dataclass(frozen=True, slots=True, kw_only=True)
class DigestStatus:
    """Where one skill listed under a `well-known` source stands against its index."""

    origin: str
    skill: str
    pinned: str | None
    """The `digest:` in the config, `None` when the skill follows the index."""
    published: str | None
    """The digest the index publishes, `None` when it no longer lists the skill."""
    held_back: bool
    """Whether a minimum release age holds the published digest back."""
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
class DigestBump:
    """One `digest:` of a `well-known` skill an autoupdate writes."""

    origin: str
    skill: str
    from_digest: str | None
    """The digest it holds now, `None` when it has none."""
    to_digest: str


@dataclass(frozen=True, slots=True, kw_only=True)
class Migration:
    """What a migration of the config file changed, or would change."""

    path: Path
    from_version: int
    """The format version the file was written in."""
    to_version: int
    """The format version it is in after the migration."""
    written: bool
    """Whether the file was rewritten: false on a dry run, or when already in the newest version."""


@dataclass(frozen=True, slots=True, kw_only=True)
class AutoupdateOutcome:
    """What an autoupdate changed, or would change."""

    path: Path
    bumps: list[Bump]
    digests: list[DigestBump]
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
class ListedAgent:
    """One agent file found in a source; `error` is set when it is invalid for `client`."""

    path: str
    client: str
    """The client whose rules it was checked against."""
    name: str | None
    description: str | None
    details: AgentDetails | None
    error: str | None


@dataclass(frozen=True, slots=True, kw_only=True)
class Listing:
    """What one git source provides at one revision."""

    repo: str
    revision: str
    release: str | None
    commit: str
    skills: list[ListedSkill]
    agents: list[ListedAgent] = field(default_factory=list[ListedAgent])
    """The agents of the directories asked for, in that order, then by path."""
