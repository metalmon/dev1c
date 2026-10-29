# MCP в редакторе на машине человека — план работ

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Человек ставит коннектор на свою машину, нажимает в окне «Настроить», и в Zed и pi появляются работающие серверы `1c` и `vrunner`; включение и выключение связи больше никогда не требует идти в редактор и жать перезапуск.

**Architecture:** Мост уже умеет всё нужное — `Serve::dispatch` разбирает «порт → процесс» (`session.rs:350`), `pipe_jsonrpc` обобщён по `AsyncRead + AsyncWrite` (`bridge.rs:353`), переживает рестарт ребёнка и отвечает `degraded_reply`, пока ребёнка нет. Работа добавляет к нему три поверхности: подкоманду `1c-connect mcp 1c|vrunner`, которую редактор запускает как свой stdio-сервер (транспорт — `tokio::io::join(stdin, stdout)` вместо TCP, тот же `dispatch`); модуль `editors.rs`, который держит наши две записи в чужих файлах настроек; и раздел «Редакторы» в окне. Плюс два исправления, без которых второй процесс опасен: межпроцессный замок проекта и признак разрешения на диске вместо `AtomicBool` в памяти окна.

**Tech Stack:** Rust, GPUI, пакет `onec-connect` (бинарь `1c-connect`), `russh`, `serde_json`; тесты — `cargo test -p onec-connect`.

**Spec:** `docs/superpowers/specs/2026-10-26-editor-mcp-design.md`

## Global Constraints

Через мост идут ровно два наших сервера: `mcp-1c` (`1c`) и `vrunner-mcp` (`vrunner`). `bsl-reference` и всё, что человек поставил сам, не трогаем ни в настройках, ни в трафике.

Чужую запись не правим и не перезаписываем никогда — ни в `settings.json` Zed, ни в `mcp.json` pi. Запись считается нашей, если её `command` — путь к нашему исполняемому файлу (`std::env::current_exe()`, сравнение по имени файла); в контейнере, где `command` — это `nc`, наши записи опознаются по именам `1c` и `vrunner`.

Путь к исполняемому файлу в JSON пишется экранированным (`C:\\Program Files\\...`), и файл сохраняется с теми переводами строк и той BOM, что в нём были.

Подкоманда `mcp` не берёт замок рабочего места (`single_instance`): его держит окно. Своего ребёнка (`mcp-1c`/`vrunner-mcp`) она поднимает сама.

Срок действия и холодный замок (`licence::State`, `gui::gate::cold_gate`) проверяются подкомандой до первого ответа агенту — иначе самостоятельный режим обходит замок готовности.

Нет окна — нет работы: `tools/call` получает отказ словами. `initialize` и `tools/list` отвечают всегда, потому что список инструментов Zed спрашивает один раз за сеанс и `notifications/tools/list_changed` не обрабатывает.

Пути настроек: Zed — `%APPDATA%\Zed\settings.json` на Windows, `$XDG_CONFIG_HOME/zed/settings.json` (умолчание `~/.config`) на Linux; pi — `~/.pi/agent/mcp.json`, на Windows под `%USERPROFILE%`.

Таймауты наших записей те же, что в образе: `1c` — 300 с, `vrunner` — 3600 с (`config/zed-settings.json`, `config/pi/mcp.json`).

## Review Focus

Файла настроек Zed ещё нет вовсе (редактор не запускался): «Настроить» обязана создать каталог и файл с одними нашими записями, а не отказать — задача 4, тест `an_absent_zed_file_is_created_with_only_our_entries`.

Настройки Zed с BOM и CRLF — после правки файл остаётся с BOM и CRLF, иначе редактор человека покажет один сплошной абзац: задача 4, тест `line_endings_and_bom_survive_the_merge`.

Путь с пробелами и кириллицей (`C:\Users\Иван Петров\...`) — в JSON уходит экранированным и разбирается обратно: задача 4, тест `a_windows_path_is_escaped_and_parses_back`.

Окно сняли `taskkill /F`, признак разрешения остался «включено» — следующий `tools/call` всё равно получает отказ, потому что живость окна проверяется замком, а не содержимым файла: задача 2, тест `a_stale_allow_file_does_not_allow`.

