---
title: Usage
description: Deploy, preview, validate and list skills with the pollen commands.
sidebar:
  order: 2
---

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

Require every git source to be pinned to a full commit SHA, offline:

```sh
pollen validate --config-only --pinned
```

A branch moves on every push and a tag can be moved; a commit cannot. Pin the
commit and keep the tag readable in a comment, which a bot such as Renovate can
keep up to date:

```yaml
revision: acad0f52027cf8f8edf7bfa6a55e13c594d8ee71  # v0.1.0
```

Whatever the flags, `update` and `validate` warn when a revision is a branch.

See which sources have a newer release, from the remotes' tags:

```sh
pollen outdated
```

Move every source to its newest release, rewriting only the `revision:` lines
of `pollen.yaml` — comments and layout are kept, and the file is written only
if it still validates:

```sh
pollen autoupdate --dry-run
pollen autoupdate
```

`--freeze` pins each source to the commit of its release instead, with the
release in a comment (`revision: <sha>  # v0.2.0`); run on an up-to-date tag,
it only freezes it. Only stable `vX.Y.Z` or `X.Y.Z` tags count as releases,
and a branch revision is left alone.

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

Deploy the skills you want in every project, from your user-level config:

```sh
pollen update --global
```

`--global` (`-g`) reads `$XDG_CONFIG_HOME/pollen/pollen.yaml` —
`~/.config/pollen/pollen.yaml` when the variable is unset, on macOS too, and
`%APPDATA%\pollen\pollen.yaml` on Windows — and, when that file sets no
`targets`, deploys into `~/.claude/skills` and `~/.agents/skills`. It works
with every command (`pollen list -g`, `pollen validate -g`) and cannot be
combined with `--config`.

A target directory belongs to one config: an update prunes whatever its config
no longer declares, so a second config deploying into the same directory would
remove the first one's skills. `pollen update` therefore refuses a directory
another config manages and names that config; declare the sources there, or
pass `--force` to hand the directory over.

Refresh from the cache on a plane:

```sh
pollen update --offline
```

The result goes to stdout and the diagnostics to stderr, so
`pollen update > changes.txt` keeps both readable. Only warnings and errors
reach stderr by default; `--log-filter info` also logs each skill as it is
installed or removed. Every flag also has an
[environment variable](/reference/environment-variables/).
