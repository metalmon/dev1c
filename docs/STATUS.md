# Working state

A ledger for picking the work up mid-stream: what is done, what is verified by
running it rather than by reasoning, and what is next. Updated as things change;
the design rationale lives in `docs/superpowers/specs/`.

Last updated: 2026-10-15.

## Where it runs

| | |
|---|---|
| repository | https://github.com/metalmon/dev1c (public) |
| kit | built in CI, artifact `dev1c-kit`: `dev1c-kit/` (installer, `dev1c.toml`, image ~1328 MB, checksums) and `1c-connect/` (mcp-1c for the 1C machine) |
| llama.cpp base | `ghcr.io/metalmon/dev1c-llama:b11606`, built from `Dockerfile.llama` |
| local container | `dev1c`, ports 3390 RDP / 3391 Gitea / 3392 model / 3393 SSH |
| workstations | `dev1`, `dev2`, `dev3` — password equals login; tunnel ports 777N / 778N / 779N |
| tunnel account | `dev` / `dev`, forwarding onto the nine workstation ports only, no shell |
| test base | Бухгалтерия 3.0.190.11, file base `C:\Users\bucher\Documents\InfoBase`, platform 8.3.27.1786 |
| its HTTP service | Apache 2.4 (`C:\Apache24`), publication `/InfoBase`, service user `mcp` / `mcp` |
| mcp-1c | upstream `feenlace/mcp-1c` v1.22.3 in the developer's kit (not in the image); the fork `D:\1c\projects\mcp-1c` is 340 commits behind it, merge in branch `merge-upstream` |
| installers | `setup/` — Cargo workspace `common` + `dev1c-setup` + `1c-connect`; each is one binary: window without arguments (Tauri 2, `gui` feature on by default), CLI with arguments; `1c-connect` lives in the tray |

Parameters live in `dev1c.toml` of the kit (never in the repository). On this
machine the kit is `dist/dev1c-kit`, pointed at a remote model with the local
llama.cpp switched off — no GPU worth the name here yet. The old `.env` from the
script days is obsolete; the scripts are gone.

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
- **Cloud endpoint measured** (2026-10-16, gateway over vLLM, Qwen3.6-35B-A3B-AWQ):
  window 65 536, prefill ≈ 22 000 tok/s at 53k tokens (first token in 2.4 s),
  generation 140–160 tok/s; the gateway preset thinks before answering, so
  reasoning eats output tokens. Numbers and the 5090 estimate are in the spec
  («Хватит ли окна на кодинг»).
- **`1c-connect`, the whole agent path** (2026-10-16, this machine): with
  `1c-connect connect` running against the local container, from inside the
  container `nc -z localhost 7771` and `7791` both open, the web client of the
  base answered HTTP 200 through `localhost:7791/InfoBase/`, and an MCP
  session through `nc -N localhost 7771` listed mcp-1c's nine tools and
  returned the configuration info of the Бухгалтерия base. `1c-connect check`:
  6 facts, 0 failed. `russh` 0.64 talks to the container's OpenSSH (ed25519,
  password), remembers the host key, refuses a changed one.
- **`dev1c-setup`.** In CI the Linux binary installed the image on a clean
  runner and passed its 14 checks; the kit from the artifact ran on this
  Windows machine: `dev1c-setup.exe status` saw the local container as done
  on all four operations. Its `verify` complained about the edited
  `dev1c.toml` — the review finding, fixed the same day (parameter files are
  no longer in the checksums).

