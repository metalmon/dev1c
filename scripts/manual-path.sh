#!/bin/sh
# Build the administrator's page for the Linux kit: the hand-written part from
# config/kit/УСТАНОВКА.md with its @MANUAL-PATH@ line replaced by the «Ручной
# путь» section, generated from the installer itself.
#
#   sh scripts/manual-path.sh <kit dir>        → <kit dir>/УСТАНОВКА.md
#
# The customer asked to be able to stand the environment up without our
# binary, so the page carries every modifying command. They are not written by
# hand: `dev1c-setup install --dry-run --commands` prints what the operations
# would run (common::docker only prints under --dry-run, so no docker and no
# GPU are needed here), and this script turns that into the section. A command
# that changes in the program changes in the page on the next kit.
#
# Two things are fed to the program rather than taken from the kit:
#   * [model] file — empty in the kit (the .gguf is the administrator's own
#     file), and with no name the model commands would print without one, so a
#     placeholder is substituted into a copy of the kit's dev1c.toml;
#   * the kit directory — the run happens in a temporary directory with
#     `--kit .`, so the printed paths stay relative and no path of the machine
#     that built the kit reaches the page.
set -eu

DIR=$(cd "$(dirname "$0")/.." && pwd)
KIT=$(cd "${1:?kit directory}" && pwd)
PAGE="$DIR/config/kit/УСТАНОВКА.md"
MARK='@MANUAL-PATH@'
MODEL=модель.gguf

SETUP="$KIT/dev1c-setup"
[ -x "$SETUP" ] || SETUP="$KIT/dev1c-setup.exe"
[ -x "$SETUP" ] || { echo "manual-path.sh: no dev1c-setup in $KIT" >&2; exit 1; }
[ -f "$KIT/dev1c.toml" ] || { echo "manual-path.sh: no dev1c.toml in $KIT" >&2; exit 1; }
grep -q "^$MARK\$" "$PAGE" || { echo "manual-path.sh: no $MARK line in $PAGE" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
sed "s|^file = \"\"|file = \"$MODEL\"|" "$KIT/dev1c.toml" >"$tmp/dev1c.toml"
(cd "$tmp" && "$SETUP" --kit . install --dry-run --commands) >"$tmp/raw.txt"

# Operation headings (== id — title) and the plan in words (· line) frame the
# commands; every other line is a command. A plan line that is itself a
# command (the plan names `docker load`, `docker volume create`, `docker run`
# as they are) is dropped: the block below it says the same, pasteably. The
# leading ./ (or .\ when the page is generated on Windows) is the kit
# directory the run was given.
awk '
    function fence() { if (open) { print "```"; open = 0; blank = 0 } }
    /^== / { fence(); sub(/^== /, ""); printf "\n**%s**\n\n", $0; blank = 1; next }
    /^· (docker|podman) / { next }
    /^· /  { fence(); sub(/^· /, ""); printf "- %s\n", $0; blank = 0; next }
    { if (!open) { if (!blank) print ""; print "```sh"; open = 1 } print }
    END { fence() }
' "$tmp/raw.txt" | sed 's|\./||g; s|\.\\||g' >"$tmp/section.md"

# A silently empty path would be the worst outcome: the page would look
# finished and stand nothing up. The three commands the whole install rests on
# have to be there, in the order the operations run them.
grep -v '^\(== \|· \)' "$tmp/raw.txt" >"$tmp/cmd.txt"
line_of() { # $1 = command prefix → line number of its first occurrence
    grep -n -- "$1" "$tmp/cmd.txt" | head -1 | cut -d: -f1
}
load=$(line_of 'docker load ')
volume=$(line_of 'docker volume create ')
run=$(line_of 'docker run ')
[ -n "$load" ] && [ -n "$volume" ] && [ -n "$run" ] \
    || { echo "manual-path.sh: the generated path has no docker load / volume create / run — nothing to install by hand" >&2; exit 1; }
[ "$load" -lt "$volume" ] && [ "$volume" -lt "$run" ] \
    || { echo "manual-path.sh: order broken — load $load, volume create $volume, run $run" >&2; exit 1; }

awk -v f="$tmp/section.md" -v mark="$MARK" '
    $0 == mark { while ((getline l < f) > 0) print l; next }
    { print }
' "$PAGE" >"$KIT/УСТАНОВКА.md"
echo "УСТАНОВКА.md: $(wc -l <"$KIT/УСТАНОВКА.md") строк, из них ручной путь $(wc -l <"$tmp/section.md")"
