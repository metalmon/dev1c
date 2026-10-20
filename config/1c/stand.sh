#!/bin/sh
# The demo stand: the training platform inside the container serves the same
# ports the tunnel from a developer's machine would (see README, "Reaching the
# 1C platform"), so Zed, the skills and the agent rules do not know which of
# the two they talk to.
#
#   1c-stand start            root, from the image's CMD: one connector
#                             session per workstation (it publishes its own
#                             base and binds the 777N/778N listeners), then
#                             the base image's entrypoint
#   1c-stand mcp <N>          one mcp-1c on stdio for workstation dev<N>
#   1c-stand vrunner <N>      one vrunner-mcp on stdio for workstation dev<N>
#   1c-stand start1c          the 1C starter on the user's base list (the
#                             desktop icon; Предприятие and Конфигуратор
#                             are its own buttons)
#   1c-stand designer         the Configurator on the user's base, directly
#   1c-stand enterprise       the thick client on it (menu entry); holds the
#                             base's one Enterprise session while open
#   1c-stand project <repository-name> status|pull|push [message]
#                             the agent's own side of the tunnel-mode circle:
#                             the bridge's project_pull/project_push run on
#                             the developer's machine, not here, so this
#                             clones $HOME/project from the local Gitea on
#                             first use and then pulls/pushes it, as the
#                             calling workstation account (login dev<N>)
#   1c-stand publish <user> <port> <base dir> <out dir>
#                             writes the publication (default.vrd) and the
#                             Apache config for one base; only the image
#                             build uses it now, to check the demo base —
#                             workstations are published by the connector
#                             itself (host.publication), not by this script
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
LOG=/var/log/1c
STARTER=/opt/1cv8t/common/1cestartt
CONNECT=/opt/1c-connect/1c-connect
CONNECT_TOML=1c-connect.toml
# The shared kit built into the image (Dockerfile.1c). It is declared there
# as an ENV, which reaches `1c-stand seed` started by init; but a desktop
# session arrives through xrdp-sesman and PAM, where the image's ENV is not
# always visible — and that is exactly where the autostart entry
# (`1c-stand connect`) comes from. This default removes the dependency
# altogether: both seed and connect are right whatever the login environment
# did, and a value set from outside still wins.
: "${ONEC_CONNECT_KIT:=/opt/dev1c-kit}"
export ONEC_CONNECT_KIT

log() { printf '[1c-stand] %s\n' "$*"; }

# --- per-user pieces ---------------------------------------------------------
home_of() { getent passwd "$1" | cut -d: -f6; }

