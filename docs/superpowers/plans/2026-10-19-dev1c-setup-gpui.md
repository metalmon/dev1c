# dev1c-setup GPUI window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `dev1c-setup`'s Tauri window with a GPUI one that looks like `1c-connect`'s — one light theme, an ordinary resizable window, the same five tabs — and drop Tauri from the crate.

**Architecture:** The window becomes three files (`src/gui/mod.rs` state and commands, `src/gui/panel.rs` view, `src/gui/window.rs` opening) modelled on the same files in `1c-connect`, which stays the reference implementation. Fonts and the palette move to `common/src/gui.rs` behind a new `gui` feature so both binaries embed them once. Operations, checks and the CLI are untouched: the window is a view over `ops::server(&cfg)`, `verify::run` and `common::run::*`, as it is today.

**Tech Stack:** Rust 2021, `gpui-kit` `=0.7.3` (GPUI + `gpui-component`), `futures` channels, `image`, `embed-resource`; `anyhow`, `serde`, `toml`, `clap` as now. No Tauri anywhere after Task 2.

**Spec:** `docs/superpowers/specs/2026-10-19-dev1c-setup-gpui-design.md`

## Global Constraints

- Every user-facing string is Russian; code comments and commit messages are English (repository rule, `README.md`).
- Commits use the identity `OktaPlus <oktacoder@gmail.com>` passed per command (`git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit`; this machine has no global git config) and end with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- **Build in CI, not on this laptop.** A cold GPUI build is 10–16 minutes here and gets OOM-killed in the background. Locally run only `cargo test --workspace --no-default-features -j 1` from `setup/`. For the real build: `git push -u origin worktree-setup-gpui` then `gh workflow run build.yml --ref worktree-setup-gpui`, watch with the Monitor tool at a 45 s poll.
- `gpui-kit` stays pinned at `=0.7.3`. Do not bump it, do not add a new dependency that is not already in `1c-connect`'s list.
- One theme: light. `Theme::change(ThemeMode::Light, None, cx)` and nothing else — no system appearance, no choice, no dark.
- The window never hardcodes the operation list: it renders whatever `ops::server(&cfg)` returned. The single exception, as today, is the status card looking up the operation whose `id` is `container`.
- Fluent device, copied from `1c-connect/src/gui/panel.rs`: layers instead of shadows, radii 4/8 px, the 4/8/12/16/24 spacing grid, IBM Plex Sans, `muted_foreground` only for placeholders.
- **No local GPUI build at all** (house rule: build in CI whatever can be built in CI). Therefore the window splits in two: `dev1c-setup/src/ui.rs` holds every pure decision (what the status card says, which tab opens, what the notice carries, which rows «Доступ» shows, whether edits are unsaved, whether closing may proceed) with no `gpui` import and no feature gate, so `cargo test --workspace --no-default-features` runs its tests locally in seconds; `dev1c-setup/src/gui/*` holds the GPUI views behind the `gui` feature and is proven by CI's Windows job. Three things stay CI-only: the palette test (it needs `gpui_kit::component::ThemeSet`), the rendering, and the live look.

## Review Focus

1. **`dev1c.toml` that does not parse** (hand-edited, lost a quote): the window must open with `Cfg::default()` on «Параметры», say what is wrong in the journal and warn that «Сохранить» overwrites the file — today the program dies before any window with the message going to an unattached console. Test in Task 3.
2. **Unsaved edits plus the header's primary button**: install runs off the saved file, so the window must refuse and point at «Сохранить» rather than silently installing the old values. Test in Task 6.
3. **An operation `Blocked` by an unfilled parameter** (`[model] file` empty): the notice bar must carry that operation's own text, and the primary button must not promise work it cannot do. Test in Task 4.
4. **`dev1c.toml` not writable** (read-only kit, file open elsewhere): «Сохранить» must say so and keep what the admin typed, not clear the fields. Test in Task 6.
5. **A worker thread panicking mid-operation**: `busy` must clear, the journal must get `PANIC_TEXT`, and the buttons must come back — otherwise the window is stuck in «идёт работа» forever. Test in Task 3.

---

### Task 1: Fonts and palette in `common`

