# 1C Development Environment — OktaPlus

A Docker image with a desktop: connect over RDP, get Zed with a preconfigured AI
agent, a language server and an MCP server for 1C. The 1C platform itself is
**not** included — the configurator and `vanessa-runner` stay on the developer's
machine, reachable through a reverse SSH tunnel, so an attested workstation needs
no changes.

The environment inside is Russian: that is the audience. Everything around it —
these scripts, comments and docs — is English.

## Quick start

Docker or Podman required. The kit `dev1c-kit/` holds the installer, the
image and the parameters, nothing else is read:

```
dev1c-setup install        # loads the image, starts the environment, checks it
dev1c-setup status         # what is done, nothing changed
dev1c-setup check          # facts; report in last-check.txt
dev1c-setup change port=4000
dev1c-setup clean --stack  # stop; --data drops the volumes too
dev1c-setup verify         # the kit against its checksums
```

Without arguments the program prints the state (the window comes later). Every
command takes `--dry-run`: it prints the plan and touches nothing. Parameters
live in `dev1c.toml` next to the program, each field explained in place. No
Compose: plain `docker run` moves to Podman or anything else.

A model file, if any, goes into `model/` of the kit and is named in
`[model] file`; a remote OpenAI-compatible server goes into `[model] url`,
`key`, `name` (the URL with its path, without the trailing `/chat/completions`:
the editor appends that itself). Nothing about 1C bases is configured here:
that lives with each developer in `1c-connect` (see "Reaching the 1C
platform").

## Three workstations in one container

Three logins — `dev1`, `dev2`, `dev3`, each password equal to its login. Every
one gets its own home: Zed settings, analyzer cache, working files and keyring
never overlap.

Shared services, one instance for everyone:

- **Gitea** — git server; accounts for `dev1`/`dev2`/`dev3` are created on first
  start
- **llama.cpp** — model server

The model occupies the whole GPU memory, so there is physically nowhere to keep a
copy per person — hence one container instead of three. Both services start with
the container, before anyone connects.

The trade-off is accepted knowingly: the `bsl-analyzer` instances share cores and
memory without isolation, so on an ERP-sized configuration one person's indexing
will be noticeable to the others. Each cache lives in its owner's home, so
reindexing does not repeat; `--mode broker` can later serve all of them from one
warm process.

## Ports

Four consecutive ports from 3390: RDP, Gitea, model server, SSH for the reverse
tunnel. A block rather than scattered numbers — easier to name to people and to
open with a single firewall rule.

| | Port |
|---|---|
| Workstations (RDP) | 3390 |
| Repositories (Gitea) | 3391 |
| Model server | 3392 |
| SSH for the tunnel | 3393 |

Taken already? `dev1c-setup change port=4000`, or edit `dev1c.toml`.

| `dev1c.toml` | Default | Purpose |
|---|---|---|
| `[server] port` | `3390` | first port of the block |
| `[server] memory` | `24g` | container memory limit |
| `[server] gpu` | `true` | `false` runs without a GPU (model on CPU) |
| `[server] engine` | `docker` | or `podman` |

Inside the container three more blocks are reserved per workstation, for the
tunnel from the developer's machine: `777N` (mcp-1c), `778N` (vrunner-mcp),
`779N` (web client of the base), `N` being the workstation number.

## The model

The model is not baked into the image: it weighs more than the image itself and
changes independently. Put the `.gguf` into `model/` of the kit, name it in
`[model] file`, and `dev1c-setup install` copies it into the named volume once;
the entrypoint finds it there and points the editor at it.

A volume, not a host directory: on Windows antivirus software keeps files in
bind-mounted directories open, and reads from inside the container fail
mid-stream with I/O errors — while the model is read in full at startup.

## Reaching the 1C platform

No platform inside the image: the configurator, `1cv8` and `vanessa-runner` stay
on the developer's machine. The environment reaches them through a **reverse SSH
tunnel** — the server side is ours, inside the container (port 3393), and the
machine with the platform only connects as a client.

The direction is inverted on purpose. In an air-gapped network workstations are
attested, and installing server services on them requires sign-off from the
security team. An SSH client, on the other hand, ships with Windows 10/11 since
1803, and running it is ordinary use of the system rather than a change to an
attested workstation.

Everything about a developer's base lives on the developer's machine, in the
`1c-connect/` kit: `1c-connect.exe`, its `1c-connect.toml`, `mcp-1c` and its
extension. On the machine with 1C, once:

1. Copy the kit anywhere (a flash drive is fine) and fill `1c-connect.toml`:
   the environment's address and SSH port (printed by `dev1c-setup install`),
   your workstation (`dev1`…`dev3`), the base, its user, the address of its
   web publication.
2. `1c-connect install` — puts the `mcp-1c` extension into the base through
   the configurator; then publish the base on the web server (configurator →
   Administration → Publish on web server, "publish extension HTTP services
   by default" ticked). `1c-connect check` tells what is still missing.
3. `1c-connect connect` — raises the tunnel with its own SSH client and keeps
   it while the window is open. `Ctrl+C` closes it; nothing stays behind.

Inside the container the agent sees the base as a plain TCP port: `777N` for
workstation `dev N` (a fresh `mcp-1c` on the base per connection), `778N` for
`vrunner-mcp` (next stage), `779N` for the web client of the same base in
Firefox: `http://localhost:779N/<publication>/`. Zed is already pointed at them
(`nc -N localhost 777N`); the server side holds no parameter about any base.

Account `dev`, password `dev` — one shared pair, because the tunnel is a shared
function rather than anybody's personal one. That account has no shell and no
sudo: sshd grants it forwarding onto the nine workstation ports and nothing
else; the host key is remembered in the kit on first connect and must match
afterwards. See the note in `config/sshd-setup.sh` on why keys are not required
for a pilot.

## The demo stand: the platform inside the container

For a customer who will install nothing on their own machines there is a second
image, `oktaplus/dev1c-1c`, built by `Dockerfile.1c` on top of `oktaplus/dev1c`:
the **training** 1C:Enterprise 8.5 (file bases only, one Enterprise session per
base, no server), a demo base per workstation, `mcp-1c` and `vanessa-runner`
inside. The agent sees the same ports as with the tunnel — `777N` (mcp-1c),
`778N` (vrunner-mcp) — served locally by `config/1c/stand.sh`, so Zed, the
skills and the rules do not change; the Configurator is in the application
menu, and `1cv8` is on the `PATH`. The base of workstation `devN` is
`~/base`, a copy of the template base made on first use; the project directory for
vrunner is `~/project`.

The platform and the template base are **not in the image**. They come in two
named volumes, `dev1c-platform` (`/opt/1cv8t`) and `dev1c-demo`
(`/opt/1c-demo`), filled once from a payload tar. They follow 1C's releases,
not ours: baked in, every change to `stand.sh` meant moving 11 GB of build
context, image and kit, and a customer who already had the platform downloaded
it again. The image is about 4 GB, the payload about 4 GB more, and the
payload survives image updates. Without the volumes the container still gives
the desktop, Zed and Gitea, and `1c-stand start` says in one line what is
missing.

The installer is 1.7 GB and comes from the 1C site through a browser form, so
both are built where the file is, not in CI. `provision.sh` is the whole host
side, with fixed names and no discovery:

```
mkdir platform                         # git- and docker-ignored
# put setup-training-8.5.1.1150-x86_64.run there (from training_8_5_1_1150_LinuxRun.tar.gz),
# unf-ready.dt and DoNotCopy.txt, the demo base of the stand (see below)
./provision.sh build                   # the stand image and the payload image
./provision.sh export                  # -> dist/dev1c-1c-payload.tar.gz, outside the build context
./provision.sh import                  # that tar -> the two volumes
./provision.sh run -e CUSTOM_API_URL=… -e CUSTOM_API_KEY=… -e CUSTOM_MODEL=…
./provision.sh status                  # what the volumes hold
docker exec dev1c test-stand           # the check of a running stand
```

On another machine only the last three lines happen, carrying two files: the
image (`docker save oktaplus/dev1c-1c:latest | gzip`) and
`dev1c-1c-payload.tar.gz`. There is no packed stand kit yet — `pack.sh` builds
the kit of the development environment, not of the stand. The payload is
rebuilt only for a new platform or a new demo configuration; `payload.conf`
inside it records which versions it was built from, and `test-stand` prints
that line first.

`DEV1C_NAME` renames the container and all four volumes together and
`DEV1C_PORT_BASE` moves the three ports, so a second stand can be raised
beside a live one without touching it.

The demo base is the training УНФ 3.0.13.251 without data: the kit's demo base
with data has users with passwords, and the training licence admits only empty
ones. `unf-ready.dt` is that empty base with the configuration's welcome wizard
already done. The wizard («Добро пожаловать!», three steps, a required name)
cannot be skipped, and finishing it creates an infobase user — so if the
customer meets it, the agent's connection breaks in the middle of the demo.
Done once at build time instead, and the name of the user it makes
(`DEMO_DB_USER`, «Администратор», empty password — the only kind the licence
allows) is carried by the publication, by the base list of the starter and by
vrunner. To make the file: build a base from `unf-empty.dt` (1cv8new.dt of the
kit), open a copy of it, walk the three steps giving that name and no password,
then `1cv8 DESIGNER /F <base> /NАдминистратор /DumpIB unf-ready.dt`. Building
without a wizard-done template needs both arguments:
`--build-arg DEMO_TEMPLATE=unf-empty.dt --build-arg DEMO_DB_USER=`.

`platform/DoNotCopy.txt` goes with that template. A BSP configuration keeps
the identifier of its file base in that file, and a base restored from a `.dt`
has no such file, so the first person in is asked whether the base was moved or
is a copy, with external resources locked and `[КОПИЯ]` in the title until they
answer. Make it once: restore `unf-ready.dt` into a base, open it, answer
«Информационная база перемещена», and take the `DoNotCopy.txt` the
configuration writes. The build drops it next to the template base and every
workstation copy inherits it. Regenerate it with the template; a stale
identifier only brings the question back.

The build itself checks the platform (ldd, a base created under Xvfb), the
mcp-1c HTTP service on the demo base through a throwaway Apache, and
vrunner-mcp's tool list. While a stand holds `777N`/`778N`, `1c-connect` from a
real machine cannot bind them: a workstation is either demo or real.

## Building a release

CI builds the image and the installers and uploads `dev1c-kit` with both kits:
`dev1c-kit/` (the installer for Windows and Linux, `dev1c.toml`, the image,
checksums) and `1c-connect/` (the Windows `mcp-1c` and its `.cfe` for the
machine with the platform). Locally, `pack.sh <image.tar.gz> <bins>` assembles
the same from already built parts; nothing is compiled on the laptop.

The model key never reaches the image: it is passed as an environment variable at
run time, so every build is clean by construction rather than "remember to build
without the key".

Building needs about 4 GB of RAM and 10 GB of disk; the build context is 2.4 MB
and everything else is downloaded. No GPU required — the CUDA runtime is only
unpacked as files.

## What is inside

**Workstation**
- Debian 13, XFCE: Materia recoloured into the 1C (Такси) palette, light by default
  with a dark twin, Russian interface, us/ru layout on Alt+Shift
- Zed with extensions (bsl, xml, html, csv, ini, sql, dockerfile, toml, powershell)
  and the 1C Light / 1C Dark themes on the same palette; agent, LSP and MCP
  configured out of the box
- `bsl-analyzer` — language server and MCP server in one binary, works offline
- `nc` endpoints in Zed for `mcp-1c` and `vrunner-mcp` on the developer's machine, over the tunnel
- Firefox ESR (the 1C web client supports it officially), SSH client, OktaPlus branding
- Windows drives are forwarded over RDP and appear in `~/thinclient_drives`

**Open projects from the home directory, not from a forwarded drive.** FUSE has
no inotify, so Zed falls back to polling: it walks every project file every two
seconds over the RDP channel. On three thousand files that saturates eight cores
and the agent stops answering — the request never leaves, because no CPU time is
left. On an ERP-sized dump the environment will stall for certain. Forwarded
drives are for exchanging files: copy the dump into `~/proj` and work there.

**Tooling**
- PowerShell 7, OneScript 2.2, Python 3 with libraries for office formats
- git, git-lfs, ripgrep, fd, xmllint, xmlstarlet, jq, mc, archivers

**Agent**
- 40 skills: superpowers (process) plus selected 1C ones, including `bsp` with 24
  reference documents on the standard subsystems library
- Only skill names and descriptions reach the model's context (~17 KB for all 40);
  the body of a skill is read through the `skill` tool once chosen
- Skills are written in Russian on purpose: they deal with 1C terminology and the
  code they describe is Russian

**Gitea** — the git server inside the image. A "Repositories" shortcut sits on
the desktop and in the panel; credentials match the system user. On first start
[PRISM](https://github.com/genlab-1c/prism) is seeded into it — an open benchmark
of LLMs on 1C tasks, a good starting point and a test bed for the skills.

The image needs no internet at run time: extensions and the analyzer are frozen
at build time. Verified with networking disabled.
