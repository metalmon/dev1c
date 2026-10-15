#!/bin/sh
# Prepare a release for the customer.
#
# Builds the image, saves it to a file and puts next to it everything needed to
# start on a machine with no internet and no sources.
#
#   ./pack.sh              build and pack
#   ./pack.sh --skip-build pack an already built image
#
# The debug model key cannot end up in a release: it takes no part in a build at
# all. In an air-gapped network llama.cpp runs locally and wants no key.
#
# The installation guide produced below stays in Russian — it is read by the
# customer's administrators, and the environment itself is Russian.
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
cd "$DIR"

IMAGE="${IMAGE:-oktaplus/dev1c:latest}"
OUT_DIR="${OUT_DIR:-dist}"
SKIP_BUILD=0
[ "${1:-}" = "--skip-build" ] && SKIP_BUILD=1

ENGINE=""
for e in docker podman; do
    command -v "$e" >/dev/null 2>&1 && { ENGINE="$e"; break; }
done
[ -n "$ENGINE" ] || { echo "Neither docker nor podman found." >&2; exit 1; }

if [ "$SKIP_BUILD" = "0" ]; then
    echo "Building the image..."
    # The model key takes no part in the build: it is passed as an environment
    # variable at run time. There used to be a --build-arg with an empty value
    # here to keep the key out of the layers — that possibility is simply gone
    # now, so every build is clean.
    $ENGINE build -t "$IMAGE" .
fi

$ENGINE image inspect "$IMAGE" >/dev/null 2>&1 || {
    echo "Image '$IMAGE' not found. Build it without --skip-build." >&2
    exit 1
}

mkdir -p "$OUT_DIR"

echo "Saving the image to a file..."
# Through a pipe, so a multi-gigabyte uncompressed tar never hits the disk
$ENGINE save "$IMAGE" | gzip -1 > "$OUT_DIR/dev1c-image.tar.gz"

# What the image cannot be started without
cp dev1c.sh "$OUT_DIR/"
[ -f dev1c.ps1 ] && cp dev1c.ps1 "$OUT_DIR/"
[ -f README.md ] && cp README.md "$OUT_DIR/"
[ -f .env.example ] && cp .env.example "$OUT_DIR/"
chmod +x "$OUT_DIR/dev1c.sh"

# The host kit: what the machine with the 1C platform needs so the agent can
# read a base — the Windows mcp-1c (installs its extension through the
# configurator) and the extension itself for a manual install. Same release as
# the binary inside the image, read from the Dockerfile so the two cannot
# drift apart. In an air-gapped network nobody can download these later.
MCP_1C_VERSION=$(sed -n 's/^ARG MCP_1C_VERSION=//p' Dockerfile)
mkdir -p "$OUT_DIR/host"
for f in mcp-1c-windows-amd64.exe MCP_HTTPService.cfe; do
    curl -fsSL "https://github.com/feenlace/mcp-1c/releases/download/$MCP_1C_VERSION/$f" \
         -o "$OUT_DIR/host/$f"
done

cat > "$OUT_DIR/SETUP.md" <<'EOF'
# Развёртывание среды разработки 1С

На машине должен быть установлен Docker или Podman. Интернет не нужен.

## 1. Загрузить образ

    docker load -i dev1c-image.tar.gz

Занимает несколько минут.

## 2. Запустить среду

Linux:

    ./dev1c.sh

Windows:

    .\dev1c.ps1

Скрипт напечатает адреса подключения.

## 3. Положить файл модели

Без модели ИИ-агент работать не будет:

    docker cp модель.gguf dev1c:/opt/models/
    docker restart dev1c

Файл с расширением .gguf вам передают отдельно — он весит больше самого образа.

## 4. Подключиться

Подключение любым RDP-клиентом на напечатанный адрес. Рабочих места три,
пароль совпадает с логином:

    dev1 / dev1
    dev2 / dev2
    dev3 / dev3

Работать можно одновременно: у каждого свой рабочий стол и свои файлы,
git-сервер и сервер модели — общие.

Чтобы внутри среды были видны диски этой машины, отметьте их в настройках
подключения ДО входа: Параметры -> Локальные ресурсы -> Подробнее -> Диски.

Важно: проект открывайте из домашнего каталога, а не прямо с подключённого
диска. Проброшенные диски годятся для обмена файлами, но работа с них идёт через
опрос каталога каждые две секунды и загружает процессор целиком.

## Управление

Linux:

    ./dev1c.sh status           что работает внутри
    ./dev1c.sh stop             остановить
    ./dev1c.sh remove           остановить и удалить данные (с подтверждением)

Windows — то же через параметры: -Status, -Stop, -Remove

## 5. Подключить базу 1С (по желанию)

Агент умеет читать работающую базу: метаданные, структуру объектов и форм,
выполнять запросы, смотреть журнал регистрации. Для этого на машине с
платформой 1С, один раз:

1. Установить расширение в базу — из каталога `host`:

       host\mcp-1c-windows-amd64.exe --install "C:\путь\к\базе" --db-user Пользователь --db-password Пароль

   Программа сама вызывает конфигуратор. Если командная строка недоступна,
   файл `host\MCP_HTTPService.cfe` добавляется через Конфигуратор:
   Конфигурация -> Расширения конфигурации -> Добавить из файла, затем F7.

2. Опубликовать базу на веб-сервере (Конфигуратор -> Администрирование ->
   Публикация на веб-сервере), на вкладке «HTTP-сервисы» включить
   «Публиковать HTTP-сервисы расширений по умолчанию». Проверка в браузере:
   `http://localhost/<имя публикации>/hs/mcp-1c/version`.

3. В файле `.env` рядом со скриптом запуска указать адрес и пользователя базы
   (образец — `.env.example`, раздел «Case 3») и перезапустить среду.

4. Поднять туннель с машины, где опубликована база (адрес среды напечатал
   скрипт запуска, порт — четвёртый из блока):

       ssh -p 3393 -N -R 7777:localhost:80 dev@<адрес среды>

   Пароль `dev`. Пока это окно открыто, агент видит базу.

## Что внутри

Среда разработки 1С: редактор Zed с ИИ-агентом, анализатор BSL, локальный
git-сервер Gitea с учебными материалами, инструменты командной строки.
Платформа 1С в образ не входит.
EOF

SIZE=$(du -h "$OUT_DIR/dev1c-image.tar.gz" | cut -f1)

echo
echo "Release ready: $OUT_DIR"
echo "  dev1c-image.tar.gz   $SIZE"
echo "  dev1c.sh, dev1c.ps1, .env.example, SETUP.md"
echo "  host/                mcp-1c for the machine with the platform ($MCP_1C_VERSION)"
echo
