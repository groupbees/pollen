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

Refresh from the cache on a plane:

```sh
pollen update --offline
```

The result goes to stdout and the diagnostics to stderr, so
`pollen update > changes.txt` keeps both readable. Every flag also has an
[environment variable](/reference/environment-variables/).
