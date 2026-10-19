# Headless Ubuntu install Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The customer's own administrator installs the environment on a headless Ubuntu 24.04 server with no internet, from the terminal, following one page of instructions — and every barrier names itself with a command that works offline.

**Architecture:** No new subsystem. The model server's container form (`serve = "docker"`) becomes the Linux default, which first requires the `dev1c-llama` image to become runnable at all. Barriers live where they already live — `OpState::Blocked` and `Check::fail` — and the Linux kit gains the pieces an offline server cannot fetch: the NVIDIA container toolkit as `.deb` files and an instruction page.

**Tech Stack:** Rust 2021 (`dev1c-setup`, `common`), Docker (20.10 on the target, modern in CI), `Dockerfile.llama` with CUDA 12.8, GitHub Actions (`build.yml`, `llama.yml`), POSIX `sh` (`pack.sh`).

**Spec:** `docs/superpowers/specs/2026-10-19-headless-ubuntu-design.md`

## Global Constraints

- Every user-facing string is Russian; code comments and commit messages are English (repository rule, `README.md`).
- Commits use `git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit` (no global identity here) and end with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- **The target server has no internet.** No remedy may say «поставьте пакет из сети»; everything it needs is in the kit or already on the machine.
- **The target Docker is 20.10.24.** No command, flag or archive format newer than that engine.
- Build in CI, not on this laptop (`docs` memory, see `build-in-ci-only`): `cargo test --workspace --no-default-features -j 1` is the only local check. For GPUI sources `rustfmt --edition 2021 --emit stdout <file>` is the local syntax gate; nothing in this plan touches them.
- GitHub runners have no GPU. CI proves everything up to the card and stops there by design; the card itself is proven on the pilot server.
- The kit's own `checksums.txt` covers every file the kit carries, the new `.deb` files included.

## Review Focus

1. **A non-root administrator in group `docker`**: install must complete and the following `check` must still read the model server's token — today `ensure_token` closes `/opt/dev1c/llama` to `root:root` and one of the two paths always loses. Test in Task 3.
2. **No `nvidia-container-toolkit` on the server**: the install must stop with the kit's own `dpkg` line, not with a raw `docker: Error response from daemon: could not select device driver`. Test in Task 4.
3. **Docker 20.10 loading `dev1c-image.tar.gz`**: an archive with attestation manifests dies on the customer's machine with a message about an unsupported manifest. Test in Task 2 (the assert runs in our build, not theirs).
4. **The kit arrives without the model or without the image archive**: each is a named barrier with a `scp` line, and nothing half-installs. Test in Task 4.
5. **The model container with no limit of its own**: prompt cache plus checkpoints push the host into swap and 1C in the environment container slows with it — the limit, the swap allowance and the warning about a small swap file are the whole of the answer. Test in Task 8.
6. **An administrator who refuses to run our binary**: the manual section must carry every command the program would run, in order, for the kit's own `dev1c.toml` — a missing `docker volume create` leaves them with an environment that starts and loses its data on the next restart. Test in Task 7.
7. **The vGPU share shrinks after installation**: the environment was built around numbers measured when the card was whole; `check` must say the model no longer fits, with both numbers, instead of leaving Zed with a context that silently does not work. Test in Task 6.

---

### Task 1: The model-server image becomes runnable

**Files:**
- Modify: `Dockerfile.llama` (the final stage, lines ~132-138)
- Modify: `setup/dev1c-setup/src/ops_llama.rs` (`llama_run_args`: the image now has an entrypoint)
- Modify: `.github/workflows/llama.yml` (the smoke step after build)

**Interfaces:**
- Produces: image `ghcr.io/<owner>/dev1c-llama:<LLAMA_VERSION>` whose default command starts the server, keeping `/opt/llama` at the same path so the main `Dockerfile`'s `COPY --from=…` still works.
- Consumes: the existing builder stage.

- [ ] **Step 1: Give the image a runtime**

Replace `FROM scratch` with a slim Debian 12 base (same libc the builder used), keep `COPY --from=builder /opt/llama /opt/llama`, and add: `ENV PATH=/opt/llama:$PATH LD_LIBRARY_PATH=/opt/llama NVIDIA_VISIBLE_DEVICES=all NVIDIA_DRIVER_CAPABILITIES=compute,utility`, `WORKDIR /opt/llama`, `ENTRYPOINT ["/opt/llama/llama-server"]`. `ops_llama::llama_run_args` passes `llama-server` as the command after the image name; with an entrypoint that word becomes an argument the server does not understand, so drop it from the args in the same commit. The existing `run_args_carry_ports_volumes_and_environment`-style test in `ops_llama.rs` pins the result: the arguments after the image name must start with `--model`.

