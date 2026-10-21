#!/bin/sh
# Build the administrator's page for the Linux kit: the hand-written part from
# config/kit/УСТАНОВКА.md with its @MANUAL-PATH@ line replaced by the «Ручной
# путь» section, generated from the installer itself.
#
#   sh scripts/manual-path.sh <kit dir> [dev1c-setup]  → <kit dir>/УСТАНОВКА.md
#
# Вторым аргументом — программа, которой генерировать: в комплекте для
# линукса она ELF, и машина сборки (windows при разработке) запустить её не
# может. pack.sh передаёт ту, которую нашёл пробой. Без аргумента скрипт
# ищет в комплекте сам.
#
# The customer asked to be able to stand the environment up without our
# binary, so the page carries every modifying command. They are not written by
# hand: `dev1c-setup install --dry-run --commands` prints what the operations
# would run (common::docker only prints under --dry-run, so no docker and no
# GPU are needed here), and this script turns that into the section. A command
# that changes in the program changes in the page on the next kit.
#
# Two things are fed to the program rather than taken from the kit:
#   * [model] file — empty in the kit (the .gguf is the administrator's own
#     file), and with no name the model commands would print without one, so a
#     placeholder is substituted into a copy of the kit's dev1c.toml;
#   * the kit directory — the run happens in a temporary directory with
#     `--kit .`, so the printed paths stay relative and no path of the machine
#     that built the kit reaches the page.
set -eu

DIR=$(cd "$(dirname "$0")/.." && pwd)
KIT=$(cd "${1:?kit directory}" && pwd)
PAGE="$DIR/config/kit/УСТАНОВКА.md"
MARK='@MANUAL-PATH@'
MODEL=модель.gguf

SETUP=${2:-}
if [ -z "$SETUP" ]; then
    SETUP="$KIT/dev1c-setup"
    [ -x "$SETUP" ] || SETUP="$KIT/dev1c-setup.exe"
