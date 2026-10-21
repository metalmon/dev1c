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
  (hides on blur, left click on the icon brings it back); the window's colour
  theme is now 1С:Такси's own palette (yellow accent `#FACC1F`, links stay
  blue), not Zed's macOS Classic (task 11, 2026-10-18 customer call); the
  window is three tabs (Параметры, Проект, Журнал) with one primary button and
  a «…» menu per tab, and its height follows the Параметры content both ways
  (task 13, same call); «Проверить» prints the seven facts as lines in the
  journal and writes `last-check.txt`; «Подключить» connects with whatever
  address is saved — a wrong one fails in the journal; the journal reads as
  sections per operation with a clock in the head line, a mark instead of
  colour alone, and «Готово за …», mirrored to `1c-connect.log` next to the
  kit (task 14, same call);
  the form saves `1c-connect.toml` through `render`; the tray menu now also
  carries two checkboxes, autostart at login (`HKCU\...\Run`, value
  `"<exe>" --tray`) and autoconnect at launch (task 16, 2026-10-18 customer
  call); the separator above the footer is now a 2 px progress bar —
  a 1 px line at rest, an accent thumb while anything is in flight: operation,
  tunnel connect, or background refresh (task 17, same call) — built in CI
  only, not yet tried by hand; the status card now follows a running
  export/import with its phase, a running clock, the thread's last line, a
  «нет новостей N мин» warning past five quiet minutes, and «Прервать» for
  the two cancellable git phases (tasks 4–6, plan 2026-10-19-progress-v1).
  `dev1c-setup.exe` shows
  the real state of the container here (image, volumes, container: done;
  model: blocked) and the access addresses. Builds belong in CI: on this
  laptop a cold `cargo check` of the GPUI window takes about 10 minutes and
  only fits at `-j 1`, in the foreground; release builds stay in CI too.

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

- *(history)* Tauri used to fire `Focused(false)` on the window when focus
  moved *into* its own WebView2 child right after the first show, flashing
  and vanishing the flyout at launch; moot since `1c-connect` moved to GPUI
  (task 7, `docs/superpowers/specs/2026-10-16-gpui-connect-design.md`).
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
- *(closed)* Dependabot's `glib` 0.18 advisory came in through
  `dev1c-setup`'s Tauri GTK chain. Both windows are GPUI now and Tauri is out
  of the workspace, so the chain is gone with it.
- The window ships only in the Windows build: Linux CI builds pass
  `--no-default-features` for both binaries: GPUI-on-Linux is not needed by
  either of them. A release build with the window is
  `windows_subsystem = "windows"`: commands get their console back through
  `common::console::attach`; without it `status` prints nowhere.
- Icons are made once with Pillow (`icons/icon.ico`, the 64 px `tray.png`,
  the SVG mark via headless Edge) and embedded by `embed-resource` through
  each crate's `icons/app.rc`; the status dot is drawn at run time over
  `tray.png`.
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
- A debug build of `1c-connect` is a console app (`windows_subsystem` only
  goes to `"windows"` in release): its conhost window steals focus on launch
  and hides the flyout right behind it, before anyone clicks anything.
- Under a test harness or other automation, the flyout's `PopUp` window opens
  behind the foreground window (Windows' foreground-lock heuristic distrusts
  a background process asking for focus); a real launch from Explorer is
  unaffected.
- GPUI builds are heavy: a cold build takes around 10 minutes on this 16 GB
  box, and `cargo` run in the background gets OOM-killed under memory
  pressure. Build GPUI crates in the foreground, `-j 1`.
