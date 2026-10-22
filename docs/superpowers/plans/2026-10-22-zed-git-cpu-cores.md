# Git из Zed ограничен двумя ядрами — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Обёртка `git` в образе ограничивает вызовы git из Zed двумя ядрами и низким приоритетом, не трогая git пользователя в терминале.

**Architecture:** В образ кладётся `/usr/local/bin/git` — POSIX-скрипт, который перекрывает `/usr/bin/git` в `PATH`. Он определяет по цепочке предков в `/proc`, что вызов сделан из Zed; если да — запускает настоящий git под `taskset` (два CPU), `nice -n 19` и `ionice -c 3`. Если нет — `exec` настоящего git без изменений. Ограничение вычисляется в рантайме из фактической маски CPU процесса, переопределяется переменной `DEV1C_GIT_CPUS`; при любом отказе (`taskset` не работает) git всё равно запускается.

**Tech Stack:** POSIX `sh` (dash в Debian), `procps`, `util-linux` (`taskset`), `coreutils`, `awk`/`sed`; Docker-образ dev1c, GitHub Actions.

**Спека:** `docs/superpowers/specs/2026-10-22-zed-git-cpu-cores-design.md`

---

## Файлы

| Файл | Действие | Ответственность |
|---|---|---|
| `config/git-cpuset.sh` | создать | обёртка git: определение вызывающего, выбор ядер, запуск |
| `config/test-git-cpuset.sh` | создать | проверка обёртки (в образе → `/usr/local/bin/test-git-cpuset`) |
| `Dockerfile` | изменить | установка обёртки и теста, `chmod +x` |
| `.github/workflows/build.yml` | изменить | прогон теста в CI |
| `config/zed-settings.json` | изменить (уже сделано) | выключенный UI git в Zed |
| `docs/STATUS.md` | изменить | замер на стенде и его результат |

---

### Task 1: Обёртка и её проверка

**Files:**
- Create: `config/test-git-cpuset.sh`
- Create: `config/git-cpuset.sh`

- [ ] **Step 1: Написать падающий тест**

Создать `config/test-git-cpuset.sh`:

```sh
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
out=$(DEV1C_GIT_REAL="$tmp/fake-git" "$tmp/zed-editor" -c 'exec "$@"' sh "$WRAPPER" version)
mask=$(field "$out" mask)
ni=$(field "$out" ni)
[ "$ni" = "19" ] || fail "из Zed приоритет не снижен: ni=$ni"
expected=$(expand "$full" | tail -2 | sort | paste -sd, -)
[ "$(expand "$mask" | sort | paste -sd, -)" = "$expected" ] \
    || fail "из Zed маска не сужена до двух последних ядер: $mask вместо $expected"

# --- 3. Явное переопределение набора ядер -------------------------------------
out=$(DEV1C_GIT_CPUS=0 DEV1C_GIT_REAL="$tmp/fake-git" "$tmp/zed-editor" -c 'exec "$@"' sh "$WRAPPER" version)
mask=$(field "$out" mask)
[ "$(expand "$mask" | paste -sd, -)" = "0" ] \
    || fail "DEV1C_GIT_CPUS не применён: $mask"

# --- 4. taskset отказал — git всё равно должен запуститься --------------------
mkdir -p "$tmp/bin"
printf '#!/bin/sh\nexit 1\n' > "$tmp/bin/taskset"
chmod +x "$tmp/bin/taskset"
out=$(PATH="$tmp/bin:$PATH" DEV1C_GIT_REAL="$tmp/fake-git" "$tmp/zed-editor" -c 'exec "$@"' sh "$WRAPPER" version)
[ -n "$(field "$out" mask)" ] || fail "git не запустился, когда taskset отказал"

# --- 5. Настоящий git через обёртку работает ----------------------------------
case "$("$WRAPPER" --version)" in
    "git version"*) ;;
    *) fail "настоящий git не отвечает через обёртку" ;;
esac

echo "ok: git из Zed ограничен двумя ядрами, вне Zed не тронут"
```

- [ ] **Step 2: Убедиться, что тест падает**

Run (на Windows, в контейнере dev1c):

```sh
docker cp config/test-git-cpuset.sh dev1c:/tmp/test-git-cpuset.sh
docker exec -u dev1 dev1c sh -c 'DEV1C_WRAPPER=/usr/local/bin/git sh /tmp/test-git-cpuset.sh'
```

Expected: `FAIL: обёртка не установлена: /usr/local/bin/git` (в образе её ещё нет) — либо, если запускать с подменой `DEV1C_WRAPPER=/tmp/git-cpuset.sh`, падение на первом же утверждении, потому что файла нет.

