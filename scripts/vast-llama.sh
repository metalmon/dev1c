#!/bin/sh
# Сервер модели на арендованной машине vast.ai — для отладки и записи демо.
#
# Там контейнер, а не хост с докером: поднять рядом образ нечем, поэтому
# llama-server живёт прямо в машине. Собирать его не нужно и нечем (сорок
# минут с CUDA): бинарники достаются из нашего публичного образа
# ghcr.io/<REPO> тем же curl — токен, манифест, слои. Докер для этого не
# требуется, а образ собран с теми же архитектурами (sm_86, sm_120, PTX 75),
# что уезжают заказчику, — то есть мерим то, что отдаём.
#
#   sh vast-llama.sh                      # модель пилота с HF
#   sh vast-llama.sh <URL или путь к .gguf>
#
# Ручки через переменные: REPO, TAG, PORT, SLOTS, FIT_CTX, DEST, MODEL_DIR.
#
# Слушает ПЕТЛЮ. Ключа у сервера нет (решение 26.10.2026) ради встроенного
# веб-чата llama.cpp, который посылать ключ не умеет; публичный IP vast.ai при
# этом значил бы сервер, открытый всем. Со своей машины:
#
#   ssh -N -L 1235:localhost:8080 -p <порт vast> root@<хост vast>
#
# и в контейнере среды CUSTOM_API_URL=http://host.docker.internal:1235/v1 —
# host.docker.internal здесь НАША машина, туннель кончается на ней.
#
# Запускать в tmux: сервер держит терминал, и обрыв ssh убил бы его.
set -eu

REPO=${REPO:-metalmon/dev1c-llama}
TAG=${TAG:-b11734}
DEST=${DEST:-/opt/llama}
MODEL_DIR=${MODEL_DIR:-/root/models}
PORT=${PORT:-8080}
SLOTS=${SLOTS:-3}
FIT_CTX=${FIT_CTX:-81920}

