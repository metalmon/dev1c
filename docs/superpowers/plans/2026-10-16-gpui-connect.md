# Окно `1c-connect` на GPUI — план внедрения

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Заменить окно `1c-connect` на Tauri/WebView2 окном на GPUI с палитрой macOS Classic и устройством по Fluent, не меняя команд, сеанса и операций.

**Architecture:** `gui.rs` становится модулем `gui/` из четырёх файлов: `mod.rs` (запуск, глобал `Ui`, канал событий, сеанс и операции), `tray.rs` (трей на `tray-icon`/`muda`), `window.rs` (флайаут: показ, скрытие, положение), `panel.rs` (единственный вид `Panel`), плюс `theme.rs` (шрифты и тема). Всё, что приходит с других потоков (туннель, операции, проверка, трей), идёт в один `futures` канал `Ev`, который читает одна задача GPUI. Окно уничтожается при скрытии и строится заново; состояние живёт в глобале.

**Tech Stack:** Rust 2021, `gpui-kit =0.7.3` (GPUI `gpui-pre 0.3.9`, `gpui-component 0.7.3`), `tray-icon 0.26` + `muda`, `futures 0.3`, `windows-sys 0.60`, `image 0.25` (уже в дереве через GPUI), `embed-resource` (уже в дереве через `gpui-pre`).

**Spec:** `docs/superpowers/specs/2026-10-16-gpui-connect-design.md`. Рабочий образец всех решений — `spike-gpui/src/main.rs` и `spike-gpui/zed-macos-classic.json` в этой же ветке (не коммитится; читать, не копировать целиком).

## Global Constraints

- Фича `gui` остаётся в `default`; Linux-сборка `--no-default-features` обязана собираться и проходить тесты без GPUI.
- Версии пинуются точно: `gpui-kit = "=0.7.3"`, `tray-icon = "=0.26.1"`, `futures = "0.3"`, `windows-sys = "0.60"` c фичами `Win32_Foundation`, `Win32_UI_WindowsAndMessaging`, `Win32_UI_HiDpi`.
- Никаких новых зависимостей сверх перечисленных; `image` и `embed-resource` берём потому, что они уже в дереве.
- Окно 380×640 логических px, отступ от угла рабочей области 12 px, окно `PopUp` + `Client`, не растягивается, не сворачивается.
- Радиусы 8 px (окно, карточки) и 4 px (элементы); отступы 12/8/4; шрифт 14, подписи 12, моно 13; тени выключены.
- Акцент только у включённого переключателя, одной главной кнопки вкладки и кольца фокуса; цвета — из `theme/macos-classic.json`, других констант цвета в коде нет.
- Параметры сохраняются кнопкой «Сохранить» (решение заказчика 2026-10-18, заменяет автосохранение); подключение — переключателем в карточке состояния.
- Журнал не длиннее 2000 строк; события `Command` и `Output` в окно не попадают.
- Строки интерфейса на русском, как в `gui.rs`/`index.html` сегодня; тексты ошибок из `start_session` переносятся дословно.
- Коммиты: идентичность через `GIT_AUTHOR_NAME=OktaPlus GIT_AUTHOR_EMAIL=oktacoder@gmail.com` (+ `GIT_COMMITTER_*`) в том же вызове, сообщение из файла.

## Review Focus

- Второй монитор или масштаб 100 %: окно обязано целиком лежать в рабочей области; начало координат не уходит в минус (`flyout_origin`, задача 4).
- Ошибка валидации при автосохранении: toml не перезаписан, набранное остаётся в поле, причина видна в нижней плашке (`save_host`, задача 3).
- Сеанс упал, пока окно спрятано: строка в журнал, трей серый, бейдж «Журнал» растёт, при следующем показе человек видит причину (`note_line`, задача 3).
- Щелчок по значку трея сразу после потери фокуса не открывает окно заново (`should_show`, задача 4).
- «Выход» при поднятом туннеле: сначала `Session::stop`, потом `cx.quit()`; туннель не остаётся сиротой (`quit`, задача 3, проверяется живым тестом задачи 7).

---

### Task 1: Зависимости, каркас `gui/`, значок exe

**Files:**
- Modify: `setup/1c-connect/Cargo.toml`
- Rewrite: `setup/1c-connect/build.rs`
- Create: `setup/1c-connect/icons/app.rc`
- Delete: `setup/1c-connect/src/gui.rs`, `setup/1c-connect/tauri.conf.json`, `setup/1c-connect/capabilities/`, `setup/1c-connect/gen/`, `setup/1c-connect/ui/`
- Create: `setup/1c-connect/src/gui/mod.rs`
- Test: `setup/1c-connect/src/gui/mod.rs` (`mod tests`)

**Interfaces:**
- Produces: `pub fn launch(kit: Kit) -> anyhow::Result<()>` (сигнатура прежняя, `main.rs` не меняется); `pub fn badge(base: &image::RgbaImage, up: bool) -> image::RgbaImage`; `pub fn web_url(cfg: &Cfg) -> String`; `pub const TRAY_PNG: &[u8]`.

- [ ] **Step 1: Перенести два существующих теста в новый модуль как падающие**

В `src/gui/mod.rs` оставить только `#[cfg(test)] mod tests` с тестами из старого `gui.rs`, переписанными под `image`:

```rust
#[test]
fn the_badge_sits_in_the_corner_and_says_up_or_down() {
    let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
    let up = badge(&base, true);
    assert_eq!(up.dimensions(), base.dimensions());
    let w = up.width() as usize;
    let at = |img: &image::RgbaImage, x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[..3].to_vec();
    let c = w - 1 - (w as f32 * 0.22) as usize;
    assert_eq!(at(&up, c, c), vec![0x2f, 0x8f, 0x4e]);
    let down = badge(&base, false);
    assert_eq!(at(&down, c, c), vec![0x8d, 0x86, 0x7e]);
    let p = at(&down, 2, 2);
    assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
}

#[test]
fn web_client_url_follows_the_publication() { /* как в старом gui.rs, без изменений */ }
```

- [ ] **Step 2: Запустить тесты, убедиться, что не компилируется**

Run: `cargo test --manifest-path setup/Cargo.toml -p onec-connect gui::`
Expected: ошибка компиляции — `badge`, `web_url`, `TRAY_PNG` не найдены.

- [ ] **Step 3: Переписать `Cargo.toml`**

Фича `gui = ["dep:gpui-kit", "dep:tray-icon", "dep:futures", "dep:image"]`; убрать `tauri`, `tauri-plugin-dialog`, `[build-dependencies] tauri-build`. Добавить:

```toml
gpui-kit = { version = "=0.7.3", optional = true }
tray-icon = { version = "=0.26.1", default-features = false, optional = true }
futures = { version = "0.3", optional = true }
image = { version = "0.25", default-features = false, features = ["png"], optional = true }

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.60", features = ["Win32_Foundation", "Win32_UI_WindowsAndMessaging", "Win32_UI_HiDpi"] }

[target.'cfg(windows)'.build-dependencies]
embed-resource = { version = "3", optional = true }
```

`windows-sys` уже есть в проекте — объединить фичи, не дублировать запись. Комментарий про webkit2gtk в шапке заменить на: Linux собирается без окна, потому что GPUI-на-Linux нам не нужен.

