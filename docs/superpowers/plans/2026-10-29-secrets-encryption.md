# Шифрование паролей в файле параметров — план работ

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** В `1c-connect.toml` не остаётся ни одного пароля открытым текстом, и файл, попавший в бэкап или на другую машину, не даёт доступа к базе.

**Architecture:** Новый модуль `secrets.rs` держит формат (`enc1:` плюс шифртекст) и две подложки: DPAPI пользователя на Windows (ключа на диске нет, шифртекст не открывается ни на другой машине, ни другим пользователем) и файл ключа `0600` с AEAD на Linux. `Cfg::load` принимает и шифртекст, и открытый текст; `Cfg::render` всегда печатает шифртекст, поэтому первое же «Сохранить» переводит существующую установку. Пароль базы заодно перестаёт идти ребёнку командной строкой — `mcp-1c` уже читает `MCP_1C_PASSWORD` из окружения.

**Tech Stack:** Rust, GPUI, пакет `onec-connect`; `aes-gcm` (уже в `Cargo.lock` через `russh`), `windows-sys` с фичей `Win32_Security_Cryptography`; тесты — `cargo test -p onec-connect --features gui`.

**Spec:** `docs/superpowers/specs/2026-10-29-secrets-encryption-design.md`

## Global Constraints

Шифруются два значения, оба в `1c-connect.toml`: `[host] password` (`cfg.rs:195`) и `[host] db_password` (`cfg.rs:205`). Ключ сервера модели в `dev1c.toml` не трогаем — решение владельца 29.10, у него вторая копия в контейнере.

Значение без префикса читается как открытый текст **всегда**, а не только в переходный период: это и путь обновления, и законный выход для закрытого контура.

Перешифровка идёт при записи, не при чтении: разовая команда (`status`, `check`) не имеет права переписать файл параметров человека.

Ни одной новой библиотеки в сборке: `aes-gcm` уже в `Cargo.lock`, у `windows-sys` добавляется одна фича.

Коннектор службой под `SYSTEM` не планируется (решение владельца 29.10) — DPAPI пользователя выбран с этим знанием.

Обещание работы: **пароля не остаётся ни в файле параметров, ни в дескрипторе публикации** — из второго он убирается целиком (задача 6, замер 29.10 в спеке). На диске он остаётся только там, где его держит сама система: в хранилище секретов пользователя (Windows) либо в файле ключа `0600` (Linux).

Защиты от кода, запущенного под тем же пользователем, эта работа не даёт и обещать не должна.

Проза и комментарии по-русски, абзац одной строкой, объясняют «почему»; осознанные срезы помечаются `ponytail:` с названием потолка. Тесты гонять в переднем плане, не в фоне.

## Review Focus

Пароль с кавычками, обратными косыми, кириллицей и пробелами (`па"роль`, `C:\x`, `пароль 1С`) — шифрование и обратный разбор обязаны вернуть ровно ту же строку: задача 1, тест `a_secret_with_quotes_and_cyrillic_survives_the_round_trip`.

Полупереведённый файл: один пароль уже в `enc1:`, второй ещё открытым — обе половины читаются правильно, и следующее «Сохранить» переводит второй: задача 2, тест `a_half_migrated_file_reads_both_halves`.

Шифртекст, записанный другим пользователем или на другой машине: поле пустое, причина названа словами, работа не падает — задача 2, тест `an_undecryptable_value_is_an_empty_field_with_a_named_reason`, и задача 4 про окно.

Файл параметров, пойманный подкомандой `mcp` в середине записи: такт опроса не падает и не считает это новыми параметрами — задача 2, тест `a_half_written_file_keeps_the_previous_cfg`.

Каталог состояния только для чтения (комплект на диске без записи) — на Linux ключ создать нельзя, и это отказ со словами, а не паника: задача 1, тест `a_read_only_state_dir_is_a_named_refusal`.

---

### Task 1: Модуль секретов — формат и две подложки