- `windows-sys` 0.60 needs the `Win32_Security` feature for `job.rs`
  (`setup/1c-connect`'s Job Object code) — not pulled in by the other
  Win32 features already enabled there.

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
   (RUSTSEC-2024-0429, `VariantStrIter`, Linux target only) came in through
   `dev1c-setup`'s Tauri 2.12.4 chain — tray-icon, muda, wry, webkit2gtk, all
   pinning `gtk ^0.18`. Closed 2026-10-19: that window is GPUI too, and with
   Tauri out of the workspace nothing pulls gtk any more. **6B next:** `host.project`
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
8. **MOSTLY CLOSED 2026-10-21 by the platform move — the corpus is accepted on
   8.3.27.1508.** Measured on the raised stand by the onboarding session (as
   `dev1`, on a fresh volume — no 8.5 corpus to delete): `syntax_help` now
   returns real text, not just signatures. `СтрДлина` → «Получает количество
   символов в строке» with a typed parameter and a «## Примеры» section;
   `Сообщить` and `ЗначениеЗаполнено` likewise with descriptions and return
   types. No `degraded`, no `bundled`, no `origin` in stderr on any of three
   calls; the cache built itself (24 MB under
   `/home/dev1/.cache/bsl-analyzer/platform-help`). Two naming corrections for
   whoever repeats it: the command is `mcp serve --profile reference` (no
   `--stdio` any more — stdio is the default), and `syntax_help` answers in
   **Markdown**, so the description is read from the «## Описание» section, not
   from `documentation.description` in `structuredContent`.
   What remains of the original task: the corpus is still built per user and on
   first use (a few minutes of stall for whoever asks first), and an
   interrupted build can leave a `work/` directory behind — on a fresh volume
   neither showed up, so this is now a nicety, not a defect. Original note kept
   for context:
   **Platform help corpus — build it once, deliberately.** `bsl-analyzer` 0.3.2
   builds it ITSELF on first use: it finds the platform in the volume, extracts
   `shcntx_ru.hbk`/`shlang_ru.hbk` and writes a snapshot under
   `~/.cache/bsl-analyzer/platform-help/auto/<hash>/` (24 MB corpus) — nothing in
   the Dockerfile asks for it. Two costs: every workplace builds its own (three
   users = three builds of several minutes each, first help lookup stalls), and
   each build leaves ~251 MB of extraction leftovers in `platform-help/work/`
   (directory named after the pid — nobody cleans it). Decide the deliberate
   form: build at image build time and ship one read-only corpus for everyone,
   or build once at first container start and share it; either way drop the
   `work/` leftovers. Note `platform-help build --hbk-dir` exists only in the
   analyzer's master docs; 0.3.2 has `platform-help package` (packaging for
   publication) plus the automatic path.

   **Measured 2026-10-21, and it is worse than «untidy»: the corpus built from
   our 8.5 is REJECTED.** The analyzer says
   `invalid help corpus: platform overlay: type property addition
   ClientApplicationForm.WindowOpeningMode already exists in the extracted data`
   for `/opt/1cv8/x86_64/8.5.1.1150`, falls back to `origin="bundled"` facts of
   `platform_version 8.3.27.2214` and reports `platform help degraded …
   descriptions are empty`. So the agent in Zed gets signatures only (enough to
   stop it inventing `Стр()`/`Заменить()`, which is the failure class we
   measured — see `docs/model-settings-qwen36.md`), from the WRONG platform
   version, with no descriptions or examples. Fix direction: a newer analyzer
   (master has the explicit `platform-help build --hbk-dir`) or report the
   overlay conflict upstream to `defin85/bsl-analyzer`. Also unset in the image:
   `EMBEDDING_URL` (no semantic `search_docs`), and the lexical index builds in
   the background on first use. Caches as seen in the live container: dev1 24 MB
   corpus + 251 MB of leftover extraction (an interrupted build — dev3 has 24 MB
   and a clean `work/`), dev2 empty; `/home` as a whole is 27 GB, worth its own
   look. ITS (`its_help`) is a separate matter:
   it is not a disc and not a docker image but 1С:Напарник behind
   `NAPARNIK_TOKEN`; the invented-API failures we measured are plain platform
   syntax and are covered by the local corpus without it.
