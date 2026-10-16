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
# LLAMA_CACHE=<dir> keeps every downloaded archive between runs (llama rows,
# OneScript, MinGit, mcp-1c). Needs docker: the vanessa-runner library tree is
# resolved by opm inside the project's OneScript image.
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

# Every archive is pinned by hash: a release asset can be replaced under the
# same name, and these are executables the customer's machines will run.
sha_of() { # $1 = file name → pinned SHA-256
    case "$1" in
        llama-b11606-bin-win-cuda-13.4-x64.zip) echo 93ba7c16d1ad8343248d540bb4c06e7e18f7d0601e762d6952bad13628295be8 ;;
        cudart-llama-bin-win-cuda-13.4-x64.zip) echo 738f8c251ac22b70c3ae6f83a10cf222725df0395246a2cf58f32bdb85fbe668 ;;
        llama-b11606-bin-ubuntu-cuda-13.4-x64.tar.gz) echo e984c5d442bf6310015440243a6f85927b320ecafccd50deaf9cc4a38daad22f ;;
        cudart-llama-b11606-bin-ubuntu-cuda-13.4-x64.tar.gz) echo 29bde5b11418e49192e790d8fdc488715253bdf9cddd6736125907fa0f14405a ;;
        mcp-1c-windows-amd64.exe) echo 0922a0e15c2ffc7201650ad0f17e96a405439a30d8b0adcb97744946a32081b2 ;;
        MCP_HTTPService.cfe) echo 76df5ccf993c10caef73fe8399d2fa7df1cc3b7e9b24fc6662ab402aeea1bf61 ;;
        OneScript-2.2.0-win-x64.zip) echo b12f2c44445e19d5c627f63af5cefd3eb7ad4e36b939bbd49c073e0956928267 ;;
        MinGit-2.56.0.2-64-bit.zip) echo da35e72aa21c005a5a0d298cfbae110bc1609a815730ea0dde84b01a1b3cd3be ;;
        *) echo "no pinned hash for $1" >&2; exit 1 ;;
    esac
}
check_sha() { # $1 = file, $2 = expected
    got=$(sha256sum "$1" | cut -c1-64)
    [ "$got" = "$2" ] || { echo "$(basename "$1"): hash $got, expected $2" >&2; exit 1; }
}
fetch() { # $1 = url, $2 = destination; verified against the pinned hash either way
    name=$(basename "$1")
    if [ -n "${LLAMA_CACHE:-}" ]; then
        mkdir -p "$LLAMA_CACHE"
        cached="$LLAMA_CACHE/$name"
        [ -f "$cached" ] || curl -fsSL -o "$cached" "$1"
        cp "$cached" "$2"
    else
        curl -fsSL -o "$2" "$1"
    fi
    check_sha "$2" "$(sha_of "$name")"
}

# The developer's kit: mcp-1c for the machine with the platform, the same
# release the image was designed against. Pinned here, not in the Dockerfile:
# the image no longer carries mcp-1c.
MCP_1C_VERSION=v1.22.3
for f in mcp-1c-windows-amd64.exe MCP_HTTPService.cfe; do
    fetch "https://github.com/feenlace/mcp-1c/releases/download/$MCP_1C_VERSION/$f" "$CONNECT/$f"
done

# The developer's program itself, Windows only (the platform is there), and
# its commented parameters.
[ -f "$BIN_DIR/1c-connect.exe" ] && cp "$BIN_DIR/1c-connect.exe" "$CONNECT/" || true
cp setup/1c-connect/1c-connect.example.toml "$CONNECT/1c-connect.toml"

