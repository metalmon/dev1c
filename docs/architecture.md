# Architecture

Why the environment has the shape it has: one container for three people, a block of four ports, the model outside the image, and what is installed inside. Each section says what was traded for what.

## Three workstations in one container

Three logins — `dev1`, `dev2`, `dev3`, each password equal to its login. Every one gets its own home: Zed settings, analyzer cache, working files and keyring never overlap.

Shared services, one instance for everyone:

- **Gitea** — git server; accounts for `dev1`/`dev2`/`dev3` are created on first start
- **llama.cpp** — model server

The model occupies the whole GPU memory, so there is physically nowhere to keep a copy per person — hence one container instead of three. Both services start with the container, before anyone connects.

The trade-off is accepted knowingly: the `bsl-analyzer` instances share cores and memory without isolation, so on an ERP-sized configuration one person's indexing will be noticeable to the others. Each cache lives in its owner's home, so reindexing does not repeat; `--mode broker` can later serve all of them from one warm process.

## Ports

Four consecutive ports from 3390: RDP, Gitea, model server, SSH for the reverse tunnel. A block rather than scattered numbers — easier to name to people and to open with a single firewall rule.

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

Inside the container three more blocks are reserved per workstation, for the tunnel from the developer's machine: `777N` (mcp-1c), `778N` (vrunner-mcp), `779N` (web client of the base), `N` being the workstation number.

## The model

The model is not baked into the image: it weighs more than the image itself and changes independently. Put the `.gguf` into `model/` of the kit, name it in `[model] file`, and `dev1c-setup install` copies it into the named volume once; the entrypoint finds it there and points the editor at it.

A volume, not a host directory: on Windows antivirus software keeps files in bind-mounted directories open, and reads from inside the container fail mid-stream with I/O errors — while the model is read in full at startup.

## What is inside

**Workstation**
- Debian 13, XFCE: Materia recoloured into the 1C (Такси) palette, light by default with a dark twin, Russian interface, us/ru layout on Alt+Shift
- Zed with extensions (bsl, xml, html, csv, ini, sql, dockerfile, toml, powershell) and the 1C Light / 1C Dark themes on the same palette; agent, LSP and MCP configured out of the box
- `bsl-analyzer` — language server and MCP server in one binary, works offline
- `nc` endpoints in Zed for `mcp-1c` and `vrunner-mcp` on the developer's machine, over the tunnel
- Firefox ESR (the 1C web client supports it officially), SSH client, OktaPlus branding
- Windows drives are forwarded over RDP and appear in `~/thinclient_drives`

**Open projects from the home directory, not from a forwarded drive.** FUSE has no inotify, so Zed falls back to polling: it walks every project file every two seconds over the RDP channel. On three thousand files that saturates eight cores and the agent stops answering — the request never leaves, because no CPU time is left. On an ERP-sized dump the environment will stall for certain. Forwarded drives are for exchanging files: copy the dump into `~/proj` and work there.

**Tooling**
- PowerShell 7, OneScript 2.2, Python 3 with libraries for office formats
- git, git-lfs, ripgrep, fd, xmllint, xmlstarlet, jq, mc, archivers

**Agent**
- 40 skills: superpowers (process) plus selected 1C ones, including `bsp` with 24 reference documents on the standard subsystems library
- Only skill names and descriptions reach the model's context (~17 KB for all 40); the body of a skill is read through the `skill` tool once chosen
- Skills are written in Russian on purpose: they deal with 1C terminology and the code they describe is Russian

**Gitea** — the git server inside the image. A "Repositories" shortcut sits on the desktop and in the panel; credentials match the system user. On first start [PRISM](https://github.com/genlab-1c/prism) is seeded into it — an open benchmark of LLMs on 1C tasks, a good starting point and a test bed for the skills.

The image needs no internet at run time: extensions and the analyzer are frozen at build time. Verified with networking disabled.

---

Next: [Reaching the 1C platform](1c-platform.md) — how a developer's base becomes a port the agent can talk to.