Два клиента разом (Zed и pi) зовут `project_push` — второй получает «занято», а не вторую выгрузку в тот же каталог: задача 1, тест `a_second_process_is_refused_not_queued`.

---

### Task 1: Замок проекта переживает второй процесс

**Files:**
- Modify: `setup/1c-connect/src/project.rs` (`LOCK` — `project.rs:509`, `facts_locked` — `project.rs:530`, `call` — `project.rs:547`)
- Modify: `setup/1c-connect/src/single_instance.rs` (вынести приём замка на произвольный файл)
- Test: там же, модули `tests`

**Interfaces:**
- Produces: `project::file_lock(path: &std::path::Path) -> std::io::Result<Option<project::FileLock>>` — приём замка на файле (`None` — занят); `project::set_lock_target(dir: &std::path::Path)` — зовётся один раз при старте процесса (окно, `serve`, `connect`, подкоманда `mcp`); `project::hold(what: &str) -> Result<project::Hold, String>` — берёт мьютекс в памяти и замок на файле `<state>/project.lock`, `Err` — готовая строка «занято: идёт project_push»; `facts_locked` и `call` работают через `hold`.
- Consumes: `single_instance::try_acquire` (тот же приём: именованный мьютекс на Windows, `flock` на Linux).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_second_process_is_refused_not_queued() {
    let d = tempdir();
    // Два захвата по одному пути — то же, что два процесса: flock держится
    // за открытый дескриптор, именованный мьютекс — за объект ядра.
    let first = project::file_lock(&d.path().join("project.lock")).unwrap().unwrap();
    assert!(project::file_lock(&d.path().join("project.lock")).unwrap().is_none());
    drop(first);
    assert!(project::file_lock(&d.path().join("project.lock")).unwrap().is_some());
}

#[test]
fn a_busy_lock_names_what_is_running() {
    let d = tempdir();
    project::set_lock_target(d.path());
    let _g = project::hold("project_push").unwrap();
    assert_eq!(project::hold("project_status").unwrap_err(), "занято: идёт project_push");
}

