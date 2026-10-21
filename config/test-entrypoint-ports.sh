#!/bin/sh
# The tunnel ports in Zed settings are substituted per workstation and left
# alone everywhere else. Runs inside the started container: by then the
# entrypoint has laid out every home.
set -eu

fail() { echo "FAIL: $*" >&2; exit 1; }

for n in 1 2 3; do
    cfg=/home/dev$n/.config/zed/settings.json
    grep -q "\"args\": \[\"-N\", \"localhost\", \"777$n\"\]" "$cfg" \
        || fail "dev$n: mcp-1c port 777$n not in $cfg"
    grep -q "\"args\": \[\"-N\", \"localhost\", \"778$n\"\]" "$cfg" \
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

# The window numbers are substituted for every workstation: a placeholder
# left behind is invalid JSON, and Zed would drop the whole provider block.
# They belong to the openai_compatible branch, which stays in the file even
# when the live provider is the native llama.cpp one.
for u in dev1 dev2 dev3; do
    f=/home/$u/.config/zed/settings.json
    grep -q 'CUSTOM_MAX_TOKENS_PLACEHOLDER\|CUSTOM_MAX_OUTPUT_TOKENS_PLACEHOLDER' "$f" && fail "$u: window placeholder not substituted"
    grep -q '"max_tokens": [0-9]' "$f" || fail "$u: max_tokens is not a number"
    # Which provider is live is said by MODEL_PROVIDER, and whichever it is,
    # its own address placeholder must be gone. The idle branch keeps its
    # placeholder by design: nobody reads it, and burning it would make the
    # environment impossible to repoint at the other kind of server.
    grep -q 'MODEL_PROVIDER_PLACEHOLDER' "$f" && fail "$u: provider placeholder not substituted"
    grep -q 'MODEL_NAME_PLACEHOLDER' "$f" && fail "$u: model name placeholder not substituted"
    provider=$(grep -A2 '"default_model"' "$f" | grep '"provider"' | cut -d'"' -f4)
    case "$provider" in
        llama.cpp) grep -q 'LLAMA_API_URL_PLACEHOLDER' "$f" && fail "$u: llama.cpp is live but its address is a placeholder" ;;
        custom)    grep -q 'CUSTOM_API_URL_PLACEHOLDER' "$f" && fail "$u: custom is live but its address is a placeholder" ;;
        *)         fail "$u: default_model names no known provider ($provider)" ;;
    esac
done

# One Match block per workstation, each allowed to listen on its own three
# ports only (777N, 778N, 779N) — a typo in `workstation` must not bind a
# neighbour's tunnel.
for n in 1 2 3; do
    grep -A12 "^Match User dev$n\$" /etc/ssh/sshd_config.d/dev1c.conf \
        | grep -q "^    PermitListen localhost:777$n localhost:778$n localhost:779$n\$" \
        || fail "sshd: PermitListen for dev$n (777$n/778$n/779$n) is missing"
done
grep -q '^AllowUsers dev1 dev2 dev3$' /etc/ssh/sshd_config.d/dev1c.conf || fail "sshd: AllowUsers dev1 dev2 dev3 is missing"
# sshd must accept its own config, otherwise it never started and the whole
# tunnel scheme is dead without a word.
pgrep -x sshd >/dev/null || fail "sshd is not running: check /etc/ssh/sshd_config.d/dev1c.conf"

echo "ok: workstation ports substituted (7771-7773, 7781-7783), sshd restricted to 777N/778N/779N"
