#!/bin/sh
# Assemble the two kits from already built parts: the image archive and the
# installer binaries come from CI (or from a local build), nothing is compiled
# here. Keeps the laptop out of the loop: it has neither the memory for the
# image nor a reason to carry a Rust toolchain.
#
#   ./pack.sh [--llama windows|linux|none] <dev1c-image.tar.gz> <dir with dev1c-setup and dev1c-setup.exe>
#
# Output: dist/dev1c-kit (the administrator's kit) and dist/1c-connect (the
# developer's, minus 1c-connect.exe until it exists).
#
# --llama picks the row of llama.cpp binaries for the model server on the host
# (serve = "host" in dev1c.toml): the kit is used on one OS, so it carries one
# row (about 550 MB) plus nssm for Windows. Default: windows — the pilot host.
# LLAMA_CACHE=<dir> keeps the downloaded archives between runs.
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
cd "$DIR"
LLAMA_ROW=windows
if [ "${1:-}" = "--llama" ]; then
    LLAMA_ROW=${2:?windows|linux|none}
    shift 2
fi
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

# The model server for the host form: llama.cpp with its CUDA runtime, the
# same build the dev1c-llama image is made from, and nssm that keeps it as a
# Windows service. nssm comes from the repository (setup/dev1c-setup/vendor,
# build 2.24-101-g897c7ad, the one Windows 10+ needs; nssm.cc is often
# unreachable) and is checked against its pinned hash here. The installer
# copies everything into C:\dev1c\llama; a foreign nssm on the machine is
# never used.
LLAMA_BUILD=b11606
NSSM_SHA256=8288b6174231954fe0d9653522f1ccd9883c135dbd99fbc53cc2ea36cd91e401
fetch() { # $1 = url, $2 = destination
    if [ -n "${LLAMA_CACHE:-}" ]; then
        mkdir -p "$LLAMA_CACHE"
        cached="$LLAMA_CACHE/$(basename "$1")"
        [ -f "$cached" ] || curl -fsSL -o "$cached" "$1"
        cp "$cached" "$2"
    else
        curl -fsSL -o "$2" "$1"
    fi
}
llama_row() { # $1 = windows|linux
    dst="$KIT/llama/bin/$1-x64-cuda13"
    tmp=$(mktemp -d)
    mkdir -p "$dst"
    rel="https://github.com/ggml-org/llama.cpp/releases/download/$LLAMA_BUILD"
    case "$1" in
        windows)
            fetch "$rel/llama-$LLAMA_BUILD-bin-win-cuda-13.4-x64.zip" "$tmp/l.zip"
            fetch "$rel/cudart-llama-bin-win-cuda-13.4-x64.zip" "$tmp/c.zip"
            unzip -qo "$tmp/l.zip" -d "$dst"
            unzip -qo "$tmp/c.zip" -d "$dst"
            cp setup/dev1c-setup/vendor/nssm.exe "$KIT/llama/nssm.exe"
            got=$(sha256sum "$KIT/llama/nssm.exe" | cut -c1-64)
            [ "$got" = "$NSSM_SHA256" ] || { echo "nssm.exe: hash $got, expected $NSSM_SHA256" >&2; exit 1; }
            ;;
        linux)
            fetch "$rel/llama-$LLAMA_BUILD-bin-ubuntu-cuda-13.4-x64.tar.gz" "$tmp/l.tgz"
            fetch "$rel/cudart-llama-$LLAMA_BUILD-bin-ubuntu-cuda-13.4-x64.tar.gz" "$tmp/c.tgz"
            tar -xzf "$tmp/l.tgz" -C "$dst"
            tar -xzf "$tmp/c.tgz" -C "$dst"
            ;;
    esac
    rm -rf "$tmp"
    # Some archives nest the files (build/bin): the installer expects them flat.
    if [ -d "$dst/build/bin" ]; then
        mv "$dst"/build/bin/* "$dst"/
        rm -rf "$dst/build"
    fi
    echo "llama $1: $(ls "$dst" | wc -l) files, $(du -sh "$dst" | cut -f1)"
}
case "$LLAMA_ROW" in
    windows|linux) llama_row "$LLAMA_ROW" ;;
    none) ;;
    *) echo "--llama: windows, linux or none" >&2; exit 2 ;;
esac

# Checksums by the installer itself, so the customer verifies with the same code.
SETUP="$KIT/dev1c-setup"
[ -x "$SETUP" ] || SETUP="$KIT/dev1c-setup.exe"
"$SETUP" --kit "$KIT" checksums >/dev/null
"$SETUP" --kit "$CONNECT" checksums >/dev/null

echo "Kits ready in $OUT:"
du -sh "$KIT" "$CONNECT"
