# Contributing

Issues and pull requests go to
<https://github.com/groupbees/pollen>. The short version: install mise, run `task setup`, keep `task test` and
`task lint` green.

Report a bug in a [GitHub issue](https://github.com/groupbees/pollen/issues),
with the version (`pollen --version`), the operating system, the
`pollen.yaml` and the command that reproduce it, and what you expected
instead. Report a vulnerability privately, as [SECURITY.md](SECURITY.md)
describes, never in a public issue or pull request.

## Development setup

[mise](https://mise.jdx.dev) installs every tool at the version
[mise.toml](mise.toml) pins: the Rust toolchain with `rustfmt` and `clippy`,
Python, uv, Node.js for the documentation site, Task, pre-commit, Trivy,
`cargo-llvm-cov` for the coverage, and `cargo-edit` for cutting a release.
[mise.lock](mise.lock) holds their checksums.

Install mise and hook it into your shell, as its
[installation guide](https://mise.jdx.dev/installing-mise.html) describes
(works the same on macOS and Linux):

```sh
curl https://mise.run | sh
```

Then, from a fresh clone, trust the repository's config, install the tools,
and install the documentation site's dependencies and the git hooks:

```sh
mise trust
```

```sh
mise install
```

```sh
task setup
```

A green `task test` means the setup is good; `task --list` shows the other
tasks, defined in [Taskfile.yml](Taskfile.yml). See the
[documentation](https://groupbees.github.io/pollen/) for running the tool
itself.

## Running the tests

The whole suite — Rust, the Python bindings and the documentation site —
which must be green before a pull request:

```sh
task test
```

The Rust tests alone:

```sh
task test:rust
```

A single test, while iterating:

```sh
task test:rust -- --lib config::tests::rejects_a_git_repo_without_a_revision
```

The workspace holds two crates: [pollen/](pollen/), the library and the CLI,
and [pollen-py/](pollen-py/), the Python bindings. `cargo test` and
`cargo build` cover `pollen` alone (`default-members`): the bindings build
through maturin, as a Python extension.

There are two layers in `pollen`, both run by the command above:

- **Unit tests**, inside each module in [pollen/src/](pollen/src/). The ones
  in [pollen/src/command/update.rs](pollen/src/command/update.rs) drive the
  orchestration through a `mockall` double of the `Materializer` trait, so
  they never touch a network. The `well-known` source and its HTTP client are
  tested against a `wiremock` server listening on `127.0.0.1`.
- **An end-to-end test**,
  [pollen/tests/end_to_end.rs](pollen/tests/end_to_end.rs), which builds a
  real git repository in a temporary directory, tags it, and runs the real
  fetch-discover-install path against it, and does the same for a
  `well-known` origin served by `wiremock`.

Everything runs offline and needs no fixtures or services — but `git` must be
on `PATH`, since the tests that create a repository call it. They configure
`user.name`, `user.email` and `commit.gpgsign=false` in each fixture
repository, so a global signing setup does not break them.

The suite runs on Linux and Windows in CI. A test whose behaviour depends on
the platform is gated with `#[cfg(unix)]` or `#[cfg(windows)]`.

The Python bindings are tested from Python, with pytest, in
[pollen-py/tests/](pollen-py/tests/). `uv run` builds the extension with
maturin first, and rebuilds it whenever a Rust source in either crate changes:

```sh
task test:python
```

```sh
task test:python -- -k show
```

They build git repositories in temporary directories, like the end-to-end
test, and need nothing else. CI runs them on Linux and Windows, with the oldest
and the newest supported Python.

The documentation site has its own unit tests, for the version planning,
the version picker and the link rewriting:

```sh
task test:docs
```

[scripts/release.sh](scripts/release.sh) is checked against a throwaway
repository and a local bare origin: it refuses a tag that exists locally or on
origin, and refuses to go on when origin's tags cannot be listed. It only ever
runs the script with `--dry-run`, and needs Python 3.11 or later:

```sh
task test:release
```

pollen is also checked against a real gateway: a LiteLLM proxy and its
Postgres, from [tests/litellm/docker-compose.yaml](tests/litellm/docker-compose.yaml).
The script uploads the skills under
[tests/litellm/skills/](tests/litellm/skills/), then deploys, freezes,
validates and re-checks them through the proxy's `well-known` index. It is
not part of `task test`, which stays offline: it pulls images and starts
services. It needs Docker with Compose, `curl` and `zip`, and port 4000 free
(`POLLEN_LITELLM_PORT` picks another):

```sh
task test:litellm
```

The Homebrew formula generator is checked against a fabricated `SHA256SUMS`
by a script that renders the formula, runs `brew style` and `brew audit` on it
in a throwaway local tap, and checks that a missing archive is refused. It
needs [Homebrew](https://brew.sh) on `PATH`, so the task runs on macOS only:

```sh
task test:homebrew
```

Coverage is measured for the Rust, Python and documentation site tests; the
shell scripts under `tests/` are left out. No threshold is enforced. One
command runs the three suites instrumented, prints their summaries and writes
their reports:

```sh
task coverage
```

| Suite | Tool | What it measures | Report |
| --- | --- | --- | --- |
| Rust | [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) | Lines and regions of `pollen`; branches need a nightly toolchain | `target/llvm-cov/html/` |
| Python | [coverage.py](https://coverage.readthedocs.io), through `pytest-cov` | Lines and branches of the Python layer in `pollen-py/python/pollen/`; not the extension module, nor the bindings' Rust code it is built from | `pollen-py/htmlcov/` |
| Documentation site | Node's test runner (`--experimental-test-coverage`) | Lines, branches and functions of `docs/src/`, test files excluded | `docs/coverage/lcov.info` |

`task coverage:rust`, `task coverage:python` and `task coverage:docs` run one
suite. After a run, print the summaries again without running the tests:

```sh
task coverage:summary
```

New behaviour comes with tests, and a fix comes with the regression test that
would have caught it.

## Pre-commit hooks

`task setup` runs `pre-commit install`, which puts the gate in place, for both the `pre-commit` and
the `commit-msg` stages; the hooks must pass before a push.

The whole repo, as CI runs it:

```sh
task lint
```

One hook, while iterating:

```sh
pre-commit run cargo-clippy --all-files
```

The hooks are listed in
[.pre-commit-config.yaml](.pre-commit-config.yaml), and the code style is the
one they enforce. pre-commit installs each upstream hook into an environment of
its own, with the Python and Node.js mise installs: `zizmor`, for instance,
comes from its PyPI wheel. The `local` hooks call the tools mise puts on the
`PATH`. Most hooks fix the file themselves, or name what to fix; these need
more:

| Hook | What it checks | How to fix |
| --- | --- | --- |
| `yamllint` | YAML style, per [.yamllint.yaml](.yamllint.yaml) — block style only, no leading `---` | Rewrite the YAML |
| `zizmor` | The GitHub workflows against GitHub's schema and its security audits: untrusted input in a `run:` script, `persist-credentials`, token permissions, unpinned actions, cache poisoning. Runs offline | Fix the workflow as the audit's linked documentation describes; never silence it with `# zizmor: ignore[…]` |
| `renovate-config-validator` | [renovate.json](renovate.json) is valid and uses no option Renovate has renamed | `npx --package renovate -- renovate-config-validator --migrate renovate.json` for a renamed option; otherwise fix the key it names |
| `cargo-clippy` | `clippy` with `pedantic` on both crates, warnings denied | Fix the finding, or add a scoped `#[allow]` with a justification |
| `pyright` | Types of the Python package, its stubs and its tests | Fix the type, or the stub in [pollen-py/python/pollen/_pollen.pyi](pollen-py/python/pollen/_pollen.pyi) |
| `license-copy` | [pollen-py/LICENSE](pollen-py/LICENSE), which the Python package ships, is identical to [LICENSE](LICENSE) | `cp LICENSE pollen-py/LICENSE` |
| `pollen-validate` | [examples/pollen.yaml](examples/pollen.yaml) still satisfies the config rules | Fix the example, or the rule that broke it |
| `pollen-schema` | [schema/pollen.schema.json](schema/pollen.schema.json) matches the config types | Re-run; the hook rewrites the file |
| `docs-biome` | Biome lint and format of the site, per [docs/biome.jsonc](docs/biome.jsonc) | `npm --prefix docs run format` |
| `no-co-authors` | The commit message carries no `Co-Authored-By:` trailer and no `Generated with` line; runs at `commit-msg`, so `--all-files` and CI skip it | Rewrite the message |

[.pre-commit-hooks.yaml](.pre-commit-hooks.yaml) at the repository root is a
different file with a confusingly similar name: it declares the hook *other*
repositories consume (see the
[pre-commit hook guide](https://groupbees.github.io/pollen/guides/pre-commit-hook/)), and nothing
in it runs here. The `pollen-validate` hook above is this repo dogfooding the
same check through `cargo run`.

`--no-verify` and `SKIP=` are not the fix. A red hook is a real finding: fix
the code, or change the configuration in the same pull request and say why.
The same hooks run in CI, so skipping locally only moves the failure somewhere
slower. The test suite deliberately runs in CI rather than in a hook, so the
commit gate stays fast.

## Continuous integration

| Workflow | Purpose | Reproduce locally |
| --- | --- | --- |
| [quality.yaml](.github/workflows/quality.yaml) | The merge gate: hooks, tests on Linux and Windows, the LiteLLM and Homebrew checks, release builds | `task lint`, `task test`, `task test:litellm`, `task test:homebrew` |
| [coverage.yaml](.github/workflows/coverage.yaml) | The Rust, Python and documentation site tests' coverage on Linux: a table per suite in the run's summary, the reports as artifacts | `task coverage` |
| [binaries.yaml](.github/workflows/binaries.yaml) | The release archive of every target, for `quality` and `release` | `task build -- --target <target>`; for musl, on Linux of the same architecture with `musl-tools`, prefixed with `CC_x86_64_unknown_linux_musl=musl-gcc` (or `CC_aarch64_unknown_linux_musl`) |
| [docs.yaml](.github/workflows/docs.yaml) | Every version of the documentation site, links validated, deployed to GitHub Pages from `main` | `git fetch --tags` then `task docs:build` |
| [security.yaml](.github/workflows/security.yaml) | Trivy scan of the repository | `trivy fs .` |
| [scorecard.yaml](.github/workflows/scorecard.yaml) | OpenSSF Scorecard analysis, for the README badge | none: it reads the repository's settings on GitHub |
| [renovate.yaml](.github/workflows/renovate.yaml) | Dependency and security update pull requests, from Dependabot alerts and [OSV](https://osv.dev), per [renovate.json](renovate.json) | none: run it manually with `dryRun` to see what it would open |
| [release.yaml](.github/workflows/release.yaml) | Publishes a `v*` tag: archives, SBOM, attestations and their Sigstore bundle, the Homebrew formula, the PyPI wheels | none: it publishes |

`quality` and `security` are the checks that block a merge. `coverage` is
informational, in a workflow of its own because every job of those two is
required. `docs` is path-filtered on `docs/` outside `docs/adr/`, `assets/`,
the mise and Task files and its own workflow, so it is never a required check.
For the Windows code paths without a Windows host, `task lint:windows` runs clippy on them.

GitHub's CodeQL code scanning, in default setup, analyses the Rust, the
Python and the workflows on every pull request, every push to `main` and
weekly. It has no workflow file and no local equivalent, and is not a
required check: its analysis outlasts the CI jobs.

The pipeline needs these secrets and variables:

- `HOMEBREW_TAP_CLIENT_ID` (variable) and `HOMEBREW_TAP_PRIVATE_KEY`
  (secret): the GitHub App that lets `release` push the formula to
  [groupbees/homebrew-tap](https://github.com/groupbees/homebrew-tap), the
  only repository the pipeline writes to besides this one.
- `RENOVATE_TOKEN` (secret): the token `renovate` opens its pull requests
  with, since a pull request opened with `GITHUB_TOKEN` would trigger no CI.

Everything else uses the workflow's own `GITHUB_TOKEN`. PyPI publishing goes
through trusted publishing, in the `pypi` environment, so no PyPI token is
stored anywhere.

## The generated JSON Schema

[schema/pollen.schema.json](schema/pollen.schema.json) is generated from
the types in [pollen/src/config/raw.rs](pollen/src/config/raw.rs) and committed, because editors and
external validators point at the file rather than at the binary. Changing the
config types means regenerating it in the same commit:

```sh
task schema
```

The `pollen-schema` pre-commit hook does this for you, and a test fails if
the committed copy is stale — so a forgotten regeneration turns up locally
rather than in review.

## The documentation site

The site at <https://groupbees.github.io/pollen/> is an
[Astro Starlight](https://starlight.astro.build) project in [docs/](docs/).
Pages are Markdown under
[docs/src/content/docs/](docs/src/content/docs/): `guides/` and `reference/`
become the two sidebar groups, ordered by `sidebar.order` in each page's
frontmatter.

- Link another page by its root-absolute path, without the `/pollen/` base:
  `[Usage](/guides/usage/)`. The build adds the base, and fails on a broken
  link or anchor, and on a relative one. Keep internal links out of the
  frontmatter, where the base is not added.
- Link a repository file by its `https://github.com/groupbees/pollen/blob/main/…`
  URL, since the site is served elsewhere.
- The theme's colours come from the logo, in
  [docs/src/styles/theme.css](docs/src/styles/theme.css).

Every stable minor release gets its own version of the site, built from the
pages of its last patch tag inside the current site's config and theme: the
latest release at the root, `main` at `/next/`, older ones at `/vX.Y/`.
Pre-release tags are not versions. With no release, `main` is served at the
root.

Preview while writing, at <http://localhost:4321/pollen/>:

```sh
task docs
```

Build the current pages alone:

```sh
npm --prefix docs run build
```

Build every version, as CI does — it needs the tags:

```sh
git fetch --tags
```

```sh
task docs:build
```

## Submitting a change

- Branch off `main`.
- Commit subjects are imperative, optionally prefixed with an
  area — read `git log --oneline` and follow what is there.
- Documentation moves with the code it describes, in the same pull request:
  the [documentation site](docs/src/content/docs/) for user-facing behaviour,
  including the [Python guide](docs/src/content/docs/guides/python.md) and
  the stubs in [pollen-py/python/pollen/](pollen-py/python/pollen/) when a
  command's options or results change,
  [README.md](README.md) when installing or the first run changes, and
  [ARCHITECTURE.md](ARCHITECTURE.md) for the design and the trade-offs behind
  it.
- [.bestpractices.json](.bestpractices.json) proposes the answers to the
  [OpenSSF Best Practices](https://www.bestpractices.dev/projects/17144)
  form, each citing the file that proves it. A change that breaks one of its
  claims — moves `SECURITY.md`, drops a linter or a test suite, changes the
  TLS or digest code — updates it in the same pull request.
- Label the pull request so it lands in the right release-notes section (see
  [.github/release.yaml](.github/release.yaml)).
- CI must be green.

## Releasing

Releases go through [scripts/release.sh](scripts/release.sh) and nowhere else:
it bumps the workspace version in `Cargo.toml` and `Cargo.lock` together,
commits, tags, and asks before pushing. The Python package takes its version
from the same field, converted to PEP 440 (`0.6.0-rc1` becomes `0.6.0rc1`).
The `release` workflow re-derives the version from the tag and refuses to
publish when the two disagree, so a hand-made tag fails loudly instead of
shipping a mislabelled binary.

```sh
task release -- --dry-run 0.2.0
```

```sh
task release -- 0.2.0
```

The release notes are the GitHub release's, generated from the labels of the
merged pull requests, sorted into the sections of
[.github/release.yaml](.github/release.yaml). A security fix's notes cite its
advisory, as [SECURITY.md](SECURITY.md#what-happens-next) describes.
