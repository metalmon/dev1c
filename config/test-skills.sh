#!/bin/sh
# Verify that the 1C skills actually run inside the Linux image.
#
# Runs at build time: if the PowerShell scripts do not work, the image must not
# build. Otherwise the breakage surfaces in the middle of someone's work and
# looks like "the agent cannot create a form for some reason".
#
# What is checked is the actual result of running a script, not whether a file is
# in place.
set -eu

SKILLS="${SKILLS:-/etc/skel/.agents/skills}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail() { echo "TEST FAILED: $*" >&2; exit 1; }
ok()   { echo "  ok: $*"; }

echo "== Environment =="

command -v pwsh >/dev/null || fail "pwsh not found"
pwsh -NoProfile -Command 'exit 0' || fail "pwsh does not start"
ok "pwsh $(pwsh -NoProfile -Command '$PSVersionTable.PSVersion.ToString()')"

command -v python3 >/dev/null || fail "python3 not found"
python3 -c 'import lxml, docx, openpyxl, yaml, requests' \
  || fail "Python libraries for the agent do not import"
ok "python3 with libraries (lxml, docx, openpyxl, yaml, requests)"

echo "== Skills =="

[ -d "$SKILLS" ] || fail "skills directory missing: $SKILLS"
COUNT=$(find "$SKILLS" -maxdepth 1 -mindepth 1 -type d | wc -l)
[ "$COUNT" -gt 0 ] || fail "no skills found"
ok "skills: $COUNT"

# Zed does not support nesting: every skill is a direct child of the directory
BAD=$(find "$SKILLS" -mindepth 2 -maxdepth 2 -name SKILL.md | wc -l)
[ "$BAD" -eq "$COUNT" ] || fail "some skills have no SKILL.md at the first level"
ok "flat structure, every skill has SKILL.md"

# Path rewrites applied
if grep -rl 'powershell\.exe' "$SKILLS" >/dev/null 2>&1; then
    fail "powershell.exe calls remain (the binary is named pwsh on Linux)"
fi
ok "powershell.exe calls replaced with pwsh"

# The frontmatter is how the agent finds a skill. Invalid YAML means the skill
# simply never appears in the catalog — silently, with no error in the UI.
python3 /usr/local/bin/normalize-skills "$SKILLS" >/dev/null \
  || fail "invalid YAML frontmatter in skills (see normalize-skills output)"
ok "frontmatter of every skill is valid"

echo "== Zed settings =="

ZED_SETTINGS="${ZED_SETTINGS:-/etc/skel/.config/zed/settings.json}"
ZED_BIN="${ZED_BIN:-/opt/zed.app/libexec/zed-editor}"

if [ -f "$ZED_SETTINGS" ] && [ -f "$ZED_BIN" ]; then
    # Zed silently ignores unknown keys and only highlights them in the editor.
    # Catch them here: settings taken from the documentation of a newer release
    # may be absent from the version in the image (that happened with
    # suggest_extensions and agent_font_size -> agent_ui_font_size).
    #
    # Our own provider and server names cannot be in the binary by definition —
    # hence the exception list. It has to match the names in settings.json: when
    # the provider was renamed from a branded name to "custom", a stale entry
    # here would have failed the build.
    #
    # The second group is tool names. The agent profiles switch individual MCP
    # tools off by name (agent.profiles.*.context_servers.*.tools), and a tool
    # of our own servers is as absent from the Zed binary as the server itself.
    # Built-in tool names are deliberately NOT listed: read_file, terminal and
    # the rest must stay checked, because a profile that names a built-in Zed
    # no longer has would silently lose it.
    UNKNOWN=""
    for k in $(grep -oE '^[[:space:]]*"[a-z_.]+"[[:space:]]*:' "$ZED_SETTINGS" \
               | tr -d ' ":' | sort -u); do
        case "$k" in
            custom|bsl|bsl-reference|1c|vrunner) continue ;;
            # Имя штатного провайдера модели (llama.cpp) в исключениях
            # намеренно НЕ стоит: шаблон выше ловит его как ключ, в бинаре Zed
            # оно есть, и пусть проверяется. Уберут провайдер в новой версии —
            # сборка упадёт здесь, а не выяснится молчащим агентом у заказчика.
            its_help|cf_load|cf_decompile|project_pull|project_push) continue ;;
            infobase_update|task_cancel|cache_show|cache_clear) continue ;;
            test_xunit|test_vanessa|test_yaxunit) continue ;;
        esac
        grep -aq "$k" "$ZED_BIN" || UNKNOWN="$UNKNOWN $k"
    done
    [ -z "$UNKNOWN" ] || fail "settings.json has keys absent from this Zed version:$UNKNOWN"
    ok "every settings.json key is known to the installed Zed"
else
    echo "  skipped: Zed settings or binary not found"
fi

echo "== Functional check: cf-init creates a configuration =="

CF_INIT="$SKILLS/cf-init/scripts/cf-init.ps1"
if [ -f "$CF_INIT" ]; then
    cd "$WORK"
    # The name is Russian on purpose: 1C identifiers are Russian, and this is
    # also the encoding check below.
    pwsh -NoProfile -File "$CF_INIT" -Name "ТестоваяКонфигурация" >/dev/null 2>&1 \
      || fail "cf-init.ps1 exited with an error"

    # The script must produce the configuration XML — found by name rather than
    # by an exact directory, which may change between upstream versions.
    FOUND=$(find "$WORK" -name 'Configuration.xml' | head -1)
    [ -n "$FOUND" ] || fail "cf-init.ps1 ran but produced no Configuration.xml"

    # And it must be valid XML containing Cyrillic
    xmllint --noout "$FOUND" || fail "the produced Configuration.xml is invalid"
    grep -q 'ТестоваяКонфигурация' "$FOUND" \
      || fail "Configuration.xml has no configuration name (encoding problem?)"
    ok "cf-init produced valid Configuration.xml with Cyrillic intact"
else
    echo "  skipped: cf-init is not part of this image"
fi

echo "== All checks passed =="
