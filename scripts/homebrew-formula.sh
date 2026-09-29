#!/usr/bin/env bash
#
# Print the Homebrew formula for a pollen release, from the release's
# SHA256SUMS. The release workflow pushes the result to groupbees/homebrew-tap.

set -euo pipefail

REPOSITORY="https://github.com/groupbees/pollen"

usage() {
  cat <<'EOF'
Usage: scripts/homebrew-formula.sh <version> <SHA256SUMS>

Print the Homebrew formula for release <version> on stdout, pointing at that
release's macOS and Linux archives and pinning them to the checksums listed in
<SHA256SUMS>.

Arguments:
  <version>     the released version, without the leading `v` (e.g. 0.2.0)
  <SHA256SUMS>  the release's checksum file

Example:
  scripts/homebrew-formula.sh 0.2.0 artifacts/SHA256SUMS > pollen.rb
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
esac

[ $# -eq 2 ] || {
  usage >&2
  die "a version and a checksum file are required"
}

version="$1"
sums="$2"

echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' ||
  die "\`$version\` is not a stable semver version"
[ -r "$sums" ] || die "cannot read ${sums}"

archive() {
  echo "pollen-$1-v${version}.tar.gz"
}

checksum() {
  local name
  name="$(archive "$1")"
  awk -v name="$name" '$2 == name || $2 == "*" name { print $1; found = 1 } END { exit !found }' "$sums" ||
    die "${sums} has no line for ${name}"
}

url() {
  echo "${REPOSITORY}/releases/download/v${version}/$(archive "$1")"
}

# Resolved up front: a failed lookup inside the heredoc would not stop the script.
mac_arm="$(checksum aarch64-apple-darwin)"
mac_intel="$(checksum x86_64-apple-darwin)"
linux_arm="$(checksum aarch64-unknown-linux-musl)"
linux_intel="$(checksum x86_64-unknown-linux-musl)"

cat <<EOF
class Pollen < Formula
  desc "Deploy Agent Skills from git repositories and local directories, declaratively"
  homepage "https://groupbees.github.io/pollen/"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "$(url aarch64-apple-darwin)"
      sha256 "${mac_arm}"
    end
    on_intel do
      url "$(url x86_64-apple-darwin)"
      sha256 "${mac_intel}"
    end
  end

  on_linux do
    on_arm do
      url "$(url aarch64-unknown-linux-musl)"
      sha256 "${linux_arm}"
    end
    on_intel do
      url "$(url x86_64-unknown-linux-musl)"
      sha256 "${linux_intel}"
    end
  end

  def install
    bin.install "pollen"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/pollen --version")
  end
end
EOF
