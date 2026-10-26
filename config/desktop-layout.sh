#!/bin/sh
# Новые значки — ПОД старыми, а не между ними.
#
#   desktop-layout <дом> <владелец> <группа>
#
# xfdesktop помнит, где лежит каждый значок, в
# ~/.config/xfce4/desktop/icons.screen0.yaml. Появившийся ярлык он кладёт в
# первое свободное место сверху — и старые значки переезжают вниз. Для
# человека это мелочь, а для онборда нет: стрелки берут координаты из этого
# самого файла, а xfdesktop пишет его не сразу. Пока запись на диске отстаёт,
# стрелка показывает на соседний значок — ровно то, что заказчик видел
# 26.10.2026, когда на стол приехал «Чат с моделью» и встал второй строкой,
# сдвинув Gitea и Zed.
#
# Поэтому: значки, которых в раскладке ещё нет, дописываем НИЖЕ самой нижней
# занятой строки. Чужие записи не трогаем вовсе — ни порядок, ни координаты;
# человек, разложивший стол по-своему, своей раскладки не теряет.
#
# Свежий дом не трогаем: там раскладки ещё нет, все значки новые и двигаться
# нечему — xfdesktop разложит их сам, и файл, который он напишет, будет
# согласован с тем, что видно на экране.
set -u

home=${1:?дом}
owner_u=${2:?владелец}
owner_g=${3:?группа}

layout="$home/.config/xfce4/desktop/icons.screen0.yaml"

# Только обычный файл: симлинк наружу и каталог на этом месте — не наше дело.
[ ! -L "$layout" ] || exit 0
[ -f "$layout" ] || exit 0
[ -d "$home/Desktop" ] || exit 0

# Точка входа работает под root, а дом правит сам пользователь: любой из
# промежуточных каталогов может оказаться ссылкой наружу, и тогда запись
# root'ом ушла бы в чужой файл. Проверки одного `-L` на сам файл мало —
# подменяют каталог выше. Поэтому разрешаем путь, только если он разворачивается
# ВНУТРЬ того же дома; та же защита стоит у ~/.pi и ~/.config/zed.
home_real=$(realpath -q "$home") || exit 0
for d in "$home/.config" "$home/.config/xfce4" "$home/.config/xfce4/desktop" "$home/Desktop"; do
    [ ! -L "$d" ] || exit 0
    real=$(realpath -q "$d") || exit 0
    case "$real/" in "$home_real"/*) ;; *) exit 0 ;; esac
done

# Какие ярлыки на столе ещё не упомянуты в раскладке.
missing=
for f in "$home"/Desktop/*.desktop; do
    [ -f "$f" ] || continue
    grep -q "\"$f\":" "$layout" && continue
    missing="$missing$f
"
done
[ -n "$missing" ] || exit 0

tmp=$(mktemp) || exit 0
# Блок `icons:` может быть не один (несколько мониторов) — каждый со своей
# нумерацией строк, поэтому максимум считаем внутри блока. Блок кончается
# строкой с отступом не больше, чем у самого `icons:`, или концом файла.
awk -v list="$missing" '
function flush(   i, p) {
    if (!in_icons) return
    for (i = 1; i <= n; i++) {
        p = F[i]
        if (p == "" || (p in seen)) continue
        max++
        printf "%*s\"%s\":\n", key_indent, "", p
        printf "%*srow: %d\n", key_indent + 2, "", max
        printf "%*scol: %d\n", key_indent + 2, "", col
    }
    in_icons = 0
}
BEGIN { n = split(list, F, "\n") }
{
    line = $0
    indent = match(line, /[^ ]/) - 1
    if (line ~ /^[ ]*icons:[ ]*$/) {
        flush()
        in_icons = 1; key_indent = indent + 2; max = -1; col = 0
        delete seen
        print; next
    }
    if (in_icons) {
        if (line ~ /^[ ]*$/) { print; next }
        if (indent <= key_indent - 2) { flush(); print; next }
        if (match(line, /^[ ]*"[^"]+":[ ]*$/)) {
            p = line; sub(/^[ ]*"/, "", p); sub(/":[ ]*$/, "", p); seen[p] = 1
        }
        if (match(line, /^[ ]*row:[ ]*[0-9]+[ ]*$/)) {
            r = line; sub(/^[ ]*row:[ ]*/, "", r); r += 0
            if (r > max) max = r
        }
        if (match(line, /^[ ]*col:[ ]*[0-9]+[ ]*$/)) {
            c = line; sub(/^[ ]*col:[ ]*/, "", c); col = c + 0
        }
        print; next
    }
    print
}
END { flush() }
' "$layout" > "$tmp" || { rm -f "$tmp"; exit 0; }

# Пустой или укоротившийся результат не ставим: раскладка — чужие данные, и
# потерять её молча хуже, чем оставить значок не на месте.
[ -s "$tmp" ] || { rm -f "$tmp"; exit 0; }
[ "$(wc -c <"$tmp")" -ge "$(wc -c <"$layout")" ] || { rm -f "$tmp"; exit 0; }

install -o "$owner_u" -g "$owner_g" -m 0644 "$tmp" "$layout" \
    && echo "раскладка стола: $(echo "$missing" | grep -c . ) новых значка(ов) дописаны вниз ($home)"
rm -f "$tmp"
