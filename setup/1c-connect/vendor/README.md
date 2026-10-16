# vendor

`vrunner-lib.sha256` — манифест дерева пакетов opm в `vrunner/lib` комплекта
разработчика: `opm install -d <пусто> vanessa-runner@3.0.2 add@6.8.0` в
`evilbeaver/onescript:2.2.0`, снят 2026-10-16 (996 файлов, 16 МБ).
hub.oscript.io отдаёт зависимости «какие сегодня», поэтому `pack.sh` сверяет
результат с этим списком файл за файлом и падает при расхождении: два
комплекта «3.0.2» не могут различаться молча.

Обновить (сознательно, с ревью диффа):

    mkdir out
    docker run --rm -v "$PWD/out:/out" --entrypoint /bin/sh evilbeaver/onescript:2.2.0 \
      -c 'opm install -d /out vanessa-runner@3.0.2 add@6.8.0 >/dev/null && chmod -R a+rX /out \
          && cd /out && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum' > vrunner-lib.sha256

Монтировать в `/out`, не в `/lib`: `/lib` — это libc контейнера, и `sh` в нём
не запустится. `-print0`/`-0` обязательны: в `add/features` есть имена с
пробелами, и без них `xargs` режет их на куски.
