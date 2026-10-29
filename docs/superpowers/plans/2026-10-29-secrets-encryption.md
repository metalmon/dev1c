# Шифрование паролей в файле параметров — план работ

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** В `1c-connect.toml` не остаётся ни одного пароля открытым текстом, в дескрипторе публикации — ни логина, ни пароля, и файл параметров, попавший в бэкап или на другую машину, не даёт доступа к базе.

**Architecture:** Новый модуль `secrets.rs` держит формат (`enc1:` плюс шифртекст) и две подложки: DPAPI пользователя на Windows (ключа на диске нет, шифртекст не открывается ни на другой машине, ни другим пользователем) и файл ключа `0600` с AEAD на Linux и в тестах. `Cfg` читает и шифртекст, и открытый текст, а печатает всегда шифртекст, поэтому первое же сохранение переводит существующую установку. Пароль базы заодно уходит из командных строк — `mcp-1c` читает его из окружения, — а из дескриптора публикации уходят и логин, и пароль: платформа берёт пользователя из запроса.

**Tech Stack:** Rust, GPUI, пакет `onec-connect`; `aes-gcm` 0.11 (уже в `Cargo.lock` через `russh`), `windows-sys` с фичей `Win32_Security_Cryptography`; тесты — `cargo test -p onec-connect --features gui`.

**Spec:** `docs/superpowers/specs/2026-10-29-secrets-encryption-design.md`

## Global Constraints

Шифруются два значения, оба в `1c-connect.toml`: `[host] password` (`cfg.rs:195`) и `[host] db_password` (`cfg.rs:205`). Ключ сервера модели в `dev1c.toml` не трогаем — решение владельца 29.10.

**Пустое значение остаётся пустым и не шифруется.** Иначе `render` перестаёт быть детерминированным (случайный `nonce` на каждое шифрование), и на образце, где оба пароля пусты, падают `render_keeps_comments_and_round_trips` (`cfg.rs:913`) и `example_prints_gitea_with_its_rule` (`cfg.rs:822`).

Значение без префикса читается как открытый текст **всегда**, а не только в переходный период: это и путь обновления, и законный выход для закрытого контура.

Перешифровка идёт при записи, не при чтении: разовая команда (`status`, `check`) не имеет права переписать файл параметров человека.

Новых библиотек не добавляем; добавить ФИЧУ уже присутствующего крейта можно, и где именно — решает замер задачи 0.

Коннектор службой под `SYSTEM` не планируется (решение владельца 29.10) — DPAPI пользователя выбран с этим знанием.

Обещание работы: **пароля не остаётся ни в файле параметров, ни в дескрипторе публикации**. Одно исключение называется честно: Конфигуратору пароль уходит ключом `/P` при снятии расширения (`ops_host.rs:1675`) — у платформы другого способа нет, и это мгновение одной команды.

Защиты от кода, запущенного под тем же пользователем, эта работа не даёт и обещать не должна.

Проза и комментарии по-русски, абзац одной строкой, объясняют «почему»; осознанные срезы помечаются `ponytail:` с названием потолка. Тесты гонять в переднем плане, не в фоне. Временные каталоги в тестах — местным приёмом `std::env::temp_dir().join(format!("…-{}", std::process::id()))` (`publication.rs:786`, `kit.rs:118`), а не новым крейтом.

## Review Focus

Пароль с кавычками, обратными косыми, кириллицей и пробелами (`па"роль`, `C:\x`, `пароль 1С`) — шифрование и обратный разбор обязаны вернуть ровно ту же строку: задача 1, тест `a_secret_with_quotes_and_cyrillic_survives_the_round_trip`.

Полупереведённый файл: один пароль уже в `enc1:`, второй ещё открытым — обе половины читаются правильно: задача 2, тест `a_half_migrated_file_reads_both_halves`.

Шифртекст, записанный другим пользователем или на другой машине: в CLI — отказ словами (иначе человек поедет с пустым паролем молча), в окне — пустое поле и причина: задача 2, тесты `a_cli_load_refuses_instead_of_running_with_an_empty_password` и `the_window_load_reports_the_field_instead_of_refusing`.

Запись, которая тихо не состоялась: после записи файл читается обратно и значения обязаны открываться — задача 2, тест `a_write_that_did_not_seal_is_caught_on_read_back`.

Публикация после задачи 6: вызов сервиса без учётных данных обязан получить `401`, а не сеанс с полными правами — задача 6, живая проверка задачи 7.