9. **Move the stand to the training platform 8.3.27.1508 (decided by the
   customer, 2026-10-21).** The reason is the market, not the analyzer: 8.5 is
   barely used at client sites, and the pilot has to look like what they run.
   The valid help corpus comes along for free — 8.3.27 is the same line as the
   facts the analyzer ships, so the overlay conflict of task 8 disappears.
   Distribution: <https://online.1c.ru/catalog/free/28765768/> (training
   8.3.27.1508, Linux 64-bit and Windows; macOS is .1606).
   The chain, in order:
   1. download the Linux distribution for the image and the Windows one for the
      machine with 1C;
   2. **the configuration changes too — УНФ cannot come along.** Its kit
      manifest (`platform/1cv8.mft`) says
      `Name=УправлениеНебольшойФирмойУчебная`, `Version=3.0.13.251`,
      **`AppVersion=8.5`**: a configuration made for 8.5 does not open under
      8.3.27, and no re-dump helps. `online.1c.ru/catalog/free/learning.php`
      pairs applied solutions with platforms, and for the 8.3 line it offers
      only the platform plus **«Демонстрационная конфигурация 1С:Платформа 8.3»**
      (`/catalog/free/33052010/`, a `1cv8.dt`, explicitly «не является
      полнофункциональной конфигурацией»); УТ is not on offer at all, and the
      other applied solution is from the ancient 8.3.13 kit. Decided
      2026-10-21: take the demo configuration. Side win — our `unf-ready.dt` is
      **1.1 GB** and the kit ships volume archives; the demo base is tens of MB.
      Check on download: the minimum platform version it declares (the page does
      not say) and whether its metadata is deep enough to demonstrate anything.
      Then load it and walk the welcome wizard (if it has one) ON 8.3.27 and
      `/DumpIB` the result — that is the one manual step. The skills
      (`cf-edit`, `meta-edit`, `form-edit`, `skd-edit`, `query-opt`) are not
      tied to УНФ; the onboarding scenario and the demo walkthrough may be, and
      need checking;
   3. rebuild the payload image (`PLATFORM_VERSION` in `Dockerfile.1c`, the
      `setup-training-<version>-x86_64.run` name, the `/opt/1cv8` symlink);
   4. check that `MCP_HTTPService.cfe` from the mcp-1c release loads into
      8.3.27 — an extension saved in 8.5 would be refused, and without it the
      agent has no live base at all;
   5. re-check vrunner and the skills on 8.3.27.

