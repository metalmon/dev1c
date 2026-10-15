#!/bin/sh
# Assemble the two kits from already built parts: the image archive and the
# installer binaries come from CI (or from a local build), nothing is compiled
# here. Keeps the laptop out of the loop: it has neither the memory for the
# image nor a reason to carry a Rust toolchain.
#
#   ./pack.sh <dev1c-image.tar.gz> <dir with dev1c-setup and dev1c-setup.exe>
#
# Output: dist/dev1c-kit (the administrator's kit) and dist/1c-connect (the
# developer's, minus 1c-connect.exe until it exists).
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
cd "$DIR"
IMAGE_TAR=${1:?path to dev1c-image.tar.gz}
BIN_DIR=${2:?directory with the dev1c-setup binaries}
OUT=${OUT_DIR:-dist}

KIT="$OUT/dev1c-kit"
CONNECT="$OUT/1c-connect"
rm -rf "$KIT" "$CONNECT"
mkdir -p "$KIT" "$CONNECT"

cp "$IMAGE_TAR" "$KIT/dev1c-image.tar.gz"
for b in dev1c-setup dev1c-setup.exe; do
    [ -f "$BIN_DIR/$b" ] && cp "$BIN_DIR/$b" "$KIT/"
done
chmod +x "$KIT/dev1c-setup" 2>/dev/null || true
cp setup/dev1c-setup/dev1c.example.toml "$KIT/dev1c.toml"

# The developer's kit: mcp-1c for the machine with the platform, the same
# release the image was designed against. Pinned here, not in the Dockerfile:
# the image no longer carries mcp-1c.
MCP_1C_VERSION=v1.22.3
for f in mcp-1c-windows-amd64.exe MCP_HTTPService.cfe; do
    curl -fsSL "https://github.com/feenlace/mcp-1c/releases/download/$MCP_1C_VERSION/$f" -o "$CONNECT/$f"
done

# The developer's program itself, Windows only (the platform is there), and
# its commented parameters.
[ -f "$BIN_DIR/1c-connect.exe" ] && cp "$BIN_DIR/1c-connect.exe" "$CONNECT/" || true
cp setup/1c-connect/1c-connect.example.toml "$CONNECT/1c-connect.toml"

# Checksums by the installer itself, so the customer verifies with the same code.
SETUP="$KIT/dev1c-setup"
[ -x "$SETUP" ] || SETUP="$KIT/dev1c-setup.exe"
"$SETUP" --kit "$KIT" checksums >/dev/null
"$SETUP" --kit "$CONNECT" checksums >/dev/null

echo "Kits ready in $OUT:"
du -sh "$KIT" "$CONNECT"
