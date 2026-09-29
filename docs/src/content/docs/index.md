---
title: pollen
description: Deploy Agent Skills from git repositories and local directories, declaratively, from a single pollen.yaml.
---

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

Start with the [installation](/guides/installation/), describe your sources
in a [configuration file](/reference/configuration/), then
[run `pollen update`](/guides/usage/). The design and the reasoning behind it
are in
[ARCHITECTURE.md](https://github.com/groupbees/pollen/blob/main/ARCHITECTURE.md).
