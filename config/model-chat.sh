#!/bin/sh
# Веб-чат с моделью стенда: открыть в браузере интерфейс llama.cpp.
#
# Адрес не зашит: его кладёт в сессию точка входа (LLAMA_API_URL для нашего
# сервера, CUSTOM_API_URL для чужого). Завершающий /v1 снимается — интерфейс
# живёт в корне, а /v1 это API.
#
# Ключа у нашего сервера нет (решение 26.10.2026) именно ради этого: послать
# ключ встроенный интерфейс llama.cpp не умеет, и с ключом каждый его запрос
# получал бы 401. У чужого сервера ключ может быть — тогда чат не откроется, и
# сказать об этом надо словами, а не пустым окном.
set -eu

url=${LLAMA_API_URL:-${CUSTOM_API_URL:-}}
url=${url%/}
url=${url%/v1}

# Сказать человеку, а не в журнал. Ярлык запускается без терминала: всё, что
# уходит в stderr, не видит НИКТО — заказчик нажал на значок и получил тишину
# (26.10.2026). Поэтому сначала диалог, и только если графики нет вовсе —
# stderr. xmessage стоит вторым: он из x11-utils, который в образе есть и без
# нас, и спасает, если zenity почему-то не поставился.
say() {
    if command -v zenity >/dev/null 2>&1 && zenity --info --no-wrap --title='Чат с моделью' --text="$1" 2>/dev/null; then
        return 0
    fi
    if [ -n "${DISPLAY:-}" ] && command -v xmessage >/dev/null 2>&1; then
        printf '%b\n' "$1" | xmessage -center -file - >/dev/null 2>&1 && return 0
    fi
    printf '%b\n' "$1" >&2
}

if [ -z "$url" ]; then
    say "Сервер модели не настроен: ни LLAMA_API_URL, ни CUSTOM_API_URL.\nБез него чат открывать нечему."
    exit 1
fi

# Проверяем до запуска браузера: пустое окно Firefox объясняет меньше, чем
# одна строка словами.
if ! curl -sf -o /dev/null --max-time 5 "$url/props"; then
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$url/props" 2>/dev/null || echo 000)
    case "$code" in
        401|403) say "Сервер модели просит ключ, а встроенный веб-чат llama.cpp\nпосылать его не умеет. Чат недоступен; агент в редакторе\nи pi работают как обычно." ;;
        000)     say "Сервер модели не отвечает ($url).\nПроверьте, что он поднят: dev1c-setup check." ;;
        *)       say "Сервер модели ответил $code на $url/props — чат не откроется." ;;
    esac
    exit 1
fi

exec /usr/local/bin/launch-maximized firefox-esr "$url/"
