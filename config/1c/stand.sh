#!/bin/sh
# The demo stand: the training platform inside the container serves the same
# ports the tunnel from a developer's machine would (see README, "Reaching the
# 1C platform"), so Zed, the skills and the agent rules do not know which of
# the two they talk to.
#
#   1c-stand start            root, from the image's CMD: one Apache per
#                             workstation, the 777N/778N listeners, then the
#                             base image's entrypoint
#   1c-stand mcp <N>          one mcp-1c on stdio for workstation dev<N>
#   1c-stand vrunner <N>      one vrunner-mcp on stdio for workstation dev<N>
#   1c-stand start1c          the 1C starter on the user's base list (the
#                             desktop icon; Предприятие and Конфигуратор
#                             are its own buttons)
#   1c-stand designer         the Configurator on the user's base, directly
#   1c-stand enterprise       the thick client on it (menu entry); holds the
#                             base's one Enterprise session while open
#   1c-stand publish <user> <port> <base dir> <out dir>
#                             writes the publication (default.vrd) and the
#                             Apache config for one base; `start` uses it per
#                             workstation, the image build uses it to check
#                             the demo base
#   1c-stand export-payload   stdout: a gzip tar of the platform and the
#                             template base, run in the payload image
#   1c-stand import-payload [--force]
#                             stdin: that tar, unpacked into the two volumes
#                             mounted at /opt/1cv8t and /opt/1c-demo
#
# The platform and the template base are NOT in the image: they come in two
# named volumes (dev1c-platform, dev1c-demo), filled once by import-payload —
# see Dockerfile.1c and provision.sh. Without them the container still gives a
# desktop, Zed and Gitea; `start` says what is missing and goes on.
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
# second Enterprise session on the same base — the client opened from the
# starter while the agent asks mcp-1c — gets "Infobase connections limitation
# reached" from the platform. The service itself keeps no session between
# calls (`reuseSessions="dontuse"` in the publication), so it blocks a person
# only for the length of a request.
set -eu

PLATFORM_VERSION=${PLATFORM_VERSION:-8.5.1.1150}
V8=/opt/1cv8/x86_64/$PLATFORM_VERSION
DEMO=/opt/1c-demo/base
# The two trees the volumes carry, and the one test for "is the stand usable".
# Both are checked, not just one: a half-filled pair is the state an interrupted
# import leaves behind, and it is worth naming rather than crashing later.
PAYLOAD_DIRS="opt/1cv8t opt/1c-demo"
payload_present() { [ -x "/opt/1cv8t/x86_64/$PLATFORM_VERSION/1cv8t" ] && [ -f "$DEMO/1Cv8.1CD" ]; }
# The demo base ships with the configuration's welcome wizard already done —
# otherwise the first person to open 1C is met by it, and finishing it creates
# an infobase user, which breaks every headless way in mid-demo. The wizard
# cannot be skipped and cannot be undone (deleting the user brings it back),
# so the base keeps the one user it made. Its password is empty, the only kind
# the training licence allows, and every way into the base carries the name:
# the list for the starter, the two launch commands, the publication, vrunner.
# A name with a space would need quoting in the starter's list — keep it one word.
# Empty on purpose means a base with no users at all (the image can be built
# that way), so `-` and not `:-`: an empty DEMO_DB_USER must stay empty.
DB_USER=${DEMO_DB_USER-Администратор}
IB_USR=${DB_USER:+Usr=$DB_USER;Pwd=;}   # tail of the publication's connection string
IB_N=${DB_USER:+/N$DB_USER}             # the key every interactive start carries
RUN=/run/1c
LOG=/var/log/1c
STARTER=/opt/1cv8t/common/1cestartt
CONNECT=/opt/1c-connect/1c-connect
CONNECT_TOML=1c-connect.toml

log() { printf '[1c-stand] %s\n' "$*"; }

# --- per-user pieces ---------------------------------------------------------
home_of() { getent passwd "$1" | cut -d: -f6; }