# Модель пилота (решение 25.10.2026, docs/pilot-acceptance.md). Сумма — та
# самая, что лежит в checksums.txt комплекта, и она же LFS-oid файла на HF:
# сверено 26.10.2026, то есть здесь поднимается байт в байт то, что уехало
# заказчику. Другая модель передаётся аргументом, и тогда сумма не сверяется.
MODEL_URL=${MODEL_URL:-https://huggingface.co/unsloth/GLM-4.7-Flash-GGUF/resolve/main/GLM-4.7-Flash-UD-Q4_K_XL.gguf}
MODEL_SHA=b0d4fbc1211f891b4cfbf2a497160bfe06a49412420068904d426b7a13f4ba7f

say() { printf '%s\n' "$*" >&2; }

# --- бинарники ---------------------------------------------------------------
# Уже лежат — ничего не качаем: инстанс переживает перезапуск сервера, а слои
# это 1,2 ГБ.
if [ ! -x "$DEST/llama-server" ]; then
    say "беру llama-server из ghcr.io/$REPO:$TAG"
    tok=$(curl -fsSL "https://ghcr.io/token?scope=repository:$REPO:pull&service=ghcr.io" \
          | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
    [ -n "$tok" ] || { say "ghcr не выдал токен — образ не публичный?"; exit 1; }
    # Все четыре типа в Accept: на узкий заголовок реестр отвечает 404, а не
    # «не тот тип» — ghcr отдаёт OCI-индекс, docker hub может отдать список
    # по-докеровски, и угадывать тут нечего.
    types='application/vnd.oci.image.index.v1+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.docker.distribution.manifest.v2+json'
    fetch() { curl -fsSL -H "Authorization: Bearer $tok" -H "Accept: $types" "https://ghcr.io/v2/$REPO/manifests/$1"; }
    man=$(fetch "$TAG")
    # Индекс: берём запись amd64/linux. Рядом с ней лежит аттестация сборки
    # (платформа unknown), и без выбора по архитектуре слоями оказались бы её
    # json-файлы вместо сервера. Каждая запись индекса — свой кусок после
    # разреза по `},{`; вложенный "platform":{…} разрезу не мешает.
    case "$man" in *'"manifests"'*)
        d=$(printf '%s' "$man" | tr -d ' \n' | sed 's/},{/}\n{/g' \
            | grep '"architecture":"amd64"' | grep -o 'sha256:[a-f0-9]\{64\}' | head -1)
        [ -n "$d" ] || { say "в индексе $TAG нет образа под amd64"; exit 1; }
        man=$(fetch "$d") ;;
    esac
    # Отрезаем всё до "layers": иначе в список попал бы digest конфига образа.
    layers=$(printf '%s' "$man" | sed 's/.*"layers"//' | grep -o 'sha256:[a-f0-9]\{64\}')
    [ -n "$layers" ] || { say "в манифесте $TAG нет слоёв"; exit 1; }
    tmp=$(mktemp -d)
    for d in $layers; do
        say "  слой ${d#sha256:}"
        curl -fsSL -H "Authorization: Bearer $tok" \
             "https://ghcr.io/v2/$REPO/blobs/$d" | tar -xz -C "$tmp" opt/llama 2>/dev/null || true
    done
    [ -x "$tmp/opt/llama/llama-server" ] || { say "в слоях нет opt/llama/llama-server"; exit 1; }
    mkdir -p "$(dirname "$DEST")"
    rm -rf "$DEST"
    mv "$tmp/opt/llama" "$DEST"
    rm -rf "$tmp"
fi
export LD_LIBRARY_PATH="$DEST"
# Единственная проверка, которая стоит того: разобранный манифест, собранные
# слои и живые библиотеки видно по одной строке версии.
"$DEST/llama-server" --version >&2

# --- модель ------------------------------------------------------------------
src=${1:-$MODEL_URL}
case "$src" in
    http*) mkdir -p "$MODEL_DIR"
           model=$MODEL_DIR/$(basename "${src%%\?*}")
           # Качаем в .part и переименовываем только после успеха: оборванная
           # закачка под настоящим именем выглядела бы готовой моделью. `-C -`
           # добирает недокачанное — 17 ГБ заново не возят.
           if [ ! -f "$model" ]; then
               say "качаю $(basename "$model") (17 ГБ)"
               curl -fL -C - -o "$model.part" "$src"
               # Сверяем только что скачанное, и только модель пилота: минута
               # на 17 ГБ против часа на «почему модель отвечает чепухой».
               # При каждом запуске это была бы минута впустую.
               if [ "$src" = "$MODEL_URL" ]; then
                   say "сверяю сумму"
                   echo "$MODEL_SHA  $model.part" | sha256sum -c - \
                       || { say "сумма не сошлась — файл битый, удалите $model.part"; exit 1; }
               fi
               mv -f "$model.part" "$model"
           fi ;;
    *)     model=$src
           [ -f "$model" ] || { say "файла модели нет: $model"; exit 1; } ;;
esac

# --- карта -------------------------------------------------------------------
# Не преграда, а предупреждение: откажется сервер сам, зато причина названа до
# того, как он час качал модель. 32 ГБ — замер 25.10.2026: три слота по 81920
# это 7,1 ГБ кэша (28 763 байта на токен) плюс 17,5 ГБ весов.
vram=$(nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits 2>/dev/null | head -1 || echo 0)
[ "${vram:-0}" -ge 32000 ] || [ "$SLOTS" -lt 3 ] \
    || say "ВНИМАНИЕ: на карте ${vram} МиБ, трём слотам по $FIT_CTX нужно около 32 ГБ"

# --- запуск ------------------------------------------------------------------
# Строка ровно как у комплекта (setup/dev1c-setup/src/llama.rs, serve_args):
# расхождение значило бы, что демо снято не с того, что уезжает заказчику.
say "поднимаю $(basename "$model") на 127.0.0.1:$PORT, слотов $SLOTS"
exec "$DEST/llama-server" \
    -m "$model" \
    --alias "$(basename "$model" .gguf)" \
    --host 127.0.0.1 --port "$PORT" \
    --parallel "$SLOTS" \
    -ngl 999 \
    --fit on --fit-ctx "$FIT_CTX" \
    --flash-attn on -ctk q8_0 -ctv q8_0 \
    --temp 0.7 --top-p 1.0 --min-p 0.01 \
    --metrics -lv 4