10. **Speculative decoding: measure it, and build the static ngram cache from
    our own corpus.** `--spec-type` in b11734 takes a LIST of
    `none | draft-simple | draft-eagle3 | draft-mtp | draft-dflash | draft-dspark
    | ngram-simple | ngram-map-k | ngram-map-k4v | ngram-mod | ngram-cache`
    (default `none`), so MTP needs no extra PR — unsloth's quants carry the MTP
    tensors. It changes SPEED only: verification keeps the output identical to
    the unspeculated model, so this is never a quality question.
    The `ngram-*` variants need no weights and no files at all — the draft comes
    from the context (`--spec-ngram-mod-n-min 48 / -n-max 64 / -n-match 24`;
    `ngram-simple`: `-size-n 12 / -size-m 48 / -min-hits 1`). Code repeats
    itself, and an agent that re-emits a module to change three lines is the
    exact case lookup speculation was made for — likely a better bet on BSL than
    MTP, whose heads were trained on the general distribution.
    `ngram-cache` is the one variant with files: `-lcs/--lookup-cache-static`
    (not updated by generation) and `-lcd/--lookup-cache-dynamic`. Nobody
    publishes a static cache — **we can build ours** from real 1C code (the demo
    configuration, BSP, the skills' examples) so the model has idiomatic BSL
    n-grams from the first token. Cheap and entirely ours, unlike MTP.
    Measure on the 5090: `none` / `draft-mtp` / `ngram-mod` /
    `draft-mtp,ngram-mod`, one session and three, tokens per second plus the
    acceptance counters the server already exposes (`--metrics` is in our
    command line): `spec_decode_num_draft_tokens_total`,
    `spec_decode_num_accepted_tokens_total`, per-draft-position acceptance, and
    `n_busy_slots_per_decode`. Expect the gain to shrink with three busy slots:
    speculation pays at low batch, and with the card saturated the drafts
    compete for the same compute.
11. **DONE 2026-10-21. The pool arithmetic is gone (see task 14).**
    With separate per-slot caches the server is told the per-slot context
    directly and `/props` reports a slot's own share — so `pool_for()`, its 6/5
    factor, the round-up and the inverse `slot_window_in()` are deleted, not
    rewritten, together with their tests. `--kv-unified-per-slot` stays relevant
    only if we ever go back to a unified pool. Original note kept for context:
    **`--kv-unified-per-slot N` may delete our pool arithmetic.** b11734 has
    «context limit per parallel slot; when set without `-c/--ctx-size`, the
    shared KV pool is sized to `n_parallel*N`» — which is exactly what
    `llama::pool_for()` hand-rolls today (window + a fifth for the answer, times
    the slots, with `slot_window_in()` as the inverse). Check whether the flag
    means the same thing, and if it does, pass the per-slot window straight to
    the server and drop the 6/5 factor, the round-up and the inverse along with
    their tests. Verify against `/props`: the slot window the server reports
    must still match the number we declare to the workplace.

12. **DONE 2026-10-21, and the answer came out better than the plan.** The customer
    put it plainly (2026-10-21): the sum should be the number you configure.
    Today `fit_ctx` means the INPUT window and `zed_numbers` adds a fifth on top
    (`max_tokens = window + window/5`), so 250 000 configured means 300 000
    positions in the sequence — past the model's native 262 144 — and that is
    the only reason YaRN ever came up. Make the knob the total instead: Zed gets
    `max_tokens = <fit_ctx>`, `max_output = 32768` (Qwen's recommendation;
    measured answers on category A were 300–900 tokens, a thirtyfold margin),
    the input is the remainder, and the pool is `fit_ctx × slots`. At the native
    ceiling that is total 262 144 → input 229 376, pool 786 432, no YaRN at all.
    The 6/5 factor, its round-up and the inverse `slot_window_in` disappear
    along with their tests — the same simplification task 11 wants. Keep what
    the original comment was defending: Zed must not be handed a number that
    makes it ask for more than the slot holds, which is exactly what naming the
    total prevents.
    Two rules the customer added (2026-10-21):
    **the number fit actually allocated is the source of truth, not our
    arithmetic.** `--fit-ctx` is a FLOOR («minimum ctx size that can be set by
    --fit»), and the card may give more. So the config names the required
    minimum total per slot, the server gets it (via `--kv-unified-per-slot`, or
    multiplied by the slots), and then `/props` is read back: the real pool
    divided by the slots is what Zed is told. Never declare a number we asked
    for — only one the server confirmed.
    **Round that division DOWN to a multiple of 8**, otherwise the declared
    window exceeds the slot's actual share. Our numbers already satisfy it
    (262 144, 229 376, 32 768), but in code it must be a rule, not a
    coincidence; `-ub 512` is in our command line, so flooring to 256 costs
    nothing and is safer at the edges — pick 8 or 256 deliberately. And clamp
    the per-slot total to the native 262 144: above it YaRN comes back.
    (Superseded framing: «replace answer = a fifth with a fixed 32768» —
    same arithmetic, one concept fewer.) Not a bug — `pool_for()` is right that a slot may hold the window
    AND the answer — but our own 1/5 rule is wasteful at a large window and it
    is what pushes us past the model's native limit. Qwen3.6 is native 262144
    per SEQUENCE (the unified pool may exceed that; a single sequence may not),
    and Qwen recommends 32768 of output. Arithmetic: with 1/5 the honest window
    is 218 453; with a fixed 32768 answer it is **229 376** (pool 3 × 262144 =
    786 432), and YaRN is needed in neither case. Measured 2026-10-21: with
    thinking off the model's answers on category A were 300–900 tokens, so
    32768 carries a thirtyfold margin. Declaring 250 000 per slot would need
    300 000 positions and therefore YaRN — and Qwen warn that its static scaling
    can cost short-context quality. So: cap `max_output` at 32768 in
    `llama::zed_numbers`, declare 229 376, drop the 6/5 factor (see task 11 —
    `--kv-unified-per-slot` may take the per-slot number directly).
13. **The bridge PRISM ↔ our harness (decided by the customer, 2026-10-21).**
    Everything measured so far is PRISM's own four-line prompt and one request
    per task: no skills, no mcp-1c, no agent loop, no chance for the model to
    compile its own code. Our product is the opposite. The bridge: run PRISM's
    tasks through our agent (`config/skills/*` plus mcp-1c, and for category B
    the metadata tools), put the answer back into PRISM's experiment format and
    score it with `prism score`. Two things it unlocks that nothing else can:
    the measured contribution of the harness itself (the first cheap step is a
    single fix round — hand the failing candidate its `oscript -check` output
    and the failed tests, let it correct once, re-score), and the only honest
    test of thinking in the agentic case, where Qwen trained `preserve_thinking`
    and where our «thinking off» verdict does NOT apply: it was measured on
    single-shot generation only.

14. **Zed has a NATIVE `llama.cpp` provider — evaluate moving off
    `openai_compatible`.** Settings shape: `api_url`, `auto_discover` (on by
    default), `available_models` (`max_tokens`, `supports_tools`,
    `supports_images`), a global `context_window` override, and
    `LLAMACPP_API_KEY`; router mode refines discovery through `/models/sse`.
    What it buys: a real provider instead of our invented «custom», the model
    name and the tool/vision flags discovered rather than substituted by the
    entrypoint.
    **The trap, and why it is OUR doing:** «Zed uses the context length the
    server reports (`/props`)» — and with `--kv-unified` `/props` reports the
    whole POOL, so auto-discovery would declare three times a slot's window.
    But that is a consequence of a flag we added ourselves, not of the
    provider. **Decided 2026-10-21: drop `--kv-unified` and run separate
    per-slot caches.** With every slot sized at the native ceiling the unified
    pool buys nothing — a sequence cannot pass 262 144 in any cache mode, so an
    uneven split upward is impossible, and the only use left (allocating a pool
    smaller than the sum and hoping the three never max out at once) is a bet we
    do not need: 3 × 262 144 = 786 432 tokens ≈ 7.5 GiB of KV at `q8_0`, and it
    fits. Note the upstream default agrees — the flag's own description says
    «default: enabled **if number of slots is auto**», and we pass `--parallel 3`
    explicitly, so llama.cpp would have used separate caches on its own.
    What the reversal pays for itself with: `/props` reports a slot's own share,
    so Zed's auto-discovery becomes correct, and the pool arithmetic of tasks 11
    and 12 (`pool_for`, the 6/5 factor, `slot_window_in`) disappears instead of
    being rewritten. Verify tomorrow how `-c` divides among slots without the
    unified flag (historically the total is split by `--parallel`; `/props`
    will show it) — that is the only open question left here.
    And the missing `max_output_tokens` is **not** a blocker: Zed sets the
    output reserve itself — a fifth of the declared window (that observation is
    where our own 1/5 came from, see the comment in `config/zed-settings.json`).
    Under task 12's redefinition (`max_tokens` = the per-slot TOTAL) that
    default is safe: Zed splits inside the declared number instead of adding on
    top, so the sum never exceeds the slot. The only cost is proportion — 52 429
    reserved for the answer instead of 32 768, i.e. 210k of input instead of
    229k. So the native provider is viable; decide whether those ~19k of input
    are worth keeping `openai_compatible` for.
    And slot save/restore is not part of any provider contract — see the batch
    section of `docs/model-settings-qwen36.md`: that needs our own caller.

**What tasks 11 and 12 actually became (2026-10-21, `setup/dev1c-setup`, 99 tests
green, `cargo test -p dev1c-setup --no-default-features`).** The customer's point
settled it: don't compute what the program can say itself.

- `/props` reports the SLOT's context (`slot_n_ctx`, verified in
  `server-context.cpp:1355`), so `pool_for()`, its 6/5 factor, the round-up and
  the inverse `slot_window_in()` are **deleted** along with their tests. Nothing
  divides or multiplies the server's number any more.
