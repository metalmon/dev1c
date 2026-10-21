#!/bin/sh
# Собрать комплект администратора под Linux целиком и воспроизводимо.
#
# Зачем отдельный скрипт: pack.sh раскладывает по комплекту ГОТОВЫЕ файлы, а
# собрать их — образы, том платформы, суммы — должен кто-то ещё. Раньше этим
# «кто-то» были руки, и комплект получался невоспроизводимым: в новой сессии
# никто не знал, из чего он собран.
#
#   sh scripts/build-kit-linux.sh [каталог комплекта]
#
# По умолчанию кладёт в dist/dev1c-kit-linux. Что нужно до запуска:
#
#   artifacts/1c-connect-linux   бинарь коннектора из CI (без него образ
#                                соберётся, но без окна коннектора)
#   bins/dev1c-setup             бинарь установщика из CI (Linux)
#   $PLATFORM_DIR                дистрибутив 1С и шаблон базы (по умолчанию
#                                platform/, переопределяется DEV1C_PLATFORM_DIR)
#   model/*.gguf                 модель (или MODEL_FILE=путь)
#
# Переменные: DEV1C_PLATFORM_DIR, MODEL_FILE, BINS_DIR, LLAMA_REF.
set -eu

KIT=${1:-dist/dev1c-kit-linux}
BINS=${BINS_DIR:-bins}
MODEL=${MODEL_FILE:-}
LLAMA=${LLAMA_REF:-ghcr.io/metalmon/dev1c-llama:b11734}
TMP=${TMPDIR:-/tmp}/dev1c-kit-build

say() { printf '\n== %s\n' "$*"; }
die() { printf 'build-kit-linux: %s\n' "$*" >&2; exit 1; }

[ -f "$BINS/dev1c-setup" ] || die "нет $BINS/dev1c-setup — возьмите артефакт dev1c-setup-linux из CI"

mkdir -p "$KIT" "$TMP"

# --- образы --------------------------------------------------------------
# Среда и стенд в один архив: стенд собран FROM среды, общие слои лежат там
# один раз. Сервер модели — отдельно: у него другая база, общего с ними нет
# (спека, раздел «Комплекты»).
say "образы среды и стенда"
./provision.sh build

say "архив среды и стенда"
docker save oktaplus/dev1c:latest oktaplus/dev1c-1c:latest | gzip -1 > "$KIT/dev1c-image.tar.gz"
sh scripts/check-image-archive.sh "$KIT/dev1c-image.tar.gz"

say "архив сервера модели"
docker pull "$LLAMA"
docker tag "$LLAMA" oktaplus/dev1c-llama:latest
# Образ в реестре — индекс с манифестом подписей, и docker save отдаёт по
# нему пустышку, возвращая при этом ноль. Пересобираем его в одноплатформенный
# одной строкой FROM: так в архив попадают слои, а не оглавление.
printf 'FROM %s\n' "$LLAMA" > "$TMP/Dockerfile.llama"
docker build --platform linux/amd64 -f "$TMP/Dockerfile.llama" -t oktaplus/dev1c-llama:latest "$TMP"
docker save oktaplus/dev1c-llama:latest | gzip -1 > "$KIT/dev1c-llama-image.tar.gz"
sh scripts/check-image-archive.sh "$KIT/dev1c-llama-image.tar.gz"

# --- том платформы и демо-базы -------------------------------------------
say "том платформы и демо-базы"
./provision.sh export "$KIT/dev1c-1c-payload.tar.gz"

# --- пакеты для доступа к карте ------------------------------------------
say "NVIDIA Container Toolkit"
ver=$(sed -n 's/^NVIDIA_TOOLKIT_VERSION=//p' pack.sh)
mkdir -p "$TMP/nvidia"
for pkg in nvidia-container-toolkit nvidia-container-toolkit-base libnvidia-container1 libnvidia-container-tools; do
    f="${pkg}_${ver}_amd64.deb"
    [ -f "$TMP/nvidia/$f" ] || curl -fsSL -o "$TMP/nvidia/$f" \
        "https://nvidia.github.io/libnvidia-container/stable/deb/amd64/$f"
done

# --- раскладка комплекта --------------------------------------------------
say "раскладка"
NVIDIA_TOOLKIT_DIR="$TMP/nvidia" OUT_DIR=$(dirname "$KIT") \
    ./pack.sh --os linux "$KIT/dev1c-image.tar.gz" "$BINS"

# --- модель ---------------------------------------------------------------
if [ -n "$MODEL" ]; then
    say "модель"
    mkdir -p "$KIT/model"
    cp "$MODEL" "$KIT/model/"
fi
name=$(ls "$KIT"/model/*.gguf 2>/dev/null | head -1 || true)
[ -n "$name" ] || die "в $KIT/model нет .gguf — положите модель или задайте MODEL_FILE"

# Путь к модели в параметрах: образец печатает его пустым, и заказчик на
# пустом поле упирается в «модель не выбрана» при живой модели рядом.
sed -i "s|^file = \"\"|file = \"model/$(basename "$name")\"|" "$KIT/dev1c.toml"

# --- суммы ----------------------------------------------------------------
# Считает сам установщик: заказчик проверяет комплект тем же кодом.
say "контрольные суммы"
"$KIT/dev1c-setup" --kit "$KIT" checksums

say "готово: $KIT"
du -sh "$KIT"
