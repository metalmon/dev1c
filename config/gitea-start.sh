#!/bin/sh
# Start the local Gitea, configure access and seed it on first run.
#
# Configuration and data are created once and then left alone: otherwise changes
# made through the web interface would be overwritten on every session login.
set -eu

GITEA_HOME="${GITEA_HOME:-/var/lib/gitea}"
GITEA_CONF="$GITEA_HOME/conf/app.ini"
GITEA_LOG="$GITEA_HOME/log/start.log"
SEED_DIR="${SEED_DIR:-/opt/seed}"
GITEA_BIN=/opt/gitea/gitea

GITEA_ADMIN="${GITEA_ADMIN:-dev1}"
GITEA_PASSWORD="${GITEA_PASSWORD:-dev1}"
GITEA_USERS="${GITEA_USERS:-dev1 dev2 dev3}"

mkdir -p "$GITEA_HOME"/conf "$GITEA_HOME"/log "$GITEA_HOME"/data "$GITEA_HOME"/repos \
         "$GITEA_HOME"/custom/templates/custom

# A banner with the credentials on every page: these users meet git for the first
# time, and a password known only from documentation is guaranteed to get lost.
# Copied every time rather than on first run only: the template may be updated
# together with the image.
if [ -f /opt/gitea/banner.tmpl ]; then
    cp /opt/gitea/banner.tmpl "$GITEA_HOME/custom/templates/custom/body_outer_pre.tmpl"
fi

if pgrep -u "$(id -u)" -f 'gitea web' >/dev/null 2>&1; then
    echo "gitea is already running" >&2
    exit 0
fi

FIRST_RUN=0
if [ ! -f "$GITEA_CONF" ]; then
    FIRST_RUN=1
    # SQLite instead of a separate DBMS: for a pilot of a few people it is more
    # than enough, and deploying and maintaining PostgreSQL for that is pointless.
    # INSTALL_LOCK=true skips the setup wizard: everything is defined here, and
    # otherwise the first visitor would be greeted by a database configuration
    # form and get lost.
    #
    # APP_NAME stays Russian: it is the interface of this environment.
    cat > "$GITEA_CONF" <<EOF
APP_NAME = OktaPlus — репозитории 1С
RUN_USER = $(id -un)
RUN_MODE = prod
WORK_PATH = $GITEA_HOME

[server]
PROTOCOL = http
DOMAIN = localhost
HTTP_ADDR = 0.0.0.0
HTTP_PORT = 3000
ROOT_URL = http://localhost:3000/
DISABLE_SSH = false
START_SSH_SERVER = true
SSH_PORT = 2222
SSH_LISTEN_PORT = 2222
APP_DATA_PATH = $GITEA_HOME/data
LFS_START_SERVER = true

[database]
DB_TYPE = sqlite3
PATH = $GITEA_HOME/data/gitea.db

[repository]
ROOT = $GITEA_HOME/repos
DEFAULT_BRANCH = main
# Pushing to a non-existent repository creates it: for a first encounter with git
# this removes the whole "create a repository in the web UI first" step
ENABLE_PUSH_CREATE_USER = true
ENABLE_PUSH_CREATE_ORG = true

[service]
DISABLE_REGISTRATION = false
REQUIRE_SIGNIN_VIEW = false
DEFAULT_KEEP_EMAIL_PRIVATE = true

[actions]
# For future CI: enabled now so nothing needs reconfiguring later
ENABLED = true

[ui]
DEFAULT_THEME = gitea-dark

[security]
INSTALL_LOCK = true
# The password equals the system account (login = password) — one pair for
# everything, otherwise a pilot user gets confused between logging into the
# environment and logging into Gitea. Gitea's own requirements (6+ characters,
# complexity check) are relaxed for that; the environment is closed and already
# behind the RDP password.
MIN_PASSWORD_LENGTH = 1
PASSWORD_COMPLEXITY = off
PASSWORD_CHECK_PWN = false

[log]
MODE = file
LEVEL = info
ROOT_PATH = $GITEA_HOME/log
EOF
fi

export GITEA_WORK_DIR="$GITEA_HOME"

