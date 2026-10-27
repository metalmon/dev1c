# Режим публикации под управлением человека — план работ

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Человек выбирает в меню трея, кто публикует базу, и — когда публикация его — вписывает её адрес в окне, ни разу не открывая `dev1c.toml`.

**Architecture:** Режим `publication` уже разобран в `ops_host` (`ensure_publication`, `stop_own_publication`) и в `Cfg::base_url`; работа добавляет к нему ровно три поверхности — подменю в трее по образцу готового «Режим работы», поле «Адрес публикации» на вкладке «Параметры», видимое только в режиме «уже опубликована», и условие в `cold_gate`. Плюс два исправления, без которых режим врёт человеку: введённый адрес сегодня затирается записью, а 404 от неопубликованного сервиса читается как «расширения в базе нет».

**Tech Stack:** Rust, GPUI, пакет `onec-connect` (бинарь `1c-connect`), `tray-icon`, тесты — `cargo test -p onec-connect`.

**Spec:** `docs/superpowers/specs/2026-10-26-publication-mode-design.md`

## Global Constraints

Режимы публикации зовутся в toml `auto` и `webserver` — значения не меняются, меняются только подписи для человека.

Чужую публикацию не трогаем ничем: не пишем и не правим её дескриптор, не гасим и не перезапускаем её веб-сервер, не перезапускаем платформу. Единственное, что мы про неё делаем, — спрашиваем по адресу и говорим человеку словами.

Человек вписывает адрес как видит его в браузере (`http://localhost/Buh`); хвост `/hs/mcp-1c` дописывает программа. `Cfg::validate` не ослабляется: он сторожит toml и проверяет уже приведённое значение.

Порт из адреса публикации идёт в туннель (`Cfg::web_port`) и никогда — в `Listen` нашего httpd (`Cfg::http_port`).

Пустых полей программа не заполняет выдумками: незаполненный адрес остаётся пустым и в окне, и в toml (решение владельца 26.10, коммит `adce1c6`).

