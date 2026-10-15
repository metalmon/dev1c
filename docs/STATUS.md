# Working state

A ledger for picking the work up mid-stream: what is done, what is verified by
running it rather than by reasoning, and what is next. Updated as things change;
the design rationale lives in `docs/superpowers/specs/`.

Last updated: 2026-10-15.

## Where it runs

| | |
|---|---|
| repository | https://github.com/metalmon/dev1c (public) |
| image | built in CI, artifact `dev1c-image` (~1328 MB) |
| llama.cpp base | `ghcr.io/metalmon/dev1c-llama:b11606`, built from `Dockerfile.llama` |
| local container | `dev1c`, ports 3390 RDP / 3391 Gitea / 3392 model / 3393 SSH |
| workstations | `dev1`, `dev2`, `dev3` — password equals login |
| tunnel account | `dev` / `dev`, port forwarding only, no shell |
| test base | Бухгалтерия 3.0.190.11, file base `C:\Users\bucher\Documents\InfoBase`, platform 8.3.27.1786 |
| its HTTP service | Apache 2.4 (`C:\Apache24`), publication `/InfoBase`, service user `mcp` / `mcp` |
| mcp-1c | upstream `feenlace/mcp-1c` v1.22.3 in the image; the fork `D:\1c\projects\mcp-1c` is 340 commits behind it |

Secrets live in `.env` (git-ignored). Currently pointed at a remote model, with
the local llama.cpp switched off — no GPU worth the name on this machine yet:

```
CUSTOM_API_URL, CUSTOM_API_KEY, CUSTOM_MODEL, LLAMA_ENABLED=0
MCP_1C_BASE_URL, MCP_1C_USER, MCP_1C_PASSWORD      # the live base, see below
```

## Verified by running it

- **Image builds in CI** and passes its own checks: services start, the model
  name reaches the editor settings for both providers, `oscript` runs Cyrillic
  code, skills are intact, no key in the image environment.
- **Local model on a GPU.** Qwen3.5-4B on a 3060 Ti: 85 tokens/s, answers
  correctly. The CUDA runtime had to be in `/opt/llama` itself — unpacked into a
  subdirectory it is invisible to `ld.so`, and llama.cpp silently falls back to
  the CPU at 4 tokens/s.
- **Remote model.** Provider `custom`, key and address from `.env`, substituted
  into the settings at start.
- **Reverse SSH tunnel.** The host forwards a port into the container; both root
  and `dev1` reach it. This is the scheme for talking to the 1C platform.
- **Payload: a live base through `mcp-1c`.** The extension v1.22.3 is installed
  in the test base, the base is published in Apache, and `mcp-1c` running
  inside the container as `dev1` (stdio, the way Zed starts it) listed its nine
  tools, returned the configuration info and ran a query against
  `Справочник.Организации`. Checked over `host.docker.internal:8081`, i.e. the
  same path minus the tunnel leg, which is verified separately above. Nothing
  runs on the host beyond the platform's own web publication.
- **Skills:** 40, flat, valid frontmatter, `cf-init` produces valid XML with
  Cyrillic intact.

## Not verified

- **The whole chain in one go:** `.env` → `/etc/environment` → Zed → `mcp-1c`
  → tunnel → Apache. Every link is verified on its own; the image with the
  wiring (`MCP_1C_*` pass-through, the `1c` context server) has not been built
  and started yet. Needs a CI build, then an RDP session with the tunnel up.
- **`vanessa-runner` and the configurator over the tunnel.** Reading the base is
  solved; building, loading and running tests on the host is not. `vrunner-mcp`
  (vanessa-runner 3.0, stdio) is the candidate, but it needs something on the
  host that answers a TCP port with a process — the SSH client alone cannot do
  that.
- **ERP-scale configuration.** Debugging happens on the Бухгалтерия test base
  (`C:\Users\bucher\Documents\InfoBase`, platform 8.3.27.1786 at
  `C:\Program Files (x86)\1cv8`). Anything may be done to it.
- **The desktop itself** — theme, layout switching, `.os` highlighting, Thunar
  bookmarks. Needs eyes in an RDP session.
- **PRISM runs.** The benchmark is seeded into Gitea; nothing has been run yet.

## Known traps, each cost a build or an evening

- `/home` is a volume: it hides what the image put there. An account added in a
  later image has no home until the entrypoint creates it, and a root-owned home
  makes sshd refuse keys without saying why.
- Settings from a new image do not reach existing homes by themselves — the
  entrypoint refreshes them, tracked by a stamp of the shipped version.
