#!/bin/sh
# Сборка комплекта поставки dev1c-kit.
#
#   ./kit.sh plan     что войдёт, чего не хватает — ничего не делает
#   ./kit.sh build    собрать комплект в dist/dev1c-kit
#
# Комплект один, частей две, и это разные адресаты:
#
#   server-linux/        администратору сервера, один раз: образы, тома,
#                        установщик, инструкция;
#   connector-windows/   разработчику 1С, на флешку: коннектор под Windows
#                        и инструменты, которые он раскладывает на машину.
#
# Почему архивы образов И томов. Образ несёт программы, а платформа 1С с
# демо-базой лежит в томах: дистрибутив 1С лицензионный, в репозиторий и в
# CI он не попадает, собирается только здесь. Без архива томов заказчик
# получит пустой стенд, который нечем наполнить (см. provision.sh export).
#
# Чего в комплекте нет и не будет: файла модели (.gguf) — его приносит
# заказчик под свою карту, и движка Docker — он ставится до нас.
set -eu

export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

OUT=${DEV1C_KIT_OUT:-dist/dev1c-kit}
IMAGE_BASE=${DEV1C_BASE:-oktaplus/dev1c:latest}
IMAGE_STAND=${DEV1C_IMAGE:-oktaplus/dev1c-1c:latest}
PAYLOAD_IMAGE=${DEV1C_PAYLOAD:-oktaplus/dev1c-1c-payload:latest}
ARTIFACTS=${DEV1C_ARTIFACTS:-artifacts}

say()  { printf '[kit] %s\n' "$*"; }
warn() { printf '[kit] %s\n' "$*" >&2; }
die()  { printf '[kit] %s\n' "$*" >&2; exit 1; }

have_image() { docker image inspect "$1" >/dev/null 2>&1; }

# Что кладём и откуда. Пара «источник → место в комплекте»; отсутствие
# источника не молчит, а печатается в plan и останавливает build.
check_one() { # $1 — что, $2 — есть ли (да/нет), $3 — пояснение
    if [ "$2" = да ]; then
        printf '  [есть]  %s\n' "$1"
    else
        printf '  [НЕТ]   %s — %s\n' "$1" "$3"
        MISSING=$((MISSING + 1))
    fi
}

plan() {
    MISSING=0
    echo "Серверная часть (server-linux):"
    check_one "образ среды $IMAGE_BASE" \
        "$(have_image "$IMAGE_BASE" && echo да || echo нет)" \
        "собрать: docker build -t $IMAGE_BASE ."
    check_one "образ стенда $IMAGE_STAND" \
        "$(have_image "$IMAGE_STAND" && echo да || echo нет)" \
        "собрать: ./provision.sh build"
    check_one "том платформы и демо-базы (payload)" \
        "$(have_image "$PAYLOAD_IMAGE" && echo да || echo нет)" \
        "собрать: ./provision.sh build (нужен каталог platform/)"
    check_one "установщик dev1c-setup (Linux)" \
        "$([ -f "$ARTIFACTS/dev1c-setup" ] && echo да || echo нет)" \
        "артефакт CI dev1c-setup-linux, положить в $ARTIFACTS/"
    check_one "инструкция config/kit/УСТАНОВКА.md" \
        "$([ -f config/kit/УСТАНОВКА.md ] && echo да || echo нет)" \
        "есть в репозитории"
    check_one "пакеты NVIDIA Container Toolkit" \
        "$([ -d nvidia-container-toolkit ] && echo да || echo нет)" \
        "скачиваются отдельно, см. УСТАНОВКА.md"

    echo "Коннектор под Windows (connector-windows):"
    check_one "1c-connect.exe" \
        "$([ -f "$ARTIFACTS/1c-connect.exe" ] && echo да || echo нет)" \
        "артефакт CI 1c-connect-windows, положить в $ARTIFACTS/"
    check_one "mcp-1c-windows-amd64.exe" \
        "$([ -f "$ARTIFACTS/mcp-1c-windows-amd64.exe" ] && echo да || echo нет)" \
        "артефакт CI mcp-1c, положить в $ARTIFACTS/"
    check_one "расширение MCP_HTTPService.cfe" \
        "$([ -f "$ARTIFACTS/MCP_HTTPService.cfe" ] && echo да || echo нет)" \
        "артефакт CI mcp-1c, положить в $ARTIFACTS/"
    check_one "инструменты vrunner/ и git/ под Windows" \
        "$([ -d "$ARTIFACTS/vrunner" ] && [ -d "$ARTIFACTS/git" ] && echo да || echo нет)" \
        "собираются pack.sh коннектора, положить в $ARTIFACTS/"

    echo
    if [ "$MISSING" -eq 0 ]; then
        say "всё на месте, можно собирать: ./kit.sh build"
    else
        say "не хватает частей: $MISSING — комплект сейчас не собрать"
    fi
    return 0
}

save_image() { # $1 — образ, $2 — файл
    say "сохраняю $1 → $2 (минуты, образ несколько гигабайт)"
    docker save "$1" | gzip > "$2.part"
    mv "$2.part" "$2"
}

build() {
    MISSING=0
    plan >/dev/null
    [ "$MISSING" -eq 0 ] || die "не хватает частей — запустите ./kit.sh plan"

    rm -rf "$OUT"
    mkdir -p "$OUT/server-linux" "$OUT/connector-windows"

    save_image "$IMAGE_BASE"  "$OUT/server-linux/dev1c-image.tar.gz"
    save_image "$IMAGE_STAND" "$OUT/server-linux/dev1c-1c-image.tar.gz"

    # Том, а не образ: платформа 1С и демо-база выгружаются тем же
    # способом, которым их потом вносят обратно (provision.sh import).
    say "выгружаю платформу и демо-базу в архив тома (около 4 ГБ)"
    docker run --rm "$PAYLOAD_IMAGE" 1c-stand export-payload \
        > "$OUT/server-linux/dev1c-1c-payload.tar.gz.part"
    mv "$OUT/server-linux/dev1c-1c-payload.tar.gz.part" \
       "$OUT/server-linux/dev1c-1c-payload.tar.gz"

    cp "$ARTIFACTS/dev1c-setup" "$OUT/server-linux/"
    cp config/kit/УСТАНОВКА.md  "$OUT/server-linux/"
    [ -d nvidia-container-toolkit ] && cp -r nvidia-container-toolkit "$OUT/server-linux/"

    cp "$ARTIFACTS/1c-connect.exe"            "$OUT/connector-windows/"
    cp "$ARTIFACTS/mcp-1c-windows-amd64.exe"  "$OUT/connector-windows/"
    cp "$ARTIFACTS/MCP_HTTPService.cfe"       "$OUT/connector-windows/"
    cp -r "$ARTIFACTS/vrunner" "$ARTIFACTS/git" "$OUT/connector-windows/"

    cp config/kit/ЧТО-ЗДЕСЬ.md "$OUT/"

    say "комплект собран: $OUT"
    du -sh "$OUT" "$OUT"/* 2>/dev/null || true
}

case "${1:-}" in
plan)  plan ;;
build) build ;;
*)     echo "usage: ./kit.sh plan | build" >&2; exit 2 ;;
esac
