#!/bin/sh
# Container entrypoint.
#
# Starts the shared services and remote access. Shared means one instance for
# every user of the image: the model takes the whole GPU memory, and the git
# server has to be single so that repositories are common.
#
# Per-user things (Zed, analyzer cache, projects) live in home directories and
# start within each session.
set -eu

log() { echo "[entrypoint] $*"; }

# --- Environment for sessions -------------------------------------------------
# Variables passed with -e exist only in this process: an RDP session is created
# by PAM, which builds the environment anew from /etc/environment. Without this
# the model key is visible in `docker exec` but not to Zed — exactly the case of
# "everything is passed, nothing works".
#
# То же относится к ENV образа, а не только к -e: DEMO_TITLE и DEMO_DB_USER
# стоят в Dockerfile.1c, но в сессию RDP не попадали — и `1c-stand`, запущенный
# из сессии (а это каждый запуск 1С с рабочего стола), откатывался на свои
# умолчания. Имя базы в списке стартера переписывалось обратно на
# «Демонстрационное приложение», хотя стенд давно вернулся на УНФ.
#
# Значение с пробелами можно писать без кавычек: pam_env берёт строку до
# конца. Проверено в образе — «Управление нашей фирмой (учебная)» доходит до
# сессии целиком и в кавычках, и без них.
for var in MODEL_PROVIDER MODEL_NAME CUSTOM_MODEL \
           LLAMA_API_URL LLAMACPP_API_KEY \
           CUSTOM_API_KEY CUSTOM_API_URL \
           ANTHROPIC_API_KEY OPENAI_API_KEY \
           HTTP_PROXY HTTPS_PROXY NO_PROXY            DEMO_TITLE DEMO_DB_USER
do
    eval "value=\${$var:-}"
    # Строка стирается ВСЕГДА, а пишется только если значение есть. Раньше
    # стирали лишь перед записью, и переменная, исчезнувшая из конфигурации
    # запуска, оставалась в /etc/environment навсегда — то есть во всех
    # сессиях RDP. Живой случай: стенд переставили с локальной модели на
    # удалённую, а мёртвый LLAMA_API_URL остался, и его читали и редактор, и
    # pi. Находка ревью 26.10.2026.
    sed -i "/^$var=/d" /etc/environment 2>/dev/null || true
    [ -n "$value" ] || continue
    echo "$var=$value" >> /etc/environment
done