#[test]
fn without_a_target_the_lock_is_in_memory_only() {
    // Разовые команды (`status`, `example`) цели не ставят: отказа быть не должно.
    let _g = project::hold("project_status").unwrap();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect project_push -- --include-ignored`
Expected: FAIL, `project::file_lock` и `project::hold` не существуют.

- [ ] **Step 3: Implement `project::file_lock`, `project::set_lock_target`, `project::hold`**

`file_lock` — тот же код, что в `single_instance` (на Windows именованный мьютекс с ключом `single_instance::guard_key` плюс суффикс `:project`, на Linux `flock` на переданном файле), вынесенный так, чтобы его звали оба. Файл замка — свой (`<state>/project.lock`), не каталог состояния: каталог уже держит замок рабочего места, и окно заперло бы само себя. `hold` берёт сперва мьютекс в памяти (быстрый путь и та же строка «занято»), затем файл, если цель задана.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect project`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/project.rs setup/1c-connect/src/single_instance.rs
git commit -m "Замок проекта держится на файле: второй процесс получает «занято»"
```

---

### Task 2: Признак разрешения живёт на диске

**Files:**
- Create: `setup/1c-connect/src/allow.rs`
- Modify: `setup/1c-connect/src/lib.rs` (объявить модуль), `setup/1c-connect/src/gui/mod.rs` (запись при щелчке переключателя и при выходе — рядом с `ui.enabled`, `gui/mod.rs:137`)
- Test: `setup/1c-connect/src/allow.rs`, модуль `tests`

**Interfaces:**
- Produces: `allow::decide(file: Option<&str>, window_alive: bool) -> bool` (чистая); `allow::path(kit: &Kit) -> PathBuf`; `allow::write(kit: &Kit, on: bool)`; `allow::read(kit: &Kit, user: &str, workstation: &str) -> bool`; `allow::watch(kit: &Kit, user: String, workstation: String) -> Arc<AtomicBool>` — задача опроса, которую берёт подкоманда `mcp`.
- Consumes: `single_instance::try_acquire` — удалось взять замок рабочего места, значит окна нет и работа запрещена.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn no_window_means_no_work() {
    assert!(!decide(Some("on"), false));
    assert!(!decide(None, false));
}

#[test]
fn a_stale_allow_file_does_not_allow() {
    // Окно сняли taskkill: файл остался «on», но замок свободен.
    assert!(!decide(Some("on"), false));
}

#[test]
fn the_switch_decides_while_the_window_lives() {
    assert!(decide(Some("on"), true));
    assert!(!decide(Some("off"), true));
    // Чего не понимаем — запрещаем: файл чужой или полузаписанный.
    assert!(!decide(Some(""), true));
    assert!(!decide(Some("вкл"), true));
    assert!(!decide(None, true));
}

#[test]
fn a_written_flag_reads_back() {
    let kit = kit_in(tempdir());
    write(&kit, true);
    assert_eq!(std::fs::read_to_string(path(&kit)).unwrap().trim(), "on");
    write(&kit, false);
    assert_eq!(std::fs::read_to_string(path(&kit)).unwrap().trim(), "off");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect allow`
Expected: FAIL, модуля `allow` нет.

- [ ] **Step 3: Implement `allow.rs` и запись из окна**

`decide` закрыта по умолчанию: разрешение — только `"on"` при живом окне. `watch` — задача, которая раз в 200 мс складывает `read` в `AtomicBool`; `ponytail:` комментарием назвать потолок: вызов, пришедший в эти 200 мс после выключения, ещё пройдёт. Окно пишет файл там же, где взводит `ui.enabled`, и пишет `off` при выходе (включая закрытие окна в трее).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect allow`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/allow.rs setup/1c-connect/src/lib.rs setup/1c-connect/src/gui/mod.rs
git commit -m "Разрешение на работу — файл в каталоге состояния, а не флаг в памяти окна"
```

---

### Task 3: Подкоманда `1c-connect mcp 1c|vrunner`

**Files:**
- Modify: `setup/1c-connect/src/main.rs` (`enum Cmd` — `main.rs:50`, разбор в `main`)
- Modify: `setup/1c-connect/src/ops_host.rs` (точка запуска подкоманды рядом с `serve`)
- Test: `setup/1c-connect/src/session.rs`, модуль `tests` (stdio-путь через `tokio::io::duplex`)

**Interfaces:**
- Consumes: `project::set_lock_target` (задача 1), `allow::watch` (задача 2), `Serve::new`/`Serve::dispatch` (`session.rs:327`, `session.rs:350`), `licence::State`, `gui::gate::cold_gate`.
- Produces: `Cmd::Mcp { server: McpServer }` (`McpServer` — `clap::ValueEnum` со значениями `1c` и `vrunner`), `ops_host::run_mcp(server: McpServer) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn the_stdio_path_answers_initialize_without_a_child() {
    // Комплекта в тестах нет, ребёнок не поднимется — и это главный случай:
    // мост обязан ответить сам, иначе Zed сочтёт сервер упавшим.
    let (mine, theirs) = tokio::io::duplex(64 * 1024);
    let serve = Serve::new(&filled(), &kit_in(tempdir()), Arc::new(|_| {}), ports(), Arc::new(AtomicBool::new(true)));
    let task = serve.dispatch(ports().mcp, theirs).unwrap();
    let reply = ask(mine, br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).await;
    assert!(reply.contains("\"result\""), "{reply}");
    task.abort();
}

#[test]
fn the_server_argument_picks_the_port() {
    assert_eq!(port_of(McpServer::OneC, ports()), ports().mcp);
    assert_eq!(port_of(McpServer::Vrunner, ports()), ports().vrunner);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect stdio_path`
Expected: FAIL, `port_of` и `McpServer` не существуют.

- [ ] **Step 3: Implement `Cmd::Mcp` и `ops_host::run_mcp`**

Поток — `tokio::io::join(tokio::io::stdin(), tokio::io::stdout())`: он реализует и `AsyncRead`, и `AsyncWrite`, и подставляется в `dispatch` вместо TCP. Порядок внутри: прочитать параметры и комплект, `project::set_lock_target`, проверить срок и холодный замок (отказ — одной строкой в stderr и выход с кодом, редактор покажет его человеку), `enabled` взять из `allow::watch`, собрать `Serve::new` и позвать `dispatch(port_of(server, ports), stdio)`. Журнал идёт в файл (`Kit::cmd_log`), не в stdout: stdout — это труба MCP.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect -- stdio_path port`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/main.rs setup/1c-connect/src/ops_host.rs setup/1c-connect/src/session.rs
git commit -m "Подкоманда mcp: редактор запускает мост на stdio, а не ищет порт"
```

---

### Task 4: Наши две записи в настройках Zed

**Files:**
- Create: `setup/1c-connect/src/editors.rs`
- Modify: `setup/1c-connect/src/lib.rs`
- Test: `setup/1c-connect/src/editors.rs`, модуль `tests`

**Interfaces:**
- Produces: `editors::Entry { name: &'static str, timeout: u32 }` и `editors::OURS: [Entry; 2]`; `editors::zed_path() -> Option<PathBuf>`; `editors::merge_zed(text: &str, exe: &Path) -> Result<String, Vec<String>>` (`Err` — имена занятых чужих записей); `editors::set_enabled_zed(text: &str, on: bool) -> String` (задача 7); `common::jsonc::as_json(text: &str) -> String` — снятие комментариев, вынесенное из теста `cfg.rs:428` в общее место (им пользуются и тесты этой задачи).
- Consumes: ничего из других задач.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_absent_zed_file_is_created_with_only_our_entries() {
    let out = merge_zed("", Path::new("/opt/1c-connect")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
    assert_eq!(v["context_servers"]["1c"]["command"], "/opt/1c-connect");
    assert_eq!(v["context_servers"]["1c"]["args"], serde_json::json!(["mcp", "1c"]));
    assert_eq!(v["context_servers"]["1c"]["timeout"], 300);
    assert_eq!(v["context_servers"]["vrunner"]["timeout"], 3600);
}

#[test]
fn foreign_keys_and_comments_survive_the_merge() {
    let before = "{\n  // моя тема\n  \"theme\": \"One Dark\",\n  \"context_servers\": {\n    \"bsl-reference\": {\"command\": \"/opt/bsl-analyzer\"}\n  }\n}\n";
    let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
    assert!(out.contains("// моя тема"), "{out}");
    assert!(out.contains("bsl-reference"), "{out}");
    assert!(out.contains("\"1c\""), "{out}");
}

#[test]
fn a_foreign_entry_with_our_name_is_reported_not_overwritten() {
    let before = "{\"context_servers\":{\"1c\":{\"command\":\"nc\",\"args\":[\"-N\",\"localhost\",\"7771\"]}}}";
    assert_eq!(merge_zed(before, Path::new("/opt/1c-connect")).unwrap_err(), vec!["1c".to_string()]);
}

#[test]
fn our_own_entry_is_updated_in_place() {
    let before = "{\"context_servers\":{\"1c\":{\"command\":\"/old/1c-connect\",\"args\":[\"mcp\",\"1c\"]}}}";
    let out = merge_zed(before, Path::new("/new/1c-connect")).unwrap();
    assert!(out.contains("/new/1c-connect") && !out.contains("/old/1c-connect"), "{out}");
}

#[test]
fn a_windows_path_is_escaped_and_parses_back() {
    let out = merge_zed("", Path::new(r"C:\Users\Иван Петров\1c-connect.exe")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
    assert_eq!(v["context_servers"]["1c"]["command"], r"C:\Users\Иван Петров\1c-connect.exe");
}

#[test]
fn line_endings_and_bom_survive_the_merge() {
    let before = "\u{feff}{\r\n  \"theme\": \"One\"\r\n}\r\n";
    let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
    assert!(out.starts_with('\u{feff}') && out.contains("\r\n") && !out.contains("\n\n"), "{out:?}");
}

#[test]
fn enabled_flips_without_touching_anything_else() {
    let on = merge_zed("", Path::new("/opt/1c-connect")).unwrap();
    let off = set_enabled_zed(&on, false);
    let v: serde_json::Value = serde_json::from_str(&as_json(&off)).unwrap();
    assert_eq!(v["context_servers"]["1c"]["enabled"], false);
    assert_eq!(v["context_servers"]["vrunner"]["enabled"], false);
    assert_eq!(set_enabled_zed(&off, true), on);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect editors`
Expected: FAIL, модуля `editors` нет.

- [ ] **Step 3: Implement `editors.rs`: пути, наши записи, хирургическая правка JSONC**

Правка текстовая, по границам нашей записи: разбирать документ целиком и печатать заново нельзя — файл человека несёт комментарии. Границы искать разбором по скобкам от ключа `"context_servers"`, снимая комментарии только для сравнения (`as_json` уже есть в `cfg.rs:428` — вынести в общее место, а не копировать). Наша запись — та, у которой `command` оканчивается тем же именем файла, что наш исполняемый (`1c-connect`/`1c-connect.exe`); всё прочее — чужое, и это `Err` со списком имён. Переводы строк и BOM определяются по входному тексту и воспроизводятся.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect editors`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/editors.rs setup/1c-connect/src/lib.rs setup/1c-connect/src/cfg.rs
git commit -m "Свои записи в настройках Zed: вписываем свои, чужое не трогаем"
```

---

### Task 5: Те же две записи в настройках pi

**Files:**
- Modify: `setup/1c-connect/src/editors.rs`
- Test: там же

**Interfaces:**
- Produces: `editors::pi_path() -> Option<PathBuf>`; `editors::merge_pi(text: &str, exe: &Path) -> Result<String, Vec<String>>`.
- Consumes: `editors::OURS` (задача 4).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn pi_gets_the_same_two_servers_under_its_own_key() {
    let out = merge_pi("", Path::new("/opt/1c-connect")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["mcpServers"]["1c"]["args"], serde_json::json!(["mcp", "1c"]));
    assert_eq!(v["mcpServers"]["vrunner"]["timeout"], 3600);
}

#[test]
fn pi_keeps_foreign_servers_and_foreign_keys() {
    let before = r#"{"_comment":"мой файл","mcpServers":{"bsl-reference":{"command":"/opt/bsl-analyzer"}}}"#;
    let out = merge_pi(before, Path::new("/opt/1c-connect")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["_comment"], "мой файл");
    assert!(v["mcpServers"]["bsl-reference"].is_object());
    assert!(v["mcpServers"]["1c"].is_object());
}

#[test]
fn a_foreign_pi_entry_with_our_name_is_reported() {
    let before = r#"{"mcpServers":{"vrunner":{"command":"nc","args":["-N","localhost","7781"]}}}"#;
    assert_eq!(merge_pi(before, Path::new("/opt/1c-connect")).unwrap_err(), vec!["vrunner".to_string()]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect pi_`
Expected: FAIL, `merge_pi` не существует.

- [ ] **Step 3: Implement `merge_pi`**

`mcp.json` — обычный JSON без комментариев, поэтому здесь честен разбор и печать через `serde_json` (с сохранением порядка ключей), а не текстовая хирургия Zed. `pi mcp add` не зовём: его ключ для таймаута не проверен, а формат файла нам известен целиком — одно место правды вместо двух. Человеку нужен `/reload` или новый сеанс pi, и об этом говорит окно (задача 6).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect pi_`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/editors.rs
git commit -m "Настройки pi: те же два сервера, разбором, а не текстом"
```

---

### Task 6: Раздел «Редакторы» в окне

**Files:**
- Modify: `setup/1c-connect/src/editors.rs` (опознание состояния), `setup/1c-connect/src/gui/panel.rs` (строки и состояние кнопки, рядом с `fact_rows` — `panel.rs:363`), `setup/1c-connect/src/gui/mod.rs` (действие кнопки)
- Test: `setup/1c-connect/src/gui/panel.rs` и `setup/1c-connect/src/editors.rs`, модули `tests`

**Interfaces:**
- Produces: `editors::Found { Missing, Ours, Foreign(Vec<String>), Absent }`; `editors::look(path: Option<&Path>, exe: &Path) -> Found`; `panel::editor_rows(zed: &Found, pi: &Found) -> Vec<(Option<Dot>, String, String)>`; `panel::setup_locked(f: &Found) -> bool`.
- Consumes: `merge_zed`, `merge_pi`, `zed_path`, `pi_path` (задачи 4 и 5).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn each_editor_says_what_is_true_about_it() {
    assert_eq!(editor_rows(&Found::Missing, &Found::Ours)[0].2, "не найден на этой машине");
    assert_eq!(editor_rows(&Found::Absent, &Found::Ours)[0].2, "найден, серверы не настроены");
    assert_eq!(editor_rows(&Found::Ours, &Found::Ours)[0].2, "настроен");
    assert_eq!(
        editor_rows(&Found::Foreign(vec!["1c".into()]), &Found::Ours)[0].2,
        "запись 1c занята чужой командой — не меняем"
    );
}

#[test]
fn the_button_is_locked_where_pressing_it_would_do_nothing_or_harm() {
    assert!(setup_locked(&Found::Missing));
    assert!(setup_locked(&Found::Foreign(vec!["1c".into()])));
    assert!(!setup_locked(&Found::Absent));
    assert!(!setup_locked(&Found::Ours));
}

#[test]
fn pi_says_that_a_running_session_needs_reload() {
    let rows = editor_rows(&Found::Missing, &Found::Ours);
    assert!(rows[1].2.contains("/reload"), "{:?}", rows[1]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect editor_rows`
Expected: FAIL, `editor_rows` не существует.

- [ ] **Step 3: Implement `look`, `editor_rows`, `setup_locked` и кнопку**

Кнопка «Настроить» зовёт `merge_*` и записывает файл рядом через временный файл и `rename`, оставив `.bak` первой правки — тем же приёмом, что раскладка настроек в образе. Отказ (`Err`) не пишет ничего и показывает строку про занятые имена. После записи состояние перечитывается, чтобы раздел говорил о файле, а не о нашем намерении.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect -- editor_rows setup_locked`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/editors.rs setup/1c-connect/src/gui/panel.rs setup/1c-connect/src/gui/mod.rs
git commit -m "Раздел «Редакторы»: что найдено, что настроено, и кнопка на одно действие"
```

---

### Task 7: В контейнере сервер выключается, а не падает

**Files:**
- Modify: `setup/1c-connect/src/tunnel.rs` (канал команды рядом с пробросом портов), `setup/1c-connect/src/session.rs` (вызов после удачного проброса и перед разбором сеанса)
- Test: `setup/1c-connect/src/editors.rs` (чистая часть уже покрыта задачей 4), `setup/1c-connect/src/session.rs` (порядок вызовов)

**Interfaces:**
- Consumes: `editors::set_enabled_zed` (задача 4), `editors::look` (задача 6).
- Produces: `Tunnel::exec(&self, cmd: &str) -> Result<String, String>`; `session::flip_with(exec: &dyn Fn(&str) -> Result<String, String>, on: bool) -> Result<(), String>` — вся логика здесь, канал подставляется (так она и проверяется); `session::flip_remote_servers(t: &Tunnel, on: bool) -> Result<(), String>` — обёртка, подставляющая `Tunnel::exec`; `verify::editor_mcp_fact(found: Option<&editors::Found>) -> Check` — факт «записи MCP в редакторе», `None` значит «проверить не удалось».

Решение владельца 29.10: отсутствие наших записей в настройках редактора — это ФАКТ набора готовности, а не строка в журнале. Иначе статус зелёный при том, что к нам никто не придёт: туннель поднят, мост отвечает, а у агента нет ни одного инструмента 1С. Факт горячий — красит точку жёлтым и называет причину, но переключатель не запирает (`gui/gate.rs`, «горячее против холодного»): запри его, и человек не смог бы включиться, чтобы починить. Место — после `TUNNEL` в `verify::order` (`verify.rs:54`), и он же в локальном режиме, где читается не по SSH, а прямо с диска через `editors::look`. Три состояния, чтобы факт не врал в обе стороны: записи есть — зелёный; записей нет — жёлтый, «в контейнере нет записей MCP, обновите образ»; прочитать не удалось — жёлтый, «не смог проверить», а не «нет записей».

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn flipping_the_remote_file_reads_modifies_writes() {
    // Канал подменён: проверяем, что мы читаем файл, правим своей же
    // функцией и пишем обратно — ни одного sed со стороны.
    let seen = std::sync::Mutex::new(Vec::new());
    let exec = |cmd: &str| -> Result<String, String> {
        seen.lock().unwrap().push(cmd.to_string());
        Ok(merge_zed("", Path::new("/opt/1c-connect")).unwrap())
    };
    flip_with(&exec, false).unwrap();
    let seen = seen.lock().unwrap();
    assert!(seen[0].contains("cat") && seen[0].contains(".config/zed/settings.json"), "{:?}", seen[0]);
    assert!(seen[1].contains("\"enabled\": false"), "{:?}", seen[1]);
}

#[test]
fn a_missing_remote_file_is_not_an_error_for_the_session() {
    let exec = |_: &str| -> Result<String, String> { Err("No such file".into()) };
    assert!(flip_with(&exec, true).is_err());
    // но сеанс от этого не падает — проверяется в session.rs тем,
    // что ошибка уходит в журнал предупреждением.
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect flipping_the_remote`
Expected: FAIL, `flip_with` не существует.

- [ ] **Step 3: Implement `Tunnel::exec` и `flip_remote_servers`**

Читаем файл настроек по SSH (`cat`), правим своей `set_enabled_zed`, пишем обратно (`cat > …` на втором канале). Своего скрипта в образе не заводим и `sed` не посылаем: правка живёт в одном месте — в `editors.rs`. Ошибка правки — предупреждение в журнал, а не отказ подключения: связь важнее настройки редактора. `ponytail:` комментарием назвать потолок: человек, правящий настройки ровно в этот миг, потеряет свою правку — цена honest read-modify-write без блокировки чужого файла.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect -- flipping_the_remote remote_file`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/tunnel.rs setup/1c-connect/src/session.rs
git commit -m "Выключение связи гасит серверы MCP в Zed, а не ломает их"
```

---

### Task 8: Слова для человека и проверка на стенде

**Files:**
- Modify: `README.md`, `УСТАНОВКА.md`, `docs/superpowers/specs/2026-10-26-editor-mcp-design.md` (отметить проверенное)
- Test: ручная проверка по списку ниже, отчёт — в коммит

- [ ] **Step 1: Проверить на стенде то, что спека назвала непроверенным**

Открывает ли `mcp-1c` соединение с базой при старте (от этого зависит, можно ли поднимать ребёнка при выключенном переключателе). Два клиента разом на файловой базе: что видит второй. `/reload` в pi действительно подхватывает дописанные серверы. Записать ответы в спеку строкой «замер 28.10».

- [ ] **Step 2: Прогнать путь человека целиком**

Чистая машина без нашего образа: установка, «Настроить», Zed поднял серверы сам; выключение связи — отказ словами и живой сервер; закрытое окно — отказ про окно; контейнерный режим — «выключен» вместо «упал»; два `project_push` разом — второй «занято».

- [ ] **Step 3: Описать это в README и УСТАНОВКА.md**

Раздел про редакторы: что делает кнопка, где лежат файлы, что делать, если запись занята чужой командой, и что pi в открытом сеансе нужен `/reload`. Тем же языком, что остальная установка — шаг и ожидаемый ответ.

- [ ] **Step 4: Commit**

```bash
git add README.md УСТАНОВКА.md docs/superpowers/specs/2026-10-26-editor-mcp-design.md
git commit -m "Редакторы: слова для человека и замеры, которых спека ждала"
```

---

## Чем доказывается, что сделано

`cargo test -p onec-connect` зелёный; `cargo fmt --check` и `cargo clippy --workspace -- -D warnings` чисты (гейт на пуш).

Zed на машине без нашего образа: нажата кнопка, серверы поднялись сами, оба инструмента отвечают, чужие комментарии и ключи в `settings.json` на месте — сверено до и после.

Переключатель выключен — `tools/call` отказ с причиной, `tools/list` тот же список; окно закрыто — отказ про незапущенное окно; включено снова — тот же сеанс редактора работает без единого перезапуска сервера.

Контейнерный режим: выключение связи показывает в Zed «выключен», включение поднимает серверы без участия человека.

Контейнер без наших записей в настройках Zed (старый образ или человек их удалил): точка жёлтая, причина называет отсутствие записей, переключатель при этом не заперт — включиться, чтобы починить, человек может.

Два `project_push` из двух клиентов разом: второй получает «занято», а не вторую выгрузку в тот же каталог.