- `fit_ctx` stays the per-slot FLOOR; the server gets it multiplied by the slots
  because that is its own unit (`-c` is the total and llama.cpp divides it back,
  padding to a multiple of 256 — `llama-context.cpp:379`).
- `zed_numbers` now follows the spec's rule, not the old code's: the workplace is
  told the INPUT — `max_tokens = окно − ответ − запас`. The answer is
  `answer_tokens()` = an eighth of the window within 8192…32768 (Qwen asks for
  32768; measured answers with thinking off were 300–900 tokens), the margin is
  4096 because Zed counts tokens approximately. The old code added a fifth ON
  TOP of the window, which contradicted both the spec and the comment in
  `zed-settings.json` that earned the margin.
- The launch line lost `-b 2048`, `-ub 512`, `--ctx-checkpoints 32`,
  `--top-k 40`, `--top-p 0.95`, `--jinja` (verbatim b11734 defaults),
  `--kv-unified` (see task 14) and `--reasoning-budget 0` (replaced by
  `--reasoning off`, which sets `enable_thinking=false` in the template kwargs
  itself — `common/arg.cpp:3711`). It gained `--temp 0.6`, `--min-p 0.03`,
  `--metrics`.
- A bug the new arithmetic exposed: `fit_ctx` was validated at ≥ 16384, but a
  16384 window leaves 4096 of input — below `MIN_BUDGET`. The floor is now
  computed from the constants (`MIN_BUDGET + ANSWER_FLOOR + ZED_MARGIN` = 28672),
  so a useless window is refused by the config instead of failing the install
  after the server is already up.
