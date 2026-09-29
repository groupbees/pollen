---
title: Installation
description: Install the pollen binary with Homebrew, from a release archive or from source.
sidebar:
  order: 1
---

## Prerequisites

- `git`, on `PATH` — `pollen` shells out to it for every remote source.
- Rust ≥ 1.97.1, only to build from source.

## Homebrew

On macOS and Linux:

```sh
brew install groupbees/tap/pollen
```

The formula installs the release archive for your platform, so nothing is
compiled; `brew upgrade` picks up each new release.

## Release archives

Download the archive for your platform from the
[releases](https://github.com/groupbees/pollen/releases) — macOS on
Apple Silicon and Intel, Linux on x86_64 and arm64 as `.tar.gz`, Windows on
x86_64 and arm64 as `.zip`. The Linux binaries are statically linked, so they
run on any distribution; the Windows ones need no Visual C++ runtime. Unpack
the archive and put `pollen` (`pollen.exe` on Windows) on your `PATH`.
`SHA256SUMS` on the same page lists the archives' checksums:

```sh
sha256sum --check --ignore-missing SHA256SUMS
```

On Windows, compare the output of this PowerShell command with the archive's
line in `SHA256SUMS`:

```powershell
Get-FileHash -Algorithm SHA256 pollen-x86_64-pc-windows-msvc-v*.zip
```

Or build it from source:

```sh
cargo install --git https://github.com/groupbees/pollen
```
