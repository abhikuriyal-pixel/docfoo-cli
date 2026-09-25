#!/usr/bin/env bash
# Offline acceptance checks for the DocFoo CLI.
#
#   ./scripts/acceptance.sh
#
# Optional live check against a real workspace + model (spends API credits):
#   DOCFOO_ACCEPTANCE_WORKSPACE=/path/to/db \
#   DOCFOO_ACCEPTANCE_QUERY="How is X connected to Y?" \
#   ./scripts/acceptance.sh
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== build =="
cargo build --offline

echo "== tests =="
cargo test --offline

echo "== smoke: version envelope =="
./target/debug/docfoo version --json | grep -q '"schema": "docfoo.cli/1"'
./target/debug/docfoo help >/dev/null
./target/debug/docfoo completions bash | grep -q docfoo

echo "== smoke: fresh workspace commands =="
WS="$(mktemp -d)"
trap 'rm -rf "$WS"' EXIT
DOCFOO_MODELS_DIR="$WS/models" ./target/debug/docfoo setup --check --json --workspace "$WS" >/dev/null
./target/debug/docfoo kg --status --json --workspace "$WS" | grep -q '"exists": false'
./target/debug/docfoo resources --list --json --workspace "$WS" >/dev/null
./target/debug/docfoo notes --list --json --workspace "$WS" >/dev/null
if ./target/debug/docfoo kg --query "anything" --workspace "$WS" 2>/dev/null; then
  echo "kg --query should have failed without a graph" >&2
  exit 1
fi

if [ -n "${DOCFOO_ACCEPTANCE_WORKSPACE:-}" ] && [ -n "${DOCFOO_ACCEPTANCE_QUERY:-}" ]; then
  echo "== live KG query =="
  ./target/debug/docfoo kg --query "$DOCFOO_ACCEPTANCE_QUERY" \
    --workspace "$DOCFOO_ACCEPTANCE_WORKSPACE" --format slack --hermes-final
fi

if command -v node >/dev/null 2>&1; then
  echo "== tests: vis frontend modules (node) =="
  node --test crates/docfoo-cli/tests/js/*.test.mjs
else
  echo "== tests: vis frontend modules skipped (node not found) =="
fi

echo
echo "all acceptance checks passed"