- [ ] **Step 2: Prove the image starts, in the same workflow that builds it**

In `llama.yml`, after the build and before the push:

```bash
docker run --rm "$image" --version
docker run --rm --entrypoint sh "$image" -c 'ldd /opt/llama/llama-server | grep -q "not found" && exit 1; exit 0'
```

Expected: the version line prints and the library check exits 0. A CI runner has no GPU, so this is a CPU-only start: it proves the image is not a corpse, nothing more.

- [ ] **Step 3: Raise the llama.cpp pin and prove the flags still exist**

Upstream has moved on and the image is being rebuilt anyway: set `ARG LLAMA_VERSION` to the current release tag. Then assert, in the same `llama.yml` smoke step, that every flag `llama::serve_args` passes is still understood:

```bash
for f in --fit --fit-ctx --fit-target --cache-ram --ctx-checkpoints --kv-unified --flash-attn -ctk -ctv --parallel --api-key-file; do
  docker run --rm --entrypoint sh "$image" -c "/opt/llama/llama-server --help" | grep -q -- "$f" || { echo "флага $f в новой сборке нет: $f"; exit 1; }
done
```

Expected: every flag found. A flag that quietly disappeared changes how the server behaves on the stand, and nothing else would notice.

- [ ] **Step 4: Build and push the new tag**

Run the workflow (`gh workflow run llama.yml`), about 40 minutes. Expected: a new `dev1c-llama:<version>` digest in ghcr. Note the digest in the task's ledger line — Task 2 pins it.

- [ ] **Step 5: Commit**

```bash
git add Dockerfile.llama .github/workflows/llama.yml setup/dev1c-setup/src/ops_llama.rs
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "fix(llama): the model-server image can actually run, on a current llama.cpp"
```

---

### Task 2: The archive the old Docker accepts

**Files:**
- Modify: `.github/workflows/build.yml` (the image save step, around line 253-260)
- Create: `scripts/check-image-archive.sh`

**Interfaces:**
- Consumes: Task 1's image tag.
- Produces: `dev1c-image.tar.gz` whose root holds `manifest.json` and no attestation manifests; `scripts/check-image-archive.sh <tar.gz>` exits non-zero otherwise.

- [ ] **Step 1: Write the failing check**

`scripts/check-image-archive.sh` lists the archive and fails when `manifest.json` is absent, or when any manifest entry carries `vnd.docker.reference.type` / an `application/vnd.in-toto` media type (that is an attestation). Run it against an archive built with attestations on purpose.

Run: `sh scripts/check-image-archive.sh /tmp/with-attestations.tar.gz` — Expected: non-zero exit, a message naming what it found.

- [ ] **Step 2: Make the real archive pass**

In `build.yml` the images are built and saved for the kit: force a single platform and no attestations (`--provenance=false --sbom=false` on any buildx call; `docker build` the classic way is also fine), then `docker save … | gzip -1` as today.

Run: `sh scripts/check-image-archive.sh dev1c-image.tar.gz` — Expected: exit 0.

- [ ] **Step 3: Wire the check into the workflow**

The pack job runs the script right after saving and before `pack.sh`; a bad archive fails the build.

- [ ] **Step 4: Commit**

```bash
git add scripts/check-image-archive.sh .github/workflows/build.yml
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "ci: the kit archive must load on Docker 20.10"
```

---

### Task 3: The model server's secret leaves /opt

**Files:**
- Modify: `setup/dev1c-setup/src/llama.rs` (`token_file`, `ensure_token`, `ensure_dir` call sites)
- Modify: `setup/dev1c-setup/src/ops_llama.rs` (the `Container` op)
- Test: `setup/dev1c-setup/src/llama.rs` (unit tests at the bottom)

**Interfaces:**
- Produces: `llama::token_path(kit: &Kit, form: Form) -> PathBuf` — the kit's own directory for `Form::Docker`, `/opt/dev1c/llama` (Linux) or `C:\dev1c\llama` (Windows) for `Form::Host`; `llama::ensure_token(ctx)` writes it with mode `600` and never calls `ensure_dir` in the docker form.
- Consumes: `Kit`, `cfg::Form`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_container_form_keeps_its_secret_in_the_kit() {
    assert_eq!(token_path(&kit, Form::Docker), kit.path("dev1c-llama.token"));
    assert_ne!(token_path(&kit, Form::Host), token_path(&kit, Form::Docker));
}

