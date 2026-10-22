#!/bin/sh
# Проверка обёртки git: из Zed — не больше двух ядер, низкий приоритет и
# idle-класс I/O; от остальных вызывающих — ничего.
#
# Настоящий git в тесте подменяется скриптом, печатающим собственное
# состояние: так видно и маска CPU, и приоритет процесса, и класс I/O.
set -eu

fail() { echo "FAIL: $*" >&2; exit 1; }

WRAPPER=${DEV1C_WRAPPER:-/usr/local/bin/git}
[ -x "$WRAPPER" ] || fail "обёртка не установлена: $WRAPPER"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

# Набор ядер по умолчанию считается из маски самого процесса, поэтому
# внешняя переменная не должна влиять на проверку.
unset DEV1C_GIT_CPUS

# Подмена настоящего git.
cat > "$tmp/fake-git" <<'EOF'
#!/bin/sh
echo "mask=$(awk '/^Cpus_allowed_list/{print $2; exit}' /proc/self/status)"
echo "ni=$(ps -o ni= -p $$ | tr -d ' ')"
echo "io=$(ionice -p $$ 2>/dev/null | awk '{print $1}')"
EOF
chmod +x "$tmp/fake-git"
export DEV1C_GIT_REAL="$tmp/fake-git"

# Процесс, который для обёртки выглядит как Zed: comm в /proc берётся из
# имени исполняемого файла.
cp /bin/sh "$tmp/zed-editor"
chmod +x "$tmp/zed-editor"

field() { printf '%s\n' "$1" | sed -n "s/^$2=//p"; }

# Развернуть список CPU ("6-7", "0-3,8-11") в отдельные номера.
expand() {
    printf '%s' "$1" | tr ',' ' ' | tr -s ' ' | tr ' ' '\n' \
        | awk -F- 'NF==2 { for (i=$1; i<=$2; i++) print i; next }
                    NF==1 && $1 != "" { print $1 }'
}

# Выполнить команду и забрать её вывод в $out. При ненулевом коде — FAIL с
# понятным текстом: молчаливый выход по set -e ничего не объясняет.
capture() {
    out=$("$@" 2>&1) || fail "команда не выполнилась: $* → $out"
}

# Вызов обёртки из процесса, который для неё выглядит как Zed.
#
# Именно sh, а не exec: exec заменяет образ процесса, и zed-editor
# исчезает из цепочки предков (comm становится именем новой программы) —
# обёртка тогда не видит Zed и ничего не ограничивает. С промежуточным sh
# цепочка остаётся zed-editor → sh → обёртка, как в бою.
zed_git() {
    "$tmp/zed-editor" -c 'sh "$@"' sh "$WRAPPER" "$@"
}

# Тот же вызов, но с двумя промежуточными оболочками:
# zed-editor → sh → sh → обёртка. Обёртка обязана дойти до Zed и здесь:
# проверяется обход цепочки предков, а не только прямой родитель.
zed_git_deep() {
    "$tmp/zed-editor" -c 'sh "$@"' sh -c 'sh "$@"' sh "$WRAPPER" "$@"
}

# Вызов из Zed с подменённым PATH: сломанный taskset/ionice/nice лежит в
# $tmp/bin и не должен помешать git запуститься.
zed_git_broken() {
    old_path=$PATH
    PATH="$tmp/bin:$PATH"
    export PATH
    capture zed_git version
    PATH=$old_path
    export PATH
}

# Есть ли в цепочке предков самой проверки процесс zed?
has_zed_ancestor() {
    pid=$$
    depth=0
    while [ "$depth" -lt 8 ] && [ "$pid" -gt 1 ]; do
        name=$(cat "/proc/$pid/comm" 2>/dev/null || true)
        case "$name" in
            *zed*) return 0 ;;
        esac
        pid=$(sed 's/^.*) //' "/proc/$pid/stat" 2>/dev/null | awk '{print $2}')
        case "$pid" in ''|*[!0-9]*) return 1 ;; esac
        depth=$((depth + 1))
    done
    return 1
}

