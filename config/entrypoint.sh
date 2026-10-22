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
for var in MODEL_PROVIDER MODEL_NAME CUSTOM_MODEL \
           LLAMA_API_URL LLAMACPP_API_KEY \
           CUSTOM_API_KEY CUSTOM_API_URL \
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
for u in dev1 dev2 dev3; do
    id "$u" >/dev/null 2>&1 || continue
    home=$(getent passwd "$u" | cut -d: -f6)
    [ -n "$home" ] || continue
    if [ ! -d "$home" ]; then
        mkdir -p "$home"
        cp -a /etc/skel/. "$home/" 2>/dev/null || true
        log "created missing home for $u"
    fi
    # The project directory exists even before anything puts a project in it.
    # Two things want it: vrunner works there and the dump lands there, and
    # bsl-analyzer's `workspace` profile refuses to start at all when its root
    # is missing — it exits 1 with `Os { code: 2, kind: NotFound }` and takes
    # `syntax_help` down with it, so the agent loses the platform reference
    # over a missing directory. An existing empty one is fine: the server
    # starts and answers «indexing, try again» for what is not ready yet.
    # In local mode stand.sh makes it; in tunnel mode nobody did.
    mkdir -p "$home/project"
    # Владелец каталога — каждый раз, и отдельно от проверки ниже. Она
    # смотрит на САМ дом и пропускает починку, когда дом уже принадлежит
    # пользователю (а он принадлежит: дома лежат в томе и переживают
    # пересборку образа). Каталог же создаётся здесь, от root, и остаётся
    # root-овским — коннектор работает от пользователя и получает «не
    # создать /home/devN/project: Отказано в доступе». Содержимое не
    # трогаем: chown на сам каталог идемпотентен и чинит уже сломанные дома.
    chown "$u:$u" "$home/project"
    # Ownership is checked every time, not only on creation: root-owned files in
    # a user's home are exactly what breaks key-based login.
    [ "$(stat -c '%U' "$home")" = "$u" ] || chown -R "$u:$u" "$home"
    # git without a password prompt and with a real identity: the credential
    # helper is `store` in /etc/gitconfig, this is the file it reads. Written
    # only when absent — a person may have changed the password or the name.
    # Symlinks are refused: this runs as root inside the user's home.
    cred="$home/.git-credentials"
    if [ ! -e "$cred" ] && [ ! -L "$cred" ]; then
        printf 'http://%s:%s@localhost:3000\n' "$u" "$u" >"$cred"
        chown "$u:$u" "$cred" && chmod 600 "$cred"
    fi
    if [ ! -e "$home/.gitconfig" ] && [ ! -L "$home/.gitconfig" ]; then
        printf '[user]\n\tname = %s\n\temail = %s@dev1c.local\n' "$u" "$u" >"$home/.gitconfig"
        chown "$u:$u" "$home/.gitconfig"
    fi
done

# --- Editor settings from the image -------------------------------------------
# Home directories live on a volume and survive image updates — which also means
# a new image's settings never reach anyone who already has a home. The symptom
# is nasty: the image carries a fix, the environment keeps the old behaviour, and
# nothing anywhere says why.
#
# So our files are refreshed when they change in the image, tracked by a stamp
# of the shipped version in .config/.from-image/. A hand-edited file is
# replaced too, but kept as .bak next to it: these are our settings, not the
# user's, and the people this environment is for do not edit JSON — while we
# change it constantly. The same goes for the desktop: theme, fonts, wallpaper
# and window manager live in xfconf files in the home, and a theme change in
# the image would otherwise reach nobody with an existing home.
# This runs as root and writes into directories owned by the user. A symlink put
# in place of any of these paths would have root write wherever it points. Today
# the workstation accounts have passwordless sudo, so that grants nothing they
# could not do directly — but sudo is exactly the kind of thing dropped when a
# pilot turns into an installation, and then this would become a real hole. So
# the paths are refused when symlinked, a parent that resolves outside .config
# is refused, and the writes do not follow links.
SHIPPED_FILES="zed/settings.json zed/AGENTS.md zed/themes/1c.json
    xfce4/xfconf/xfce-perchannel-xml/xsettings.xml
    xfce4/xfconf/xfce-perchannel-xml/xfwm4.xml
    xfce4/xfconf/xfce-perchannel-xml/xfce4-desktop.xml
    gtk-3.0/settings.ini gtk-4.0/settings.ini"
