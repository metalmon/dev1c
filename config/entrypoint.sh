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

# --- Editor settings from the image -------------------------------------------
# Home directories live on a volume and survive image updates — which also means
# a new image's settings never reach anyone who already has a home. The symptom
# is nasty: the image carries a fix, the environment keeps the old behaviour, and
# nothing anywhere says why.
#
# So our two files are refreshed when they change in the image, tracked by a
# stamp of the shipped version. A hand-edited file is replaced too, but kept as
# .bak next to it: these are our settings, not the user's, and the people this
# environment is for do not edit JSON — while we change it constantly.
# This runs as root and writes into directories owned by the user. A symlink put
# in place of any of these paths would have root write wherever it points. Today
# the workstation accounts have passwordless sudo, so that grants nothing they
# could not do directly — but sudo is exactly the kind of thing dropped when a
# pilot turns into an installation, and then this would become a real hole. So
# the paths are refused when symlinked, and the writes do not follow links.
for home in /home/*; do
    zed="$home/.config/zed"
    [ -d "$zed" ] || continue
    if [ -L "$home" ] || [ -L "$home/.config" ] || [ -L "$zed" ]; then
        log "skipping $home: a symlinked path on the way to the settings"
        continue
    fi
    owner_u=$(stat -c '%U' "$home")
    owner_g=$(stat -c '%G' "$home")
    for f in settings.json AGENTS.md; do
        src="/etc/skel/.config/zed/$f"
        dst="$zed/$f"
        stamp="$zed/.$f.from-image"
        [ -f "$src" ] || continue
        if [ -L "$dst" ] || [ -L "$dst.bak" ] || [ -L "$stamp" ]; then
            log "skipping $dst: symlink"
            continue
        fi
        shipped=$(md5sum "$src" | cut -d' ' -f1)
        [ "$(cat "$stamp" 2>/dev/null)" = "$shipped" ] && continue
        [ -f "$dst" ] && cp --remove-destination "$dst" "$dst.bak"
        # install after rm: writing over a path replaces the file itself rather
        # than following whatever it might point at
        rm -f "$dst"
        install -o "$owner_u" -g "$owner_g" -m 0644 "$src" "$dst"
        tmp=$(mktemp "$zed/.from-image.XXXXXX") || continue
        echo "$shipped" > "$tmp"
        chown "$owner_u:$owner_g" "$tmp"
        mv -f "$tmp" "$stamp"
        log "$f updated from the image in $home"
    done
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
    # Same reasoning as above, and not theoretical: `sed -i` on a symlinked path
    # reads whatever it points at and leaves the result in its place. Pointed at
    # /etc/shadow, it put the shadow file's contents into a user's directory —
    # the file itself survived, but the mechanism works, and with different
    # permissions that is a leak.
    if [ -L "$cfg" ]; then
        log "skipping $cfg: symlink"
        continue
    fi

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
        log "llama.cpp: model $(basename "$MODEL_FILE"), context ${LLAMA_CTX:-196608}"
        # --host 0.0.0.0 so the server is reachable from the host too, and
        # --alias so it reports a stable model name: that same name goes into the
        # editor's settings above.
        #
        # One slot, not three. llama.cpp splits --ctx-size between slots, so
        # --parallel 3 quietly gave each workstation a third of it: 32768 became
        # about 11000 tokens per request, while this environment's prompt — the
        # editor's own instructions plus the skill catalog — is around 17500. The
        # agent answered nothing, and the reason was only visible in the server's
        # log: "request (17475 tokens) exceeds the available context size (11008
        # tokens)". Requests from several people queue instead, which is the
        # right trade: waiting beats not working.
        #
        # The KV cache is quantised to q8_0, which roughly halves its memory at
        # no noticeable quality cost and is what makes a context this long fit on
        # a card at all. Flash attention is required for a quantised cache.
        /usr/local/bin/llama-server \
            --model "$MODEL_FILE" \
            --alias "$MODEL_ALIAS" \
            --host 0.0.0.0 --port 8080 \
            --n-gpu-layers "${LLAMA_GPU_LAYERS:-999}" \
            --ctx-size "${LLAMA_CTX:-196608}" \
            --parallel "${LLAMA_PARALLEL:-1}" \
            --cache-type-k "${LLAMA_CACHE_TYPE:-q8_0}" \
            --cache-type-v "${LLAMA_CACHE_TYPE:-q8_0}" \
            --flash-attn "${LLAMA_FLASH_ATTN:-on}" \
            --batch-size "${LLAMA_BATCH:-2048}" \
            --ubatch-size "${LLAMA_UBATCH:-512}" \
            `# Thought blocks are kept out of the answer itself. Note what this
             # does NOT do: a model distilled on reasoning traces writes its
             # thinking regardless — neither this flag nor enable_thinking in the
             # template stops it, that behaviour lives in the weights. What it
             # does mean is that such a model needs a generous answer budget: cut
             # it short and the reply ends mid-thought with finish_reason
             # "length" and an empty content, which reads as "the model said
             # nothing".` \
            --reasoning "${LLAMA_REASONING:-off}" \
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
