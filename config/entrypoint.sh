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
           CUSTOM_MAX_TOKENS CUSTOM_MAX_OUTPUT_TOKENS \
           ANTHROPIC_API_KEY OPENAI_API_KEY \
           HTTP_PROXY HTTPS_PROXY NO_PROXY
do
    eval "value=\${$var:-}"
    [ -n "$value" ] || continue
    sed -i "/^$var=/d" /etc/environment 2>/dev/null || true
    echo "$var=$value" >> /etc/environment
done

# --- Home directories ---------------------------------------------------------
# /home is a volume, so it hides whatever the image put there — and an account
# added in a later image has no home at all on an existing volume. Nothing
# complains: the user simply has no files, and sshd quietly refuses their key
# because the directory it does find belongs to root.
for u in dev1 dev2 dev3 dev; do
    id "$u" >/dev/null 2>&1 || continue
    home=$(getent passwd "$u" | cut -d: -f6)
    [ -n "$home" ] || continue
    if [ ! -d "$home" ]; then
        mkdir -p "$home"
        cp -a /etc/skel/. "$home/" 2>/dev/null || true
        log "created missing home for $u"
    fi
    # Ownership is checked every time, not only on creation: root-owned files in
    # a user's home are exactly what breaks key-based login.
    [ "$(stat -c '%U' "$home")" = "$u" ] || chown -R "$u:$u" "$home"
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
# placeholders; everything concrete is filled in here from the environment:
# CUSTOM_API_URL, CUSTOM_API_KEY and CUSTOM_MODEL name the OpenAI-compatible
# server — a remote one, or the one dev1c-setup runs next to this container —
# and CUSTOM_MAX_TOKENS / CUSTOM_MAX_OUTPUT_TOKENS carry the window it measured.
# No CUSTOM_API_URL means no model: the editor works, the agent stays silent,
# and dev1c-setup check says so.
#
# Only placeholders are replaced. If someone edited the settings by hand, their
# change stays: there is no placeholder left to overwrite.
#
# /etc/skel is deliberately NOT in this list. It is the reference copy the block
# above hands out, and substituting into it burns the placeholders on first run:
# after that the environment can never be pointed at a different provider,
# because there is nothing left to replace. Homes get the substitution; the
# reference stays pristine.
for cfg in /home/*/.config/zed/settings.json; do
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

    # Workstation ports for the tunnel from the machine with 1C: dev1 -> 7771
    # (mcp-1c) and 7781 (vrunner-mcp), dev2 -> 7772/7782, dev3 -> 7773/7783.
    # Fixed by convention so that neither side needs a parameter; 1c-connect
    # derives the same numbers from the workstation name. Any other home (the
    # tunnel account, a future user) keeps the placeholder: nc with a word for
    # a port fails loudly instead of binding somebody else's tunnel.
    u=${cfg#/home/}; u=${u%%/*}
    case "$u" in
        dev[1-9])
            n=${u#dev}
            sed -i \
                -e "s|WORKSTATION_1C_PORT_PLACEHOLDER|$((7770 + n))|" \
                -e "s|WORKSTATION_VRUNNER_PORT_PLACEHOLDER|$((7780 + n))|" \
                "$cfg"
            ;;
    esac

    # The window numbers: measured by dev1c-setup for a local model server,
    # or the defaults for a remote one. Always substituted, so the file is
    # valid JSON whatever the provider.
    sed -i \
        -e "s|CUSTOM_MAX_TOKENS_PLACEHOLDER|${CUSTOM_MAX_TOKENS:-55296}|" \
        -e "s|CUSTOM_MAX_OUTPUT_TOKENS_PLACEHOLDER|${CUSTOM_MAX_OUTPUT_TOKENS:-8192}|" \
        "$cfg"

    if [ -n "${CUSTOM_API_URL:-}" ]; then
        name="${CUSTOM_MODEL:-model}"
        sed -i \
            -e "s|CUSTOM_API_URL_PLACEHOLDER|$CUSTOM_API_URL|" \
            -e "s|CUSTOM_MODEL_PLACEHOLDER|$name|g" \
            "$cfg"
        log "model: $name at $CUSTOM_API_URL ($cfg)"
    else
        log "model: not configured — CUSTOM_API_URL is empty ($cfg)"
    fi
done

# --- Gitea --------------------------------------------------------------------
# A service rather than a session autostart: repositories must be reachable at
# all times, including when nobody is connected — for CI, for instance.
if [ "${GITEA_ENABLED:-1}" = "1" ]; then
    log "Gitea: starting"
    su gitea -s /bin/sh -c "GITEA_HOME=/var/lib/gitea /usr/local/bin/gitea-start" &
fi

# --- Remote access ------------------------------------------------------------
rm -f /var/run/xrdp/*.pid 2>/dev/null || true

# sshd serves the reverse tunnel from the machine that has 1C installed
/usr/local/bin/sshd-setup &

/usr/sbin/xrdp-sesman --nodaemon &

log "ready: RDP 3389, Gitea 3000, SSH 2022"
exec /usr/sbin/xrdp --nodaemon
