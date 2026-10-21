#!/bin/sh
# =============================================================================
# The demo stand's payload: the 1C platform and the template base live in two
# named volumes, not in the image.
#
#   ./provision.sh build            both images: the stand and the payload
#   ./provision.sh export [file]    the payload image -> one tar.gz
#   ./provision.sh import [file]    that tar.gz -> the two volumes
#   ./provision.sh run [docker args…]   (re)create the stand container
#   ./provision.sh status           what is in the volumes right now
#
# Fixed names, no discovery: one stand per machine, the same names every time.
#
#   image      oktaplus/dev1c-1c:latest          ~4 GB, rebuilt often
#   payload    oktaplus/dev1c-1c-payload:latest  ~4 GB, rebuilt per 1C release
#   volumes    dev1c-platform -> /opt/1cv8t      the training platform
#              dev1c-demo     -> /opt/1c-demo    the template base
#              dev1c-home, dev1c-gitea           as for the plain dev1c image
#   ports      3390 RDP, 3391 Gitea, 3393 SSH (DEV1C_PORT_BASE moves all three)
#
# DEV1C_NAME renames the container and all four volumes together: a second
# stand beside a live one, and the way `run` is tested without touching it.
# DEV1C_MEMORY is the ceiling for the container as a whole (default 8g).
#
# `run` passes its extra arguments to `docker run`, which is where the model
# settings go — this script never reads or writes a key:
#
#   ./provision.sh run -e MODEL_PROVIDER=llama.cpp -e LLAMA_API_URL=… \
#        -e LLAMACPP_API_KEY=… -e MODEL_NAME=…
#
# The build needs `platform/` (the 1C installer, the demo template, see
# Dockerfile.1c); import and run need only the tar and Docker.
# =============================================================================
set -eu
# Git Bash rewrites anything that looks like a Unix path before docker.exe sees
# it, so `/opt/1cv8t` inside the container turns into a directory under the Git
# installation. Harmless everywhere else; without it this script only works on
# Linux. Container-side paths are also given to `docker run` without a leading
# slash where a PATH lookup will do.
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

