# pollen — agent guide

Rust CLI that deploys Agent Skills declared in `pollen.yaml`, with Python
bindings in `pollen-py/` (PyPI: `pollen-skills`). Design and its
reasons: [ARCHITECTURE.md](ARCHITECTURE.md); workflow and CI:
[CONTRIBUTING.md](CONTRIBUTING.md). Using pollen (not developing it):
[`skills/pollen`](skills/pollen/SKILL.md).

## Before you push

```sh
task test           # Rust, the Python bindings, the docs site
task lint           # every pre-commit hook: fmt, clippy, schema, ruff, pyright, docs…
task lint:windows   # clippy on the Windows code paths, from any host
```

Tools come from `mise install` (versions in `mise.toml`); `task --list`
shows the rest.

## Rules

- New behaviour comes with tests; a fix comes with the regression test that
  would have caught it. Tests stay offline.
- Config types changed → `task schema` in the same commit.
- A command's options or results changed → update `pollen-py` in the same PR:
  the binding in `src/lib.rs`, the dataclasses in `python/pollen/_types.py`,
  the stubs in `_pollen.pyi`, and the Python guide.
- The library never prints; only a subcommand's `run` writes to stdout.
- A user-facing change updates `docs/src/content/docs/` (and the environment
  variables table for a new flag) in the same PR.
- Platform-specific code is gated `#[cfg(unix)]` / `#[cfg(windows)]`; CI runs
  Linux and Windows.
- Releases only through `scripts/release.sh <version>` (`task release -- <version>`).
- Commits: descriptive titles, no Conventional Commits prefix, no
  `Co-Authored-By` or "Generated with" line (a hook rejects them). PRs:
  squash by default.

## Invariants — do not "fix" these

- pollen only replaces or removes directories recorded in `.pollen.json`.
- One config per target directory; `--force` is the only override.
- No lockfile: the commit pinned in `pollen.yaml` is the lock (see
  ARCHITECTURE.md, *Design decisions*).
- A revision resolves to a commit or the run fails — never a guess.