- `/etc/skel` is never substituted into: that would burn the placeholders and
  make the environment impossible to repoint at another provider.
- `nvprune` only trims static archives. A finished `.so` cannot be cut, which is
  why rebuilding against fewer architectures saved almost nothing until cuBLAS
  was linked statically.
- `GGML_STATIC=ON` links everything statically and breaks the build outright;
  the one line in `ggml/src/ggml-cuda/CMakeLists.txt` is patched instead.
- Debian 13 rejects NVIDIA's SHA1-signed repository key, so the llama.cpp build
  stage runs on Debian 12.
- Libraries must not be picked by name: that left `libmtmd` behind and the
  server would not start. The build now runs `llama-server --version` to prove
  it works.
- `llama.cpp` divides `--ctx-size` between slots. With `--parallel 3` each
  workstation got a third, and this environment's prompt did not fit — the agent
  answered nothing, and only the server's own log said why.
- Windows `ssh.exe` refuses to sign with a key whose ACLs it dislikes
  (`we did not send a packet, disable method`). Not a problem for the pilot,
  where the tunnel uses a password.
- The launch script recreates the container from the image, so anything placed
  inside with `docker cp` is gone on the next start.
- `mcp-1c --install` drives the configurator and needs the base user even on a
  file base: without `--db-user` it dies with «Пользователь ИБ не
  идентифицирован».
- Port 80 on this machine is taken by another project (a web app answers
  there), so the Apache service does not start; and starting the service needs
  admin rights this account lacks. For checks Apache runs by hand on 8081 from
  a copied config: `httpd.exe -f <conf with Listen 8081> -d C:/Apache24`.
- `mcp-1c` defaults to `http://localhost:8080/hs/mcp-1c`, which inside the
  container is llama.cpp. Without `MCP_1C_BASE_URL` the server starts, lists
  its tools and fails on every call — set the variable.
- Git Bash rewrites `/tmp/...` arguments into Windows paths before `docker
  exec` sees them; prefix the command with `MSYS_NO_PATHCONV=1`.

## Next

1. **Build and start the image with the `1c` server:** CI build, `.env` with
   `MCP_1C_*`, tunnel `-R 7777:localhost:80` (or 8081 here), then in an RDP
   session ask the agent for the configuration info. This closes the chain.
2. **Skills on top of `mcp-1c`:** `1c-help-mcp` and `bsp-patterns` were written
   for another MCP server (`helpsearch`, `ssl_search`…) and now name the real
   tools; the rest of the kit never used MCP. Try them on real tasks.
3. **Host kit for the pilot:** `pack.sh` / `pack.ps1` now download the Windows
   `mcp-1c.exe` and the `.cfe` of the pinned release into `dist/host/` and
   `SETUP.md` tells the customer what to do with them. Not run yet — the next
   release build will show whether the download and the guide hold up.
4. **The fork `metalmon/mcp-1c`:** 14 own commits (business tools for
   Бухгалтерия, HTTP transport) on top of v1.6.0; upstream is at v1.22.3 and the
   merge conflicts in 10 files, mostly `server.go`, `main.go` and the extension
   module. The image does not need the fork; merge it when the business tools
   are needed again. A dry run lives in a scratch worktree on branch
   `scratch/merge-upstream`.
5. **`vanessa-runner` over the tunnel**, then **PRISM** on real tasks, then
   shorten skill descriptions (17 KB per request), then **ERP**.

## Commands

```sh
./dev1c.sh | .\dev1c.ps1          # start, stop, remove, status, logs
./pack.sh  | .\pack.ps1           # build a release into dist/

gh run list --limit 5             # CI
gh run download <id> -n dev1c-image -D <dir>
docker load -i <dir>/dev1c-image.tar.gz

# tunnel from the machine with the platform: its web server into the container
ssh -p 3393 -N -R 7777:localhost:80 dev@<host>

# the base's HTTP service, on the host and from inside the container
curl -u mcp:mcp http://localhost:8081/InfoBase/hs/mcp-1c/version
docker exec -e MCP_1C_BASE_URL=http://host.docker.internal:8081/InfoBase/hs/mcp-1c \
  -e MCP_1C_USER=mcp -e MCP_1C_PASSWORD=mcp -u dev1 dev1c mcp-1c --version

# extension into a base (Windows, drives the configurator)
mcp-1c.exe --install "C:\Users\bucher\Documents\InfoBase" --db-user mcp --db-password mcp
```
