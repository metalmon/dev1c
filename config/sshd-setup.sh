#!/bin/sh
# SSH server for the reverse tunnel from the machine that runs 1C.
#
# The scheme: that host connects to us as a client and forwards its own local
# port into the container (ssh -R). The container then talks to localhost and
# lands on the host. Execution stays on the host, where a process able to run
# vrunner and the platform lives.
#
# Why this direction and not ssh from us to the host: workstations in an
# air-gapped network are attested, and installing server services on them needs
# sign-off. An SSH client, by contrast, ships with Windows 10/11 and changes
# nothing in the system.
set -eu

SSH_PORT="${SSH_PORT:-2022}"
CONF=/etc/ssh/sshd_config.d/dev1c.conf

mkdir -p /etc/ssh/sshd_config.d /run/sshd

cat > "$CONF" <<EOF
# A separate port: 22 is left free so nothing conflicts with it
Port $SSH_PORT

# Password login is allowed on purpose. Keys would be stronger, but they mean
# somebody has to generate a pair, copy the public half into
# ~/.ssh/authorized_keys inside the container and keep it there across volume
# recreation — three manual steps before the first tunnel works. For a pilot
# whose point is to show git and an AI agent, that is friction that buys nothing:
# the environment already sits inside a closed network, behind the RDP password.
#
# The cost is stated plainly: anyone who can reach this port can log in by
# guessing dev1/dev1 — the same accounts that hold passwordless sudo over
# RDP. What limits the damage over SSH is the Match block below: no shell, no
# tty, no command, remote forwarding onto three ports and nothing else. Still:
# do not publish the port beyond the network where the environment lives.
# Outside a pilot, switch PasswordAuthentication to no and distribute keys.
PasswordAuthentication yes
PermitRootLogin no
PubkeyAuthentication yes

# The point of the whole scheme: allow reverse tunnels and let them listen on all
# container interfaces. Otherwise the forwarded port is visible to sshd alone and
# applications in the session cannot reach it.
AllowTcpForwarding yes
GatewayPorts clientspecified
PermitTunnel no

# The tunnel has to survive idle periods: without keepalives it silently drops
# after a few minutes of inactivity, and the agent gets "connection refused" out
# of nowhere.
ClientAliveInterval 30
ClientAliveCountMax 6
TCPKeepAlive yes

# The tunnel comes in under the workstation's own account (dev1, dev2, dev3 —
# the same system users, password equal to the login). One shared account
# with PermitListen on all nine ports would let a typo in `workstation` bind a
# neighbour's ports — and 778N runs the Configurator on that developer's
# machine.
AllowUsers dev1 dev2 dev3
EOF

# Over SSH these accounts get nothing but forwarding. ForceCommand applies when
# a client asks for a shell or a command; `ssh -N`, which is what a tunnel is,
# asks for neither and keeps working. So even someone who guesses the password
# ends up with port forwarding and no shell. Each workstation may listen on
# its own three ports only: dev1 -> 7771 (mcp-1c), 7781 (vrunner-mcp), 7791
# (web client of the base); dev2 -> 7772/7782/7792; dev3 -> 7773/7783/7793.
# Anything else is refused by sshd at the request, so a mistyped workstation
# name fails on the client with a clear message instead of binding a
# neighbour's tunnel. Remote forwarding only: no local forwards into the
# network, no tty, no X11, no agent — over SSH these sudo-capable RDP accounts
# can do exactly one thing.
for n in 1 2 3; do
    cat >> "$CONF" <<EOF
Match User dev$n
    ForceCommand /bin/false
    PermitTTY no
    X11Forwarding no
    AllowAgentForwarding no
    AllowTcpForwarding remote
    AllowStreamLocalForwarding no
    PermitOpen none
    PermitListen localhost:777$n localhost:778$n localhost:779$n
EOF
done

# Host keys are generated on first start rather than at build time: otherwise
# every deployment would share one fingerprint, and connecting to one
# installation would be indistinguishable from connecting to another.
ssh-keygen -A >/dev/null 2>&1 || true

exec /usr/sbin/sshd -D -e