# Процесс, оторванный от цепочки предков.
#
# setsid --fork переводит процесс-внука в init: предков у него не
# остаётся. --wait не нужен и даже мешает: пока setsid ждёт, внук остаётся
# его потомком, и цепочка предков не обрывается.
cat > "$tmp/detached" <<'EOF'
#!/bin/sh
i=0
while [ "$i" -lt 200 ]; do
    ppid=$(sed 's/^.*) //' /proc/$$/stat 2>/dev/null | awk '{print $2}')
    [ "$ppid" = "1" ] && break
    sleep 0.05
    i=$((i + 1))
done
"$@" > "$DETACH_OUT" 2>&1
echo $? > "$DETACH_RC"
EOF
chmod +x "$tmp/detached"

# Вызов «не из Zed»: процесс отрывается от цепочки предков самой проверки.
# Иначе проверка, запущенная из терминала Zed, сама оказалась бы «внутри
# Zed» — ограничение применилось бы по-настоящему, и пункт «вне Zed ничего
# не меняется» вругал бы на ровном месте.
not_zed() {
    if ! command -v setsid >/dev/null 2>&1; then
        if has_zed_ancestor; then
            fail "проверка идёт из Zed, а setsid недоступен: пункт «вне Zed» проверить нельзя"
        fi
        capture "$@"
        return
    fi
    rm -f "$tmp/nz.out" "$tmp/nz.rc"
    DETACH_OUT="$tmp/nz.out" DETACH_RC="$tmp/nz.rc" \
        setsid --fork "$tmp/detached" "$@"
    # setsid --fork не ждёт внука: ждём код возврата сами.
    i=0
    while [ ! -f "$tmp/nz.rc" ] && [ "$i" -lt 400 ]; do
        sleep 0.05
        i=$((i + 1))
    done
    [ -f "$tmp/nz.rc" ] || fail "оторванный процесс не отработал: $*"
    out=$(cat "$tmp/nz.out")
    rc=$(cat "$tmp/nz.rc")
    [ "$rc" = "0" ] || fail "команда не выполнилась: $* → $out"
}

full=$(awk '/^Cpus_allowed_list/{print $2; exit}' /proc/self/status)
full_set=$(expand "$full" | sort | paste -sd, -)
last_two=$(expand "$full" | tail -2 | sort | paste -sd, -)

# Класс I/O проверяем, только если ionice есть в системе: без него и
# обёртка не может его выставить, и проверка была бы заведомо ложной.
have_ionice=0
if command -v ionice >/dev/null 2>&1; then
    have_ionice=1
fi

# Ожидания для вызова из Zed: маска сужена до двух последних ядер, приоритет
# низкий, I/O — idle.
expect_zed() {
    what=$1
    mask=$(field "$out" mask)
    ni=$(field "$out" ni)
    io=$(field "$out" io)
    [ "$(expand "$mask" | sort | paste -sd, -)" = "$last_two" ] \
        || fail "$what: маска не сужена до двух последних ядер: $mask вместо $last_two"
    [ "$ni" = "19" ] || fail "$what: приоритет не снижен: ni=$ni"
    [ "$have_ionice" = 1 ] || return 0
    [ "$io" = "idle" ] || fail "$what: I/O-класс не idle: $io"
}

# Ожидания для вызова не из Zed: маска полная, приоритет обычный, I/O — не
# idle.
expect_not_zed() {
    what=$1
    mask=$(field "$out" mask)
    ni=$(field "$out" ni)
    io=$(field "$out" io)
    [ "$(expand "$mask" | sort | paste -sd, -)" = "$full_set" ] \
        || fail "$what: маска изменена: $mask вместо $full"
    [ "$ni" = "0" ] || fail "$what: приоритет изменён: ni=$ni"
    [ "$io" != "idle" ] || fail "$what: I/O-класс не должен быть idle"
}

# --- 1. Вызов не из Zed: ничего не меняется -----------------------------------
not_zed "$WRAPPER" version
expect_not_zed "вне Zed"

