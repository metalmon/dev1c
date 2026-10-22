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
#
# set -u, но без set -e: обёртка обязана запустить git в любом случае, даже
# если что-то по дороге сломалось.
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
    set -- "$REAL_GIT" "$@"
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
