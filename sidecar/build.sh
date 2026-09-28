#!/usr/bin/env bash
# Build the DocFoo CLI sidecar with bun.
# Usage: ./build.sh [output]   (default: docfoo-agent in this directory)
# Cross-compile with DOCFOO_SIDECAR_TARGET, e.g. bun-linux-x64 or bun-windows-x64.
set -euo pipefail
cd "$(dirname "$0")"

command -v bun >/dev/null || { echo "bun is required. Install it with: npm i -g bun"; exit 1; }

if [ ! -d node_modules ]; then
  bun install
fi
node scripts/dedupe-pi-ai.mjs || true

OUT="${1:-docfoo-agent}"
ARGS=(build --compile --minify --bytecode ./main.ts --outfile "$OUT")
if [ -n "${DOCFOO_SIDECAR_TARGET:-}" ]; then
  ARGS+=(--target "$DOCFOO_SIDECAR_TARGET")
fi
echo "Building sidecar: $OUT"
bun "${ARGS[@]}"
echo "Done."