fi
[ -x "$SETUP" ] || { echo "manual-path.sh: $SETUP не запускается — передайте вторым аргументом программу для этой машины" >&2; exit 1; }
# Прогон делается из временного каталога, так что относительный путь к
# программе оттуда не нашёлся бы.
SETUP=$(cd "$(dirname "$SETUP")" && pwd)/$(basename "$SETUP")
[ -f "$KIT/dev1c.toml" ] || { echo "manual-path.sh: no dev1c.toml in $KIT" >&2; exit 1; }
grep -q "^$MARK\$" "$PAGE" || { echo "manual-path.sh: no $MARK line in $PAGE" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
sed "s|^file = \"\"|file = \"$MODEL\"|" "$KIT/dev1c.toml" >"$tmp/dev1c.toml"
(cd "$tmp" && "$SETUP" --kit . install --dry-run --commands) >"$tmp/raw.txt"

# Предел контейнера модели из [model] memory комплекта: он задан человеком —
# и тогда это его решение, верное на любой машине, которое страница обязана
# напечатать как есть. Пусто (так комплект и уезжает) — предел считается от
# памяти ЭТОЙ машины, и в странице ему место только заглушкой. Читаем именно
# [model]: раздел [server] тоже зовёт свой предел memory.
MODEL_MEMORY=$(awk '
    /^\[/        { section = $0; next }
    section == "[model]" && /^memory[ \t]*=/ {
        sub(/^memory[ \t]*=[ \t]*"?/, ""); sub(/".*$/, ""); print; exit
    }
' "$KIT/dev1c.toml")

# Заглушка вместо посчитанного предела — и только вместо посчитанного.
mask_memory() {
    if [ -n "$MODEL_MEMORY" ]; then
        cat
    else
        sed -e 's|--memory [0-9][0-9]*m|--memory <ПРЕДЕЛ>m|g' \
            -e 's|--memory-swap [0-9][0-9]*m|--memory-swap <ПРЕДЕЛ×2>m|g' \
            -e 's|--memory-reservation [0-9][0-9]*m|--memory-reservation <ПРЕДЕЛ÷2>m|g'
    fi
}

# Operation headings (== id — title) and the plan in words (· line) frame the
# commands; every other line is a command. A plan line that is itself a
# command (the plan names `docker load`, `docker volume create`, `docker run`
# as they are) is dropped: the block below it says the same, pasteably. The
# leading ./ (or .\ when the page is generated on Windows) is the kit
# directory the run was given.
awk '
    function fence() { if (open) { print "```"; open = 0; blank = 0 } }
    /^== / { fence(); sub(/^== /, ""); printf "\n**%s**\n\n", $0; blank = 1; next }
    /^· (docker|podman) / { next }
    # Раздел памяти называется числами ЭТОЙ машины — на машине сборки они
    # чужие. Он нужен тому, кто ставит программой; страница объясняет предел
    # сама, в списке оговорок, и заглушками в команде.
    /^· память: / { next }
    /^· /  { fence(); sub(/^· /, ""); printf "- %s\n", $0; blank = 0; next }
    { if (!open) { if (!blank) print ""; print "```sh"; open = 1 } print }
    END { fence() }
' "$tmp/raw.txt" \
    | sed 's|\./||g; s|\.\\||g' \
    | mask_memory \
    >"$tmp/section.md"

# Предел контейнера МОДЕЛИ считается от памяти этой машины, а комплект
# собирается не на сервере: число машины сборки здесь — это 4 ГБ пределу на
# 32-гигабайтном сервере, то есть сломанный сервер, а не потеря настройки
# (как с --cache-ram). Поэтому выше оно заменено заглушкой, которую страница
# объясняет: вставленная как есть, она остановит docker с внятной ошибкой, а
# не покалечит сервер молча.
#
# Проверка смотрит ровно на эту форму: программа печатает посчитанный предел
# только в мегабайтах (`format!("{l}m")`). Предел контейнера СРЕДЫ приходит
# из [server] memory комплекта как есть («8g») — он задан человеком, верен на
# любой машине, и печатать его на странице нужно. Якорить проверку на любую
# цифру нельзя: «--memory 8g» ломал бы сборку комплекта целиком.
#
# Заданный оператором [model] memory печатается в мегабайтах той же формой
# («22g» программа тоже приводит к «22528m»), так что проверять там нечего:
# считанного предела в странице нет по построению. Там, где он возможен —
# комплект с пустым [model] memory, то есть ровно тот, который уезжает к
# заказчику, — проверка работает и обязана уметь провалиться.
if [ -z "$MODEL_MEMORY" ] && grep -q -- '--memory[a-z-]* [0-9][0-9]*m' "$tmp/section.md"; then
    echo "manual-path.sh: a computed memory limit from the build machine survived in the page" >&2
    exit 1
fi

# A silently lost step would be the worst outcome: the page looks finished and
# stands up half a server. So every operation is accounted for by name — the
# five that must carry commands, and the three that carry none by design (env
# checks, llama.answers measures, services waits). An operation that is not in
# either list is new, and whether the page needs its commands is a decision,
# not a default.
REQUIRED="image volumes model llama.container container"
SILENT="env llama.answers services"
awk '
    /^== / { id = $2; n[id] += 0; order[++k] = id; next }
    /^· /  { next }
    { n[id]++ }
    END { for (i = 1; i <= k; i++) print order[i], n[order[i]] }
' "$tmp/raw.txt" >"$tmp/ops.txt"
while read -r id n; do
    case " $REQUIRED $SILENT " in
        *" $id "*) ;;
        *) echo "manual-path.sh: unknown operation $id — decide whether the page carries its commands" >&2; exit 1 ;;
    esac
    case " $REQUIRED " in
        *" $id "*)
            [ "$n" -gt 0 ] || { echo "manual-path.sh: operation $id printed no commands — the page would lose that step" >&2; exit 1; } ;;
    esac
done <"$tmp/ops.txt"
for id in $REQUIRED; do
    grep -q "^$id " "$tmp/ops.txt" \
        || { echo "manual-path.sh: operation $id is missing from the path entirely" >&2; exit 1; }
done

# And the three commands the whole install rests on, in the order the
# operations run them.
grep -v '^\(== \|· \)' "$tmp/raw.txt" >"$tmp/cmd.txt"
line_of() { # $1 = command prefix → line number of its first occurrence
    grep -n -- "$1" "$tmp/cmd.txt" | head -1 | cut -d: -f1
}
load=$(line_of 'docker load ')
volume=$(line_of 'docker volume create ')
run=$(line_of 'docker run ')
[ -n "$load" ] && [ -n "$volume" ] && [ -n "$run" ] \
    || { echo "manual-path.sh: the generated path has no docker load / volume create / run — nothing to install by hand" >&2; exit 1; }
[ "$load" -lt "$volume" ] && [ "$volume" -lt "$run" ] \
    || { echo "manual-path.sh: order broken — load $load, volume create $volume, run $run" >&2; exit 1; }

awk -v f="$tmp/section.md" -v mark="$MARK" '
    $0 == mark { while ((getline l < f) > 0) print l; next }
    { print }
' "$PAGE" >"$KIT/УСТАНОВКА.md"
echo "УСТАНОВКА.md: $(wc -l <"$KIT/УСТАНОВКА.md") строк, из них ручной путь $(wc -l <"$tmp/section.md")"
