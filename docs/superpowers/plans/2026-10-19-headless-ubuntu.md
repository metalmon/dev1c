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
5. **The vGPU share shrinks after installation**: the environment was built around numbers measured when the card was whole; `check` must say the model no longer fits, with both numbers, instead of leaving Zed with a context that silently does not work. Test in Task 6.

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

- [ ] **Step 3: Build and push the new tag**

Run the workflow (`gh workflow run llama.yml`), about 40 minutes. Expected: a new `dev1c-llama:b11606` digest in ghcr. Note the digest in the task's ledger line — Task 2 pins it.

- [ ] **Step 4: Commit**

```bash
git add Dockerfile.llama .github/workflows/llama.yml
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "fix(llama): the model-server image can actually run"
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

### Task 7: The instruction the administrator reads

**Files:**
- Create: `config/kit/УСТАНОВКА.md` (source), copied into the Linux kit by `pack.sh`
- Modify: `pack.sh`

- [ ] **Step 1: Write the page**

One page, Russian, in this order: what the server needs before we start (Docker, GPU driver, the toolkit if it is not already there — from the kit), what to copy and where (`dev1c-image.tar.gz`, the `.gguf`), the three commands (`./dev1c-setup status`, `./dev1c-setup install`, `./dev1c-setup check`), what is normal to see, and where the log is. Every command ready to paste. No step the program cannot itself check and explain.

- [ ] **Step 2: Walk it yourself against the program's texts**

Read the page next to `dev1c-setup status` output on a machine without the kit files and confirm that each barrier the page mentions is a barrier the program itself prints. Fix whichever is wrong — the program wins.

- [ ] **Step 3: Commit**

```bash
git add config/kit pack.sh
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "docs(kit): the administrator's page travels in the Linux kit"
```

---

### Task 8: Proof

**Files:**
- Modify: `.github/workflows/build.yml` (the `dev1c-smoke` job)

- [ ] **Step 1: CI installs from the Linux kit with the container form**

The smoke job already installs through the Linux binary. Point it at the Linux kit, with `serve = "docker"`, and assert that it reaches the card barrier and says so in Russian — the runner has no GPU, and that is the honest stopping point.

Run: the workflow. Expected: the job is green, its log contains the barrier text from Task 4, and no raw `docker:` error.

- [ ] **Step 2: A real server**

On a machine with an NVIDIA card (the pilot server, or ours): unpack the Linux kit, follow `УСТАНОВКА.md` as written, install as a non-root user in group `docker`, then again as root on a clean copy. Both must finish; `./dev1c-setup check` must be green; the model server must answer. Record what the page got wrong.

- [ ] **Step 3: Commit the corrections**

```bash
git add -A
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "fix(kit): what the live run on Ubuntu showed"
```
