from pathlib import Path

import pytest
from conftest import commit_and_tag

import pollen

REVIEWER = (
    "---\n"
    "name: reviewer\n"
    "description: Reviews code.\n"
    "tools: Read, Grep\n"
    "model: opus\n"
    "permissionMode: plan\n"
    "hooks:\n"
    "  Stop: []\n"
    "---\n"
    "You review code.\n"
)


@pytest.fixture
def agents_workspace(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """The working directory, holding a `pollen.yaml` sending one agent to every client."""
    root = tmp_path / "workspace"
    (root / "agents").mkdir(parents=True)
    (root / "agents" / "reviewer.md").write_text(REVIEWER)
    (root / "pollen.yaml").write_text(
        "version: 2\nrepos:\n  - repo: local\n    agents:\n      - path: agents\n"
    )
    monkeypatch.chdir(root)
    return root


def test_update_deploys_an_agent_into_each_clients_agent_directory(
    agents_workspace: Path, cache_dir: Path
) -> None:
    report = pollen.update(cache_dir=cache_dir)

    assert [(summary.target, summary.kind) for summary in report.summaries] == [
        (Path(".claude/agents"), pollen.Kind.AGENTS),
        (Path(".opencode/agents"), pollen.Kind.AGENTS),
    ]
    [change] = report.summaries[0].changes
    assert change.action is pollen.Action.ADDED
    assert change.details == pollen.AgentDetails(
        tools="Read, Grep", model="opus", permission="plan", hooks=True, mcp_servers=False
    )
    assert (agents_workspace / ".opencode" / "agents" / "reviewer.md").read_text() == REVIEWER


def test_list_installed_reads_the_agents_each_directory_records(
    agents_workspace: Path, cache_dir: Path
) -> None:
    pollen.update(cache_dir=cache_dir)

    deployed = pollen.list_installed()

    assert [(entry.target, entry.kind) for entry in deployed] == [
        (Path(".claude/agents"), pollen.Kind.AGENTS),
        (Path(".opencode/agents"), pollen.Kind.AGENTS),
    ]
    [agent] = deployed[0].agents
    assert (agent.name, agent.repo, agent.source_path) == (
        "reviewer",
        "local",
        "agents/reviewer.md",
    )
    assert deployed[0].skills == []


def test_validate_lists_the_selected_agents(agents_workspace: Path, cache_dir: Path) -> None:
    validation = pollen.validate(cache_dir=cache_dir)

    assert validation.skills == []
    assert validation.agents is not None
    [agent] = validation.agents
    assert (agent.name, agent.details.model) == ("reviewer", "opus")


def test_an_agent_breaking_a_clients_rule_raises_with_the_client(
    agents_workspace: Path, cache_dir: Path
) -> None:
    (agents_workspace / "agents" / "helper.md").write_text("---\ndescription: Helps.\n---\n")

    with pytest.raises(pollen.PollenError, match="1 agent\\(s\\) are invalid") as raised:
        pollen.validate(cache_dir=cache_dir)

    assert "for claude-code, the frontmatter needs `name`" in str(raised.value)
    pollen.validate(cache_dir=cache_dir, clients=["opencode"])


def test_show_lists_the_agents_of_a_directory_checked_against_a_client(
    origin: Path, cache_dir: Path
) -> None:
    (origin / "agents").mkdir()
    (origin / "agents" / "reviewer.md").write_text(REVIEWER)
    (origin / "agents" / "helper.md").write_text("---\ndescription: Helps.\n---\n")
    commit_and_tag(origin, "v2.0.0")

    listing = pollen.show(
        origin.as_posix(), agents=[("agents", "claude-code")], cache_dir=cache_dir
    )

    by_path = {agent.path: agent for agent in listing.agents}
    reviewer = by_path["agents/reviewer.md"]
    assert (reviewer.client, reviewer.name, reviewer.description) == (
        "claude-code",
        "reviewer",
        "Reviews code.",
    )
    assert reviewer.details is not None
    assert reviewer.details.hooks
    assert by_path["agents/helper.md"].error is not None


def test_show_rejects_an_unknown_client(origin: Path, cache_dir: Path) -> None:
    with pytest.raises(ValueError, match="cursor"):
        # The invalid client is the point: the stubs rightly reject it.
        pollen.show(origin.as_posix(), agents=[("agents", "cursor")], cache_dir=cache_dir)  # pyright: ignore[reportArgumentType]