---

### Task 0: Замеры, без которых следующие задачи угадывают

**Files:**
- Modify: `docs/superpowers/specs/2026-10-29-secrets-encryption-design.md` (дописать ответы строкой «замер 29.10»)

Эта задача не пишет кода. Она отвечает на три вопроса, на которых стоят задачи 1, 5 и 6; без ответов исполнитель посреди работы упрётся в то, чего план не знал.

- [ ] **Step 1: Поддерживает ли комплектный `mcp-1c` переменную `MCP_1C_PASSWORD`**

В комплект едет upstream `v1.23.0` (`pack.sh:207`). Проверить на машине, где комплект есть: `mcp-1c-windows-amd64.exe --help`, либо поиск строки `MCP_1C_PASSWORD` в бинаре. Не поддерживает — задача 5 откладывается до обновления комплекта, и это записывается в спеку.

- [ ] **Step 2: Чем брать случайность для `nonce` и что это стоит**

`cargo tree -p onec-connect -i aes-gcm` и просмотр включённых фич: есть ли в дереве `getrandom`/`rand_core` с рабочим источником. Ответ решает, добавляется ли фича `aead/os_rng`, прямой `getrandom` — или ничего. Записать ответ и назвать его в задаче 1.

- [ ] **Step 3: Переживает ли факт расширения дескриптор без учётных данных**

`ops_host::extension_reach` (`ops_host.rs:1052`) ходит с `basic_auth(user, Some(password))`, и на стенде пароль пуст. Проверить на стенде: дескриптор без `Usr`/`Pwd` плюс пустой пароль — отдаёт версию или `Denied`. `Denied` значит, что факт `EXTENSION` после задачи 6 покраснеет на каждом стенде, и задача 6 обязана это лечить.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-10-29-secrets-encryption-design.md
git commit -m "Замеры перед шифрованием: окружение mcp-1c, источник случайности, факт расширения"
```

---

### Task 1: Модуль секретов — формат и две подложки

**Files:**
- Create: `setup/1c-connect/src/secrets.rs`
- Modify: `setup/1c-connect/src/lib.rs` (объявить модуль), `setup/1c-connect/Cargo.toml` (фича `Win32_Security_Cryptography` у `windows-sys`, прямая зависимость `aes-gcm = "0.11"`, и то, что назвал замер задачи 0 шаг 2)
- Test: `setup/1c-connect/src/secrets.rs`, модуль `tests`

**Interfaces:**
- Produces: `secrets::PREFIX` (`"enc1:"`); `secrets::Store` с двумя конструкторами — `Store::system(state_dir: &Path) -> Result<Store, String>` (на Windows DPAPI, каталог не используется; на Linux файл ключа в этом каталоге, он же `Kit::state`) и `Store::file(dir: &Path) -> Store` (файл ключа; им же пользуются тесты на обеих платформах); `Store::seal(&self, plain: &str) -> Result<String, String>` — пустую строку возвращает пустой, непустую отдаёт с префиксом; `Store::open(&self, value: &str) -> Result<String, OpenError>` — значение без префикса возвращает как есть; `OpenError::Foreign` — «шифртекст не наш» (другой пользователь, другая машина, ключа нет, ключ не тот), `OpenError::Broken(String)` — всё прочее.
- Третьей подложки-пустышки нет: `Store::file` во временном каталоге делает то же самое и честнее — проверяет настоящий путь шифрования.

- [ ] **Step 1: Write the failing tests**

```rust
fn tmp(name: &str) -> std::path::PathBuf {
    // Местный приём: без новых крейтов, уникально на процесс (publication.rs:786).
    let d = std::env::temp_dir().join(format!("secrets-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_value_without_the_prefix_comes_back_untouched() {
    let s = Store::file(&tmp("plain"));
    assert_eq!(s.open("открытый пароль").unwrap(), "открытый пароль");
    assert_eq!(s.open("").unwrap(), "");
}

#[test]
fn an_empty_secret_is_not_sealed() {
    let s = Store::file(&tmp("empty"));
    assert_eq!(s.seal("").unwrap(), "", "пустое значение обязано остаться пустым");
}

#[test]
fn a_sealed_value_carries_the_prefix_and_not_the_secret() {
    let s = Store::file(&tmp("seal"));
    let sealed = s.seal("пароль 1С").unwrap();
    assert!(sealed.starts_with(PREFIX), "{sealed}");
    assert!(!sealed.contains("пароль"), "секрет виден в шифртексте: {sealed}");
    assert_eq!(s.open(&sealed).unwrap(), "пароль 1С");
}

#[test]
fn a_secret_with_quotes_and_cyrillic_survives_the_round_trip() {
    let s = Store::file(&tmp("round"));
    for plain in [r#"па"роль"#, r"C:\x\y", "пароль 1С", "p w", "ПРОБЕЛ в конце "] {
        let sealed = s.seal(plain).unwrap();
        assert_eq!(s.open(&sealed).unwrap(), plain, "не сошлось на {plain:?}");
    }
}

#[test]
fn the_same_secret_seals_differently_twice() {
    // Случайный nonce: одинаковый шифртекст выдал бы, что пароль не менялся.
    let s = Store::file(&tmp("nonce"));
    assert_ne!(s.seal("секрет").unwrap(), s.seal("секрет").unwrap());
}

#[test]
fn a_foreign_ciphertext_is_named_foreign_not_broken() {
    let mine = Store::file(&tmp("mine"));
    let theirs = Store::file(&tmp("theirs"));
    let sealed = mine.seal("секрет").unwrap();
    assert!(matches!(theirs.open(&sealed), Err(OpenError::Foreign)));
}

#[test]
fn a_ciphertext_without_a_key_is_foreign_too() {
    let dir = tmp("nokey");
    let sealed = Store::file(&dir).seal("секрет").unwrap();
    std::fs::remove_file(dir.join("secret.key")).unwrap();
    assert!(matches!(Store::file(&dir).open(&sealed), Err(OpenError::Foreign)));
}

#[cfg(unix)]
#[test]
fn a_read_only_state_dir_is_a_named_refusal() {
    // Под root права не действуют — тогда проверять нечего, тест пропускается.
    if unsafe { libc::geteuid() } == 0 { return }
    let dir = tmp("readonly");
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o500)).unwrap();
    let e = Store::file(&dir).seal("секрет").unwrap_err();
    assert!(e.contains("ключ") && e.contains(&dir.display().to_string()), "{e}");
}

