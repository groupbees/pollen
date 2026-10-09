#!/usr/bin/env bash
#
# Install or upgrade pollen from its GitHub release — macOS, Linux and WSL.
#
#   install.sh [version] [install-dir]
#
#   version      a release tag (v0.4.0); default: the latest release
#   install-dir  where the binary goes; default: ~/.local/bin
#
# With no install-dir and a Homebrew-managed pollen, it upgrades through brew
# instead, so the two never fight over the binary. The archive is checked
# against the release's SHA256SUMS before anything is installed.

set -euo pipefail

repo="groupbees/pollen"
version="${1:-}"
dir="${2:-}"

die() { echo "error: $*" >&2; exit 1; }

if [[ -z "$dir" ]] && command -v brew >/dev/null && brew list --formula pollen >/dev/null 2>&1; then
  echo "pollen is installed with Homebrew: upgrading through brew."
  brew upgrade groupbees/tap/pollen || true
  pollen --version
  exit 0
fi
dir="${dir:-$HOME/.local/bin}"

if [[ -z "$version" ]]; then
  version="$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  [[ -n "$version" ]] || die "cannot read the latest release (proxy? set https_proxy)"
fi

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-musl" ;;
  *) die "unsupported system $(uname -s); on Windows use install.ps1" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64)  arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac

archive="pollen-${arch}-${os}-${version}.tar.gz"
base="https://github.com/$repo/releases/download/$version"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -fsSL -o "$work/$archive" "$base/$archive" || die "cannot download $archive"
curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS" || die "cannot download SHA256SUMS"

expected="$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$work/SHA256SUMS")"
[[ -n "$expected" ]] || die "$archive is not listed in SHA256SUMS"
if command -v sha256sum >/dev/null; then
  actual="$(sha256sum "$work/$archive" | awk '{ print $1 }')"
else
  actual="$(shasum -a 256 "$work/$archive" | awk '{ print $1 }')"
fi
[[ "$actual" == "$expected" ]] || die "checksum mismatch for $archive"

tar -xzf "$work/$archive" -C "$work"
mkdir -p "$dir"
install -m 755 "$work/${archive%.tar.gz}/pollen" "$dir/pollen"
echo "installed pollen $version in $dir"

case ":$PATH:" in
  *":$dir:"*) "$dir/pollen" --version ;;
  *) echo "note: $dir is not on PATH — add it to your shell profile, then open a new shell." ;;
esac
