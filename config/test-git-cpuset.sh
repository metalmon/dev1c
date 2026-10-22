#!/bin/sh
# Проверка обёртки git: из Zed — не больше двух ядер, низкий приоритет и
# idle-класс I/O; от остальных вызывающих — ничего.
#
# Настоящий git в тесте подменяется скриптом, печатающим собственное
# состояние: так видно и маска CPU, и приоритет процесса, и класс I/O.
#
# Проверяется и сама установка: `git` по имени обязан резолвиться в
# /usr/local/bin/git — иначе Zed позовёт настоящий git мимо обёртки.
set -eu

fail() { echo "FAIL: $*" >&2; exit 1; }

WRAPPER=${DEV1C_WRAPPER:-/usr/local/bin/git}
[ -x "$WRAPPER" ] || fail "обёртка не установлена: $WRAPPER"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

# Набор ядер по умолчанию считается из маски самого процесса, поэтому
# внешняя переменная не должна влиять на проверку.
unset DEV1C_GIT_CPUS

# Подмена настоящего git: печатает своё состояние и аргументы, код возврата
# берёт из DEV1C_FAKE_EXIT. Обёртка не должна терять ни аргументы, ни код.
cat > "$tmp/fake-git" <<'EOF'
#!/bin/sh
echo "mask=$(awk '/^Cpus_allowed_list/{print $2; exit}' /proc/self/status)"
echo "ni=$(ps -o ni= -p $$ | tr -d ' ')"
echo "io=$(ionice -p $$ 2>/dev/null | awk '{print $1}')"
printf 'args='
sep=
for a in "$@"; do
    printf '%s%s' "$sep" "$a"
    sep=,
done
printf '\n'
exit "${DEV1C_FAKE_EXIT:-0}"
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
# обёртка тогда не видит Zed и ничего не ограничивает.
zed_git() {
    "$tmp/zed-editor" -c 'sh "$@"' sh "$WRAPPER" "$@"
}

# Обёртка — прямой ребёнок Zed, без единой промежуточной оболочки: так Zed
# и запускает git. Реализация, которая смотрит только деда и пропускает
# непосредственного родителя, обязана здесь упасть.
zed_git_direct() {
    "$tmp/zed-editor" -c '"$@"' sh "$WRAPPER" "$@"
}

