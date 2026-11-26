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
discovery:
  - url: https://gateway.example.com
    skills:
      - name: pdf-summarizer
        digest: sha256:3f2a9c0d5b8e7f41a6c2d9e0b1f3a4c5d6e7f8091a2b3c4d5e6f708192a3b4c5
```

A file takes its skills from two kinds of sources, each with a list of its
own: `repos`, git repositories and the config's own directory, which list
where to look under `skills`, by `path`; and `discovery`, origins serving a
discovery index, which list the skills of the index under `skills`, by `name`.
A file needs at least one of the two lists.

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `version` | no | `1` | The [format version](#format-versions) the file is written in. This page describes version 2. |
| `clients` | no | `all` | Version 2 only. The clients an entry naming neither `clients` nor `targets` deploys for, as one value or a list: `all`, `claude-code` (skills in `.claude/skills`) or `opencode` (skills in `.agents/skills`, the directory most other Agent Skills clients scan too). The directories are in the working directory, or in the home directory with `--global`. `all` is every client this pollen knows. |
| `repos[].repo` | yes | — | A git URL, a path to a git repository, or `local` for the directory holding the config file. |
| `repos[].revision` | for git | — | Tag, branch or commit to check out. A branch draws a warning, since each update deploys whatever it points to then; a full commit SHA is the only revision that cannot move (`pollen validate --pinned` enforces it). Forbidden on `local`. |
| `repos[].skills[].path` | yes | — | Directory to search, relative to the source root. Write it with `/`, which every platform reads. |
| `repos[].skills[].recurse` | no | `false` | Search the whole subtree instead of the immediate children. |
| `repos[].skills[].exclude` | no | — | Regular expression rejecting skills whose path under `path` matches. |
| `repos[].skills[].clients` | no | the top-level `clients` | The clients these skills are deployed for, as one value or a list, with the values of the top-level `clients`. Forbidden with `targets`. |
| `repos[].skills[].targets` | no | — | Directories these skills are deployed into, instead of any client's skill directory; each gets a full copy. A leading `~` is expanded (`~/` everywhere, `~\` on Windows too); a relative path resolves against the working directory, so a user-level config should use `~` paths. Forbidden with `clients`. |
| `discovery[].url` | yes | — | An origin, `https://host[:port]` with no path, serving an [Agent Skills discovery index](/guides/well-known-sources/). |
| `discovery[].exclude` | no | — | Regular expression rejecting the skills of the index whose name matches. Forbidden with `skills`. |
| `discovery[].clients` | no | the top-level `clients` | The clients the origin's skills are deployed for, as `repos[].skills[].clients`. Forbidden with `targets`. |
| `discovery[].targets` | no | — | Directories the origin's skills are deployed into, as `repos[].skills[].targets`. Forbidden with `clients`. |
| `discovery[].skills[].name` | yes | — | A skill to take from the index; without a `skills` list, every skill is taken. |
| `discovery[].skills[].digest` | no | — | The `sha256:<64 hex digits>` of the artifact to deploy. The run fails when the index publishes another one (`pollen validate --pinned` requires it). |

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

## Where the skills go

Each item of a `repo`'s `skills` and each `discovery` origin names where its
skills go: `clients`, the clients to deploy them for, or `targets`, directories
of their own, never both. An entry naming neither takes the top-level
`clients`, which defaults to `all`, so by default every skill goes to both
`.claude/skills` and `.agents/skills`. Set the top-level `clients` to deploy
for some clients only — Claude Code alone, here, in the project or under the
home directory with `--global`:

```yaml
version: 2
clients: claude-code
repos:
  - repo: local
    skills:
      - path: skills
```

Set `clients` or `targets` on an entry to send its skills somewhere else. Here
the review skills go to OpenCode only, and the team's to a directory of their
own:

```yaml
version: 2
repos:
  - repo: local
    skills:
      - path: skills/review
        clients: opencode
      - path: skills/team
        targets:
          - ~/team/skills
```

A client the table of clients does not list is served through
`.agents/skills`, the `opencode` client's skill directory, when it reads that
directory, or through `targets`. Each directory gets a full copy of the skills
sent to it, and two entries sending one skill name to the same directory is an
error.

An update also visits each client skill directory, in the project or under the
home directory with `--global`, whose `.pollen.json` names this config, and
prunes what the config does not send there: dropping a client, or moving an
entry from `clients` to `targets`, removes what pollen deployed in that
client's directory. A directory left with nothing pollen deployed loses its
`.pollen.json`, and is removed when empty. A directory only `targets` named
keeps its skills once no entry names it. For one run, `--target` replaces
every entry's destination, and `--client` keeps only what the entries send to
those clients; that run visits no other directory.

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
| 2 | pollen 0.8 and later | A `repo` lists where to look under `skills`. The origins serving a discovery index move out of `repos` to a list of their own, `discovery`, each naming its origin under `url`. Each item of `skills` and each `discovery` origin names its destination with `clients` or `targets`, and the top-level `clients` covers the entries naming neither, with no top-level `targets`. |

`pollen config migrate` rewrites a file in the newest version, in place:
comments and layout are kept, a top-level `targets` is copied onto every
entry, and the file is written only if it declares the same sources and
destinations as before. `--dry-run` shows what it would do.

```console
$ pollen config migrate
pollen.yaml: migrated from version 1 to version 2
```

A version 1 file has no `version`, `paths` in place of each `repo` entry's
`skills`, its discovery origins in `repos`, each under `well-known`, and one
top-level `targets` for every entry in place of `clients` and `targets` on
each:

```yaml
targets:
  - ~/team/skills
repos:
  - repo: local
    paths:
      - path: skills
  - well-known: https://gateway.example.com
    skills:
      - name: pdf-summarizer
```

The schema validates a file against the version it declares, so an editor
pointed at it checks version 1 files too.
