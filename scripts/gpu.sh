#!/bin/sh
# Сборка и тесты на второй машине (home-gpu), а не на этой.
#
# Зачем. Здешняя машина — i7-4770K 2013 года, 4 ядра и 16 ГБ, и диск `E:`
# забит под ноль: сборка на ней падает «нет места» и держит правило «одна
# cargo за раз». На home-gpu — i5-12400F, 6 ядер и 48 ГБ, и это втрое
# быстрее. Решение владельца 9.11: собираем ТАМ.
#
# Git по ssh на той машине не поднят (служебные команды git не в PATH у
# неинтерактивного сеанса), поэтому исходники едут архивом — их 8 МБ, это
# пара секунд. История сборке не нужна.
#
# Пользование:
#   scripts/gpu.sh sync                     # отправить исходники как есть
#   scripts/gpu.sh run cargo test -p onec-connect gui::peer
#   scripts/gpu.sh cargo test --workspace   # то же, но с sync впереди
#   scripts/gpu.sh pull target/release/1c-connect.exe  # забрать файл обратно
#
# Каталог сборки живёт ТАМ и сюда не едет: 33 ГБ по сети каждый раз — это
# не перенос сборки, а её пересылка.
set -e

HOST=${GPU_HOST:-home-gpu}
DIR=${GPU_DIR:-C:/dev1c}
ROOT=$(cd "$(dirname "$0")/.." && pwd)

WIN=$(echo "$DIR" | tr '/' '\\')

sync() {
  cd "$ROOT"
  # Каталог заводится ОТДЕЛЬНЫМ вызовом: склеенный с распаковкой через `&`
  # он закрывает поток раньше, чем архив доедет, — tar отвечает «broken
  # pipe», и исходники молча остаются прежними.
  ssh "$HOST" "cmd /c \"if not exist $WIN mkdir $WIN\"" >/dev/null 2>&1
  # Едет не только `setup/`: тесты и сборка вшивают файлы из КОРНЯ репозитория
  # (`include_str!("../../../config/...")`, `pack.sh`, `Dockerfile.1c`), и без
  # них компиляция тестов падает «не удаётся найти указанный путь» — а
  # собственно библиотека при этом собирается, то есть беда видна только на
  # прогоне. Список добыт grep-ом по `include_str!`/`include_bytes!`, а не по
  # памяти; добавится новый — поправить здесь.
  #
  # НЕ едут: каталог сборки (33 ГБ), история, `dist` (595 МБ), `artifacts`,
  # `bins`, `nvidia-container-toolkit` — к компиляции они отношения не имеют.
  tar czf - --exclude='setup/target' --exclude='.git' \
      setup config rust-toolchain.toml pack.sh Dockerfile.1c \
    | ssh "$HOST" "cmd /c \"cd /d $WIN && tar xzf -\""
}

case "$1" in
  sync)
    sync
    echo "исходники на $HOST:$DIR"
    ;;
  run)
    shift
    ssh "$HOST" "cmd /c \"cd /d $(echo "$DIR" | tr '/' '\\')\\setup && $*\""
    ;;
  pull)
    # Забрать собранное: путь задаётся от `setup/`.
    scp "$HOST:$DIR/setup/$2" "$ROOT/setup/$2"
    echo "забрано в setup/$2"
    ;;
  cargo)
    sync
    shift
    ssh "$HOST" "cmd /c \"cd /d $(echo "$DIR" | tr '/' '\\')\\setup && cargo $*\""
    ;;
  *)
    echo "как пользоваться: $0 sync | run <команда> | cargo <аргументы> | pull <путь от setup/>" >&2
    exit 2
    ;;
esac