IMAGE=oktaplus/dev1c-1c:latest
PAYLOAD_IMAGE=oktaplus/dev1c-1c-payload:latest
# The stand image is built FROM this one (Dockerfile.1c: ARG BASE), and this
# one holds the desktop, Zed, Gitea and xrdp — everything that does not need
# the 1C distribution and is therefore built in CI too. Three images is a
# consequence of the licence, not a taste: the 1C installer cannot live in the
# repository, so whatever needs it builds only here.
BASE_IMAGE=${DEV1C_BASE:-oktaplus/dev1c:latest}
# One knob, everything derived from it: the container and all four volumes are
# named after it, the ports count up from the base. The defaults are the only
# values anybody should use — a second name exists so that a stand can be
# raised next to a live one without touching its volumes, which is also how
# this script gets tested.
NAME=${DEV1C_NAME:-dev1c}
PORT_BASE=${DEV1C_PORT_BASE:-3390}
# A ceiling for the whole container, not a share per workstation: all three
# sit in it together with Gitea, three copies of Zed, the bsl analyzer, the
# Apache workers and the 1C clients. On the limit the kernel kills processes
# inside, which on a stand means a 1C session dying during the demonstration.
# 8g is sized for the stand's machine: 32 GB total, about 20 of them taken by
# the model (its own container, outside this limit), the rest left to the host.
# A smaller machine wants a smaller ceiling — this laptop runs 6g.
MEMORY=${DEV1C_MEMORY:-8g}
# Предел выше наличной памяти ничего не ограничивает: вместо отказа одному
# процессу кончается память у всей машины Docker, и 1С при запуске говорит
# «недостаточно памяти», не называя причины. Поэтому опускаем до 80% того,
# что есть, оставляя ВМ её ядро, страничный кэш (под 1С он большой — база
# читается файлами) и соседние контейнеры. Явно заданный DEV1C_MEMORY
# уважаем, но предупреждаем.
clamp_memory() {
    have=$(docker info --format '{{.MemTotal}}' 2>/dev/null) || return 0
    [ -n "$have" ] && [ "$have" -gt 0 ] 2>/dev/null || return 0
    case "$MEMORY" in
        *g) want=$(( ${MEMORY%g} * 1073741824 )) ;;
        *m) want=$(( ${MEMORY%m} * 1048576 )) ;;
        *)  return 0 ;;
    esac
    cap=$(( have * 80 / 100 ))
    [ "$want" -le "$cap" ] && return 0
    warn "предел памяти $MEMORY выше того, что есть у машины Docker ($(( have / 1073741824 )) ГиБ)"
    if [ -n "${DEV1C_MEMORY:-}" ]; then
        warn "DEV1C_MEMORY задан явно — оставляю как есть, но 1С может сказать «недостаточно памяти»"
        return 0
    fi
    MEMORY="$(( cap / 1048576 ))m"
    warn "опускаю до $MEMORY; поднять можно через DEV1C_MEMORY, если памяти добавится"
}
CONTAINER=$NAME
VOL_PLATFORM=$NAME-platform
VOL_DEMO=$NAME-demo
# Тома рабочих мест и Gitea переопределяются по имени, а платформа с
# демо-базой — нет. Причина в том, что эти два тома единственные, которые
# может понадобиться завести заново, не тронув старые: база рабочего места,
# собранная прошлой платформой, новой не открывается («требуется более новая
# версия платформы»), а seed её не пересоздаёт — он создаёт базу только
# когда её нет. Поднять стенд на свежих томах и оставить прошлые на диске
# целыми — единственный путь, не требующий ничего удалять.
VOL_HOME=${DEV1C_HOME_VOL:-$NAME-home}
VOL_GITEA=${DEV1C_GITEA_VOL:-$NAME-gitea}
# NOT under platform/: that directory is a build context, and 2.4 GB of payload
# would be transferred to the daemon on every build of either target.
TAR=dist/dev1c-1c-payload.tar.gz
# Образы комплекта: среда, стенд поверх неё и сервер модели. Стенд собран
# FROM среды — в одном архиве их общие слои лежат один раз; сервер модели от
# другой базы, поэтому едет отдельным архивом (спека, раздел «Комплекты»).
BASE_IMAGE=oktaplus/dev1c:latest
LLAMA_IMAGE=oktaplus/dev1c-llama:latest
MOUNTS="-v $VOL_PLATFORM:/opt/1cv8t -v $VOL_DEMO:/opt/1c-demo"

say() { printf '[provision] %s\n' "$*"; }
warn() { echo "[provision] $*" >&2; }

# Самый свежий файл, входящий в базовый образ: Dockerfile и config/, кроме
# config/1c/ — тот принадлежит образу стенда.
base_inputs_newest() {
    find Dockerfile config -path 'config/1c' -prune -o -type f -exec stat -c %Y {} + 2>/dev/null         | sort -nr | head -1
}

# Собрать базовый образ, если его нет или он старше собственных исходников.
# Ловушка здесь молчаливая: правка в config/gitea-start.sh попадает в БАЗОВЫЙ
# образ, сборка стенда его не трогает, стенд наследует вчерашнюю копию — и
# человек видит «готово», получив образ без своей правки.
ensure_base() {
    if [ -n "${DEV1C_SKIP_BASE:-}" ]; then
        say "базовый образ пропущен (DEV1C_SKIP_BASE)"
        return 0
    fi
    built=$(docker image inspect "$BASE_IMAGE" --format '{{.Created}}' 2>/dev/null || true)
    if [ -z "$built" ]; then
        say "базового образа $BASE_IMAGE нет — собираю его первым"
        docker build -t "$BASE_IMAGE" . || die "базовый образ не собрался"
        return 0
    fi
    newest=$(base_inputs_newest)
    built_epoch=$(date -d "$built" +%s 2>/dev/null || echo "")
    if [ -z "$built_epoch" ] || [ -z "$newest" ]; then
        warn "не смог сравнить возраст базового образа с его файлами — соберите его сами, если правили Dockerfile или config/"
        return 0
    fi
    if [ "$built_epoch" -lt "$newest" ]; then
        say "базовый образ старше файлов, которые в него входят — собираю заново"
        docker build -t "$BASE_IMAGE" . || die "базовый образ не собрался"
    else
        say "базовый образ свежий"
    fi
}