**Files:**
- Create: `setup/common/src/gui.rs`
- Create: `setup/common/theme/1c.json` (git mv from `setup/1c-connect/theme/1c.json`)
- Create: `setup/common/fonts/IBMPlexSans-Regular.ttf`, `-SemiBold.ttf`, `LICENSE-OFL.txt` (git mv from `setup/1c-connect/fonts/`)
- Modify: `setup/common/Cargo.toml` (new optional dep `gpui-kit`, new feature `gui`), `setup/common/src/lib.rs` (`#[cfg(feature = "gui")] pub mod gui;`)
- Modify: `setup/1c-connect/Cargo.toml` (feature `gui` adds `common/gui`), `setup/1c-connect/src/gui/theme.rs`

**Interfaces:**
- Produces: `common::gui::LIGHT: &str = "1C Light"`, `common::gui::DARK: &str = "1C Dark"`, `common::gui::THEME_JSON: &str`, `common::gui::fonts(cx: &mut App)` (embeds both ttf), `common::gui::palette(cx: &mut App)` (parses `THEME_JSON`, puts both themes into `Theme::global_mut`, panics on a broken file as `1c-connect` does today).
- Consumes: nothing.

- [ ] **Step 1: Move the data files and point the old code at them**

`git mv` the json and the `fonts/` directory into `setup/common/`. `1c-connect/src/gui/theme.rs` keeps `Choice`, `MenuMode`, `menu_mode`, `apply_menu_theme`, `apply` and `install`, but `install` now calls `common::gui::fonts(cx)` and `common::gui::palette(cx)` instead of its own `include_str!`/`include_bytes!`.

- [ ] **Step 2: Move the palette test to `common` and run it**

The test `theme_file_has_one_light_and_one_dark_theme_with_the_fluent_radii` moves verbatim into `common/src/gui.rs` (it asserts the two theme names in order, `(radius, radius_lg, shadow) == (Some(4), Some(8), Some(false))`, `font_family == "IBM Plex Sans"`, `primary == "#FACC1F"`). The two remaining tests (`theme_choice_round_trips…`, `menu_mode_follows_the_choice…`) stay in `1c-connect`.

Run locally: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS (the palette test is not in this run: it needs gpui). The palette test itself is proven by CI's Windows job in Task 2 step 5.

- [ ] **Step 3: Verify `1c-connect` still compiles without the window**

Run: `cargo test --manifest-path setup/Cargo.toml -p onec-connect --no-default-features` — Expected: PASS. Its two theme tests (`theme_choice_round_trips…`, `menu_mode_follows_the_choice…`) live under the `gui` feature and are proven by CI in Task 2 step 5.

- [ ] **Step 4: Commit**

```bash
git add -A setup/common setup/1c-connect
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "refactor(common): fonts and the 1C palette move to common behind a gui feature"
```

---

### Task 2: Crate without Tauri, window that opens

**Files:**
- Delete: `setup/dev1c-setup/tauri.conf.json`, `setup/dev1c-setup/capabilities/`, `setup/dev1c-setup/gen/`, `setup/dev1c-setup/ui/`, `setup/dev1c-setup/src/gui.rs`
- Create: `setup/dev1c-setup/src/gui/mod.rs`, `setup/dev1c-setup/src/gui/window.rs`, `setup/dev1c-setup/src/gui/panel.rs`, `setup/dev1c-setup/icons/app.rc`
- Modify: `setup/dev1c-setup/Cargo.toml`, `setup/dev1c-setup/build.rs`

**Interfaces:**
- Consumes: `common::gui::{fonts, palette}` (Task 1); `Kit`, `Cfg::load`.
- Produces: `gui::launch(kit: Kit) -> anyhow::Result<()>` — the same signature `main.rs:118` already calls, so `main.rs` does not change. `gui::window::MIN_SIZE: Size<Pixels>`, `gui::window::open(cx: &mut App)`.

- [ ] **Step 1: Swap the dependencies**

