#!/usr/bin/env bash
#
# Regenerate schema/pollen.schema.json from the Rust config types. The
# committed copy is what editors and CI point at, so it must never drift.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

destination="schema/pollen.schema.json"
staging="$(mktemp)"
trap 'rm -f "$staging"' EXIT

cargo run --quiet -- schema >"$staging"
mkdir -p "$(dirname "$destination")"
mv "$staging" "$destination"
trap - EXIT
