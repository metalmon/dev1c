#!/bin/sh
# Проверка обёртки git: из Zed — не больше двух ядер и низкий приоритет,
# от остальных вызывающих — ничего.
#
# Настоящий git в тесте подменяется скриптом, печатающим собственное
# состояние: так видно и маску CPU, и приоритет процесса.
set -eu

fail() { echo "FAIL: $*" >&2; exit 1; }

WRAPPER=${DEV1C_WRAPPER:-/usr/local/bin/git}
[ -x "$WRAPPER" ] || fail "обёртка не установлена: $WRAPPER"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

# Подмена настоящего git.
cat > "$tmp/fake-git" <<'EOF'
#!/bin/sh
echo "mask=$(awk '/^Cpus_allowed_list/{print $2; exit}' /proc/self/status)"
echo "ni=$(ps -o ni= -p $$ | tr -d ' ')"
EOF
chmod +x "$tmp/fake-git"

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

# Вызов обёртки из процесса, который для неё выглядит как Zed.
#
# Именно sh, а не exec: exec заменяет образ процесса, и zed-editor
# исчезает из цепочки предков (comm становится именем новой программы) —
# обёртка тогда не видит Zed и ничего не ограничивает. С промежуточным sh
# цепочка остаётся zed-editor → sh → обёртка, как в бою.
#
# Окружение (DEV1C_GIT_CPUS, DEV1C_GIT_REAL, PATH) задаётся вызывающим:
# подстановка перед вызовом функции действует в подстановке команды, то
# есть в отдельном процессе, и наружу не выходит.
zed_git() {
    "$tmp/zed-editor" -c 'sh "$@"' sh "$WRAPPER" "$@"
}

full=$(awk '/^Cpus_allowed_list/{print $2; exit}' /proc/self/status)
full_set=$(expand "$full" | sort | paste -sd, -)

# --- 1. Вызов не из Zed: ничего не меняется -----------------------------------
out=$(DEV1C_GIT_REAL="$tmp/fake-git" "$WRAPPER" version)
mask=$(field "$out" mask)
ni=$(field "$out" ni)
[ "$(expand "$mask" | sort | paste -sd, -)" = "$full_set" ] \
    || fail "вне Zed маска изменена: $mask вместо $full"
[ "$ni" = "0" ] || fail "вне Zed приоритет изменён: ni=$ni"

# --- 2. Вызов из Zed: маска сужена, приоритет низкий --------------------------
out=$(DEV1C_GIT_REAL="$tmp/fake-git" zed_git version)
mask=$(field "$out" mask)
ni=$(field "$out" ni)
[ "$ni" = "19" ] || fail "из Zed приоритет не снижен: ni=$ni"
expected=$(expand "$full" | tail -2 | sort | paste -sd, -)
[ "$(expand "$mask" | sort | paste -sd, -)" = "$expected" ] \
    || fail "из Zed маска не сужена до двух последних ядер: $mask вместо $expected"

# --- 3. Явное переопределение набора ядер -------------------------------------
out=$(DEV1C_GIT_CPUS=0 DEV1C_GIT_REAL="$tmp/fake-git" zed_git version)
mask=$(field "$out" mask)
[ "$(expand "$mask" | paste -sd, -)" = "0" ] \
    || fail "DEV1C_GIT_CPUS не применён: $mask"

# --- 4. taskset отказал — git всё равно должен запуститься --------------------
mkdir -p "$tmp/bin"
printf '#!/bin/sh\nexit 1\n' > "$tmp/bin/taskset"
chmod +x "$tmp/bin/taskset"
out=$(PATH="$tmp/bin:$PATH" DEV1C_GIT_REAL="$tmp/fake-git" zed_git version)
[ -n "$(field "$out" mask)" ] || fail "git не запустился, когда taskset отказал"

# --- 5. Настоящий git через обёртку работает ----------------------------------
case "$("$WRAPPER" --version)" in
    "git version"*) ;;
    *) fail "настоящий git не отвечает через обёртку" ;;
esac

echo "ok: git из Zed ограничен двумя ядрами, вне Zed не тронут"