**Files:**
- Create: `setup/1c-connect/src/secrets.rs`
- Modify: `setup/1c-connect/src/lib.rs` (объявить модуль), `setup/1c-connect/Cargo.toml` (фича `Win32_Security_Cryptography` у `windows-sys`, прямая зависимость `aes-gcm`)
- Test: `setup/1c-connect/src/secrets.rs`, модуль `tests`

**Interfaces:**
- Produces: `secrets::PREFIX` (`"enc1:"`); `secrets::Store` с тремя способами постройки — `Store::system(state_dir: &Path) -> Result<Store, String>` (на Windows DPAPI, и каталог не используется; на Linux файл ключа в этом каталоге — он же `Kit::state`), `Store::file(dir: &Path)` (файл ключа, нужен обеим платформам для тестов), `Store::plain()` (пустышка для тестов, ничего не шифрует); `Store::seal(&self, plain: &str) -> Result<String, String>` (отдаёт значение с префиксом); `Store::open(&self, value: &str) -> Result<String, OpenError>`, где `OpenError::Foreign` — «шифртекст не наш» (другой пользователь, другая машина, битый ключ), `OpenError::Broken(String)` — всё прочее; значение без префикса `open` возвращает как есть.
- Consumes: ничего из других задач.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_value_without_the_prefix_comes_back_untouched() {
    let s = Store::plain();
    assert_eq!(s.open("открытый пароль").unwrap(), "открытый пароль");
    assert_eq!(s.open("").unwrap(), "");
}

#[test]
fn a_sealed_value_carries_the_prefix_and_not_the_secret() {
    let s = Store::file(tempdir().path());
    let sealed = s.seal("пароль 1С").unwrap();
    assert!(sealed.starts_with(PREFIX), "{sealed}");
    assert!(!sealed.contains("пароль"), "секрет виден в шифртексте: {sealed}");
    assert_eq!(s.open(&sealed).unwrap(), "пароль 1С");
}

#[test]
fn a_secret_with_quotes_and_cyrillic_survives_the_round_trip() {
    let s = Store::file(tempdir().path());
    for plain in [r#"па"роль"#, r"C:\x\y", "пароль 1С", "p w", "", "ПРОБЕЛ в конце "] {
        let sealed = s.seal(plain).unwrap();
        assert_eq!(s.open(&sealed).unwrap(), plain, "не сошлось на {plain:?}");
    }
}

#[test]
fn a_foreign_ciphertext_is_named_foreign_not_broken() {
    let mine = Store::file(tempdir().path());
    let theirs = Store::file(tempdir().path());
    let sealed = mine.seal("секрет").unwrap();
    assert!(matches!(theirs.open(&sealed), Err(OpenError::Foreign)));
}

#[test]
fn a_read_only_state_dir_is_a_named_refusal() {
    // Комплект может лежать на диске только для чтения: ключ создать негде,
    // и это отказ словами, а не паника.
    let dir = read_only_tempdir();
    let e = Store::file(dir.path()).seal("секрет").unwrap_err();
    assert!(e.contains("ключ") && e.contains(&dir.path().display().to_string()), "{e}");
}

