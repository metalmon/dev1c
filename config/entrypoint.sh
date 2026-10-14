#!/bin/sh
# Container entrypoint.
#
# Starts the shared services and remote access. Shared means one instance for
# every user of the image: the model takes the whole GPU memory, and the git
# server has to be single so that repositories are common.
#
# Per-user things (Zed, analyzer cache, projects) live in home directories and
# start within each session.
set -eu

log() { echo "[entrypoint] $*"; }

# --- Environment for sessions -------------------------------------------------
# Variables passed with -e exist only in this process: an RDP session is created
# by PAM, which builds the environment anew from /etc/environment. Without this
# the model key is visible in `docker exec` but not to Zed — exactly the case of
# "everything is passed, nothing works".
for var in CUSTOM_API_KEY CUSTOM_API_URL CUSTOM_MODEL \
           ANTHROPIC_API_KEY OPENAI_API_KEY \
           HTTP_PROXY HTTPS_PROXY NO_PROXY
do
    eval "value=\${$var:-}"
    [ -n "$value" ] || continue
    sed -i "/^$var=/d" /etc/environment 2>/dev/null || true
    echo "$var=$value" >> /etc/environment
done

# --- Model server in Zed settings ---------------------------------------------
# Nobody edits JSON to point the environment at a model. The settings ship with
# placeholders; everything concrete is filled in here, from the environment and
# from what is actually present in /opt/models.
#
# Two cases:
#   CUSTOM_API_URL set  — a remote OpenAI-compatible server, used as the default
#   not set             — the local llama.cpp, named after the model file
#
# Only placeholders are replaced. If someone edited the settings by hand, their
# change stays: there is no placeholder left to overwrite.
# Searched recursively: a .gguf may be uploaded as a bare file or as the whole
# directory a model hub produced (LM Studio, for one, nests them as
# publisher/model/file.gguf).
MODEL_FILE="${LLAMA_MODEL:-}"
[ -n "$MODEL_FILE" ] || MODEL_FILE=$(find /opt/models -name '*.gguf' -type f 2>/dev/null | sort | head -1)
MODEL_ALIAS=""
[ -n "$MODEL_FILE" ] && MODEL_ALIAS=$(basename "$MODEL_FILE" .gguf)

for cfg in /home/*/.config/zed/settings.json /etc/skel/.config/zed/settings.json; do
    [ -f "$cfg" ] || continue

    if [ -n "${CUSTOM_API_URL:-}" ]; then
        name="${CUSTOM_MODEL:-model}"
        sed -i \
            -e "s|CUSTOM_API_URL_PLACEHOLDER|$CUSTOM_API_URL|" \
            -e "s|CUSTOM_MODEL_PLACEHOLDER|$name|g" \
            -e "s|\"provider\": \"llama.cpp\"|\"provider\": \"custom\"|" \
            -e "s|DEFAULT_MODEL_PLACEHOLDER|$name|" \
            "$cfg"
        log "model: remote server, $name ($cfg)"
    elif [ -n "$MODEL_ALIAS" ]; then
        sed -i -e "s|DEFAULT_MODEL_PLACEHOLDER|$MODEL_ALIAS|" "$cfg"
        log "model: local llama.cpp, $MODEL_ALIAS ($cfg)"
    fi
done

# --- Gitea --------------------------------------------------------------------
# A service rather than a session autostart: repositories must be reachable at
# all times, including when nobody is connected — for CI, for instance.
if [ "${GITEA_ENABLED:-1}" = "1" ]; then
    log "Gitea: starting"
    su gitea -s /bin/sh -c "GITEA_HOME=/var/lib/gitea /usr/local/bin/gitea-start" &
fi

# --- llama.cpp ----------------------------------------------------------------
# The model is mounted as a volume at /opt/models: it weighs more than the image
# and changes independently. With no model the service simply does not start —
# that is not an error, the environment stays usable, only the agent is absent.
if [ "${LLAMA_ENABLED:-1}" = "1" ]; then
    if [ -n "$MODEL_FILE" ] && [ -f "$MODEL_FILE" ]; then
        log "llama.cpp: model $(basename "$MODEL_FILE")"
        # --host 0.0.0.0 so the server is reachable from the host too, and
        # --alias so it reports a stable model name: that same name goes into the
        # editor's settings above. Layer and context values live in variables —
        # they differ radically between an 8 GB card and a 32 GB one.
        /usr/local/bin/llama-server \
            --model "$MODEL_FILE" \
            --alias "$MODEL_ALIAS" \
            --host 0.0.0.0 --port 8080 \
            --n-gpu-layers "${LLAMA_GPU_LAYERS:-999}" \
            --ctx-size "${LLAMA_CTX:-32768}" \
            --parallel "${LLAMA_PARALLEL:-3}" \
            --cont-batching \
            >/var/log/llama-server.log 2>&1 &
    else
        log "llama.cpp: no model in /opt/models — service not started"
        log "           upload a .gguf there, or set LLAMA_MODEL"
    fi
fi

# --- Remote access ------------------------------------------------------------
rm -f /var/run/xrdp/*.pid 2>/dev/null || true

# sshd serves the reverse tunnel from the machine that has 1C installed
/usr/local/bin/sshd-setup &

/usr/sbin/xrdp-sesman --nodaemon &

log "ready: RDP 3389, Gitea 3000, llama.cpp 8080, SSH 2022"
exec /usr/sbin/xrdp --nodaemon
