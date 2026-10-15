#!/bin/sh
# 1C development environment.
#
#   ./dev1c.sh            start
#   ./dev1c.sh stop       stop
#   ./dev1c.sh remove     stop and drop data (asks for confirmation)
#   ./dev1c.sh status     what runs inside
#   ./dev1c.sh logs       container log
#
# One container for everyone: three workstations (dev1, dev2, dev3 — password
# equals login) plus shared services, a git server and a model server. The model
# occupies the whole GPU memory, so there is nowhere to keep a copy per person.
#
# Ports: four consecutive from 3390. Taken? Pick another start:
#   PORT=4000 ./dev1c.sh
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
NAME="${NAME:-dev1c}"
IMAGE="${IMAGE:-oktaplus/dev1c:latest}"
MEMORY="${MEMORY:-24g}"
PORT="${PORT:-3390}"

ENGINE=""
for e in docker podman; do
    command -v "$e" >/dev/null 2>&1 && { ENGINE="$e"; break; }
done
[ -n "$ENGINE" ] || { echo "Neither docker nor podman found." >&2; exit 1; }

case "${1:-start}" in
    stop)
        $ENGINE rm -f "$NAME" >/dev/null 2>&1 || true
        echo "Environment stopped."
        exit 0 ;;
    remove)
        $ENGINE rm -f "$NAME" >/dev/null 2>&1 || true
        # Volumes hold repositories, working files and history — dropped only on
        # explicit confirmation.
        printf "Delete data (repositories, projects, settings)? [y/N]: "
        read -r answer
        case "$answer" in
            [yY]*)
                for v in home gitea models; do
                    $ENGINE volume rm "$NAME-$v" >/dev/null 2>&1 || true
                done
                echo "Data deleted." ;;
            *) echo "Data kept." ;;
        esac
        exit 0 ;;
    status)
        $ENGINE ps --filter "name=$NAME" --format '{{.Names}}  {{.Status}}' || true
        echo
        # pgrep -x matches the process name exactly; -f would match the probe
        # command itself and every service would look alive.
        $ENGINE exec "$NAME" sh -c '
            for s in gitea llama-server xrdp sshd; do
                if pgrep -x "$s" >/dev/null 2>&1; then
                    printf "  %-14s running\n" "$s"
                else
                    printf "  %-14s stopped\n" "$s"
                fi
            done
        ' 2>/dev/null || echo "  (container is not running)"
        exit 0 ;;
    logs)
        $ENGINE logs --tail 50 "$NAME"
        exit 0 ;;
esac

# Without the image the engine would go to a registry, which in an air-gapped
# network looks like a network problem and misleads everyone.
if ! $ENGINE image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "Image '$IMAGE' not found. Load it from the release file:" >&2
    echo "  $ENGINE load -i dev1c-image.tar.gz" >&2
    exit 1
fi

$ENGINE rm -f "$NAME" >/dev/null 2>&1 || true

# Key and address of the remote model are passed at run time instead of being
# baked into the image: every build stays clean, and the repository does not
# reveal what we run and where. See .env.example. In an air-gapped network .env
# is not needed at all — llama.cpp there wants neither key nor address.
# Passed into the container as they are, no renaming: what is written in
# .env is what arrives inside. LLAMA_ENABLED=0 turns off the local model
# server — useful while the machine has no GPU worth the name, or when a
# remote model is used instead.
PASS_VARS="CUSTOM_API_URL CUSTOM_API_KEY CUSTOM_MODEL LLAMA_ENABLED
           MCP_1C_BASE_URL MCP_1C_USER MCP_1C_PASSWORD"
if [ -f "$DIR/.env" ]; then
    for v in $PASS_VARS; do
        eval "cur=\${$v:-}"
        [ -n "$cur" ] && continue
        val=$(tr -d '\r' < "$DIR/.env" |
              awk -F= -v k="$v" '$1 == k { sub(/^[^=]*=/, ""); print }')
        [ -n "$val" ] && eval "$v=\$val"
    done
fi

# Data lives in named volumes only. Bind-mounted host directories are unreliable
# on Windows: antivirus software keeps the files open and reads from inside the
# container fail mid-stream with I/O errors rather than a clear refusal. The
# model is uploaded into its volume once — see the hint printed at the end.
#
# --init gives us a real init as PID 1. Without it PID 1 is xrdp, which neither
# reaps orphans nor arranges sessions properly.
set -- run -d --name "$NAME" --restart unless-stopped --init \
    -p "$PORT:3389" \
    -p "$((PORT + 1)):3000" \
    -p "$((PORT + 2)):8080" \
    -p "$((PORT + 3)):2022" \
    --shm-size 2g \
    --memory "$MEMORY" \
    --cap-add SYS_ADMIN \
    --security-opt seccomp=unconfined \
    --security-opt apparmor=unconfined \
    --device /dev/fuse \
    -v "$NAME-home:/home" \
    -v "$NAME-gitea:/var/lib/gitea" \
    -v "$NAME-models:/opt/models"

# The engine socket is not exposed inside: access to it equals root on the host,
# and nothing in this environment uses it. Enable explicitly if ever needed.
[ "${DOCKER_SOCK:-0}" = "1" ] && set -- "$@" -v /var/run/docker.sock:/var/run/docker.sock

# Podman runs in a user namespace: without keep-id files in volumes end up owned
# by somebody else.
[ "$ENGINE" = "podman" ] && set -- "$@" --userns=keep-id

# Passed through under the same names they have in .env — no renaming anywhere,
# so there is nothing to get wrong. The provider in the editor's settings is
# called "custom", and Zed derives the key variable from the provider name.
for v in $PASS_VARS; do
    eval "val=\${$v:-}"
    [ -n "$val" ] && set -- "$@" -e "$v=$val"
done

if [ "${NO_GPU:-0}" != "1" ]; then
    if [ "$ENGINE" = "podman" ]; then
        set -- "$@" --device nvidia.com/gpu=all
    else
        set -- "$@" --gpus all
    fi
fi

set -- "$@" "$IMAGE"

if ! $ENGINE "$@" >/dev/null; then
    echo >&2
    echo "Start failed. Two usual reasons:" >&2
    echo "  ports $PORT-$((PORT + 3)) are taken  ->  PORT=4000 $0" >&2
    echo "  no GPU or no driver                 ->  NO_GPU=1 $0" >&2
    exit 1
fi

echo
echo "1C development environment started ($ENGINE):"
echo
echo "  Workstations    localhost:$PORT   —  dev1 / dev1,  dev2 / dev2,  dev3 / dev3"
echo "  Repositories    http://localhost:$((PORT + 1))"
echo "  Model server    http://localhost:$((PORT + 2))"
echo "  SSH for tunnel  port $((PORT + 3))"
echo

if ! $ENGINE exec "$NAME" sh -c 'ls /opt/models/*.gguf' >/dev/null 2>&1; then
    echo "No model file — the agent will stay silent. Upload one once:"
    echo "  $ENGINE cp model.gguf $NAME:/opt/models/"
    echo "  $ENGINE restart $NAME"
    echo
fi

# Explicit: otherwise the exit code is inherited from the model-file check and a
# fully started environment looks like a failure.
exit 0
