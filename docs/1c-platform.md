# Reaching the 1C platform

The platform is not in the image. This page is the whole of how the environment reaches it, and why the connection is made backwards.

No platform inside the image: the configurator, `1cv8` and `vanessa-runner` stay on the developer's machine. The environment reaches them through a **reverse SSH tunnel** — the server side is ours, inside the container (port 3393), and the machine with the platform only connects as a client.

The direction is inverted on purpose. In an air-gapped network workstations are attested, and installing server services on them requires sign-off from the security team. An SSH client, on the other hand, ships with Windows 10/11 since 1803, and running it is ordinary use of the system rather than a change to an attested workstation.

Everything about a developer's base lives on the developer's machine, in the `1c-connect/` kit: `1c-connect.exe`, its `1c-connect.toml`, `mcp-1c` and its extension. On the machine with 1C, once:

1. Copy the kit anywhere (a flash drive is fine) and fill `1c-connect.toml`: the environment's address and SSH port (printed by `dev1c-setup install`), your workstation (`dev1`…`dev3`), the base, its user, the address of its web publication.
2. `1c-connect install` — puts the `mcp-1c` extension into the base through the configurator; then publish the base on the web server (configurator → Administration → Publish on web server, "publish extension HTTP services by default" ticked). `1c-connect check` tells what is still missing.
3. `1c-connect connect` — raises the tunnel with its own SSH client and keeps it while the window is open. `Ctrl+C` closes it; nothing stays behind.

Inside the container the agent sees the base as a plain TCP port: `777N` for workstation `dev N` (a fresh `mcp-1c` on the base per connection), `778N` for `vrunner-mcp` (next stage), `779N` for the web client of the same base in Firefox: `http://localhost:779N/<publication>/`. Zed is already pointed at them (`nc -N localhost 777N`); the server side holds no parameter about any base.

Account `dev`, password `dev` — one shared pair, because the tunnel is a shared function rather than anybody's personal one. That account has no shell and no sudo: sshd grants it forwarding onto the nine workstation ports and nothing else; the host key is remembered in the kit on first connect and must match afterwards. See the note in `config/sshd-setup.sh` on why keys are not required for a pilot.

---

If nobody is to install anything at all, see [the demo stand](demo-stand.md), which carries the platform inside the container instead.
