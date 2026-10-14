# Session startup. The order here matters.
#
# 1. Keyboard layout — a safety net until xfce4-settings-daemon starts and
#    applies its own configuration (see keyboard-layout.xml).
setxkbmap -layout us,ru -option grp:alt_shift_toggle

# 2. Interface scale, derived from the actual RDP session resolution
. /usr/local/bin/xfce-autoscale

# 3. The D-Bus session bus is started HERE, before the keyring and before XFCE.
#    Otherwise the keyring registers on one bus while applications look for it on
#    another, and every call to secrets fails with "DBus error: i/o error".
#    XFCE picks up the existing bus instead of starting its own.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    eval "$(dbus-launch --sh-syntax --exit-with-session)"
    export DBUS_SESSION_BUS_ADDRESS DBUS_SESSION_BUS_PID
fi

# 4. The keyring — now on the right bus. Without it Zed panics on any access to
#    secrets.
. /usr/local/bin/keyring-start

# 5. Thunar sidebar bookmarks: the template holds __HOME__ because GTK reads the
#    entries as literal URIs and expands no variables, while each workstation has
#    its own home directory. A one-off substitution — bookmarks the user adds
#    later are left alone.
_bm="$HOME/.config/gtk-3.0/bookmarks"
if [ -f "$_bm" ] && grep -q __HOME__ "$_bm" 2>/dev/null; then
    sed -i "s|__HOME__|$HOME|g" "$_bm"
fi
unset _bm

# 6. Access to the shared Gitea for this user: credentials, a tea token, working
#    copies of the examples. In the background, so login is not delayed while the
#    service is still coming up.
/usr/local/bin/gitea-user-setup >/dev/null 2>&1 &

exec startxfce4
