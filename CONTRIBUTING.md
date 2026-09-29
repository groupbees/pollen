# Contributing

Issues and pull requests go to
<https://github.com/groupbees/pollen>. The short version: install the
toolchain, run `pre-commit install`, keep `cargo test` green.

## Development setup

The toolchain version is pinned in
[rust-toolchain.toml](rust-toolchain.toml); `rustup` installs it on the first
`cargo` invocation, with `rustfmt` and `clippy`.

Install `rustup` (works the same on macOS and Linux):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Install `pre-commit`:

```sh
uv tool install pre-commit
```

Install `shellcheck`, which the shell hook shells out to:

**macOS**

```sh
brew install shellcheck
```

**Linux**

```sh
sudo apt install -y shellcheck
```

Install Node.js at the version in [docs/.nvmrc](docs/.nvmrc), for the
documentation site and its hooks, from <https://nodejs.org/en/download> or
any version manager that reads `.nvmrc`.

`cargo-edit` is needed only to cut a release:

```sh
cargo install cargo-edit --locked
```

Then, from a fresh clone:

```sh
pre-commit install
```

```sh
npm ci --prefix docs
```

```sh
cargo test
```

A green `cargo test` means the setup is good. See the
[documentation](https://groupbees.github.io/pollen/) for running the tool
itself.

## Running the tests

The whole suite, which must be green before a pull request:

```sh
cargo test --all-targets
```

A single test, while iterating:

```sh
cargo test --lib config::tests::rejects_a_git_repo_without_a_revision
```

There are two layers, both run by the command above:

- **Unit tests**, inside each module in [src/](src/). The ones in
  [src/command/update.rs](src/command/update.rs) drive the orchestration
  through a `mockall` double of the `Materializer` trait, so they never touch
  a network.
- **An end-to-end test**, [tests/end_to_end.rs](tests/end_to_end.rs), which
  builds a real git repository in a temporary directory, tags it, and runs the
  real fetch-discover-install path against it.

Everything runs offline and needs no fixtures or services — but `git` must be
on `PATH`, since the tests that create a repository call it. They configure
`user.name`, `user.email` and `commit.gpgsign=false` in each fixture
repository, so a global signing setup does not break them.

The suite runs on Linux and Windows in CI. A test whose behaviour depends on
the platform is gated with `#[cfg(unix)]` or `#[cfg(windows)]`.

The documentation site has its own unit tests, for the version planning,
the version picker and the link rewriting:

```sh
npm --prefix docs test
```

New behaviour comes with tests, and a fix comes with the regression test that
would have caught it.

## Pre-commit hooks

`pre-commit install` puts the gate in place; the hooks must pass before a push.

The whole repo:

```sh
pre-commit run --all-files
```

One hook, while iterating:

```sh
pre-commit run cargo-clippy --all-files
```

| Hook | What it checks | How to fix |
| --- | --- | --- |
| `trailing-whitespace`, `end-of-file-fixer` | Whitespace hygiene | Re-run; the hook rewrites the files |
| `check-yaml`, `check-added-large-files`, `check-merge-conflict`, `detect-private-key` | Basic file sanity | Fix the file it names |
| `yamllint` | YAML style, per [.yamllint.yaml](.yamllint.yaml) — block style only, no leading `---` | Rewrite the YAML |
| `actionlint` | Workflow syntax and expressions | Fix the workflow |
| `cargo-fmt` | Formatting | `cargo fmt` |
| `cargo-clippy` | `clippy` with `pedantic`, warnings denied | Fix the finding, or add a scoped `#[allow]` with a justification |
| `pollen-validate` | [examples/pollen.yaml](examples/pollen.yaml) still satisfies the config rules | Fix the example, or the rule that broke it |
| `pollen-schema` | [schema/pollen.schema.json](schema/pollen.schema.json) matches the config types | Re-run; the hook rewrites the file |
| `shellcheck` | The scripts in [scripts/](scripts/) | Fix the script |
| `docs-biome` | Biome lint and format of the site, per [docs/biome.json](docs/biome.json) | `npm --prefix docs run format` |
| `docs-astro-check` | Types and page frontmatter of the site | Fix the file it names |

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

| Workflow | Triggers on | What it does | Reproduce locally |
| --- | --- | --- | --- |
| [quality.yaml](.github/workflows/quality.yaml) | every push to `main`, every pull request | `pre-commit run --all-files` on Linux, `cargo test --all-targets --locked` on both Linux (`ubuntu-24.04`) and Windows (`windows-2025`), and the documentation site's tests | `pre-commit run --all-files`, `cargo test --all-targets --locked` and `npm --prefix docs test`; for Windows, `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings` type-checks the Windows code paths from any host |
| [docs.yaml](.github/workflows/docs.yaml) | pushes to `main` and pull requests touching `docs/`, `assets/` or the workflow; manual | Builds every version of the documentation site, links validated; on `main`, deploys it to GitHub Pages | `git fetch --tags` then `npm --prefix docs run build:versions` |
| [security.yaml](.github/workflows/security.yaml) | every push to `main`, every pull request, daily, manual | `trivy fs` over the repo, failing on fixable `HIGH`/`CRITICAL`, and a second non-blocking scan uploaded to code scanning | `trivy fs .` |
| [release.yaml](.github/workflows/release.yaml) | tag `v*` | Checks the tag against `Cargo.toml`, builds the `.tar.gz` archives for Linux (musl, x86_64 and arm64) and macOS (arm64 and x86_64) and the `.zip` archives for Windows (MSVC, x86_64 and arm64, each on its native runner), creates the GitHub release with a `SHA256SUMS` over all of them and notes starting from the previous stable tag, then runs `docs` on `main` so the site's root serves the new release. A pre-release tag (`v0.2.0-rc1`) builds the archives and keeps them as workflow artifacts, without a release page | — |

`quality` and `security` are the checks that block a merge; `docs` is
path-filtered, so it is never a required check. `release` has no local
equivalent: it publishes, and it runs only from a tag.

It needs no secrets. `release` uses the workflow's own `GITHUB_TOKEN` with
`contents: write`, scoped to the job that creates the release; `security`
holds `security-events: write` for the SARIF upload and nothing else. The
`docs` deploy job holds `pages: write` and `id-token: write`, and `release`'s
`docs` job `actions: write` to start it. Pages deploys only from `main`, in
the `github-pages` environment.

## The generated JSON Schema

[schema/pollen.schema.json](schema/pollen.schema.json) is generated from
the types in [src/config.rs](src/config.rs) and committed, because editors and
external validators point at the file rather than at the binary. Changing the
config types means regenerating it in the same commit:

```sh
./scripts/generate-schema.sh
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
npm --prefix docs run dev
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
npm --prefix docs run build:versions
```

## Cutting a release

Releases go through [scripts/release.sh](scripts/release.sh) and nowhere else:
it bumps `Cargo.toml` and `Cargo.lock` together, commits, tags, and asks
before pushing. The `release` workflow re-derives the version from the tag and
refuses to publish when the two disagree, so a hand-made tag fails loudly
instead of shipping a mislabelled binary.

```sh
./scripts/release.sh --dry-run 0.2.0
```

```sh
./scripts/release.sh 0.2.0
```

## Submitting a change

- Branch off `main`.
- Commit subjects are imperative and lowercase, optionally prefixed with an
  area — read `git log --oneline` and follow what is there.
- Documentation moves with the code it describes, in the same pull request:
  the [documentation site](docs/src/content/docs/) for user-facing behaviour,
  [README.md](README.md) when installing or the first run changes, and
  [ARCHITECTURE.md](ARCHITECTURE.md) for the design and the trade-offs behind
  it.
- Label the pull request so it lands in the right release-notes section (see
  [.github/release.yaml](.github/release.yaml)).
- CI must be green.
