#!/bin/sh
# The demo stand: the training platform inside the container serves the same
# ports the tunnel from a developer's machine would (see README, "Reaching the
# 1C platform"), so Zed, the skills and the agent rules do not know which of
# the two they talk to.
#
#   1c-stand start            root, from the image's CMD: Xvfb, one Apache per
#                             workstation, the 777N/778N listeners, then the
#                             base image's entrypoint
#   1c-stand mcp <N>          one mcp-1c on stdio for workstation dev<N>
#   1c-stand vrunner <N>      one vrunner-mcp on stdio for workstation dev<N>
#   1c-stand designer         the Configurator on the user's base (menu entry)
#   1c-stand publish <user> <port> <base dir> <out dir>
#                             writes the publication (default.vrd) and the
#                             Apache config for one base; `start` uses it per
#                             workstation, the image build uses it to check
#                             the demo base
#
# Everything about a workstation's base happens as that user: the base lives
# in the home (a volume, survives image updates), the Configurator in the RDP
# session, the Apache workers and both MCP servers all run as dev<N>. One
# owner, no group juggling with www-data — the spike spent an hour on that.
#
# The base and the project directory are seeded lazily, on the first use, not
# at container start: the base entrypoint creates missing homes from /etc/skel
# only when the directory does not exist yet, and seeding first would leave a
# home with a base and no settings.
#
# The listeners are plain loopback TCP without authentication, like the tunnel
# ports they stand in for: inside the container any account can open any
# workstation's port. The pilot's accounts have passwordless sudo, so this adds
# nothing they lack; UNIX sockets per user would need Zed in the base image to
# stop using `nc localhost 777N`. Revisit together with sudo when a pilot
# becomes an installation.
#
# The training edition allows one Enterprise session per base. Each workstation
# has its own base and its own Apache (its own session pool), so the three
# never queue on each other; the Configurator coexists with the session. A
# second Enterprise session on the same base — the thick client opened from the
# menu while the agent asks mcp-1c — gets "Infobase connections limitation
# reached" from the platform; the HTTP-service session lingers sessionMaxAge
# (20 s) after the agent's last call.
set -eu

V8=/opt/1cv8/x86_64/${PLATFORM_VERSION:-8.5.1.1150}
DEMO=/opt/1c-demo/base
RUN=/run/1c
LOG=/var/log/1c
DISPLAY_VFB=:99

log() { printf '[1c-stand] %s\n' "$*"; }

# --- per-user pieces ---------------------------------------------------------
home_of() { getent passwd "$1" | cut -d: -f6; }

seed() { # runs as the workstation user; $HOME is theirs
    # The file, not the directory: the Apache module creates the directory with
    # a lock file on its first (failed) look at a base that is not there yet.
    if [ ! -f "$HOME/base/1Cv8.1CD" ]; then
        mkdir -p "$HOME/base" && cp -a "$DEMO/." "$HOME/base/"
        log "demo base seeded into $HOME/base" >&2
    fi
    mkdir -p "$HOME/project"
}

case "${1:-}" in
mcp)
    n=$2; export HOME; HOME=$(home_of "dev$n"); cd "$HOME"; seed
    # --base is the HTTP service itself, not the publication (as in 1c-connect.toml)
    exec mcp-1c --base "http://127.0.0.1:$((8080 + n))/base/hs/mcp-1c"
    ;;
vrunner)
    n=$2; export HOME; HOME=$(home_of "dev$n"); cd "$HOME"; seed
    export DISPLAY=$DISPLAY_VFB
    cd "$HOME/project"
    exec vrunner-mcp
    ;;
designer)
    seed
    exec "$V8/1cv8" DESIGNER /F "$HOME/base" /DisableStartupDialogs /DisableStartupMessages
    ;;
publish)
    u=$2; port=$3; base=$4; d=$5; mkdir -p "$d/pub"
    # The publication: what webinstt writes, with the HTTP services on and
    # nothing else. Written on every start — nothing in it is anybody's edit.
    cat >"$d/pub/default.vrd" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<point xmlns="http://v8.1c.ru/8.2/virtual-resource-system"
		xmlns:xs="http://www.w3.org/2001/XMLSchema"
		xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
		base="/base"
		ib="File=$base;">
	<ws pointEnableCommon="false"/>
	<httpServices publishByDefault="true" publishExtensionsByDefault="true"/>
	<standardOdata enable="false"/>
</point>
EOF
    # Its own Apache rather than a vhost in the shared one: the workers must
    # run as the base's owner, and a process has one uid. The module block is
    # what webinstt generates.
    cat >"$d/apache.conf" <<EOF
ServerRoot "/etc/apache2"
ServerName localhost
DefaultRuntimeDir $d
PidFile $d/apache.pid
User $u
Group $u
Listen 127.0.0.1:$port
ErrorLog $d/apache.log
LogLevel warn
IncludeOptional mods-enabled/*.load
IncludeOptional mods-enabled/*.conf
LoadModule _1cws_module "$V8/wsap24t.so"
Alias "/base" "$d/pub/"
<Directory "$d/pub/">
    AllowOverride All
    Options None
    Require all granted
    SetHandler 1c-application
    ManagedApplicationDescriptor "$d/pub/default.vrd"
</Directory>
EOF
    chown -R "$u:$u" "$d"
    ;;
start)
    mkdir -p "$RUN" "$LOG"
    Xvfb "$DISPLAY_VFB" -screen 0 1280x1024x24 -ac -nolisten tcp >"$LOG/xvfb.log" 2>&1 &
    for n in 1 2 3; do
        u="dev$n"; id "$u" >/dev/null 2>&1 || continue
        d="$RUN/$u"
        "$0" publish "$u" $((8080 + n)) "$(home_of "$u")/base" "$d"
        apache2 -f "$d/apache.conf" -k start
        # One MCP server per connection, as the workstation user; nc -N on the
        # Zed side closes the socket at EOF and the server exits with it.
        socat "TCP4-LISTEN:$((7770 + n)),bind=127.0.0.1,reuseaddr,fork" \
              "EXEC:/usr/local/bin/1c-stand mcp $n,su=$u" >"$LOG/$u-mcp.log" 2>&1 &
        socat "TCP4-LISTEN:$((7780 + n)),bind=127.0.0.1,reuseaddr,fork" \
              "EXEC:/usr/local/bin/1c-stand vrunner $n,su=$u" >"$LOG/$u-vrunner.log" 2>&1 &
        log "$u: HTTP service 127.0.0.1:$((8080 + n))/base, mcp-1c on $((7770 + n)), vrunner-mcp on $((7780 + n))"
    done
    exec /usr/local/bin/entrypoint
    ;;
*)
    echo "usage: 1c-stand start | mcp <N> | vrunner <N> | designer" >&2
    exit 2
    ;;
esac
