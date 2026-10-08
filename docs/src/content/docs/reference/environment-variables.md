---
title: Environment variables
description: Every option of pollen, as an environment variable.
sidebar:
  order: 2
---

Every option is also an environment variable:

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `POLLEN_CONFIG_FILE` | no | `pollen.yaml` | Path to the configuration file. |
| `POLLEN_GLOBAL` | no | `false` | Use the user-level config and the user-level skill directories, as `--global` does. |
| `POLLEN_SKILLS_DIR` | no | — | Target directories, overriding the config's `targets`, separated like `PATH`: `;` on Windows, `:` elsewhere. |
| `POLLEN_CACHE_DIR` | no | `<cache>/pollen` | Where the git checkouts are kept between runs. |
| `POLLEN_OFFLINE` | no | `false` | Work from the cache only, contacting no remote. |
| `POLLEN_DRY_RUN` | no | `false` | Report what `update` would change, and stop. |
| `POLLEN_FORCE` | no | `false` | Let `update` take over a directory it did not install. |
| `POLLEN_LOG_FILTER` | no | `warn` | `tracing` filter directive, e.g. `info` to follow each skill as it is installed, or `pollen=debug`. |
