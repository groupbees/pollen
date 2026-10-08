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

Keep one user-level config for those directories: an update prunes whatever
its config no longer declares, so a second config deploying into
`~/.claude/skills` would remove the first one's skills.

Refresh from the cache on a plane:

```sh
pollen update --offline
```

The result goes to stdout and the diagnostics to stderr, so
`pollen update > changes.txt` keeps both readable. Only warnings and errors
reach stderr by default; `--log-filter info` also logs each skill as it is
installed or removed. Every flag also has an
[environment variable](/reference/environment-variables/).