#[cfg(windows)]
#[test]
fn the_system_store_on_windows_round_trips_through_dpapi() {
    let s = Store::system().unwrap();
    let sealed = s.seal("пароль 1С").unwrap();
    assert!(!sealed.contains("пароль"));
    assert_eq!(s.open(&sealed).unwrap(), "пароль 1С");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect secrets`
Expected: FAIL, модуля `secrets` нет.

- [ ] **Step 3: Implement `secrets.rs`**

Windows: `CryptProtectData`/`CryptUnprotectData` (`windows-sys`, фича `Win32_Security_Cryptography`), область — пользователь, описание не передаём; `ERROR_INVALID_DATA` и прочие отказы расшифровки — это `OpenError::Foreign`, потому что снаружи они неотличимы и обвинять человека в «битом файле» нельзя. Linux и тесты: `aes-gcm` с ключом из файла `<dir>/secret.key`, права `0600`, случайный `nonce` на каждое шифрование и он же в начале шифртекста; ключ рождается при первой записи, и его отсутствие при чтении — тоже `Foreign`. Кодирование — base64 или hex, на выбор исполнителя, но одно на оба случая.

`ponytail:` — на файле ключа: он копируется вместе с конфигом, и на Linux защита слабее, чем на Windows; путь наверх — secret service, когда появится машина, где он есть по умолчанию.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect secrets`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/secrets.rs setup/1c-connect/src/lib.rs setup/1c-connect/Cargo.toml
git commit -m "Модуль секретов: enc1 и две подложки — DPAPI на Windows, файл ключа на Linux"
```

---

### Task 2: Параметры читаются и пишутся через секреты

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs` (`load` — `cfg.rs:290`, `load_unchecked` — `cfg.rs:316`, `render` — `cfg.rs:466`)
- Test: там же, модуль `tests`

**Interfaces:**
- Consumes: `secrets::Store`, `secrets::OpenError` (задача 1).
- Produces: `Cfg::load`/`load_unchecked` расшифровывают оба пароля через `Store::system()`; `Cfg::render` печатает их через `Store::seal`; `Cfg::load_with(kit, &Store)` и `Cfg::render_with(&Store)` — те же пути с подставленной подложкой, ими и проверяется; `cfg::Undecryptable { field: &'static str }` в составе ошибки загрузки — чтобы окно (задача 4) могло отличить «не расшифровали» от «файл битый».

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_plaintext_file_still_loads() {
    let kit = kit_with(r#"[host]
password = "открытый"
db_password = "тоже открытый"
"#);
    let c = Cfg::load_with(&kit, &Store::plain()).unwrap();
    assert_eq!(c.host.password, "открытый");
    assert_eq!(c.host.db_password, "тоже открытый");
}

#[test]
fn render_always_writes_ciphertext() {
    let s = Store::file(tempdir().path());
    let mut c = filled();
    c.host.password = "секрет".into();
    c.host.db_password = "второй".into();
    let text = c.render_with(&s);
    assert!(!text.contains("секрет") && !text.contains("второй"), "{text}");
    assert_eq!(text.matches(secrets::PREFIX).count(), 2);
    // И прочитали обратно то же самое.
    let back = Cfg::load_with(&kit_with(&text), &s).unwrap();
    assert_eq!(back.host.password, "секрет");
    assert_eq!(back.host.db_password, "второй");
}

#[test]
fn a_half_migrated_file_reads_both_halves() {
    let s = Store::file(tempdir().path());
    let sealed = s.seal("зашифрованный").unwrap();
    let kit = kit_with(&format!("[host]\npassword = \"{sealed}\"\ndb_password = \"открытый\"\n"));
    let c = Cfg::load_with(&kit, &s).unwrap();
    assert_eq!(c.host.password, "зашифрованный");
    assert_eq!(c.host.db_password, "открытый");
}

#[test]
fn an_undecryptable_value_is_an_empty_field_with_a_named_reason() {
    let mine = Store::file(tempdir().path());
    let theirs = Store::file(tempdir().path());
    let sealed = mine.seal("секрет").unwrap();
    let kit = kit_with(&format!("[host]\npassword = \"{sealed}\"\n"));
    let c = Cfg::load_with(&kit, &theirs).expect("загрузка не падает");
    assert_eq!(c.host.password, "", "поле обязано быть пустым, а не мусором");
    assert_eq!(c.undecryptable, vec!["password"], "назвать, что именно не открылось");
}

#[test]
fn reading_does_not_rewrite_the_file() {
    let kit = kit_with("[host]\npassword = \"открытый\"\n");
    let before = std::fs::read_to_string(kit.state_path(cfg::FILE)).unwrap();
    let _ = Cfg::load_with(&kit, &Store::plain()).unwrap();
    assert_eq!(std::fs::read_to_string(kit.state_path(cfg::FILE)).unwrap(), before);
}

#[test]
fn a_half_written_file_keeps_the_previous_cfg() {
    // Подкоманда `mcp` перечитывает файл раз в 200 мс и может поймать его в
    // середине записи: это не новые параметры, а ещё не дописанный файл.
    let kit = kit_with("[host]\npassword = \"откры");
    assert!(Cfg::load_with(&kit, &Store::plain()).is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect cfg::tests`
Expected: FAIL, `load_with`/`render_with`/`undecryptable` не существуют.

- [ ] **Step 3: Implement**

`load`/`load_unchecked` зовут `load_with(kit, &Store::system(kit.state())?)`; `render` — `render_with(&Store::system(kit.state())?)`, то есть `render` получает подложку от вызывающего (у него есть `Kit`), а не ищет её сам. Невозможность построить подложку — отказ со словами, а не паника. Про временные файлы и резервные копии отдельного правила не нужно: `render` отдаёт шифртекст, поэтому всё, что из него пишется — и временный файл, и `.bak`, если он когда-нибудь появится, — несёт шифртекст по построению; скажи это комментарием, чтобы следующий не заводил правило заново. Нерасшифрованное поле становится пустым, его имя уходит в `Cfg::undecryptable` (поле не из toml: `#[serde(skip)]`), и `validate` на пустом пароле ругается ровно как раньше — отдельного правила не заводим.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect cfg`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/cfg.rs
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
    // Остальные поля по-прежнему видны: отладка не должна ослепнуть.
    assert!(text.contains(&c.host.workstation));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p onec-connect debug_output_never`
Expected: FAIL — производный `Debug` печатает пароли полями.

- [ ] **Step 3: Implement**

Свой `Debug` для `Host` перечисляет поля как производный, кроме двух паролей — на их месте `"<скрыт>"`. `Debug` для `Cfg` — через него. Комментарий объясняет, почему производный здесь нельзя: любая отладочная строка в журнал стала бы утечкой, а журнал человек прикладывает к обращению.

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
- Modify: `setup/1c-connect/src/gui/save.rs` или `gui/panel.rs` (чистая часть), `setup/1c-connect/src/gui/mod.rs` (показ при загрузке)
- Test: `setup/1c-connect/src/gui/tests.rs`

**Interfaces:**
- Consumes: `Cfg::undecryptable` (задача 2).
- Produces: `panel::undecryptable_line(fields: &[String]) -> Option<Line>` — строка для человека, `None` когда всё открылось.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_undecryptable_password_is_explained_in_words() {
    let l = panel::undecryptable_line(&["password".to_string()]).expect("строка есть");
    assert_eq!(l.kind, Kind::Warn);
    assert!(l.text.contains("другой машине") || l.text.contains("другим пользователем"), "{}", l.text);
    assert!(l.text.contains("введите"), "{}", l.text);
}

#[test]
fn both_fields_are_named_in_one_line() {
    let l = panel::undecryptable_line(&["password".into(), "db_password".into()]).unwrap();
    assert!(l.text.contains("рабочего места") && l.text.contains("базы"), "{}", l.text);
}

#[test]
fn nothing_is_said_when_everything_opened() {
    assert!(panel::undecryptable_line(&[]).is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect --features gui undecryptable`
Expected: FAIL, `undecryptable_line` не существует.

- [ ] **Step 3: Implement**

Строка идёт в журнал окна при загрузке параметров, поля паролей при этом пустые (их такими отдала задача 2) — то есть человек видит пустое поле и рядом причину. Имена полей переводятся в слова человека: `password` — «пароль рабочего места», `db_password` — «пароль пользователя базы».

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p onec-connect --features gui undecryptable`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/gui/
git commit -m "Нерасшифрованный пароль: пустое поле и причина словами, а не отказ связи"
```

---

### Task 5: Пароль базы уходит ребёнку окружением, а не командной строкой

**Files:**
- Modify: `setup/1c-connect/src/bridge.rs` (`mcp_command` — `bridge.rs:24-27`)
- Test: там же, модуль `tests`

**Interfaces:**
- Produces: `mcp_command` без аргумента `--password`; пароль, пользователь и адрес базы идут переменными `MCP_1C_PASSWORD`, `MCP_1C_USER`, `MCP_1C_BASE_URL`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_db_password_never_appears_in_the_command_line() {
    let mut cfg = filled();
    cfg.host.db_password = "секрет-базы".into();
    let c = mcp_command(&cfg, &kit_for_test());
    let args: Vec<String> = c.as_std().get_args().map(|a| a.to_string_lossy().into()).collect();
    assert!(!args.iter().any(|a| a.contains("секрет-базы")), "{args:?}");
    assert!(!args.iter().any(|a| a == "--password"), "{args:?}");
    let envs: Vec<(String, String)> = c
        .as_std()
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_string_lossy().into(), v?.to_string_lossy().into())))
        .collect();
    assert!(envs.contains(&("MCP_1C_PASSWORD".into(), "секрет-базы".into())), "{envs:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p onec-connect the_db_password_never`
Expected: FAIL — сегодня пароль идёт `--password`.

- [ ] **Step 3: Implement**

Переменные вместо ключей. Комментарий называет причину (командная строка процесса на Windows видна любому процессу пользователя) и факт, на котором это стоит: `mcp-1c` читает окружение, и оно перебивает ключи (`internal/config/config.go` в форке; в комплекте едет `v1.23.0`, где это тем более ожидается). Остальные ключи (`--dump`, `--quiet`) не трогаем.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p onec-connect bridge`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add setup/1c-connect/src/bridge.rs
git commit -m "Пароль базы уходит mcp-1c окружением: командная строка видна всем"
```

---

### Task 6: Учётные данные уходят из дескриптора публикации

**Files:**
- Modify: `setup/1c-connect/src/publication.rs` (`vrd` — `publication.rs:65-77`)
- Test: там же, модуль `tests`

**Interfaces:**
- Produces: `vrd(cfg)` без `Usr`/`Pwd` в атрибуте `ib` — только `File="…";` либо `Srvr="…";Ref="…";`.
- Consumes: ничего из других задач; порядок в плане свободный, но делать после задачи 2 удобнее — тогда пароль уже нигде не печатается, кроме этого места.

Это не косметика, а закрытие открытой двери, и замер это показал (спека, «Пароля в дескрипторе публикации не должно быть вовсе»): с нынешним дескриптором вызов сервиса БЕЗ учётных данных отдаёт `200`, то есть любой, кто дотянулся до порта, получает сеанс с правами записанного пользователя. Без атрибутов — `401`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_descriptor_carries_no_credentials() {
    let text = vrd(&cfg_for_test());
    assert!(!text.contains("Usr="), "{text}");
    assert!(!text.contains("Pwd="), "{text}");
    // Пароль из параметров в дескриптор не попадает ни в каком виде.
    assert!(!text.contains(&cfg_for_test().host.db_password), "{text}");
}

#[test]
fn the_connection_string_itself_is_unchanged() {
    // Строку базы собирает `ib_connection`, и она не менялась: файловая база
    // остаётся файловой, серверная — серверной.
    let mut c = cfg_for_test();
    c.host.base_path = "/home/dev1/base".into();
    assert!(vrd(&c).contains("File=&quot;/home/dev1/base&quot;;"));
    c.host.base_path = r"srv\base".into();
    let t = vrd(&c);
    assert!(t.contains("Srvr=&quot;srv&quot;;") && t.contains("Ref=&quot;base&quot;;"), "{t}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p onec-connect publication`
Expected: FAIL — сегодня дескриптор несёт `Usr`/`Pwd`.

- [ ] **Step 3: Implement**

Убрать из формата `vrd` оба атрибута. Обязательные атрибуты публикации (`publishExtensionsByDefault`, `reuseSessions="dontuse"`, `poolSize="1"`, `sessionMaxAge="1"`) не трогать — без них учебная лицензия отдаёт агенту 500 на втором вызове, и это уже проверено тестом `vrd_carries_the_mandatory_attributes`. Комментарий над функцией объясняет, почему учётных данных там нет: платформа берёт пользователя из запроса, `mcp-1c` всегда ходит с Basic (`onec/client.go:76`), а с атрибутами любой, кто дотянулся до порта, входил без пароля.

- [ ] **Step 4: Проверить, что наш же опрос публикации выжил**

`publication.rs:119-121` спрашивает `/version` БЕЗ учётных данных и смотрит только `send().is_ok()`. После правки ответом будет `401` — убедись, что проба по-прежнему считает это «публикация отвечает» (для `reqwest` 401 — это успешный ответ, а не ошибка), и если нет, поправь её так, чтобы `401` читался как «сервис жив и спрашивает пароль». Тест на это обязателен.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p onec-connect -- publication vrd`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add setup/1c-connect/src/publication.rs
git commit -m "Дескриптор публикации без логина и пароля: вход только по учётным данным запроса"
```

---

### Task 7: Слова для человека и проверка на живой машине

**Files:**
- Modify: `config/kit/УСТАНОВКА.md`, `README.md`, `docs/superpowers/specs/2026-10-29-secrets-encryption-design.md` (отметить замеренное)
- Test: ручная проверка по списку ниже, отчёт — в коммит

- [ ] **Step 1: Замерить то, чего мы не знаем**

Поддерживает ли `mcp-1c` из комплекта (`v1.23.0`) переменную `MCP_1C_PASSWORD`: `mcp-1c-windows-amd64.exe --help` либо поиск строки в бинаре. Не поддерживает — задача 5 откатывается до обновления комплекта, и это записывается в спеку.

- [ ] **Step 2: Пройти путь человека целиком**

Публикация после задачи 6: вызов сервиса без учётных данных отдаёт `401`, с верными — работает, веб-клиент в корне по-прежнему открывается и спрашивает логин сам. Установка с открытыми паролями: до «Сохранить» всё работает, после — в файле два `enc1:` и ни одного пароля. Копия файла параметров на другую машину (и, на Windows, открытие под другим пользователем): поле пустое, причина названа, после ввода заново работает. `tasklist` во время работы агента: ни одного процесса с паролем в командной строке. Поиск пароля по журналу за полный цикл — ни одной строки. Стенд через `provision.sh`: без участия человека, после первого запуска в файлах шифртекст.

- [ ] **Step 3: Описать это словами**

В `УСТАНОВКА.md` — что человек увидит при обновлении (ничего), что увидит при переносе на другую машину (пустое поле и причина) и что делать. Честная граница: пароля не остаётся **в файле параметров**, но он остаётся в `default.vrd`, пока базу публикуем мы, и защищён там правами владельца. Про защиту от кода, запущенного под тем же пользователем, сказать прямо, что её нет.

- [ ] **Step 4: Commit**

```bash
git add README.md config/kit/УСТАНОВКА.md docs/superpowers/specs/2026-10-29-secrets-encryption-design.md
git commit -m "Шифрование паролей: слова для человека и замеры, которых ждала спека"
```

---

## Чем доказывается, что сделано

`cargo test -p onec-connect --features gui` зелёный; `cargo fmt --check` и `cargo clippy --workspace -- -D warnings` чисты.

В файле параметров после «Сохранить» нет ни одного пароля, и он читается обратно — проверено на живой установке, не только тестом.

Файл параметров на другой машине не открывается: поле пустое, причина названа словами, ввод заново лечит.

`tasklist` во время работы агента не показывает пароля ни в одной командной строке; поиск по журналу за полный цикл его не находит.

Установка с открытыми паролями продолжает работать до первого «Сохранить» — ни один существующий стенд не встал от обновления.

Дескриптор публикации не несёт ни логина, ни пароля, и вызов сервиса без учётных данных получает `401` вместо сеанса с полными правами.