# Accounts are created before the server starts: the command works against the
# database directly.
if [ "$FIRST_RUN" = "1" ]; then
    "$GITEA_BIN" --config "$GITEA_CONF" migrate >>"$GITEA_LOG" 2>&1 || true
    # One account per workstation, password equal to the login — the same pair as
    # for the system login. Separate passwords for the environment and for the
    # repositories reliably confuse people during a pilot.
    for u in $GITEA_USERS; do
        admin_flag=""
        [ "$u" = "$GITEA_ADMIN" ] && admin_flag="--admin"
        # shellcheck disable=SC2086
        "$GITEA_BIN" --config "$GITEA_CONF" admin user create \
            --username "$u" --password "$u" \
            --email "$u@oktaplus.local" $admin_flag --must-change-password=false \
            >>"$GITEA_LOG" 2>&1 || true
    done
fi

"$GITEA_BIN" web --config "$GITEA_CONF" >>"$GITEA_LOG" 2>&1 &
GITEA_PID=$!

# Wait for HTTP: the organisation and the seeding below talk to the API.
i=0
while [ $i -lt 60 ]; do
    if curl -sf -o /dev/null "http://localhost:3000/api/v1/version" 2>/dev/null; then
        break
    fi
    i=$((i + 1))
    sleep 1
done

# --- The developers' organisation ----------------------------------------------
# One repository per base lives under dev1c/<base>, every workstation an owner:
# push-to-create then works for any of them, and 1c-connect's first
# project_push creates the repository. Idempotent and run every start: the org
# is created once, owners are re-added (a PUT for a member is a no-op).
api() { # $1 = method, $2 = path, $3 = json body (optional) → HTTP status
    if [ -n "${3:-}" ]; then
        curl -s -o /dev/null -w '%{http_code}' -X "$1" -u "${GITEA_ADMIN}:${GITEA_PASSWORD}" \
            -H 'Content-Type: application/json' -d "$3" "http://localhost:3000/api/v1$2" 2>/dev/null || true
    else
        curl -s -o /dev/null -w '%{http_code}' -X "$1" -u "${GITEA_ADMIN}:${GITEA_PASSWORD}" \
            "http://localhost:3000/api/v1$2" 2>/dev/null || true
    fi
}
if [ "$(api GET /orgs/dev1c)" != "200" ]; then
    api POST /orgs '{"username":"dev1c","full_name":"Разработка 1С","visibility":"public","repo_admin_change_team_access":true}' >/dev/null
    echo "organisation dev1c created" >>"$GITEA_LOG"
fi
owners=$(curl -sf -u "${GITEA_ADMIN}:${GITEA_PASSWORD}" "http://localhost:3000/api/v1/orgs/dev1c/teams" 2>/dev/null \
    | jq -r '.[] | select(.name=="Owners") | .id' 2>/dev/null | head -1)
if [ -n "$owners" ]; then
    for u in $GITEA_USERS; do
        api PUT "/teams/$owners/members/$u" >/dev/null
    done
else
    echo "organisation dev1c: Owners team not found, members not added" >>"$GITEA_LOG"
fi

# --- Seeding repositories (first run only) ------------------------------------
if [ "$FIRST_RUN" = "1" ] && [ -d "$SEED_DIR" ]; then
    for repo in "$SEED_DIR"/*; do
        [ -d "$repo" ] || continue
        name=$(basename "$repo")
        work="$(mktemp -d)"
        cp -r "$repo"/. "$work"/ 2>/dev/null || continue
        rm -rf "$work/.git"
        (
            cd "$work"
            git init -q -b main
            git add -A
            # Author comes from the placeholder in /etc/gitconfig, no need to
            # change it. The commit message is Russian: it is content the users
            # read in the web interface.
            git commit -qm "Исходный импорт: $name" || exit 0
            git remote add origin \
                "http://${GITEA_ADMIN}:${GITEA_PASSWORD}@localhost:3000/${GITEA_ADMIN}/${name}.git"
            git push -q -u origin main 2>/dev/null \
                && echo "repository $name pushed" >>"$GITEA_LOG"
            # A push creates the repository as private, and it stays invisible on
            # the front page without logging in. For a first encounter the
            # opposite is needed: log in and immediately see what to work with.
            curl -sf -X PATCH \
                -u "${GITEA_ADMIN}:${GITEA_PASSWORD}" \
                -H 'Content-Type: application/json' \
                -d '{"private": false}' \
                "http://localhost:3000/api/v1/repos/${GITEA_ADMIN}/${name}" \
                >/dev/null 2>&1 || true
        ) || true
        rm -rf "$work"
    done
fi

wait "$GITEA_PID"