In `Cargo.toml`: feature `gui = ["dep:gpui-kit", "dep:futures", "dep:image", "dep:embed-resource", "common/gui"]`; drop `tauri`, `tauri-plugin-dialog`, `tauri-build`. Copy the `gpui-kit`, `futures` and `image` lines verbatim from `setup/1c-connect/Cargo.toml`; `embed-resource` goes under `[target.'cfg(windows)'.build-dependencies]`. No `tray-icon`, no `raw-window-handle` — there is no tray and no HWND need here.

- [ ] **Step 2: Port `build.rs` and the resource script**

Copy `setup/1c-connect/build.rs` and `setup/1c-connect/icons/app.rc`, changing the icon path to `dev1c-setup`'s own `icons/icon.ico` and the manifest's assembly name. The `.rc` must keep the Windows manifest (DPI-aware, common-controls v6) that `tauri-build` used to supply.

- [ ] **Step 3: Open an empty window**

`gui::launch` loads the config, calls `gpui_kit::application().with_assets(...).run(...)`, then inside: `gpui_kit::init(cx)`, `common::gui::fonts(cx)`, `common::gui::palette(cx)`, `Theme::change(ThemeMode::Light, None, cx)`, and `window::open(cx)`.

`window::open` uses `WindowOptions { window_min_size: Some(MIN_SIZE), window_bounds: centered, kind: WindowKind::Normal, is_resizable: true, window_decorations: Some(WindowDecorations::Client), icon, ..TitleBar::window_options() }` and `gpui_kit::open_window`. `MIN_SIZE` is `size(px(760.), px(560.))`. Quit mode stays the default (closing the window ends the program); `cx.set_quit_mode(QuitMode::Explicit)` is `1c-connect`'s need, not ours.

The panel renders, for now, `TitleBar::new().child("dev1c-setup — среда разработки 1С")` над пустым полем.

- [ ] **Step 4: Verify nothing Tauri is left and the workspace builds without the window**

Run: `grep -ri tauri setup/dev1c-setup/` — Expected: no output.
Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS, same count as before this plan.

- [ ] **Step 5: Push the branch and build it in CI**

```bash
git add -A setup
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): GPUI window skeleton, Tauri out of the crate"
git push -u origin worktree-setup-gpui
gh workflow run build.yml --ref worktree-setup-gpui
```

Expected: the Windows job builds `dev1c-setup.exe` with the `gui` feature; the Linux job builds with `--no-default-features`.

---

### Task 3: State, events, worker threads

**Files:**
- Create: `setup/dev1c-setup/src/ui.rs` (pure logic and its tests, no gpui, no feature gate)
- Modify: `setup/dev1c-setup/src/gui/mod.rs`, `setup/dev1c-setup/src/main.rs` (`mod ui;`)

**Interfaces:**
- Consumes: `ops::server`, `ops::Ctx`, `verify::{run, summary, report}`, `common::run::{status, apply_all, clean_apply}`, `common::op::{OpState, CleanLevel}`, `cfg::{Cfg, FILE}`.
- Produces in `ui.rs` (no gpui, tested locally): `Line`, `Kind`, `OpView`, `Which`, `Busy`, `Dot`, `Tab`, `LOG_CAP: usize = 2000`, `LOG_FILE: &str = "dev1c-setup.log"`, `PANIC_TEXT`, `load_or_default(kit: &Kit) -> (Cfg, Option<String>)`, `op_view(id: String, title: String, st: OpState) -> OpView`, `notice(ops: &[OpView]) -> Option<String>`, `start_tab(file_exists: bool) -> Tab`, `guarded<E>(send: impl Fn(E), as_line: impl FnOnce(Line) -> E, body: impl FnOnce() -> E, on_panic: impl FnOnce() -> E)` — the sender is a closure precisely so the panic path is testable without a GPUI channel.
- Produces in `gui/mod.rs` (GPUI, proven by CI): `Ui` global with `working(&self) -> bool`, `Ev`, `handle(ev, cx)`, `refresh_ops(cx)`, `run_op(cx, which)`, `run_check(cx)`, `note_line(ui, line)`.

