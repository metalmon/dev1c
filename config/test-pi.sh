#!/bin/sh
# Проверка агента pi в образе: бинарь на месте, Node в поставку не приехал,
# расширения из каталога агента грузятся.
#
# Запускается от пользователя стенда (dev1/dev2/dev3), не от root: pi от root
# не запускается, как и Zed. Из сборки зовётся так же, как соседние проверки:
#   docker exec -u dev1 dev1c sh /usr/local/bin/test-pi
#
# Проверяется дело, а не наличие файлов: версия берётся у самого бинаря, а
# загрузка расширения — по строке, которую расширение печатает.
set -eu

fail() { echo "FAIL: $*" >&2; exit 1; }
ok()   { echo "  ok: $*"; }

# Уборка на любом выходе, в том числе через fail. Без неё заглушка сервера
# остаётся жить и держит порт: следующий прогон поднимает свою, получает
# «Address already in use» и дальше отвечает ПРЕЖНЯЯ — то есть проверка
# проходит по чужому серверу. На это уже попались 25.10.2026.
cleanup() {
    [ -n "${probe:-}" ] && rm -f "$probe"
    [ -n "${stub:-}" ] && kill "$stub" 2>/dev/null
    [ -n "${WORK:-}" ] && rm -rf "$WORK"
    return 0
}
trap cleanup EXIT INT TERM

# Свой каталог под вывод, а не постоянные пути в /tmp. Стенд многопользователь-
# ский: файл с постоянным именем в /tmp, оставленный dev1, у dev3 не
# перезаписывается — и проверка читает ЧУЖОЙ вывод, то есть проходит по тому,
# чего сама не делала. Поймано 25.10.2026 прогоном от второго пользователя.
WORK=$(mktemp -d)

# Какую версию ждём — написано в образе той же сборкой, что её поставила
# (ARG PI_VERSION). Своего числа здесь нет: разошлось бы с Dockerfile.
PI_VERSION_WANT=${PI_VERSION_WANT:-$(cat /opt/pi/.version 2>/dev/null)}
[ -n "$PI_VERSION_WANT" ] || fail "в образе нет /opt/pi/.version — нечем сверить версию"

echo "== Бинарь =="

command -v pi >/dev/null || fail "pi не установлен"
pi --version 2>&1 | grep -q "$PI_VERSION_WANT" \
    || fail "pi не отвечает версией $PI_VERSION_WANT"
ok "pi $PI_VERSION_WANT"

# Node нужен только в стадии сборки. Если он здесь — бинарь собран не
# самостоятельным, и в образе выросла лишняя поверхность обновлений.
command -v node >/dev/null && fail "node попал в итоговый образ"
command -v npm  >/dev/null && fail "npm попал в итоговый образ"
ok "Node и npm в образе отсутствуют"

echo "== Расширения =="

# Риск, который спека велит проверять первым: самостоятельный бинарь обязан
# грузить расширение из каталога агента. Не грузит — план с бинарём рассыпается,
# и это надо узнать здесь, а не на стенде.
probe="$HOME/.pi/agent/extensions/probe.js"
mkdir -p "$HOME/.pi/agent/extensions"
cat > "$probe" <<'JS'
export default function (pi) {
    console.log("PROBE-EXTENSION-LOADED");
}
JS
# `--version` расширений не грузит (проверено на v1.4.0), а `--list-models`
# грузит: провайдеры у pi и приходят расширениями. `--offline` — чтобы
# проверка не ходила в сеть за каталогами моделей.
pi --offline --list-models 2>&1 | grep -q "PROBE-EXTENSION-LOADED" \
    || fail "самостоятельный бинарь не загрузил расширение из ~/.pi/agent/extensions"
ok "расширение из каталога агента загружено"
rm -f "$probe"
probe=

echo "== Настройки =="

for f in AGENTS.md mcp.json settings.json; do
    [ -s "$HOME/.pi/agent/$f" ] || fail "нет ~/.pi/agent/$f"
    [ "$(stat -c %U "$HOME/.pi/agent/$f")" = "$(id -un)" ] \
        || fail "$f принадлежит не пользователю, а $(stat -c %U "$HOME/.pi/agent/$f")"
done
ok "три файла настроек на месте и принадлежат пользователю"

# Инструкции — тот же файл, что у Zed. Разойдутся — разойдётся и поведение
# агентов, а понять это по работе невозможно.
cmp -s "$HOME/.pi/agent/AGENTS.md" "$HOME/.config/zed/AGENTS.md" \
    || fail "инструкции pi и Zed разошлись"
ok "инструкции те же, что у Zed"