- [ ] **Step 4: `build.rs` и `icons/app.rc`**

`icons/app.rc` — одна строка: `1 ICON "icon.ico"`. `build.rs`:

```rust
fn main() {
    #[cfg(all(windows, feature = "gui"))]
    embed_resource::compile("icons/app.rc", embed_resource::NONE).manifest_optional().unwrap();
}
```

Фича `gui` для `embed-resource`: добавить `"dep:embed-resource"` в список `gui` (cargo принимает build-dependency в фиче).

- [ ] **Step 5: Удалить файлы Tauri и написать каркас `gui/mod.rs`**

Удалить перечисленное в **Files**. В `gui/mod.rs`: `pub const TRAY_PNG: &[u8] = include_bytes!("../../icons/tray.png");`, `badge` (тело из старого `gui.rs`, вход/выход `image::RgbaImage`), `web_url` (без изменений), и временный `launch`, который открывает пустое окно GPUI 380×640 c `gpui_kit::application().run(..)`, `gpui_kit::init(cx)`, `cx.set_quit_mode(QuitMode::Explicit)`. `lib.rs` не меняется (`#[cfg(feature = "gui")] pub mod gui;`).

- [ ] **Step 6: Тесты и обе сборки зелёные**

Run: `cargo test --manifest-path setup/Cargo.toml -p onec-connect gui::` → 2 passed.
Run: `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features` → как до изменений (74+39+13+2 passed).
Run: `cargo build --release --manifest-path setup/Cargo.toml -p onec-connect` → собирается; записать размер `setup/target/release/1c-connect.exe` в `docs/STATUS.md` (раздел «Working state», строка про `installers`).

- [ ] **Step 7: Commit**

`chore(1c-connect): swap Tauri for gpui-kit, keep badge and web_url, embed exe icon via embed-resource`

---

### Task 2: Тема и шрифты

**Files:**
- Create: `setup/1c-connect/theme/macos-classic.json` (из `spike-gpui/zed-macos-classic.json`, как есть)
- Create: `setup/1c-connect/fonts/IBMPlexSans-Regular.ttf`, `IBMPlexSans-SemiBold.ttf`, `Lilex-Regular.ttf`, `LICENSE-OFL.txt` (файлы из `spike-gpui/fonts/`; лицензия — текст OFL 1.1 с упоминанием IBM Plex и Lilex)
- Create: `setup/1c-connect/src/gui/theme.rs`
- Test: там же