Shapes follow `1c-connect/src/gui/mod.rs` field for field where the meaning is the same (`kit`, `cfg`, `log`, `unread`, `tab`, `busy`, `started`, `pending`, `status`, `window`, `panel`, `dialog_open`, `tx`); `ops: Vec<OpView>` and `failed_checks: usize` are ours; `session`, `link`, `project`, `tray`, `height`, `hidden_at` do not exist here. `Which` is `Install | Change | CleanStack | CleanData | Check` with `title()` giving the Russian button label.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_broken_config_file_yields_defaults_and_the_reason() {
    // kit with dev1c.toml = "[server]\nname = \n"
    let (cfg, err) = load_or_default(&kit);
    assert_eq!(cfg.server.name, Cfg::default().server.name);
    assert!(err.unwrap().contains("dev1c.toml"));
}

#[test]
fn a_good_config_file_yields_no_reason() { /* err.is_none() */ }

#[test]
fn the_notice_carries_the_text_of_the_first_blocked_operation() {
    let ops = vec![op_view("model".into(), "Модель".into(), OpState::Blocked("модель не выбрана: [model] file".into()))];
    assert_eq!(notice(&ops).unwrap(), "модель не выбрана: [model] file");
    assert!(notice(&[op_view("env".into(), "Движок".into(), OpState::Missing)]).is_none());
}

#[test]
fn the_first_tab_is_operations_unless_the_file_is_gone() {
    assert_eq!(start_tab(true), Tab::Ops);
    assert_eq!(start_tab(false), Tab::Params);
}

#[test]
fn a_panicking_worker_still_reports_the_end_of_work_and_a_line() {
    // guarded(&tx, || panic!("boom"), || Ev::Done(None)) sends Ev::Line(PANIC_TEXT) then Ev::Done
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features ui::` — Expected: FAIL, functions not found.

- [ ] **Step 3: Implement the state, the event loop and the workers**

`Ev` is `Line(Line) | Done(Option<String>) | Checks(Vec<Check>) | Ops(Vec<OpView>)`; `handle(ev, cx)` mirrors `1c-connect`'s: `Line` notes and refreshes, `Done` clears `busy`, takes `started`, writes «Готово за …» or the error, then `refresh_ops`; `Checks` fills `failed_checks` and journal lines; `Ops` replaces the list and decrements `pending`.

Workers are `std::thread::spawn` + `guarded`, one per command, with the `Ctx` sink turning `common::Event` into `Ev::Line` exactly as `gui.rs:54-70` does today (including `Event::Progress(p) => p.phase.title()` and `kit.journal(&e)` first). `run_op` for `Which::Install` keeps today's tail: run checks, write the report, print each fact, fail if any failed.

- [ ] **Step 4: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features ui::` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A setup/dev1c-setup/src/gui
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): window state, events and worker threads"
```

---

### Task 4: Header, tabs, Операции, Журнал

**Files:**
- Modify: `setup/dev1c-setup/src/gui/panel.rs`, `setup/dev1c-setup/src/gui/mod.rs`

**Interfaces:**
- Consumes: Task 3's `Ui`, `OpView`, `Which`, `notice`.
- Produces in `ui.rs`: `status_line(ops: &[OpView], name: &str) -> (Dot, String, String, Option<Which>)` — indicator, title, subline, primary button; `Panel` with `Render`; `Panel::layer(bg, border)` helper copied from `1c-connect`.

- [ ] **Step 1: Write the failing test for the status card**

One test per row of the spec's table, asserting title and primary label (`ops` built with `op_view`, container operation id `container`):

```rust
#[test]
fn the_status_card_names_the_state_and_the_next_step() {
    assert_eq!(card(&[missing("container"), missing("image")]), ("Среда не установлена", Some(Which::Install)));
    assert_eq!(card(&[done("image"), missing("container")]), ("Среда установлена не полностью", Some(Which::Install)));
    assert_eq!(card(&[done("container"), drift("image")]), ("Среда работает, параметры разошлись", Some(Which::Change)));
    assert_eq!(card(&[done("container"), blocked("model")]), ("Среда работает, не всё настроено", None));
    assert_eq!(card(&[done("container"), done("image")]), ("Среда работает", None));
}
```

(`card` is a test helper over `status_line`; the labels come from `Which::title`: `Install` → «Установить» when nothing is done, «Доустановить» otherwise, `Change` → «Применить изменения». Keep that split inside `status_line` so the test above is the whole contract.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features ui::` — Expected: FAIL.