# --- Home directories ---------------------------------------------------------
# /home is a volume, so it hides whatever the image put there — and an account
# added in a later image has no home at all on an existing volume. Nothing
# complains: the user simply has no files, and sshd quietly refuses their key
# because the directory it does find belongs to root.
for u in dev1 dev2 dev3; do
    id "$u" >/dev/null 2>&1 || continue
    home=$(getent passwd "$u" | cut -d: -f6)
    [ -n "$home" ] || continue
    if [ ! -d "$home" ]; then
        mkdir -p "$home"
        cp -a /etc/skel/. "$home/" 2>/dev/null || true
        log "created missing home for $u"
    fi
    # The project directory exists even before anything puts a project in it.
    # Two things want it: vrunner works there and the dump lands there, and
    # bsl-analyzer's `workspace` profile refuses to start at all when its root
    # is missing — it exits 1 with `Os { code: 2, kind: NotFound }` and takes
    # `syntax_help` down with it, so the agent loses the platform reference
    # over a missing directory. An existing empty one is fine: the server
    # starts and answers «indexing, try again» for what is not ready yet.
    # In local mode stand.sh makes it; in tunnel mode nobody did.
    # Всё, что не настоящий каталог, — отказ, как и с .git-credentials
    # ниже: мы здесь root в чужом доме. Символическая ссылка отдала бы
    # пользователю то, на что указывает; жёсткая ссылка на чужой файл
    # (`ln /etc/shadow ~/project`) — сам файл. Не удаляем: дом его, и молча
    # подменять его вещи хуже, чем не сделать шаг и сказать об этом.
    # `chown -h` закрывает подмену в промежутке между проверкой и сменой
    # владельца: по ссылке он не идёт.
    if [ -L "$home/project" ] || { [ -e "$home/project" ] && [ ! -d "$home/project" ]; }; then
        log "$u: ~/project не каталог (ссылка или файл) — не создаю и владельца не меняю"
    else
        mkdir -p "$home/project"
        # Владелец каталога — каждый раз, и отдельно от проверки ниже. Она
        # смотрит на САМ дом и пропускает починку, когда дом уже принадлежит
        # пользователю (а он принадлежит: дома лежат в томе и переживают
        # пересборку образа). Каталог же создаётся здесь, от root, и
        # остаётся root-овским — коннектор работает от пользователя и
        # получает «не создать /home/devN/project: Отказано в доступе».
        # Содержимое не трогаем: chown на сам каталог идемпотентен и чинит
        # уже сломанные дома.
        chown -h "$u:$u" "$home/project"
    fi
    # Ownership is checked every time, not only on creation: root-owned files in
    # a user's home are exactly what breaks key-based login.
    [ "$(stat -c '%U' "$home")" = "$u" ] || chown -R "$u:$u" "$home"
    # git without a password prompt and with a real identity: the credential
    # helper is `store` in /etc/gitconfig, this is the file it reads. Written
    # only when absent — a person may have changed the password or the name.
    # Symlinks are refused: this runs as root inside the user's home.
    cred="$home/.git-credentials"
    if [ ! -e "$cred" ] && [ ! -L "$cred" ]; then
        printf 'http://%s:%s@localhost:3000\n' "$u" "$u" >"$cred"
        chown "$u:$u" "$cred" && chmod 600 "$cred"
    fi
    if [ ! -e "$home/.gitconfig" ] && [ ! -L "$home/.gitconfig" ]; then
        printf '[user]\n\tname = %s\n\temail = %s@dev1c.local\n' "$u" "$u" >"$home/.gitconfig"
        chown "$u:$u" "$home/.gitconfig"
    fi
done

# --- Editor settings from the image -------------------------------------------
# Home directories live on a volume and survive image updates — which also means
# a new image's settings never reach anyone who already has a home. The symptom
# is nasty: the image carries a fix, the environment keeps the old behaviour, and
# nothing anywhere says why.
#
# So our files are refreshed when they change in the image, tracked by a stamp
# of the shipped version in .config/.from-image/. A hand-edited file is
# replaced too, but kept as .bak next to it: these are our settings, not the
# user's, and the people this environment is for do not edit JSON — while we
# change it constantly. The same goes for the desktop: theme, fonts, wallpaper
# and window manager live in xfconf files in the home, and a theme change in
# the image would otherwise reach nobody with an existing home.
# This runs as root and writes into directories owned by the user. A symlink put
# in place of any of these paths would have root write wherever it points. Today
# the workstation accounts have passwordless sudo, so that grants nothing they
# could not do directly — but sudo is exactly the kind of thing dropped when a
# pilot turns into an installation, and then this would become a real hole. So
# the paths are refused when symlinked, a parent that resolves outside .config
# is refused, and the writes do not follow links.
# Панель в списке с 25.10.2026. Её тут не было, и правка панели в образе не
# доходила ни до кого с уже созданным домом — а дома живут на томе и
# переживают любое обновление. Тот же случай, что с темой и обоями, ради
# которого список и появился: размер надписи раскладки поправлен в образе, а
# на стенде остался бы прежним.
SHIPPED_FILES="zed/settings.json zed/AGENTS.md zed/themes/1c.json
    xfce4/xfconf/xfce-perchannel-xml/xsettings.xml
    xfce4/xfconf/xfce-perchannel-xml/xfwm4.xml
    xfce4/xfconf/xfce-perchannel-xml/xfce4-desktop.xml
    xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml
    gtk-3.0/settings.ini gtk-4.0/settings.ini"
# Файлы pi (25.10.2026) лежат не под .config, а в ~/.pi/agent — у pi свой
# каталог настроек, и переносить его мы не можем. Значит второй корень и второй
# список, но раскладка одна и та же: отпечаток, .bak, отказ по символическим
# ссылкам. Отдельная реализация тут была бы второй дырой в безопасности,
# которую надо закрывать дважды.
SHIPPED_PI_FILES="agent/AGENTS.md agent/mcp.json agent/settings.json
    agent/extensions/llama-single.js"