- [ ] **Step 3: Написать обёртку**

Создать `config/git-cpuset.sh`:

```sh
#!/bin/sh
# Обёртка git: вызовы из Zed ограничиваются двумя ядрами.
#
# Зачем: Zed переспрашивает статус у git каждые несколько секунд и запускает
# эти вызовы параллельно. На выгрузке 1С (десятки тысяч файлов) один вызов
# стоит десятки секунд процессорного времени, и восемь таких вызовов забирают
# все ядра машины. Здесь git из Zed получает не больше двух ядер и самый
# низкий приоритет, а git пользователя в терминале не трогается.
#
# Переменные:
#   DEV1C_GIT_CPUS  — набор ядер для git ("6,7"). По умолчанию — последние
#                     два CPU из текущей маски. Пустая строка — без
#                     ограничения.
#   DEV1C_GIT_REAL  — путь к настоящему git (для проверки).
set -u

REAL_GIT="${DEV1C_GIT_REAL:-/usr/bin/git}"

# Кто зовёт: смотрим цепочку предков. Zed может звать git напрямую, а может
# через промежуточную оболочку.
from_zed() {
    pid=$1
    depth=0
    while [ "$depth" -lt 8 ] && [ "$pid" -gt 1 ]; do
        name=$(cat "/proc/$pid/comm" 2>/dev/null || true)
        case "$name" in
            *zed*) return 0 ;;
        esac
        # В /proc/<pid>/stat имя процесса стоит в скобках и может содержать
        # пробелы, поэтому поле ppp ищем после последней ')'.
        pid=$(sed 's/^.*) //' "/proc/$pid/stat" 2>/dev/null | awk '{print $2}')
        case "$pid" in ''|*[!0-9]*) return 1 ;; esac
        depth=$((depth + 1))
    done
    return 1
}

# Набор ядер для git: список CPU через запятую. Пустая строка — без
# ограничения.
git_cpus() {
    if [ -n "${DEV1C_GIT_CPUS:-}" ]; then
        printf '%s' "$DEV1C_GIT_CPUS"
        return 0
    fi
    mask=$(awk '/^Cpus_allowed_list/{print $2; exit}' /proc/self/status 2>/dev/null || true)
    [ -n "$mask" ] || return 0
    # Разворачиваем диапазоны и оставляем последние два CPU.
    out=""
    for part in $(printf '%s' "$mask" | tr ',' ' ' | tr -s ' '); do
        case "$part" in
            *-*)
                first=${part%%-*}
                last=${part##*-}
                case "$first$last" in *[!0-9]*) continue ;; esac
                i=$first
                while [ "$i" -le "$last" ]; do
                    out="$out $i"
                    i=$((i + 1))
                done
                ;;
            *)
                case "$part" in *[!0-9]*) continue ;; esac
                out="$out $part"
                ;;
        esac
    done
    # shellcheck disable=SC2086
    set -- $out
    count=$#
    [ "$count" -gt 0 ] || return 0
    if [ "$count" -eq 1 ]; then
        printf '%s' "$1"
        return 0
    fi
    while [ "$count" -gt 2 ]; do
        shift
        count=$((count - 1))
    done
    printf '%s,%s' "$1" "$2"
}

# Запуск git с ограничениями. Порядок обёрток не важен: каждая выставляет своё
# свойство и exec-ает следующую.
run_git() {
    cpus=$1
    shift
    if [ -n "$cpus" ] && command -v taskset >/dev/null 2>&1 \
        && taskset -c "$cpus" true >/dev/null 2>&1; then
        set -- taskset -c "$cpus" "$@"
    fi
    if command -v ionice >/dev/null 2>&1; then
        set -- ionice -c 3 "$@"
    fi
    if command -v nice >/dev/null 2>&1; then
        set -- nice -n 19 "$@"
    fi
    exec "$@"
}

if ! from_zed "$PPID"; then
    exec "$REAL_GIT" "$@"
fi

run_git "$(git_cpus)" "$@"
```

- [ ] **Step 4: Прогнать тест — он должен пройти**

```sh
docker cp config/git-cpuset.sh dev1c:/tmp/git-cpuset.sh
docker exec -u dev1 dev1c sh -c 'chmod +x /tmp/git-cpuset.sh && DEV1C_WRAPPER=/tmp/git-cpuset.sh sh /tmp/test-git-cpuset.sh'
```

Expected: `ok: git из Zed ограничен двумя ядрами, вне Zed не тронут`

- [ ] **Step 5: Проверить, что скрипт POSIX-совместим (dash), а не только под bash**

```sh
docker exec -u dev1 dev1c sh -c 'DEV1C_WRAPPER=/tmp/git-cpuset.sh dash /tmp/test-git-cpuset.sh'
```

