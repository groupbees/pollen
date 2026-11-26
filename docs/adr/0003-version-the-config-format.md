# 0003. Version the config format, and parse it apart from running it

- **Status**: accepted
- **Date**: 2026-11-25

## Context

`pollen.yaml` has had one shape since the first release: a `repo` entry lists
where to look under `paths`, a `well-known` entry lists skills by name under
`skills`. Deploying subagent definitions beside skills
([#84](https://github.com/groupbees/pollen/issues/84)) wants a source to list
`skills:` and `agents:` side by side, which renames `paths`. That is the first
breaking change the file would take, and not the last one a young format
will want.

Breaking every config on upgrade is not an option for a file that sits in
many repositories and is read by whatever pollen each machine and CI runner
has installed. Neither is keeping every old key forever as an alias: the
parser, the schema and the docs would then describe two names for one thing
with no end date.

The code also had one type, `config::Config`, that was at once the serde
shape, the validated value and what every command read, so a change of
shape reached every command.

## Decision

A file declares the version of the format it is written in with a
top-level `version` key, an integer. A file without one is in version 1, so
every existing config keeps its meaning without being touched. pollen reads
every version up to the newest it knows, and refuses a newer one with a
message saying to upgrade pollen.

Version 2 changes two things. A `repo` entry lists where to look under
`skills`, as a `well-known` entry already lists its skills. And the origins
serving a discovery index leave `repos` for a list of their own,
`discovery`, each naming its origin under `url`: the two kinds of sources
share no key but `skills`, and that only by name, so one list for both had
every key forbidden on one kind or the other. With a list each, a key that
does not belong to a kind of source is an unknown key, which the schema and
the parser reject without a rule of their own, and `repos` means what it
says. Folding origins into `repo` instead would need a second key to tell an
index from a git remote served over HTTPS, or a guess.

Parsing is kept apart from running. `config::raw` holds one set of serde
types per version, each owning its JSON Schema and its validation in its own
vocabulary, so an error names the keys the file uses. A validated file is
converted into the model every command runs on, `config::Config` and the
`config::Repo` enum, an older version through each newer one
(`V1 -> V2 -> Config`). There is one model however many versions are read,
and a combination version 1 allows but means nothing, such as `paths` on a
`well-known` source, cannot be represented in it.

`pollen config migrate` rewrites a file in the newest version, line by line
as `autoupdate` does, so comments and layout survive, and writes it only if
the result loads to the same sources and targets. To version 2, it renames
each `paths:`, and moves each `well-known` entry, with the comments above
it, to a `discovery:` list after `repos:`. It sits under `config`,
with the commands that work on the file rather than on the skills, as
`buf config migrate` does.

The version moves only when a key is renamed or removed, or changes
meaning. A new optional key, such as `agents:`, is added to the current
version without moving it.

An old version is deprecated, with a warning pointing to
`pollen config migrate`, in a release after the one that introduced its
successor, and dropped only in a later release, never in the same one.

## Consequences

- No config breaks on upgrade. A config written in a newer version than the
  installed pollen fails with a message to upgrade rather than with an
  unknown key.
- The published JSON Schema covers every version it reads, choosing on
  `version`, so an editor pointed at it keeps validating version 1 files.
- Each new version costs one set of raw types, one conversion and the
  migration rewrite. Commands do not change with the shape of the file.
- Two versions are live at once for a while: the docs describe the newest
  one, with the difference spelled out, and both have tests.
- Teams adopting version 2 need pollen 0.8 or later everywhere the config is
  read, CI and pre-commit hooks included.

## Alternatives considered

- **No version, renamed keys kept as aliases.** No new key, but every alias
  stays in the parser and the schema with no point at which it can go, and
  nothing tells a reader which form is current.
- **A minimum pollen version in the config (`requires-pollen: ">=0.8"`)**, as
  Terraform's `required_version` or pre-commit's
  `minimum_pre_commit_version`. It gives a clear message to an old pollen,
  which `version` now also gives, but no way to change a key.
- **A version that moves with every schema change**, as docker-compose's
  `version:` did. Users ended up copying a number whose meaning they did not
  know, and Compose now ignores it. Moving only on a breaking change, rarely,
  with a migration command, is what golangci-lint (`version: "2"`,
  `golangci-lint migrate`) and buf (`version: v2`, `buf config migrate`) do.
- **`repo:` with a `type: well-known` key for every source.** One key name
  for all sources, at the cost of a second key and a migration for every
  `well-known` entry, for no new capability.
- **`well-known` entries kept in `repos`**, the shape of version 1, with
  `skills` on both kinds. No entry moves on migration, but every key of an
  entry stays forbidden on one kind of source or the other, and `repos`
  keeps holding origins that are not repositories.
- **Re-serialising the YAML to migrate it.** Simpler, but drops every
  comment, including the `# vX.Y.Z` next to a pinned commit.