# Отпечаток каталога: имена и содержимое, без дат и прав. Нужен скиллам —
# они приезжают деревом, а не файлом, и «изменилось ли» по дате не определить:
# `cp -a` переносит время, а правка агента его меняет.
dir_sum() {
    find "$1" -type f -exec md5sum {} + 2>/dev/null         | sed "s| $1/| |" | sort | md5sum | cut -d" " -f1
}

# Один файл из образа в дом. $1 — корень ($HOME-относительный: .config или
# .pi), $2 — путь внутри корня, он же путь внутри /etc/skel/$1. Остальное
# (home, stamps, owner_*) берётся из объемлющего цикла.
#
# Внутри нельзя `continue`: это функция, и пропуск файла — это `return`.
seed_from_image() {
    base=$1
    f=$2
    root="$home/$base"
    src="/etc/skel/$base/$f"
    dst="$root/$f"
    # Имена отпечатков у .config прежние — иначе при первом запуске нового
    # образа перераскладываются все файлы у всех. Файлы pi получают приставку.
    case "$base" in
        .config) stamp="$stamps/$(echo "$f" | tr / _)" ;;
        *)       stamp="$stamps/${base#.}_$(echo "$f" | tr / _)" ;;
    esac
    [ -f "$src" ] || return 0
    parent=$(dirname "$dst")
    if [ -L "$dst" ] || [ -L "$dst.bak" ] || [ -L "$stamp" ] || [ -L "$parent" ]; then
        log "skipping $dst: symlink"
        return 0
    fi
    # На месте файла — каталог, на месте каталога — файл. Оба достижимы
    # простыми командами в своём доме (`mkdir ~/.pi/agent/mcp.json`,
    # `touch ~/.config/zed/themes`), и оба валят точку входа: `rm -f` по
    # каталогу и `install -d` по файлу отказывают, а с `set -eu` отказ значит
    # выход — контейнер не доходит ни до Gitea, ни до xrdp, ни до sshd, то
    # есть ОДИН пользователь не поднимает стенд НИКОМУ. Срабатывало только
    # когда отпечаток разошёлся, то есть ровно при обновлении образа.
    if [ -e "$dst" ] && [ ! -f "$dst" ]; then
        log "skipping $dst: не обычный файл"
        return 0
    fi
    if [ -e "$parent" ] && [ ! -d "$parent" ]; then
        log "skipping $parent: не каталог"
        return 0
    fi
    # the parent is created below if missing; an existing one must resolve
    # inside the root (a link deeper in the path would lead root elsewhere)
    root_real=$(realpath "$root")
    if [ -e "$parent" ]; then
        case "$(realpath "$parent")" in "$root_real"|"$root_real"/*) ;; *)
            log "skipping $dst: parent resolves outside $root"; return 0 ;; esac
    fi
    shipped=$(md5sum "$src" | cut -d' ' -f1)
    # The settings carry the model address and window substituted below;
    # a substituted placeholder is gone for good, so when the model
    # variables change the file has to come from the image again. The
    # stamp therefore covers the image copy AND the model variables: the
    # same image with a new model re-seeds settings.json (a hand edit in it
    # is lost then — the .bak keeps it), the other files are untouched by this.
    # У pi по тем же подстановкам приезжают порты серверов MCP в mcp.json —
    # но они зависят только от имени пользователя, а не от переменных, так что
    # соли им не нужно.
    if [ "$f" = zed/settings.json ]; then
        shipped=$(printf '%s|%s|%s|%s|%s' "$shipped" "${MODEL_PROVIDER:-}" "${MODEL_NAME:-}" \
            "${LLAMA_API_URL:-}" "${CUSTOM_API_URL:-}" | md5sum | cut -d' ' -f1)
    fi
    [ "$(cat "$stamp" 2>/dev/null)" = "$shipped" ] && return 0
    # Владелец у .bak выставляется явно: `cp` от root делает файл root-овским,
    # и сохранённая правка пользователя оказывалась в его доме чужой — удалить
    # или открыть на запись он её не мог. Поймано проверкой test-pi-seed
    # 25.10.2026; дефект был и у настроек Zed, а не только у файлов pi.
    #
    # `-P` и `-h` — против подмены между проверкой и использованием: выше
    # доказано, что ни $dst, ни $dst.bak не ссылки, но дом принадлежит
    # пользователю, и между той проверкой и этой строкой он может подставить
    # ссылку. С `-P` копируется сама ссылка, а не то, куда она указывает; с
    # `-h` владелец меняется у ссылки, а не у цели. На обычном файле — ровно
    # то же поведение, что без флагов, так что цена нулевая.
    if [ -f "$dst" ]; then
        cp -P --remove-destination "$dst" "$dst.bak"
        chown -h "$owner_u:$owner_g" "$dst.bak"
    fi
    # install after rm: writing over a path replaces the file itself rather
    # than following whatever it might point at
    rm -f "$dst"
    # the directories are the user's (their own themes go to zed/themes too)
    [ -d "$parent" ] || install -d -o "$owner_u" -g "$owner_g" "$parent"
    # Порты туннеля подставляются ДО установки, в своём временном файле, а не
    # `sed -i` по готовому файлу в доме. Разница в том, кто и куда пишет: sed
    # от root идёт по пути, который принадлежит пользователю, и между проверкой
    # и записью любой промежуточный каталог можно подменить ссылкой. Здесь root
    # пишет только в каталог отпечатков (он проверен выше) и ставит готовый
    # файл на место. Замечание разбора безопасности 25.10.2026.
    # Подстановка идёт ПОТОКОМ прямо в install, без временного файла. Временный
    # файл пришлось бы класть в каталог, который принадлежит пользователю, а он
    # может подменить его ссылкой между созданием и записью — и root напишет
    # туда, куда ссылка ведёт. Замечание разбора безопасности 25.10.2026.
    sub=
    if [ "$base/$f" = ".pi/agent/mcp.json" ]; then
        # Номера те же, что у Zed: dev1 -> 7771/7781, dev2 -> 7772/7782,
        # dev3 -> 7773/7783. Любой другой дом (учётка туннеля, будущий
        # пользователь) остаётся с подстановкой: nc со словом вместо порта
        # падает громко, а не занимает чужой туннель.
        u=${home#/home/}
        case "$u" in
            dev[1-9])
                n=${u#dev}
                sub="s|WORKSTATION_1C_PORT_PLACEHOLDER|$((7770 + n))|;s|WORKSTATION_VRUNNER_PORT_PLACEHOLDER|$((7780 + n))|"
                ;;
        esac
    fi
    if [ -n "$sub" ]; then
        sed -e "$sub" "$src" | install -o "$owner_u" -g "$owner_g" -m 0644 /dev/stdin "$dst"
    else
        install -o "$owner_u" -g "$owner_g" -m 0644 "$src" "$dst"
    fi
    tmp=$(mktemp "$stamps/.tmp.XXXXXX") || return 0
    echo "$shipped" > "$tmp"
    chown "$owner_u:$owner_g" "$tmp"
    mv -f "$tmp" "$stamp"
    log "$base/$f updated from the image in $home"
}

for home in /home/*; do
    cfg="$home/.config"
    [ -d "$cfg/zed" ] || continue
    if [ -L "$home" ] || [ -L "$cfg" ]; then
        log "skipping $home: a symlinked path on the way to the settings"
        continue
    fi
    owner_u=$(stat -c '%U' "$home")
    owner_g=$(stat -c '%G' "$home")
    stamps="$cfg/.from-image"
    [ -L "$stamps" ] && { log "skipping $home: $stamps is a symlink"; continue; }
    [ -d "$stamps" ] || install -d -o "$owner_u" -g "$owner_g" "$stamps"
    for f in $SHIPPED_FILES; do
        seed_from_image .config "$f"
    done
    # Файлы pi. Отпечатки для них лежат в том же .config/.from-image: это
    # единственный каталог отпечатков дома, и он уже проверен выше на ссылку.
    # Сам ~/.pi у дома прежнего образа не существует — создаём и отдаём
    # пользователю, иначе pi получит каталог, принадлежащий root.
    # Ссылка — отказ по тем же причинам, что у .config. Обычный файл на этом
    # месте — тоже отказ, и это не теория: `rm -rf ~/.pi && touch ~/.pi`
    # пользователю доступен, а точка входа работает с `set -eu` — падение здесь
    # означает, что контейнер не доходит ни до Gitea, ни до xrdp, ни до sshd,
    # то есть ОДИН пользователь не поднимает стенд НИКОМУ.
    if [ -L "$home/.pi" ]; then
        log "skipping $home/.pi: symlink"
    elif [ -e "$home/.pi" ] && [ ! -d "$home/.pi" ]; then
        log "skipping $home/.pi: not a directory"
    elif [ ! -d "$home/.pi" ] && ! install -d -o "$owner_u" -g "$owner_g" "$home/.pi"; then
        log "skipping $home/.pi: cannot create"
    else
        for f in $SHIPPED_PI_FILES; do
            seed_from_image .pi "$f"
        done
    fi

    # --- Скиллы ---------------------------------------------------------
    # Скиллы — это и есть знание агента, и до сих пор они доезжали только в
    # НОВЫЙ дом: дома живут на томе, и стенд, обновлённый на свежий образ,
    # навсегда оставался со старым набором. Обе проверки при этом зелёные —
    # test-skills смотрит /etc/skel при сборке, test-pi сверяет скиллы из
    # ДОМА с запросом к модели.
    #
    # Затирать нельзя: правила агента прямо говорят, что ~/.agents/skills его
    # — «правь готовые и заводи новые». Поэтому правило такое же, как у
    # настроек, только без .bak (копия дерева бессмысленна):
    #   нет в доме        → кладём;
    #   есть и НЕ тронут  → заменяем новым из образа;
    #   есть и правлен    → оставляем как есть и говорим об этом в журнале.
    # «Не тронут» значит «совпадает с тем, что образ давал в прошлый раз»:
    # отпечаток берётся с содержимого, а не с даты.
    skel_skills=/etc/skel/.agents/skills
    skills="$home/.agents/skills"
    if [ -d "$skel_skills" ] && [ ! -L "$home/.agents" ] && [ ! -L "$skills" ]; then
        [ -d "$skills" ] || install -d -o "$owner_u" -g "$owner_g" "$skills"
        for src in "$skel_skills"/*; do
            [ -d "$src" ] || continue
            name=$(basename "$src")
            dst="$skills/$name"
            stamp="$stamps/skill_$name"
            now=$(dir_sum "$src")
            if [ ! -e "$dst" ]; then
                cp -a "$src" "$dst" && chown -R "$owner_u:$owner_g" "$dst"
                echo "$now" > "$stamp" && chown "$owner_u:$owner_g" "$stamp"
                log "skill $name: новый, положен в $home"
            elif [ "$now" = "$(cat "$stamp" 2>/dev/null)" ]; then
                : # образ не менялся — делать нечего
            elif [ "$(dir_sum "$dst")" = "$(cat "$stamp" 2>/dev/null)" ]; then
                rm -rf "$dst"
                cp -a "$src" "$dst" && chown -R "$owner_u:$owner_g" "$dst"
                echo "$now" > "$stamp" && chown "$owner_u:$owner_g" "$stamp"
                log "skill $name: обновлён из образа в $home"
            else
                log "skill $name: правлен на месте, из образа не обновляю ($home)"
            fi
        done
    fi
done

# --- Model server in Zed settings ---------------------------------------------
# Nobody edits JSON to point the environment at a model. The settings ship with
# placeholders; everything concrete is filled in here from the environment:
# The settings carry two providers and MODEL_PROVIDER says which one is live:
# "llama.cpp" is our own server (LLAMA_API_URL, LLAMACPP_API_KEY) and takes its
# context window from the server's own /props — nothing of ours is declared for
# it; "custom" is any other OpenAI-compatible server (CUSTOM_API_URL,
# CUSTOM_API_KEY), which has no /props, so the window stays declared there.
# MODEL_NAME names the model for both. No address at all means no model: the
# editor works, the agent stays silent, and dev1c-setup check says so.
#
# Only placeholders are replaced. If someone edited the settings by hand, their
# change stays: there is no placeholder left to overwrite.
#
# /etc/skel is deliberately NOT in this list. It is the reference copy the block
# above hands out, and substituting into it burns the placeholders on first run:
# after that the environment can never be pointed at a different provider,
# because there is nothing left to replace. Homes get the substitution; the
# reference stays pristine.
for cfg in /home/*/.config/zed/settings.json; do
    [ -f "$cfg" ] || continue
    # Same reasoning as above, and not theoretical: `sed -i` on a symlinked path
    # reads whatever it points at and leaves the result in its place. Pointed at
    # /etc/shadow, it put the shadow file's contents into a user's directory —
    # the file itself survived, but the mechanism works, and with different
    # permissions that is a leak.
    if [ -L "$cfg" ]; then
        log "skipping $cfg: symlink"
        continue
    fi
    # Промежуточные каталоги — тоже. Подменив ~/.config/zed ссылкой на дом
    # соседа, рабочее место заставило бы root переписать ЧУЖИЕ настройки:
    # `sed -i` идёт по ссылке и кладёт результат там, куда она ведёт. Наружу
    # при этом ничего не утекает, но испортить соседа — достаточная причина.
    # Замечание соседней сессии 25.10.2026.
    #
    # Подстановку не переносим в seed_from_image, как сделано для mcp.json:
    # этот цикл чинит и те дома, где settings.json разложен прежней точкой
    # входа и до сих пор с подстановками, — отпечаток у них не менялся, и
    # раскладка их не тронет.
    h=${cfg%/.config/zed/settings.json}
    # Сами звенья — ссылками быть не должны, как и в цикле раскладки: realpath
    # ниже разрешил бы ссылку и сверил уже разрешённое с разрешённым, то есть
    # пропустил бы её.
    if [ -L "$h/.config" ] || [ -L "$h/.config/zed" ]; then
        log "skipping $cfg: .config или .config/zed — ссылка"
        continue
    fi
    case "$(realpath "${cfg%/settings.json}" 2>/dev/null)" in
        "$(realpath "$h/.config" 2>/dev/null)"/zed) ;;
        *) log "skipping $cfg: путь ведёт за пределы .config этого дома"; continue ;;
    esac

    # Workstation ports for the tunnel from the machine with 1C: dev1 -> 7771
    # (mcp-1c) and 7781 (vrunner-mcp), dev2 -> 7772/7782, dev3 -> 7773/7783.
    # Fixed by convention so that neither side needs a parameter; 1c-connect
    # derives the same numbers from the workstation name. Any other home (the
    # tunnel account, a future user) keeps the placeholder: nc with a word for
    # a port fails loudly instead of binding somebody else's tunnel.
    u=${cfg#/home/}; u=${u%%/*}
    case "$u" in
        dev[1-9])
            n=${u#dev}
            sed -i \
                -e "s|WORKSTATION_1C_PORT_PLACEHOLDER|$((7770 + n))|" \
                -e "s|WORKSTATION_VRUNNER_PORT_PLACEHOLDER|$((7780 + n))|" \
                "$cfg"
            ;;
    esac

    # The window numbers: measured by dev1c-setup for a local model server,
    # or the defaults for a remote one. Always substituted, so the file is
    # valid JSON whatever the provider.
    sed -i \
        -e "s|CUSTOM_MAX_TOKENS_PLACEHOLDER|${CUSTOM_MAX_TOKENS:-53248}|" \
        `# 32768, а не 8192: модель размышляет, и размышления сидят в выходном` \
        `# бюджете. Авторы PRISM поднимали свой предел 16384 -> 32768 -> 65536,` \
        `# потому что на 32768 многословные размышляющие модели отдавали ПУСТОЙ` \
        `# ответ даже на простых задачах. Это предел для провайдера custom,` \
        `# то есть для стендов с чужой моделью: у штатного llama.cpp Zed берёт` \
        `# окно с /props и делит сам.` \
        -e "s|CUSTOM_MAX_OUTPUT_TOKENS_PLACEHOLDER|${CUSTOM_MAX_OUTPUT_TOKENS:-32768}|" \
        "$cfg"

    # Which of the two providers in the settings is the live one. dev1c-setup
    # says so with MODEL_PROVIDER; with nothing said, the one whose address
    # arrived wins, so a kit that predates this variable keeps working.
    provider="${MODEL_PROVIDER:-}"
    if [ -z "$provider" ] && [ -n "${LLAMA_API_URL:-}" ]; then
        provider="llama.cpp"
    fi
    if [ -z "$provider" ] && [ -n "${CUSTOM_API_URL:-}" ]; then
        provider="custom"
    fi
    # CUSTOM_MODEL — прежнее имя переменной: комплект и CI, собранные до
    # перехода на штатный провайдер, присылают его, и ломать их незачем.
    name="${MODEL_NAME:-${CUSTOM_MODEL:-model}}"

    # Both addresses are substituted whenever they are known: the file carries
    # both providers, and a placeholder left in the idle branch harms nobody —
    # it is never read — while a missing substitution in the live one shows up
    # as "the agent does not answer" with no reason given.
    # Адрес нашего сервера кладётся БЕЗ завершающего /v1: путь Zed дописывает
    # сам, и с /v1 выходит /v1/v1/models — на него llama.cpp отвечает 404, а в
    # панели агента это выглядит как «моделей нет», без единой строки о
    # причине. dev1c-setup эту форму и кладёт (model_root), но переменную
    # можно задать и руками, поэтому срезаем здесь. Пойман 25.10.2026 ровно
    # так: вручную поднятый стенд, pi отвечает, панель молчит.
    #
    # CUSTOM_API_URL не трогаем: там провайдер openai_compatible, и у него
    # адрес как раз с /v1.
    if [ -n "${LLAMA_API_URL:-}" ]; then
        llama_url=${LLAMA_API_URL%/}
        llama_url=${llama_url%/v1}
        sed -i -e "s|LLAMA_API_URL_PLACEHOLDER|$llama_url|" "$cfg"
    fi
    if [ -n "${CUSTOM_API_URL:-}" ]; then
        sed -i -e "s|CUSTOM_API_URL_PLACEHOLDER|$CUSTOM_API_URL|" "$cfg"
    fi

    if [ -n "$provider" ]; then
        sed -i \
            -e "s|MODEL_PROVIDER_PLACEHOLDER|$provider|" \
            -e "s|MODEL_NAME_PLACEHOLDER|$name|g" \
            "$cfg"
        case "$provider" in
            llama.cpp) log "model: $name at ${LLAMA_API_URL:-?} (native provider) ($cfg)" ;;
            *)         log "model: $name at ${CUSTOM_API_URL:-?} (openai_compatible) ($cfg)" ;;
        esac
    else
        log "model: not configured — neither LLAMA_API_URL nor CUSTOM_API_URL is set ($cfg)"
    fi