# --- 2. Вызов из Zed: маска сужена, приоритет низкий, I/O — idle --------------
capture zed_git version
expect_zed "из Zed"

# --- 3. То же через две промежуточные оболочки: обход цепочки предков --------
capture zed_git_deep version
expect_zed "из Zed через две оболочки"

# --- 4. Явное переопределение набора ядер -------------------------------------
DEV1C_GIT_CPUS=0
export DEV1C_GIT_CPUS
capture zed_git version
mask=$(field "$out" mask)
[ "$(expand "$mask" | paste -sd, -)" = "0" ] \
    || fail "DEV1C_GIT_CPUS не применён: $mask"
unset DEV1C_GIT_CPUS

# --- 5. Пустой DEV1C_GIT_CPUS — ограничения нет вовсе ------------------------
# Задано пустой строкой — значит ограничивать нечем: ни taskset, но
# приоритет и класс I/O остаются.
DEV1C_GIT_CPUS=
export DEV1C_GIT_CPUS
capture zed_git version
mask=$(field "$out" mask)
[ "$(expand "$mask" | sort | paste -sd, -)" = "$full_set" ] \
    || fail "пустой DEV1C_GIT_CPUS должен снять ограничение по CPU: $mask вместо $full"
ni=$(field "$out" ni)
[ "$ni" = "19" ] || fail "пустой DEV1C_GIT_CPUS: приоритет не снижен: ni=$ni"
io=$(field "$out" io)
if [ "$have_ionice" = 1 ]; then
    [ "$io" = "idle" ] || fail "пустой DEV1C_GIT_CPUS: I/O-класс не idle: $io"
fi
unset DEV1C_GIT_CPUS

# --- 6. taskset отказал — git идёт без cap, но с остальными ограничениями ----
mkdir -p "$tmp/bin"
printf '#!/bin/sh\nexit 1\n' > "$tmp/bin/taskset"
chmod +x "$tmp/bin/taskset"
zed_git_broken
mask=$(field "$out" mask)
[ "$(expand "$mask" | sort | paste -sd, -)" = "$full_set" ] \
    || fail "при отказе taskset маска должна остаться полной: $mask вместо $full"
ni=$(field "$out" ni)
[ "$ni" = "19" ] || fail "при отказе taskset приоритет должен остаться низким: ni=$ni"
rm -f "$tmp/bin/taskset"

# --- 7. ionice отказал — git всё равно должен запуститься --------------------
printf '#!/bin/sh\nexit 1\n' > "$tmp/bin/ionice"
chmod +x "$tmp/bin/ionice"
zed_git_broken
mask=$(field "$out" mask)
[ "$(expand "$mask" | sort | paste -sd, -)" = "$last_two" ] \
    || fail "при отказе ionice маска должна остаться суженной: $mask"
ni=$(field "$out" ni)
[ "$ni" = "19" ] || fail "при отказе ionice приоритет должен остаться низким: ni=$ni"
rm -f "$tmp/bin/ionice"

# --- 8. nice отказал — git всё равно должен запуститься ----------------------
printf '#!/bin/sh\nexit 1\n' > "$tmp/bin/nice"
chmod +x "$tmp/bin/nice"
zed_git_broken
mask=$(field "$out" mask)
[ "$(expand "$mask" | sort | paste -sd, -)" = "$last_two" ] \
    || fail "при отказе nice маска должна остаться суженной: $mask"
io=$(field "$out" io)
if [ "$have_ionice" = 1 ]; then
    [ "$io" = "idle" ] || fail "при отказе nice I/O-класс должен остаться idle: $io"
fi
rm -f "$tmp/bin/nice"

# --- 9. Настоящий git через обёртку работает ---------------------------------
unset DEV1C_GIT_REAL
capture "$WRAPPER" --version
case "$out" in
    "git version"*) ;;
    *) fail "настоящий git не отвечает через обёртку: $out" ;;
esac

echo "ok: git из Zed ограничен двумя ядрами, вне Zed не тронут"