seed() { # runs as the workstation user; $HOME is theirs
    # Отметка «первый запуск пройден» для Zed. Без неё редактор встречает
    # гостя собственным онбордингом — «Finish setup», — который закрывает
    # наш и которого на демонстрации быть не должно: человек пришёл
    # смотреть 1С и агента, а не настраивать редактор.
    #
    # Отметка живёт в kv_store базы Zed, а не в настройках, поэтому её
    # нельзя положить в образ: дома рабочих мест — том, и образ их не
    # перекрывает. Ставим на входе, в seed, — тогда почин достаётся и
    # уже поднятым стендам.
    #
    # Таблицу создаём ту же, что создаёт сам Zed (его миграция
    # KeyValueStore шаг 0 — CREATE TABLE IF NOT EXISTS, проверено), и
    # вставляем без замены: если гость что-то уже настроил, не трогаем.
    zed_db=$HOME/.local/share/zed/db/0-stable/db.sqlite
    mkdir -p "$(dirname "$zed_db")"
    python3 - "$zed_db" <<'ZEDDB' || log "отметку первого запуска Zed поставить не удалось"
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
db.execute('CREATE TABLE IF NOT EXISTS kv_store (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT')
db.execute("INSERT OR IGNORE INTO kv_store (key, value) VALUES ('first_open', 'false')")
# Сам экран онбординга Zed хранит как вкладку рабочего пространства, и
# восстанавливает её при каждом запуске — поэтому «Finish setup» его и не
# закрывает насовсем. Убираем вкладку вместе с её записью в items, иначе
# в панели останется призрак без содержимого.
for table in ('onboarding_pages', 'welcome_pages'):
    try:
        ids = [r[0] for r in db.execute('SELECT item_id FROM %s' % table)]
    except sqlite3.OperationalError:
        continue
    for item in ids:
        db.execute('DELETE FROM items WHERE item_id = ?', (item,))
    db.execute('DELETE FROM %s' % table)
db.commit()
ZEDDB

    payload_present || {
        log "no platform in this container: /opt/1cv8t and /opt/1c-demo are empty (see provision.sh)" >&2
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
    # Refreshed when the image brings a different one, not only created when
    # missing: the entry carries the command line and the icon name, and a home
    # is a volume that outlives the image. Same rule as the 1c-connect copy below.
    for l in 1c-start; do
        d="$HOME/Desktop/$l.desktop"; s="/usr/share/applications/$l.desktop"
        if [ -d "$HOME/Desktop" ] && ! cmp -s "$s" "$d"; then
            cp "$s" "$d" && chmod +x "$d"
        fi
    done
    # Настройки терминала: копирование выделением и вставка на Ctrl+V.
    # Сверяем при каждом входе, как и ярлыки выше: файлы лежат в томе
    # домашнего каталога, и обновление образа их не трогает.
    for f in terminalrc accels.scm; do
        s="/usr/share/oktaplus/terminal/$f"; d="$HOME/.config/xfce4/terminal/$f"
        if [ -f "$s" ] && ! cmp -s "$s" "$d"; then
            mkdir -p "$HOME/.config/xfce4/terminal"
            cp "$s" "$d"
        fi
    done
    # Та же беда во встроенном терминале Zed: Ctrl+V там не вставлял вовсе.
    s=/usr/share/oktaplus/terminal/zed-keymap.json; d="$HOME/.config/zed/keymap.json"
    if [ -f "$s" ] && ! cmp -s "$s" "$d"; then
        mkdir -p "$HOME/.config/zed"
        cp "$s" "$d"
    fi
    # the launchers of the first days, superseded by the starter
    rm -f "$HOME/Desktop/1c-configurator.desktop" "$HOME/Desktop/1c-enterprise.desktop"
    # 1c-connect, if the image carries it. The tools it needs (mcp-1c,
    # vrunner, git) live in one place now, built once into the image —
    # /opt/dev1c-kit, see Dockerfile.1c — and ONEC_CONNECT_KIT (set there as
    # an image-wide ENV) points every workstation's 1c-connect at it, so
    # nothing here links anything for them any more: a workstation's home is
    # a volume, and a fixed path in the image is exactly what survives that.
    #
    # Still per-workstation, because this directory holds a copy of the
    # program itself: each account gets its own, refreshed when the image
    # brings a new build. It is no longer a kit of its own, though — the
    # tools and the mcp-1c binary are not linked here any more, they live in
    # the shared kit — so ONEC_CONNECT_KIT (defaulted at the top of this
    # script) must always point at /opt/dev1c-kit; Kit::discover's fallback
    # to this directory would find a kit with no tools in it. Its bookkeeping
    # (setup.log, install.state) no longer lands next to it either: on Linux
    # everything the connector writes goes to the state directory below.
    if [ -x "$CONNECT" ]; then
        mkdir -p "$HOME/dev1c"
        cmp -s "$CONNECT" "$HOME/dev1c/1c-connect" || cp "$CONNECT" "$HOME/dev1c/1c-connect"
        # The lock and pid files of the single-instance guard before it moved
        # to an advisory flock on the state directory itself: nothing creates
        # them any more, and a volume from those days still carries them.
        rm -f "$HOME/dev1c"/instance-*.lock "$HOME/dev1c"/instance-*.pid
        # State: the toml, the tools version stamp, the window's journal and
        # the publication all live here now (ops_host::tools_dir()), and the
        # single-instance lock is an advisory flock on this very directory —
        # never inside the shared kit, which is one read-only copy for every
        # workstation. $XDG_DATA_HOME/dev1c mirrors tools_dir()'s own
        # fallback to ~/.local/share/dev1c; an empty value falls back the
        # same way the program does (ops_host::env_dir).
        t="${XDG_DATA_HOME:-$HOME/.local/share}/dev1c"
        mkdir -p "$t"
        # A home is a volume and survives an image change, so carry over what
        # the person had next to the program before state moved out, rather
        # than writing the stand's defaults over it: otherwise an edited base
        # path, db_user, the chosen theme and autoconnect would quietly go
        # back to factory while the old files sat next door and confused
        # everyone.
        for f in "$CONNECT_TOML" install.state setup.log known_hosts; do
            if [ -f "$HOME/dev1c/$f" ] && [ ! -e "$t/$f" ]; then
                mv "$HOME/dev1c/$f" "$t/$f"
                log "1c-connect: $f перенесён в $t" >&2
            fi
        done
        if [ ! -e "$t/$CONNECT_TOML" ]; then
            u=$(id -un); n=${u#dev}
            # The server fields stay at their defaults: in local mode there is
            # no tunnel to raise. The base, its publication and the project
            # are the stand's own, the same ones the agent's tools see.
            # publication = "auto": the connector writes its own default.vrd
            # and apache.conf under $HOME/dev1c/pub and runs its own Apache —
            # the stand no longer publishes workstation bases itself (see
            # `start`). base_url matches what Cfg::base_url() computes for
            # that publication: the root of its own port, no "/base" segment.
            cat >"$t/$CONNECT_TOML" <<EOF
[host]
mode = "local"
workstation = "$u"
base_path = "$HOME/base"
base_url = "http://localhost:$((8080 + n))/hs/mcp-1c"
publication = "auto"
project = "$HOME/project"
db_user = "$DB_USER"
db_password = ""
EOF
            log "1c-connect: $t/$CONNECT_TOML written for $u" >&2
        fi
        # Tools: here the shared kit IS what is laid out. ops_host compares
        # the kit's tools.version (stamped at build time) with the state's
        # (written when `host.tools` copies the tools onto a machine) — and
        # without the second one the connector reports its tools missing and
        # the window says the agent will not see the base. Links, not copies:
        # /opt/dev1c-kit is one read-only copy for every account, and
        # duplicating ~100 MB per login would buy nothing. The names are the
        # ones ops_host::{oscript_exe,vrunner_lib,git_exe} resolve under
        # tools_dir().
        if [ -f "$ONEC_CONNECT_KIT/tools.version" ]; then
            # The links and the stamp go under one condition. If the tools
            # are really laid out (the person ran `1c-connect install`, so the
            # state holds copies, not links), touch neither: replacing just
            # the stamp would tell ops_host::tools_state that what is laid out
            # is exactly what the kit ships, and switch off Drifted — the very
            # case the two stamps are compared for.
            ours=yes
            for d in vrunner git; do
                [ ! -e "$t/$d" ] || [ -L "$t/$d" ] || ours=no
            done
            if [ "$ours" = yes ]; then
                for d in vrunner git; do
                    ln -sfn "$ONEC_CONNECT_KIT/$d" "$t/$d"
                done
                cp -f "$ONEC_CONNECT_KIT/tools.version" "$t/tools.version"
            fi
        fi
    fi
}

case "${1:-}" in
mcp)
    n=$2; export HOME; HOME=$(home_of "dev$n"); cd "$HOME"; seed
    # --base is the HTTP service itself, not the publication (as in
    # 1c-connect.toml). The connector publishes it at the root of its own
    # port now (host.publication), no "/base" segment.
    exec mcp-1c --base "http://127.0.0.1:$((8080 + n))/hs/mcp-1c"
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
project)
    # The agent's own side of the circle (design doc, "Как ветка попадает в
    # контейнер"): project_status/project_pull/project_push are bridge tools
    # and run on the developer's machine, not here, so the container's
    # project directory needs its own way to reach the same Gitea — this is
    # it. Run as the workstation account itself (not root), so $HOME is
    # already that account's; the repository name is the last path segment
    # of the project on the developer's machine, which this side cannot
    # guess — project_status (the bridge tool) names it for the agent.
    name=${2:?"usage: $0 project <repository-name> status|pull|push [message]"}
    sub=${3:?"usage: $0 project <repository-name> status|pull|push [message]"}
    msg=${4:-}
    u=$(id -un)
    dir="$HOME/project"
    # Password equals the login in the pilot — but it no longer rides in the
    # URL: a clean address would otherwise still leak it three ways (process
    # arguments, the stored "origin" remote in .git/config, and git's own
    # `fatal:` text on a failed clone/fetch, which this subcommand's own log
    # lines carry to support). Instead it goes on a header, on every call
    # that reaches Gitea, mirroring how the connector itself authenticates
    # (host.rs).
    #
    # That header itself stays out of the process table too: git reads
    # GIT_CONFIG_COUNT/GIT_CONFIG_KEY_n/GIT_CONFIG_VALUE_n from the
    # environment, so credential.helper and the Authorization header never
    # appear as a "-c" argument that `ps` on this machine could show anyone.
    url="http://127.0.0.1:3000/dev1c/$name.git"
    auth="Authorization: Basic $(printf '%s' "$u:$u" | base64 | tr -d '\n')"
    gitc() {
        GIT_CONFIG_COUNT=2 \
            GIT_CONFIG_KEY_0=credential.helper GIT_CONFIG_VALUE_0= \
            GIT_CONFIG_KEY_1=http.extraHeader GIT_CONFIG_VALUE_1="$auth" \
            git "$@"
    }
    if [ ! -d "$dir/.git" ]; then
        log "клонирую dev1c/$name из Gitea (http://127.0.0.1:3000), ветка $u"
        gitc clone --depth 1 -b "$u" "$url" "$dir"
    fi
    case "$sub" in
    status)
        branch=$(git -C "$dir" symbolic-ref --short HEAD)
        if ahead=$(git -C "$dir" rev-list --count "origin/$u..HEAD" 2>/dev/null); then :; else ahead=все; fi
        changed=$(git -C "$dir" status --porcelain | wc -l)
        log "ветка $branch"
        log "неотправленных коммитов: $ahead"
        log "изменено: $changed"
        if [ "$ahead" != 0 ] || [ "$changed" -gt 0 ]; then
            log "эти правки в базу загрузить нельзя, пока они не отправлены"
        fi
        ;;
    pull)
        log "забираю файлы из Gitea"
        gitc -C "$dir" pull --rebase origin "$u"
        ;;
    push)
        log "отправляю файлы в Gitea"
        git -c "user.name=$u" -c "user.email=$u@dev1c.local" -C "$dir" add -A
        git -c "user.name=$u" -c "user.email=$u@dev1c.local" -C "$dir" diff --cached --quiet \
            || git -c "user.name=$u" -c "user.email=$u@dev1c.local" -C "$dir" commit -q -m "${msg:-правка}"
        gitc -C "$dir" push origin "$u"
        ;;
    *)
        echo "usage: $0 project <repository-name> status|pull|push [message]" >&2
        exit 2
        ;;
    esac
    ;;
