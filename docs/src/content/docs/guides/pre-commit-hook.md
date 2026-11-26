---
title: Pre-commit hook
description: Validate a pollen.yaml on every commit with the published pre-commit hook.
sidebar:
  order: 4
---

`pollen` publishes a hook, so a repository that carries a `pollen.yaml`
can keep it honest. In that repository's `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/groupbees/pollen
    rev: <sha>  # vX.Y.Z
    hooks:
      - id: pollen-validate
```

Pin `rev` to the commit of the latest release, with its tag in a comment:
`pre-commit autoupdate --freeze` writes both. The hook reads only the
[format versions](/reference/configuration/#format-versions) its release
knows, so it refuses a `pollen.yaml` written in a newer one.

The hook runs on every `pollen.yaml` a commit touches and checks that it
parses and its rules hold. It fetches nothing, so
it stays fast and works offline — and therefore says nothing about the skills
those sources would yield. Run [`pollen validate`](/guides/usage/) for that, in CI or by hand.

To require every git source to be pinned to a commit, pass `--pinned`:

```yaml
      - id: pollen-validate
        args:
          - --pinned
```
