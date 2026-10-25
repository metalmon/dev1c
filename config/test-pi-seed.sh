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

U=${DEV1C_SEED_USER:-dev3}
HOME_DIR=/home/$U
SKEL=/etc/skel/.pi/agent
STAMPS=$HOME_DIR/.config/.from-image
PI_FILES="AGENTS.md mcp.json settings.json extensions/llama-single.js"

[ -d "$HOME_DIR/.config/zed" ] || fail "у $U нет дома с настройками Zed — раскладка его пропустит"

seed() { DEV1C_SEED_ONLY=1 /usr/local/bin/entrypoint > /tmp/seed.log 2>&1 || fail "точка входа упала: $(tail -3 /tmp/seed.log)"; }

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
cp "$SKEL/settings.json" /tmp/skel-settings.json
printf '%s\n' '// правка человека' >> "$dst"
rm -f "$dst.bak"

seed
grep -q 'правка человека' "$dst" || fail "правку заменили, хотя образ не менялся"
[ -f "$dst.bak" ] && fail "появился .bak, хотя образ не менялся"
ok "образ не менялся — правка человека на месте, .bak нет"

# Теперь файл в образе другой: так выглядит обновление стенда.
printf '%s\n' '{"theme": "system", "quietStartup": true}' > "$SKEL/settings.json"
seed
cp /tmp/skel-settings.json "$SKEL/settings.json"

grep -q quietStartup "$dst" || fail "новый файл из образа в дом не приехал"
[ -s "$dst.bak" ] || fail "прежнего файла нет рядом как .bak"
grep -q 'правка человека' "$dst.bak" || fail "в .bak не правка человека"
[ "$(stat -c %U "$dst.bak")" = "$U" ] || fail ".bak принадлежит не пользователю"
ok "образ поменялся — файл заменён, правка человека лежит рядом как .bak"

echo "== Символическая ссылка на месте ~/.pi =="

# Та же защита, что у .config: точка входа работает от root и писала бы по
# ссылке туда, куда та указывает.
rm -rf "$HOME_DIR/.pi"
mkdir -p /tmp/pi-elsewhere
ln -s /tmp/pi-elsewhere "$HOME_DIR/.pi"

seed
grep -q "skipping $HOME_DIR/.pi: symlink" /tmp/seed.log \
    || fail "точка входа не сказала, что пропускает ссылку: $(grep -i pi /tmp/seed.log | head -3)"
[ -z "$(ls -A /tmp/pi-elsewhere)" ] || fail "по ссылке что-то записано: $(ls -A /tmp/pi-elsewhere)"
ok "ссылка пропущена, по ней ничего не записано"

# Дом возвращается в рабочее состояние: следом в этом же контейнере может
# идти проверка test-pi, и она запускает pi от пользователя.
rm -f "$HOME_DIR/.pi"
rm -f "$STAMPS"/pi_*
seed
[ -s "$HOME_DIR/.pi/agent/mcp.json" ] || fail "дом не восстановлен после проверки"
rm -f "$dst.bak"
rmdir /tmp/pi-elsewhere

echo "pi: раскладка по домам проверена"
