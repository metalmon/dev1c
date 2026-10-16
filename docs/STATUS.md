# Working state

A ledger for picking the work up mid-stream: what is done, what is verified by
running it rather than by reasoning, and what is next. Updated as things change;
the design rationale lives in `docs/superpowers/specs/`.

Last updated: 2026-10-15.

## Where it runs

| | |
|---|---|
| repository | https://github.com/metalmon/dev1c (public) |
| kit | built in CI, artifact `dev1c-kit`: `dev1c-kit/` (installer, `dev1c.toml`, image ~1328 MB, checksums) and `1c-connect/` (≈ 230 MB: mcp-1c, `vrunner/` = OneScript 2.2.0 + vanessa-runner 3.0.2 + add 6.8.0, `git/` = MinGit 2.56.0.2, `tools.version`) |
| llama.cpp | `ghcr.io/metalmon/dev1c-llama:b11606` (built from `Dockerfile.llama`) is no longer the base of `dev1c` — the image has no CUDA; the same build b11606 ships in the kit as `llama/bin/<os>` for the host form |
| local container | `dev1c`, ports 3390 RDP / 3391 Gitea / 3393 SSH; 3392 belongs to the model container when `serve = "docker"` |
| model server | `dev1c-setup` runs it: `serve = "host"` (default) — `llama-server` as a Windows service `dev1c-llama` via nssm in `C:\dev1c\llama` (Linux: systemd, `/opt/dev1c/llama`), `serve = "docker"` — container `<name>-llama` from `oktaplus/dev1c-llama` in the same kit tar, `serve = "remote"` — any OpenAI-compatible server. Zed reaches local forms as `http://host.docker.internal:<port>/v1` |
| workstations | `dev1`, `dev2`, `dev3` — password equals login; tunnel ports 777N / 778N / 779N |
| tunnel account | none: each workstation tunnels as itself (`dev1`/`dev1` …), sshd `Match User` gives it remote forwarding onto its own three ports only — no shell, no tty, no local forwards |
| test base | Бухгалтерия 3.0.190.11, file base `C:\Users\bucher\Documents\InfoBase`, platform 8.3.27.1786 |
| its HTTP service | Apache 2.4 (`C:\Apache24`), publication `/InfoBase`, service user `mcp` / `mcp` |
| mcp-1c | upstream `feenlace/mcp-1c` v1.22.3 in the developer's kit (not in the image); the fork `D:\1c\projects\mcp-1c` is 340 commits behind it, merge in branch `merge-upstream` |
| installers | `setup/` — Cargo workspace `common` + `dev1c-setup` + `1c-connect`; each is one binary: window without arguments (GPUI (gpui-kit 0.7.3), `gui` feature on by default), CLI with arguments; `1c-connect` lives in the tray (release `1c-connect.exe` ≈ 23.9 MB) |

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

