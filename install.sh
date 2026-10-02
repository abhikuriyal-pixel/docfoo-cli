#!/usr/bin/env bash
# Install docfoo + docfoo-agent on Linux/WSL x64.
#
#   ./install.sh                 # download the latest release
#   ./install.sh --local         # build from this checkout
#   ./install.sh --skip-setup    # don't run `docfoo setup`
#   ./install.sh --prefix DIR    # install somewhere else (default ~/.local/bin)
#   ./install.sh --version 0.2.0 # install a specific release
set -euo pipefail

REPO="${DOCFOO_REPO:-abhikuriyal-pixel/docfoo-cli-releases}"
PREFIX="${DOCFOO_PREFIX:-$HOME/.local/bin}"

# The release repo is public, so no token is needed. GITHUB_TOKEN/GH_TOKEN are
# still honored if DOCFOO_REPO points at a private repository.
AUTH=()
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
if [ -n "$TOKEN" ]; then
  AUTH=(-H "Authorization: Bearer $TOKEN")
fi
VERSION="${DOCFOO_VERSION:-latest}"
LOCAL=0
SKIP_SETUP=0

usage() {
  sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    --local) LOCAL=1 ;;
    --skip-setup) SKIP_SETUP=1 ;;
    --prefix) PREFIX="${2:?--prefix needs a directory}"; shift ;;
    --version) VERSION="${2:?--version needs a value}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

case "$(uname -s)" in
  Linux) ;;
  *) echo "install.sh supports Linux/WSL; use the Windows zip from the releases page instead." >&2; exit 1 ;;
esac

mkdir -p "$PREFIX"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if [ "$LOCAL" = 1 ]; then
  command -v cargo >/dev/null || { echo "cargo is required for --local" >&2; exit 1; }
  echo "building docfoo (release)…"
  cargo build --release
  install -m 0755 target/release/docfoo "$PREFIX/docfoo"
  echo "building the sidecar…"
  command -v bun >/dev/null || { echo "bun is required to build the sidecar (npm i -g bun)" >&2; exit 1; }
  ( cd sidecar && { [ -d node_modules ] || bun install; } && ./build.sh docfoo-agent )
  install -m 0755 sidecar/docfoo-agent "$PREFIX/docfoo-agent"
else
  if [ "$VERSION" = latest ]; then
    API="https://api.github.com/repos/$REPO/releases/latest"
    TAG="$(curl -fsSL "${AUTH[@]}" "$API" | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -1)"
    [ -n "$TAG" ] || { echo "could not find a release for $REPO" >&2; exit 1; }
  else
    TAG="${VERSION#v}"
  fi
  ASSET="docfoo-cli-${TAG}-linux-x64.tar.gz"
  URL="https://github.com/$REPO/releases/download/v${TAG}/${ASSET}"
  echo "downloading $URL"
  curl -fsSL "${AUTH[@]}" -o "$TMP/$ASSET" "$URL"
  if curl -fsSL "${AUTH[@]}" -o "$TMP/$ASSET.sha256" "$URL.sha256" 2>/dev/null; then
    ( cd "$TMP" && sha256sum -c "$ASSET.sha256" )
  else
    echo "warning: no .sha256 asset — skipping checksum verification" >&2
  fi
  tar -xzf "$TMP/$ASSET" -C "$TMP"
  install -m 0755 "$TMP/docfoo-cli-${TAG}-linux-x64/docfoo" "$PREFIX/docfoo"
  install -m 0755 "$TMP/docfoo-cli-${TAG}-linux-x64/docfoo-agent" "$PREFIX/docfoo-agent"
fi

echo "installed: $PREFIX/docfoo"
case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) echo "note: add $PREFIX to your PATH (export PATH=\"$PREFIX:\$PATH\")" ;;
esac

if [ "$SKIP_SETUP" = 0 ]; then
  "$PREFIX/docfoo" setup || echo "run \`docfoo setup\` later to provision scan dependencies"
fi
