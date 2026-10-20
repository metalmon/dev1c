#!/bin/sh
# Состав комплектов описан в одном месте — разделе «Комплекты» спецификации
# docs/superpowers/specs/2026-10-16-setup-binaries-design.md. Этот скрипт
# собирает по нему; расхождение — дефект спецификации, а не скрипта.
# Assemble the kits from already built parts: the image archive and the
# installer binaries come from CI (or from a local build), nothing is compiled
# here. Keeps the laptop out of the loop: it has neither the memory for the
# image nor a reason to carry a Rust toolchain.
#
#   ./pack.sh [--llama windows|linux|none] [--os linux|windows] <dev1c-image.tar.gz> <dir with dev1c-setup and dev1c-setup.exe>
#
# --os windows (default, the pilot host for the first two kits): produces
# dist/dev1c-kit (the administrator's kit) and dist/1c-connect (the
# developer's, minus 1c-connect.exe until it exists).
#
# --os linux: a second administrator's kit, dist/dev1c-kit-linux, for a
# headless Ubuntu server with no internet — serve = "docker" only, so it
# carries the environment+model-server image, no llama row, no nssm, no
# developer's kit, and the NVIDIA Container Toolkit as .deb files (the server
# cannot download them) at nvidia-container-toolkit/ inside the kit — read
# from a directory of the same name the workflow fetched them into (override
# with NVIDIA_TOOLKIT_DIR), checked against the pins below.
#
# --llama picks the row of llama.cpp binaries for the model server on the host
# (serve = "host" in dev1c.toml): the kit is used on one OS, so it carries one
# row (about 550 MB) plus nssm for Windows. Default: windows — the pilot host.
# Only --os windows builds a host row; the Linux kit always serves docker and
# refuses --llama rather than ignoring it.
# LLAMA_CACHE=<dir> keeps every downloaded archive between runs (llama rows,
# OneScript, MinGit, mcp-1c). Needs docker: the vanessa-runner library tree is
# resolved by opm inside the project's OneScript image.
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
cd "$DIR"
LLAMA_ROW=windows
LLAMA_GIVEN=
OS=windows
while [ "${1:-}" = "--llama" ] || [ "${1:-}" = "--os" ]; do
    case "$1" in
        --llama) LLAMA_ROW=${2:?windows|linux|none}; LLAMA_GIVEN=1; shift 2 ;;
        --os) OS=${2:?linux|windows}; shift 2 ;;
    esac
done
case "$OS" in
    linux|windows) ;;
    *) echo "--os: linux or windows" >&2; exit 2 ;;
esac
# The Linux kit serves docker and has no llama row at all, so --llama with it
# asks for something that will not happen. Refused, not discarded: a flag that
# vanishes silently is how a kit ends up without the row its caller wanted.
if [ "$OS" = "linux" ] && [ -n "$LLAMA_GIVEN" ]; then
    echo "--llama is for --os windows only: the Linux kit serves docker and carries no llama row" >&2
    exit 2
fi
IMAGE_TAR=${1:?path to dev1c-image.tar.gz}
BIN_DIR=${2:?directory with the dev1c-setup binaries}
OUT=${OUT_DIR:-dist}

if [ "$OS" = "linux" ]; then
    KIT="$OUT/dev1c-kit-linux"
else
    KIT="$OUT/dev1c-kit"
fi
CONNECT="$OUT/1c-connect"
rm -rf "$KIT"
mkdir -p "$KIT"
if [ "$OS" = "windows" ]; then
    rm -rf "$CONNECT"
    mkdir -p "$CONNECT"
fi

cp "$IMAGE_TAR" "$KIT/dev1c-image.tar.gz"
if [ "$OS" = "linux" ]; then
    # No .exe, ever — this kit runs on the server, not on a developer's
    # Windows machine, even when BIN_DIR carries both binaries.
    [ -f "$BIN_DIR/dev1c-setup" ] || { echo "pack.sh --os linux: dev1c-setup (Linux binary) not in $BIN_DIR" >&2; exit 1; }
    cp "$BIN_DIR/dev1c-setup" "$KIT/"
else
    # One kit, one OS — the mirror of the rule above. This kit runs on a
    # Windows host, so the Linux binary has no business in it even when
    # BIN_DIR carries both: a kit that holds two platforms invites the
    # wrong one to be run, and the customer has no way to tell which is
    # meant for them (the customer asked for this, 20.10.2026).
    [ -f "$BIN_DIR/dev1c-setup.exe" ] || { echo "pack.sh --os windows: dev1c-setup.exe not in $BIN_DIR" >&2; exit 1; }
    cp "$BIN_DIR/dev1c-setup.exe" "$KIT/"