seed() { # runs as the workstation user; $HOME is theirs
    payload_present || {
        log "no platform in this container: the volumes dev1c-platform and dev1c-demo are empty (see provision.sh)" >&2
        return 1
    }
    # The file, not the directory: the Apache module creates the directory with
    # a lock file on its first (failed) look at a base that is not there yet.
    if [ ! -f "$HOME/base/1Cv8.1CD" ]; then
        mkdir -p "$HOME/base" && cp -a "$DEMO/." "$HOME/base/"
        log "demo base seeded into $HOME/base" >&2
    fi
    mkdir -p "$HOME/project"
    # The list of infobases for the starter: the person opens 1C the way they
    # are used to — one window with the base, and the buttons «1С:Предприятие»
    # and «Конфигуратор» in it — instead of two desktop icons wired to command
    # lines. Written once; a base added by hand stays.
    # The training starter is 1cestartt, and its list lives in the directory
    # of the same name: .1C/1cestartt, not .1C/1cestart. Written for whatever
    # starter the image carries, by the name of its binary.
    v8i="$HOME/.1C/$(basename "$STARTER")/ibases.v8i"
    if [ ! -e "$v8i" ]; then
        mkdir -p "$(dirname "$v8i")"
        # UTF-8 with a BOM and CRLF: without them the starter opens with an
        # empty list and asks to add a base, as if the file were not there.
        # AdditionalParameters: the starter appends them to whatever it
        # launches, Предприятие and Конфигуратор alike (checked by reading the
        # command line of the session it started). Scheduled jobs are off that
        # way without touching the base itself — in a file base they run inside
        # the session, and the configuration's internet support opens by asking
        # an update server that is unreachable here, then falls back to a ping
        # the platform cannot run on Linux.
        # /N<имя> rides along the same way: the base has one infobase user
        # with an empty password, so without it the person gets a login
        # dialog before the demo. Unquoted, the form the platform itself
        # writes when the welcome wizard restarts the client.
        { printf '\357\273\277'
          printf '[%s]\r\nConnect=File="%s";\r\nID=%s\r\nOrderInList=1\r\nFolder=/\r\nOrderInTree=1\r\nAdditionalParameters=/AllowExecuteScheduledJobs -Off %s\r\n' \
            "Управление нашей фирмой (учебная)" "$HOME/base" "$(id -un)-base" "$IB_N"
        } >"$v8i"
        log "1C: $v8i written" >&2
    fi
    # the starter on the desktop, next to Zed and Gitea (same way as the base
    # image puts those there: a copy with the executable bit)
    for l in 1c-start; do
        d="$HOME/Desktop/$l.desktop"
        if [ -d "$HOME/Desktop" ] && [ ! -e "$d" ]; then
            cp "/usr/share/applications/$l.desktop" "$d" && chmod +x "$d"
        fi
    done
    # the launchers of the first days, superseded by the starter
    rm -f "$HOME/Desktop/1c-configurator.desktop" "$HOME/Desktop/1c-enterprise.desktop"
    # 1c-connect, if the image carries it. Its kit is the directory of the
    # binary (common::kit), and every workstation needs its own parameters —
    # so each gets its own copy next to its own toml, refreshed when the image
    # brings a new build.
    if [ -x "$CONNECT" ]; then
        mkdir -p "$HOME/dev1c"
        cmp -s "$CONNECT" "$HOME/dev1c/1c-connect" || cp "$CONNECT" "$HOME/dev1c/1c-connect"
        if [ ! -e "$HOME/dev1c/$CONNECT_TOML" ]; then
            u=$(id -un); n=${u#dev}
            # The server fields stay at their defaults: in local mode there is
            # no tunnel to raise. The base, its publication and the project
            # are the stand's own, the same ones the agent's tools see.
            cat >"$HOME/dev1c/$CONNECT_TOML" <<EOF
[host]
mode = "local"
workstation = "$u"
base_path = "$HOME/base"
base_url = "http://127.0.0.1:$((8080 + n))/base/hs/mcp-1c"
project = "$HOME/project"
db_user = "$DB_USER"
db_password = ""
EOF
            log "1c-connect: $HOME/dev1c/$CONNECT_TOML written for $u" >&2
        fi
    fi
}

case "${1:-}" in
mcp)
    n=$2; export HOME; HOME=$(home_of "dev$n"); cd "$HOME"; seed
    # --base is the HTTP service itself, not the publication (as in 1c-connect.toml)
    exec mcp-1c --base "http://127.0.0.1:$((8080 + n))/base/hs/mcp-1c"
    ;;
vrunner)
    n=$2; export HOME; HOME=$(home_of "dev$n"); cd "$HOME"; seed
    # Its own virtual display with its own cookie, as this user: the
    # Configurator vrunner launches in batch mode needs one, nobody looks at it.
    cd "$HOME/project"
    # vanessa-runner reads the infobase credentials from the environment when
    # a command does not carry them (`&ВОкружении("VRUNNER_DBUSER")`), so the
    # agent's tool calls need not know the name.
    VRUNNER_DBUSER=$DB_USER VRUNNER_DBPWD=
    export VRUNNER_DBUSER VRUNNER_DBPWD
    exec xvfb-run -a vrunner-mcp
    ;;
start1c)
    # The starter: it shows the base list and the two buttons, and it is what
    # a 1C developer knows. Its sessions carry the keys from the list's
    # AdditionalParameters (see `seed`), so scheduled jobs are off there too.
    seed
    exec "$STARTER"
    ;;
designer)
    seed
    exec "$V8/1cv8" DESIGNER /F "$HOME/base" $IB_N /DisableStartupDialogs /DisableStartupMessages
    ;;
