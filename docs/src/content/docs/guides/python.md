---
title: Python
description: Drive pollen from Python with the pollen-skills package, which returns typed results instead of printing them.
sidebar:
  order: 4
---

The `pollen-skills` package runs pollen inside Python. Every command is a
function that returns frozen dataclasses instead of printing, and raises
`pollen.PollenError` when it fails. The engine is the CLI's own Rust code,
compiled into the package, so the behaviour is the same.

## Installation

Python ≥ 3.11 and `git` on `PATH`:

```sh
pip install pollen-skills
```

Or, in a uv project:

```sh
uv add pollen-skills
```

Wheels are published for Linux (glibc and musl), macOS and Windows, on x86_64
and arm64. On any other platform, pip builds the package from source, which
needs Rust ≥ 1.97.1. The package imports as `pollen`.

## Deploying

```python
import pollen

report = pollen.update(config="pollen.yaml")
for summary in report.summaries:
    for change in summary.changes:
        print(summary.target, change.action, change.name, change.origin)
```

`update` returns a `Report` with one `Summary` per target directory. Each
change has an `action`, a `pollen.Action` that compares equal to its value
(`"added"`, `"updated"`, `"unchanged"`, `"removed"`). `report.count(action)`
totals one action across every target:

```python
preview = pollen.update(dry_run=True)
if preview.count(pollen.Action.REMOVED):
    ...
```

## Options

Every function takes keyword arguments only, except `show`'s `repo`. These
work like the CLI's global flags:

| Argument | CLI flag | Default |
| --- | --- | --- |
| `config` | `--config` | `pollen.yaml`, from the working directory |
| `global_` | `--global` | `False`; cannot be combined with `config` |
| `targets` | `--target` | the config's `targets` |
| `cache_dir` | `--cache-dir` | the user cache directory |
| `offline` | `--offline` | `False` |

Paths can be `str` or `pathlib.Path`. The package does not read the
`POLLEN_*` [environment variables](/reference/environment-variables/): pass
the arguments instead.

## Functions

| Function | CLI equivalent | Returns |
| --- | --- | --- |
| `update(dry_run=False, force=False)` | `pollen update` | `Report` |
| `list_installed()` | `pollen list` | `list[Deployed]`, one per target |
| `validate(config_only=False, pinned=False)` | `pollen validate` | `Validation` |
| `outdated()` | `pollen outdated` | `list[SourceStatus]` |
| `autoupdate(freeze=False, dry_run=False)` | `pollen autoupdate` | `AutoupdateOutcome` |
| `show(repo, revision=None, path=None)` | `pollen show` | `Listing` |
| `schema()` | `pollen schema` | the JSON Schema, as a `str` |

`validate` checks one config file. It raises `PollenError` when the file is
invalid or a selected skill breaks the Agent Skills specification, with every
offending skill in the message. Otherwise it returns the skills the config
selects, or `skills=None` with `config_only=True`.

`outdated` and `autoupdate` ask each remote for its tags, so they take no
`offline` argument. `autoupdate` rewrites `pollen.yaml` unless `dry_run`;
`outcome.written` says whether it did.

`show` lists invalid skills with their `error` set, as the CLI does, instead
of raising.

The types are documented in the package itself: `help(pollen.Listing)`, or
your editor, since the package ships type stubs.

## Errors, logging and interrupts

A failed command raises `pollen.PollenError`, whose message is the full error
chain the CLI would print:

```python
try:
    pollen.update()
except pollen.PollenError as error:
    print(f"pollen failed: {error}")
```

pollen logs through Python's `logging`, under the `pollen` logger, with one
child logger per Rust module (`pollen.command.update`). Configure it like any
other logger, before or after the import:

```python
import logging

logging.getLogger("pollen").setLevel(logging.INFO)
```

Commands release the GIL while they run, so other threads keep going. Ctrl-C
raises `KeyboardInterrupt` and stops the command, leaving the target
directories as the CLI leaves them when interrupted.
