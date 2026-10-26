#!/bin/sh
# Раскладка файлов pi по домам: проверяется не наличие файлов в образе, а то,
# что правка из образа доезжает до УЖЕ созданного дома. Дома живут на томе и
# переживают обновление образа — на этом уже попадались с панелью XFCE, когда
# правка размера надписи раскладки не доходила ни до кого.
#
# Запускается от root внутри контейнера: сценарии переставляют каталоги в доме,
# а раскладку выполняет сама точка входа через крючок DEV1C_SEED_ONLY — то есть
# проверяется настоящий код, а не его пересказ.
#
# Подопытный дом — dev3: он последний по счёту и в пилоте обычно пустой.
set -eu

fail() { echo "FAIL: $*" >&2; exit 1; }
ok()   { echo "  ok: $*"; }

[ "$(id -u)" = 0 ] || fail "проверка запускается от root"

# Свой каталог под рабочие файлы, а не постоянные имена в /tmp. Проверка идёт
# от root, а /tmp в контейнере открыт на запись любому: подложенная ссылка на
# месте журнала увела бы вывод точки входа куда угодно, а подменённый образец
# настроек root скопировал бы в /etc/skel и разослал по всем домам. Тот же
# дефект, что был в test-pi.
WORK=$(mktemp -d)
SKEL_SAVE=$WORK/settings.json.image

# Образец в образе подменяется на время одного сценария. Если проверка упадёт
# между подменой и возвратом, подделка останется в /etc/skel и уедет во все
# дома, созданные позже, — поэтому возврат в trap, а не по прямой.
cleanup() {
    [ -s "$SKEL_SAVE" ] && cp "$SKEL_SAVE" /etc/skel/.pi/agent/settings.json
    rm -rf "$WORK"
    return 0
}
trap cleanup EXIT INT TERM

U=${DEV1C_SEED_USER:-dev3}
HOME_DIR=/home/$U
SKEL=/etc/skel/.pi/agent
STAMPS=$HOME_DIR/.config/.from-image
PI_FILES="AGENTS.md mcp.json settings.json extensions/llama-single.js"

[ -d "$HOME_DIR/.config/zed" ] || fail "у $U нет дома с настройками Zed — раскладка его пропустит"

LOG=$WORK/seed.log
seed() { DEV1C_SEED_ONLY=1 /usr/local/bin/entrypoint > "$LOG" 2>&1 || fail "точка входа упала: $(tail -3 "$LOG")"; }

# Проверка трижды сносит ~/.pi, а там pi держит сессии, auth.json и
# models-store.json. В сборке дом пустой; запущенная на живом стенде она без
# предупреждения стёрла бы историю работы. Поэтому: своё — сносим, чужое — нет.
if [ -d "$HOME_DIR/.pi" ]; then
    own="agent AGENTS.md mcp.json settings.json extensions"
    for e in $(ls -A "$HOME_DIR/.pi" 2>/dev/null) $(ls -A "$HOME_DIR/.pi/agent" 2>/dev/null); do
        # `.bak` бывает у любого нашего файла — это и есть след раскладки, а
        # не чужое добро: сравниваем имя без него.
        case " $own " in *" ${e%.bak} "*) ;; *)
            fail "в доме $U есть своё ($e) — проверка снесла бы это; задайте DEV1C_SEED_USER на пустой дом" ;;
        esac
    done
fi

echo "== Дом прежнего образа =="

# Дом есть, каталога .pi в нём нет — ровно то состояние, в котором окажется
# стенд, обновлённый на образ с pi.
rm -rf "$HOME_DIR/.pi"
rm -f "$STAMPS"/pi_*
seed

[ -d "$HOME_DIR/.pi" ] || fail "каталог ~/.pi не создан"
[ "$(stat -c %U "$HOME_DIR/.pi")" = "$U" ] \
    || fail "~/.pi принадлежит не пользователю, а $(stat -c %U "$HOME_DIR/.pi")"
for f in $PI_FILES; do
    [ -s "$HOME_DIR/.pi/agent/$f" ] || fail "файл $f в дом не приехал"
    [ "$(stat -c %U "$HOME_DIR/.pi/agent/$f")" = "$U" ] \
        || fail "$f принадлежит не пользователю, а $(stat -c %U "$HOME_DIR/.pi/agent/$f")"
done
ok "каталог создан, все файлы приехали и принадлежат $U"