# Порты туннеля подставлены точкой входа. Оставшаяся подстановка означает, что
# nc пойдёт на порт по имени и сервер молча не поднимется.
grep -q PLACEHOLDER "$HOME/.pi/agent/mcp.json" \
    && fail "в mcp.json осталась подстановка — точка входа не подставила порты"
ok "порты серверов MCP подставлены"

# Серверы видны самому pi. bsl-reference обязан подключиться: он в образе
# целиком. Туннельные 1c и vrunner без рабочего места не подключатся — от них
# требуется только попасть в список.
pi mcp list > "$WORK"/pi-mcp.txt 2>&1 || true
for s in bsl-reference 1c vrunner; do
    grep -q "^$s:" "$WORK"/pi-mcp.txt || fail "pi не видит сервер MCP $s"
done
grep -q "^bsl-reference: connected" "$WORK"/pi-mcp.txt \
    || fail "bsl-reference не подключился: $(grep '^bsl-reference' "$WORK"/pi-mcp.txt)"
ok "серверы MCP видны: bsl-reference подключён, 1c и vrunner объявлены"

echo "== Модель =="

# Порт тоже свой у каждого: 18099 на всех означает, что два пользователя
# стенда не могут проверяться разом, а забытая чужая заглушка отвечает вместо
# своей — и проверка проходит по чужому серверу.
PORT=${PI_TEST_PORT:-$((18000 + $(id -u) % 1000))}
STUB=${PI_TEST_STUB:-/usr/local/bin/test-pi-stub}