- [ ] **Step 3: Implement `status_line` and the four-element shell**

Header: icon, «dev1c-setup», status card (dot, title, subline, «Проверить» + primary), copied in device from `1c-connect`'s header plus status layer. Under it the notice bar, lit by `notice(&ops)` or by Task 3's config error. Then `TabBar` with five tabs and badges: Операции (count of `Missing`+`Drifted`, danger colour when any `Drifted`), Параметры, Доступ, Проверка (`failed_checks`), Журнал (`unread`). Footer: kit path or last status line; the thin progress line lives in the separator above it when `busy.is_some() || pending > 0`.

- [ ] **Step 4: Fill «Операции» and «Журнал»**

Операции: one row per `OpView` — dot by kind, title, detail; footer buttons «Снять контейнер» (`Which::CleanStack`) and «Удалить данные…», the second revealing an input that must equal `cfg.server.name` before the danger button runs `Which::CleanData`. Журнал: lines by `Kind`, section heads with `HH:MM`, «Очистить» in the footer, `LOG_CAP` truncation.

- [ ] **Step 5: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A setup/dev1c-setup/src
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): header, tabs, operations and journal"
```

---

### Task 5: Доступ and Проверка

**Files:**
- Modify: `setup/dev1c-setup/src/gui/panel.rs`

**Interfaces:**
- Consumes: `Cfg::ports()`, `Cfg::model_url()`, `llama::facts_load`, `verify::run` via Task 3's `run_check`.
- Produces in `ui.rs`: `access_rows(cfg: &Cfg) -> Vec<(&'static str, String, &'static str)>` — label, value, explanation.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_access_rows_follow_the_port_block_and_the_model_url() {
    let mut c = Cfg::default();
    c.server.port = 4000;
    let rows = access_rows(&c);
    assert!(rows.iter().any(|(_, v, _)| v.contains("4000")));   // RDP
    assert!(rows.iter().any(|(_, v, _)| v.contains("4001")));   // Gitea
    assert!(rows.iter().any(|(_, v, _)| v == &c.model_url()));
    assert!(rows.iter().any(|(_, v, _)| v.contains("4003")));   // SSH for 1c-connect
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features access_rows` — Expected: FAIL.

- [ ] **Step 3: Implement both tabs**

Доступ: the rows from `access_rows`, value in the monospace style the HTML used `<code>` for. Проверка: the `Check` list with a summary line and a «Проверить» button; failures in the danger colour with their hint; the report path goes to the journal, as today.

- [ ] **Step 4: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A setup/dev1c-setup/src/gui/panel.rs
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): access and check tabs"
```

---

### Task 6: Параметры with an explicit Save

**Files:**
- Modify: `setup/dev1c-setup/src/gui/panel.rs`, `setup/dev1c-setup/src/gui/mod.rs`

**Interfaces:**
- Consumes: `Cfg::{validate, render}`, `common::kit::write_text`, `cfg::FILE`.
- Produces: `Fields` (one `Entity<InputState>` per editable value plus the two switches/selects), `fields_to_cfg(f: &Fields, cx: &App) -> Cfg`, `ui::fields_differ(a: &Cfg, b: &Cfg) -> bool`, `ui::primary_blocked(dirty: bool) -> Option<&'static str>`, `save_cfg(ui: &mut Ui, cfg: Cfg) -> Result<(), String>`, `pick_model_file(...)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_form_of_the_model_decides_which_fields_travel() {
    // serve = "remote" clears file; serve = "host"/"docker" clears url and key,
    // the same rule app.js had, so validate never sees both
}

#[test]
fn unsaved_edits_are_seen_and_block_the_primary_button() {
    let saved = Cfg::default();
    let mut edited = saved.clone();
    edited.server.port = 4000;
    assert!(fields_differ(&saved, &edited));
    assert!(!fields_differ(&saved, &saved.clone()));
}

#[test]
fn a_rejected_config_keeps_the_fields_and_returns_the_russian_reason() {
    let mut c = Cfg::default();
    c.model.agents = 9;
    let e = save_cfg(&mut ui, c).unwrap_err();
    assert!(e.contains("agents"));
    // and dev1c.toml on disk is unchanged
}