# Tools for vrunner on the developer's machine. OneScript's archive already
# has the bin/ + lib/ layout its oscript.cfg expects (lib.system = ../lib), so
# it is unpacked as vrunner/ and the opm packages land in the same lib/:
# vanessa-runner (with every dependency vendored under its oscript_modules)
# and add — bddRunner.epf and xddTestRunner.epf, which vrunner looks for at
# <bin>/../lib/add. opm resolves "what the hub has today", so the tree is
# compared file by file with the committed manifest: two kits of the same
# version cannot differ silently. MinGit: the workstations have no git, or an
# ancient one. 1c-connect copies vrunner/ and git/ into %LOCALAPPDATA%\dev1c
# at install (see host.tools).
ONESCRIPT_VERSION=2.2.0
VRUNNER_VERSION=3.0.2
ADD_VERSION=6.8.0
MINGIT_VERSION=2.56.0.2
MANIFEST="$DIR/setup/1c-connect/vendor/vrunner-lib.sha256"
tools_row() {
    tmp=$(mktemp -d)
    mkdir -p "$CONNECT/vrunner" "$CONNECT/git"
    fetch "https://github.com/EvilBeaver/OneScript/releases/download/v$ONESCRIPT_VERSION/OneScript-$ONESCRIPT_VERSION-win-x64.zip" "$tmp/os.zip"
    unzip -qo "$tmp/os.zip" -d "$CONNECT/vrunner"
    [ -f "$CONNECT/vrunner/bin/oscript.exe" ] || { echo "OneScript: bin/oscript.exe not in the archive" >&2; exit 1; }
    fetch "https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.2/MinGit-$MINGIT_VERSION-64-bit.zip" "$tmp/git.zip"
    unzip -qo "$tmp/git.zip" -d "$CONNECT/git"
    [ -f "$CONNECT/git/cmd/git.exe" ] || { echo "MinGit: cmd/git.exe missing" >&2; exit 1; }
    # The library tree: opm inside the project's own OneScript image, into an
    # empty directory, checked against the manifest, then merged into lib/.
    # The mount is /out, not /lib: /lib is the container's own libc.
    mkdir -p "$tmp/opm"
    docker run --rm -v "$tmp/opm:/out" --entrypoint /bin/sh "evilbeaver/onescript:$ONESCRIPT_VERSION" \
        -c "opm install -d /out vanessa-runner@$VRUNNER_VERSION add@$ADD_VERSION >/dev/null && chmod -R a+rX /out"
    (cd "$tmp/opm" && sha256sum -c --quiet "$MANIFEST") \
        || { echo "vrunner lib differs from $MANIFEST — hub.oscript.io moved; review and regenerate the manifest (setup/1c-connect/vendor/README.md)" >&2; exit 1; }
    [ "$(cd "$tmp/opm" && find . -type f | wc -l)" = "$(wc -l <"$MANIFEST")" ] \
        || { echo "vrunner lib has files the manifest does not list" >&2; exit 1; }
    cp -r "$tmp/opm"/. "$CONNECT/vrunner/lib/"
    [ -f "$CONNECT/vrunner/lib/vanessa-runner/src/mcp.os" ] && [ -f "$CONNECT/vrunner/lib/add/xddTestRunner.epf" ] \
        || { echo "vrunner lib: mcp.os or xddTestRunner.epf missing" >&2; exit 1; }
    printf 'OneScript %s; vanessa-runner %s; add %s; MinGit %s\n' "$ONESCRIPT_VERSION" "$VRUNNER_VERSION" "$ADD_VERSION" "$MINGIT_VERSION" >"$CONNECT/tools.version"
    rm -rf "$tmp"
    echo "tools: $(du -sh "$CONNECT/vrunner" | cut -f1) vrunner, $(du -sh "$CONNECT/git" | cut -f1) git"
}
tools_row

# The model server for the host form: llama.cpp with its CUDA runtime, the
# same build the dev1c-llama image is made from, and nssm that keeps it as a
# Windows service. nssm comes from the repository (setup/dev1c-setup/vendor,
# build 2.24-101-g897c7ad, the one Windows 10+ needs; nssm.cc is often
# unreachable) and is checked against its pinned hash here. The installer
# copies everything into C:\dev1c\llama; a foreign nssm on the machine is
# never used.
LLAMA_BUILD=b11606
NSSM_SHA256=8288b6174231954fe0d9653522f1ccd9883c135dbd99fbc53cc2ea36cd91e401
llama_row() { # $1 = windows|linux
    dst="$KIT/llama/bin/$1-x64-cuda13"
    tmp=$(mktemp -d)
    mkdir -p "$dst"
    rel="https://github.com/ggml-org/llama.cpp/releases/download/$LLAMA_BUILD"
    case "$1" in
        windows)
            # Both zips are flat.
            fetch "$rel/llama-$LLAMA_BUILD-bin-win-cuda-13.4-x64.zip" "$tmp/l.zip"
            fetch "$rel/cudart-llama-bin-win-cuda-13.4-x64.zip" "$tmp/c.zip"
            unzip -qo "$tmp/l.zip" -d "$dst"
            unzip -qo "$tmp/c.zip" -d "$dst"
            cp setup/dev1c-setup/vendor/nssm.exe "$KIT/llama/nssm.exe"
            check_sha "$KIT/llama/nssm.exe" "$NSSM_SHA256"
            ;;
        linux)
            # Both tarballs nest their files under one top-level directory.
            fetch "$rel/llama-$LLAMA_BUILD-bin-ubuntu-cuda-13.4-x64.tar.gz" "$tmp/l.tgz"
            fetch "$rel/cudart-llama-$LLAMA_BUILD-bin-ubuntu-cuda-13.4-x64.tar.gz" "$tmp/c.tgz"
            tar -xzf "$tmp/l.tgz" -C "$dst" --strip-components=1
            tar -xzf "$tmp/c.tgz" -C "$dst" --strip-components=1
            ;;
    esac
    rm -rf "$tmp"
    [ -f "$dst/llama-server" ] || [ -f "$dst/llama-server.exe" ] || { echo "llama $1: llama-server not at the top of $dst" >&2; exit 1; }
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
