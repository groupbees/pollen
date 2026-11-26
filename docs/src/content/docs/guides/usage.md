---
title: Usage
description: Deploy, preview, validate, list and inspect skills and agents with the pollen commands.
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

`update` prints one block per directory it writes or prunes, and under each
agent what its frontmatter grants — tools, model, permission, and whether it
declares hooks or MCP servers — since that is what an agent is allowed to do:

```console
$ pollen update
+ review-guide  local skills/review-guide
1 skill in .claude/skills: 1 added

+ planner   local agents/planner.md
            model: sonnet
+ reviewer  local agents/reviewer.md
            tools: Read, Grep, Glob; model: opus; permission: plan; hooks
2 agents in .claude/agents: 2 added
```

Check the config and every skill and agent it selects, without deploying:

```sh
pollen validate
```

A skill is invalid when its frontmatter breaks the specification, or when one
of its text files holds a character an agent reads but a reviewer does not
see: zero-width and bidirectional controls, tag characters, stray variation
selectors, fillers and terminal control codes. Such characters are how a skill
smuggles in instructions nobody approved, so `update`, `validate` and `show`
refuse it, naming the file and where the first one sits. Emoji sequences,
Persian or Indic joiners and a byte order mark opening a file are allowed.
An agent file is checked the same way, and is invalid too when it breaks the
rules of a client it is deployed for (see
[Agents](/reference/configuration/#agents)).

For a source you trust, `--allow-hidden-characters` makes `update` and
`validate` accept such a skill or agent, logging a warning for each file. The flag
lowers the check for every source; when one skill is at fault, `exclude` it
instead.

A repository that publishes skills can run the same check in GitHub Actions
with [groupbees/skill-action](https://github.com/groupbees/skill-action), which
also lints every skill with
[skill-validator](https://github.com/agent-ecosystem/skill-validator).

Check config files alone, fetching nothing:

```sh
pollen validate --config-only pollen.yaml
```

Require every git source to be pinned to a full commit SHA, offline:

```sh
pollen validate --config-only --pinned
```

A branch moves on every push and a tag can be moved; a commit cannot. Pin the
commit and keep the tag readable in a comment, which a bot such as Renovate can
keep up to date:

```yaml
revision: acad0f52027cf8f8edf7bfa6a55e13c594d8ee71  # v0.1.0
```

Whatever the flags, `update` and `validate` warn when a revision is a branch.
A `discovery` origin is pinned by listing each skill with its `digest`; see
[Well-known sources](/guides/well-known-sources/#pinning).

See which sources have a newer release, from the remotes' tags, and which
pinned `discovery` skills a newer digest, from their indexes:

```sh
pollen outdated
```

Move every source to its newest release, rewriting only the `revision:` lines
of `pollen.yaml` — comments and layout are kept, and the file is written only
if it still validates:

```sh
pollen autoupdate --dry-run
pollen autoupdate
```

`--freeze` pins each source to the commit of its release instead, with the
release in a comment (`revision: <sha>  # v0.2.0`); run on an up-to-date tag,
it only freezes it. Only stable `vX.Y.Z` or `X.Y.Z` tags count as releases,
and a branch revision is left alone.

Leave out releases younger than a minimum age, so that a broken or compromised
release has time to be found and withdrawn before it reaches you:

```sh
pollen outdated --min-release-age 7d
pollen autoupdate --freeze --min-release-age 7d
```

Each source then moves to the newest release at least that old, and
`outdated` names the newest one it passed over (`v0.3.0 is 2 days old`). The
age is whole days or hours (`7d`, `12h`); `0d` turns it off. A source already
pinned to a release keeps it, whatever its age. A source pinned to a commit that
no release names moves to the newest release old enough, even when that release
is older than the commit.

The age comes from the tag: the tagger date of an annotated tag, the committer
date of a lightweight one. Whoever makes the tag sets that date, so a
backdated tag passes. To read it, pollen fetches each candidate tag one commit
deep into the cache.

See what a git source provides before adding it to the config — each skill's
path, in the form a `repo` entry's `skills:` takes, its description, and
the commit to pin:

```sh
pollen show https://github.com/groupbees/skills-opensource
pollen show git@github.com:org/skills.git --revision v0.2.0 --path skills/docker
pollen show https://github.com/groupbees/pollen --format json
```

It reads the newest release unless `--revision` names another (`HEAD` when the
source has none), lists invalid skills with their error instead of stopping,
and writes nothing. With no config to say which client a directory of agents
is written for, `--agents CLIENT:PATH` names it: the agent files directly under
`PATH` are listed, checked against `CLIENT`'s rules, each with what its
frontmatter grants:

```console
$ pollen show https://github.com/acme/agents --agents claude-code:claude
https://github.com/acme/agents @ v1.4.0 (06fcca9e)
  claude/docs.md      invalid agent: for claude-code, the frontmatter needs `name`, a non-empty string
  claude/planner.md   claude-code agent: Plans work.
                      model: sonnet
  claude/reviewer.md  claude-code agent: Reviews code.
                      tools: Read, Grep, Glob; model: opus; permission: plan; hooks
0 skill(s); 2 agent(s), 1 invalid
pin with: revision: 06fcca9e695383cad0f846abe1740f6392190683  # v1.4.0
```

List what is currently deployed, skills and agents:

```sh
pollen list
```

Print the JSON Schema for the config file:

```sh
pollen schema
```

Deploy somewhere else for this run only, into directories of your own in
place of every entry's `clients` or `targets`, or for some clients only:
`--client` deploys the skills and agents the config sends to those clients
and nothing else, so an entry sent to other clients or to `targets` is left
out. Such a run visits no other directory, so it prunes nothing elsewhere.
`--target` deploys skills only, so a run into a scratch directory never
touches an agent directory:

```sh
pollen update --target ~/.claude/skills --target ~/.agents/skills
pollen update --client claude-code
```

Deploy the skills you want in every project, from your user-level config:

```sh
pollen update --global
```

`--global` (`-g`) reads `$XDG_CONFIG_HOME/pollen/pollen.yaml` —
`~/.config/pollen/pollen.yaml` when the variable is unset, on macOS too, and
`%APPDATA%\pollen\pollen.yaml` on Windows — and takes the skill directories
of its clients under the home directory, so an entry naming no `targets`
deploys into `~/.claude/skills` and `~/.agents/skills` by default. Agents go
to `~/.claude/agents` and `$XDG_CONFIG_HOME/opencode/agents`, or
`~/.config/opencode/agents` when the variable is unset or empty — on Windows
too, where OpenCode follows the same layout. It works
with every command (`pollen list -g`, `pollen validate -g`) and cannot be
combined with `--config`.

A target directory belongs to one config: an update prunes whatever its config
no longer declares, so a second config deploying into the same directory would
remove the first one's skills or agents. `pollen update` therefore refuses a directory
another config manages and names that config; declare the sources there, or
pass `--force` to hand the directory over.

Managed names in `.pollen.json`, skills and agents alike, follow the same
rules as skill names. A path such
as `../outside` refuses that target before installing, pruning or sweeping
staging material. Preserve the state file and review the invalid record before
another attempt. Neither `--force` nor `--dry-run` bypasses this validation.

Give your agents the pollen skill, so they edit `pollen.yaml`, pin sources and
fix pollen's errors the way this guide describes — add it as a source:

```yaml
  - repo: https://github.com/groupbees/pollen
    revision: <sha>  # vX.Y.Z
    skills:
      - path: skills
```

Rewrite a config written in an older [format version](/reference/configuration/#format-versions)
in the newest one, keeping its comments; `--dry-run` shows what it would do:

```sh
pollen config migrate
```

Refresh from the cache on a plane:

```sh
pollen update --offline
```

The result goes to stdout and the diagnostics to stderr, so
`pollen update > changes.txt` keeps both readable. Only warnings and errors
reach stderr by default; `--log-filter info` also logs each skill as it is
installed or removed, and `--log-format json` writes each log line as a JSON
object, for a pipeline that parses them. The final `Error: …` message of a
failed run stays plain text. Every flag also has an
[environment variable](/reference/environment-variables/).
