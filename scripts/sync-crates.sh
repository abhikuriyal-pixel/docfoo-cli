#!/usr/bin/env bash
# Sync the vendored crates from a DocFoo (desktop) checkout.
#
# The CLI keeps its own copy of docfoo-kg and docfoo-ocr so a plain clone
# builds without the desktop repo. This script is the one-way sync: the
# desktop repo is upstream, the CLI copies are read-only mirrors.
#
#   ./scripts/sync-crates.sh                       # copy from ../DocFoo
#   DOCFOO_UPSTREAM=/path/to/DocFoo ./scripts/sync-crates.sh
#   ./scripts/sync-crates.sh --check               # exit 1 when out of sync
set -euo pipefail
cd "$(dirname "$0")/.."

UPSTREAM="${DOCFOO_UPSTREAM:-../DocFoo}"
CRATES=(docfoo-kg docfoo-ocr)
CHECK=0
if [ "${1:-}" = "--check" ]; then
  CHECK=1
elif [ $# -gt 0 ]; then
  echo "usage: $0 [--check]" >&2
  exit 2
fi

for crate in "${CRATES[@]}"; do
  src="$UPSTREAM/crates/$crate"
  dst="crates/$crate"
  if [ ! -d "$src" ]; then
    echo "upstream crate not found: $src" >&2
    exit 1
  fi
  if [ "$CHECK" = 1 ]; then
    if ! diff -r --exclude=target "$src" "$dst" >/dev/null 2>&1; then
      echo "crates/$crate is out of sync with $src" >&2
      diff -r --exclude=target "$src" "$dst" | head -20 >&2 || true
      exit 1
    fi
    echo "crates/$crate is in sync"
  else
    rm -rf "$dst"
    mkdir -p "$(dirname "$dst")"
    cp -r "$src" "$dst"
    echo "synced crates/$crate from $src"
  fi
done
