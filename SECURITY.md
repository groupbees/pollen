# Security policy

## Reporting a vulnerability

Report a vulnerability privately, with the **Report a vulnerability** button
of the repository's
[Security tab](https://github.com/groupbees/pollen/security/advisories/new).
Never open a public issue or pull request for it.

Include the version (`pollen --version`), the operating system, a
`pollen.yaml` that reproduces the problem, and what an attacker gains.

## Supported versions

pollen is a 0.x project: only the latest release receives security fixes. It
is published as GitHub Releases archives, as the `pollen-skills` package on
PyPI and through the `groupbees/tap` Homebrew tap, all built from the same
`v*` tag.

## Scope

In scope, because they are where pollen handles untrusted input or secrets:

- the credentials file and the helper commands it names, and any path where
  a token is written, logged or sent to an origin other than the one the
  credentials file names, redirects included;
- HTTP requests to `well-known` origins: token handling, redirects, digest
  verification, bounds on what is downloaded;
- unpacking an archive: anything that writes outside the skill directory, or
  bypasses the limits on entries and unpacked size;
- writing to the target directories: replacing or removing anything not
  recorded in `.pollen.json`;
- resolving a revision to a commit other than the one asked for.

Out of scope: the content of a skill you chose to deploy, a malicious
`pollen.yaml` you chose to run (it names sources and paths, never commands),
and vulnerabilities in a dependency with no way to reach them through pollen
(report those upstream).

## What happens next

You get an acknowledgement within a few days. The report is confirmed or
declined with reasons, a fix is prepared in a private fork, and the fix ships
in a release published with a GitHub security advisory that credits the
reporter unless they prefer not to be named.