- **Inference server, host form** (2026-10-16, this machine, 3060 Ti 8 GB with
  the Volt pilot's `volt-llama` already on the card): `install --only
  llama.bin` lays 58 files into `C:\dev1c\llama` under an admin-only ACL,
  `llama.service` registers `dev1c-llama` through nssm on port 1236 (1235 is
  `volt-llama`), `llama.answers` reads `/props` — window 76544 tokens, 3 slots,
  prefill 1726 tok/s — and writes `max_tokens = 66304` into `install.state`;
  `install --only container` recreates the environment with `CUSTOM_API_URL=
  http://host.docker.internal:1236/v1`, `CUSTOM_API_KEY`, `CUSTOM_MODEL`,
  `CUSTOM_MAX_TOKENS`; from inside the container `curl host.docker.internal:
  1236/health` with the token answers 200. `check`: видеокарта, сервер модели,
  три числа — green. `clean --stack` removes the service and the container,
  keeps the files and the model; `install --only container` brings the
  environment back. CI (run 38892442454): image without CUDA builds, both
  images in one tar, install and check with `serve = "remote"` pass,
  `test-entrypoint-ports` confirms the window numbers are substituted.

- **vrunner bridge (stage 6A, 2026-10-16):** `jsonrpc.rs` framing and routing
  on bytes (line split across reads, 8 MiB overflow passthrough, filtered
  `tools/list` with the three bridge tools on the last page, disabled tools
  answered with `isError`, cancel), `bridge::pipe_jsonrpc` against a scripted
  child (filter, platform call refused while `1cv8.exe` runs, own tool in its
  own task, cancel swallowed for ours and forwarded for foreign ids, child
  death still answers a pending `tools/list` with our tools), git tools on a
  real git with a bare origin (first push creates `main` + `dev2`, dirty tree
  refused, rebase conflict aborted and named by file, `project_status`), the
  project mutex (second caller told «занято»), Job Object (dropping it kills
  `cmd.exe` + `ping`). `pack.sh --llama none` end to end here: kit 232 MB,
  opm tree matches the manifest (996 files), `tools.version` written.
  `1c-connect install` copied the tools to `%LOCALAPPDATA%\dev1c` (`git
  version 2.56.0.windows.2`), `check` fact 4 «инструменты комплекта» green,
  and `oscript.exe lib/vanessa-runner/src/mcp.os` from that copy answers
  `initialize` (AutumnMCP) and `tools/list` with 61 tools — 10 s to the first
  reply. After the final review's fix pass, CI run 38921692754 is green end
  to end: both OSes, the image check for the per-workstation `Match` blocks,
  the live tunnel as `dev1` and `dev2`, and the Linux kit packed with the
  vrunner row.

- **vrunner project flow (stage 6B, 2026-10-18):** `host.project` on a real
  git (Cyrillic path with a space, base-directory refusal, drift by branch /
  origin / files, idempotent second init), `local::call` against a scripted
  MCP child (initialize → call → reap; a child that dies is an error, not a
  hang), `flow` export/import order through fakes (no push after a failed
  decompile, import refused while the platform runs, full-load warning
  without `ConfigDumpInfo.xml`, the way back after a failed `cf_load`, busy
  like any tool), the thirteen `check` facts, connection arguments replacing
  the agent's (case-insensitive) — 93 tests. CI run 39117216055 and later:
  the image creates the `dev1c` organisation with the three workstations as
  owners, `~/.git-credentials` 0600 per home, and a push-to-create by `dev2`
  lands in the organisation. On this machine, against the running container
  patched additively (org, credentials, `Match User dev1..3` appended, sshd
  reloaded): `1c-connect install` laid out `C:\proj\InfoBase` (`oscript
  2.2.0` answers from it), `check` 13/13 with the base user `mcp`, the window
  shows the «Проект» tab and the parameters fit without scrolling, and
  `1c-connect export` decompiled Бухгалтерия into `src` (92 k files,
  `ConfigDumpInfo.xml` present) and committed it on `dev1` with the CI-built
  `1c-connect.exe`.

## Not verified

- The push of that first export did not reach Gitea from this laptop: with
  Docker at 4.7 GB and under 1 GB free, `git pack-objects` over 92 k objects
  crawled at 4 % CPU for two hours (`pack.window=0` did not help), and every
  background attempt was killed for memory. Still to run when the machine is
  free: the push (branches `main` and `dev1` in `dev1c/InfoBase`), the edit
  in the container → `import` → incremental `cf_load`, the bridge over a live
  tunnel from Zed, and the Job Object against a real Configurator. The
  project-side git code is covered by tests on a real git with a bare origin.
- The container form (`serve = "docker"`) ran only through its unit tests and
  the CI image save: no live `docker run --gpus all` of `dev1c-llama` here
  (the card is shared with `volt-llama`).
- The new image is not deployed on this machine: the local `dev1c` still runs
  the old one, so the local `check` fails «модель в настройках Zed» and «порты
  рабочих мест» — image staleness, both pass in CI.
- A Zed agent reply through the host-form server was not tried over RDP.

- **The whole chain in one go:** `.env` → `/etc/environment` → Zed → `mcp-1c`
  → tunnel → Apache. Every link is verified on its own; the image with the
  wiring (`MCP_1C_*` pass-through, the `1c` context server) has not been built
  and started yet. Needs a CI build, then an RDP session with the tunnel up.
- **`vanessa-runner` and the configurator over the tunnel.** Reading the base is
  solved; the bridge for building, loading and tests is in (6A), the project
  flow around it (6B) is not: nothing has loaded a configuration from git yet.
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
- The settings seeding in the entrypoint re-copies `settings.json` from the
  image only when its stamp changes; substituted placeholders are gone for
  good, so a new model address or window never reached a home that already
  had the file. The stamp now covers the `CUSTOM_*` variables too (a hand
  edit in settings.json is lost on a model change; `.bak` keeps it).
- `icacls <dir> /inheritance:r /grant:r …:(OI)(CI)F /T` writes the inheritable
  ACEs onto the files too, where they do not apply — every file ends up with
  an empty DACL, unreadable and unrunnable even for administrators (nssm.exe
  «Отказано в доступе», the token «не читается»). Grant the directory without
  `/T`, then `icacls <dir>\* /reset /T` so the children inherit.
- nssm.cc did not resolve from this machine (and is flaky in CI): `nssm.exe`
  (2.24-101) is vendored in `setup/dev1c-setup/vendor` with its SHA-256 pinned
  in `pack.sh`; the llama/cudart archives are pinned the same way.
- A Windows path in TOML needs single quotes (`file = 'C:\models\x.gguf'`) or
  doubled backslashes; the sample says so now.
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
- `opm install -d <dir> vanessa-runner` vendors every dependency under
  `vanessa-runner/oscript_modules` — but resolves them as the hub has them
  today, hence the committed manifest `setup/1c-connect/vendor/vrunner-lib.sha256`.
  `vanessa-add` is the hub package `add` (6.8.0); vrunner looks for its
  `xddTestRunner.epf` at `<bin>/../lib/add`, so OneScript is unpacked with its
  own `bin/` + `lib/` layout and the opm tree merged into that `lib/`.
- In the OneScript image mount the output at `/out`, never `/lib`: `/lib` is
  the container's libc and `sh` stops loading. `find … | xargs sha256sum`
  without `-print0` loses the eight `add/features` names with spaces.
- OneScript's version flag is `-version` (one dash); `--version` prints the
  usage and exits 0.
- OneScript strings compare case-insensitively, so a child written in it reads
  `Tools/Call` and a `Name` key as a tool call. The bridge therefore forwards
  only parsed objects it re-serialises itself, with a fixed list of methods and
  `tools/call` rebuilt from the checked `name`/`arguments`.
- `std::sync::MutexGuard` held across an `.await` makes the future `!Send`;
  take the decision out of the lock before awaiting.
- `PathBuf::file_name` on Linux sees `C:\proj\Buh` as one segment: derive
  names from strings by both separators when the config may come from Windows.
- `vrunner-mcp` takes the base user only from tool arguments: `VRUNNER_DBUSER`
  / `VRUNNER_DBPWD` are ignored (while `VRUNNER_IBCONNECTION` works), so the
  bridge and the local caller put `ibconnection`, `db-user`, `db-pwd` into every
  vrunner call — and replace whatever the agent sent, or the host's password
  would travel to an address of the agent's choosing. `cf_decompile`/`cf_load`
  also need `oscript` on PATH for their background tasks; `background: false`
  makes them synchronous.
- On Windows `git commit` cannot detach its auto-gc: committing a 92 k-file
  dump repacked for an hour at 2 GB RAM. The bridge's git runs with
  `gc.auto=0`, `core.fscache`, `core.preloadindex`. The first push of such a
  dump is still minutes of `pack-objects` on a healthy machine and hopeless on
  a starved one — do it once, before the pilot, not on the user's laptop.
- The test base's user is `mcp`/`mcp` (publication answers 200 to anything,
  so the publication proves nothing about Configurator credentials);
  «Пользователь ИБ не идентифицирован» is the platform saying "wrong user".