- Touched beyond the crate: `config/kit/УСТАНОВКА.md` (the manual formulas — a
  test keeps the page and the code in agreement), the comment block in
  `config/zed-settings.json` (its fallback VALUES are unchanged and still obey
  the new rule: 55296 + 8192 = 63488 against 65536), the spec section «Локальная
  модель», and the `fit_ctx` field label in the GUI.

15. **`target_platform_version` per client project — the analyzer does NOT read
    the platform version from the dumped code.** Checked in the analyzer's docs
    (`docs/configuration/PROJECT_CONFIGURATION.md`): the version comes from
    `target_platform_version` in the project's `bsl-analyzer.toml`, and if the
    key is absent, from the built-in catalog — currently `8.3.27`. The
    configuration's own `CompatibilityMode` is never consulted. Two details
    worth keeping: the catalog is attested for the whole release line by the
    first three components, so `8.3.27.1508` / `.1644` / `.2214` are one and the
    same to it — which is exactly why the corpus was refused on 8.5 (a different
    line, hence the overlay conflict) and accepted after the move; and on a
    mismatched line it degrades safely rather than switching catalogs: a miss in
    the platform catalog stops counting as proof of absence, `UnresolvedName`
    and its kin go quiet, and the host logs an unsupported-target warning. So a
    wrong value produces silence, not false «no such name».
    **Decided by the customer 2026-10-21: the file must be WRITTEN, and written
    automatically — the task goes to the connector (`1c-connect`).** My earlier
    «not needed, the built-in default matches» holds only for the demo stand and
    only while we stay on 8.3.27. Two things break it: we may move back to 8.5,
    and — more importantly — developers will dump THEIR OWN configurations from
    their machines through the connector, and we do not know those versions in
    advance. On a different release line the analyzer degrades quietly:
    `UnresolvedName` and its kin go silent, which is exactly the signal that
    catches invented function names — measured as the top cause of failures
    (46 of 75 runs did not compile; see `docs/model-settings-qwen36.md`).
    Mechanics for whoever implements it: the file lives in the project root
    (`~/project/bsl-analyzer.toml` — vrunner's working directory, where
    `cf_decompile` puts sources) and is auto-discovered (priority:
    `bsl-analyzer.toml`, then legacy `.bsl-analyzer.json`, then
    `.bsl-language-server.json`; the CLI also takes `-c <path>`). One key is
    enough: `target_platform_version = "8.3.21"` — three components are the
    whole granularity the catalog has, the fourth is ignored. Two cautions: a
    `bsl-analyzer.toml` that fails to parse does NOT fall back to JSON, so a
    broken file is worse than none — write it only when the version is actually
    known, and do not clobber a file the developer edited. The version itself
    the connector already knows: it finds `1cv8` on the Windows machine to run
    the designer (`find_1cv8`), and `mcp-1c` can report the live base's
    platform and compatibility mode.
    **The accepted line is readable at runtime — there is no list to maintain**
    (answered to the connector session 2026-10-21, after it asked for one).
    `~/.cache/bsl-analyzer/platform-help/auto/current` holds the hash of the
    active snapshot, and that snapshot's `manifest.json` names the line:
    `{"corpus_id": "installed-8.3.27.1508", "platform_version":
    "8.3.27.1508"}`, with `source.json` naming the very hbk files it was built
    from. On 8.5 the build fails and the bundled `8.3.27` stays in force, so
    after the roll-back to 8.5.1.1150 the accepted line is 8.3.27, not 8.5.
    **And the key's meaning is narrower than it looks:** it never adds correct
    checking for another line — the catalog has no facts for it — it only
    silences false positives. Hence: equal lines → the key is redundant; client
    NEWER than the catalog (exactly the 8.5 case) → write it, or names added in
    8.5 arrive as unresolved and the agent «fixes» correct code; client older →
    do not bother, neither option catches the use of newer API.
    **Verified on OUR 0.3.2, not from master docs (2026-10-21) — and it
    overturned two of my own claims.** `check-config --config <file>` rejects
    unknown fields with a parse error and lists the accepted ones: `source`,
    `analysis`, `diagnostics`, `code_lens`, `formatting`,
    `target_platform_version`, `platform_help`, `min_platform_version`,
    `compatibility_mode`, `search`, `features`, `output`. So the key is at the
    ROOT, a section is rejected, and a misplaced key fails loudly. The bundled
    line of 0.3.2 is `8.3.27.2214` (it says so in its own degradation warning).
    Claim overturned #1: «the key only silences false positives» holds for
    `target_platform_version` ONLY. The other two add real checks:
    `min_platform_version` (the minimum platform the code must compile on)
    turns on `PlatformMemberNewerThanMinVersion` — silent until set, then it
    reports every use of a platform member available only in a later version
    (global function, type in `Новый`, a value's method, `Асинх`/`Ждать`);
    `compatibility_mode` turns on `PlatformMemberHiddenByCompatibilityMode` —
    the 30 globals a low mode hides (`СтрНайти`, `СтрРазделить`, bitwise, regex,
    `КопииБазыДанных`) plus `Запрос.ТребуемаяАктуальностьДанных`.
    Claim overturned #2: the configuration's own `<CompatibilityMode>` IS read —
    priority is the setting, then `Configuration.xml` of the main configuration,
    then «unknown». So `compatibility_mode` is normally NOT written; it is for
    the case where the dump does not reflect the client's base (dump says
    8.2.13, the client runs 8.3.15 → write the CLIENT's mode).
    **Corrected again by the connector session, with measurements (2026-10-21)
    — and it was right.** `min_platform_version` = «the client's platform» is
    worthless: the diagnostic fires on members NEWER than the minimum, and code
    written on the developer's machine is limited to that machine's members
    anyway, so a minimum equal to it is silence. The valuable value is the
    PRODUCTION platform of the customer, and that is human knowledge, not a
    fact of the environment — the connector cannot discover it from the dev
    machine. So it writes the machine's version as a safe value (no false
    alarms) with a comment saying what to change it to; optionally a field in
    `1c-connect.toml` («целевая платформа заказчика», empty by default) would
    let a human answer it once. `target_platform_version`: only when the
    client's line is newer than the catalog's. `compatibility_mode`: not
    written — see below.
    **`[diagnostics]`, `[code_lens]`, `[formatting]`, `[search]`, `[features]`,
    `[output]` are INERT in 0.3.2** (measured by the connector session: seven
    forms — `enabled = false`, `disabled`/`exclude`/`only` lists,
    `[diagnostics.rules]`, `[diagnostics.severity]`, per-rule parameters, both
    legacy JSON formats — all «Configuration is valid» and all leaving the same
    7 diagnostics, while `analyze --only-diagnostic LineLength` does filter).
    So rule configuration lives on the launch side only, a typo in a rule name
    is silent, and **a rule profile for the agent is not worth writing in this
    release** — I was about to propose one to quiet the stylistic noise.
    **What DOES work with no config file at all** (measured here): a dump with
    `<CompatibilityMode>Version8_2_13</CompatibilityMode>` and a module calling
    `СтрНайти`/`СтрРазделить` yields
    `Major PlatformMemberHiddenByCompatibilityMode: 'СтрНайти' недоступен в
    режиме совместимости 8.2.13: виден с режима 8.3.x`. The mode comes from the
    dump, no settings involved — which confirms both «don't write
    `compatibility_mode`» and the caveat I put in `config/agents-global.md`
    (the agent gets a clear message instead of silence on an old mode).
    `[source]` is unnecessary too (auto-discovery finds `src/Configuration.xml`,
    `src/cfe/*`, `src/epf/*`, `src/erf/*`), and `[platform_help]` already
    defaults to `auto` — so the client-corpus item below is about the 8.5
    corpus BUILD defect, not about configuration.
    **Follow-up worth its own task: a corpus for the CLIENT's line**, built from
    their own `shcntx_ru.hbk` / `shlang_ru.hbk` — they sit next to the `1cv8`
    the connector already locates with `find_1cv8`. That makes both the key and
    the checks true instead of a compromise.
    Original framing, kept because it is still the fallback for a hand-made
    project: the action is for the pilot: when the agent
    works on a CLIENT configuration pinned to an older compatibility mode, put
    `bsl-analyzer.toml` with `target_platform_version` of that client's line in
    the project root — otherwise the agent writes what 8.3.27 allows and the
    client's configuration refuses it.

