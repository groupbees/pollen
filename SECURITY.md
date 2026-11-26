# Security policy

## Reporting a vulnerability

Report a vulnerability privately, with the **Report a vulnerability** button
of the repository's
[Security tab](https://github.com/groupbees/pollen/security/advisories/new).
Never open a public issue or pull request for it.

Include the version (`pollen --version`), the operating system, a
`pollen.yaml` that reproduces the problem, and what an attacker gains.

## Supported versions

Fixes ship in a new release, and only the latest release receives security
fixes. pollen is published as GitHub Releases archives, as the
`pollen-skills` package on PyPI and through the `groupbees/tap` Homebrew tap,
all built from the same `v*` tag.

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

Out of scope:

- the content of a skill you chose to deploy;
- a malicious `pollen.yaml` you chose to run: it names sources and paths,
  never commands;
- a vulnerability in a dependency with no way to reach it through pollen:
  report it upstream;
- a finding that needs an already-compromised host or account;
- scanner output with no path to exploitation.

## What happens next

1. You get an acknowledgement within a few days.
2. The report is confirmed or declined with reasons.
3. A fix is prepared in the advisory's private fork.
4. The fix ships in a release whose notes cite the advisory's GHSA identifier,
   and its CVE when there is one.
5. The GitHub security advisory is published, with a CVE requested through
   GitHub when the impact warrants one, and credits the reporter unless they
   prefer not to be named.
