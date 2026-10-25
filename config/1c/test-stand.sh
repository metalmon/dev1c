#!/bin/sh
# The runnable check of the demo stand, inside a running container:
#   docker exec <container> test-stand
# Every workstation: the connector's own Apache answers the HTTP service of
# the base (host.publication, no stand-owned publish anymore), mcp-1c
# answers initialize through the same port Zed uses, vrunner-mcp lists its
# tools through its port. Fails loudly on the first miss.
set -u
fail=0
T=$(mktemp -d) && trap 'rm -rf "$T"' EXIT
say() { printf '%s\n' "$*"; }
mcp() { # $1 = port, $2 = third request (JSON-RPC), $3 = pattern expected in the answer
    ( printf '%s\n%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-stand","version":"0"}}}' \
        '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
        "$2"; sleep 25 ) \
      | timeout 120 nc -N localhost "$1" 2>/dev/null | tr -d '\000' > $T/mcp.out
    grep -aq "$3" $T/mcp.out && ! grep -aq '❌' $T/mcp.out
}
# The platform and the template base come in two volumes, not in the image.
# Without them nothing below can pass, and the reason is one line, so say it
# and stop rather than printing fifteen failures.
V=${PLATFORM_VERSION:-8.5.1.1150}
if [ -x "/opt/1cv8t/x86_64/$V/1cv8t" ] && [ -f /opt/1c-demo/base/1Cv8.1CD ]; then
    say "ok   payload: $(tr '\n' ' ' </opt/1c-demo/payload.conf 2>/dev/null)"
else
    say "FAIL payload: /opt/1cv8t and /opt/1c-demo are empty — ./provision.sh import"
    exit 1
fi
# dev1c-onboarding: файлы на месте — программа, оба пункта запуска
# (меню и автозапуск сессии). Не per-workstation: программа одна на образ.
if [ -f /opt/onboarding/main.py ] && [ -f /opt/onboarding/scenario.toml ] \
    && [ -f /usr/share/applications/1c-onboarding.desktop ] \
    && [ -f /etc/xdg/autostart/zz-1c-onboarding.desktop ]; then
    if (cd /opt/onboarding && python3 -c 'import main' >$T/onboarding.out 2>&1); then
        say "ok   onboarding: /opt/onboarding, точка входа импортируется"
    else
        say "FAIL onboarding: main.py не импортируется: $(tr '\n' ' ' <$T/onboarding.out)"; fail=1
    fi
else
    say "FAIL onboarding: не все файлы на месте (/opt/onboarding или один из двух .desktop)"
    fail=1
fi
# Правила агента: добавка стенда дописана к общим (Dockerfile.1c, cat >>), и
# версия платформы в ней — та, что правда стоит в образе. Строка «8.3.27»
# пережила в них смену платформы и обещала агенту не то, что он найдёт
# (поймано 25.10.2026). Сверка дешевле, чем доверие к прозе.
# Ворота перед Gitea: хук на месте, исполняемый и включён системным
# конфигом. Без бита исполнения git пропускает хук молча, а бит не
# переживает сборочный контекст с Windows — поэтому проверяется именно он,
# а не только наличие файла.
H=$(git config --system --get core.hooksPath 2>/dev/null)
if [ "$H" = /opt/git-hooks ] && [ -x /opt/git-hooks/pre-push ]; then
    say "ok   git-хуки: pre-push на месте и включён ($H)"
else
    say "FAIL git-хуки: hooksPath='$H', pre-push $([ -x /opt/git-hooks/pre-push ] && echo есть || echo 'не исполняемый или отсутствует')"
    fail=1
fi
A=/etc/skel/.config/zed/AGENTS.md
if ! grep -q 'демо-стенд' $A 2>/dev/null; then
    say "FAIL правила агента: в $A нет добавки стенда (config/1c/agents-1c.md)"; fail=1
elif grep -q "$V" $A; then
    say "ok   правила агента: добавка стенда на месте, платформа $V названа верно"
else
    say "FAIL правила агента: платформа в $A ($(grep -o '8\.[0-9][0-9.]*' $A | sort -u | tr '\n' ' ')) не та, что в образе ($V)"; fail=1
fi
# apache2 путём рабочего места, а не root. 1c-connect зовёт его по имени из
# $PATH, пакет кладёт в /usr/sbin, и /etc/profile отдаёт этот каталог только
# root. Проверяем в вычищенном окружении: `runuser -u devN` без -l наследует
# путь root, и проверка, сделанная через него, была бы зелёной на сломанном
# стенде — именно так этот дефект сюда и дожил.
if env -i PATH=/usr/local/bin:/usr/bin:/bin sh -c 'command -v apache2' >/dev/null 2>&1; then
    say "ok   apache2 находится путём рабочего места"