done

# Крючок для проверок: раскладка файлов из образа — единственная часть точки
# входа, которую нужно прогонять по многу раз и на подготовленных домах
# (дом прежнего образа, правленный человеком файл, символическая ссылка на
# месте каталога). Службы для этого поднимать незачем. Тот же приём, что
# DEV1C_GIT_TTY в обёртке git: крючок только для теста, в работе переменной
# нет. Проверка — test-pi-seed.
if [ -n "${DEV1C_SEED_ONLY:-}" ]; then
    log "seed only: раскладка выполнена, службы не поднимаются"
    exit 0
fi

# --- Gitea --------------------------------------------------------------------
# A service rather than a session autostart: repositories must be reachable at
# all times, including when nobody is connected — for CI, for instance.
if [ "${GITEA_ENABLED:-1}" = "1" ]; then
    log "Gitea: starting"
    su gitea -s /bin/sh -c "GITEA_HOME=/var/lib/gitea /usr/local/bin/gitea-start" &
fi

# --- Remote access ------------------------------------------------------------
rm -f /var/run/xrdp/*.pid 2>/dev/null || true

# sshd serves the reverse tunnel from the machine that has 1C installed
/usr/local/bin/sshd-setup &

/usr/sbin/xrdp-sesman --nodaemon &

log "ready: RDP 3389, Gitea 3000, SSH 2022"
exec /usr/sbin/xrdp --nodaemon