- PowerShell `Start-Process -ArgumentList` splits an argument with a space
  (`--message "первая выгрузка"` became two arguments and clap refused).
- A line-framer with a size limit has two overflow paths: the limit crossed
  with no newline in sight, and the newline arriving in the same read as the
  overflow. The first was tested, the second emitted an 8 MiB `Line` — found
  by the bridge test that expected a parse error for an oversized request.

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
5. **Inference server** (stage 5) — done 2026-10-16, plan
   `docs/superpowers/plans/2026-10-16-inference-server.md`: three forms in
   `[model] serve`, host service by default, measurement and sync into Zed,
   three new `check` facts, window support, kit with the llama row, image
   without CUDA. Left: live run of the container form; deploying the new
   image here; an agent reply over RDP against the host-form server; a
   «Обзор…» file picker in the window (text field today).
6. **(superseded) Local model as a sibling container, the volt-llama way** (decided
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
6. **`vrunner-mcp` in the developer's kit** (stage 6). 6A done 2026-10-16,
   plan `docs/superpowers/plans/2026-10-16-vrunner-bridge.md`: `[host]
   project`/`tools`, the 778N bridge (`jsonrpc.rs` + `bridge::pipe_jsonrpc`),
   git tools `project_status`/`project_pull`/`project_push` on a project
   mutex, Job Object, `host.tools` into `%LOCALAPPDATA%\dev1c`, the kit rows
   (OneScript, vanessa-runner + add by manifest, MinGit), tunnel under the
   workstation's account. The review's minors are closed too: junk and
   oversized agent lines get a JSON-RPC parse error (id null), a panic inside
   a git tool no longer jams the project mutex, `origin` is pointed at the
   configured remote before every git call, the bridge's output channel is
   bounded. Still accepted: abort of a bridge task does not interrupt the
   running git call (the mutex frees when it ends); `/P…` redaction also
   blots `/Path…` in journal lines. Dependabot's `glib` 0.18.5 alert
   (RUSTSEC-2024-0429, `VariantStrIter`, Linux target only) stays: every
   layer of Tauri 2.12.4 — tray-icon, muda, wry, webkit2gtk — pins `gtk
   ^0.18`, and the fixed glib 0.22 is only reachable through gtk 0.19, which
   none of them has adopted (not even `tauri 3.0.0-alpha.5`); the iterator
   is not used here and the Linux build has no window. Revisit when
   `webkit2gtk-rs` releases against gtk 0.19. **6B next:** `host.project`
   (git init, remote, `.gitattributes`, `autumn-properties.json`),
   `export`/`import`, the «Проект» tab and project fields in the window,
   the remaining `check` facts (vrunner-mcp answers, project, Gitea,
   Configurator, removable media), the image (org `dev1c`, git credentials,
   AGENTS.md), live run here with Бухгалтерия. **6B done 2026-10-18** except
   the tail of the live run (see «Not verified»): plan
   `docs/superpowers/plans/2026-10-17-vrunner-project.md`. Left for the
   merge with the other tracks (GPUI window in worktree `pgui`, the image
   with the platform inside): the «Проект» tab logic is in `flow.rs` /
   `project.rs` / `verify.rs`, not in the window. This is what makes PRISM
   category B possible.
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