for home in /home/*; do
    cfg="$home/.config"
    [ -d "$cfg/zed" ] || continue
    if [ -L "$home" ] || [ -L "$cfg" ]; then
        log "skipping $home: a symlinked path on the way to the settings"
        continue
    fi
    owner_u=$(stat -c '%U' "$home")
    owner_g=$(stat -c '%G' "$home")
    cfg_real=$(realpath "$cfg")
    stamps="$cfg/.from-image"
    [ -L "$stamps" ] && { log "skipping $home: $stamps is a symlink"; continue; }
    [ -d "$stamps" ] || install -d -o "$owner_u" -g "$owner_g" "$stamps"
    for f in $SHIPPED_FILES; do
        src="/etc/skel/.config/$f"
        dst="$cfg/$f"
        stamp="$stamps/$(echo "$f" | tr / _)"
        [ -f "$src" ] || continue
        parent=$(dirname "$dst")
        if [ -L "$dst" ] || [ -L "$dst.bak" ] || [ -L "$stamp" ] || [ -L "$parent" ]; then
            log "skipping $dst: symlink"
            continue
        fi
        # the parent is created below if missing; an existing one must resolve
        # inside .config (a link deeper in the path would lead root elsewhere)
        if [ -e "$parent" ]; then
            case "$(realpath "$parent")" in "$cfg_real"|"$cfg_real"/*) ;; *)
                log "skipping $dst: parent resolves outside $cfg"; continue ;; esac
        fi
        shipped=$(md5sum "$src" | cut -d' ' -f1)
        # The settings carry the model address and window substituted below;
        # a substituted placeholder is gone for good, so when the model
        # variables change the file has to come from the image again. The
        # stamp therefore covers the image copy AND the model variables: the
        # same image with a new model re-seeds settings.json (a hand edit in it
        # is lost then — the .bak keeps it), the other files are untouched by this.
        if [ "$f" = zed/settings.json ]; then
            shipped=$(printf '%s|%s|%s|%s|%s' "$shipped" "${MODEL_PROVIDER:-}" "${MODEL_NAME:-}" \
                "${LLAMA_API_URL:-}" "${CUSTOM_API_URL:-}" | md5sum | cut -d' ' -f1)
        fi
        [ "$(cat "$stamp" 2>/dev/null)" = "$shipped" ] && continue
        [ -f "$dst" ] && cp --remove-destination "$dst" "$dst.bak"
        # install after rm: writing over a path replaces the file itself rather
        # than following whatever it might point at
        rm -f "$dst"
        # the directories are the user's (their own themes go to zed/themes too)
        [ -d "$parent" ] || install -d -o "$owner_u" -g "$owner_g" "$parent"
        install -o "$owner_u" -g "$owner_g" -m 0644 "$src" "$dst"
        tmp=$(mktemp "$stamps/.tmp.XXXXXX") || continue
        echo "$shipped" > "$tmp"
        chown "$owner_u:$owner_g" "$tmp"
        mv -f "$tmp" "$stamp"
        log "$f updated from the image in $home"
    done
done

# --- Model server in Zed settings ---------------------------------------------
# Nobody edits JSON to point the environment at a model. The settings ship with
# placeholders; everything concrete is filled in here from the environment:
# The settings carry two providers and MODEL_PROVIDER says which one is live:
# "llama.cpp" is our own server (LLAMA_API_URL, LLAMACPP_API_KEY) and takes its
# context window from the server's own /props — nothing of ours is declared for
# it; "custom" is any other OpenAI-compatible server (CUSTOM_API_URL,
# CUSTOM_API_KEY), which has no /props, so the window stays declared there.
# MODEL_NAME names the model for both. No address at all means no model: the
# editor works, the agent stays silent, and dev1c-setup check says so.
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
        -e "s|CUSTOM_MAX_TOKENS_PLACEHOLDER|${CUSTOM_MAX_TOKENS:-53248}|" \
        -e "s|CUSTOM_MAX_OUTPUT_TOKENS_PLACEHOLDER|${CUSTOM_MAX_OUTPUT_TOKENS:-8192}|" \
        "$cfg"

    # Which of the two providers in the settings is the live one. dev1c-setup
    # says so with MODEL_PROVIDER; with nothing said, the one whose address
    # arrived wins, so a kit that predates this variable keeps working.
    provider="${MODEL_PROVIDER:-}"
    if [ -z "$provider" ] && [ -n "${LLAMA_API_URL:-}" ]; then
        provider="llama.cpp"
    fi
    if [ -z "$provider" ] && [ -n "${CUSTOM_API_URL:-}" ]; then
        provider="custom"
    fi
    # CUSTOM_MODEL — прежнее имя переменной: комплект и CI, собранные до
    # перехода на штатный провайдер, присылают его, и ломать их незачем.
    name="${MODEL_NAME:-${CUSTOM_MODEL:-model}}"

    # Both addresses are substituted whenever they are known: the file carries
    # both providers, and a placeholder left in the idle branch harms nobody —
    # it is never read — while a missing substitution in the live one shows up
    # as "the agent does not answer" with no reason given.
    if [ -n "${LLAMA_API_URL:-}" ]; then
        sed -i -e "s|LLAMA_API_URL_PLACEHOLDER|$LLAMA_API_URL|" "$cfg"
    fi
    if [ -n "${CUSTOM_API_URL:-}" ]; then
        sed -i -e "s|CUSTOM_API_URL_PLACEHOLDER|$CUSTOM_API_URL|" "$cfg"
    fi

    if [ -n "$provider" ]; then
        sed -i \
            -e "s|MODEL_PROVIDER_PLACEHOLDER|$provider|" \
            -e "s|MODEL_NAME_PLACEHOLDER|$name|g" \
            "$cfg"
        case "$provider" in
            llama.cpp) log "model: $name at ${LLAMA_API_URL:-?} (native provider) ($cfg)" ;;
            *)         log "model: $name at ${CUSTOM_API_URL:-?} (openai_compatible) ($cfg)" ;;
        esac
    else
        log "model: not configured — neither LLAMA_API_URL nor CUSTOM_API_URL is set ($cfg)"
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