// DPAPI на сборочном агенте не зовём (спека: «в тестах нельзя звать DPAPI»):
// тест остаётся для ручного прогона на машине человека.
#[cfg(windows)]
#[ignore = "зовёт DPAPI пользователя — гонять руками, не в CI"]
#[test]
fn the_system_store_on_windows_round_trips_through_dpapi() {
    let s = Store::system(&tmp("dpapi")).unwrap();
    let sealed = s.seal("пароль 1С").unwrap();
    assert!(!sealed.contains("пароль"));
    assert_eq!(s.open(&sealed).unwrap(), "пароль 1С");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect secrets`
Expected: FAIL, модуля `secrets` нет.

- [ ] **Step 3: Implement `secrets.rs`**

Windows: `CryptProtectData`/`CryptUnprotectData` (`windows-sys`, фича `Win32_Security_Cryptography`), область — пользователь; любой отказ расшифровки — `OpenError::Foreign`, потому что снаружи они неотличимы и обвинять человека в «битом файле» нельзя. Linux и тесты: `aes-gcm` 0.11 с ключом из `<dir>/secret.key`, права `0600`, случайный `nonce` на каждое шифрование и он же в начале шифртекста; источник случайности — тот, что назвал замер задачи 0. Кодирование одно на обе подложки, hex или base64 — на выбор исполнителя.

`ponytail:` — на файле ключа: он копируется вместе с конфигом, и на Linux защита слабее, чем на Windows; путь наверх — secret service, когда появится машина, где он есть по умолчанию.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect secrets`
Expected: PASS (кроме `#[ignore]`-теста DPAPI — его прогнать руками и сказать результат в отчёте)

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/secrets.rs setup/1c-connect/src/lib.rs setup/1c-connect/Cargo.toml
git commit -m "Модуль секретов: enc1 и две подложки — DPAPI на Windows, файл ключа на Linux"
```

---

### Task 2: Параметры читаются и пишутся через секреты

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs` (`load` — `cfg.rs:290`, `load_unchecked` — `cfg.rs:316`, `render` — `cfg.rs:466`)
- Modify (вызовы `render`): `setup/1c-connect/src/gui/mod.rs:1565`, `setup/1c-connect/src/gui/save.rs:126`, `setup/1c-connect/src/gui/save.rs:154`, `setup/1c-connect/src/ops_host.rs:2737`, плюс тесты `cfg.rs:822` и `cfg.rs:913`
- Modify: `setup/common/src/kit.rs` (`write_text` — `kit.rs:106-111`) либо вызывающий код — см. шаг 3 про проверку после записи
- Test: `setup/1c-connect/src/cfg.rs`, модуль `tests`

**Interfaces:**
- Consumes: `secrets::Store`, `secrets::OpenError` (задача 1).
- Produces: `pub fn render(&self, store: &secrets::Store) -> Result<String, String>` — подпись МЕНЯЕТСЯ, аргумент добавляется, все четыре вызова правятся в этой же задаче; `pub fn load(kit: &Kit) -> Result<Cfg>` отказывает, когда хоть одно значение не расшифровалось; `pub fn load_unchecked(kit: &Kit) -> Result<Cfg>` отдаёт пустое поле и список; поле `pub undecryptable: Vec<String>` в `Cfg` с `#[serde(skip)]` — имена полей, которые не открылись (`"password"`, `"db_password"`).
- Отдельной структуры-ошибки `Undecryptable` нет: список в `Cfg` достаточен, и его читает окно (задача 4).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_plaintext_file_still_loads() {
    let kit = kit_with("[host]\npassword = \"открытый\"\ndb_password = \"тоже открытый\"\n");
    let c = Cfg::load_unchecked(&kit).unwrap();
    assert_eq!(c.host.password, "открытый");
    assert_eq!(c.host.db_password, "тоже открытый");
    assert!(c.undecryptable.is_empty());
}

#[test]
fn render_always_writes_ciphertext() {
    let s = Store::file(&tmp("render"));
    let mut c = filled();
    c.host.password = "секрет".into();
    c.host.db_password = "второй".into();
    let text = c.render(&s).unwrap();
    assert!(!text.contains("секрет") && !text.contains("второй"), "{text}");
    assert_eq!(text.matches(secrets::PREFIX).count(), 2);
}

#[test]
fn an_empty_password_stays_empty_in_the_file() {
    // Образец (1c-connect.example.toml) несёт оба пароля пустыми, и `render`
    // обязан остаться детерминированным — иначе падают cfg.rs:822 и cfg.rs:913.
    let s = Store::file(&tmp("renderempty"));
    let c = Cfg::default();
    let text = c.render(&s).unwrap();
    assert!(!text.contains(secrets::PREFIX), "{text}");
    assert_eq!(text, c.render(&s).unwrap(), "render обязан быть детерминированным");
}

#[test]
fn a_half_migrated_file_reads_both_halves() {
    let s = Store::file(&tmp("half"));
    let sealed = s.seal("зашифрованный").unwrap();
    let kit = kit_with(&format!("[host]\npassword = \"{sealed}\"\ndb_password = \"открытый\"\n"));
    let c = Cfg::load_unchecked_with(&kit, &s).unwrap();
    assert_eq!(c.host.password, "зашифрованный");
    assert_eq!(c.host.db_password, "открытый");
}

#[test]
fn the_window_load_reports_the_field_instead_of_refusing() {
    let (mine, theirs) = (Store::file(&tmp("w1")), Store::file(&tmp("w2")));
    let kit = kit_with(&format!("[host]\npassword = \"{}\"\n", mine.seal("секрет").unwrap()));
    let c = Cfg::load_unchecked_with(&kit, &theirs).expect("окно не отказывает");
    assert_eq!(c.host.password, "", "поле обязано быть пустым, а не мусором");
    assert_eq!(c.undecryptable, vec!["password".to_string()]);
}

#[test]
fn a_cli_load_refuses_instead_of_running_with_an_empty_password() {
    // `validate` пароли НЕ проверяет (cfg.rs:350-380), поэтому без этого
    // отказа `serve`/`connect`/`mcp` поехали бы с пустым паролем молча.
    let (mine, theirs) = (Store::file(&tmp("c1")), Store::file(&tmp("c2")));
    let kit = kit_with(&format!("[host]\npassword = \"{}\"\n", mine.seal("секрет").unwrap()));
    let e = Cfg::load_with(&kit, &theirs).unwrap_err().to_string();
    assert!(e.contains("другой машине") || e.contains("другим пользователем"), "{e}");
    assert!(e.contains("введите"), "{e}");
}

#[test]
fn reading_does_not_rewrite_the_file() {
    let kit = kit_with("[host]\npassword = \"открытый\"\n");
    let before = std::fs::read_to_string(kit.state_path(cfg::FILE)).unwrap();
    let _ = Cfg::load_unchecked(&kit).unwrap();
    assert_eq!(std::fs::read_to_string(kit.state_path(cfg::FILE)).unwrap(), before);
}

#[test]
fn a_write_that_did_not_seal_is_caught_on_read_back() {
    // Анти-цель спеки: «шифрование, которое тихо не состоялось». Запись
    // обязана прочитать файл обратно и убедиться, что значения открываются.
    let s = Store::file(&tmp("readback"));
    let kit = kit_with("");
    let mut c = filled();
    c.host.password = "секрет".into();
    cfg::write_checked(&kit, &c, &s).unwrap();
    let back = Cfg::load_unchecked_with(&kit, &s).unwrap();
    assert_eq!(back.host.password, "секрет");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect cfg::tests`
Expected: FAIL — `render` без аргумента, `undecryptable` и `write_checked` не существуют.

- [ ] **Step 3: Implement**

`load_with`/`load_unchecked_with` принимают подложку (ими и проверяется), обычные `load`/`load_unchecked` строят её через `Store::system(kit.state())`; невозможность построить — отказ со словами, не паника. Нерасшифрованное поле становится пустым, его имя уходит в `undecryptable`; `load` при непустом списке отказывает словами («пароль зашифрован для другого пользователя или другой машины — откройте окно и введите заново»), `load_unchecked` отдаёт как есть: окно обязано дать человеку ввести пароль, а CLI — не ехать молча.

`cfg::write_checked(kit, cfg, store)` — единственный путь записи: печатает `render`, пишет (`kit::write_text`, сегодня это голый `fs::write` — временного файла и `.bak` в проекте нет, правил про них не заводим), затем читает файл обратно и проверяет, что оба значения открываются в те же строки. Не сошлось — отказ, и пусть лучше человек увидит отказ, чем файл, который завтра не откроется.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect cfg`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/cfg.rs setup/1c-connect/src/gui/ setup/1c-connect/src/ops_host.rs setup/common/src/kit.rs
git commit -m "Параметры: пароли читаются и из открытого текста, а пишутся только шифртекстом"
```

---

### Task 3: Пароль не печатается ни в журнал, ни в отладку

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs` (снять `derive(Debug)` у `Cfg` — `cfg.rs:176` — и у `Host` — `cfg.rs:182`, написать свой)
- Test: там же

**Interfaces:**
- Produces: `impl Debug for Host` и `impl Debug for Cfg`, печатающие на месте паролей `"<скрыт>"`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn debug_output_never_carries_a_password() {
    let mut c = filled();
    c.host.password = "секрет-ssh".into();
    c.host.db_password = "секрет-базы".into();
    let text = format!("{c:?}");
    assert!(!text.contains("секрет-ssh") && !text.contains("секрет-базы"), "{text}");
    assert_eq!(text.matches("<скрыт>").count(), 2);
    assert!(text.contains(&c.host.workstation), "отладка не должна ослепнуть");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p onec-connect debug_output_never`
Expected: FAIL — производный `Debug` печатает пароли полями.

- [ ] **Step 3: Implement**

Свой `Debug` для `Host` перечисляет поля как производный, кроме двух паролей. Проверить, что `Cfg`/`Host` не лежат внутри чужих `#[derive(Debug)]` и не печатаются через `{:?}` где-то ещё (`ops_host::Ctx`, `gui::Ui`) — иначе сборка встанет на чужом файле, а смысл работы потеряется.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p onec-connect debug_output_never`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/cfg.rs
git commit -m "Debug параметров прячет пароли: журнал прикладывают к обращению"
```

---

### Task 4: Окно говорит, когда пароль не расшифровался

**Files:**
- Modify: `setup/1c-connect/src/gui/panel.rs` (чистая часть), `setup/1c-connect/src/gui/mod.rs` (показ при загрузке)
- Test: `setup/1c-connect/src/gui/tests.rs`

**Interfaces:**
- Consumes: `Cfg::undecryptable` (задача 2).
- Produces: `panel::undecryptable_line(fields: &[String]) -> Option<Line>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_undecryptable_password_is_explained_in_words() {
    let l = panel::undecryptable_line(&["password".to_string()]).expect("строка есть");
    assert_eq!(l.kind, Kind::Warn);
    assert!(l.text.contains("другой машине") || l.text.contains("другим пользователем"), "{}", l.text);
    assert!(l.text.contains("введите"), "{}", l.text);
    assert!(l.text.contains("рабочего места"), "поле названо словами человека: {}", l.text);
}

#[test]
fn both_fields_are_named_in_one_line() {
    let l = panel::undecryptable_line(&["password".into(), "db_password".into()]).unwrap();
    assert!(l.text.contains("рабочего места") && l.text.contains("базы"), "{}", l.text);
}

#[test]
fn an_empty_list_says_nothing_and_a_full_one_says_something() {
    // Пара: пустой список молчит, непустой говорит — иначе тест прошёл бы и
    // у функции, которая всегда молчит.
    assert!(panel::undecryptable_line(&[]).is_none());
    assert!(panel::undecryptable_line(&["db_password".into()]).is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect --features gui undecryptable`
Expected: FAIL, `undecryptable_line` не существует.

- [ ] **Step 3: Implement**

Строка идёт в журнал окна при загрузке параметров, поля паролей при этом пустые. Имена полей переводятся в слова: `password` — «пароль рабочего места», `db_password` — «пароль пользователя базы». Окно перечитывает файл при каждом показе (`window.rs:261`, `reread_and_recheck`) — значит строка обязана появляться один раз на загрузку, а не сыпаться в журнал на каждое открытие окна; как именно, решает исполнитель, но в отчёте скажи, чем это закрыл.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect --features gui undecryptable`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/gui/
git commit -m "Нерасшифрованный пароль: пустое поле и причина словами, а не отказ связи"
```

---

### Task 5: Пароль базы уходит из командных строк

**Files:**
- Modify: `setup/1c-connect/src/bridge.rs` (`mcp_command` — `bridge.rs:24-27`), `setup/1c-connect/src/ops_host.rs` (`--db-password` у установки расширения — `ops_host.rs:1636`)
- Test: там же, модули `tests`

**Interfaces:**
- Produces: `mcp_command` без `--password`; пароль идёт переменной `MCP_1C_PASSWORD`. Установка расширения — тем же способом.

Что остаётся и почему — записать комментариями, а не умолчанием: `--base`/`--user`/`--dump`/`--quiet` остаются ключами (окружение их всё равно перебивает, и меньше правок); `db-pwd` в аргументах инструмента (`bridge.rs:237`) остаётся, потому что уходит в трубу процесса, а не в командную строку; `/P` Конфигуратору (`ops_host.rs:1675`) остаётся, потому что у платформы другого способа нет — это названо в спеке как единственное исключение.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_db_password_never_appears_in_the_command_line() {
    let mut cfg = filled();
    cfg.host.db_password = "секрет-базы".into();
    let c = mcp_command(&cfg, &kit_for_test());
    let args: Vec<String> = c.as_std().get_args().map(|a| a.to_string_lossy().into()).collect();
    assert!(!args.iter().any(|a| a.contains("секрет-базы")), "{args:?}");
    assert!(!args.iter().any(|a| a == "--password"), "{args:?}");
    let envs: Vec<(String, String)> = c.as_std().get_envs()
        .filter_map(|(k, v)| Some((k.to_string_lossy().into(), v?.to_string_lossy().into())))
        .collect();
    assert!(envs.contains(&("MCP_1C_PASSWORD".into(), "секрет-базы".into())), "{envs:?}");
}

#[test]
fn the_install_command_hides_the_password_too() {
    let mut cfg = filled();
    cfg.host.db_password = "секрет-базы".into();
    let c = ops_host::install_command(&cfg, &kit_for_test());
    let args: Vec<String> = c.as_std().get_args().map(|a| a.to_string_lossy().into()).collect();
    assert!(!args.iter().any(|a| a.contains("секрет-базы")), "{args:?}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect -- the_db_password_never the_install_command_hides`
Expected: FAIL — сегодня пароль идёт ключом в обоих местах.

- [ ] **Step 3: Implement**

Переменные вместо ключей в обоих местах. Комментарий называет причину (командная строка процесса на Windows видна любому процессу пользователя) и факт, на котором это стоит: `mcp-1c` читает окружение и оно перебивает ключи (`internal/config/config.go`), подтверждено замером задачи 0.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect -- bridge ops_host`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/bridge.rs setup/1c-connect/src/ops_host.rs
git commit -m "Пароль базы уходит окружением: командная строка видна всем"
```

---

### Task 6: Учётные данные уходят из дескриптора публикации

**Files:**
- Modify: `setup/1c-connect/src/publication.rs` (`vrd` — `publication.rs:65-77`, и комментарии `:26`, `:62`, `:84`, `:124`, `:232`, `:964`)
- Modify: `setup/1c-connect/src/verify.rs` (`descriptor_fact` — `verify.rs:758-771`)
- Modify: `setup/1c-connect/src/gui/save.rs` (`database_changed` — `save.rs:102-104`)
- Modify: `config/1c/stand.sh` (`IB_USR` — `stand.sh:118`, `:667`)
- Test: `publication.rs` (переписать четыре существующих теста), `gui/save/tests.rs:297,310`

Это не косметика, а закрытие открытой двери (спека, замер 29.10): с нынешним дескриптором вызов сервиса БЕЗ учётных данных отдаёт `200`, без атрибутов — `401`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_descriptor_carries_no_credentials() {
    let text = vrd(&cfg_for_test());
    assert!(!text.contains("Usr=") && !text.contains("Pwd="), "{text}");
    assert!(!text.contains(&cfg_for_test().host.db_password), "{text}");
}

#[test]
fn the_connection_string_itself_is_unchanged() {
    let mut c = cfg_for_test();
    c.host.base_path = "/home/dev1/base".into();
    assert!(vrd(&c).contains("File=&quot;/home/dev1/base&quot;;"));
    c.host.base_path = r"srv\base".into();
    let t = vrd(&c);
    assert!(t.contains("Srvr=&quot;srv&quot;;") && t.contains("Ref=&quot;base&quot;;"), "{t}");
}

#[test]
fn descriptor_matches_detects_a_changed_base_path() {
    // Прежний тест ловил смену ПАРОЛЯ; пароля в дескрипторе больше нет, и
    // сторожить теперь надо то, что в нём осталось.
    let dir = tmp("vrd");
    let mut c = cfg_for_test();
    write_vrd(&dir, &vrd(&c)).unwrap();
    assert!(descriptor_matches(&dir, &c));
    c.host.base_path = "/home/dev1/other".into();
    assert!(!descriptor_matches(&dir, &c));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect publication`
Expected: FAIL — дескриптор несёт `Usr`/`Pwd`; заодно падают `vrd_carries_the_mandatory_attributes` (`publication.rs:743`), `vrd_escapes_xml_special_characters_in_every_substituted_value` (`:767`), `descriptor_matches_detects_a_changed_password` (`:782`), `vrd_never_leaks_the_password_into_the_journal_by_itself` (`:815`) — это ожидаемо, их правит шаг 3.

- [ ] **Step 3: Implement формат и переписать задетые тесты**

Убрать из `vrd` оба атрибута; обязательные атрибуты публикации (`publishExtensionsByDefault`, `reuseSessions="dontuse"`, `poolSize="1"`, `sessionMaxAge="1"`) не трогать — без них учебная лицензия отдаёт агенту 500 на втором вызове. Переписать: `vrd_carries_the_mandatory_attributes` (убрать ожидание `Pwd`), `vrd_escapes_xml_special_characters_in_every_substituted_value` (перенести на `File=`/`Srvr=`/`Ref=` — экранирование по-прежнему нужно пути базы), `descriptor_matches_detects_a_changed_password` (заменён тестом выше), `vrd_never_leaks_the_password_into_the_journal_by_itself` (проверить, на чём он стоит, и сохранить смысл). Комментарии `publication.rs:26`, `:62`, `:84`, `:124`, `:232`, `:964` утверждают, что дескриптор несёт логин и пароль, — переписать. У `write_vrd` права `0600` остаются, но причина меняется: не пароль, а путь базы и состав публикации — чужое дело; скажи это, иначе следующий читатель их снимет.

- [ ] **Step 4: Слова факта готовности**

`descriptor_fact` (`verify.rs:758-771`) говорит про пароль буквально: «совпадает с текущим паролем базы», «разошёлся с текущим паролем базы», «`install` перепишет текущим паролем». Пароля там нет — перепиши слова под то, что проверка делает на самом деле («совпадает с нашим» / «разошёлся с нашим»), логику `descriptor_matches` не трогай: она сравнивает текст целиком и остаётся полезной.

- [ ] **Step 5: Разделить два разных вопроса в `database_changed`**

`database_changed` (`save.rs:102-104`) сегодня истинна при смене пути базы, пользователя ИЛИ пароля, и у неё ДВА потребителя: переподъём публикации (`save.rs:161`) и вопрос человеку «агент работает с базой X, перейдёт на Y» (`save.rs:251` → `save_question`). Заведи `descriptor_changed` (только `base_path`) для публикации и оставь `database_changed` как есть для вопроса и перезапуска сеанса: сеанс с новым паролем нужен так же, как с новым путём, а дескриптор — нет. Тесты `gui/save/tests.rs:297` и `:310` задеты, их смысл сохранить.

- [ ] **Step 6: Стенд**

`config/1c/stand.sh:118,667` пишет свой `default.vrd` с `IB_USR=Usr=$DB_USER;Pwd=;` — то есть анонимный вход там остаётся, и живая проверка задачи 7 на стенде даст `200`, что выглядит как провал правки. Привести к тому же виду (без `Usr`/`Pwd`) либо явно записать в отчёте, что стенд остаётся с анонимным входом и почему.

- [ ] **Step 7: Проверить, что наш же опрос публикации выжил**

`publication.rs:119-121` зовёт `/version` без учётных данных и смотрит только `send().is_ok()` — для `reqwest` `401` это успешный ответ, то есть проба переживёт правку без изменений. Убедись в этом тестом с локальным `TcpListener`, отдающим `401`, либо назови это ручным замером в отчёте. И проверь по замеру задачи 0, не покраснел ли факт `EXTENSION`.

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p onec-connect -- publication vrd descriptor database_changed`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add setup/1c-connect/src/publication.rs setup/1c-connect/src/verify.rs setup/1c-connect/src/gui/save.rs config/1c/stand.sh
git commit -m "Дескриптор публикации без логина и пароля: вход только по учётным данным запроса"
```

---

### Task 7: Проверка на живой машине

- [ ] **Step 1: Путь человека целиком**

Установка с открытыми паролями: до сохранения всё работает, после — в файле два `enc1:` и ни одного пароля. Копия файла параметров на другую машину (и на Windows — открытие под другим пользователем): окно показывает пустое поле и причину, `1c-connect serve` отказывается ехать со словами, после ввода заново работает и то, и другое.

- [ ] **Step 2: Публикация**

После задачи 6: вызов сервиса без учётных данных — `401`, с верными — работает, веб-клиент в корне открывается и спрашивает логин сам. Факт «расширение в базе» зелёный.

- [ ] **Step 3: Следы**

`tasklist` во время работы агента: ни одного процесса с паролем в командной строке. Поиск пароля по журналу за полный цикл — ни одной строки. `findstr /s` по каталогу состояния и каталогу публикации — пароль находится только там, где мы его сами оставили, и это совпадает с тем, что обещано.

- [ ] **Step 4: Отчёт в спеку**

Записать результаты строкой «замер», включая тот `#[ignore]`-тест DPAPI, который гоняется руками.

---

### Task 8: Слова для человека

- [ ] **Step 1: Написать**

В `config/kit/УСТАНОВКА.md` — что человек увидит при обновлении (ничего), что увидит при переносе на другую машину (пустое поле, причина, отказ CLI) и что делать. В `README.md` — раздел-вход. Честные границы, обе: пароля не остаётся ни в файле параметров, ни в дескрипторе публикации, но он мгновение живёт в командной строке Конфигуратора при снятии расширения; и защиты от кода, запущенного под тем же пользователем, у нас нет.

- [ ] **Step 2: Commit**

```bash
git add README.md config/kit/УСТАНОВКА.md
git commit -m "Шифрование паролей: слова для человека и честные границы"
```

---

## Чем доказывается, что сделано

`cargo test -p onec-connect --features gui` зелёный; `cargo fmt --check` и `cargo clippy --workspace -- -D warnings` чисты.

В файле параметров после сохранения нет ни одного пароля, и он читается обратно — проверено на живой установке, не только тестом.

Файл параметров на другой машине не открывается: окно показывает пустое поле и причину, CLI отказывается ехать.

`tasklist` во время работы агента не показывает пароля ни в одной командной строке; поиск по журналу за полный цикл его не находит.

Дескриптор публикации не несёт ни логина, ни пароля, и вызов сервиса без учётных данных получает `401` вместо сеанса с полными правами.

Установка с открытыми паролями продолжает работать до первого сохранения — ни один существующий стенд не встал от обновления.
