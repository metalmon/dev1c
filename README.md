# 1C Development Environment — OktaPlus

A Docker image with a desktop: connect over RDP, get Zed with a preconfigured AI
agent, a language server and an MCP server for 1C. The 1C platform itself is
**not** included — the configurator and `vanessa-runner` stay on the developer's
machine, reachable through a reverse SSH tunnel, so an attested workstation needs
no changes.

The environment inside is Russian: that is the audience. Everything around it —
these scripts, comments and docs — is English.

## Quick start

Docker or Podman required. No build needed: the image ships as a file.

**1. Load the image**

```sh
docker load -i dev1c-image.tar.gz
```

**2. Start it**

```sh
./dev1c.sh                  # Linux host
.\dev1c.ps1                 # Windows host
```

The script finds the engine, starts the container and prints the addresses.
No Compose: plain `docker run` moves to Podman or anything else without
rewriting the wrapper.

**3. Add a model file** — any `.gguf`, served by llama.cpp:

```sh
docker cp model.gguf dev1c:/opt/models/
docker restart dev1c
```

The environment works without it, but the agent stays silent.

| Action | Linux | Windows |
|---|---|---|
| start | `./dev1c.sh` | `.\dev1c.ps1` |
| stop | `./dev1c.sh stop` | `.\dev1c.ps1 -Stop` |
| stop and drop data | `./dev1c.sh remove` | `.\dev1c.ps1 -Remove` |
| what runs inside | `./dev1c.sh status` | `.\dev1c.ps1 -Status` |
| container log | `./dev1c.sh logs` | `.\dev1c.ps1 -Logs` |

### Using a remote model instead of the local one

Only needed when the model lives on a separate server — during debugging, for
example. Copy the sample and fill in two values:

```sh
cp .env.example .env
```

```
API_KEY=your-access-key
API_URL=https://model.your-domain.com/api/v1/threads
```

Give the URL with its path but without the trailing `/chat/completions` — the
editor appends that itself. The launch script picks `.env` up automatically.

`.env` is listed in `.gitignore` and never enters the repository: a key committed
once stays in git history forever, even after the file is deleted.

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

Taken already? Pick another start: `PORT=4000 ./dev1c.sh` or
`.\dev1c.ps1 -Port 4000`.

| Variable (Linux) | Parameter (Windows) | Default | Purpose |
|---|---|---|---|
| `PORT` | `-Port` | `3390` | first port of the block |
| `MEMORY` | `-Memory` | `24g` | container memory limit |
| `NO_GPU=1` | `-NoGpu` | — | run without a GPU (model on CPU) |
| `DOCKER_SOCK=1` | `-DockerSocket` | — | expose the engine socket inside; off by default, it equals root on the host |

## The model

The model is not baked into the image: it weighs more than the image itself and
changes independently. Upload the `.gguf` into the named volume:

```sh
docker cp model.gguf dev1c:/opt/models/
docker restart dev1c
```

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

Raising the tunnel by hand, from the machine that has 1C:

```sh
ssh -p 3393 -N -R 7777:localhost:7777 dev@<environment-host>
```

Account `dev`, password `dev` — one shared pair, because the tunnel is a shared
function rather than anybody's personal one. That account has no shell and no
sudo: sshd grants it port forwarding and nothing else. See the note in
`config/sshd-setup.sh` on why keys are not required for a pilot and when to
switch to them.

`7777` is the port of whatever listens on the machine with the platform; the
same port becomes reachable as `localhost:7777` inside the container.

A client-side utility is planned: bring the tunnel up, check connectivity, show
status — one window instead of a command with four flags. The scheme has not yet
been verified on a machine with the platform installed.

## Building a release

```sh
./pack.sh                   # or .\pack.ps1
```

The model key never reaches the image: it is passed as an environment variable at
run time, so every build is clean by construction rather than "remember to build
without the key".

Building needs about 4 GB of RAM and 10 GB of disk; the build context is 2.4 MB
and everything else is downloaded. No GPU required — the CUDA runtime is only
unpacked as files.

## What is inside

**Workstation**
- Debian 13, XFCE: dark Materia theme, Russian interface, us/ru layout on Alt+Shift
- Zed with extensions (bsl, xml, html, csv, ini, sql, dockerfile, toml, powershell)
  and the macOS Classic theme; agent, LSP and MCP configured out of the box
- `bsl-analyzer` — language server and MCP server in one binary, works offline
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