# Порты туннеля подставлены — те же номера, что в настройках Zed этого дома.
n=${U#dev}
grep -q "\"$((7770 + n))\"" "$HOME_DIR/.pi/agent/mcp.json" \
    || fail "порт mcp-1c $((7770 + n)) не подставлен"
grep -q "\"$((7780 + n))\"" "$HOME_DIR/.pi/agent/mcp.json" \
    || fail "порт vrunner $((7780 + n)) не подставлен"
! grep -q PLACEHOLDER "$HOME_DIR/.pi/agent/mcp.json" || fail "в mcp.json осталась подстановка"
ok "порты серверов MCP подставлены"

echo "== Правленный человеком файл =="

# Общее правило отпечатка: файл в образе поменялся — дом получает новый, а
# правка человека остаётся рядом как .bak. Пока образ не менялся, правку никто
# не трогает — иначе .bak терял бы смысл.
dst=$HOME_DIR/.pi/agent/settings.json
cp "$SKEL/settings.json" "$SKEL_SAVE"
printf '%s\n' '// правка человека' >> "$dst"
rm -f "$dst.bak"

seed
grep -q 'правка человека' "$dst" || fail "правку заменили, хотя образ не менялся"
[ -f "$dst.bak" ] && fail "появился .bak, хотя образ не менялся"
ok "образ не менялся — правка человека на месте, .bak нет"

# Теперь файл в образе другой: так выглядит обновление стенда.
printf '%s\n' '{"theme": "system", "quietStartup": true}' > "$SKEL/settings.json"
seed
cp "$SKEL_SAVE" "$SKEL/settings.json"

grep -q quietStartup "$dst" || fail "новый файл из образа в дом не приехал"
[ -s "$dst.bak" ] || fail "прежнего файла нет рядом как .bak"
grep -q 'правка человека' "$dst.bak" || fail "в .bak не правка человека"
[ "$(stat -c %U "$dst.bak")" = "$U" ] || fail ".bak принадлежит не пользователю"
ok "образ поменялся — файл заменён, правка человека лежит рядом как .bak"

echo "== На месте ~/.pi обычный файл =="

# Человек заглядывает в ~/.pi руками, и `rm -rf ~/.pi && touch ~/.pi` — вещь
# достижимая. Точка входа работает с `set -eu`: падение здесь означает, что
# контейнер не доходит ни до Gitea, ни до xrdp, ни до sshd — то есть один
# пользователь не поднимает стенд НИКОМУ. Пропустить такой дом и идти дальше.
rm -rf "$HOME_DIR/.pi"
: > "$HOME_DIR/.pi"
chown "$U:$U" "$HOME_DIR/.pi"

seed
grep -q "skipping $HOME_DIR/.pi" "$LOG" \
    || fail "точка входа не сказала, что пропускает дом: $(grep -i pi "$LOG" | head -3)"
[ -f "$HOME_DIR/.pi" ] || fail "файл на месте ~/.pi подменён каталогом"
ok "обычный файл на месте ~/.pi пропущен, точка входа дошла до конца"

echo "== Символическая ссылка на месте ~/.pi =="

# Та же защита, что у .config: точка входа работает от root и писала бы по
# ссылке туда, куда та указывает.
rm -rf "$HOME_DIR/.pi"
mkdir -p "$WORK/elsewhere"
ln -s "$WORK/elsewhere" "$HOME_DIR/.pi"

seed
grep -q "skipping $HOME_DIR/.pi: symlink" "$LOG" \
    || fail "точка входа не сказала, что пропускает ссылку: $(grep -i pi "$LOG" | head -3)"
[ -z "$(ls -A "$WORK/elsewhere")" ] || fail "по ссылке что-то записано: $(ls -A "$WORK/elsewhere")"
ok "ссылка пропущена, по ней ничего не записано"

echo "== На месте файла — каталог, на месте каталога — файл =="

# Два пути, которыми пользователь валит точку входа целиком, и оба достижимы
# простыми командами в своём доме:
#   mkdir ~/.pi/agent/mcp.json          — `rm -f` по каталогу отказывает;
#   touch ~/.pi/agent/extensions        — `install -d` по файлу отказывает.
# `set -eu` превращает отказ в выход из скрипта, и контейнер не доходит ни до
# Gitea, ни до xrdp, ни до sshd: ОДИН пользователь не поднимает стенд НИКОМУ.
# Срабатывает только когда отпечаток разошёлся — то есть ровно при обновлении
# образа, когда раскладка и должна работать.
rm -rf "$HOME_DIR/.pi"
rm -f "$STAMPS"/pi_*
seed
rm -f "$HOME_DIR/.pi/agent/mcp.json"
mkdir -p "$HOME_DIR/.pi/agent/mcp.json"
rm -rf "$HOME_DIR/.pi/agent/extensions"
: > "$HOME_DIR/.pi/agent/extensions"
chown -R "$U:$U" "$HOME_DIR/.pi"
rm -f "$STAMPS"/pi_*

seed
[ -d "$HOME_DIR/.pi/agent/mcp.json" ] || fail "каталог на месте файла подменён"
[ -f "$HOME_DIR/.pi/agent/extensions" ] || fail "файл на месте каталога подменён"
grep -q "skipping $HOME_DIR/.pi/agent/mcp.json" "$LOG"     || fail "про каталог на месте файла не сказано: $(grep -i skipping "$LOG" | head -3)"
grep -q "skipping $HOME_DIR/.pi/agent/extensions" "$LOG"     || fail "про файл на месте каталога не сказано: $(grep -i skipping "$LOG" | head -3)"
ok "каталог на месте файла и файл на месте каталога пропущены, точка входа дошла до конца"

# Дом в порядок, иначе следующие сценарии начнут с мусора.
rm -rf "$HOME_DIR/.pi"
rm -f "$STAMPS"/pi_*
seed

echo "== Промежуточный каталог — ссылка наружу =="

# Дома живут на томе, и в них остаётся всё, что положил предыдущий гость.
# Проверка на ссылку стоит на последнем звене пути; промежуточное звено
# закрывает другое правило — родитель обязан разрешаться ВНУТРИ корня
# (realpath). Проверяется именно оно: подменим ~/.pi/agent ссылкой наружу.
rm -rf "$HOME_DIR/.pi"
install -d -o "$U" -g "$U" "$HOME_DIR/.pi"
mkdir -p "$WORK/outside"
ln -s "$WORK/outside" "$HOME_DIR/.pi/agent"
chown -h "$U:$U" "$HOME_DIR/.pi/agent"

seed
[ -z "$(ls -A "$WORK/outside")" ]     || fail "по ссылке наружу записано: $(ls -A "$WORK/outside")"
grep -q "skipping $HOME_DIR/.pi/agent" "$LOG"     || fail "точка входа молча прошла мимо ссылки: $(grep -i "\.pi" "$LOG" | head -3)"
ok "промежуточный каталог-ссылка наружу отвергнут, по нему ничего не записано"

echo "== ~/.config/zed подменён ссылкой на дом соседа =="

# Та же защита для цикла подстановки в настройках Zed: подменив промежуточный
# каталог ссылкой, рабочее место заставило бы root переписать чужие настройки.
NEIGH=${DEV1C_SEED_NEIGHBOUR:-dev2}
if [ -d "/home/$NEIGH/.config/zed" ]; then
    before=$(md5sum "/home/$NEIGH/.config/zed/settings.json" | cut -d" " -f1)
    mv "$HOME_DIR/.config/zed" "$HOME_DIR/.config/zed.real"
    ln -s "/home/$NEIGH/.config/zed" "$HOME_DIR/.config/zed"
    chown -h "$U:$U" "$HOME_DIR/.config/zed"

    seed
    after=$(md5sum "/home/$NEIGH/.config/zed/settings.json" | cut -d" " -f1)
    rm -f "$HOME_DIR/.config/zed"
    mv "$HOME_DIR/.config/zed.real" "$HOME_DIR/.config/zed"
    [ "$before" = "$after" ] || fail "настройки соседа $NEIGH переписаны через ссылку"
    ok "ссылка на дом соседа отвергнута, чужие настройки не тронуты"
fi

echo "== На месте ~/Desktop обычный файл =="

# `touch ~/Desktop` достижим пользователю, а раскладка ярлыков делала там
# install -d. С `set -eu` это значит выход из точки входа: один пользователь
# не поднимает стенд НИКОМУ.
rm -rf "$HOME_DIR/Desktop"
: > "$HOME_DIR/Desktop"
chown "$U:$U" "$HOME_DIR/Desktop"
seed
grep -q "skipping $HOME_DIR/Desktop" "$LOG"     || fail "про файл на месте ~/Desktop не сказано: $(grep -i desktop "$LOG" | head -2)"
[ -f "$HOME_DIR/Desktop" ] || fail "файл на месте ~/Desktop подменён каталогом"
ok "обычный файл на месте ~/Desktop пропущен, точка входа дошла до конца"

# Дом в порядок: каталог и ярлыки из образа.
rm -f "$HOME_DIR/Desktop"
seed
ls "$HOME_DIR/Desktop"/*.desktop >/dev/null 2>&1 || fail "ярлыки в дом не вернулись"
ok "ярлыки разложены из образа"

echo "== Новый значок встаёт НИЖЕ старых =="

# xfdesktop кладёт появившийся ярлык в первое свободное место сверху, и старые
# значки переезжают. Онборд наводит стрелку по координатам из этого файла, а
# пишется он не сразу — съехавший значок означает стрелку на соседа. Старые
# записи обязаны остаться на своих строках, новые — уйти вниз.
L="$HOME_DIR/.config/xfce4/desktop/icons.screen0.yaml"
install -d -o "$U" -g "$U" "$(dirname "$L")"
cat > "$L" <<YAML
configs:
- level: 0
  icons:
    "$HOME_DIR":
      row: 0
      col: 0
    "$HOME_DIR/Desktop/gitea.desktop":
      row: 2
      col: 0
    "$HOME_DIR/Desktop/1c-start.desktop":
      row: 1
      col: 0
YAML
chown "$U:$U" "$L"
seed
grep -q "\"$HOME_DIR/Desktop/gitea.desktop\":" "$L" || fail "запись Gitea пропала из раскладки"
gitea_row=$(grep -A1 "\"$HOME_DIR/Desktop/gitea.desktop\":" "$L" | sed -n 's/.*row: *//p')
[ "$gitea_row" = 2 ] || fail "Gitea переехала с 2-й строки на $gitea_row — старые значки двигать нельзя"
chat_row=$(grep -A1 "\"$HOME_DIR/Desktop/model-chat.desktop\":" "$L" | sed -n 's/.*row: *//p')
[ -n "$chat_row" ] || fail "нового значка нет в раскладке: $(cat "$L")"
[ "$chat_row" -gt 2 ] || fail "«Чат с моделью» встал на строку $chat_row — выше занятых"
ok "новые значки дописаны вниз ($chat_row), старые остались на месте"

# Второй проход ничего не меняет: иначе каждая перезагрузка стенда плодила бы
# дубли и уводила значки всё ниже.
sum=$(md5sum "$L" | cut -d" " -f1)
seed
[ "$sum" = "$(md5sum "$L" | cut -d" " -f1)" ] || fail "повторный прогон переписал раскладку"
ok "повторный прогон раскладку не трогает"

# Промежуточный каталог — ссылка наружу. Дом правит пользователь, точка входа
# работает под root: подменил каталог выше файла — и запись ушла бы в чужое.
OUT=$(mktemp -d)
cp "$L" "$OUT/icons.screen0.yaml"
before=$(md5sum "$OUT/icons.screen0.yaml" | cut -d" " -f1)
rm -rf "$HOME_DIR/.config/xfce4"
ln -s "$OUT" "$HOME_DIR/.config/xfce4"
seed
[ "$before" = "$(md5sum "$OUT/icons.screen0.yaml" | cut -d" " -f1)" ] \
    || fail "по ссылке наружу раскладка переписана root'ом"
rm -f "$HOME_DIR/.config/xfce4"
rm -rf "$OUT"
ok "каталог-ссылка наружу отвергнут, чужой файл не тронут"

echo "== Исчезнувшая переменная уходит из сессий =="

# /etc/environment раньше только дописывался: переменная, исчезнувшая из
# конфигурации запуска, оставалась во всех сессиях RDP навсегда. Стенд
# переставили с локальной модели на удалённую — а мёртвый LLAMA_API_URL
# остался, и его читали и редактор, и pi.
saved_env=${LLAMA_API_URL:-}
LLAMA_API_URL="http://proba-ischeznet:9999"; export LLAMA_API_URL
seed
grep -q "^LLAMA_API_URL=http://proba-ischeznet:9999$" /etc/environment     || fail "переменная не доехала до /etc/environment"
unset LLAMA_API_URL
seed
grep -q "^LLAMA_API_URL=" /etc/environment     && fail "переменная осталась в /etc/environment после того, как исчезла из запуска"
ok "исчезнувшая переменная убрана из /etc/environment"
if [ -n "$saved_env" ]; then LLAMA_API_URL=$saved_env; export LLAMA_API_URL; seed; fi

echo "== Скиллы: новый, нетронутый, правленый =="

# Скиллы — знание агента, и до 26.10.2026 они доезжали только в новый дом.
# Затирать их нельзя: правила агента говорят, что каталог его. Поэтому три
# разных случая, и все три проверяются делом.
SK=/etc/skel/.agents/skills
HS=$HOME_DIR/.agents/skills
rm -rf "$WORK/skel-skills" && cp -a "$SK" "$WORK/skel-skills"

# 1. Новый скилл в образе — появляется в доме.
mkdir -p "$SK/проба-нового"
printf -- "---
name: проба-нового
description: проба
---
тело
" > "$SK/проба-нового/SKILL.md"
rm -f "$STAMPS"/skill_*
seed
[ -s "$HS/проба-нового/SKILL.md" ] || fail "новый скилл в дом не приехал"
[ "$(stat -c %U "$HS/проба-нового/SKILL.md")" = "$U" ] || fail "новый скилл принадлежит не пользователю"
ok "новый скилл из образа приехал в существующий дом"

# 2. Образ поменялся, в доме не трогали — обновляется.
printf -- "---
name: проба-нового
description: проба
---
тело второй версии
" > "$SK/проба-нового/SKILL.md"
seed
grep -q "второй версии" "$HS/проба-нового/SKILL.md" || fail "нетронутый скилл не обновился"
ok "нетронутый скилл обновился из образа"

# 3. Правлен на месте — остаётся как есть, и об этом сказано.
printf -- "правка агента
" >> "$HS/проба-нового/SKILL.md"
printf -- "---
name: проба-нового
description: проба
---
тело третьей версии
" > "$SK/проба-нового/SKILL.md"
seed
grep -q "правка агента" "$HS/проба-нового/SKILL.md" || fail "правка агента затёрта обновлением"
grep -q "третьей версии" "$HS/проба-нового/SKILL.md" && fail "правленый скилл всё-таки заменён"
grep -q "skill проба-нового: правлен на месте" "$LOG"     || fail "про правленый скилл в журнале ни слова: $(grep -i skill "$LOG" | head -3)"
ok "правленый скилл не тронут, и это сказано в журнале"

# 4. Висячая ссылка на месте скилла: по ней ничего не создаётся.
rm -rf "$HS/проба-нового"
ln -s "$WORK/skill-elsewhere" "$HS/проба-нового"
chown -h "$U:$U" "$HS/проба-нового"
rm -f "$STAMPS/skill_проба-нового"
seed
[ -e "$WORK/skill-elsewhere" ] && fail "root создал файл по висячей ссылке"
grep -q "skill проба-нового: ссылка" "$LOG"     || fail "про ссылку на месте скилла в журнале ни слова"
ok "висячая ссылка на месте скилла отвергнута"
rm -f "$HS/проба-нового"

# Образ и дом возвращаются к исходному.
rm -rf "$SK" && cp -a "$WORK/skel-skills" "$SK"
rm -rf "$HS/проба-нового"
rm -f "$STAMPS"/skill_*

echo "== Адрес модели задан с завершающим /v1 =="

# Zed дописывает путь сам: с /v1 в адресе выходит /v1/v1/models, llama.cpp
# отвечает 404, и панель агента показывает «моделей нет» без единой строки о
# причине. dev1c-setup кладёт адрес без /v1, но переменную задают и руками —
# точка входа обязана срезать. Поймано 25.10.2026 на живом стенде: pi отвечал,
# панель молчала.
ZED=$HOME_DIR/.config/zed/settings.json
saved_url=${LLAMA_API_URL:-}
LLAMA_API_URL="http://proba-model:8099/v1"; export LLAMA_API_URL
rm -f "$STAMPS/zed_settings.json"
seed
grep -q '"api_url": "http://proba-model:8099"' "$ZED"     || fail "завершающий /v1 не срезан: $(grep -o '"api_url": "[^"]*"' "$ZED" | head -1)"
ok "завершающий /v1 срезан, Zed получает адрес в своей форме"

# Дом возвращается к настоящему адресу.
if [ -n "$saved_url" ]; then LLAMA_API_URL=$saved_url; export LLAMA_API_URL; else unset LLAMA_API_URL; fi
rm -f "$STAMPS/zed_settings.json"
seed

# Дом возвращается в рабочее состояние: следом в этом же контейнере может
# идти проверка test-pi, и она запускает pi от пользователя. `rm -rf`, а не
# `rm -f`: после сценария со ссылкой на месте ~/.pi остаётся НАСТОЯЩИЙ каталог
# с битой ссылкой внутри, и `rm -f` его не берёт — следующий прогон тогда
# видит в доме «чужое» и отказывается работать.
rm -rf "$HOME_DIR/.pi"
rm -f "$STAMPS"/pi_*
seed
[ -s "$HOME_DIR/.pi/agent/mcp.json" ] || fail "дом не восстановлен после проверки"
rm -f "$dst.bak"

echo "pi: раскладка по домам проверена"