- **Both windows** (2026-10-16, this machine, debug builds; restyled the same
  day to Windows 11 / Fluent after the first look was rejected as unprofessional):
  `1c-connect.exe` without arguments puts the Zed icon with the grey status dot
  into the tray and opens the flyout at the bottom-right of the work area
  (hides on blur, left click on the icon brings it back); «Проверить» lists the seven facts and writes
  `last-check.txt`; «Подключить» with the sample address refuses in the log;
  the form saves `1c-connect.toml` through `render`. `dev1c-setup.exe` shows
  the real state of the container here (image, volumes, container: done;
  model: blocked) and the access addresses. `cargo check` with Tauri fits
  this laptop at 3 jobs (4.5 min, under 4 GB); release builds stay in CI.

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
- The CRLF normalisation in the Dockerfile (`sed -i 's/\r$//'` over
  `/usr/local/bin`) rewrote the bytes of the `mcp-1c` ELF; the binary passed
  its own check in the build layer and died with SIGILL (exit 132) in the
  running container. Now text files only.
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
- A GitHub runner (16 GB) runs out of memory around `docker build`'s
  "exporting layers" of this 3 GB image, and dockerd keeps most of it
  afterwards: the runner then SIGTERMs whatever runs next (exit 143, job
  shown as "cancelled", no notice anywhere). It hit the build itself, the
  installer's `check` and `rustup` on different days' luck; the diagnostics
  step showed 215 MB available and the runner's 3 GB swap full. `build.yml`
  now adds an 8 GB swap file before the build and caps the container at 6g
  on the runner.
- The `dev1c-kit` artifact is a zip and drops the mode bits: the Linux
  `dev1c-setup` arrives as `-rw-r--r--` and needs `chmod +x`. Pack the kits as
  a tar inside the artifact, or say so in the README, before a Linux customer
  meets it.

- Tauri fires `Focused(false)` on the window when focus moves *into* its own
  WebView2 child right after the first show: a flyout that hides on blur
  flashed and vanished at launch. The handler now waits 250 ms and hides only
  if `GetForegroundWindow` is neither our window nor its child.
- Author CSS beats the `hidden` attribute: a `.badge { display: inline-block }`
  rule kept a hidden badge visible. The stylesheet carries
  `[hidden] { display: none !important }` for that reason.
- A flyout that hides on blur cannot be screenshotted by a script that then
  steals focus: the driver re-shows it with `ShowWindow` and an Alt tap before
  `SetForegroundWindow` (`drive.py` in the session scratchpad, not in the repo).
- Dependabot flags `glib` 0.18 (advisory for 0.15–0.20) in `setup/Cargo.lock`:
  it is Tauri's Linux GTK dependency, compiled only in a Linux build with the
  window, which CI never does. Goes away when Tauri bumps gtk-rs.
- The window ships only in the Windows build: Linux CI builds pass
  `--no-default-features` (no webkit2gtk on the runner). A release build with
  the window is `windows_subsystem = "windows"`: commands get their console
  back through `common::console::attach`; without it `status` prints nowhere.
- Tauri's `tauri icon` is not used: `icons/icon.ico` and the 64 px `tray.png`
  are made once with Pillow (the SVG mark via headless Edge); the status dot
  is drawn at run time over `tray.png`.

## Next

Spec `2026-10-16-setup-binaries-design.md`, plan
`docs/superpowers/plans/2026-10-16-dev1c-setup.md`. Stages 1 and 2 done on
2026-10-16: the image carries the `nc` endpoints and `PermitListen`, `mcp-1c`
and `MCP_1C_*` are out of it; `dev1c-setup` (CLI) installs the image, checks
it and writes `last-check.txt`; CI brings the image up through it and packs
`dev1c-kit`. Known limitation: `dev1c-setup change` rewrites `dev1c.toml`
without the comments; the commented sample stays as `dev1c-setup example`.