#[test]
fn an_unwritable_file_is_reported_not_swallowed() {
    // read-only dev1c.toml: save_cfg returns Err mentioning the path
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features ui::` — Expected: FAIL.

- [ ] **Step 3: Implement the tab**

Two groups, «Контейнер» (name, port, memory, engine, GPU switch) and «Модель для агентов» (serve, file or url, name, key, port, agents, fit_ctx), with the hints from the old `index.html` word for word and the `data-form` show/hide rule as a Rust condition on `serve`. The file row carries the folder-icon button that calls `pick_model_file`; a chosen path whose extension is not `.gguf` produces a `Warn` line in the journal, and `dialog_open` guards the window while the picker is up. «Сохранить» is enabled only when `fields_differ`, writes through `Cfg::render`, then `refresh_ops`; `Cfg::validate` errors land in the panel's footer line.

- [ ] **Step 4: Guard the primary button**

When `fields_differ` is true, the header's primary button does not run: the footer says «Сначала сохраните параметры» and «Сохранить» gets the accent. Add the test that `primary_blocked(dirty: bool) -> Option<&'static str>` returns that text only when dirty.

- [ ] **Step 5: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A setup/dev1c-setup/src/gui
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): parameters tab with an explicit save"
```

---

### Task 7: Closing during work asks first

**Files:**
- Modify: `setup/dev1c-setup/src/gui/window.rs`, `setup/dev1c-setup/src/gui/panel.rs`

**Interfaces:**
- Consumes: `Ui::working()`.
- Produces: `ui::should_close(working: bool) -> bool`, the confirmation through `AlertDialog::new(cx).confirm()`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn closing_is_free_when_idle_and_asks_while_working() {
    assert!(ui::should_close(false));
    assert!(!ui::should_close(true));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --manifest-path setup/Cargo.toml -p dev1c-setup --no-default-features should_close` — Expected: FAIL.

- [ ] **Step 3: Implement the guard**

In `window::open`, register `window.on_window_should_close(...)`: when `Ui::working()` is false, return true; otherwise open `AlertDialog::new(cx).confirm().title("Установка не закончена").description("Прервать? Повторный запуск доустановит недостающее.")` with `on_ok` quitting and `on_cancel` doing nothing, and return false.

- [ ] **Step 4: Run the tests**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A setup/dev1c-setup/src/gui
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "feat(dev1c-setup): closing mid-operation asks for confirmation"
```

---

### Task 8: Documents, CI and the live look

**Files:**
- Modify: `docs/STATUS.md`, `docs/superpowers/specs/2026-10-16-setup-binaries-design.md`, `docs/superpowers/specs/2026-10-16-gpui-connect-design.md`, `.github/workflows/build.yml` (the comment on line 55)

- [ ] **Step 1: Correct every place that calls `dev1c-setup` a Tauri program**

`STATUS.md`: the installers row, the Tauri focus history note, and the Linux-build note about webkit2gtk, which no longer applies to either binary. `2026-10-16-setup-binaries-design.md`: the «Стек» decision. `2026-10-16-gpui-connect-design.md`: mark the non-goal «`dev1c-setup` не трогаем» as done, pointing at this plan's spec.

- [ ] **Step 2: Build the branch in CI and keep the artifacts**

```bash
git push
gh workflow run build.yml --ref worktree-setup-gpui
```

Expected: Windows job green with the `gui` feature, Linux job green with `--no-default-features`, the `dev1c-smoke` job still installing through the Linux binary.

- [ ] **Step 3: Look at the window on this machine**

Download the Windows artifact, run `dev1c-setup.exe` with no arguments next to a kit. Check: light window with its own Windows-style caption, five tabs, «Проверить» works, «Сохранить» only after an edit, closing during work asks. Compare side by side with `1c-connect.exe` and fix spacing against its panel, not against this plan.

- [ ] **Step 4: Commit**

```bash
git add -A docs .github
git -c user.name=OktaPlus -c user.email=oktacoder@gmail.com commit -m "docs: dev1c-setup is a GPUI program now"
```
