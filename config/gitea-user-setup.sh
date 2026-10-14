#!/bin/sh
# Set up Gitea access for the current user.
#
# Runs inside their session rather than as a service: the files land in the home
# directory, and the Gitea service runs under its own account which cannot write
# there.
#
# It checks for the files rather than for "first login": home directories live on
# a volume and survive image updates, but if the volume is ever recreated the
# settings would be gone and git would suddenly start asking for a password.
set -eu

GITEA_URL="${GITEA_URL:-http://localhost:3000}"
USER_NAME="$(id -un)"
GITEA_BIN=/opt/gitea/gitea
GITEA_CONF=/var/lib/gitea/conf/app.ini

# Wait for the service: logging in right after the container starts may catch it
# unready, and the setup would silently do nothing.
i=0
while [ $i -lt 30 ]; do
    curl -sf -o /dev/null "$GITEA_URL/api/v1/version" 2>/dev/null && break
    i=$((i + 1))
    sleep 1
done

# git push and pull without a password prompt
if [ ! -f "$HOME/.git-credentials" ]; then
    printf 'http://%s:%s@localhost:3000\n' "$USER_NAME" "$USER_NAME" > "$HOME/.git-credentials"
    chmod 600 "$HOME/.git-credentials"
fi

# Name and email: the placeholders from /etc/gitconfig are replaced with the real
# user, otherwise every commit in the team comes from one author.
git config --global user.name "$USER_NAME" 2>/dev/null || true
git config --global user.email "$USER_NAME@oktaplus.local" 2>/dev/null || true

# A token for tea, so `tea pr` and `tea issue` work right away without
# `login add`. Only Gitea itself can issue one, which requires access to its
# database — available to the image administrator, not to the user. Hence sudo.
#
# The existence of app.ini is deliberately NOT checked here: the Gitea config is
# 600 gitea:gitea inside a 700 directory, so `test -f` run as the user is always
# false — even when the file is there, while the command below works fine through
# sudo. That check silently disabled the whole tea setup. The token itself tells
# us whether it worked.
if [ ! -f "$HOME/.config/tea/config.yml" ]; then
    TOKEN=$(sudo -n -u gitea "$GITEA_BIN" --config "$GITEA_CONF" \
        admin user generate-access-token \
        --username "$USER_NAME" \
        --token-name "workstation-$(date +%s)" \
        --scopes write:repository,write:issue,write:user,read:organization \
        --raw 2>/dev/null || true)

    if [ -n "${TOKEN:-}" ]; then
        mkdir -p "$HOME/.config/tea"
        cat > "$HOME/.config/tea/config.yml" <<EOF
logins:
  - name: local
    url: $GITEA_URL
    token: $TOKEN
    default: true
    user: $USER_NAME
preferences:
  editor: true
  flat_dirs: true
EOF
        chmod 600 "$HOME/.config/tea/config.yml"
    fi
fi

# Working copies of the examples, kept apart from ~/proj where real dumps live.
# A clone rather than a file copy: an example has a remote, so it can be edited,
# committed and pushed, with the result visible in the web interface.
EXAMPLES="$HOME/examples"
mkdir -p "$EXAMPLES"
for repo in /opt/seed/*; do
    [ -d "$repo" ] || continue
    name=$(basename "$repo")
    [ -d "$EXAMPLES/$name" ] && continue
    git clone -q "$GITEA_URL/${GITEA_ADMIN:-dev1}/${name}.git" "$EXAMPLES/$name" 2>/dev/null || true
done