enterprise)
    seed
    # Scheduled jobs off: in a file base they run inside this very session, and
    # the configuration's internet-support subsystem starts by asking
    # update-api.1c.ru whether there is an update. Here that host is
    # unreachable, the subsystem falls back to diagnostics — `ping ya.ru`
    # through ЗапуститьПриложение, which the platform cannot launch on Linux
    # ("не удалось распознать схему URI") — and the person gets a dialog about
    # a URI scheme instead of the demo. A stand has nothing to update anyway.
    exec "$V8/1cv8" ENTERPRISE /F "$HOME/base" $IB_N /AllowExecuteScheduledJobs -Off \
        /DisableStartupDialogs /DisableStartupMessages
    ;;
seed)
    # runuser without -l keeps root's HOME: take the caller's own
    export HOME; HOME=$(home_of "$(id -un)"); seed
    ;;
connect)
    # the autostart entry of the session: seed first (the toml and the copy
    # live in the home, which may be a fresh volume), then the tray.
    export HOME; HOME=$(home_of "$(id -un)"); seed
    [ -x "$HOME/dev1c/1c-connect" ] || { log "1c-connect is not in this image"; exit 0; }
    # its own log: an autostart entry has nowhere to print, and today the
    # Linux tray still falls over (GTK is not initialised in the process —
    # see the spec of the window), so the reason must survive the session
    exec "$HOME/dev1c/1c-connect" --tray >>"$HOME/dev1c/1c-connect.log" 2>&1
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
		ib="File=$base;$IB_USR">
	<ws pointEnableCommon="false"/>
	<!-- reuseSessions="dontuse", and on the service itself: the container
	     element does not take these attributes, the platform refuses the
	     whole file («Ошибка при разборе»). The training edition allows one
	     Enterprise session per base, and a pooled session of the HTTP
	     service holds it between the agent's calls, so the person opening
	     1C is told the connection limit is reached. Without the pool the
	     service takes the base only for the length of a request. -->
	<httpServices publishByDefault="true" publishExtensionsByDefault="true">
		<service name="MCPService" rootUrl="mcp-1c" enable="true"
				reuseSessions="dontuse" sessionMaxAge="1" poolSize="1" poolTimeout="1"/>
	</httpServices>
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
# One worker process: the 1C module keeps a session pool per process, and a
# second process means a second Enterprise session on the base — the one the
# training edition refuses ("предельное количество подключений к ИБ").
ServerLimit 1
StartServers 1
ThreadsPerChild 16
MinSpareThreads 16
MaxSpareThreads 16
MaxRequestWorkers 16
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
export-payload)
    # Run in the payload image (`docker build --target payload`). Paths are
    # relative to / so that the import lands straight in the mounted volumes,
    # and ownership is numeric: the archive crosses images.
    payload_present || { log "this image has no payload: build with --target payload"; exit 1; }
    exec tar -C / --numeric-owner -czf - $PAYLOAD_DIRS
    ;;
import-payload)
    # Run in the stand image with both volumes mounted; stdin is what
    # export-payload wrote. An already filled pair is left alone unless
    # --force: re-importing over a running stand would swap the template base
    # under the workstations that were cloned from it.
    [ "$(id -u)" = 0 ] || { log "import-payload runs as root"; exit 1; }
    if payload_present && [ "${2:-}" != "--force" ]; then
        log "the platform and the demo base are already here; --force to replace them"
        exit 0
    fi
    tar -C / --numeric-owner -xzf -
    payload_present || { log "import-payload: the archive did not hold $PAYLOAD_DIRS"; exit 1; }
    log "payload in place: $(tr '\n' ' ' </opt/1c-demo/payload.conf 2>/dev/null)"
    ;;
start)
    mkdir -p "$RUN" "$LOG"
    if ! payload_present; then
        # Not a failure of the container: the desktop, Zed and Gitea are the
        # base image's and work without 1C. Exiting here would put the
        # container in a restart loop and hide the one line that matters.
        log "1C is not in this container: the volumes dev1c-platform and dev1c-demo are empty"
        log "fill them once:  ./provision.sh import <payload.tar.gz>"
        exec /usr/local/bin/entrypoint
    fi
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
    # Seed the bases ahead of the first use: a УНФ base is 1.9 GB, and the
    # first agent call would otherwise wait for the copy. In the background,
    # after the base entrypoint has created the homes (it does so only for a
    # missing directory, hence not before it). The lazy seed above stays as
    # the fallback.
    ( for n in 1 2 3; do
          u="dev$n"; id "$u" >/dev/null 2>&1 || continue
          until [ -d "$(home_of "$u")" ]; do sleep 2; done
          runuser -u "$u" -- /usr/local/bin/1c-stand seed
      done ) >"$LOG/seed.log" 2>&1 &
    exec /usr/local/bin/entrypoint
    ;;
*)
    echo "usage: 1c-stand start | mcp <N> | vrunner <N> | start1c | designer | enterprise | connect | seed | publish ... | export-payload | import-payload [--force]" >&2
    exit 2
    ;;
esac
