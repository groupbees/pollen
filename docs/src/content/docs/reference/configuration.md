---
title: Configuration file
description: Every key of pollen.yaml, how skills are discovered, and the JSON Schema.
sidebar:
  order: 1
---

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
[examples/pollen.yaml](https://github.com/groupbees/pollen/blob/main/examples/pollen.yaml)
for a commented config.

Set `targets` yourself to deploy elsewhere — a single directory, or the
user-level pair:

```yaml
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
