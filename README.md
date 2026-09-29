# pollen

<p align="center">
  <img src="assets/logo.svg" alt="pollen logo" width="480">
</p>

Deploy [Agent Skills](https://agentskills.io) from git repositories and local
directories, declaratively, from a single `pollen.yaml`.

## Description

`pollen` is a small CLI that reads a `pollen.yaml` describing where your
skills come from — git repositories pinned to a revision, or directories next
to the config file — and makes your skills directories match it: it installs
what is declared, refreshes what moved, and removes what you dropped. The file
is shaped after `pre-commit`'s: a list of repos, each with a revision and the
paths to pull from.

Out of the box it deploys to **both** `.claude/skills/`, which Claude Code
reads, and `.agents/skills/`, the cross-client convention every other Agent
Skills client scans — so one `pollen update` serves Claude Code, Cursor,
Codex, Gemini CLI, Copilot, OpenCode and the rest without configuring
anything. A skill folder that also carries a `.claude-plugin/plugin.json` is
copied whole, so plugin-shaped skills keep working too.

What it is not: a registry, a package manager with dependency resolution, or a
skill authoring tool. It moves directories that already exist and validates
them against the specification.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the design and the reasoning behind
it.

## Getting started

### Prerequisites

- `git`, on `PATH` — `pollen` shells out to it for every remote source.
- Rust ≥ 1.97.1, only to build from source.

### Installation

Download the archive for your platform from the
[releases](https://github.com/groupbees/pollen/releases) — macOS on
Apple Silicon and Intel, Linux on x86_64 and arm64 as `.tar.gz`, Windows on
x86_64 and arm64 as `.zip`. The Linux binaries are statically linked, so they
run on any distribution; the Windows ones need no Visual C++ runtime. Unpack
the archive and put `pollen` (`pollen.exe` on Windows) on your `PATH`.
`SHA256SUMS` on the same page lists the archives' checksums:

```sh
sha256sum --check --ignore-missing SHA256SUMS
```

On Windows, compare the output of this PowerShell command with the archive's
line in `SHA256SUMS`:

```powershell
Get-FileHash -Algorithm SHA256 pollen-x86_64-pc-windows-msvc-v*.zip
```

Or build it from source:

```sh
cargo install --git https://github.com/groupbees/pollen
```

### Configuration

`pollen.yaml`, read from the working directory unless `--config` says
otherwise:

```yaml
repos:
  - repo: https://github.com/toto/tata
    revision: 1.2.3
    paths:
      - path: mydir/
        recurse: true
        exclude: ^toto
  - repo: local
    paths:
      - path: skills
```

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `targets` | no | `.claude/skills` and `.agents/skills` | Directories the skills are deployed into, each getting a full copy. A leading `~` is expanded (`~/` everywhere, `~\` on Windows too); a relative path resolves against the working directory. |
| `repos[].repo` | yes | — | A git URL, a path to a git repository, or `local` for the directory holding the config file. |
| `repos[].revision` | for git | — | Tag, branch or commit to check out. Forbidden on `local`. |
| `repos[].paths[].path` | yes | — | Directory to search, relative to the source root. Write it with `/`, which every platform reads. |
| `repos[].paths[].recurse` | no | `false` | Search the whole subtree instead of the immediate children. |
| `repos[].paths[].exclude` | no | — | Regular expression rejecting skills whose path under `path` matches. |

A `path` that itself holds a `SKILL.md` is taken as one skill. Otherwise its
subdirectories are searched, and a skill found on the way is never descended
into. A `local` source's paths resolve against the config file's directory, so
a config can be shared without its sources moving. See
[examples/pollen.yaml](examples/pollen.yaml) for a commented config.

Set `targets` yourself to deploy elsewhere — a single directory, or the
user-level pair:

```yaml
targets:
  - ~/.claude/skills
  - ~/.agents/skills
```

A [JSON Schema](schema/pollen.schema.json) describes the file. Point your
editor at it for completion and inline errors, by adding this first line to
`pollen.yaml`:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/groupbees/pollen/main/schema/pollen.schema.json
```

`pollen schema` prints the same document, for a validator that wants it on
stdin. It covers the file's shape; `pollen validate` goes further and checks
the sources and the skills themselves.

Every option is also an environment variable:

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `POLLEN_CONFIG_FILE` | no | `pollen.yaml` | Path to the configuration file. |
| `POLLEN_SKILLS_DIR` | no | — | Target directories, overriding the config's `targets`, separated like `PATH`: `;` on Windows, `:` elsewhere. |
| `POLLEN_CACHE_DIR` | no | `<cache>/pollen` | Where the git checkouts are kept between runs. |
| `POLLEN_OFFLINE` | no | `false` | Work from the cache only, contacting no remote. |
| `POLLEN_DRY_RUN` | no | `false` | Report what `update` would change, and stop. |
| `POLLEN_FORCE` | no | `false` | Let `update` take over a directory it did not install. |
| `POLLEN_LOG_FILTER` | no | `info` | `tracing` filter directive, e.g. `pollen=debug`. |

### Usage

Deploy everything the config declares:

```sh
pollen update
```

See what would change first:

```sh
pollen update --dry-run
```

Check the config and every skill it selects, without deploying:

```sh
pollen validate
```

Check config files alone, fetching nothing:

```sh
pollen validate --config-only pollen.yaml
```

List what is currently deployed:

```sh
pollen list
```

Print the JSON Schema for the config file:

```sh
pollen schema
```

Deploy somewhere else, for this run only:

```sh
pollen update --target ~/.claude/skills --target ~/.agents/skills
```

Refresh from the cache on a plane:

```sh
pollen update --offline
```

The result goes to stdout and the diagnostics to stderr, so
`pollen update > changes.txt` keeps both readable.

### Pre-commit hook

`pollen` publishes a hook, so a repository that carries a `pollen.yaml`
can keep it honest. In that repository's `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/groupbees/pollen
    rev: v0.1.0
    hooks:
      - id: pollen-validate
```

Pin `rev` to a released tag. The hook runs on every `pollen.yaml` a commit
touches and checks that it parses and its rules hold. It fetches nothing, so
it stays fast and works offline — and therefore says nothing about the skills
those sources would yield. Run `pollen validate` for that, in CI or by hand.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache-2.0 — see [LICENSE](LICENSE).