fi
chmod +x "$KIT/dev1c-setup" 2>/dev/null || true
if [ "$OS" = "linux" ]; then
    # The program prints its own sample: one source for the comments and the
    # structure, so dev1c.toml and Cfg::default never drift apart.
    "$KIT/dev1c-setup" example --serve docker >"$KIT/dev1c.toml"
else
    cp setup/dev1c-setup/dev1c.example.toml "$KIT/dev1c.toml"
fi

# The one place the NVIDIA Container Toolkit version is written. The workflow
# reads it from here (`sed -n 's/^NVIDIA_TOOLKIT_VERSION=//p' pack.sh`) instead
# of repeating it: three copies of one version is how a kit ships packages
# whose hashes belong to another version. The pins below name it through this
# variable too, so bumping it without renewing the hashes fails the build on a
# hash mismatch instead of shipping unchecked .deb files.
NVIDIA_TOOLKIT_VERSION=1.20.1-1

# Every archive is pinned by hash: a release asset can be replaced under the
# same name, and these are executables the customer's machines will run.
sha_of() { # $1 = file name → pinned SHA-256
    case "$1" in
        llama-b11734-bin-win-cuda-13.4-x64.zip) echo 16ebb234a4723d5900cff196b5e6ce8c0559202cfe16379c08511f41611e0ad2 ;;
        cudart-llama-bin-win-cuda-13.4-x64.zip) echo 738f8c251ac22b70c3ae6f83a10cf222725df0395246a2cf58f32bdb85fbe668 ;;
        llama-b11734-bin-ubuntu-cuda-13.4-x64.tar.gz) echo f165c871d0473ace56b05b5ed32fef7ecb47d6a01ca06cbda0ad9d55132b0892 ;;
        cudart-llama-b11734-bin-ubuntu-cuda-13.4-x64.tar.gz) echo e0aa65934ed0171a8784a32d072ac473cd001d3ff041bc08ff370a908a853daa ;;
        mcp-1c-windows-amd64.exe) echo 0922a0e15c2ffc7201650ad0f17e96a405439a30d8b0adcb97744946a32081b2 ;;
        MCP_HTTPService.cfe) echo 76df5ccf993c10caef73fe8399d2fa7df1cc3b7e9b24fc6662ab402aeea1bf61 ;;
        OneScript-2.2.0-win-x64.zip) echo b12f2c44445e19d5c627f63af5cefd3eb7ad4e36b939bbd49c073e0956928267 ;;
        MinGit-2.56.0.2-64-bit.zip) echo da35e72aa21c005a5a0d298cfbae110bc1609a815730ea0dde84b01a1b3cd3be ;;
        "nvidia-container-toolkit_${NVIDIA_TOOLKIT_VERSION}_amd64.deb") echo a95c9d206a7ecd6a90adff9290635271fa167a86b5767b17eaa0478a552eb02c ;;
        "nvidia-container-toolkit-base_${NVIDIA_TOOLKIT_VERSION}_amd64.deb") echo e01964a064fbdb62425ab22ec5832349f92c5b0b50c8f23fdf14dad004db4fb2 ;;
        "libnvidia-container1_${NVIDIA_TOOLKIT_VERSION}_amd64.deb") echo 9d803cd9c898cb2fa927529d5ac4fc09f3d65c8cfbde8985de1b9398e53a4656 ;;
        "libnvidia-container-tools_${NVIDIA_TOOLKIT_VERSION}_amd64.deb") echo 8bd131b8977a3db667982024850ae7b50cfa02d6b2be06a718064ec814778f05 ;;
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

if [ "$OS" = "windows" ]; then

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
    # The mount is /out, not /lib: /lib is the container's own libc. Rootful
    # docker (CI) writes the tree as root; a+rwX lets this user copy and
    # remove it afterwards.
    mkdir -p "$tmp/opm"
    docker run --rm -v "$tmp/opm:/out" --entrypoint /bin/sh "evilbeaver/onescript:$ONESCRIPT_VERSION" \
        -c "opm install -d /out vanessa-runner@$VRUNNER_VERSION add@$ADD_VERSION >/dev/null && chmod -R a+rwX /out"
    (cd "$tmp/opm" && sha256sum -c --quiet "$MANIFEST") \
        || { echo "vrunner lib differs from $MANIFEST — hub.oscript.io moved; review and regenerate the manifest (setup/1c-connect/vendor/README.md)" >&2; exit 1; }
    [ "$(cd "$tmp/opm" && find . -type f | wc -l)" = "$(wc -l <"$MANIFEST")" ] \
        || { echo "vrunner lib has files the manifest does not list" >&2; exit 1; }
    mkdir -p "$CONNECT/vrunner/lib"
    cp -r "$tmp/opm"/. "$CONNECT/vrunner/lib/"
    [ -f "$CONNECT/vrunner/lib/vanessa-runner/src/mcp.os" ] && [ -f "$CONNECT/vrunner/lib/add/xddTestRunner.epf" ] \
        || { echo "vrunner lib: mcp.os or xddTestRunner.epf missing" >&2; exit 1; }
    printf 'OneScript %s; vanessa-runner %s; add %s; MinGit %s\n' "$ONESCRIPT_VERSION" "$VRUNNER_VERSION" "$ADD_VERSION" "$MINGIT_VERSION" >"$CONNECT/tools.version"
    rm -rf "$tmp"
    echo "tools: $(du -sh "$CONNECT/vrunner" | cut -f1) vrunner, $(du -sh "$CONNECT/git" | cut -f1) git"
}
tools_row

