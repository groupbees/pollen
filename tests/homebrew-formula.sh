#!/usr/bin/env bash
#
# Check scripts/homebrew-formula.sh against a fabricated SHA256SUMS: the
# formula it prints passes `brew style` and `brew audit` and pins every
# archive, and a missing archive is refused. Needs `brew` on PATH.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Homebrew only lints formulae that live in a tap, so this one gets a
# throwaway local tap.
TAP="pollen-test/formula-check"

work="$(mktemp -d)"
cleanup() {
  brew untap --force "$TAP" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

version="1.2.3"
targets=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  aarch64-unknown-linux-musl
  x86_64-unknown-linux-musl
  x86_64-pc-windows-msvc
)

for target in "${targets[@]}"; do
  printf '%064x  pollen-%s-v%s.tar.gz\n' "${#target}" "$target" "$version"
done >"$work/SHA256SUMS"

scripts/homebrew-formula.sh "$version" "$work/SHA256SUMS" >"$work/pollen.rb"

for target in "${targets[@]:0:4}"; do
  grep -q "releases/download/v${version}/pollen-${target}-v${version}.tar.gz" "$work/pollen.rb" || {
    echo "the formula has no URL for ${target}" >&2
    exit 1
  }
done

brew tap-new --no-git "$TAP" >/dev/null
cp "$work/pollen.rb" "$(brew --repository "$TAP")/Formula/pollen.rb"
brew style "$TAP/pollen"
brew audit --strict --formula "$TAP/pollen"

grep -v aarch64-apple-darwin "$work/SHA256SUMS" >"$work/SHA256SUMS.partial"
if scripts/homebrew-formula.sh "$version" "$work/SHA256SUMS.partial" >/dev/null 2>&1; then
  echo "a SHA256SUMS missing an archive was accepted" >&2
  exit 1
fi

echo "homebrew formula: ok"
