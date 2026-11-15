#!/usr/bin/env bash
#
# Check pollen against a real gateway: start the LiteLLM proxy of
# tests/litellm/docker-compose.yaml, upload the skills under
# tests/litellm/skills/, and deploy, pin and re-check them through its
# `well-known` index. Needs `docker` with Compose, `curl` and `zip`.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

port="${POLLEN_LITELLM_PORT:-4000}"
url="http://127.0.0.1:${port}"
key="sk-pollen-test"
compose=(docker compose --project-name pollen-litellm --file tests/litellm/docker-compose.yaml)

work="$(mktemp -d)"
cleanup() {
  "${compose[@]}" down --volumes >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

export POLLEN_LITELLM_PORT="$port"
"${compose[@]}" up --detach --quiet-pull

# The proxy migrates its database before it listens; that takes a while.
for _ in $(seq 1 120); do
  curl --silent --fail "${url}/health/liveliness" >/dev/null && break
  sleep 2
done
curl --silent --show-error --fail "${url}/health/liveliness" >/dev/null

for skill in tests/litellm/skills/*/; do
  name="$(basename "$skill")"
  # The proxy takes a zip whose files sit under one top-level folder.
  (cd tests/litellm/skills && zip --quiet --recurse-paths "${work}/${name}.zip" "$name")
  curl --silent --show-error --fail \
    --header "Authorization: Bearer ${key}" \
    --form "display_title=${name}" \
    --form "files[]=@${work}/${name}.zip" \
    "${url}/v1/skills?beta=true&custom_llm_provider=litellm_proxy" >/dev/null
done

# The index is public; the token checks that pollen sends it without harm.
cat >"${work}/credentials.yaml" <<EOF
credentials:
  - origin: ${url}
    env: POLLEN_LITELLM_TOKEN
EOF
cat >"${work}/pollen.yaml" <<EOF
repos:
  - well-known: ${url}
    skills:
      - name: pdf-summarizer
EOF

pollen() {
  POLLEN_LITELLM_TOKEN="$key" cargo run --quiet --package pollen -- \
    --config "${work}/pollen.yaml" \
    --cache-dir "${work}/cache" \
    --credentials-file "${work}/credentials.yaml" \
    --target "${work}/deployed" \
    "$@"
}

pollen update
diff --recursive tests/litellm/skills/pdf-summarizer "${work}/deployed/pdf-summarizer"
if [ -e "${work}/deployed/experimental-draft" ]; then
  echo "a skill the config does not list was deployed" >&2
  exit 1
fi

pollen autoupdate --freeze
grep --quiet 'digest: sha256:' "${work}/pollen.yaml" || {
  echo "autoupdate --freeze wrote no digest" >&2
  exit 1
}
pollen validate --pinned
pollen outdated | grep --quiet 'pdf-summarizer.*up to date' || {
  echo "outdated does not see the frozen digest as current" >&2
  exit 1
}
pollen --offline update | grep --quiet '1 unchanged'

cat >"${work}/pollen.yaml" <<EOF
repos:
  - well-known: ${url}
    exclude: ^experimental-
EOF
pollen update | grep --quiet '1 skill in'

echo "litellm: ok"