1. **`1c-connect`** (stage 3) — done 2026-10-16, plan
   `docs/superpowers/plans/2026-10-16-1c-connect.md`: `install`/`uninstall`
   of the extension, the embedded tunnel onto 777N/779N with reconnect, the
   bridge to `mcp-1c.exe`, `check` with `last-check.txt`; proved here end to
   end and in CI (the live tunnel test against the installed image). Zed's
   per-server `timeout` is set in the image: 300 s for `1c`, 3600 s for
   `vrunner` (Zed's default is 60 s per MCP call — too short for a dump).
   Long operations are better served as jobs (start → id → poll); decide when
   `vrunner-mcp` arrives. Final review (fresh agent) found and the fix pass
   closed: reconnect gave up on a port sshd still held after a drop; local
   faults were retried forever; the sample `1c-connect.toml` was dialled
   forever; a dead web server behind 779N was silent. Deferred minors:
   `find_1cv8` sorts versions as strings; the «was/became» fingerprint shows
   two formats; `check` waits 60 s on a filtered port; `pack.sh` packs a kit
   without `1c-connect.exe` silently; `Cfg::publication`/`render` unused by
   any command; `mcp-1c` exit 0 and Warn events are not journaled; the
   `httpport` publication mode from the spec is not implemented yet.
2. **Windows of both programs** (stage 4) — done 2026-10-16, plan
   `docs/superpowers/plans/2026-10-16-windows-and-tray.md`; tried by hand here
   in debug builds (CI green for both stage commits, 99c3c77 and 4f08d07); the
   release exe from the CI kit not yet run by a person. Not tried:
   the tray menu by click, «Подключить» against a live environment from the
   window (the CLI path is the same `session::start`). Left for later: the
   tray menu does not grey out «Подключить» while connected (a second click
   is a no-op); the window asks no questions (`AlwaysDefault`), so the
   data-wiping clean is guarded only by the typed container name.
3. **Skills on top of `mcp-1c`:** `1c-help-mcp` and `bsp-patterns` now name
   the real tools; try them on real tasks once `1c-connect` is up.
4. **The fork `metalmon/mcp-1c`:** merged with upstream v1.22.3 in branch
   `merge-upstream` (worktree `D:\1c\projects\mcp-1c-upstream-merge`), tests
   green in its CI; fast-forward `main` when the business tools are needed.
5. **Local model as a sibling container, the volt-llama way** (decided
   2026-10-16, spec section «Локальная модель»): CUDA and llama.cpp leave the
   `dev1c` image; `dev1c-setup` gets an operation `llama` that runs
   `dev1c-llama` (own slim image, `--fit on --fit-ctx 65536`, three slots,
   unified cache, GPU and RAM measured first) and then syncs the numbers into
   Zed: `max_tokens = window/agents − 8192 − margin` (`agents` defaults to 1:
   the window is not split three ways, eviction under three busy agents is a
   recompute, not a failure; target ≥ 128k per agent, YaRN beyond 262k as the
   measured reserve), `max_output_tokens`, `auto_compact` threshold, read back
   from `/props`; `check` states the three numbers. Zed
   owns the sliding window (`agent.auto_compact`, present in the image's
   build), llama owns capacity, the installer keeps them in step. Own plan
   after `1c-connect`. Today the model is served by the other project's
   `volt-llama` container from the same `dev1c-models` volume, and `dev1c`
   points at it as remote.
6. **`vrunner-mcp` in the developer's kit** (stage 6): portable OneScript plus
   vanessa-runner, port 778N; check first that `vrunner-mcp` runs from a
   portable OneScript on Windows. This is what makes PRISM category B possible.
7. **PRISM** on real tasks (category A already runnable inside), then shorten
   skill descriptions (17 KB per request), then **ERP**.

## Commands

```sh
dev1c-setup status|install|change k=v|check|clean --stack|--data|verify   # in the kit
cd setup && cargo test --workspace     # unit tests of the installers (light)
./pack.sh <image.tar.gz> <bins>        # assemble the two kits from built parts

gh run list --limit 5                  # CI: setup (binaries) + build (image, kit)
gh run download <id> -n dev1c-kit -D <dir>

# tunnel from the machine with the platform, by hand until 1c-connect exists
ssh -p 3393 -N -R 7771:localhost:7771 -R 7791:localhost:80 dev@<host>

# the base's HTTP service on this machine
curl -u mcp:mcp http://localhost:8081/InfoBase/hs/mcp-1c/version

# extension into a base (Windows, drives the configurator)
mcp-1c.exe --install "C:\Users\bucher\Documents\InfoBase" --db-user mcp --db-password mcp
```
