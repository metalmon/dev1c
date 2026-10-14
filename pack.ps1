# Prepare a release for the customer.
#
# Builds the image, saves it to a file and puts next to it everything needed to
# start on a machine with no internet and no sources.
#
#   .\pack.ps1              build and pack
#   .\pack.ps1 -SkipBuild   pack an already built image
#
# The debug model key cannot end up in a release: it takes no part in a build at
# all. In an air-gapped network llama.cpp runs locally and wants no key.
#
# The installation guide produced below stays in Russian — it is read by the
# customer's administrators, and the environment itself is Russian.

param(
    [switch]$SkipBuild,
    [string]$Image = "oktaplus/dev1c:latest",
    [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not $SkipBuild) {
    Write-Host "Building the image..." -ForegroundColor Cyan
    # The model key takes no part in the build: it is passed as an environment
    # variable at run time. There used to be a --build-arg with an empty value
    # here to keep the key out of the layers — that possibility is simply gone
    # now, so every build is clean.
    #
    # Plain docker build rather than compose: a release must not depend on the
    # customer having compose, nor the build on their configuration.
    docker build -t $Image .
    if ($LASTEXITCODE -ne 0) { Write-Host "Build failed." -ForegroundColor Red; exit 1 }
}

$exists = docker image inspect $Image 2>$null
if (-not $exists) {
    Write-Host "Image '$Image' not found. Build it without -SkipBuild." -ForegroundColor Red
    exit 1
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$tar = Join-Path $OutDir "dev1c-image.tar"

Write-Host "Saving the image to a file..." -ForegroundColor Cyan
docker save -o $tar $Image

# gzip roughly halves the size and docker load reads it without unpacking
Write-Host "Compressing..." -ForegroundColor Cyan
$gz = "$tar.gz"
if (Test-Path $gz) { Remove-Item $gz }
$in = [System.IO.File]::OpenRead($tar)
$out = [System.IO.File]::Create($gz)
$gzip = New-Object System.IO.Compression.GzipStream($out, [System.IO.Compression.CompressionLevel]::Optimal)
$in.CopyTo($gzip)
$gzip.Close(); $out.Close(); $in.Close()
Remove-Item $tar

# What the image cannot be started without. Compose is deliberately not shipped:
# startup goes through plain docker run so the image travels to podman and
# anything else.
Copy-Item dev1c.ps1 $OutDir -Force
if (Test-Path dev1c.sh) { Copy-Item dev1c.sh $OutDir -Force }
if (Test-Path README.md) { Copy-Item README.md $OutDir -Force }
if (Test-Path .env.example) { Copy-Item .env.example $OutDir -Force }

$size = [math]::Round((Get-Item $gz).Length / 1GB, 2)

@"
# Развёртывание среды разработки 1С

На машине должен быть установлен Docker Desktop. Интернет не нужен.

## 1. Загрузить образ

    docker load -i dev1c-image.tar.gz

Занимает несколько минут.

## 2. Запустить среду

    .\dev1c.ps1

Скрипт напечатает адреса подключения.

## 3. Положить файл модели

Без модели ИИ-агент работать не будет:

    docker cp модель.gguf dev1c:/opt/models/
    docker restart dev1c

Файл с расширением .gguf вам передают отдельно — он весит больше самого образа.

## 4. Подключиться

Подключение обычным «Подключением к удалённому рабочему столу» Windows на
напечатанный адрес. Рабочих места три, пароль совпадает с логином:

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

    .\dev1c.ps1 -Status         что работает внутри
    .\dev1c.ps1 -Stop           остановить
    .\dev1c.ps1 -Remove         остановить и удалить данные (с подтверждением)

## Что внутри

Среда разработки 1С: редактор Zed с ИИ-агентом, анализатор BSL, локальный
git-сервер Gitea с учебными материалами, инструменты командной строки.
Платформа 1С в образ не входит.
"@ | Set-Content (Join-Path $OutDir "SETUP.md") -Encoding UTF8

Write-Host ""
Write-Host "Release ready: $OutDir" -ForegroundColor Green
Write-Host "  dev1c-image.tar.gz   $size GB"
Write-Host "  dev1c.ps1, dev1c.sh, .env.example, SETUP.md"
Write-Host ""