# $1 — форма ответа, дальше любые аргументы заглушки (--id, --key, --port=)
stub_start() {
    shape=$1
    shift
    port=$PORT
    for a in "$@"; do [ "$a" = "${a#--port=}" ] || port=${a#--port=}; done
    python3 "$STUB" --port "$port" --shape "$shape" --dump "$WORK"/pi-request.json "$@" &
    stub=$!
    i=0
    while [ "$i" -lt 100 ]; do
        # Без -f: готовность — это «порт отвечает по HTTP», а заглушка с
        # --key на запрос без ключа честно отвечает 401, и -f счёл бы это
        # неготовностью.
        curl -s -o /dev/null "http://127.0.0.1:$port/props" 2>/dev/null && return 0
        i=$((i + 1))
        sleep 0.1
    done
    fail "заглушка сервера модели не поднялась на порту $port"
}
stub_stop() { kill "$stub" 2>/dev/null || true; wait "$stub" 2>/dev/null || true; stub=; }

# 1. Обычный одномодельный llama.cpp: модель видна, окно прочитано у сервера.
stub_start ok
LLAMA_API_URL="http://127.0.0.1:$PORT" pi --offline --list-models >"$WORK"/pi-models.txt 2>&1 || true
grep -q "GLM-4.7-Flash-UD-Q4_K_XL" "$WORK"/pi-models.txt \
    || fail "pi не увидел модель: $(head -3 "$WORK"/pi-models.txt)"
# Окно у pi в столбце рядом с моделью и сокращено до «161.0K». Проверяется
# именно оно: умолчание pi для объявленного провайдера — 128000, то есть
# «128.0K», и спутать нельзя. Заглушка отдаёт 161024 — столько насчитал живой
# сервер на 5090 для этой модели.
grep -qE "GLM-4\.7-Flash-UD-Q4_K_XL[[:space:]]+(161024|161\.0K)" "$WORK"/pi-models.txt \
    || fail "окно не прочитано из meta.n_ctx: $(grep -i glm "$WORK"/pi-models.txt | head -1)"
ok "модель видна, окно взято у сервера (161024)"

# 2. Агент отвечает, и до модели доходят инструкции и скиллы. Это главное, что
#    тут проверяется: файлы могут лежать на месте и не попадать в запрос.
LLAMA_API_URL="http://127.0.0.1:$PORT" pi --offline --no-session --print \
    "скажи слово готово" >"$WORK"/pi-answer.txt 2>&1 || true
grep -q "готово" "$WORK"/pi-answer.txt \
    || fail "агент не ответил через заглушку: $(head -5 "$WORK"/pi-answer.txt)"
[ -s "$WORK"/pi-request.json ] || fail "запрос к модели не дошёл до заглушки"
python3 - "$WORK"/pi-request.json "$HOME/.agents/skills" <<'PY' || fail "в запросе к модели нет инструкций или скиллов"
import json, os, sys
req = json.load(open(sys.argv[1], encoding="utf-8"))
text = "\n".join(
    m.get("content") if isinstance(m.get("content"), str)
    else json.dumps(m.get("content"), ensure_ascii=False)
    for m in req.get("messages", [])
)
root = sys.argv[2]
names = [d for d in os.listdir(root) if os.path.isdir(os.path.join(root, d))]
missing = [n for n in names if n not in text]
if missing:
    print(f"скиллов в запросе нет: {len(missing)} из {len(names)}, например {missing[:3]}")
    sys.exit(1)
# Якорь из правил агента: строка оттуда обязана быть в системной подсказке.
if "OneScript" not in text:
    print("в запросе нет правил агента (искали слово OneScript)")
    sys.exit(1)
print(f"  ok: до модели дошли правила агента и все {len(names)} скиллов")
PY
stub_stop

# 3. Чужой шлюз без meta.n_ctx: модель не регистрируется вовсе. Объявить окно
#    наугад нельзя — агент тихо поглупеет, и никто не поймёт почему.
stub_start no-meta
LLAMA_API_URL="http://127.0.0.1:$PORT" pi --offline --list-models >"$WORK"/pi-nometa.txt 2>&1 || true
grep -q "GLM-4.7-Flash-UD-Q4_K_XL" "$WORK"/pi-nometa.txt \
    && fail "модель без meta.n_ctx зарегистрирована — окно взялось неизвестно откуда"
ok "без meta.n_ctx модель не регистрируется"
stub_stop

# 4. Сервера модели нет вовсе: pi обязан запуститься и сказать об этом, а не
#    упасть. Порт тот же, но заглушки на нём уже нет.
#
#    Код возврата НЕ маскируется, и ищется строка, которую печатает САМ pi:
#    собственное сообщение расширения («сервер модели … не ответил») совпало бы
#    с мягким образцом, и проверка проходила бы, даже если pi после этого падает.
LLAMA_API_URL="http://127.0.0.1:$PORT" pi --offline --list-models >"$WORK"/pi-dead.txt 2>&1 \
    || fail "без сервера модели pi вышел с ошибкой: $(head -3 "$WORK"/pi-dead.txt)"
grep -q "No models available" "$WORK"/pi-dead.txt \
    || fail "без сервера модели pi не сказал своими словами, что моделей нет: $(head -3 "$WORK"/pi-dead.txt)"
ok "без сервера модели pi запускается, выходит без ошибки и говорит, что модели нет"

# 5. Адрес и ключ берутся ПАРОЙ. Вход: стенд удалённый (CUSTOM_*), а
#    LLAMACPP_API_KEY остался в окружении от прежнего локального сервера.
#    Возьмёт расширение ключ из другой пары — заглушка ответит 401, и модели не
#    будет, хотя у Zed она есть.
stub_start ok --key pravilnyi
CUSTOM_API_URL="http://127.0.0.1:$PORT" CUSTOM_API_KEY=pravilnyi \
    LLAMACPP_API_KEY=chuzhoi \
    pi --offline --list-models >"$WORK"/pi-pair.txt 2>&1 || true
grep -q "GLM-4.7-Flash-UD-Q4_K_XL" "$WORK"/pi-pair.txt \
    || fail "адрес и ключ взяты из разных пар: $(head -3 "$WORK"/pi-pair.txt)"
ok "адрес и ключ берутся парой"
stub_stop

# 6. Первый адрес не отвечает, второй отвечает — модель берётся со второго.
#    Иначе у pi модели нет там, где у Zed она есть, и молча.
stub_start ok
LLAMA_API_URL="http://127.0.0.1:$((PORT + 1))" CUSTOM_API_URL="http://127.0.0.1:$PORT" \
    pi --offline --list-models >"$WORK"/pi-fallback.txt 2>&1 || true
grep -q "GLM-4.7-Flash-UD-Q4_K_XL" "$WORK"/pi-fallback.txt \
    || fail "откат на второй адрес не работает: $(head -3 "$WORK"/pi-fallback.txt)"
ok "мёртвый первый адрес — модель берётся со второго"
stub_stop

# 7. Какой провайдер живой, говорит MODEL_PROVIDER — та же переменная, по
#    которой точка входа выбирает провайдера в настройках Zed. Две заглушки с
#    разными именами моделей показывают, какую выбрало расширение.
stub_start ok --id MODEL-LLAMA
llama_stub=$stub
stub_start ok --port="$((PORT + 2))" --id MODEL-CUSTOM
MODEL_PROVIDER=custom \
    LLAMA_API_URL="http://127.0.0.1:$PORT" \
    CUSTOM_API_URL="http://127.0.0.1:$((PORT + 2))" \
    pi --offline --list-models >"$WORK"/pi-provider.txt 2>&1 || true
kill "$llama_stub" 2>/dev/null || true
grep -q "MODEL-CUSTOM" "$WORK"/pi-provider.txt \
    || fail "MODEL_PROVIDER=custom не выбрал адрес custom: $(grep -i MODEL- "$WORK"/pi-provider.txt | head -2)"
ok "MODEL_PROVIDER выбирает, какой из двух адресов живой"
stub_stop

echo "pi: все проверки прошли"
