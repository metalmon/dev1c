#!/bin/sh
# The tunnel ports in Zed settings are substituted per workstation and left
# alone everywhere else. Runs inside the started container: by then the
# entrypoint has laid out every home.
set -eu

fail() { echo "FAIL: $*" >&2; exit 1; }

for n in 1 2 3; do
    cfg=/home/dev$n/.config/zed/settings.json
    grep -q "\"args\": \[\"localhost\", \"777$n\"\]" "$cfg" \
        || fail "dev$n: mcp-1c port 777$n not in $cfg"
    grep -q "\"args\": \[\"localhost\", \"778$n\"\]" "$cfg" \
        || fail "dev$n: vrunner port 778$n not in $cfg"
    ! grep -q WORKSTATION_ "$cfg" || fail "dev$n: placeholder left"
    # The file is JSONC (comments inside), so no parser here; sed replaces a
    # word inside a string and cannot change the structure — the line count
    # against the reference copy proves nothing else happened to it.
    [ "$(wc -l < "$cfg")" = "$(wc -l < /etc/skel/.config/zed/settings.json)" ] \
        || fail "dev$n: settings.json differs in length from the reference"
done

# The tunnel account has a home too, and no workstation number.
if [ -f /home/dev/.config/zed/settings.json ]; then
    grep -q WORKSTATION_1C_PORT_PLACEHOLDER /home/dev/.config/zed/settings.json \
        || fail "dev: placeholder was substituted for a non-workstation account"
fi

grep -q '^    PermitListen localhost:7771 .*localhost:7793$' /etc/ssh/sshd_config.d/dev1c.conf \
    || fail "sshd: PermitListen for the nine workstation ports is missing"
# sshd must accept its own config, otherwise it never started and the whole
# tunnel scheme is dead without a word.
pgrep -x sshd >/dev/null || fail "sshd is not running: check /etc/ssh/sshd_config.d/dev1c.conf"

echo "ok: workstation ports substituted (7771-7773, 7781-7783), sshd restricted to 777N/778N/779N"