Expected: та же строка `ok: ...`. Если `dash` в образе отсутствует — прогнать `sh` (в Debian `sh` и есть dash) и убедиться, что в скрипте нет конструкций bash (`[[`, `${var//}`, `local`).

- [ ] **Step 6: Закоммитить**

```sh
git add config/git-cpuset.sh config/test-git-cpuset.sh
git commit -m "Обёртка git: вызовы из Zed ограничены двумя ядрами"
```

---

### Task 2: Установка в образ

**Files:**
- Modify: `Dockerfile` (блок `COPY config/...` — строки 327–336; блок `chmod +x` — строки 353–359)

- [ ] **Step 1: Добавить копирование**

В блок «--- Scripts ---» (после строки с `config/test-entrypoint-ports.sh`) добавить две строки:

```dockerfile
COPY config/git-cpuset.sh      /usr/local/bin/git
COPY config/test-git-cpuset.sh /usr/local/bin/test-git-cpuset
```

- [ ] **Step 2: Добавить в chmod**

В существующий `chmod +x` (строки 355–359) дописать `/usr/local/bin/git /usr/local/bin/test-git-cpuset`:

```dockerfile
    && chmod +x /usr/local/bin/xfce-autoscale /usr/local/bin/keyring-start \
                /usr/local/bin/xsession /usr/local/bin/sshd-setup \
                /usr/local/bin/entrypoint /usr/local/bin/gitea-user-setup \
                /usr/local/bin/gitea-start /usr/local/bin/normalize-skills \
                /usr/local/bin/test-skills /usr/local/bin/test-entrypoint-ports \
                /usr/local/bin/git /usr/local/bin/test-git-cpuset \
```

- [ ] **Step 3: Проверить сборку**

```sh
docker build --check .
```

Expected: без ошибок (это тот же шаг, что в CI: «Lint Dockerfile»).

- [ ] **Step 4: Прогнать тест в работающем контейнере без пересборки**

Файлы уже скопированы в контейнер в Task 1 — поставить их на места образа вручную и прогнать:

```sh
docker cp config/git-cpuset.sh dev1c:/usr/local/bin/git
docker cp config/test-git-cpuset.sh dev1c:/usr/local/bin/test-git-cpuset
docker exec dev1c sh -c 'chmod +x /usr/local/bin/git /usr/local/bin/test-git-cpuset && sh /usr/local/bin/test-git-cpuset'
```

Expected: `ok: git из Zed ограничен двумя ядрами, вне Zed не тронут`

- [ ] **Step 5: Проверить, что настоящий git в контейнере не сломан**

```sh
docker exec dev1c git --version
docker exec dev1c sh -c 'command -v git; ls -l /usr/local/bin/git'
```

Expected: `git version 2.x.y`, `/usr/local/bin/git` — обычный файл (не symlink на `/usr/bin/git`).

- [ ] **Step 6: Закоммитить**

```sh
git add Dockerfile
git commit -m "Образ: обёртка git и её проверка"
```

---

### Task 3: Выключенный UI git в Zed

**Files:**
- Modify: `config/zed-settings.json` (правка уже в рабочей копии, строки ~387–404)

- [ ] **Step 1: Убедиться, что блок на месте и корректен**

```sh
git diff config/zed-settings.json
```

Expected: блок `"git"` с `"disable_git": true`, `"git_gutter": "hide"`, `"inline_blame": {"enabled": false}` и поясняющими комментариями на русском.

- [ ] **Step 2: Проверить, что файл остаётся JSONC и что Zed его принимает**

Файл с комментариями нельзя проверять обычным парсером JSON — так же, как это уже сделано для остальных правок (`test-entrypoint-ports.sh` сравнивает длину с эталоном). Эталон в образе — тот же файл из репозитория, поэтому проверка в контейнере:

```sh
docker cp config/zed-settings.json dev1c:/etc/skel/.config/zed/settings.json
docker exec dev1c sh -c 'wc -l < /etc/skel/.config/zed/settings.json; grep -c "disable_git" /etc/skel/.config/zed/settings.json'
```

Expected: `18` (число строк) и `1` — блок `disable_git` в файле есть ровно один раз.

- [ ] **Step 3: Закоммитить**

```sh
git add config/zed-settings.json
git commit -m "Zed: выключен UI git (панель, структура, вкладки, заколотка)"
```

---

### Task 4: Тест в CI

**Files:**
- Modify: `.github/workflows/build.yml` (шаг «1C tooling works», строка 292)

- [ ] **Step 1: Добавить прогон**

После строки `docker exec dev1c sh /usr/local/bin/test-entrypoint-ports` добавить:

```yaml
          docker exec dev1c sh /usr/local/bin/test-git-cpuset
```

- [ ] **Step 2: Проверить YAML**

```sh
python -c "import yaml,sys; yaml.safe_load(open('.github/workflows/build.yml', encoding='utf-8')); print('yaml ok')"
```

Expected: `yaml ok`

- [ ] **Step 3: Закоммитить**

```sh
git add .github/workflows/build.yml
git commit -m "CI: проверка обёртки git в контейнере"
```

---

### Task 5: Замер на стенде

**Files:**
- Modify: `docs/STATUS.md` (раздел «Verified by running it»)

- [ ] **Step 1: Замерить параллельность git под обёрткой**

Внутри контейнера (обёртка уже установлена в Task 2):

```sh
docker exec -u dev1 -e HOME=/home/dev1 dev1c sh -c '
cd /home/dev1/project
export LC_ALL=C
echo "--- без ограничения ---"
TIMEFORMAT="real=%3R user=%3U sys=%3S"
time ( for i in 1 2 3 4 5 6 7 8; do /usr/bin/git --no-optional-locks status --porcelain=v1 --untracked-files=all --no-renames -z -- src >/dev/null & done; wait )
echo "--- через обёртку (вызов не из Zed — без ограничения, это контроль) ---"
time ( for i in 1 2 3 4 5 6 7 8; do /usr/local/bin/git --no-optional-locks status --porcelain=v1 --untracked-files=all --no-renames -z -- src >/dev/null & done; wait )'
```

Expected: обе строки дают близкие числа — контрольный вызов идёт не из Zed, поэтому ограничения не применяется. Это проверка прозрачности.

- [ ] **Step 2: Замерить с ограничением (вызов «из Zed»)**

```sh
docker exec -u dev1 -e HOME=/home/dev1 dev1c sh -c '
cd /home/dev1/project
export LC_ALL=C
cp /bin/sh /tmp/zed-editor && chmod +x /tmp/zed-editor
TIMEFORMAT="real=%3R user=%3U sys=%3S"
echo "--- восемь status из «Zed» ---"
time ( for i in 1 2 3 4 5 6 7 8; do /tmp/zed-editor -c "exec \"\$@\"" sh /usr/local/bin/git --no-optional-locks status --porcelain=v1 --untracked-files=all --no-renames -z -- src >/dev/null & done; wait )'
```

Expected: `CPU/wall` ≤ ~2.2 против ~7.5 в контроле; суммарный CPU (`user+sys`) заметно ниже.

- [ ] **Step 3: Проверить на живом Zed**

```sh
docker exec -u dev1 -e HOME=/home/dev1 -e DISPLAY=:99 dev1c bash -c '
pkill -f "[z]ed-editor" 2>/dev/null; sleep 2
nohup /usr/local/bin/zed /home/dev1/project >/tmp/zed-gitcap.log 2>&1 &
sleep 45
ps -eo pid,ppid,psr,ni,comm | grep -E "git|zed" | head -20'
```

Expected: у процессов `git` `psr` — из пары последних ядер (6-7), `ni` = 19; у `zed-editor` — обычные `ni`.

- [ ] **Step 4: Записать числа в `docs/STATUS.md`**

В раздел «Verified by running it» добавить запись о том, что git из Zed ограничен двумя ядрами, с фактическими числами замера и оговоркой, что общий объём работы git не уменьшился — уменьшилась параллельность и суммарный CPU при параллельных вызовах.

- [ ] **Step 5: Закоммитить**

```sh
git add docs/STATUS.md
git commit -m "Замер: git из Zed на двух ядрах, параллельность 7.5 → 2.1"
```

---

## Проверки приёмки (сводно)

1. `sh /usr/local/bin/test-git-cpuset` → `ok: ...` — в контейнере и в CI.
2. `docker exec dev1c git --version` → обычный вывод; `/usr/local/bin/git` — файл.
3. Восемь параллельных `git status` из «Zed»: `CPU/wall` ≤ 2.2, суммарный CPU ниже контрольного.
4. Процессы git при живом Zed: `psr` из выбранной пары ядер, `ni=19`.
5. Git в терминале пользователя: `ni=0`, полная маска CPU.
6. `docker build --check .` → без ошибок.

## Что не входит

- Полное отключение git в Zed: `disable_git` прячет только UI, поллинг продолжается
  (апстрим [#13304](https://github.com/zed-industries/zed/issues/13304)).
- Сокращение воркспейса Zed и панели проекта — отдельная работа (спека про GPU и рендер).
- Эксклюзивные ядра для git — отказ заказчика, в плане их нет.