else
    say "FAIL apache2 не найден путём рабочего места (/usr/local/bin:/usr/bin:/bin) — публикация базы не поднимется при входе по RDP"
    fail=1
fi
LIST='{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
INFO='{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_configuration_info","arguments":{}}}'
for n in 1 2 3; do
    u="dev$n"; id "$u" >/dev/null 2>&1 || continue
    # seed first (idempotent; the start does it in the background, but a
    # 1.9 GB copy may still be running): the MCP checks below hold their
    # connection for 25 s only. A real tool call, not tools/list: the list
    # answers even when the HTTP service path is wrong.
    runuser -u "$u" -- /usr/local/bin/1c-stand seed >/dev/null 2>&1
    # Both MCP ports are the connector's own now, not socat's: start a
    # session for this workstation if nothing answers yet (a fresh
    # container, or a stand that has not been told to run start), and stop
    # only what we started here — a connector the stand itself is running
    # stays untouched.
    serve_pid=
    if ! nc -z -w1 localhost $((7770 + n)) 2>/dev/null; then
        runuser -u "$u" -- env HOME="/home/$u" "/home/$u/dev1c/1c-connect" serve \
            >/var/log/1c/$u-connect.log 2>&1 &
        serve_pid=$!
        for try in 1 2 3 4 5 6 7 8 9 10; do
            nc -z -w1 localhost $((7770 + n)) 2>/dev/null && break
            sleep 1
        done
    fi
    # Публикация базы поднимается коннектором и прогревается не мгновенно:
    # первое обращение к ней может прийти раньше, чем 1С успеет ответить, и
    # тогда mcp-1c отдаёт HTTP 500 с телом, не похожим на ответ расширения.
    # Отличить такой отказ от настоящего можно было только прочитав
    # следующую строку, где тот же сервис той же базы отвечает исправно, —
    # а однажды его спишут на гонку, когда он будет настоящим. Поэтому
    # ждём готовности сервиса, и только потом спрашиваем агента.
    for try in $(seq 30); do
        [ "$(curl -s -o /dev/null -m 2 -w '%{http_code}' \
             "http://127.0.0.1:$((8080 + n))/hs/mcp-1c/version" 2>/dev/null)" = 200 ] && break
        sleep 1
    done
    mcp $((7770 + n)) "$INFO" '"result"' && say "ok   $u: mcp-1c on $((7770 + n)) reads the base" \
        || { say "FAIL $u: mcp-1c on $((7770 + n)): $(grep -ao '"text":"[^"]\{0,120\}' $T/mcp.out | head -n 1) (see /var/log/1c/$u-mcp.log)"; fail=1; }
    mcp $((7780 + n)) "$LIST" '"cf_decompile"' && say "ok   $u: vrunner-mcp on $((7780 + n))" \
        || { say "FAIL $u: vrunner-mcp on $((7780 + n)) (see /var/log/1c/$u-vrunner.log)"; fail=1; }
    # the base's one Enterprise session may still be busy with the agent's
    # call above (the pool keeps it 20 s): on the connection-limit answer,
    # wait and ask again rather than fail
    for try in 1 2 3 4 5 6; do
        code=$(curl -s -o $T/hs.$n -w '%{http_code}' "http://127.0.0.1:$((8080 + n))/hs/mcp-1c/version")
        grep -q 'количество подключений\|connections limitation' $T/hs.$n || break
        sleep 15
    done
    case "$code" in
        200) say "ok   $u: HTTP service $(tr -d ' \n' <$T/hs.$n)" ;;
        404) say "WARN $u: HTTP service answers 404 — the mcp-1c extension is not applied to the base" ;;
        401) say "FAIL $u: HTTP service answers 401 — the publication does not carry the infobase user (DEMO_DB_USER)"; fail=1 ;;
        *)   say "FAIL $u: HTTP service http=$code $(head -c 200 $T/hs.$n)"; fail=1 ;;
    esac
    [ -f "/home/$u/base/1Cv8.1CD" ] && [ "$(stat -c %U "/home/$u/base/1Cv8.1CD")" = "$u" ] \
        && say "ok   $u: base seeded, owned by $u" || { say "FAIL $u: base missing or not owned by $u"; fail=1; }
    # the person's own way in: the base of the stand has one infobase user, and
    # without /N in the list the starter opens a login dialog instead of 1C
    v8i="/home/$u/.1C/1cestartt/ibases.v8i"
    [ -z "${DEMO_DB_USER-}" ] || grep -aq "/N${DEMO_DB_USER}" "$v8i" 2>/dev/null \
        && say "ok   $u: base list carries the infobase user" \
        || { say "FAIL $u: $v8i has no /N${DEMO_DB_USER} — the starter will ask who you are"; fail=1; }
    [ -z "$serve_pid" ] || { kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null; }
done
exit $fail
