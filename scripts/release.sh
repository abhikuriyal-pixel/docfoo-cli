#!/usr/bin/env bash
# Build a Linux x64 release tarball: dist/docfoo-<version>-linux-x64.tar.gz
# plus its .sha256 sidecar. Run from anywhere; builds the repo it lives in.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ -n "$VERSION" ] || { echo "could not read the workspace version" >&2; exit 1; }

case "$(uname -s)" in
  Linux) OS_TAG="linux-x64" ;;
  *) echo "release.sh builds Linux artifacts; use release.ps1 on Windows" >&2; exit 1 ;;
esac

echo "building docfoo $VERSION ($OS_TAG)…"
cargo build --release
( cd sidecar && { [ -d node_modules ] || bun install; } && ./build.sh docfoo-agent )

NAME="docfoo-${VERSION}-${OS_TAG}"
DIST="dist/${NAME}"
rm -rf "$DIST"
mkdir -p "$DIST"
install -m 0755 target/release/docfoo "$DIST/docfoo"
install -m 0755 sidecar/docfoo-agent "$DIST/docfoo-agent"
cp install.sh README.md "$DIST/"

tar -C dist -czf "dist/${NAME}.tar.gz" "$NAME"
( cd dist && sha256sum "${NAME}.tar.gz" > "${NAME}.tar.gz.sha256" )
echo "wrote dist/${NAME}.tar.gz"
