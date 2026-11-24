---
title: Configuration file
description: Every key of pollen.yaml, how skills are discovered, and the JSON Schema.
sidebar:
  order: 1
---

`pollen.yaml`, read from the working directory unless `--config` says
otherwise:

```yaml
version: 2
repos:
  - repo: https://github.com/toto/tata
    revision: 1.2.3
    skills:
      - path: mydir/
        recurse: true
        exclude: ^toto
  - repo: local
    skills:
      - path: skills
  - well-known: https://gateway.example.com
    skills:
      - name: pdf-summarizer
        digest: sha256:3f2a9c0d5b8e7f41a6c2d9e0b1f3a4c5d6e7f8091a2b3c4d5e6f708192a3b4c5
```

Each entry of `repos` is a `repo`, a git repository or the config's own
directory, or a `well-known` origin. Each lists what it takes under `skills`:
where to look, by `path`, on a `repo`; skills of the index, by `name`, on a
`well-known` origin.

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `version` | no | `1` | The [format version](#format-versions) the file is written in. This page describes version 2. |
| `targets` | no | `.claude/skills` and `.agents/skills`; with `--global`, `~/.claude/skills` and `~/.agents/skills` | Directories the skills are deployed into, each getting a full copy. A leading `~` is expanded (`~/` everywhere, `~\` on Windows too); a relative path resolves against the working directory, so a user-level config should use `~` paths. |
| `repos[].repo` | unless `well-known` | — | A git URL, a path to a git repository, or `local` for the directory holding the config file. |
| `repos[].revision` | for git | — | Tag, branch or commit to check out. A branch draws a warning, since each update deploys whatever it points to then; a full commit SHA is the only revision that cannot move (`pollen validate --pinned` enforces it). Forbidden on `local` and `well-known`. |
| `repos[].skills[].path` | with `repo` | — | Directory to search, relative to the source root. Write it with `/`, which every platform reads. Forbidden on `well-known`. |
| `repos[].skills[].recurse` | no | `false` | With `repo`: search the whole subtree instead of the immediate children. |
| `repos[].skills[].exclude` | no | — | With `repo`: regular expression rejecting skills whose path under `path` matches. |
| `repos[].well-known` | unless `repo` | — | An origin, `https://host[:port]` with no path, serving an [Agent Skills discovery index](/guides/well-known-sources/). |
| `repos[].exclude` | no | — | With `well-known`: regular expression rejecting the skills of the index whose name matches. Forbidden with `skills`. |
| `repos[].skills[].name` | with `well-known` | — | A skill to take from the index; without a `skills` list, every skill is taken. Forbidden on `repo`. |
| `repos[].skills[].digest` | no | — | With `well-known`: the `sha256:<64 hex digits>` of the artifact to deploy. The run fails when the index publishes another one (`pollen validate --pinned` requires it). |

A `path` that itself holds a `SKILL.md` is taken as one skill. Otherwise its
subdirectories are searched, and a skill found on the way is never descended
into. A `local` source's paths resolve against the config file's directory, so
a config can be shared without its sources moving. See
[examples/pollen.yaml](https://github.com/groupbees/pollen/blob/main/examples/pollen.yaml)
for a commented config.

Every selected skill directory must remain inside its source root after symlink
resolution. A child directory symlink resolving outside the root refuses the run.
Links within the root retain their selected name and path. Excluded candidates
are not selected and do not enter this check.

Set `targets` yourself to deploy elsewhere — a single directory, or the
user-level pair, which is also what `--global` uses when the user-level config
sets none:

```yaml
version: 2
targets:
  - ~/.claude/skills
  - ~/.agents/skills
```

A
[JSON Schema](https://github.com/groupbees/pollen/blob/main/schema/pollen.schema.json)
describes the file. Point your editor at it for completion and inline errors, by adding this first line to
`pollen.yaml`:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/groupbees/pollen/main/schema/pollen.schema.json
```

`pollen schema` prints the same document, for a validator that wants it on
stdin. It covers the file's shape; `pollen validate` goes further and checks
the sources and the skills themselves (see [Usage](/guides/usage/)).

## Format versions

A file declares the version of the format it is written in with `version`,
and one without it is in version 1. pollen reads every version up to the
newest it knows, so upgrading pollen never breaks a config, and a file in a
newer version than the installed pollen reads is refused with a message
saying to upgrade it. The version only moves when a key is renamed or
removed, or changes meaning; a new optional key does not move it.

| Version | Read by | Change |
| --- | --- | --- |
| 1 | every release | The original format: a `repo` lists where to look under `paths`. |
| 2 | pollen 0.8 and later | A `repo` lists where to look under `skills`, as a `well-known` source lists its skills. |

`pollen migrate` rewrites a file in the newest version, in place: comments and
layout are kept, and the file is written only if it declares the same sources
and targets as before. `--dry-run` shows what it would do.

```console
$ pollen migrate
pollen.yaml: migrated from version 1 to version 2
```

A version 1 file is the same file with `paths` in place of each `repo`
entry's `skills`, and no `version`:

```yaml
repos:
  - repo: local
    paths:
      - path: skills
```

The schema validates a file against the version it declares, so an editor
pointed at it checks version 1 files too.
