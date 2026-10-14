#!/bin/sh
# Start the keyring daemon within a session.
#
# Zed panics in oo7/dbus/api/prompt.rs when the keyring needs an unlock dialog:
# instead of a window it gets a dead worker thread. From the outside this looks
# like "the agent does not answer" — with a perfectly valid key, provider and
# network.
#
# The recipe was established by experiment, and every part of it matters:
#
#   --unlock          reads a password from stdin and unlocks the keyring right
#                     away; without it the collection stays locked and Zed asks
#                     for a dialog
#   the NUL byte      that is how unlock mode expects an empty password
#   --replace         a safety net: displaces someone else's instance if it
#                     started first. The XFCE autostart entries are removed at
#                     build time (they launched a daemon with a locked
#                     collection), but the daemon can still come up through bus
#                     activation
#
# The --login variant creates a collection named Default, which libsecret does
# not treat as the primary one: Zed ends up in a dialog anyway. login.keyring is
# what is needed.
#
# An empty password is acceptable: entry into the environment is already behind
# the RDP password, a second one protects nothing, and asking for it on every
# login is a reliable way to hear "it keeps prompting me again".
#
# Sourced from .xsession with a dot so the daemon's variables reach the session.

_kr=$(printf '\000' | gnome-keyring-daemon --unlock --replace 2>/dev/null)
[ -n "$_kr" ] && eval "$_kr"

export GNOME_KEYRING_CONTROL SSH_AUTH_SOCK
unset _kr