Галочка, которую человек ставит в Конфигураторе, зовётся «Публиковать HTTP-сервисы расширений по умолчанию» (в английской локализации — «Publish extensions' HTTP services by default»), вкладка «HTTP-сервисы» диалога «Публикация на веб-сервере». Это единственное его действие; перезапуск веб-сервера не нужен (замер 27.10).

## Review Focus

Адрес с портом (`http://localhost:8081/Buh`): порт обязан уйти в туннель и не попасть в `Listen` — задача 1 сторожит это тестом.

Адрес с кириллицей, пробелами или чужой машиной вместо `localhost`: человек должен прочитать отказ про адрес публикации, а не про поле `base_url` в toml — задача 4.

Переключение режима под идущей операцией: нативный пункт меню уже отметил себя сам, и вернуть отметку обязаны мы — задача 5.

Переключение на «уже опубликована», когда свой httpd и не поднимался: уход обязан быть тихим успехом, а не отказом — задача 6.

Смена рабочего места в режиме «уже опубликована»: за ним едет порт нашего httpd (`carried_http_port`), а свой адрес публикации обязан пережить переезд — задача 4.

---

### Task 1: Адрес публикации — приведение к полному и отказ от выдумки

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs` (`base_url` — `cfg.rs:365`, рядом `derived_base_url` — `cfg.rs:69`)
- Test: там же, модуль `tests` в конце файла

**Interfaces:**
- Produces: `cfg::full_base_url(typed: &str) -> String`, `Cfg::publication_address_missing(&self) -> bool`; `Cfg::base_url()` в режиме `webserver` с пустым полем отдаёт пустую строку.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_typed_address_is_completed_to_the_service_url() {
    assert_eq!(full_base_url("http://localhost/Buh"), "http://localhost/Buh/hs/mcp-1c");
    assert_eq!(full_base_url("localhost/Buh"), "http://localhost/Buh/hs/mcp-1c");
    assert_eq!(full_base_url("http://localhost/Buh/"), "http://localhost/Buh/hs/mcp-1c");
    assert_eq!(full_base_url("http://localhost:8081/Buh"), "http://localhost:8081/Buh/hs/mcp-1c");
    assert_eq!(full_base_url("http://localhost/Buh/hs/mcp-1c"), "http://localhost/Buh/hs/mcp-1c");
    assert_eq!(full_base_url("  "), "");
}

#[test]
fn a_completed_address_passes_validate() {
    let mut c = filled();
    c.host.publication = "webserver".into();
    c.host.base_url = full_base_url("http://localhost/Buh");
    assert!(c.validate().is_ok(), "{:?}", c.validate().unwrap_err());
}

#[test]
fn an_address_we_cannot_fix_is_refused_in_words_about_the_publication() {
    let mut c = filled();
    c.host.publication = "webserver".into();
    // Чужая машина, пробел, кириллица в пути — приведение их не спасает.
    for typed in ["http://192.168.1.10/Buh", "http://localhost/Бух", "http://localhost/My Base"] {
        c.host.base_url = full_base_url(typed);
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("адрес публикации"), "{typed}: {e}");
    }
}

#[test]
fn webserver_without_an_address_does_not_invent_one() {
    let mut c = filled();
    c.host.publication = "webserver".into();
    c.host.base_url = String::new();
    assert_eq!(c.base_url(), "");
    assert!(c.publication_address_missing());
    // Порт туннеля на незаданном адресе — всё ещё 80, а не паника.
    assert_eq!(c.web_port(), 80);
}

#[test]
fn auto_mode_still_counts_its_own_address() {
    let mut c = filled();
    c.host.workstation = "dev1".into();
    c.host.publication = "auto".into();
    c.host.base_url = "http://localhost/Buh/hs/mcp-1c".into();
    assert_eq!(c.base_url(), "http://localhost:8081/hs/mcp-1c");
    assert!(!c.publication_address_missing());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect --lib cfg::`
Expected: FAIL — `full_base_url` и `publication_address_missing` не определены.

- [ ] **Step 3: Implement `full_base_url(typed: &str) -> String` and `Cfg::publication_address_missing(&self) -> bool` in `setup/1c-connect/src/cfg.rs`**

Приведение: обрезать пробелы, пустое вернуть пустым, дописать `http://` если нет схемы, снять хвостовые `/`, дописать `/hs/mcp-1c` если его ещё нет. `base_url()` в ветке `webserver` отдаёт `host.base_url` как есть (пустой — пустым), ветку `derived_base_url` из неё убрать; `derived_base_url` остаётся — его зовёт `carried_base_url`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect --lib cfg::`
Expected: PASS, и ранее написанный `web_port_follows_cfg_base_url_not_the_stored_field` тоже зелёный.

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/cfg.rs
git commit -m "Адрес публикации: дописываем хвост сами, пустое не выдумываем"
```

---

### Task 2: Пустой адрес публикации запирает переключатель

**Files:**
- Modify: `setup/1c-connect/src/gui/gate.rs` (`cold_gate` — `gate.rs:35`)
- Test: там же, модуль `tests`

**Interfaces:**
- Consumes: `Cfg::publication_address_missing` (задача 1).
- Produces: `cold_gate` отдаёт `Some("не задан адрес публикации — впишите его и нажмите «Сохранить»")`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn webserver_without_an_address_locks_the_switch() {
    let mut c = filled_cfg();
    c.host.publication = "webserver".into();
    c.host.base_url = String::new();
    assert_eq!(open(&c).as_deref(), Some("не задан адрес публикации — впишите его и нажмите «Сохранить»"));
    c.host.base_url = "http://localhost/Buh/hs/mcp-1c".into();
    assert_eq!(open(&c), None);
    // В обычном режиме адрес считается сам — замку там не за что цепляться.
    c.host.publication = "auto".into();
    c.host.base_url = String::new();
    assert_eq!(open(&c), None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p onec-connect --lib gui::gate::`
Expected: FAIL — замок отдаёт `None`.

- [ ] **Step 3: Add the condition to `cold_gate` in `setup/1c-connect/src/gui/gate.rs`**

Ставится после условия про пользователя базы и до блока `h.mode != "local"`: порядок замка — от основания к вершине, а адрес публикации ниже транспорта и выше ничего.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p onec-connect --lib gui::gate::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/gui/gate.rs
git commit -m "Замок: без адреса публикации включать нечего"
```

---

### Task 3: Поле «Адрес публикации» на «Параметрах»

**Files:**
- Modify: `setup/1c-connect/src/gui/panel.rs` (`Fields` — `panel.rs:48`, `shown_fields` — `panel.rs:504`, `field_value` — `panel.rs:512`, `host_of` — `panel.rs:532`, `field_states` — `panel.rs:822`, `adopt_fields` — `panel.rs:805`, `fields_differ` — `panel.rs:588`, вкладка «Параметры» — `panel.rs:1122`)
- Test: там же, модуль `tests`

**Interfaces:**
- Consumes: `cfg::derived_base_url` (для показа пустым).
- Produces: `panel::shown_fields(mode: &str, publication: &str) -> &'static [&'static str]`, `panel::shown_base_url(host: &Host) -> &str`; поле `Fields.base_url`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_address_field_belongs_to_the_webserver_mode_only() {
    assert!(shown_fields("tunnel", "webserver").contains(&"base_url"));
    assert!(shown_fields("local", "webserver").contains(&"base_url"));
    assert!(!shown_fields("tunnel", "auto").contains(&"base_url"));
    assert!(!shown_fields("local", "auto").contains(&"base_url"));
    // Прежнее правило не тронуто: адрес среды прячется в локальном режиме.
    assert!(!shown_fields("local", "auto").contains(&"server"));
}

#[test]
fn a_derived_address_is_shown_as_an_empty_field() {
    let mut h = Host::default();
    h.base_path = "C:\\Bases\\Buh".into();
    h.base_url = cfg::derived_base_url(&h.base_path);
    assert_eq!(shown_base_url(&h), "");
    h.base_url = "http://localhost/UNF/hs/mcp-1c".into();
    assert_eq!(shown_base_url(&h), "http://localhost/UNF/hs/mcp-1c");
    h.base_url = String::new();
    assert_eq!(shown_base_url(&h), "");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect --lib gui::panel::`
Expected: FAIL — `shown_fields` принимает один аргумент, `shown_base_url` не определена.

- [ ] **Step 3: Add the field and thread the publication through `setup/1c-connect/src/gui/panel.rs`**

`Fields.base_url: Entity<InputState>` и строка в `field_states`/`field_value`; `shown_fields` принимает вторым аргументом публикацию, зовущие (`host_of`, `adopt_fields`, `fields_differ`, отрисовка вкладки) передают `cfg.host.publication`. Ряд в столбце — под «Базой» и над «Пользователем базы», подпись «Адрес публикации», подсказка полем-плейсхолдером: `http://localhost/Buh`. Показываемое значение берёт `shown_base_url`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect --lib gui::panel::`
Expected: PASS

- [ ] **Step 5: Check the window still builds and draws**

Run: `cargo build -p onec-connect`
Expected: сборка без ошибок; `cargo fmt --check` чистый.

- [ ] **Step 6: Commit**

```bash
git add setup/1c-connect/src/gui/panel.rs
git commit -m "Поле адреса публикации — только в режиме «уже опубликована»"
```

---

### Task 4: Запись не затирает введённый адрес

**Files:**
- Modify: `setup/1c-connect/src/gui/save.rs` (`save_host` — `save.rs:94`, `carried_base_url` — `save.rs:10`)
- Test: там же, модуль `tests`

**Interfaces:**
- Consumes: `cfg::full_base_url` (задача 1), `panel::shown_fields` (задача 3).
- Produces: `save::base_url_to_write(saved: &Host, from_form: &str, publication: &str, new_base_path: &str) -> String`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn what_a_person_typed_is_what_gets_written() {
    let saved = Host { publication: "webserver".into(), ..Host::default() };
    assert_eq!(
        base_url_to_write(&saved, "http://localhost/Buh", "webserver", "C:\\Bases\\Buh"),
        "http://localhost/Buh/hs/mcp-1c"
    );
    // Пустое поле остаётся пустым: выдуманного адреса здесь не появляется.
    assert_eq!(base_url_to_write(&saved, "   ", "webserver", "C:\\Bases\\Buh"), "");
}

#[test]
fn an_own_address_survives_a_workstation_move() {
    let saved = Host { publication: "webserver".into(), base_url: "http://localhost/UNF/hs/mcp-1c".into(), ..Host::default() };
    assert_eq!(base_url_to_write(&saved, "http://localhost/UNF", "webserver", "C:\\Bases\\Other"), "http://localhost/UNF/hs/mcp-1c");
}

#[test]
fn a_foreign_address_waits_through_a_round_trip_to_our_own_httpd() {
    // Переключились на «публикует коннектор» и обратно — вписанное не стёрто.
    let saved = Host { publication: "auto".into(), base_url: "http://localhost/UNF/hs/mcp-1c".into(), base_path: "C:\\Bases\\UNF".into(), ..Host::default() };
    assert_eq!(base_url_to_write(&saved, "", "auto", "C:\\Bases\\UNF"), "http://localhost/UNF/hs/mcp-1c");
}

#[test]
fn in_auto_mode_the_address_still_follows_the_base_folder() {
    let saved = Host { publication: "auto".into(), base_path: "C:\\Bases\\Buh".into(), base_url: cfg::derived_base_url("C:\\Bases\\Buh"), ..Host::default() };
    assert_eq!(base_url_to_write(&saved, "", "auto", "C:\\Bases\\UNF"), cfg::derived_base_url("C:\\Bases\\UNF"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect --lib gui::save::`
Expected: FAIL — `base_url_to_write` не определена.

- [ ] **Step 3: Implement `base_url_to_write` and call it from `save_host` in `setup/1c-connect/src/gui/save.rs`**

В режиме `webserver` — `cfg::full_base_url(from_form)`; в режиме `auto` — прежний `carried_base_url`. Строка `host.base_url = carried_base_url(…)` в `save_host` заменяется вызовом новой функции; значение из формы приходит в `host.base_url` от `panel::host_of`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect --lib gui::save::`
Expected: PASS

- [ ] **Step 5: Check the refusal text a person reads**

Run: `cargo test -p onec-connect --lib cfg::`
Expected: PASS. Если `validate` отвергает приведённый адрес (чужая машина, пробелы, кириллица), в окно идёт его текст — проверить глазами, что он говорит про адрес публикации, и поправить формулировку в `cfg::validate`, не ослабляя само правило.

- [ ] **Step 6: Commit**

```bash
git add setup/1c-connect/src/gui/save.rs setup/1c-connect/src/cfg.rs
git commit -m "Введённый адрес публикации доживает до toml"
```

---

### Task 5: Уход из своей публикации убирает за собой

**Files:**
- Modify: `setup/1c-connect/src/publication.rs` (рядом со `stop` — `publication.rs:517`), `setup/1c-connect/src/ops_host.rs` (`stop_own_publication` — `ops_host.rs:1225`)
- Test: `setup/1c-connect/src/publication.rs`, модуль `tests`

**Interfaces:**
- Produces: `publication::remove_descriptor(pub_dir: &Path) -> std::io::Result<()>`, `ops_host::leave_own_publication(ctx: &Ctx) -> Result<bool>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn leaving_our_own_publication_takes_the_descriptor_with_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("default.vrd"), "<point/>").unwrap();
    remove_descriptor(dir.path()).unwrap();
    assert!(!dir.path().join("default.vrd").exists());
    // Второй раз — не ошибка: уходить уже не из чего.
    remove_descriptor(dir.path()).unwrap();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p onec-connect --lib publication::`
Expected: FAIL — `remove_descriptor` не определена.

- [ ] **Step 3: Implement `remove_descriptor` and `leave_own_publication`**

`remove_descriptor` удаляет `default.vrd`, отсутствие файла — успех. `leave_own_publication` в режиме `webserver` отдаёт `Ok(false)` и ничего не делает; иначе гасит свой httpd (`stop_own_publication`) и зовёт `remove_descriptor` по `publication::Paths::of(&ctx.cfg, &ctx.kit)?.pub_dir`. Зовётся, пока в параметрах ещё стоит `auto`: записав режим первым, мы оставили бы httpd жить.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p onec-connect --lib publication::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/publication.rs setup/1c-connect/src/ops_host.rs
git commit -m "Уход в чужую публикацию забирает наш дескриптор с паролем базы"
```

---

### Task 6: Подменю «Публикация» в трее

**Files:**
- Modify: `setup/1c-connect/src/gui/tray.rs` (`Tray` — `tray.rs:25`, `MODES` — `tray.rs:41`, `MenuAction` — `tray.rs:45`, `menu_action` — `tray.rs:64`, `build` — `tray.rs:139`, `set_mode` — `tray.rs:225`, `on_menu`/`MenuAction::Mode` — `tray.rs:341`, `apply_mode` — `tray.rs:288`), `setup/1c-connect/src/gui/mod.rs` (`tray.set_mode` при запуске — `mod.rs:2867`)
- Test: `setup/1c-connect/src/gui/tray.rs`, модуль `tests`

**Interfaces:**
- Consumes: `ops_host::leave_own_publication` (задача 5), `save::save_needs_confirm`, `gui::confirm_database_change`, `tray::mode_change_blocked`.
- Produces: `MenuAction::Publication(&'static str)`, `Tray::set_publication(&self, publication: &str)`, `tray::PUBLICATIONS`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_publication_submenu_speaks_in_human_words() {
    assert_eq!(menu_action("publication:auto"), Some(MenuAction::Publication("auto")));
    assert_eq!(menu_action("publication:webserver"), Some(MenuAction::Publication("webserver")));
    assert_eq!(menu_action("publication:iis"), None);
    // Значения toml не меняются, меняются подписи.
    let labels: Vec<&str> = PUBLICATIONS.iter().map(|(_, _, text)| *text).collect();
    assert_eq!(labels, vec!["Публикует коннектор", "Уже опубликована"]);
    assert_eq!(PUBLICATIONS.map(|(v, ..)| v), ["auto", "webserver"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p onec-connect --lib gui::tray::`
Expected: FAIL — `PUBLICATIONS` и `MenuAction::Publication` не определены.

- [ ] **Step 3: Add the submenu by the shape of «Режим работы» in `setup/1c-connect/src/gui/tray.rs`**

`const PUBLICATIONS: [(&str, &str, &str); 2]` со значениями `auto`/`webserver`, id `publication:auto`/`publication:webserver` и подписями из теста; `Tray.publications: [CheckMenuItem; 2]`; `Submenu::with_items("Публикация", …)` следом за «Режим работы»; `set_publication` по образцу `set_mode`; `launch` поправляет отметку из прочитанного toml там же, где зовёт `set_mode`.

- [ ] **Step 4: Implement the handler `MenuAction::Publication(p)` in `on_menu`**

Повторяет `MenuAction::Mode` шаг в шаг: отказ под идущей операцией (`mode_change_blocked`) с возвратом отметки на действующее значение; совпадение с текущим — тоже возврат; затем `leave_own_publication` (только при уходе в `webserver`), и `apply_publication(cx, p)` — прямо или после `confirm_database_change`, если `save_needs_confirm`. `apply_publication` пишет `ui.cfg.host.publication`, зовёт `save_host`, двигает отметку и перезапускает сеанс — как `apply_mode`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p onec-connect --lib gui::`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add setup/1c-connect/src/gui/tray.rs setup/1c-connect/src/gui/mod.rs
git commit -m "Подменю «Публикация» в трее: выбор человека, не правка toml"
```

---

### Task 7: Отказы в режиме «уже опубликована» называют причину

**Files:**
- Modify: `setup/1c-connect/src/verify.rs` (`extension_fact` — `verify.rs:602`), `setup/1c-connect/src/ops_host.rs` (`Reach` — `ops_host.rs:865`)
- Test: `setup/1c-connect/src/verify.rs`, модуль `tests`

**Interfaces:**
- Consumes: `ops_host::Reach`.
- Produces: `verify::extension_fact(reach: Option<&Reach>, busy: Option<(&str, &str)>, publication: &str) -> Check`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_404_in_a_foreign_publication_points_at_the_checkbox() {
    let c = extension_fact(Some(&Reach::Absent), None, "webserver");
    assert!(c.text.contains("HTTP-сервисы расширений"), "{}", c.text);
    assert!(!c.ok);
}

#[test]
fn a_404_in_our_own_publication_still_means_the_extension_is_missing() {
    let c = extension_fact(Some(&Reach::Absent), None, "auto");
    assert_eq!(c.text, "расширения в базе нет");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect --lib verify::`
Expected: FAIL — `extension_fact` принимает два аргумента.

- [ ] **Step 3: Thread the publication into `extension_fact` in `setup/1c-connect/src/verify.rs`**

В режиме `webserver` исход `Reach::Absent` говорит, что расширение в базе есть, а его HTTP-сервис не опубликован, и называет галочку: «Конфигуратор → Публикация на веб-сервере → HTTP-сервисы → „Публиковать HTTP-сервисы расширений по умолчанию“». Подсказка факта — туда же, а не на `base_url` в toml. В режиме `auto` текст прежний.

- [ ] **Step 4: Say the price of a foreign publication where the person meets it**

Там же, в режиме `webserver`, исход «база занята» (`Reach::Refused` с `onec_error::is_busy`) говорит, что база занята сеансом агента и это ожидаемо: сеансами нашего сервиса в чужой публикации не управляет никто. Их родной способ — перезапуск платформы — называем словами; сами не перезапускаем ничего.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p onec-connect`
Expected: PASS целиком — набор фактов зовут и окно, и `check`.

- [ ] **Step 6: Commit**

```bash
git add setup/1c-connect/src/verify.rs setup/1c-connect/src/ops_host.rs
git commit -m "404 от чужой публикации больше не зовёт переустанавливать расширение"
```

---

### Task 8: Слова для человека — установка и спека

**Files:**
- Modify: `УСТАНОВКА.md`, `docs/superpowers/specs/2026-10-16-gpui-connect-design.md` (раздел про публикацию), `docs/STATUS.md` (задача 28)
- Test: нет; проверка — чтение и `cargo test -p onec-connect`

- [ ] **Step 1: Describe the one checkbox in `УСТАНОВКА.md`**

Шагом playbook, в той же форме, что соседние: что сделать, и какой ответ ожидать. Текст: если база уже опубликована, выберите в меню значка «Публикация → Уже опубликована», впишите адрес, как открываете базу в браузере, и поставьте в Конфигураторе галочку «Публиковать HTTP-сервисы расширений по умолчанию». Перезапуск веб-сервера не нужен — проверено замером.

- [ ] **Step 2: Record the price in the spec**

В спеке окна — две формы режима и их цена: свой httpd отпускает базу сразу, чужая публикация переиспользует соединение и держит её, пока живёт пул. Это цена чужой публикации, а не наша недоделка.

- [ ] **Step 3: Close task 28 in `docs/STATUS.md`**

«ПОЧИНЕНО В КОДЕ, ЖИВЬЁМ НЕ ПРОВЕРЕНО» со ссылкой на замысел и на этот план, и с условием закрытия из замысла: на машине с настоящей публикацией режим работает, `default.vrd` после перехода нет, возврат поднимает наш httpd на `808N`.

- [ ] **Step 4: Commit**

```bash
git add УСТАНОВКА.md docs/superpowers/specs/2026-10-16-gpui-connect-design.md docs/STATUS.md
git commit -m "Режим «уже опубликована»: одна галочка в Конфигураторе и её цена"
```

---

## Чем доказывается, что сделано

`cargo test -p onec-connect` зелёный, `cargo fmt --check` чистый.

Делом, на машине с настоящей публикацией (в план не входит — отдельный шаг перед тем, как называть режим готовым): база, опубликованная Apache на 80, видна коннектору в режиме «уже опубликована»; `default.vrd` после перехода на диске нет; возврат на «публикует коннектор» поднимает наш httpd на `808N` и не падает на занятом 80. Плюс незакрытый замер из замысла: держится ли файловая база после вызова агента и сколько.
