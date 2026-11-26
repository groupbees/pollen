#!/usr/bin/env bash
#
# Check the refusals of scripts/release.sh around an existing tag, against a
# throwaway repository and a local bare origin. Always a dry run.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
script="$PWD/scripts/release.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Keeps the developer's signing, hooks and url rewrites out of the fixture.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.com

# A dry run only asks cargo whether set-version exists.
mkdir "$work/bin"
printf '#!/bin/sh\nexit 0\n' >"$work/bin/cargo"
chmod +x "$work/bin/cargo"
export PATH="$work/bin:$PATH"

git init --quiet --bare --initial-branch=main "$work/origin.git"
git init --quiet --initial-branch=main "$work/repo"
cd "$work/repo"
printf '[workspace.package]\nversion = "0.1.0"\n' >Cargo.toml
git add Cargo.toml
git commit --quiet --message init
git remote add origin "$work/origin.git"
git push --quiet origin main

fail() {
  echo "FAIL: $1" >&2
  cat "$work/output" >&2
  exit 1
}

refuses() {
  if "$script" --dry-run "$1" >"$work/output" 2>&1; then
    fail "$1 was accepted, expected: $2"
  fi
  grep -qF "$2" "$work/output" || fail "$1 was refused without: $2"
}

"$script" --dry-run 0.2.0 >"$work/output" 2>&1 || fail "a free tag was refused"

git tag v0.3.0
refuses 0.3.0 "the tag v0.3.0 already exists"

git --git-dir="$work/origin.git" tag v0.4.0 main
refuses 0.4.0 "the tag v0.4.0 already exists on origin"

# The fetch has to succeed and only the tag listing fail.
real_git="$(command -v git)"
cat >"$work/bin/git" <<STUB
#!/bin/sh
[ "\$1" = ls-remote ] && exit 128
exec "$real_git" "\$@"
STUB
chmod +x "$work/bin/git"
refuses 0.5.0 "cannot list the tags on origin"

echo "scripts/release.sh refuses the tags it should."
