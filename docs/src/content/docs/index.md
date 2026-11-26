---
title: pollen
description: Deploy Agent Skills from git repositories, local directories and well-known indexes, declaratively, from a single pollen.yaml.
---

`pollen` makes your skills directories match a `pollen.yaml`: it installs the
[Agent Skills](https://agentskills.io) the file declares, refreshes what moved
and removes what you dropped, for Claude Code and every other Agent Skills
client. It deploys agent definitions the same way, as is, into the agent
directories of Claude Code and OpenCode.

- [Installation](/guides/installation/): Homebrew, release archives, the
  Python package.
- [Configuration file](/reference/configuration/): the sources to deploy from
  and where the skills and agents go.
- [Usage](/guides/usage/): `pollen update` and the commands that preview,
  validate, pin and upgrade.
- [Well-known sources](/guides/well-known-sources/): skills served by a
  discovery index, such as a LiteLLM gateway.
- [Pre-commit hook](/guides/pre-commit-hook/): validate a `pollen.yaml` on
  every commit.
- [Python](/guides/python/): the same engine, as a Python API.

The design and the reasoning behind it are in
[ARCHITECTURE.md](https://github.com/groupbees/pollen/blob/main/ARCHITECTURE.md).
