#!/bin/sh
# The runnable check of the demo stand, inside a running container:
#   docker exec <container> test-stand
# Every workstation: its Apache answers the HTTP service of the base, mcp-1c
# answers initialize through the same port Zed uses, vrunner-mcp lists its
# tools through its port. Fails loudly on the first miss.
set -u
fail=0
say() { printf '%s\n' "$*"; }
mcp() { # $1 = port, $2 = third request (JSON-RPC), $3 = pattern expected in the answer
    ( printf '%s\n%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-stand","version":"0"}}}' \
        '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
        "$2"; sleep 25 ) \
      | timeout 120 nc -N localhost "$1" 2>/dev/null | tr -d '\000' > /tmp/mcp.out
    grep -aq "$3" /tmp/mcp.out && ! grep -aq '❌' /tmp/mcp.out
}
LIST='{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
INFO='{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_configuration_info","arguments":{}}}'
pgrep -x Xvfb >/dev/null && say "ok   Xvfb" || { say "FAIL Xvfb not running"; fail=1; }
for n in 1 2 3; do
    u="dev$n"; id "$u" >/dev/null 2>&1 || continue
    # first: the first MCP connection seeds the base into the home. A real
    # tool call, not tools/list: the list answers even when the HTTP service
    # path is wrong.
    mcp $((7770 + n)) "$INFO" '"result"' && say "ok   $u: mcp-1c on $((7770 + n)) reads the base" \
        || { say "FAIL $u: mcp-1c on $((7770 + n)): $(grep -ao '"text":"[^"]\{0,120\}' /tmp/mcp.out | head -n 1) (see /var/log/1c/$u-mcp.log)"; fail=1; }
    mcp $((7780 + n)) "$LIST" '"cf_decompile"' && say "ok   $u: vrunner-mcp on $((7780 + n))" \
        || { say "FAIL $u: vrunner-mcp on $((7780 + n)) (see /var/log/1c/$u-vrunner.log)"; fail=1; }
    code=$(curl -s -o /tmp/hs.$n -w '%{http_code}' "http://127.0.0.1:$((8080 + n))/base/hs/mcp-1c/version")
    case "$code" in
        200) say "ok   $u: HTTP service $(tr -d ' \n' </tmp/hs.$n)" ;;
        404) say "WARN $u: HTTP service answers 404 — the mcp-1c extension is not applied to the base" ;;
        *)   say "FAIL $u: HTTP service http=$code $(head -c 200 /tmp/hs.$n)"; fail=1 ;;
    esac
    [ -f "/home/$u/base/1Cv8.1CD" ] && [ "$(stat -c %U "/home/$u/base/1Cv8.1CD")" = "$u" ] \
        && say "ok   $u: base seeded, owned by $u" || { say "FAIL $u: base missing or not owned by $u"; fail=1; }
done
exit $fail