#[test]
fn the_secret_is_written_closed_to_its_owner() {
    // mode & 0o777 == 0o600 on unix; the file is not world-readable
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features llama::` — Expected: FAIL, `token_path` not found.

- [ ] **Step 3: Implement**

`ensure_token` takes the path from `token_path(&ctx.kit, ctx.cfg.form())`. The docker form stops calling `ensure_dir` (that directory belongs to the systemd service form only). `llama_run_args` already passes the token through `--env-file`; that file moves with the token and gets the same mode.

- [ ] **Step 4: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup/src
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "fix(dev1c-setup): the container form keeps its secret in the kit, not in /opt"
```

---

### Task 4: Barriers that work offline

**Files:**
- Create: `setup/dev1c-setup/src/barrier.rs` (pure texts and verdicts, no side effects)
- Modify: `setup/dev1c-setup/src/ops.rs` (`Env`, `Image`, `Model`), `setup/dev1c-setup/src/ops_llama.rs` (`Container`), `setup/dev1c-setup/src/main.rs` (`mod barrier;`)
- Test: `setup/dev1c-setup/src/barrier.rs`

**Interfaces:**
- Produces: `barrier::docker_verdict(out: &str, code: i32) -> Option<String>` (permission denied / daemon down / fine), `barrier::gpu_in_container(ok: bool, kit_has_debs: bool) -> Option<String>`, `barrier::missing_file(what: &str, path: &Path, host_hint: &str) -> String`.
- Consumes: `common::docker::probe` output.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn docker_without_rights_says_how_to_get_them() {
    let t = docker_verdict("permission denied while trying to connect to the Docker daemon socket", 1).unwrap();
    assert!(t.contains("usermod -aG docker"));
    assert!(t.contains("sudo"));
}

#[test]
fn a_dead_daemon_is_not_a_rights_problem() {
    let t = docker_verdict("Cannot connect to the Docker daemon at unix:///var/run/docker.sock", 1).unwrap();
    assert!(t.contains("systemctl"));
    assert!(!t.contains("usermod"));
}

#[test]
fn a_card_invisible_to_containers_points_at_the_kit_not_at_the_internet() {
    let t = gpu_in_container(false, true).unwrap();
    assert!(t.contains("dpkg -i"));
    assert!(!t.contains("apt"), "сервер без интернета: {t}");
    // Нет пакетов в комплекте — честно говорим, что нужен администратор.
    assert!(gpu_in_container(false, false).unwrap().contains("nvidia-container-toolkit"));
    assert!(gpu_in_container(true, true).is_none());
}

#[test]
fn a_missing_file_comes_with_the_line_that_brings_it() {
    let t = missing_file("архив образа", Path::new("/srv/kit/dev1c-image.tar.gz"), "<user>@<сервер>");
    assert!(t.contains("scp"));
    assert!(t.contains("/srv/kit"));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features barrier::` — Expected: FAIL.

- [ ] **Step 3: Implement and wire in**

`Env` asks the engine through `docker_verdict`; `Image` and `Model` answer `Blocked(missing_file(..))`; the docker form's `Container` probes the card from inside a container — `docker run --rm --gpus all <образ модели> --version` is the cheapest honest probe once Task 1 made the image runnable — and answers `Blocked(gpu_in_container(..))`. The probe runs only when the image is already loaded; before that the archive barrier speaks first.

- [ ] **Step 4: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup/src
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): barriers name themselves with commands that work offline"
```

---

### Task 5: The Linux kit

**Files:**
- Modify: `pack.sh` (a Linux kit beside the Windows one), `setup/dev1c-setup/src/main.rs` (`example` takes a form)
- Modify: `.github/workflows/build.yml` (fetch the toolkit `.deb` files, pack the Linux kit, upload it)
- Test: `setup/dev1c-setup/src/cfg.rs`

**Interfaces:**
- Produces: `dist/dev1c-kit-linux/` — `dev1c-setup`, `dev1c.toml` with `serve = "docker"`, `dev1c-image.tar.gz`, `nvidia-container-toolkit/*.deb`, `УСТАНОВКА.md`, `checksums.txt`; CLI `dev1c-setup example --serve docker|host|remote`.
- Consumes: Task 2's archive, Task 7's instruction file.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_sample_can_be_printed_for_any_form() {
    for form in ["host", "docker", "remote"] {
        let text = Cfg::example_for(form);
        assert!(text.contains(&format!("serve = \"{form}\"")));
        // Комментарии образца на месте: файл остаётся человеческим.
        assert!(text.contains('#'));
        // И он читается обратно без потерь.
        let back: Cfg = toml::from_str(&text).unwrap();
        assert_eq!(back.model.serve, form);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features example_for` — Expected: FAIL.

- [ ] **Step 3: Implement `Cfg::example_for` and the CLI flag**

`example` gains `--serve <form>`; the default stays what it is today, so `EXAMPLE == Cfg::default()` and the round-trip tests in `cfg.rs` keep passing.

- [ ] **Step 4: Assemble the Linux kit**

`pack.sh` learns `--os linux|windows` (default windows, as today): the Linux kit takes no `.exe`, no llama row, no `nssm`, writes `dev1c.toml` with `./dev1c-setup example --serve docker`, copies `nvidia-container-toolkit/*.deb` from a directory the workflow prepared, and runs `checksums` over the result.

In `build.yml` the `.deb` files are fetched once from NVIDIA's repository with their SHA-256 pinned in `pack.sh`, next to the other pinned downloads.

Run: `./pack.sh --os linux dev1c-image.tar.gz bins && ls dist/dev1c-kit-linux` — Expected: the layout above; `checksums.txt` lists the `.deb` files.

- [ ] **Step 5: Commit**

```bash
git add pack.sh setup/dev1c-setup/src .github/workflows/build.yml
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(kit): a Linux kit with the container form and the toolkit packages"
```

---

### Task 6: The install path under SSH, and the shrinking card

**Files:**
- Modify: `setup/dev1c-setup/src/main.rs` (no TTY → `--yes` for safe work, refusal for destructive)
- Modify: `setup/dev1c-setup/src/verify.rs` (the model fact compares the measured window against what the card can give now)
- Test: both files

**Interfaces:**
- Produces: `main::answers_without_tty(has_tty: bool, cmd: &Cmd) -> Answer` (Default | Refuse), `verify::model_window_fact(measured: u32, available: u32, fit_ctx: u32) -> Check`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_ssh_session_without_a_terminal_does_not_hang_on_questions() {
    assert_eq!(answers_without_tty(false, &Cmd::Install { only: None }), Answer::Default);
    // Удаление данных без терминала не делается молча: только явным ключом.
    assert_eq!(answers_without_tty(false, &Cmd::Clean { stack: false, data: true }), Answer::Refuse);
    assert_eq!(answers_without_tty(true, &Cmd::Clean { stack: false, data: true }), Answer::Ask);
    // Снять контейнер без терминала можно: данные остаются на месте.
    assert_eq!(answers_without_tty(false, &Cmd::Clean { stack: true, data: false }), Answer::Default);
}

#[test]
fn a_card_that_shrank_is_a_failed_fact_with_both_numbers() {
    let c = model_window_fact(65536, 20000, 65536);
    assert!(!c.ok);
    assert!(c.detail.contains("65536") && c.detail.contains("20000"));
    assert!(model_window_fact(65536, 70000, 65536).ok);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features tty model_window` — Expected: FAIL.

- [ ] **Step 3: Implement**

TTY is `std::io::IsTerminal` on stdin. The window fact reads the measured number from the kit state and the available one from `llama::gpu`; the card is shared by vGPU, so «было столько, сейчас столько» is the whole message.

- [ ] **Step 4: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup/src
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): installs over SSH without a terminal, and notices a shrunken card"
```

---

### Task 7: The instruction, and the same install without our binary

**Files:**
- Create: `config/kit/УСТАНОВКА.md` (the hand-written part), `scripts/manual-path.sh` (generates the command section)
- Modify: `setup/dev1c-setup/src/main.rs` (`--commands`), `pack.sh`
- Test: `setup/dev1c-setup/src/main.rs`, plus the generator's own assertions

**Interfaces:**
- Produces: `dev1c-setup install --dry-run --commands` prints every modifying command, one per line, in the order the operations would run them; `scripts/manual-path.sh <kit dir>` turns that into the instruction's «Ручной путь» section.
- Consumes: `Event::Command`, which `common::docker` already emits in dry-run without executing anything.

- [ ] **Step 1: Write the failing test for the flag**

```rust
#[test]
fn dry_run_with_commands_prints_the_command_lines_and_nothing_else() {
    // Событие Command печатается как есть, без значка и без «$».
    assert_eq!(command_line(&Event::Command("docker volume create dev1c-home".into())), Some("docker volume create dev1c-home".to_string()));
    assert_eq!(command_line(&Event::Ok("готово".into())), None);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features command_line` — Expected: FAIL.

- [ ] **Step 3: Implement the flag**

`--commands` turns `Event::Command` into a printed line; everything else keeps today's behaviour. Passwords are already redacted upstream by `docker::redact`, so the printed lines are safe to paste into a page.

- [ ] **Step 4: Generate the manual section**

`scripts/manual-path.sh` runs the binary in the kit against the kit's own `dev1c.toml` and writes the section: a line of prose per operation (from the plan the program prints) followed by its commands. It fails if the output carries no `docker load`, no `docker volume create` and no `docker run` — that would mean the generator silently produced an empty path.

Run: `sh scripts/manual-path.sh dist/dev1c-kit-linux` — Expected: a section where those three appear, in that order.

- [ ] **Step 5: Write the hand-written part of the page**

One page, Russian: what the server needs before we start (Docker, GPU driver, the toolkit from the kit), what to copy and where, how to check the sums without us (`sha256sum -c checksums.txt`), the short path of three commands (`./dev1c-setup status`, `install`, `check`), then the generated «Ручной путь» section, and at its only un-pasteable step — the model measurement — which number to read from the server's answer and where to put it. Then: what is normal to see, and where the log is.

- [ ] **Step 6: Walk it yourself against the program's texts**

Read the page next to `dev1c-setup status` output on a machine without the kit files and confirm that each barrier the page mentions is a barrier the program itself prints. Fix whichever is wrong — the program wins.

- [ ] **Step 7: Commit**

```bash
git add config/kit scripts/manual-path.sh setup/dev1c-setup/src pack.sh
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "docs(kit): the administrator's page, and the same install without our binary"
```

---

### Task 8: Memory for a stand that holds many sessions

**Files:**
- Modify: `setup/dev1c-setup/src/cfg.rs` (`[model] memory`, `container_env` carries the slot numbers), `setup/dev1c-setup/src/llama.rs` (`cache_ram_mb`), `setup/dev1c-setup/src/ops_llama.rs` (`llama_run_args`), `setup/dev1c-setup/src/verify.rs` (the swap fact)
- Test: the same files

**Interfaces:**
- Produces: `llama::zed_numbers(slot_window: u32, agents: u16) -> ZedNumbers { window, max_output, max_tokens }`, `verify::declared_window_fact(declared: u32, available: u32) -> Check`, `cfg::Model::memory` (string like `22g`, empty means «посчитать»), `llama::container_memory_mb(total_mb: u64, env_limit_mb: u64, host_reserve_mb: u64) -> u32`, `llama::cache_ram_from_limit(limit_mb: u32) -> u32`, `verify::swap_fact(swap_total_mb: u64, limit_mb: u32) -> Check`, `llama::kv_bytes_per_token(kv_cache_mb: u32, window: u32) -> u32`, `llama::window_that_fits(free_vram_mb: u32, kv_bytes_per_token: u32, slots: u16) -> u32`, `llama::warm_sessions_mb(kv_bytes_per_token: u32, window: u32, sessions: u16) -> u32`.
- Consumes: `/proc/meminfo` on Linux for total and swap; the model server's own report of its KV cache size and window (the measurement step already runs it).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_window_that_fits_comes_from_the_model_not_from_a_table() {
    // Сервер сообщил: окно 65 536 токенов заняло 640 МиБ кэша →
    // 10 КиБ на токен (гибридная модель класса Qwen3.6-35B-A3B).
    assert_eq!(kv_bytes_per_token(640, 65536), 10 * 1024);
    // 13 ГиБ свободной видеопамяти, три слота: по 250k на слот и ещё остаётся.
    assert!(window_that_fits(13312, 10 * 1024, 3) >= 250_000);
    // 32 КиБ на токен (класс Qwen3.8-27B): три слота по 250k уже не помещаются.
    assert!(window_that_fits(13312, 32 * 1024, 3) < 250_000);
    // Девять тёплых сеансов по 250k при 10 КиБ — это 22 ГиБ.
    assert_eq!(warm_sessions_mb(10 * 1024, 250_000, 9), 21_972);
}

#[test]
fn what_zed_is_told_matches_one_slot_and_carries_the_answer_too() {
    // Слот 250k, один агент: окно слота целиком, ответ — пятая часть,
    // max_tokens — их сумма, иначе Zed упрётся в предел на длинном диалоге.
    let z = zed_numbers(250_000, 1);
    assert_eq!(z.window, 250_000);
    assert_eq!(z.max_output, 50_000);
    assert_eq!(z.max_tokens, 300_000);
    // Три агента у одного человека делят его слот.
    assert_eq!(zed_numbers(250_000, 3).window, 83_333);
}

#[test]
fn a_window_zed_believes_but_the_server_cannot_give_is_a_failed_fact() {
    let c = declared_window_fact(250_000, 180_000);
    assert!(!c.ok);
    assert!(c.detail.contains("250") && c.detail.contains("180"));
    assert!(declared_window_fact(180_000, 180_000).ok);
}

#[test]
fn the_model_container_gets_what_is_left_of_the_machine() {
    // Стенд: 32 ГБ всего, 8 ГБ среде, 2 ГБ хосту.
    assert_eq!(container_memory_mb(32768, 8192, 2048), 22528);
    // Маленькая машина не уходит в минус и не получает предел ниже пола.
    assert_eq!(container_memory_mb(8192, 8192, 2048), MIN_MODEL_MB);
}

#[test]
fn the_prompt_cache_is_half_the_container_not_half_the_machine() {
    assert_eq!(cache_ram_from_limit(22528), 8192); // упирается в потолок
    assert_eq!(cache_ram_from_limit(4096), 2048);
    assert_eq!(cache_ram_from_limit(1024), 1024); // не ниже пола
}

#[test]
fn too_little_swap_is_a_warning_with_both_numbers() {
    let c = swap_fact(2048, 22528);
    assert!(!c.ok && c.info, "это предупреждение, а не отказ");
    assert!(c.detail.contains("2") && c.detail.contains("22"));
    assert!(swap_fact(32768, 22528).ok);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features memory swap_fact cache_ram` — Expected: FAIL.

- [ ] **Step 3: Implement**

`[model] memory` joins the sample with a comment saying what empty means. `llama_run_args` gains `--memory <предел>`, `--memory-swap <удвоенный предел>` and `--memory-reservation <половина>`; `serve_args` takes `--cache-ram` from `cache_ram_from_limit`, whose ceiling rises from 8192 MiB to the container's own limit minus the server's working set — eight gigabytes is not enough for six sleeping 250k sessions.

The measurement step stores `kv_bytes_per_token` in the kit state the first time the server answers, and `window_that_fits` turns it into the two sentences a person needs: what fits, and what was asked for. Asking for more than fits stops the install with both numbers — never silently installs the smaller window. KV stays `q8_0` everywhere: `-ctk`/`-ctv` are not configurable (customer's decision 2026-10-19, `q4_0` degrades).

The swap fact reads `/proc/meminfo`; on Windows it is skipped (no `Check` at all, not a false pass). Its threshold is what the warm sessions need (`warm_sessions_mb`), not the container limit.

- [ ] **Step 4: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup/src
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): the model container gets a memory limit of its own"
```

---

### Task 9: Proof

**Files:**
- Modify: `.github/workflows/build.yml` (the `dev1c-smoke` job)

- [ ] **Step 1: CI installs from the Linux kit with the container form**

The smoke job already installs through the Linux binary. Point it at the Linux kit, with `serve = "docker"`, and assert that it reaches the card barrier and says so in Russian — the runner has no GPU, and that is the honest stopping point.

Run: the workflow. Expected: the job is green, its log contains the barrier text from Task 4, and no raw `docker:` error.

- [ ] **Step 2: A real server**

On a machine with an NVIDIA card (the pilot server, or ours): unpack the Linux kit, follow `УСТАНОВКА.md` as written, install as a non-root user in group `docker`, then again as root on a clean copy. Both must finish; `./dev1c-setup check` must be green; the model server must answer. Record what the page got wrong.

- [ ] **Step 3: The same server, by hand**

On a clean copy of the same machine, bring the environment up **without running our binary at all**: only the commands from the «Ручной путь» section, pasted in order. The result must be the same — the container runs, Gitea answers, the model answers. This is the step that proves the section is not decorative; whatever it is missing goes back into the generator, not into the page by hand.

- [ ] **Step 4: Commit the corrections**

```bash
git add -A
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "fix(kit): what the live run on Ubuntu showed"
```
