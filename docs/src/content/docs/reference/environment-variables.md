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
| `POLLEN_FREEZE` | no | `false` | Make `autoupdate` pin each source to the commit of its release, and each listed `well-known` skill to its digest, as `--freeze` does. |
| `POLLEN_MIN_RELEASE_AGE` | no | — | Make `outdated` and `autoupdate` leave out releases younger than this age, in whole days or hours (`7d`, `12h`), as `--min-release-age` does. |
| `POLLEN_GLOBAL` | no | `false` | Use the user-level config and the user-level skill directories, as `--global` does. |
| `POLLEN_PINNED` | no | `false` | Make `validate` require a full commit SHA on every git source and a digest on every `well-known` skill, as `--pinned` does. |
| `POLLEN_SKILLS_DIR` | no | — | Target directories, in place of every entry's `clients` or `targets`, separated like `PATH`: `;` on Windows, `:` elsewhere. |
| `POLLEN_CLIENTS` | no | — | Clients to deploy for, keeping only what the config's entries send to them, comma-separated: `claude-code`, `opencode` or `all`, as `--client` does. Forbidden with `POLLEN_SKILLS_DIR`. |
| `POLLEN_CACHE_DIR` | no | `<cache>/pollen` | Where the git checkouts, discovery indexes and skill artifacts are kept between runs. |
| `POLLEN_OFFLINE` | no | `false` | Work from the cache only, contacting no remote. |
| `POLLEN_CREDENTIALS_FILE` | no | `<config>/pollen/credentials.yaml` | The [credentials file](/guides/well-known-sources/#authentication) for `well-known` origins, as `--credentials-file` does. A file named here must exist. |
| `POLLEN_DRY_RUN` | no | `false` | Report what `update`, `autoupdate` or `config migrate` would change, and stop. |
| `POLLEN_FORCE` | no | `false` | Let `update` take over a directory it did not install. |
| `POLLEN_ALLOW_HIDDEN_CHARACTERS` | no | `false` | Let `update` and `validate` accept a skill whose files hold invisible characters, with a warning for each file. |
| `POLLEN_LOG_FILTER` | no | `warn` | `tracing` filter directive, e.g. `info` to follow each skill as it is installed, or `pollen=debug`. |
| `POLLEN_LOG_FORMAT` | no | `text` | Format of the logs on stderr, `text` or `json` (one object per line), as `--log-format` does. |
| `POLLEN_REVISION` | no | newest release | The tag, branch or commit `show` reads, as `--revision` does. |
| `POLLEN_PATH` | no | — | Directory of the source `show` is limited to, as `--path` does. |
| `POLLEN_FORMAT` | no | `text` | What `show` prints, `text` or `json`, as `--format` does. |