16. **Техдолг заказчика 2026-10-21: у длинных операций нет отмены.**
    Кнопка «Отменить» в окне живёт только у двух фаз — `gui::can_cancel`
    отвечает `true` на `Phase::Push` и `Phase::Pull`, и поток там сам
    замечает флаг `Ui::cancel`. Самые долгие операции отмены не имеют вовсе:
    выгрузка конфигурации (`cf_decompile` — минуты, а на УНФ-масштабе
    десятки минут и 90 тысяч файлов), загрузка (`cf_load`), обновление базы,
    прогоны тестов. Все они идут через `local::call`, где слова `cancel` нет
    ни разу: есть только срок 3600 с, по которому вызов перестают ждать — а
    сам Конфигуратор продолжает работать, занимая базу и диск.
    
    Что это значит на деле: человек, нажавший «Выгрузить» не на той базе или
    с не тем каталогом, ничего не может сделать, кроме снятия процесса
    руками; агент, получивший долгий инструмент, не может его отозвать.
    
    Куда смотреть при починке: `Phase` и `can_cancel` в `gui/mod.rs`,
    `local::call` (срок без отмены), `flow.rs` (круг выгрузки — там флаг
    `cancel` уже протянут в `project::push`/`pull`, то есть механизм есть и
    его надо довести до остальных фаз), и `job`/`pipe_process` — убивать
    надо группу процессов, иначе Конфигуратор переживёт отмену. Честная
    отмена долгой операции 1С — это снятие дочернего процесса, а не
    прекращение ожидания: второе мы уже умеем и оно хуже, чем ничего, потому
    что выглядит как отмена.