connect)
    # the autostart entry of the session: seed first (the toml and the copy
    # live in the home, which may be a fresh volume), then the tray.
    export HOME; HOME=$(home_of "$(id -un)"); seed
    [ -x "$HOME/dev1c/1c-connect" ] || { log "1c-connect is not in this image"; exit 0; }
    # its own log: an autostart entry has nowhere to print, and today the
    # Linux tray still falls over (GTK is not initialised in the process —
    # see the spec of the window), so the reason must survive the session
    # Its own name: the window's journal now lives in the state directory
    # ($XDG_DATA_HOME/dev1c/1c-connect.log, and the window truncates that one
    # at a megabyte), while this file only catches what the process printed
    # past the journal — no reason to give them one name.
    exec "$HOME/dev1c/1c-connect" --tray >>"$HOME/dev1c/autostart.log" 2>&1
    ;;
import)
    # Вторая половина круга второго акта: вернуть правку агента в базу.
    # Зовётся кнопкой онбординга, как и всё остальное, через 1c-stand —
    # путь к коннектору остаётся в одном месте, а не разъезжается по
    # сценарию и ярлыкам (тот же приём, что у connect и onboarding выше).
    #
    # Первая половина, раскладка конфигурации в файлы, в акт не входит:
    # она идёт около тринадцати минут, а акт обещает пять. На стенде
    # проект разложен заранее.
    export HOME; HOME=$(home_of "$(id -un)"); seed
    [ -x "$HOME/dev1c/1c-connect" ] || { log "1c-connect is not in this image"; exit 1; }
    exec "$HOME/dev1c/1c-connect" import
    ;;
