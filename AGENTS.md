<!-- agents-md-manager:start — managed section, do not edit by hand -->
# pollen — agent guide

Rust CLI that deploys Agent Skills declared in `pollen.yaml`. Design and its
reasons: [ARCHITECTURE.md](ARCHITECTURE.md); workflow and CI:
[CONTRIBUTING.md](CONTRIBUTING.md). Using pollen (not developing it):
[`skills/pollen`](skills/pollen/SKILL.md).

## Before you push

```sh
cargo test --all-targets --locked
cargo clippy --all-targets -- -D warnings          # pedantic, warnings denied
cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings
pre-commit run --all-files                         # fmt, schema, docs, actionlint…
```

## Rules

- New behaviour comes with tests; a fix comes with the regression test that
  would have caught it. Tests stay offline.
- Config types changed → `./scripts/generate-schema.sh` in the same commit.
- A user-facing change updates `docs/src/content/docs/` (and the environment
  variables table for a new flag) in the same PR.
- Platform-specific code is gated `#[cfg(unix)]` / `#[cfg(windows)]`; CI runs
  Linux and Windows.
- Releases only through `scripts/release.sh <version>`.
- Commits: descriptive titles, no Conventional Commits prefix, no
  `Co-Authored-By` or "Generated with" line (a hook rejects them). PRs:
  squash by default.

## Invariants — do not "fix" these

- pollen only replaces or removes directories recorded in `.pollen.json`.
- One config per target directory; `--force` is the only override.
- No lockfile: the commit pinned in `pollen.yaml` is the lock (see
  ARCHITECTURE.md, *Design decisions*).
- A revision resolves to a commit or the run fails — never a guess.
<!-- agents-md-manager:end -->