**Interfaces:**
- Produces: `pub fn install(kit: &Kit, cx: &mut App)` — шрифты, загрузка тем, `light_theme`/`dark_theme`, затем `apply(Choice::load(kit), cx)`; `pub const LIGHT: &str = "macOS Classic Light"`; `pub const DARK: &str = "macOS Classic Dark"`; `pub const THEME_JSON: &str`; `pub const STATE_KEY: &str = "theme"`;

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)] pub enum Choice { #[default] System, Light, Dark }
impl Choice {
    pub fn parse(s: &str) -> Choice;                 // "light" | "dark" → соответствующее, всё прочее → System
    pub fn as_str(self) -> &'static str;             // "system" | "light" | "dark"
    pub fn load(kit: &Kit) -> Choice;                // kit.state_get(STATE_KEY)
    pub fn store(self, kit: &Kit) -> anyhow::Result<()>;
}
pub fn apply(choice: Choice, cx: &mut App);          // System → Theme::sync_system_appearance(None, cx); Light/Dark → Theme::change(ThemeMode::..)
```

- [ ] **Step 1: Падающий тест**

```rust
#[test]
fn theme_file_has_one_light_and_one_dark_theme_with_the_fluent_radii() {
    let set: gpui_kit::component::ThemeSet = serde_json::from_str(THEME_JSON).unwrap();
    let names: Vec<_> = set.themes.iter().map(|t| (t.name.to_string(), t.mode.is_dark())).collect();
    assert_eq!(names, vec![(DARK.into(), true), (LIGHT.into(), false)]);
    for t in &set.themes {
        assert_eq!((t.radius, t.radius_lg, t.shadow), (Some(4), Some(8), Some(false)));
        assert_eq!(t.font_family.as_deref(), Some("IBM Plex Sans"));
        assert!(t.colors.primary.is_some() && t.colors.ring.is_some());
    }
}

#[test]
fn theme_choice_round_trips_through_kit_state_and_defaults_to_system() {
    let kit = Kit::at(tempfile::tempdir().unwrap().path());
    assert_eq!(Choice::load(&kit), Choice::System);
    Choice::Dark.store(&kit).unwrap();
    assert_eq!(Choice::load(&kit), Choice::Dark);
    assert_eq!(Choice::parse("garbage"), Choice::System);
}
```

`tempfile` — проверить, что он уже в `[dev-dependencies]` workspace; если нет, временная папка через `std::env::temp_dir()` с уникальным именем, новых зависимостей не добавлять.

- [ ] **Step 2: Запустить, убедиться, что падает** (`THEME_JSON` не найден).

- [ ] **Step 3: Реализовать `theme.rs`**

`install`: `cx.text_system().add_fonts(vec![Cow::Borrowed(include_bytes!(..).as_slice()), ..])` для трёх файлов; `ThemeRegistry::global_mut(cx).load_themes_from_str(THEME_JSON)`; взять `themes()[LIGHT]`/`[DARK]`, положить в `Theme::global_mut(cx).light_theme/dark_theme`; `apply(Choice::load(kit), cx)`. Звать из `launch` сразу после `gpui_kit::init(cx)`. `Panel::new` (задача 6) при `Choice::load(kit) == System` зовёт `Theme::sync_system_appearance(Some(window), cx)`, чтобы смена темы Windows подхватывалась при следующем показе.

- [ ] **Step 4: Тест зелёный**, `cargo test -p onec-connect gui::theme`.

- [ ] **Step 5: Commit** — `feat(1c-connect): macOS Classic theme and Zed fonts for the GPUI window`

---

### Task 3: Глобал `Ui`, канал событий, сеанс и операции

**Files:**
- Modify: `setup/1c-connect/src/gui/mod.rs`
- Test: там же

**Interfaces:**
- Consumes: `session::{start, Session, State}`, `cfg::{Cfg, Host, FILE}`, `ops_host::{self, Ctx}`, `verify`, `common::{Event, Sink, check::Check, run, op::{OpState, CleanLevel}, kit::write_text, AlwaysDefault}`.
- Produces (всё `pub(crate)` в `gui`):

```rust
pub struct Ui {
    pub kit: Kit, pub cfg: Cfg, pub session: Option<Session>, pub link: State,
    pub log: Vec<Line>, pub unread: usize, pub tab: Tab,
    pub checks: Vec<Check>, pub failed_checks: usize, pub ops: Vec<OpView>,
    pub busy: Option<Busy>, pub status: Option<Line>,
    pub tray: Option<tray::Tray>, pub window: Option<WindowHandle<panel::Panel>>,
    pub hidden_at: Option<Instant>, pub dialog_open: bool,
    pub tx: futures::channel::mpsc::UnboundedSender<Ev>,
}
impl Global for Ui {}
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Kind { Ok, Warn, Fail, Say }
#[derive(Clone, PartialEq, Debug)] pub struct Line { pub kind: Kind, pub text: String }
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)] pub enum Tab { #[default] Params, Ext, Check, Log }
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Busy { Ext, Check }
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Dot { Off, Busy, Up, Down }
#[derive(Clone, Debug)] pub struct OpView { pub id: String, pub title: String, pub dot: Dot, pub detail: String }
pub enum Ev { Tray(tray_icon::TrayIconEvent), Menu(tray_icon::menu::MenuEvent), Link(State), Line(Line), Done(Option<String>), Checks(Vec<Check>) }
pub const LOG_CAP: usize = 2000;

pub fn line_of(e: &Event) -> Option<Line>;                 // Command/Output → None
pub fn link_line(l: &State, host: &Host) -> Option<Line>;  // тексты из set_link сегодняшнего gui.rs
pub fn note_line(ui: &mut Ui, line: Line);                 // push с потолком LOG_CAP; unread += 1, если tab != Log или window.is_none()
pub fn op_view(id: String, title: String, st: OpState) -> OpView; // Done→Up, Missing→Off «не установлено», Drifted→Busy «было: … → станет: …», Blocked→Down
pub fn save_host(ui: &mut Ui, mut host: Host) -> Result<(), String>; // project/tools из ui.cfg; validate; write_text(kit.path(FILE), render); ui.cfg = cfg
pub fn start_session(cx: &mut App) -> Result<(), String>;  // тексты ошибок из старого start_session дословно
pub fn stop_session(cx: &mut App);                          // Session::stop в std::thread, set_link(Off) сразу
pub fn toggle_link(cx: &mut App);
pub fn run_op(cx: &mut App, uninstall: bool);               // поток; Ev::Line по ходу; Ev::Done в конце
pub fn run_check(cx: &mut App);                             // поток; verify::run + write last-check.txt; Ev::Checks
pub fn refresh_ops(cx: &mut App);                           // common::run::status → ui.ops
pub fn open_web(cx: &mut App);                              // cx.open_url(web_url(&ui.cfg))
pub fn quit(cx: &mut App);                                  // stop_session, затем cx.quit()
fn handle(ev: Ev, cx: &mut App);
fn ctx_for(ui: &Ui) -> Ctx;                                 // sink: kit.journal(&e) → иначе line_of → Ev::Line
pub fn launch(kit: Kit) -> anyhow::Result<()>;
```

- [ ] **Step 1: Падающие тесты для чистых функций**

```rust
#[test]
fn command_and_output_never_reach_the_window() {
    assert_eq!(line_of(&Event::Command("x".into())), None);
    assert_eq!(line_of(&Event::Output { command: "x".into(), text: "y".into() }), None);
    assert_eq!(line_of(&Event::Warn("w".into())), Some(Line { kind: Kind::Warn, text: "w".into() }));
}

#[test]
fn the_log_keeps_the_last_two_thousand_lines_and_counts_unread() {
    let mut ui = Ui::for_test(); // kit во временной папке, tab = Params, window = None
    for i in 0..LOG_CAP + 5 { note_line(&mut ui, Line { kind: Kind::Say, text: i.to_string() }); }
    assert_eq!(ui.log.len(), LOG_CAP);
    assert_eq!(ui.log[0].text, "5");
    assert_eq!(ui.unread, LOG_CAP + 5);
    ui.tab = Tab::Log; ui.unread = 0; // окна нет — строки всё равно непрочитанные
    note_line(&mut ui, Line { kind: Kind::Say, text: "x".into() });
    assert_eq!(ui.unread, 1);
}

#[test]
fn a_bad_server_leaves_the_toml_alone_and_says_why() {
    let mut ui = Ui::for_test();
    let before = std::fs::read_to_string(ui.kit.path(cfg::FILE)).ok();
    let mut host = ui.cfg.host.clone();
    host.server = "no-port".into();
    let err = save_host(&mut ui, host).unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(std::fs::read_to_string(ui.kit.path(cfg::FILE)).ok(), before);
}

#[test]
fn link_lines_say_where_and_as_whom() {
    let h = Host { server: "s:1".into(), workstation: "dev2".into(), ..Default::default() };
    assert_eq!(link_line(&State::Connecting, &h).unwrap().text, "подключаюсь к s:1 как dev2");
    assert_eq!(link_line(&State::Off, &h).unwrap().text, "отключено");
    assert!(link_line(&State::Up { since: Instant::now() }, &h).is_none());
}
```

`Ui::for_test()` — `#[cfg(test)]`, `Kit::at(tempdir)`, `Cfg::default()`, `tray: None`, канал создаётся и отбрасывается. Что именно `validate` отвергает — посмотреть в `cfg.rs:91`; подобрать заведомо негодный `server`.

- [ ] **Step 2: Запустить, убедиться, что падают** (типы не найдены).

- [ ] **Step 3: Реализовать перечисленное в Interfaces**

`launch`: `application().with_assets(gpui_kit::assets::Assets).run(|cx| { gpui_kit::init(cx); theme::install(&kit, cx); cx.set_quit_mode(QuitMode::Explicit); канал; tray::build; cx.set_global(Ui{..}); задача-читатель канала → handle; refresh_ops; window::show_window(cx) })`. Пока задач 4 и 5 нет, `tray::build` и `window::show_window` — заглушки (`Ok(None)` / пустое окно из задачи 1). `handle`: `Tray`/`Menu` → `tray::on_tray`/`on_menu` (задача 5); `Link(l)` → `set_link`; `Line` → `note_line` + `refresh_windows`; `Done(err)` → `busy = None`, `status`, `refresh_ops`; `Checks` → `checks`, `failed_checks`, `busy = None`, `status` («Провалено: N. Отчёт — last-check.txt в комплекте» / «Все факты в порядке. Отчёт — last-check.txt»). `set_link`: `ui.link = l`, `link_line` → `note_line`, `tray.set_state(l.is_up(), l.is_on())`, `refresh_windows`. `on_state` для `session::start` — `move |l| { let _ = tx.unbounded_send(Ev::Link(l)); }`.

- [ ] **Step 4: Тесты зелёные**, `cargo test -p onec-connect gui::` (6 passed), `--no-default-features` по-прежнему зелёный.

- [ ] **Step 5: Commit** — `feat(1c-connect): Ui global, one event channel, session and ops wired into the GPUI window`

---

### Task 5: Трей

**Files:**
- Create: `setup/1c-connect/src/gui/tray.rs`
- Test: там же

**Interfaces:**
- Consumes: `badge`, `TRAY_PNG`, `Ev`, `toggle_link`, `run_check`, `open_web`, `quit`, `window::{show_window, toggle_window}`.
- Produces:

```rust
pub struct Tray { icon: tray_icon::TrayIcon, toggle: tray_icon::menu::MenuItem, themes: [tray_icon::menu::CheckMenuItem; 3] }
pub fn build(tx: UnboundedSender<Ev>, choice: theme::Choice) -> anyhow::Result<Tray>;  // меню: toggle «Подключить», check «Проверить», web «Открыть веб-клиент», разделитель, Submenu «Тема» {theme:system «Системная», theme:light «Светлая», theme:dark «Тёмная» — CheckMenuItem, отмечен choice}, разделитель, quit «Выход»; левый щелчок без меню; set_event_handler ×2 → tx
impl Tray {
    pub fn set_state(&self, up: bool, on: bool);                 // значок badge(up), подсказка «1c-connect: подключён/не подключён», надпись toggle «Отключить»/«Подключить»
    pub fn set_theme(&self, choice: theme::Choice);              // отмечен ровно один пункт
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum MenuAction { Toggle, Check, Web, Theme(theme::Choice), Quit }
pub fn menu_action(id: &str) -> Option<MenuAction>;             // "theme:light" → Theme(Light) и т. д.
pub fn icon(up: bool) -> tray_icon::Icon;                       // image::load_from_memory(TRAY_PNG) → badge → Icon::from_rgba
pub fn on_tray(ev: TrayIconEvent, cx: &mut App);                // Click{Left, Up} → window::toggle_window
pub fn on_menu(ev: MenuEvent, cx: &mut App);                    // Toggle → toggle_link (ошибка → note_line Fail + show_window); Check → show_window + ui.tab = Check + run_check; Web → open_web; Theme(c) → c.store(kit), theme::apply(c, cx), tray.set_theme(c); Quit → quit
```

- [ ] **Step 1: Падающие тесты**

```rust
#[test]
fn menu_ids_map_to_actions_and_unknown_ids_to_nothing() {
    assert_eq!(menu_action("toggle"), Some(MenuAction::Toggle));
    assert_eq!(menu_action("quit"), Some(MenuAction::Quit));
    assert_eq!(menu_action("theme:dark"), Some(MenuAction::Theme(theme::Choice::Dark)));
    assert_eq!(menu_action("theme:system"), Some(MenuAction::Theme(theme::Choice::System)));
    assert_eq!(menu_action("nope"), None);
}

#[test]
fn tray_icon_is_the_badged_png() {
    let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
    assert_eq!(icon_rgba(true).len(), (base.width() * base.height() * 4) as usize);
}
```

`icon_rgba(up) -> Vec<u8>` — внутренняя функция, из которой `icon` делает `Icon::from_rgba(.., w, h)`; тест не трогает Win32.

- [ ] **Step 2: Запустить, убедиться, что падают.**
- [ ] **Step 3: Реализовать `tray.rs`** по образцу `spike-gpui/src/main.rs` (`build_tray`, `on_tray`, `on_menu`, обработчики через `set_event_handler`); подменю — `muda::Submenu::with_items("Тема", true, &[..])`, пункты — `CheckMenuItem::with_id(id, text, true, checked, None)`, `set_theme` — `set_checked` на всех трёх. В `launch` (задача 3) заменить заглушку на `Some(tray::build(tx.clone(), theme::Choice::load(&kit))?)`.
- [ ] **Step 4: Тесты зелёные.**
- [ ] **Step 5: Commit** — `feat(1c-connect): tray on tray-icon/muda with the badged icon and one-item toggle`

---

### Task 4: Флайаут: показ, скрытие, положение

**Files:**
- Create: `setup/1c-connect/src/gui/window.rs`
- Test: там же

**Interfaces:**
- Consumes: `Ui`, `panel::Panel`.
- Produces:

```rust
pub const SIZE: Size<Pixels>;                       // 380×640
pub const GAP: Pixels;                              // 12
pub const JUST_HIDDEN: Duration;                    // 400 мс
pub fn flyout_origin(work_area: Bounds<Pixels>, size: Size<Pixels>, gap: Pixels) -> Point<Pixels>; // правый нижний угол, не меньше origin рабочей области
pub fn work_area(cx: &App) -> Bounds<Pixels>;       // Windows: SPI_GETWORKAREA / GetDpiForSystem; иначе primary_display().bounds()
pub fn should_show(hidden_at: Option<Instant>, now: Instant) -> bool;
pub fn show_window(cx: &mut App);                   // есть окно → activate_window; иначе open_window(PopUp, Client, no titlebar, not resizable/minimizable, focus, show, icon из icons/icon.png) → ui.window
pub fn hide_window(window: &mut Window, cx: &mut App); // ui.window = None, hidden_at = now, remove_window
pub fn toggle_window(cx: &mut App);                 // should_show → show_window
```

- [ ] **Step 1: Падающие тесты**

```rust
#[test]
fn the_flyout_sits_in_the_corner_and_never_leaves_the_work_area() {
    let wa = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(700.)));
    assert_eq!(flyout_origin(wa, SIZE, GAP), point(px(608.), px(48.)));
    let tiny = Bounds::new(point(px(100.), px(50.)), size(px(300.), px(300.)));
    assert_eq!(flyout_origin(tiny, SIZE, GAP), point(px(100.), px(50.)));
}

#[test]
fn a_click_right_after_hiding_means_close_not_reopen() {
    let now = Instant::now();
    assert!(should_show(None, now));
    assert!(!should_show(Some(now - Duration::from_millis(100)), now));
    assert!(should_show(Some(now - Duration::from_millis(900)), now));
}
```

- [ ] **Step 2: Запустить, убедиться, что падают.**
- [ ] **Step 3: Реализовать `window.rs`** по образцу спайка (`flyout_bounds`, `show_window`, `hide_window`, `toggle_window`); `open_window` — `gpui_kit::open_window(opts, cx, |w, cx| cx.new(|cx| Panel::new(w, cx)))`; создать `panel.rs` с `Panel { focus: FocusHandle }` и `Render`, рисующим пустой `div()`, — задача 6 его заполнит.
- [ ] **Step 4: Тесты зелёные.**
- [ ] **Step 5: Commit** — `feat(1c-connect): flyout window placement, hide on blur, 400 ms tray-click rule`

---

### Task 6: Вид `Panel`

**Files:**
- Create/Modify: `setup/1c-connect/src/gui/panel.rs`
- Test: там же

**Interfaces:**
- Consumes: `Ui`, `Tab`, `Dot`, `Line`, `Kind`, `State`, `Check`, `OpView`, `save_host`, `toggle_link`, `run_op`, `run_check`, `window::hide_window`, `gpui_kit::component::{button, input, switch, tab, select}`, `ActiveTheme`, `h_flex`, `v_flex`.
- Produces:

```rust
pub struct Panel { focus: FocusHandle, fields: Fields }
pub struct Fields { server, password, base_path, db_user, db_password, base_url, dump: Entity<InputState>, workstation: /* Select state или сегмент dev1/dev2/dev3 */ }
impl Panel { pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self; }
pub fn status_line(l: &State, ports: &cfg::Ports) -> (Dot, &'static str, String); // Off→(Off,"Не подключён","туннель в среду разработки выключен"); Connecting→(Busy,"Подключаюсь…",""); Up→(Up,"Подключён","N мин · mcp-1c 777N · web 779N" — порты через аргумент Ports); Retrying(w)→(Busy,"Переподключаюсь",w); Failed(w)→(Down,"Не подключён",w)
pub fn check_dot(c: &Check) -> Dot;                           // ok&&!info→Up, ok&&info→Off, !ok→Down
pub fn host_of(f: &Fields, cx: &App) -> Host;                 // значения полей; project/tools пустые — их подставит save_host
```

- [ ] **Step 1: Падающие тесты**

```rust
#[test]
fn status_line_reads_like_the_old_window() {
    let p = cfg::Ports { mcp: 7771, vrunner: 7781, web: 7791 };
    assert_eq!(status_line(&State::Off, &p), (Dot::Off, "Не подключён", "туннель в среду разработки выключен".into()));
    assert_eq!(status_line(&State::Failed("нет сети".into()), &p), (Dot::Down, "Не подключён", "нет сети".into()));
    let (d, t, s) = status_line(&State::Up { since: Instant::now() }, &p);
    assert_eq!((d, t), (Dot::Up, "Подключён"));
    assert_eq!(s, "0 мин · mcp-1c 7771 · web 7791");
}

#[test]
fn check_dots_tell_fail_from_note() {
    assert_eq!(check_dot(&Check::pass("a", "")), Dot::Up);
    assert_eq!(check_dot(&Check::note("a", "")), Dot::Off);
    assert_eq!(check_dot(&Check::fail("a", "", "")), Dot::Down);
}
```

- [ ] **Step 2: Запустить, убедиться, что падают.**

- [ ] **Step 3: Реализовать `Panel::new`**

Поля — `cx.new(|cx| InputState::new(window, cx).placeholder(..).default_value(..))` из `ui.cfg.host`; пароли `.masked(true)`; плейсхолдеры из `index.html` (`192.168.1.10:3393`, `C:\Bases\Buh`, `http://localhost/Buh/hs/mcp-1c`). Рабочее место — `Select` из `gpui-component` со значениями `dev1/dev2/dev3`; если его API требует больше 20 строк обвязки, взять сегментный `TabBar::segmented()` из трёх `Tab` — решение строителя, в коде одной строкой комментария почему. На каждое поле `cx.subscribe(&state, |this, _, ev: &InputEvent, cx| ..)`: `Blur` или `PressEnter` → `save_host(ui, host_of(..))`; `Ok` → `status = Line{Ok,"Сохранено в 1c-connect.toml"}`, `Err(e)` → `status = Line{Fail,e}`; `refresh_ops`. `cx.observe_window_activation` → если `!is_window_active() && !dialog_open` → `hide_window`. Таймер минут: `cx.spawn` раз в 30 с `cx.notify()`, пока окно живо.

- [ ] **Step 4: Реализовать `render`**

Дерево — как в спайке после Fluent-правок (`spike-gpui/src/main.rs`, `impl Render for Panel`), с данными из `Ui` вместо заглушек:

- шапка: значок 24 px (`img` из `icons/icon.png`, `size_6`), «1c-connect» SemiBold, ✕ ghost compact → `hide_window`;
- карточка-слой: кружок по `Dot` (`Off`→`muted_foreground`, `Busy`→`primary`, `Up`→`success`, `Down`→`danger`), заголовок SemiBold, подстрока 12 px, `Switch` (`checked = link.is_on()`, `on_change` → `toggle_link`);
- `TabBar::underline()` с `Tab::suffix` бейджами: Проверка — `failed_checks` (если > 0), Журнал — `unread` (если > 0 и вкладка не открыта); щелчок → `ui.tab`, при `Log` → `unread = 0`;
- тело по `ui.tab`: Params — поля по спеке (сервер; рабочее место + пароль в ряд; база с кнопкой «Выбрать…» `.outline()`; пользователь базы + пароль в ряд; адрес публикации; каталог выгрузки с «Выбрать…»); Ext — `ui.ops` слоями; Check — подсказка или `ui.checks` (`what`, `detail`, при `Down` ещё `hint` 12 px); Log — `div().id("log").overflow_y_scroll()`, `font_family("Lilex")`, цвет по `Kind` (`Ok`→`success`, `Warn`→`warning`, `Fail`→`danger`, `Say`→`foreground`), прокрутка вниз при новых строках (`ScrollHandle::scroll_to_bottom` после `notify`);
- подвал: слева `ui.status` или путь комплекта 12 px; справа по вкладке: Params — «Подключить»/«Отключить» (`.primary()`); Ext — «Снять» (`.outline()`) + «Установить» (`.primary()`), обе `disabled(busy.is_some())`, `loading` у той, что в работе; Check — «Проверить» (`.primary()`, `loading` при `Busy::Check`); Log — «Очистить» (`.outline()`);
- корень: `track_focus`, `on_key_down` Esc → `hide_window`, `rounded_lg`, рамка `window_border`, `p_4` (внешние поля 16 px), `gap_3`.

Диалог «Выбрать…»: `dialog_open = true`; `cx.prompt_for_paths(PathPromptOptions { directories: true, files: false, multiple: false, prompt: None })`; в `cx.spawn` дождаться, `dialog_open = false`, `activate_window`, выбранный путь → `set_value` поля и `save_host`.

- [ ] **Step 5: Тесты зелёные**, `cargo test -p onec-connect gui::` и `--no-default-features`.

- [ ] **Step 6: Живой прогон на Windows** (отладочная сборка открывает консоль — запускать из терминала, а не `Start-Process`): открылось у трея; переключатель поднимает туннель (или пишет причину, если `1c-connect.toml` образцовый); щелчок мимо прячет; значок возвращает; Esc прячет; «Выбрать…» не прячет окно и возвращает его; уход из поля пишет toml (проверить дату файла); «Проверка» рисует факты и бейдж; «Тема → Светлая» в трее перекрашивает открытое окно, после перезапуска тема та же, «Системная» возвращает системную; «Выход» завершает процесс и туннель (нет `1c-connect` в диспетчере). Отдельно, на пробу: `WindowBackgroundAppearance::Blurred` с подложкой `background.opacity(0.8)` — если текст чёткий и фон размыт, оставить; иначе `Opaque` (спека, «Материал»).

- [ ] **Step 7: Commit** — `feat(1c-connect): Panel view — status card, pivot tabs, autosaving parameters, checks, log`

---

### Task 7: Хвосты: CI, документация, чистка

**Files:**
- Modify: `.github/workflows/build.yml:50-52` (комментарий: окно на GPUI, Linux без окна, потому что GPUI-на-Linux не нужен)
- Modify: `docs/STATUS.md:23` (Tauri 2 → GPUI; размер бинаря из задачи 1), строки 203–205 и 229–235 (ловушки Tauri: пометить как исторические или удалить)
- Modify: `docs/superpowers/specs/2026-10-16-setup-binaries-design.md`, раздел «Окна и трей (этап 4)»: абзац «Сборка» и «Вид» — одна фраза «заменено, см. `2026-10-16-gpui-connect-design.md`»
- Modify: `setup/1c-connect/Cargo.toml` — проверить, что комментарии не упоминают Tauri/webkit
- Delete: `spike-gpui/` (не в индексе; удалить с диска после живого прогона задачи 6)

- [ ] **Step 1: Grep на остатки**

Run: `grep -rn -i 'tauri\|webview\|withGlobalTauri' setup/1c-connect .github docs/STATUS.md`
Expected: только исторические упоминания в `STATUS.md`, помеченные как прошлое.

- [ ] **Step 2: Полные проверки**

Run: `cargo test --manifest-path setup/Cargo.toml --workspace` и `--no-default-features` → зелёные.
Run: `cargo build --release --manifest-path setup/Cargo.toml -p onec-connect` → собирается.

- [ ] **Step 3: Commit** — `docs(1c-connect): GPUI window in STATUS and the setup spec; CI comments; drop the spike`

---

### Task 8: Вкладка «Проект» и каталог проекта в параметрах (перенос этапа 6B из Tauri-окна `main`)

Пока окно переезжало на GPUI, в `main` Tauri-окно получило этап 6B (коммиты 560770b, bd638de, 8f62260, 5bf71f6): вкладку «Проект», поле «Каталог проекта» в параметрах, выгрузку/загрузку через git, и из формы ушёл «Каталог выгрузки конфигурации» (`dump` остаётся в toml). Ветка перебазирована на этот `main`; `src/gui.rs` (Tauri) удалён, его логика ниже переносится в `gui/` один в один. Образец: `git show main:setup/1c-connect/src/gui.rs` (команды `run`, `project`, `open_dir`, `save_cfg`), `git show main:setup/1c-connect/ui/app.js` (`refreshProject`, `RUN_TITLES`, действия вкладки), `git show main:setup/1c-connect/ui/index.html` (поле проекта с подсказкой).

**Files:**
- Modify: `setup/1c-connect/src/gui/mod.rs`, `setup/1c-connect/src/gui/panel.rs`
- Modify: `docs/superpowers/specs/2026-10-16-gpui-connect-design.md` («Состояние и вид»: список полей и вкладок), `docs/superpowers/plans/2026-10-16-gpui-connect.md` (эта задача уже вписана)
- Test: оба файла (`mod tests`)

**Interfaces (consumes):** `crate::project::{Project, Layout, layout_state, status}` (`Project::from_cfg(&Cfg, &Path) -> Project` с полями `dir: PathBuf`, `branch: String`; `layout_state(&Project) -> Layout {Missing, BaseDir(String), Drifted(String), Ok}`; `status(&Project) -> Result<String, String>`), `crate::ops_host::tools_dir() -> PathBuf`, `crate::flow::{export(&Ctx, &str) -> Result<String, String>, import(&Ctx) -> Result<String, String>}`, `common::Runner` (для `ctx.ok(text)`).

**Interfaces (produces):**

```rust
pub enum Tab { Params, Ext, Project, Check, Log }          // Project между Ext и Check, в этом порядке и во вкладках
pub enum Busy { Ext, Project, Check }
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Which { Install, Uninstall, Export, Import }
impl Which { pub fn title(self) -> &'static str }          // «— устанавливаю —», «— снимаю расширение —», «— выгружаю в git —», «— загружаю из git —»
#[derive(Clone, Debug, PartialEq)] pub struct ProjectView { pub dir: String, pub branch: String, pub lines: Vec<String>, pub ok: bool, pub why: String }
pub enum Ev { .., Project(ProjectView) }                   // handle: ui.project = Some(v); refresh_windows
pub fn run_op(cx: &mut App, which: Which);                 // busy = Project для Export/Import, иначе Ext; строка Kind::Say с which.title(); ui.tab = Tab::Log; поток: Install → apply_all, Uninstall → revert_apply (как сейчас), Export → flow::export(&ctx, "выгрузка из 1c-connect").map(|t| ctx.ok(t)), Import → flow::import(&ctx).map(|t| ctx.ok(t)); ошибки → Ev::Done(Some(text)); guarded() как сейчас
pub fn refresh_project(cx: &mut App);                      // поток: Project::from_cfg(&cfg, &tools_dir()); layout_state → (ok, why): Missing «каталог проекта ещё не разложен», BaseDir(f) «это каталог базы ({f})», Drifted(d) d, Ok ("", true); lines = status(&p).lines() при ok, иначе пусто; → Ev::Project. Зовётся при открытии вкладки Project, после Ev::Done и после удачного сохранения.
pub fn open_dir(cx: &mut App);                             // explorer / xdg-open на cfg.host.project; ошибка → note_line Fail «не открыть {dir}: {e}»
pub fn save_host(ui: &mut Ui, mut host: Host) -> Result<(), String>; // теперь переносит из текущих cfg `dump` и `tools` (как save_cfg в main), `project` берётся из формы
pub fn project_rows(v: Option<&ProjectView>) -> Vec<(Dot, String, String)>; // panel.rs, чистая: None → [(Off, «Каталог проекта задаётся в «Параметрах»; install раскладывает git.», "")]; !ok → [(Busy, dir, why)] + подсказка «Параметры → Проект, затем Расширение → Установить» третьим полем или второй строкой; ok → [(Up, dir, «ветка {branch}»)] + по строке на каждую line: разбить по первому ": " на (title, detail), Dot::Off
```

`Ui` получает поле `pub project: Option<ProjectView>`.

**Вкладка «Параметры»:** ряд «Каталог выгрузки конфигурации» убрать; после «Адрес публикации» ряд «Каталог проекта» с кнопкой «Выбрать…» (placeholder `C:\proj\Buh`) и подсказкой под полем 13 px приглушённым: «Исходники в src, git с веткой рабочего места. Не каталог базы.». `Fields.dump` → `Fields.project`; `host_of` берёт `project` из поля, `dump` пустой; `fields_differ` сравнивает `project` вместо `dump`; тест `dirty_ignores_project_and_tools_but_sees_a_changed_field` переименовать в `dirty_ignores_dump_and_tools_but_sees_a_changed_field` и поправить (`dump`/`tools` игнорируются, `project` — нет).

**Вкладка «Проект»:** строки из `project_rows` слоями, как «Расширение». Плашка действий: «Открыть каталог» (outline → `open_dir`), «Загрузить из git» (outline → `run_op(Import)`), «Выгрузить в git» (primary → `run_op(Export)`); все `disabled(busy.is_some())`, `loading` у «Загрузить»/«Выгрузить» при `Busy::Project`. Бейджи и прочее без изменений; индекс вкладок сдвигается (Check = 3, Log = 4).

- [ ] **Step 1: Падающие тесты** — `which_titles_are_the_old_log_lines` (четыре заголовка), `project_rows_explain_a_missing_layout_and_list_status_lines` (None → одна строка-подсказка; `ok=false` → dir + why; `ok=true` c `lines = ["ветка: dev1", "изменено: 2 файла"]` → первая строка dir/«ветка dev1», затем («ветка», «dev1»), («изменено», «2 файла»)), обновлённый тест `fields_differ`.
- [ ] **Step 2: Убедиться, что не компилируется.**
- [ ] **Step 3: Реализовать** по Interfaces; ничего из Tauri-образца не копировать дословно, только логику и тексты.
- [ ] **Step 4: Проверки:** `cargo test --manifest-path setup/Cargo.toml --workspace --no-default-features -j 1` зелёный; один `cargo check --manifest-path setup/Cargo.toml -p onec-connect -j 1 --all-targets` (`CARGO_TARGET_DIR=C:\cargo-target\gpui-connect`, передний план, timeout 600000, перезапуск при лимите) без ошибок и предупреждений. Полные тесты с окном — в CI.
- [ ] **Step 5: Commit** — `feat(1c-connect): Проект tab and project directory in the GPUI window (stage 6B port)`

---

### Task 9: Высота окна по содержимому вкладки «Параметры»

Заказчик (2026-10-18): на вкладке «Параметры» остаётся небольшая ненужная прокрутка. Окно должно само подбирать высоту так, чтобы «Параметры» помещались целиком; прокрутка остаётся только как страховка на маленьких рабочих областях.

**Files:** Modify `setup/1c-connect/src/gui/window.rs`, `setup/1c-connect/src/gui/panel.rs`, `setup/1c-connect/src/gui/mod.rs` (поле в `Ui`); spec «Окно» (380×640 → «380 × высота по содержимому, не меньше 640 и не больше рабочей области»).

**Interfaces (produces):**

```rust
// window.rs
pub const MIN_SIZE: Size<Pixels>;                                  // 380×640 (бывший SIZE)
pub fn wanted_size(cx: &App) -> Size<Pixels>;                      // Ui.height.unwrap_or(MIN_SIZE.height), ширина MIN_SIZE.width
pub fn fit_height(window: &mut Window, cx: &mut App, content: Pixels, viewport: Pixels); // если content > viewport: new_h = min(window.bounds().size.height + (content - viewport), work_area height - 2*GAP); Ui.height = Some(new_h); window.resize(size(w, new_h)); затем вернуть окно в угол: на Windows SetWindowPos(hwnd, 0, x, y, 0, 0, SWP_NOSIZE|SWP_NOZORDER|SWP_NOACTIVATE) с origin = flyout_origin(work_area, new size, GAP) в физических px (умножить на window.scale_factor()); hwnd — из raw_window_handle::HasWindowHandle for Window (RawWindowHandle::Win32(h) → h.hwnd). Делается один раз на окно (флаг в Panel).
pub fn grown_height(current: Pixels, content: Pixels, viewport: Pixels, work_area_h: Pixels, gap: Pixels) -> Option<Pixels>; // чистая: None, если content <= viewport; иначе Some(min(current + content - viewport, work_area_h - 2*gap))
// mod.rs
pub struct Ui { .., pub height: Option<Pixels> }                   // запомненная высота; show_window открывает окно сразу с ней, второй показ без скачка
```

**panel.rs:** тело «Параметры» — обёртка `div().flex_1().min_h_0()` получает `on_children_prepainted` → запоминает высоту viewport (bounds[0].size.height); прокручиваемый контейнер `div().id("params").overflow_y_scroll()` получает `on_children_prepainted` → высота содержимого (bounds[0].size.height); когда известны оба и `Panel.fitted == false` — `window::fit_height(window, cx, content, viewport)`, `fitted = true`. Слушатели через `cx.listener`. Прокрутка у «Параметры» остаётся (страховка), но после подбора высоты её не должно быть видно.

- [ ] **Step 1: Падающий тест** `grown_height_grows_by_the_overflow_and_stops_at_the_work_area`: `(640, 700, 560, 1300, 12) → Some(780)`; `(640, 500, 560, 1300, 12) → None`; `(640, 2000, 560, 1000, 12) → Some(976)`.
- [ ] **Step 2: Убедиться, что падает.** **Step 3: Реализовать.**
- [ ] **Step 4: Проверки:** `--no-default-features` тесты; один `cargo check -p onec-connect -j 1 --all-targets` (`CARGO_TARGET_DIR=C:\cargo-target\gpui-connect`, передний план, timeout 600000). Полные тесты и живой прогон — CI и заказчик.
- [ ] **Step 5: Commit** — `feat(1c-connect): window height fits the Параметры tab`

---

### Task 10: Меню трея в теме приложения

Заказчик (2026-10-18): контекстное меню значка в трее должно быть светлым или тёмным вместе с темой окна. Это Win32-меню `muda`; у Windows нет документированного API, общепринятый приём (Проводник, Notepad++, wxWidgets): `uxtheme.dll` ординалы 135 `SetPreferredAppMode(mode)` и 136 `FlushMenuThemes()`; режимы `Default = 0, AllowDark = 1, ForceDark = 2, ForceLight = 3`. Доступно с Windows 10 1903; на старых системах ординалов нет — тогда ничего не делать.

**Files:** Modify `setup/1c-connect/src/gui/theme.rs` (новая функция), `setup/1c-connect/src/gui/mod.rs` (вызов при старте до `tray::build`), `setup/1c-connect/Cargo.toml` (фича `Win32_System_LibraryLoader` у `windows-sys`), spec «Выбор темы — в меню трея» (одна фраза про меню).

**Interfaces (produces):**

```rust
// theme.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum MenuMode { Light, Dark }
pub fn menu_mode(choice: Choice, system_dark: bool) -> MenuMode;   // чистая: Light → Light, Dark → Dark, System → по system_dark
pub fn apply_menu_theme(mode: MenuMode);                           // Windows: LoadLibraryW("uxtheme.dll"), GetProcAddress по ординалам 135/136 (MAKEINTRESOURCE: указатель с номером в младшем слове), SetPreferredAppMode(ForceDark|ForceLight), затем FlushMenuThemes(); если библиотека или функции не нашлись — тихо ничего; не-Windows — no-op. unsafe минимально, с комментарием про недокументированность.
```

`apply(choice, cx)` после смены темы GPUI зовёт `apply_menu_theme(menu_mode(choice, Theme::global(cx).is_dark()))`; `install` — то же при старте, до `tray::build` в `launch`. Проверить, что `Theme::global(cx).is_dark()` отражает уже применённый режим (иначе взять `cx.window_appearance()` для System).

- [ ] **Step 1: Падающий тест** `menu_mode_follows_the_choice_and_the_system_only_when_asked`: `(Light, true) → Light`, `(Dark, false) → Dark`, `(System, true) → Dark`, `(System, false) → Light`.
- [ ] **Step 2–3:** убедиться, что падает; реализовать.
- [ ] **Step 4: Проверки:** `--no-default-features` тесты; один `cargo check -p onec-connect -j 1 --all-targets` (`CARGO_TARGET_DIR=C:\cargo-target\gpui-connect`, передний план, timeout 600000).
- [ ] **Step 5: Commit** — `feat(1c-connect): tray menu follows the app theme (uxtheme preferred app mode)`

---

### Task 11: Токены темы по палитре 1С

Заказчик (2026-10-18): светлая тема должна быть максимально похожа на 1С:Предприятие (Такси), тёмная — на тех же токенах, акцент не синий. Палитра снята с живого тонкого клиента 8.3.27 (Бухгалтерия 3.0, схема по умолчанию): фон окна `#F2F2F2`, содержимое `#FFFFFF`, карточки `#FFFBE3`, панель разделов и заголовок `#FBED9E`, жёлтый акцент `#FACC1F`, тёмный жёлтый `#C8BA6B`, светлый крем `#FDF7D7`, текст `#333333`, вторичный текст `#4D4D4D`, плейсхолдер `#C7C7C7`, рамки `#A0A0A0`/`#B1B1B1`, ссылка `#1C55AE`, зелёный `#009646`, красный `#B22222`, оранжевый `#DF9C4B`/`#C27E33`.

**Files:** Rename `setup/1c-connect/theme/macos-classic.json` → `setup/1c-connect/theme/1c.json` (git mv) и переписать; Modify `setup/1c-connect/src/gui/theme.rs` (`THEME_JSON`, `LIGHT = "1C Light"`, `DARK = "1C Dark"`, тест), spec «Тема и шрифты» (палитра 1С вместо macOS Classic, акцент жёлтый, ссылки синие остаются ссылками), `docs/STATUS.md` (одна фраза), plan Global Constraints (`theme/1c.json`).

**Содержимое `theme/1c.json`** — те же ключи, что сейчас (структура `ThemeSet`, `font.*`, `radius 4`, `radius.lg 8`, `shadow false`); значения:

Светлая `"1C Light"` (`mode: light`):
`background #FFFFFF`, `foreground #333333`, `border #D9D9D9`, `input.border #B1B1B1`, `ring #B1B1B1`, `caret #333333`, `selection.background #FBED9E`, `muted.background #F2F2F2`, `muted.foreground #4D4D4D`, `accent.background #FDF7D7`, `accent.foreground #333333`, `secondary.background #FFFBE3`, `secondary.hover.background #FDF7D7`, `secondary.active.background #FBED9E`, `secondary.foreground #333333`, `primary.background #FACC1F`, `primary.hover.background #FBD64A`, `primary.active.background #E6B800`, `primary.foreground #333333`, `success.background #009646`, `success.foreground #009646`, `danger.background #B22222`, `danger.foreground #B22222`, `warning.background #DF9C4B`, `warning.foreground #C27E33`, `info.background #1C55AE`, `info.foreground #1C55AE`, `link #1C55AE`, `link.hover #1C7BCC`, `link.active #163F80`, `popover.background #FFFFFF`, `popover.foreground #333333`, `list.background #FFFFFF`, `list.active.background #FBED9E`, `list.active.border #C8BA6B`, `list.hover.background #FDF7D7`, `list.even.background #FFFFFF`, `list.head.background #F2F2F2`, `tab_bar.background #FFFFFF`, `tab_bar.segmented.background #F2F2F2`, `tab.background #FFFFFF`, `tab.foreground #4D4D4D`, `tab.active.background #FFFFFF`, `tab.active.foreground #333333`, `title_bar.background #FBED9E`, `title_bar.border #C8BA6B`, `status_bar.background #F2F2F2`, `status_bar.border #D9D9D9`, `sidebar.background #FBED9E`, `sidebar.border #C8BA6B`, `sidebar.foreground #333333`, `sidebar.accent.background #FDF7D7`, `sidebar.accent.foreground #333333`, `sidebar.primary.background #FACC1F`, `sidebar.primary.foreground #333333`, `scrollbar.background #00000000`, `scrollbar.thumb.background #C8C8C8AA`, `scrollbar.thumb.hover.background #A0A0A0`, `switch.background #C8C8C8`, `switch.thumb.background #FFFFFF`, `group_box.background #FFFBE3`, `group_box.foreground #333333`, `skeleton.background #F2F2F2`, `slider.background #FACC1F`, `slider.thumb.background #FFFFFF`, `progress.bar.background #FACC1F`, `table.background #FFFFFF`, `table.head.foreground #4D4D4D`, `table.row.border #E6E6E6`, `drag.border #FACC1F`, `drop_target.background #FBED9E80`, `overlay #00000022`, `window.border #B1B1B1`, `base.red #B22222`, `base.red.light #D9564E`, `base.green #009646`, `base.green.light #35BD33`, `base.blue #1C55AE`, `base.blue.light #1C7BCC`, `base.yellow #FACC1F`, `base.yellow.light #FBED9E`, `base.magenta #8E44AD`, `base.magenta.light #B07CC6`, `base.cyan #1C7BCC`, `base.cyan.light #50B3DC`.

Тёмная `"1C Dark"` (`mode: dark`), те же роли на тёплых тёмных серых, акцент тот же жёлтый:
`background #262626`, `foreground #DCDCDC`, `border #4A4A4A`, `input.border #5A5A5A`, `ring #5A5A5A`, `caret #DCDCDC`, `selection.background #FACC1F40`, `muted.background #333333`, `muted.foreground #A6A6A6`, `accent.background #3A3A3A`, `accent.foreground #DCDCDC`, `secondary.background #2F2D26`, `secondary.hover.background #3A3830`, `secondary.active.background #45423A`, `secondary.foreground #DCDCDC`, `primary.background #FACC1F`, `primary.hover.background #FBD64A`, `primary.active.background #E6B800`, `primary.foreground #262626`, `success.background #3DBB6E`, `success.foreground #3DBB6E`, `danger.background #E06060`, `danger.foreground #E06060`, `warning.background #DF9C4B`, `warning.foreground #DF9C4B`, `info.background #6FA3F0`, `info.foreground #6FA3F0`, `link #6FA3F0`, `link.hover #8FB8F5`, `link.active #5A8CD8`, `popover.background #2C2C2C`, `popover.foreground #DCDCDC`, `list.background #262626`, `list.active.background #45423A`, `list.active.border #C8BA6B`, `list.hover.background #333333`, `list.even.background #262626`, `list.head.background #2C2C2C`, `tab_bar.background #262626`, `tab_bar.segmented.background #2F2D26`, `tab.background #262626`, `tab.foreground #A6A6A6`, `tab.active.background #262626`, `tab.active.foreground #DCDCDC`, `title_bar.background #3A3A3A`, `title_bar.border #4A4A4A`, `status_bar.background #2C2C2C`, `status_bar.border #4A4A4A`, `sidebar.background #2C2C2C`, `sidebar.border #4A4A4A`, `sidebar.foreground #DCDCDC`, `sidebar.accent.background #3A3A3A`, `sidebar.accent.foreground #DCDCDC`, `sidebar.primary.background #FACC1F`, `sidebar.primary.foreground #262626`, `scrollbar.background #00000000`, `scrollbar.thumb.background #6A6A6AAA`, `scrollbar.thumb.hover.background #8A8A8A`, `switch.background #4A4A4A`, `switch.thumb.background #E5E5E5`, `group_box.background #2F2D26`, `group_box.foreground #DCDCDC`, `skeleton.background #333333`, `slider.background #FACC1F`, `slider.thumb.background #E5E5E5`, `progress.bar.background #FACC1F`, `table.background #262626`, `table.head.foreground #A6A6A6`, `table.row.border #3A3A3A`, `drag.border #FACC1F`, `drop_target.background #45423A80`, `overlay #00000055`, `window.border #4A4A4A`, `base.red #E06060`, `base.red.light #F08A8A`, `base.green #3DBB6E`, `base.green.light #6FD79A`, `base.blue #6FA3F0`, `base.blue.light #8FB8F5`, `base.yellow #FACC1F`, `base.yellow.light #FBED9E`, `base.magenta #B07CC6`, `base.magenta.light #C9A0DC`, `base.cyan #50B3DC`, `base.cyan.light #7FCBE8`.

- [ ] **Step 1: Обновить тест** `theme_file_has_one_light_and_one_dark_theme_with_the_fluent_radii` под новые имена; добавить проверку, что `primary.background` обеих тем — `#FACC1F`.
- [ ] **Step 2–3:** тест падает → переписать файл и константы.
- [ ] **Step 4: Проверки:** `--no-default-features` тесты; один `cargo check -p onec-connect -j 1 --all-targets` (`CARGO_TARGET_DIR=C:\cargo-target\gpui-connect`, передний план, timeout 600000).
- [ ] **Step 5: Commit** — `feat(1c-connect): 1C palette — light like Такси, dark on the same tokens, yellow accent`