# The developer's own web server: a trimmed Apache HTTP Server, vendored in
# the repository rather than downloaded (setup/1c-connect/vendor/httpd/README.md
# — same reasoning as nssm.exe below: a few MB is cheap to carry, an Apache
# Lounge URL is not a stable thing to depend on at build time). 1c-connect
# copies it into %LOCALAPPDATA%\dev1c alongside vrunner/ and git/ (see
# host.tools) to publish the active 1C database itself.
httpd_row() {
    src="$DIR/setup/1c-connect/vendor/httpd"
    [ -d "$src" ] || { echo "setup/1c-connect/vendor/httpd is missing — the kit would ship with no web server; see setup/1c-connect/vendor/httpd/README.md" >&2; exit 1; }
    [ -f "$src/bin/httpd.exe" ] || { echo "setup/1c-connect/vendor/httpd: bin/httpd.exe missing" >&2; exit 1; }
    cp -r "$src" "$CONNECT/httpd"
    echo "httpd: $(du -sh "$CONNECT/httpd" | cut -f1)"
}
httpd_row

# The model server for the host form: llama.cpp with its CUDA runtime, the
# same build the dev1c-llama image is made from, and nssm that keeps it as a
# Windows service. nssm comes from the repository (setup/dev1c-setup/vendor,
# build 2.24-101-g897c7ad, the one Windows 10+ needs; nssm.cc is often
# unreachable) and is checked against its pinned hash here. The installer
# copies everything into C:\dev1c\llama; a foreign nssm on the machine is
# never used.
LLAMA_BUILD=b11734
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

fi # OS = windows (developer's kit + the host-form llama row)

if [ "$OS" = "linux" ]; then
    # The server cannot reach the internet, so these four .deb files travel
    # in the kit. The workflow fetches them from NVIDIA's repository into a
    # directory of the same name as the one they end up at inside the kit;
    # pack.sh only verifies and copies — the pins in sha_of are what make a
    # swapped file visible, and NVIDIA_TOOLKIT_VERSION above is the one place
    # the version is written.
    src="${NVIDIA_TOOLKIT_DIR:-nvidia-container-toolkit}"
    dst="$KIT/nvidia-container-toolkit"
    mkdir -p "$dst"
    for pkg in nvidia-container-toolkit nvidia-container-toolkit-base libnvidia-container1 libnvidia-container-tools; do
        f="${pkg}_${NVIDIA_TOOLKIT_VERSION}_amd64.deb"
        [ -f "$src/$f" ] || { echo "nvidia-container-toolkit: $f missing in $src (the workflow must fetch it before calling pack.sh)" >&2; exit 1; }
        cp "$src/$f" "$dst/"
        check_sha "$dst/$f" "$(sha_of "$f")"
    done
    echo "nvidia-container-toolkit: $(ls "$dst" | wc -l) packages, $(du -sh "$dst" | cut -f1)"

    # The administrator's page. Only this kit carries one: its content is
    # Linux-shaped (sha256sum -c, dpkg -i, ./dev1c-setup) and the headless
    # server is what it is for; the Windows kit instructs through the
    # installer's own window. The «Ручной путь» section is generated from the
    # installer in the kit, so the commands cannot drift from what the program
    # does — see scripts/manual-path.sh.
    sh scripts/manual-path.sh "$KIT"
fi

# Checksums by the installer itself, so the customer verifies with the same code.
SETUP="$KIT/dev1c-setup"
[ -x "$SETUP" ] || SETUP="$KIT/dev1c-setup.exe"
"$SETUP" --kit "$KIT" checksums >/dev/null
if [ "$OS" = "windows" ]; then
    "$SETUP" --kit "$CONNECT" checksums >/dev/null
fi

echo "Kits ready in $OUT:"
if [ "$OS" = "windows" ]; then
    du -sh "$KIT" "$CONNECT"
else
    du -sh "$KIT"
fi