die() { printf '[provision] %s\n' "$*" >&2; exit 1; }

volumes() {
    for v in "$VOL_PLATFORM" "$VOL_DEMO" "$VOL_HOME" "$VOL_GITEA"; do
        docker volume inspect "$v" >/dev/null 2>&1 || docker volume create "$v" >/dev/null
    done
}

case "${1:-}" in
build)
    # Каталог с дистрибутивом 1С и шаблоном базы. Отдельной переменной,
    # потому что он весит полтора гигабайта и на рабочем диске места под
    # него может не быть: заказчик просил собирать на C.
    PLATFORM_DIR=${DEV1C_PLATFORM_DIR:-platform}
    [ -d "$PLATFORM_DIR" ] || die "$PLATFORM_DIR не найден: там лежат установщик 1С и шаблон базы (DEV1C_PLATFORM_DIR меняет путь)"
    # Сперва базовый образ: стенд собирается FROM него, и правка в config/
    # иначе молча не доедет.
    ensure_base
    mkdir -p artifacts
    # Each target gets only the context it uses, and BuildKit transfers a named
    # context only when a stage mounts it: the stand image pulls 68 MB of
    # artifacts/ instead of 3.8 GB of platform/, which is minutes per rebuild.
    say "the stand image (no platform, no demo base)"
    docker build -f Dockerfile.1c --build-context artifacts=artifacts -t "$IMAGE" .
    say "the payload image (platform + template base)"
    docker build -f Dockerfile.1c --build-context platform="$PLATFORM_DIR" --target payload -t "$PAYLOAD_IMAGE" .
    ;;
export)
    out=${2:-$TAR}
    docker image inspect "$PAYLOAD_IMAGE" >/dev/null 2>&1 || die "no $PAYLOAD_IMAGE: ./provision.sh build"
    mkdir -p "$(dirname "$out")"
    say "writing $out (this takes a while: about 4 GB through gzip)"
    docker run --rm "$PAYLOAD_IMAGE" 1c-stand export-payload > "$out.part"
    mv "$out.part" "$out"
    say "$out: $(du -h "$out" | cut -f1)"
    ;;
import)
    src=${2:-$TAR}
    [ -f "$src" ] || die "no $src: ./provision.sh export first, or name the file"
    docker image inspect "$IMAGE" >/dev/null 2>&1 || die "no $IMAGE: ./provision.sh build"
    volumes
    say "unpacking $src into $VOL_PLATFORM and $VOL_DEMO"
    # Третий аргумент идёт насквозь: это --force, замена уже внесённых томов.
    # Нужен при смене версии платформы — без него import молча ничего не
    # делает и говорит об этом одной строкой, которую легко принять за успех.
    # shellcheck disable=SC2086
    docker run --rm -i $MOUNTS "$IMAGE" 1c-stand import-payload ${3:-} < "$src"
    ;;
run)
    shift
    clamp_memory
    volumes
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    # shellcheck disable=SC2086
    docker run -d --name "$CONTAINER" --restart unless-stopped --init \
        --shm-size 2g --memory "$MEMORY" --cap-add SYS_ADMIN \
        --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
        --device /dev/fuse --add-host host.docker.internal:host-gateway \
        -p "$PORT_BASE:3389" -p "$((PORT_BASE + 1)):3000" -p "$((PORT_BASE + 3)):2022" \
        -v "$VOL_HOME:/home" -v "$VOL_GITEA:/var/lib/gitea" $MOUNTS \
        "$@" "$IMAGE" >/dev/null
    say "$CONTAINER is up: RDP $PORT_BASE, Gitea $((PORT_BASE + 1)), SSH $((PORT_BASE + 3))"
    say "the check of a running stand: docker exec $CONTAINER test-stand"
    ;;
