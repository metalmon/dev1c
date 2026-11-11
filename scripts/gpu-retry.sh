#!/bin/sh
# Обёртка над `gpu.sh` с повтором: sshd на home-gpu периодически отказывает
# в соединении (`kex_exchange_identification: read: Software caused connection
# abort` — MaxStartups), и `gpu.sh sync` при этом ВОЗВРАЩАЕТ НОЛЬ: отказ
# случается в середине конвейера `tar | ssh`, а код выхода даёт последний.
#
# Из-за этого обычный `until gpu.sh sync; do …` ломается молча: он считает
# отказ успехом, и следующая сборка идёт по СТАРОЙ копии исходников. Ровно
# это 11.11 дважды дало «те же ошибки компиляции» на уже исправленном дереве.
#
# Признак отказа поэтому ищется в выводе, а не в коде выхода.
#
# Пользование — как у gpu.sh, первым словом подкоманда:
#   scripts/gpu-retry.sh sync
#   scripts/gpu-retry.sh run cargo test --workspace
set -e

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TRIES=${GPU_TRIES:-10}
PAUSE=${GPU_PAUSE:-12}

i=1
while [ "$i" -le "$TRIES" ]; do
  out=$("$ROOT/scripts/gpu.sh" "$@" 2>&1) && code=0 || code=$?
  # Отказ ssh узнаётся по тексту И по коду 255 (его даёт сам ssh, когда до
  # команды дело не дошло): у `sync` код теряется в конвейере, у `run`/`cargo`
  # он доходит, а текст бывает разным — поэтому признаков два.
  case "$out$code" in
    *kex_exchange_identification*|*"banner exchange"*|*"Broken pipe"*|*"Connection closed"*|*"Connection reset"*)
      i=$((i + 1))
      sleep "$PAUSE"
      continue
      ;;
  esac
  if [ "$code" = 255 ]; then
    i=$((i + 1))
    sleep "$PAUSE"
    continue
  fi
  printf '%s\n' "$out" | grep -v 'post-quantum\|store now, decrypt later\|openssh.com/pq.html\|^$' || true
  exit "$code"
done

echo "home-gpu не отвечает: $TRIES попыток подряд отказали на ssh" >&2
exit 1