17. **Техдолг заказчика 2026-10-21: проверка состояния лжёт, пока идёт
    выгрузка.** Замечено живьём: во время `cf_decompile` достаточно скрыть и
    снова открыть окно — идёт расчёт готовности, и в журнал приходит
    «vrunner-mcp не отвечает», хотя он в этот момент как раз изо всех сил
    работает над выгрузкой. Это ровно та ложь, против которой делалась вся
    задача про зелёную точку, только с другой стороны: красный факт при
    исправной работе.
    
    Причина не в сроке, а в самой пробе: `verify::vrunner_fact` не
    спрашивает работающий процесс, а **запускает второй** —
    `bridge::vrunner_command` плюс `local::call("cache_show")`. Пока первый
    держит Конфигуратор и базу, второй либо отвечает ошибкой, либо не
    укладывается в срок, и человек читает приговор инструментам, которые
    исправны. Заодно это лишний процесс поверх занятой базы.
    
    Как починить честно: пока операция идёт (окно знает это по `Ui::running`,
    а `can_cancel`/`serving` уже различают состояния), пробу не поднимать
    вовсе, а факт говорить словами — «занят: идёт выгрузка», как
    `Check::note`, а не `fail`. Тот же разбор нужен и соседним фактам,
    которые ходят к занятой базе (публикация, расширение, «в базе открыт
    сеанс»): во время своей же долгой операции они говорят не о поломке, а о
    том, что машина занята нами. Входы: `start_readiness`/`apply_ready` в
    `gui/mod.rs`, `facts_for_readiness` и `vrunner_fact` в `verify.rs`.
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
