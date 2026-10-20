# =============================================================================
# Theme build stage. Materia is built from source (meson + npm + sassc); there is
# no reason to drag that toolchain into the runtime image — we take the result.
# =============================================================================
FROM debian:13-slim AS theme-builder

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      git ca-certificates meson ninja-build sassc npm librsvg2-bin bc resvg \
    && rm -rf /var/lib/apt/lists/*

# Materia, recoloured into the 1C palette with its own change_color.sh (the
# oomox preset path): the same tokens 1c-connect's window uses — sampled from
# 1C:Enterprise 8.3.27 Такси, see setup/1c-connect/theme/1c.json. Light is the
# default on the desktop (Такси is light); dark is the same roles on warm dark
# greys. The accent is the amber #E6B800 (the pressed-button shade of the
# window's #FACC1F) in both: Materia also paints active tab labels and links
# with it, and the bright yellow was too pale for text on white. The text on
# the accent is derived by contrast (on-primary) and comes out dark. Radius
# 4, compact density, like the window.
# --inkscape False: rsvg-convert renders the assets, inkscape is not here.
RUN git clone --depth=1 https://github.com/nana-4/materia-theme.git /tmp/materia \
    && cd /tmp/materia \
    `# the render script knows resvg by its old name; four hover/active handle` \
    `# ids are absent from assets.svg, and resvg fails on them where inkscape` \
    `# wrote an empty PNG - a tolerant wrapper keeps the build going` \
    && printf '#!/bin/sh\nresvg "$@" || exit 0\n' > /usr/local/bin/rendersvg && chmod +x /usr/local/bin/rendersvg \
    && printf '%s\n' 'BG=f5f5f5' 'FG=333333' 'MATERIA_VIEW=ffffff' 'MATERIA_SURFACE=ffffff' \
         'HDR_BG=f0f0f0' 'HDR_FG=333333' 'SEL_BG=e6b800' 'ROUNDNESS=4' 'MATERIA_STYLE_COMPACT=True' 'MATERIA_PANEL_OPACITY=1' > /tmp/1c-light \
    && printf '%s\n' 'BG=262626' 'FG=dcdcdc' 'MATERIA_VIEW=262626' 'MATERIA_SURFACE=2c2c2c' \
         'HDR_BG=3a3a3a' 'HDR_FG=dcdcdc' 'SEL_BG=e6b800' 'ROUNDNESS=4' 'MATERIA_STYLE_COMPACT=True' 'MATERIA_PANEL_OPACITY=1' > /tmp/1c-dark \
    && ./change_color.sh --inkscape False -t /usr/share/themes -o 1C-light /tmp/1c-light \
    && ./change_color.sh --inkscape False -t /usr/share/themes -o 1C-dark /tmp/1c-dark \
    `# Materia labels desktop icons white with a shadow, for dark wallpapers;` \
    `# on the light theme's wallpaper that is white on white. Appended last,` \
    `# so it wins over the theme's own rule.` \
    && printf '\nXfdesktopIconView.view { color: #333333; text-shadow: none; }\n' >> /usr/share/themes/1C-light/gtk-3.0/gtk.css \
    && test -f /usr/share/themes/1C-light/gtk-3.0/gtk.css && test -f /usr/share/themes/1C-dark/gtk-3.0/gtk.css \
    && test -d /usr/share/themes/1C-light/xfwm4

# xfwm4 in Debian is built WITHOUT SVG support (ldd shows no librsvg), while
# Materia ships title-bar buttons as SVG only. Result: a title bar with no
# minimize/maximize/close buttons — no way to close a window with the mouse.
# Convert them to PNG, which xfwm4 does read. The look is preserved.
RUN set -eu; \
    for d in /usr/share/themes/1C-*/xfwm4; do \
      [ -d "$d" ] || continue; \
      for f in "$d"/*.svg; do \
        [ -e "$f" ] || continue; \
        rsvg-convert "$f" -o "${f%.svg}.png"; \
      done; \
      echo "$d: $(ls "$d"/*.png 2>/dev/null | wc -l) png"; \
    done


# =============================================================================
# Runtime image
#
# Layer order is deliberate: packages first, then everything downloaded over the
# network, and only afterwards our own configs, scripts and skills. Those change
# on every iteration, and with the old order a one-line edit in a shell script
# invalidated the heavy download layers to deliver a changed comment.
# Rarely-changing layers go up, frequently-changing ones down.
# =============================================================================
FROM debian:13-slim

ARG DEBIAN_FRONTEND=noninteractive

# debian-slim strips /usr/share/locale on EVERY package install, so Russian
# translations are re-enabled BEFORE apt install — otherwise XFCE arrives
# without them. The file is named zz-* so dpkg reads it last and path-include
# wins.
RUN echo 'path-include /usr/share/locale/ru/*' > /etc/dpkg/dpkg.cfg.d/zz-ru-locale

# firefox-esr is the working browser: the 1C web client officially supports
# Firefox, WebKit browsers are not supported.
#
# WebKitGTK is deliberately absent. Probing the Tauri engine inside an xrdp
# session served its purpose, but epiphany-browser pulled in libwebkitgtk,
# javascriptcore and libflite — around 160 MB for a browser nobody uses.
RUN apt-get update && apt-get install -y --no-install-recommends \
      xfce4 xfce4-terminal dbus-x11 xfce4-xkb-plugin \
      xrdp xorgxrdp \
      mesa-vulkan-drivers libvulkan1 \
      libfontconfig1 libasound2t64 libxkbcommon-x11-0 libgl1 \
      locales tzdata fonts-dejavu-core fonts-liberation2 \
      ca-certificates curl openssh-client \
      `# openssh-server serves the reverse tunnel from the machine running 1C.` \
      `#   The direction is inverted on purpose: we host the server, inside the` \
      `#   container, and the host only connects as a client. In an air-gapped` \
      `#   network workstations are attested, and installing server services on` \
      `#   them needs security sign-off; an SSH client ships with Windows 10/11` \
      `#   since 1803, and running it is ordinary use of the system.` \
      openssh-server \
      x11-utils \
      firefox-esr firefox-esr-l10n-ru \
\
      arc-theme papirus-icon-theme \
      fonts-inter fonts-jetbrains-mono \
      `# librsvg2-common is the SVG LOADER for gdk-pixbuf. Without it GTK cannot` \
      `#   draw vector icons and falls back to low-resolution PNGs from hicolor:` \
      `#   large icons look blurry and SVG-only sets (Papirus) do not render at` \
      `#   all.` \
      `#   librsvg2-bin (the rsvg-convert utility) is not needed here: it served` \
      `#   the wallpaper generator, and the wallpaper is now copied as a file.` \
      librsvg2-common \
      gnome-keyring libsecret-1-0 \
      `# --- Command line ---` \
      `# git: without it Zed shows neither file status nor history, and a 1C` \
      `#   configuration dump lives precisely in git. git-lfs: ERP dumps are big.` \
      git git-lfs \
      `# Search: ripgrep is used by humans and by the agent alike; on an ERP dump` \
      `#   (hundreds of thousands of files) plain grep is unacceptably slow.` \
      ripgrep fd-find \
      `# XML is the main source format of 1C: validation and xpath over the dump` \
      libxml2-utils xmlstarlet \
      `# Archives: .cf and .cfe are zip underneath and get opened regularly` \
      unzip zip p7zip-full \
      `# Everyday tools` \
      less nano vim-tiny tree jq rsync htop mc bash-completion \
      `# Network diagnostics: without them it is unclear why SSH to the platform` \
      `#   does not go through` \
      iputils-ping dnsutils netcat-openbsd \
      sudo \
      `# fuse3 forwards Windows drives over RDP. The rdpdr channel in xrdp is on` \
      `#   and FuseMountName is configured, but without fuse there is nothing to` \
      `#   mount with: drives ticked in mstsc simply never appear in` \
      `#   ~/thinclient_drives.` \
      fuse3 \
      `# xdg-utils provides xdg-open: opening files and links from applications` \
      xdg-utils desktop-file-utils shared-mime-info \
      `# xdg-desktop-portal is how Zed opens a folder picker. Without the portal` \
      `#   "Open folder" and "Add project" end in a panic with "portal request` \
      `#   failed": the application calls a service the system does not have.` \
      `#   -gtk is the dialog implementation for non-GNOME environments.` \
      xdg-desktop-portal xdg-desktop-portal-gtk \
      `# Russian spell checking: comments in 1C code and the documentation are` \
      `#   written in Russian. System-wide dictionaries are visible to Firefox` \
      `#   and anything going through hunspell/enchant.` \
      hunspell hunspell-ru hunspell-en-us \
      `# --- Python for the agent ---` \
      `# The agent regularly gets tasks like "parse this document", "count over` \
      `# the dump", "build a report". Installed as packages rather than with pip:` \
      `# Debian 13 forbids installing into the system python (PEP 668).` \
      python3 python3-venv python3-pip \
      python3-lxml python3-yaml python3-requests python3-dateutil python3-chardet \
      `# office formats: contracts, acts, exports to Excel` \
      python3-docx python3-openpyxl python3-xlsxwriter python3-pypdf \
      python3-pandas python3-tabulate python3-bs4 python3-pil \
    && rm -rf /var/lib/apt/lists/*

# Windows drives are mounted by xrdp-chansrv through FUSE. user_allow_other is
# required so the mount is visible to every process in the session, not only to
# the one that mounted it: otherwise the file manager and the editor see nothing.
# The /dev/fuse device itself is granted by the launch script — Docker does not
# hand it to a container by default.
RUN grep -qx 'user_allow_other' /etc/fuse.conf || echo 'user_allow_other' >> /etc/fuse.conf

RUN sed -i 's/^# *ru_RU.UTF-8/ru_RU.UTF-8/' /etc/locale.gen \
    && locale-gen \
    && ln -snf /usr/share/zoneinfo/Europe/Moscow /etc/localtime \
    && echo Europe/Moscow > /etc/timezone


# =============================================================================
# Everything fetched over the network. These layers are the expensive ones and
# almost never change — hence they sit above all our own files.
# =============================================================================

RUN curl -fsSL https://zed.dev/api/releases/stable/latest/zed-linux-x86_64.tar.gz \
      | tar -xz -C /opt \
    && ln -s /opt/zed.app/bin/zed /usr/local/bin/zed

# Application menu entry plus icon (the tarball ships them, but outside XDG paths)
RUN ln -s /opt/zed.app/share/applications/dev.zed.Zed.desktop /usr/share/applications/ \
    && mkdir -p /usr/share/icons/hicolor/512x512/apps \
    && ln -s /opt/zed.app/share/icons/hicolor/512x512/apps/zed.png \
             /usr/share/icons/hicolor/512x512/apps/zed.png \
    && ln -s /opt/zed.app/share/icons/hicolor/512x512/apps/zed.png \
             /usr/share/pixmaps/zed.png

# --- 1C tooling: everything network-bound is frozen at build time -------------
# In the customer's air-gapped network the extension can neither download itself
# nor fetch the language server binary, so both downloads happen here, while
# there is internet. Versions are pinned: "latest" would make builds
# irreproducible.
#
# Note the -app- asset: bsl-analyzer-linux-amd64 is a DOWNLOADER that calls an
# update server at startup and will not work offline. The -app- build is the
# application itself: both the language server (`lsp`) and the MCP server
# (`mcp serve`) live in one binary.
ARG BSL_ANALYZER_VERSION=v0.3.2
RUN mkdir -p /opt/bsl-analyzer \
    && curl -fsSL "https://github.com/itrous/bsl-analyzer/releases/download/${BSL_ANALYZER_VERSION}/bsl-analyzer-app-linux-amd64" \
       -o /opt/bsl-analyzer/bsl-analyzer \
    && chmod +x /opt/bsl-analyzer/bsl-analyzer \
    && /opt/bsl-analyzer/bsl-analyzer --version

# Zed extensions frozen at build time. Otherwise Zed reaches for the registry on
# first launch (visible as "installing extension html latest version").
#   bsl        — 1C grammar plus the language server binding (the important one)
#   xml        — a 1C configuration dump is XML, the main source format
#   html, csv  — web forms and data exchange
#   ini        — nethasp.ini, conf.cfg and other platform configs
#   sql        — PostgreSQL under 1C
#   dockerfile, toml, powershell — the wrapper: image, configs, host scripts
# markdown and yaml are not needed — both are built into Zed.
# ponytail: versions are not pinned, latest comes from the registry. If build
#   reproducibility is ever required, the API accepts ?version=.
ARG ZED_EXTENSIONS="bsl xml html csv ini sql dockerfile toml powershell"
RUN set -eu; \
    for id in ${ZED_EXTENSIONS}; do \
      dir=/etc/skel/.local/share/zed/extensions/installed/$id; \
      mkdir -p "$dir"; \
      curl -fsSL "https://api.zed.dev/extensions/$id/download" | tar -xz -C "$dir"; \
      echo "extension $id: $(ls "$dir" | tr '\n' ' ')"; \
    done

# --- PowerShell ---------------------------------------------------------------
# The 1C skills rest on 28,840 lines of PowerShell: parsing and assembling XML
# forms, data composition schemas, spreadsheet documents, metadata. Rewriting
# that in Python would take months and leave a permanent fork with no upstream.
# Verified: the scripts contain no Windows specifics (no COM, WMI, registry,
# System.Windows), only the cross-platform .NET core — exactly what PowerShell
# Core was made for. The price is about 200 MB in the image.
ARG PWSH_VERSION=7.6.6
RUN mkdir -p /opt/powershell \
    && curl -fsSL "https://github.com/PowerShell/PowerShell/releases/download/v${PWSH_VERSION}/powershell-${PWSH_VERSION}-linux-x64.tar.gz" \
       | tar -xz -C /opt/powershell \
    && chmod +x /opt/powershell/pwsh \
    && ln -s /opt/powershell/pwsh /usr/local/bin/pwsh \
    && pwsh -NoProfile -Command '$PSVersionTable.PSVersion.ToString()'

# The model server is NOT in this image any more: dev1c-setup runs it as a
# service on the host or as the sibling container dev1c-llama (Dockerfile.llama),
# and Zed reaches it as a remote OpenAI-compatible provider. That took CUDA
# (about 630 MB) and the only reason to hand this container a GPU out of here.

# --- Gitea --------------------------------------------------------------------
# A git server inside the image rather than a separate service. For a pilot this
# is deliberate: the goal is to introduce 1C developers to git, and an extra
# "first set up a server" step loses half of them. One Go binary, SQLite for
# storage. Data lives in a volume, so it survives image updates.
ARG GITEA_VERSION=1.25.2
RUN mkdir -p /opt/gitea \
    && curl -fsSL "https://dl.gitea.com/gitea/${GITEA_VERSION}/gitea-${GITEA_VERSION}-linux-amd64" \
       -o /opt/gitea/gitea \
    && chmod +x /opt/gitea/gitea \
    && /opt/gitea/gitea --version | head -1 \
    `# icon: take the Gitea logo from its own assets, or the launcher is blank` \
    && curl -fsSL "https://raw.githubusercontent.com/go-gitea/gitea/main/public/assets/img/logo.svg" \
       -o /usr/share/pixmaps/gitea.svg \
    && cp /usr/share/pixmaps/gitea.svg /usr/share/icons/hicolor/scalable/apps/gitea.svg 2>/dev/null || true

# Content Gitea is seeded with on first start. PRISM is an open benchmark of LLMs
# on 1C tasks (MIT): a newcomer opens it, reads the README and immediately sees
# what models are measured on and how the tasks are built. It serves both as an
# introduction to git and as material for debugging the skills.
RUN git clone --depth=1 https://github.com/genlab-1c/prism.git /opt/seed/prism \
    && rm -rf /opt/seed/prism/.git \
    && echo "seed: $(ls /opt/seed | tr '\n' ' ')"

# --- tea: the Gitea CLI -------------------------------------------------------
# Open a pull request, look through issues and CI runs without leaving the
# terminal. Gitea itself runs as a service in this same container; this is only
# the client.
ARG TEA_VERSION=0.16.0
RUN curl -fsSL "https://dl.gitea.com/tea/${TEA_VERSION}/tea-${TEA_VERSION}-linux-amd64" \
       -o /usr/local/bin/tea \
    && chmod +x /usr/local/bin/tea \
    && tea --version | head -1

# --- OneScript ----------------------------------------------------------------
# A standalone interpreter of the 1C language: it needs no platform. Required for
# two things:
#   1. The PRISM benchmark: category A tasks (algorithms) run exactly in
#      OneScript, without a platform — roughly half of the set.
#   2. Debugging the skills and the project's own .os helper scripts.
# Version 2.0+ is needed for vanessa-runner 3.0, should it ever be installed
# alongside (vrunner itself lives on the machine with the platform and is useless
# here).
ARG ONESCRIPT_VERSION=2.2.0
RUN curl -fsSL "https://github.com/EvilBeaver/OneScript/releases/download/v${ONESCRIPT_VERSION}/OneScript-${ONESCRIPT_VERSION}-linux-x64.zip" \
       -o /tmp/onescript.zip \
    && mkdir -p /opt/onescript \
    && unzip -q /tmp/onescript.zip -d /opt/onescript \
    && rm /tmp/onescript.zip \
    `# the archive unpacks into a bin/ subdirectory, not into the root` \
    && chmod +x /opt/onescript/bin/oscript /opt/onescript/bin/opm \
    && ln -sf /opt/onescript/bin/oscript /usr/local/bin/oscript \
    && ln -sf /opt/onescript/bin/opm /usr/local/bin/opm \
    && oscript -version

# The SSL skill (standard subsystems library) comes from a separate MIT
# repository: 24 topic references with signatures, examples and anti-patterns.
# It is needed because a model unfamiliar with the library invents methods that
# do not exist — writing its own email or access-rights handling instead of
# calling the ready subsystem. The `bsp-patterns` skill only covers general
# patterns.
#
# Cloned into /opt here, and copied into the skills directory further down: that
# way editing our own skills does not re-run the clone.
RUN git clone --depth=1 https://github.com/brake71/1c-ssl-skills.git /opt/ssl-skills \
    && rm -rf /opt/ssl-skills/.git \
    && echo "ssl-skills: $(ls /opt/ssl-skills/skills/bsp/references | wc -l) references"

# Theme from the build stage: finished files only, no meson/npm/sassc
COPY --from=theme-builder /usr/share/themes/1C-light /usr/share/themes/1C-light
COPY --from=theme-builder /usr/share/themes/1C-dark  /usr/share/themes/1C-dark


# =============================================================================
# Our own files from here on. Everything below is cheap to rebuild, which is the
# point of the ordering: editing a script or a skill costs seconds, not a
# re-download of the heavy layers.
# =============================================================================

# --- Branding -----------------------------------------------------------------
COPY config/brand/ /usr/share/oktaplus/
# Wallpaper as a ready file. It used to be drawn on every build by
# make-wallpaper, for whose sake imagemagick and rsvg-convert lived in the image.
# The picture is identical in every build, so it was generated once (the script
# stays in config/ in case branding changes) and is simply copied.
COPY config/brand/wallpaper.jpg /usr/share/backgrounds/dev1c.jpg

# --- Scripts ------------------------------------------------------------------
COPY config/autoscale.sh        /usr/local/bin/xfce-autoscale
COPY config/keyring-start.sh    /usr/local/bin/keyring-start
COPY config/xsession.sh         /usr/local/bin/xsession
COPY config/sshd-setup.sh       /usr/local/bin/sshd-setup
COPY config/entrypoint.sh       /usr/local/bin/entrypoint
COPY config/gitea-user-setup.sh /usr/local/bin/gitea-user-setup
COPY config/gitea-start.sh      /usr/local/bin/gitea-start
COPY config/normalize-skills.py /usr/local/bin/normalize-skills
COPY config/test-skills.sh      /usr/local/bin/test-skills
COPY config/test-entrypoint-ports.sh /usr/local/bin/test-entrypoint-ports

# These scripts are edited on Windows too, where editors save CRLF. In a shebang
# that is fatal: the system looks for an interpreter named "/bin/sh" with a
# trailing carriage return and reports "no such file" — about a file that is
# right there. Normalising at build time removes the dependency on whatever
# editor touched the file.
#
# The \r in the sed expression is a GNU sed escape, not the character itself. A
# literal carriage return cannot survive inside a Dockerfile: any tool with
# universal newline handling turns it into a line break and tears the
# instruction apart — which is exactly how this file once stopped building.
#
# Text files only (grep -I fails on a binary): the same directory holds
# downloaded binaries, and sed once rewrote the \r\n sequences inside mcp-1c —
# the ELF still ran in the build layer, where it was checked before this step,
# and died with SIGILL in the running container.
RUN find /usr/local/bin -maxdepth 1 -type f -exec grep -Iq . {} \; -exec sed -i 's/\r$//' {} + \
    && head -1 /usr/local/bin/gitea-start | grep -qx '#!/bin/sh' \
    && chmod +x /usr/local/bin/xfce-autoscale /usr/local/bin/keyring-start \
                /usr/local/bin/xsession /usr/local/bin/sshd-setup \
                /usr/local/bin/entrypoint /usr/local/bin/gitea-user-setup \
                /usr/local/bin/gitea-start /usr/local/bin/normalize-skills \
                /usr/local/bin/test-skills /usr/local/bin/test-entrypoint-ports \
    && test -s /usr/share/backgrounds/dev1c.jpg \
    && echo "scripts normalised"

# --- Zed ----------------------------------------------------------------------
# Global settings: telemetry and auto-updates off, language server and models
# configured. Global rather than per-project — Restricted Mode does not block
# those.
# Терминал: копирование выделением и вставка на Ctrl+V — человек приходит
# из Windows и ждёт именно этого. Ctrl+C под копирование не отдан: им
# прерывают работающую команду, другого способа у человека нет.
# Две копии: в /etc/skel для новых учётных записей и в /usr/share/oktaplus
# для сверки на входе (дома рабочих мест — том, образ их не перекрывает).
# Скобочная вставка в readline. Без неё вставка во встроенный терминал Zed
# приходит с маркерами наружу: bash печатает «^[[200~текст~» и ругается на
# несуществующую команду (поймано на стенде 20.10.2026). Zed оборачивает
# вставляемое в маркеры, а bash их не понимает, пока режим выключен.
# В Debian этот ключ выключен, хотя сам bash 5.2 включает его по умолчанию.
# Включённый режим заодно не даёт вставленным многострочным командам
# выполниться самим: человек увидит их и нажмёт ввод сам.
RUN { echo ""; echo "# Вставка приходит как текст, а не как набор с клавиатуры"; echo "set enable-bracketed-paste on"; } >> /etc/inputrc

COPY config/terminalrc           /etc/skel/.config/xfce4/terminal/terminalrc
COPY config/terminal-accels.scm  /etc/skel/.config/xfce4/terminal/accels.scm
COPY config/terminalrc           /usr/share/oktaplus/terminal/terminalrc
COPY config/terminal-accels.scm  /usr/share/oktaplus/terminal/accels.scm
COPY config/zed-settings.json /etc/skel/.config/zed/settings.json
# The 1C themes for Zed: the same tokens as the desktop and 1c-connect.
COPY config/zed-theme-1c.json /etc/skel/.config/zed/themes/1c.json
# Agent rules shared by every project: what this environment has, what it lacks
# and how to run 1C code. Without them the model answers with guesses — it
# declared OneScript impossible on Linux, confusing it with Windows Script Host.
COPY config/agents-global.md /etc/skel/.config/zed/AGENTS.md

# --- Desktop environment ------------------------------------------------------
# XFCE autostart brings up its own keyring daemon — with a locked collection.
# Ours (keyring-start with --unlock) displaces it via --replace, but the order is
# not guaranteed: lose that race and the session ends up with two daemons and two
# keyrings (login.keyring and Default.keyring), libsecret goes to the locked one,
# and Zed panics in oo7 instead of showing an unlock dialog — which looks exactly
# like "the agent does not answer". The daemon starts once, from .xsession, and
# brings up all its components itself, so the autostart entries are redundant.
RUN rm -f /etc/xdg/autostart/gnome-keyring-*.desktop

COPY config/xsettings.xml      /etc/skel/.config/xfce4/xfconf/xfce-perchannel-xml/xsettings.xml
COPY config/xfwm4.xml          /etc/skel/.config/xfce4/xfconf/xfce-perchannel-xml/xfwm4.xml
COPY config/xfce4-desktop.xml  /etc/skel/.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-desktop.xml
# Panel with a keyboard layout indicator (plugin-19 = xkb). A copy of the stock
# /etc/xdg/xfce4/panel/default.xml with one added item.
COPY config/xfce4-panel.xml    /etc/skel/.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml

# The same file in both directories: GTK3 and GTK4 read settings separately, and
# without the second copy some windows stay dark under a light theme.
COPY config/gtk-settings.ini /etc/skel/.config/gtk-3.0/settings.ini
COPY config/gtk-settings.ini /etc/skel/.config/gtk-4.0/settings.ini

COPY config/keyboard-layout.xml /etc/skel/.config/xfce4/xfconf/xfce-perchannel-xml/keyboard-layout.xml
# Safety net: the layout is overwritten by xorgxrdp and xfce4-settings-daemon,
# and this autostart entry runs after both. Its result is written to
# /tmp/xkb-applied.log for diagnostics.
COPY config/keyboard-autostart.desktop /etc/xdg/autostart/zz-keyboard.desktop
# Screen blanking and the screensaver: over RDP they look like a frozen session
COPY config/session-tweaks.desktop /etc/xdg/autostart/zz-session-tweaks.desktop

# Thunar sidebar bookmarks
COPY config/thunar-bookmarks /etc/skel/.config/gtk-3.0/bookmarks

# Gitea launcher and the credentials banner template
COPY config/gitea-banner.tmpl /opt/gitea/banner.tmpl
COPY config/gitea.desktop     /usr/share/applications/gitea.desktop

# bash additions: history, prompt with the git branch, aliases
COPY config/bashrc-extra.sh /etc/profile.d/zz-dev1c.sh
RUN echo '. /etc/profile.d/zz-dev1c.sh' >> /etc/bash.bashrc

# 1C files open in Zed on double click. Without the association Thunar does not
# know what to do with .bsl and asks to pick a program every time.
# The visible comment stays Russian: it is shown in the file manager, and the
# audience of this environment is Russian-speaking.
RUN printf '%s\n' \
      '<?xml version="1.0" encoding="UTF-8"?>' \
      '<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">' \
      '  <mime-type type="text/x-bsl">' \
      '    <comment>Модуль 1С:Предприятие</comment>' \
      '    <comment xml:lang="en">1C:Enterprise module</comment>' \
      '    <glob pattern="*.bsl"/>' \
      '    <glob pattern="*.os"/>' \
      '    <sub-class-of type="text/plain"/>' \
      '  </mime-type>' \
      '</mime-info>' \
    > /usr/share/mime/packages/bsl.xml \
    && update-mime-database /usr/share/mime \
    && mkdir -p /etc/skel/.config \
    && printf '%s\n' \
      '[Default Applications]' \
      'text/x-bsl=dev.zed.Zed.desktop' \
      'text/plain=dev.zed.Zed.desktop' \
      'application/xml=dev.zed.Zed.desktop' \
      'text/xml=dev.zed.Zed.desktop' \
      'text/html=firefox-esr.desktop' \
      'x-scheme-handler/http=firefox-esr.desktop' \
      'x-scheme-handler/https=firefox-esr.desktop' \
    > /etc/skel/.config/mimeapps.list

# --- Agent skills -------------------------------------------------------------
# Zed reads them from ~/.agents/skills/ as a flat list (nesting is not
# supported). The selection is documented in
# docs/superpowers/specs/*skills-revision.md
#
# The skills themselves stay in Russian: they deal with 1C terminology, and the
# code they describe is Russian.
COPY config/skills/ /etc/skel/.agents/skills/

# The SSL reference, cloned higher up, plus two mechanical edits the skills need
# because they were written for Windows and Claude Code. Both are repeatable on
# any fresh upstream version:
#   powershell.exe  -> pwsh              (the binary is named differently on Linux)
#   .claude/skills/ -> ~/.agents/skills/ (Zed uses another path)
RUN cp -r /opt/ssl-skills/skills/bsp /etc/skel/.agents/skills/bsp \
    && find /etc/skel/.agents/skills \( -name '*.md' -o -name '*.ps1' -o -name '*.json' \) -type f -print0 \
       | xargs -0 sed -i \
           -e 's|powershell\.exe|pwsh|g' \
           -e 's|\.claude/skills/|~/.agents/skills/|g' \
    && echo "skills installed: $(ls /etc/skel/.agents/skills | wc -l)" \
    && echo "powershell.exe mentions left: $(grep -rl 'powershell\.exe' /etc/skel/.agents/skills 2>/dev/null | wc -l)"

# Skills arrive from Windows repositories, and two things break them silently: a
# BOM before `---` (the parser does not see the frontmatter and the skill is not
# loaded at all) and unquoted values containing `[` (YAML reads them as a list
# and the parse collapses). The script fixes both and exits non-zero if any
# invalid frontmatter remains.
RUN python3 /usr/local/bin/normalize-skills /etc/skel/.agents/skills

# Smoke test: if the skills' PowerShell scripts do not run, the build must fail
# here rather than be discovered by a developer in the field.
RUN test-skills

# --- git ----------------------------------------------------------------------
# Settings shared by every user of the image.
# safe.directory — otherwise git complains about "dubious ownership" on volume
# directories: the owner on the host and the uid in the container differ, and
# work stops.
# autocrlf=input — 1C dumps often arrive from Windows; the repository keeps LF.
# One file instead of a pile of commands: visible as a whole and easy to edit.
COPY config/gitconfig /etc/gitconfig
RUN git lfs install --system \
    && git config --system --get user.name

# --- Users --------------------------------------------------------------------
# Three workstations in one image. More is deliberately not offered: the server
# is single, and a fourth concurrent BSL analyzer on an ERP-sized configuration
# would run out of memory before being any use.
#
# Each gets its own /home: own Zed with history and settings, own analyzer cache,
# own projects. Only the services are shared — Gitea and sshd, which start
# with the container rather than within a user session.
#
# Password equals login: the environment sits behind the RDP password already,
# and memorising three separate pairs for a pilot is friction with no benefit.
RUN echo 'dev1 ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/dev \
    && echo 'dev2 ALL=(ALL) NOPASSWD:ALL' >> /etc/sudoers.d/dev \
    && echo 'dev3 ALL=(ALL) NOPASSWD:ALL' >> /etc/sudoers.d/dev \
    && chmod 0440 /etc/sudoers.d/dev \
    && for u in dev1 dev2 dev3; do \
         useradd -m -s /bin/bash "$u"; \
         echo "$u:$u" | chpasswd; \
         usermod -aG sudo "$u"; \
         cp /usr/local/bin/xsession "/home/$u/.xsession"; \
         mkdir -p "/home/$u/Desktop" \
                  "/home/$u/.local/share/zed/db" \
                  "/home/$u/.local/share/keyrings" \
                  "/home/$u/.cache/bsl-analyzer" \
                  "/home/$u/proj" \
                  "/home/$u/examples"; \
         cp /opt/zed.app/share/applications/dev.zed.Zed.desktop "/home/$u/Desktop/"; \
         cp /usr/share/applications/gitea.desktop "/home/$u/Desktop/"; \
         chmod +x "/home/$u/Desktop/dev.zed.Zed.desktop" "/home/$u/Desktop/gitea.desktop"; \
         chown -R "$u:$u" "/home/$u"; \
       done

# A service account for Gitea: the service is shared and must not depend on
# whether anyone has logged in, or under which login.
RUN useradd -r -m -d /var/lib/gitea -s /bin/sh gitea \
    && mkdir -p /var/lib/gitea \
    && chown -R gitea:gitea /var/lib/gitea

# The SSH tunnel comes in under the workstation's own account (dev1..dev3):
# sshd lets each one forward only its own three ports and never a shell (see
# sshd-setup). No separate tunnel account.

# --- Environment --------------------------------------------------------------
# ENV alone is not enough here: an xrdp session is created by PAM and the image
# environment is lost, so the values also go where pam_env reads them at RDP
# login.
#   ZED_ALLOW_EMULATED_GPU — Zed refuses to start on software Vulkan (lavapipe)
#     without it
#   GTK_A11Y=none — there is no accessibility bus in a container, and without
#     this every GTK application floods the log with org.a11y.Bus warnings
#
# NVIDIA: driver libraries are mounted from the host by nvidia-container-toolkit,
# nothing needs installing into the image. The capabilities matter though:
#   compute — CUDA, utility — nvidia-smi, graphics — OpenGL/Vulkan.
# By default the toolkit passes only compute,utility, and without graphics
# hardware rendering inside the container will not work — which is precisely what
# people expect when they pass a GPU through.
ENV NVIDIA_VISIBLE_DEVICES=all \
    NVIDIA_DRIVER_CAPABILITIES=compute,utility,graphics

# The interface inside is Russian: that is the audience of this environment.
ENV LANG=ru_RU.UTF-8 TZ=Europe/Moscow ZED_ALLOW_EMULATED_GPU=1 GTK_A11Y=none
RUN printf 'LANG=ru_RU.UTF-8\nLANGUAGE=ru_RU:ru\nTZ=Europe/Moscow\nZED_ALLOW_EMULATED_GPU=1\nGTK_A11Y=none\n' >> /etc/environment \
    && printf 'LANG=ru_RU.UTF-8\nLANGUAGE=ru_RU:ru\n' > /etc/default/locale

# The model key never reaches the image: it is passed as an environment variable
# at run time (dev1c.sh and dev1c.ps1 read it from .env or from the environment),
# and the entrypoint moves it into /etc/environment where the RDP session sees
# it. A build argument was more convenient, but wrote the key into image layers
# and into `docker history`: a release had to be built with a separate command,
# and somebody had to remember that. Now every build is clean by construction.

# --- Remote desktop -----------------------------------------------------------
# Session resolution is chosen by the RDP client, but it can only pick from the
# mode list of xorgxrdp, and the stock list ends at 1920x1080. On a 2K/4K or
# ultrawide monitor the session would hit that ceiling.
RUN sed -i 's|^\( *Modes \).*|\1"3840x2160" "3440x1440" "2560x1440" "2560x1080" "1920x1200" "1920x1080" "1680x1050" "1600x900" "1440x900" "1366x768" "1280x1024" "1280x720" "1024x768" "800x600"|' \
      /etc/X11/xrdp/xorg.conf \
    && grep -c 3840x2160 /etc/X11/xrdp/xorg.conf

# The xrdp login screen is the first thing a person sees, before any theme loads.
# xrdp accepts a logo in BMP only, and it is copied as a ready file: converting
# from SVG kept imagemagick and rsvg-convert in the image, while the picture came
# out identical in every build. To regenerate after a branding change:
#   rsvg-convert -w 250 config/brand/oktaplus-lockup-horizontal.svg -o /tmp/l.png
#   magick /tmp/l.png -background "#F4F6F9" -flatten -type TrueColor BMP3:config/brand/login-logo.bmp
# The dialog background stays light: xrdp label colours are not configurable, and
# on a dark background they would be unreadable.
#
# The title stays Russian — it is part of the interface, which is Russian.
COPY config/brand/login-logo.bmp /usr/share/oktaplus/login-logo.bmp
RUN sed -i \
      -e 's|^ls_top_window_bg_color=.*|ls_top_window_bg_color=16191F|' \
      -e 's|^ls_bg_color=.*|ls_bg_color=F4F6F9|' \
      -e 's|^ls_logo_filename=.*|ls_logo_filename=/usr/share/oktaplus/login-logo.bmp|' \
      -e 's|^ls_logo_width=.*|ls_logo_width=250|' \
      -e 's|^ls_logo_height=.*|ls_logo_height=60|' \
      -e 's|^ls_logo_y_pos=.*|ls_logo_y_pos=28|' \
      -e 's|^#ls_title=.*|ls_title=OktaPlus — среда разработки 1С|' \
      /etc/xrdp/xrdp.ini

EXPOSE 3389
# sshd runs alongside xrdp: the machine with 1C connects to it and forwards its
# own port into the container. Host keys are generated on first start (see
# sshd-setup), so every deployment has its own fingerprint.
CMD ["/usr/local/bin/entrypoint"]