# Вызов по имени, а не по пути: проверяется, что PATH приводит к обёртке.
# Именно `git "$@"`, а не `sh git ...`: sh открыл бы файл с именем git.
zed_git_path() {
    "$tmp/zed-editor" -c 'git "$@"' sh "$@"
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
first_cpu=$(expand "$full" | head -1)

# Класс I/O проверяем, только если обёртка действительно может его
# выставить: наличие ionice ничего не значит (в cgroup v2 `ionice -c 3`
# часто не работает). Проба та же, что и в обёртке.
have_ionice=0
if command -v ionice >/dev/null 2>&1 && ionice -c 3 true >/dev/null 2>&1; then
    have_ionice=1
fi

# Отличимо ли сужение до двух последних ядер от полной маски: на
# двухъядерном раннере они совпадают, и проверка default-cap ничего не
# доказывает. Тогда честно пишем skip, а не делаем вид, что проверили.
can_check_cap=1
n_cpus=$(expand "$full" | wc -l | tr -d ' ')
if [ "$n_cpus" -lt 3 ]; then
    can_check_cap=0
    echo "skip: ядер меньше трёх, default-cap неотличим от полной маски"
fi

# Ожидания для вызова из Zed: маска сужена до двух последних ядер, приоритет
# низкий, I/O — idle.
expect_zed() {
    what=$1
    mask=$(field "$out" mask)
    ni=$(field "$out" ni)
    io=$(field "$out" io)
    if [ "$can_check_cap" = 1 ]; then
        [ "$(expand "$mask" | sort | paste -sd, -)" = "$last_two" ] \
            || fail "$what: маска не сужена до двух последних ядер: $mask вместо $last_two"
    fi
    [ "$ni" = "19" ] || fail "$what: приоритет не снижен: ni=$ni"
    [ "$have_ionice" = 1 ] || return 0
    [ "$io" = "idle" ] || fail "$what: I/O-класс не idle: $io"
}

# Ожидания для вызова не из Zed: маска полная, приоритет обычный, I/O — не
# idle. На двухъядерном раннере маска полная и при сужении, поэтому тут
# держат проверку приоритет и класс I/O.
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

# --- 1. Установка через PATH --------------------------------------------------
# В бою обёртка работает только потому, что /usr/local/bin идёт в PATH
# раньше /usr/bin. Если git по имени — не обёртка, Zed позовёт настоящий git.
resolved=$(command -v git 2>/dev/null || true)
if [ "$WRAPPER" != "/usr/local/bin/git" ]; then
    echo "skip: обёртка проверяется из $WRAPPER, установку через PATH не проверить"
elif [ "$resolved" != "/usr/local/bin/git" ]; then
    fail "git по имени резолвится в ${resolved:-ничего}, а не в /usr/local/bin/git: обёртка не перекрывает настоящий git"
else
    capture zed_git_path version
    expect_zed "git по имени из Zed"
fi

# --- 2. Вызов не из Zed: ничего не меняется -----------------------------------
not_zed "$WRAPPER" version
expect_not_zed "вне Zed"

# --- 3. Обёртка — прямой ребёнок Zed ------------------------------------------
capture zed_git_direct version
expect_zed "прямой ребёнок Zed"

# --- 4. Вызов из Zed: маска сужена, приоритет низкий, I/O — idle --------------
capture zed_git version
expect_zed "из Zed"

# --- 5. То же через две промежуточные оболочки: обход цепочки предков --------
capture zed_git_deep version
expect_zed "из Zed через две оболочки"

# --- 6. Явное переопределение набора ядер -------------------------------------
# Берём первый CPU текущей маски, а не литерал 0: на раннере с чужим cpuset
# taskset -c 0 отказал бы, и проверка ничего бы не проверила.
DEV1C_GIT_CPUS=$first_cpu
export DEV1C_GIT_CPUS
capture zed_git version
mask=$(field "$out" mask)
[ "$(expand "$mask" | paste -sd, -)" = "$first_cpu" ] \
    || fail "DEV1C_GIT_CPUS=$first_cpu не применён: $mask"
unset DEV1C_GIT_CPUS

# --- 7. Пустой DEV1C_GIT_CPUS — ограничения по CPU нет ------------------------
# Задано пустой строкой — значит ограничивать нечем: ни taskset, но
# приоритет и класс I/O остаются.
DEV1C_GIT_CPUS=
export DEV1C_GIT_CPUS
capture zed_git version
mask=$(field "$out" mask)
if [ "$can_check_cap" = 1 ]; then
    [ "$(expand "$mask" | sort | paste -sd, -)" = "$full_set" ] \
        || fail "пустой DEV1C_GIT_CPUS должен снять ограничение по CPU: $mask вместо $full"
fi
ni=$(field "$out" ni)
[ "$ni" = "19" ] || fail "пустой DEV1C_GIT_CPUS: приоритет не снижен: ni=$ni"
io=$(field "$out" io)
if [ "$have_ionice" = 1 ]; then
    [ "$io" = "idle" ] || fail "пустой DEV1C_GIT_CPUS: I/O-класс не idle: $io"
fi
unset DEV1C_GIT_CPUS

# --- 8. taskset отказал — git идёт без cap, но с остальными ограничениями ----
mkdir -p "$tmp/bin"
printf '#!/bin/sh\nexit 1\n' > "$tmp/bin/taskset"
chmod +x "$tmp/bin/taskset"
zed_git_broken
mask=$(field "$out" mask)
if [ "$can_check_cap" = 1 ]; then
    [ "$(expand "$mask" | sort | paste -sd, -)" = "$full_set" ] \
        || fail "при отказе taskset маска должна остаться полной: $mask вместо $full"
fi
ni=$(field "$out" ni)
[ "$ni" = "19" ] || fail "при отказе taskset приоритет должен остаться низким: ni=$ni"
rm -f "$tmp/bin/taskset"

# --- 9. ionice отказал — git всё равно должен запуститься --------------------
printf '#!/bin/sh\nexit 1\n' > "$tmp/bin/ionice"
chmod +x "$tmp/bin/ionice"
zed_git_broken
mask=$(field "$out" mask)
if [ "$can_check_cap" = 1 ]; then
    [ "$(expand "$mask" | sort | paste -sd, -)" = "$last_two" ] \
        || fail "при отказе ionice маска должна остаться суженной: $mask"
fi
ni=$(field "$out" ni)
[ "$ni" = "19" ] || fail "при отказе ionice приоритет должен остаться низким: ni=$ni"
rm -f "$tmp/bin/ionice"

# --- 10. nice отказал — git всё равно должен запуститься ---------------------
printf '#!/bin/sh\nexit 1\n' > "$tmp/bin/nice"
chmod +x "$tmp/bin/nice"
zed_git_broken
mask=$(field "$out" mask)
if [ "$can_check_cap" = 1 ]; then
    [ "$(expand "$mask" | sort | paste -sd, -)" = "$last_two" ] \
        || fail "при отказе nice маска должна остаться суженной: $mask"
fi
io=$(field "$out" io)
if [ "$have_ionice" = 1 ]; then
    [ "$io" = "idle" ] || fail "при отказе nice I/O-класс должен остаться idle: $io"
fi
rm -f "$tmp/bin/nice"

# --- 11. Аргументы и код возврата доходят без искажения ----------------------
capture zed_git status --short -uno
got=$(field "$out" args)
[ "$got" = "status,--short,-uno" ] || fail "из Zed аргументы искажены: [$got]"
not_zed "$WRAPPER" status --short -uno
got=$(field "$out" args)
[ "$got" = "status,--short,-uno" ] || fail "вне Zed аргументы искажены: [$got]"

# Ненулевой код возврата git не должен подменяться кодом обёртки. Код 0 уже
# проверен выше: capture() падает на любом ненулевом.
DEV1C_FAKE_EXIT=3
export DEV1C_FAKE_EXIT
rc=0
out=$(zed_git version 2>&1) || rc=$?
[ "$rc" = 3 ] || fail "из Zed код возврата git не дошёл: $rc вместо 3"
rc=0
out=$("$WRAPPER" version 2>&1) || rc=$?
[ "$rc" = 3 ] || fail "вне Zed код возврата git не дошёл: $rc вместо 3"
unset DEV1C_FAKE_EXIT

# --- 12. Настоящий git через обёртку работает --------------------------------
unset DEV1C_GIT_REAL
capture "$WRAPPER" --version
case "$out" in
    "git version"*) ;;
    *) fail "настоящий git не отвечает через обёртку: $out" ;;
esac

echo "ok: git из Zed ограничен двумя ядрами, вне Zed не тронут"