kit)
    # Комплект администратора под Linux целиком: образы, оба архива, том
    # платформы, пакеты NVIDIA, раскладка, путь к модели и суммы.
    #
    #   ./provision.sh kit [каталог] [каталог с dev1c-setup]
    #
    # Что нужно до запуска: докер, дистрибутив 1С в $PLATFORM_DIR и модель
    # (в model/ комплекта или в MODEL_FILE). Бинарники установщика и
    # коннектора команда берёт из CI сама; RUN_ID задаёт конкретный прогон,
    # DEV1C_FETCH=1 обновляет уже лежащие.
    #
    # Порядок важен: pack.sh стирает каталог комплекта (кроме model) — так и
    # надо, иначе в нём копятся следы прошлых сборок. Поэтому всё готовится
    # во временном каталоге, а раскладывает pack.sh сам, последним шагом
    # считая суммы — по всему, что в комплекте, включая модель.
    out=${2:-dist/dev1c-kit-linux}
    bins=${3:-bins}
    llama=${LLAMA_REF:-ghcr.io/metalmon/dev1c-llama:b11734}
    tmp=${TMPDIR:-/tmp}/dev1c-kit
    mkdir -p "$out" "$tmp" "$bins" artifacts

    # Бинарники берём из CI сами: иначе «воспроизводимо» выходит с
    # оговоркой — два файла надо принести руками, и в новой сессии никто не
    # знает откуда. Лежащие файлы не трогаем, чтобы можно было собрать
    # комплект с бинарником из конкретного прогона или собранным локально;
    # обновить принудительно — DEV1C_FETCH=1.
    fetch() { # $1 — имя артефакта, $2 — куда, $3 — файл внутри
        [ -n "${DEV1C_FETCH:-}" ] || [ ! -f "$2/$3" ] || return 0
        command -v gh >/dev/null 2>&1 \
            || die "нет $2/$3, и gh недоступен — положите артефакт $1 из CI вручную"
        [ -n "${RUN_ID:-}" ] || RUN_ID=$(gh run list --branch main --status success \
            --limit 1 --json databaseId --jq '.[0].databaseId')
        [ -n "$RUN_ID" ] || die "не нашёл успешного прогона main — положите артефакт $1 вручную"
        say "беру $1 из прогона $RUN_ID"
        gh run download "$RUN_ID" -n "$1" -D "$2" >/dev/null \
            || die "не скачался артефакт $1 из прогона $RUN_ID"
    }
    fetch dev1c-setup-linux "$bins" dev1c-setup
    # Машине сборки нужен бинарник, который на ней запускается: pack.sh
    # печатает им образец dev1c.toml и подписывает комплект. В комплект он не
    # попадает — pack.sh --os linux отказывается кладть туда .exe.
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*) fetch dev1c-setup-windows "$bins" dev1c-setup.exe ;;
    esac
    fetch 1c-connect-linux artifacts 1c-connect
    # Артефакт кладёт файл под своим именем; образу он нужен как
    # 1c-connect-linux (см. Dockerfile.1c).
    [ ! -f artifacts/1c-connect ] || mv artifacts/1c-connect artifacts/1c-connect-linux
    chmod +x "$bins/dev1c-setup" 2>/dev/null || true
    [ -f "$bins/dev1c-setup" ] || die "нет $bins/dev1c-setup"

    "$0" build

    # DEV1C_REUSE=1 — не делать заново то, что уже лежит в $tmp. Нужно, когда
    # чинишь поздний шаг: архивы по четыре гигабайта пересобирать незачем. По
    # умолчанию выключено — образы могли с тех пор пересобраться, а комплект
    # с архивом от прошлых образов об этом не скажет.
    have() { [ -n "${DEV1C_REUSE:-}" ] && [ -s "$1" ] && say "беру готовый $1"; }

    say "архив среды и стенда"
    have "$tmp/dev1c-image.tar.gz" || {
        docker save "$BASE_IMAGE" "$IMAGE" | gzip -1 > "$tmp/dev1c-image.tar.gz"
        sh scripts/check-image-archive.sh "$tmp/dev1c-image.tar.gz"
    }

    say "архив сервера модели"
    # В реестре он лежит индексом с манифестом подписей, и docker save
    # отдаёт по нему оглавление вместо слоёв, возвращая при этом ноль.
    # Пересобираем одной строкой FROM — получается одноплатформенный образ,
    # который сохраняется целиком.
    # Dockerfile подаётся на вход, а не файлом в каталоге: каталог пришлось бы
    # отдать докеру аргументом, а MSYS_NO_PATHCONV=1 выше (он нужен для путей
    # внутри контейнера) оставляет его unix-путём, которого докер на windows
    # не находит.
    have "$tmp/dev1c-llama-image.tar.gz" || {
        docker pull "$llama"
        printf 'FROM %s\n' "$llama" | docker build --platform linux/amd64 -t "$LLAMA_IMAGE" -
        docker save "$LLAMA_IMAGE" | gzip -1 > "$tmp/dev1c-llama-image.tar.gz"
        sh scripts/check-image-archive.sh "$tmp/dev1c-llama-image.tar.gz"
    }

    have "$tmp/dev1c-1c-payload.tar.gz" || "$0" export "$tmp/dev1c-1c-payload.tar.gz"

    say "пакеты NVIDIA Container Toolkit"
    ver=$(sed -n 's/^NVIDIA_TOOLKIT_VERSION=//p' pack.sh)
    mkdir -p "$tmp/nvidia"
    for pkg in nvidia-container-toolkit nvidia-container-toolkit-base libnvidia-container1 libnvidia-container-tools; do
        f="${pkg}_${ver}_amd64.deb"
        # Пишем перенаправлением, а не curl -o: curl здесь windows-овский, а
        # MSYS_NO_PATHCONV=1 выше оставляет путь unix-ом, и писать ему
        # оказывается некуда. Через .part, чтобы оборванная закачка не сошла
        # в следующий раз за готовый пакет.
        [ -f "$tmp/nvidia/$f" ] || { curl -fsSL \
            "https://nvidia.github.io/libnvidia-container/stable/deb/amd64/$f" \
            > "$tmp/nvidia/$f.part" && mv "$tmp/nvidia/$f.part" "$tmp/nvidia/$f"; }
    done

    # Модель лежит в комплекте и остаётся там: раскладка каталог model не
    # трогает (pack.sh), так что переносить двадцать гигабайт никуда не надо
    # — и суммы по ней посчитаются вместе со всем остальным.
    if [ -n "${MODEL_FILE:-}" ]; then
        say "модель"
        mkdir -p "$out/model"
        cp "$MODEL_FILE" "$out/model/"
    fi
    model=$(ls "$out"/model/*.gguf 2>/dev/null | head -1 || true)
    [ -n "$model" ] || die "нет модели: положите .gguf в $out/model или задайте MODEL_FILE"

    say "раскладка"
    NVIDIA_TOOLKIT_DIR="$tmp/nvidia" LLAMA_IMAGE="$tmp/dev1c-llama-image.tar.gz" \
        PAYLOAD_ARCHIVE="$tmp/dev1c-1c-payload.tar.gz" \
        OUT_DIR=$(dirname "$out") ./pack.sh --os linux "$tmp/dev1c-image.tar.gz" "$bins"

    # Путь к модели в параметрах: образец печатает его пустым, и заказчик на
    # пустом поле упирается в «модель не выбрана» при живой модели рядом.
    # Правится после раскладки: dev1c.toml в суммы не входит — его правят по
    # назначению (integrity::listed).
    sed -i "s|^file = \"\"|file = \"model/$(basename "$model")\"|" "$out/dev1c.toml"

    say "готово: $out ($(du -sh "$out" | cut -f1))"
    ;;

status)
    for v in "$VOL_PLATFORM" "$VOL_DEMO" "$VOL_HOME" "$VOL_GITEA"; do
        docker volume inspect "$v" >/dev/null 2>&1 \
            && printf '%-16s present\n' "$v" || printf '%-16s MISSING\n' "$v"
    done
    # shellcheck disable=SC2086
    docker run --rm $MOUNTS "$IMAGE" sh -c \
        'cat /opt/1c-demo/payload.conf 2>/dev/null || echo "payload: the volumes are empty"'
    ;;
*)
    echo "usage: ./provision.sh build | export [file] | import [file] | kit [dir] [bins] | run [docker args…] | status" >&2
    exit 2
    ;;
esac
