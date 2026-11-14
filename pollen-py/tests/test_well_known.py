import hashlib
import json
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

import pollen

SCHEMA = "https://schemas.agentskills.io/discovery/0.2.0/schema.json"
TOKEN = "s3cret-for-tests"


def skill_md(name: str, description: str) -> bytes:
    return f"---\nname: {name}\ndescription: {description}\n---\n".encode()


def digest(body: bytes) -> str:
    return f"sha256:{hashlib.sha256(body).hexdigest()}"


class Origin:
    """A local origin serving a discovery index, to clients sending `TOKEN`."""

    def __init__(self) -> None:
        self.files: dict[str, bytes] = {}
        origin = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                body = origin.files.get(self.path)
                if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                    self.send_response(401)
                    body = b""
                elif body is None:
                    self.send_response(404)
                    body = b""
                else:
                    self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, format: str, *args: object) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"

    def publish(self, skills: dict[str, bytes]) -> None:
        entries = [
            {
                "name": name,
                "type": "skill-md",
                "description": name,
                "url": f"{name}/SKILL.md",
                "digest": digest(body),
            }
            for name, body in skills.items()
        ]
        self.files = {
            "/.well-known/agent-skills/index.json": json.dumps(
                {"$schema": SCHEMA, "skills": entries}
            ).encode(),
        } | {f"/.well-known/agent-skills/{name}/SKILL.md": body for name, body in skills.items()}


@pytest.fixture
def origin() -> Iterator[Origin]:
    served = Origin()
    thread = threading.Thread(target=served.server.serve_forever, daemon=True)
    thread.start()
    yield served
    served.server.shutdown()


@pytest.fixture
def credentials(tmp_path: Path, origin: Origin, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setenv("POLLEN_TEST_WELL_KNOWN_TOKEN", TOKEN)
    path = tmp_path / "credentials.yaml"
    path.write_text(
        f"credentials:\n  - origin: {origin.url}\n    env: POLLEN_TEST_WELL_KNOWN_TOKEN\n"
    )
    return path


@pytest.fixture
def config(tmp_path: Path, origin: Origin) -> Path:
    path = tmp_path / "pollen.yaml"
    path.write_text(f"repos:\n  - well-known: {origin.url}\n    skills:\n      - name: alpha\n")
    return path


def test_update_deploys_a_skill_from_an_authenticated_origin(
    tmp_path: Path, origin: Origin, credentials: Path, config: Path
) -> None:
    body = skill_md("alpha", "first")
    origin.publish({"alpha": body, "beta": skill_md("beta", "b")})
    target = tmp_path / "deployed"

    report = pollen.update(
        config=config,
        targets=[target],
        cache_dir=tmp_path / "cache",
        credentials_file=credentials,
    )

    assert [change.name for change in report.summaries[0].changes] == ["alpha"]
    [deployed] = pollen.list_installed(config=config, targets=[target])
    assert deployed.skills[0].digest == digest(body)


def test_an_origin_refusing_the_request_raises(
    tmp_path: Path, origin: Origin, config: Path
) -> None:
    origin.publish({"alpha": skill_md("alpha", "first")})
    empty = tmp_path / "none.yaml"
    empty.write_text("credentials: []\n")

    with pytest.raises(pollen.PollenError, match="requires authentication"):
        pollen.update(
            config=config,
            targets=[tmp_path / "deployed"],
            cache_dir=tmp_path / "cache",
            credentials_file=empty,
        )


def test_freeze_pins_the_digest_and_outdated_sees_it_move(
    tmp_path: Path, origin: Origin, credentials: Path, config: Path
) -> None:
    first = skill_md("alpha", "first")
    origin.publish({"alpha": first})
    cache_dir = tmp_path / "cache"

    frozen = pollen.autoupdate(
        config=config, cache_dir=cache_dir, credentials_file=credentials, freeze=True
    )

    assert frozen.digests == [
        pollen.DigestBump(
            origin=origin.url, skill="alpha", from_digest=None, to_digest=digest(first)
        )
    ]
    assert f"digest: {digest(first)}" in config.read_text()

    second = skill_md("alpha", "second")
    origin.publish({"alpha": second})
    [status] = pollen.outdated(config=config, cache_dir=cache_dir, credentials_file=credentials)

    assert status == pollen.DigestStatus(
        origin=origin.url,
        skill="alpha",
        pinned=digest(first),
        published=digest(second),
        held_back=False,
        is_outdated=True,
    )
