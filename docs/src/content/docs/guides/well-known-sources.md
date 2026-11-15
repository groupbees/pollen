---
title: Well-known sources
description: Deploy skills from a gateway or a site publishing an Agent Skills discovery index, pin them by digest, and authenticate to it.
sidebar:
  order: 3
---

A gateway, a registry or a documentation site can publish its skills at
`/.well-known/agent-skills/index.json`, as the
[Agent Skills Discovery via Well-Known URIs](https://github.com/cloudflare/agent-skills-discovery-rfc)
specification (v0.2.0) describes. LiteLLM does it for the skills stored on its
proxy, once `litellm_settings.public_skills_index` is on. A `well-known:`
entry in `pollen.yaml` deploys those skills like any other.

## Declaring the source

Take every skill of the index, leaving some out by name:

```yaml
repos:
  - well-known: https://gateway.example.com
    exclude: ^experimental-
```

Or take a list of skills, each pinned to the digest of its artifact:

```yaml
repos:
  - well-known: https://gateway.example.com
    skills:
      - name: pdf-summarizer
        digest: sha256:3f2a9c0d5b8e7f41a6c2d9e0b1f3a4c5d6e7f8091a2b3c4d5e6f708192a3b4c5
      - name: code-review
```

`well-known` is an origin — scheme, host and optional port — with no path.
pollen reads `<origin>/.well-known/agent-skills/index.json`, downloads each
selected skill, either a `SKILL.md` or a `.tar.gz` or `.zip` archive, checks
it against the index's `digest`, and deploys it. The keys are in the
[configuration reference](/reference/configuration/).

An entry pollen cannot install is skipped with a warning, and the rest of the
index deploys: a type pollen does not know, a name the
[Agent Skills specification](https://agentskills.io/specification) refuses
(such as `PDF_Summarizer`), or a digest that is not `sha256:` and 64 lowercase
hex digits. Listed under `skills`, the same entry fails every command instead,
since it was asked for by name.

An index whose `$schema` is not the v0.2.0 one, or that has none, is refused.

## Pinning

The `digest` is the SHA-256 of the artifact, so a pinned skill deploys exactly
those bytes or the run fails. The same commands as for git sources manage it:

```sh
pollen autoupdate --freeze        # write the published digest of every listed skill
pollen validate --pinned          # require a digest on every listed skill
pollen outdated                   # show the skills whose published digest moved
pollen autoupdate                 # move each pinned digest to the published one
```

`--freeze` pins only the skills listed under `skills`: an entry that takes the
whole index has nothing to pin, and `validate --pinned` refuses it.

A digest guarantees integrity, not retrievability. An index only advertises the
current artifact of each skill, so once the publisher replaces one, a pinned
config fails until `autoupdate` moves the pin, and another machine can deploy
the pinned bytes only while the publisher still serves them.

The index records no dates, so `--min-release-age` holds back every changed
digest: `outdated` reports it as having no recorded date, and `autoupdate`
leaves the pin alone.

Artifacts are cached by digest. `--offline` deploys from the cached index and
artifacts, and an unchanged skill is not downloaded again.

## Authentication

The specification defines none; gateways usually want a bearer token. pollen
reads tokens from a user-level credentials file, never from `pollen.yaml`, so a
cloned repository cannot make pollen run a command or send a token. The project
config only names the origin.

The file is `$XDG_CONFIG_HOME/pollen/credentials.yaml` —
`~/.config/pollen/credentials.yaml` when the variable is unset, on macOS too,
and `%APPDATA%\pollen\credentials.yaml` on Windows. `--credentials-file`
(`POLLEN_CREDENTIALS_FILE`) names another one, which must then exist.

```yaml
credentials:
  # A credential helper: a command that prints a bearer token on stdout
  - origin: https://gateway.example.com
    command:
      - my-sso-cli
      - token
      - --audience
      - skills

  # A token read from an environment variable, e.g. in CI
  - origin: https://skills.example.org
    env: SKILLS_TOKEN
```

- Each entry takes either `command` or `env`. The token is sent as
  `Authorization: Bearer <token>`.
- The origin must match exactly: scheme, host and port. The token is never
  sent to an artifact hosted on another origin, nor across a redirect to one.
- `command` is an argument list, run without a shell, at most once per run and
  only when a source on that origin is used. Its standard input and error stay
  on the terminal, so the helper can run an interactive sign-in, in the
  browser or with a device code, and keep its own token cache. pollen keeps
  the token in memory for the run, and never stores or logs it.
- A failing helper, an empty token, an unset variable, or a `401` or `403`
  answer fails the run, naming the origin. pollen does not retry or refresh.
- `validate --config-only`, which the pre-commit hook runs, fetches nothing
  and never runs a helper.

## What changes in the deployed skill

pollen copies regular files only. A link inside an archive is left out, and
one pointing outside the skill refuses the archive, as do absolute paths, `..`
components and an archive unpacking to more than 256 MiB.

File modes come from the archive. A publisher that repacks skills without them
— LiteLLM writes every zip entry as `0644` — deploys scripts without the
executable bit. A skill that relies on executable scripts is better served from
its git repository.
