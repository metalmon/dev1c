#!/bin/sh
# =============================================================================
# The demo stand's payload: the 1C platform and the template base live in two
# named volumes, not in the image.
#
#   ./provision.sh build            both images: the stand and the payload
#   ./provision.sh export [file]    the payload image -> one tar.gz
#   ./provision.sh import [file]    that tar.gz -> the two volumes
#   ./provision.sh run [docker args…]   (re)create the stand container
#   ./provision.sh status           what is in the volumes right now
#
# Fixed names, no discovery: one stand per machine, the same names every time.
#
#   image      oktaplus/dev1c-1c:latest          ~4 GB, rebuilt often
#   payload    oktaplus/dev1c-1c-payload:latest  ~4 GB, rebuilt per 1C release
#   volumes    dev1c-platform -> /opt/1cv8t      the training platform
#              dev1c-demo     -> /opt/1c-demo    the template base
#              dev1c-home, dev1c-gitea           as for the plain dev1c image
#   ports      3390 RDP, 3391 Gitea, 3393 SSH
#
# `run` passes its extra arguments to `docker run`, which is where the model
# settings go — this script never reads or writes a key:
#
#   ./provision.sh run -e CUSTOM_API_URL=… -e CUSTOM_API_KEY=… -e CUSTOM_MODEL=…
#
# The build needs `platform/` (the 1C installer, the demo template, see
# Dockerfile.1c); import and run need only the tar and Docker.
# =============================================================================
set -eu
# Git Bash rewrites anything that looks like a Unix path before docker.exe sees
# it, so `/opt/1cv8t` inside the container turns into a directory under the Git
# installation. Harmless everywhere else; without it this script only works on
# Linux. Container-side paths are also given to `docker run` without a leading
# slash where a PATH lookup will do.
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

IMAGE=oktaplus/dev1c-1c:latest
PAYLOAD_IMAGE=oktaplus/dev1c-1c-payload:latest
CONTAINER=dev1c
VOL_PLATFORM=dev1c-platform
VOL_DEMO=dev1c-demo
VOL_HOME=dev1c-home
VOL_GITEA=dev1c-gitea
TAR=platform/dev1c-1c-payload.tar.gz
MOUNTS="-v $VOL_PLATFORM:/opt/1cv8t -v $VOL_DEMO:/opt/1c-demo"

say() { printf '[provision] %s\n' "$*"; }
die() { printf '[provision] %s\n' "$*" >&2; exit 1; }

volumes() {
    for v in "$VOL_PLATFORM" "$VOL_DEMO" "$VOL_HOME" "$VOL_GITEA"; do
        docker volume inspect "$v" >/dev/null 2>&1 || docker volume create "$v" >/dev/null
    done
}

case "${1:-}" in
build)
    [ -d platform ] || die "platform/ is missing: the 1C installer and the demo template live there"
    say "the stand image (no platform, no demo base)"
    docker build -f Dockerfile.1c --build-context platform=platform -t "$IMAGE" .
    say "the payload image (platform + template base)"
    docker build -f Dockerfile.1c --build-context platform=platform --target payload -t "$PAYLOAD_IMAGE" .
    ;;
export)
    out=${2:-$TAR}
    docker image inspect "$PAYLOAD_IMAGE" >/dev/null 2>&1 || die "no $PAYLOAD_IMAGE: ./provision.sh build"
    say "writing $out (this takes a while: about 4 GB through gzip)"
    docker run --rm "$PAYLOAD_IMAGE" 1c-stand export-payload > "$out.part"
    mv "$out.part" "$out"
    say "$out: $(du -h "$out" | cut -f1)"
    ;;
import)
    src=${2:-$TAR}
    [ -f "$src" ] || die "no $src: ./provision.sh export first, or name the file"
    docker image inspect "$IMAGE" >/dev/null 2>&1 || die "no $IMAGE: ./provision.sh build"
    volumes
    say "unpacking $src into $VOL_PLATFORM and $VOL_DEMO"
    # shellcheck disable=SC2086
    docker run --rm -i $MOUNTS "$IMAGE" 1c-stand import-payload < "$src"
    ;;
run)
    shift
    volumes
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    # shellcheck disable=SC2086
    docker run -d --name "$CONTAINER" --restart unless-stopped --init \
        --shm-size 2g --cap-add SYS_ADMIN \
        --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
        --device /dev/fuse --add-host host.docker.internal:host-gateway \
        -p 3390:3389 -p 3391:3000 -p 3393:2022 \
        -v "$VOL_HOME:/home" -v "$VOL_GITEA:/var/lib/gitea" $MOUNTS \
        "$@" "$IMAGE" >/dev/null
    say "$CONTAINER is up: RDP 3390, Gitea 3391, SSH 3393"
    say "the check of a running stand: docker exec $CONTAINER test-stand"
    ;;
status)
    for v in "$VOL_PLATFORM" "$VOL_DEMO" "$VOL_HOME" "$VOL_GITEA"; do
        docker volume inspect "$v" >/dev/null 2>&1 \
            && printf '%-16s present\n' "$v" || printf '%-16s MISSING\n' "$v"
    done
    # shellcheck disable=SC2086
    docker run --rm $MOUNTS "$IMAGE" sh -c \
        'cat /opt/1c-demo/payload.conf 2>/dev/null || echo "payload: the volumes are empty"'
    ;;
*)
    echo "usage: ./provision.sh build | export [file] | import [file] | run [docker args…] | status" >&2
    exit 2
    ;;
esac