onboarding)
    # Автозапуск при входе (onboarding-autostart.desktop, с --autostart)
    # и пункт меню «Знакомство со стендом» (onboarding.desktop, без
    # флага) зовут одну и ту же команду с разным хвостом — main.py решает
    # по --autostart, уважать ли отметку «закрыт в этом сеансе» (пункт
    # меню обязан работать всегда, даже после первого закрытия).
    # seed первым — значок 1c-start на столе и сама раскладка xfdesktop
    # нужны world.py, чтобы навести стрелку; то же, что у connect выше.
    shift
    export HOME; HOME=$(home_of "$(id -un)"); seed
    exec python3 /opt/onboarding/main.py "$@"
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
    mkdir -p "$LOG"
    if ! payload_present; then
        # Not a failure of the container: the desktop, Zed and Gitea are the
        # base image's and work without 1C. Exiting here would put the
        # container in a restart loop and hide the one line that matters.
        log "1C is not in this container: /opt/1cv8t and /opt/1c-demo are empty — the two payload volumes"
        log "fill them once:  ./provision.sh import <payload.tar.gz>"
        exec /usr/local/bin/entrypoint
    fi
    for n in 1 2 3; do
        u="dev$n"; id "$u" >/dev/null 2>&1 || continue
        # No connector started here, and no supervised loop: there is no
        # headless owner of these ports (see the spec). The ports live only
        # while a connector runs — the desktop autostart's "1c-connect --tray"
        # once the person logs in, or a hand-run "1c-stand serve" — and that
        # connector also raises the base's publication itself
        # (host.publication, $HOME/dev1c/pub, its own Apache). Starting one
        # here as well is exactly the old bug: two connectors fighting over
        # the same two ports. test-stand starts its own `serve` for the
        # check and stops only that one.
        log "$u: mcp-1c on $((7770 + n)), vrunner-mcp on $((7780 + n)), HTTP service on $((8080 + n)) — owned by whichever connector the person or test-stand runs"
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
    echo "usage: 1c-stand start | mcp <N> | vrunner <N> | start1c | designer | enterprise | connect | import | onboarding | seed | project <repository-name> status|pull|push [message] | publish ... | export-payload | import-payload [--force]" >&2
    exit 2
    ;;
esac
