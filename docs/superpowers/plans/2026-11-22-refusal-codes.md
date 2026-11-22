# Коды отказов и метка сборки — план работ

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Каждый отказ, который человек видит, несёт четырёхзначный номер и метку сборки, чтобы из фразы, произнесённой по телефону, мы попадали ровно в одну точку кода.

**Architecture:** Номера и их формат живут в общем крейте `common` (его видят оба бинаря); каждая точка отказа несёт литерал номера; бухгалтерию держит тест, который читает исходники мастерской и падает на повторе, на номере с пенсии и на голом отказе без номера. Метку сборки кладёт `build.rs` в переменные окружения сборки.

**Tech Stack:** Rust 2021, крейты `onec-connect` и `dev1c-setup` в мастерской `setup/`, окно на `gpui-kit`, ошибки через `anyhow`, тесты — `cargo test --lib`.

**Spec:** `docs/superpowers/specs/2026-11-22-refusal-codes-design.md`

## Global Constraints

- Номер — ровно четыре цифры: первые две — область, последние две — место. Области: 10 параметры, 11 запуск, 12 ворота, 13 отказы 1С, 14 операции, 15 факты проверки, 16 мост и сеанс.
- Номер стоит **впереди** текста: `1204 · Нельзя включить: каталога базы нет`. Разделитель — пробел, `·`, пробел.
- Номер не переезжает никогда: убранная ветка отправляет свой номер в список пенсионеров, соседу он не достаётся.
- Зелёные строки номеров не несут. Жёлтые (`Check::busy`) — несут.
- Метка сборки в строку отказа не повторяется: она стоит один раз в шапке окна (`1c-connect · 22.11 пилот`).
- Отсутствие `.git` при сборке — не отказ сборки, а короткая форма метки `22.11 без хеша`.
- Внешних крейтов не добавлять. Чтение исходников в тесте — обычный `std::fs`.
- Крейт `dev1c-setup` в этой работе номеров не получает и правок не требует: новое поле `Check` необязательное.
- Сборка без окна (`--no-default-features`) обязана остаться рабочей: механизм номеров живёт вне фичи `gui`.
- Прогон после каждой задачи: `cargo test --lib` в `setup/1c-connect` с `CARGO_TARGET_DIR=C:\cargo-target\pilot-28-10`. На машине одна цель сборки — две `cargo` разом не запускать.

## Review Focus

- **Подвал обрезан двумя строками** (`panel.rs:306`): на самом длинном отказе («инструментов в комплекте нет (vrunner/, git/, httpd/) — скопируйте полный комплект заново») номер обязан остаться видимым. Тест — в задаче 4.
- **`explain` подменяет текст отказа** (`gui/mod.rs:996`): номер места, где упали, обязан доехать до человека вместе с номером причины. Тест — в задаче 5.
- **Сборка в копии без `.git`** (`gpu.sh sync`): метка обязана получиться, сборка — не падать. Тест — в задаче 2.
- **Cargo не пересобирает `build.rs` сам**: без `rerun-if-changed` метка останется вчерашней, и обещание «видно, какая сборка» станет враньём. Тест — в задаче 2.
- **Новое поле в общем `Check`**: `dev1c-setup` обязан собираться без единой правки. Тест — в задаче 1.

---

### Task 1: Механизм номеров и его сторож

**Files:**
- Create: `setup/common/src/code.rs`
- Modify: `setup/common/src/lib.rs` (объявить `pub mod code;`), `setup/common/src/check.rs` (поле и конструкторы), `setup/common/Cargo.toml` не трогать
- Test: тесты внутри `setup/common/src/code.rs`

**Interfaces:**
- Consumes: ничего.
- Produces: `common::code::say(code: u16, text: impl AsRef<str>) -> String` — «`1204 · текст`»; `common::code::split(text: &str) -> (Option<u16>, &str)` — обратная к `say`, вынимает номер из начала строки и отдаёт остаток без него (номера нет — `(None, text)`); `common::code::AREAS: &[(u16, &str)]`; `common::code::RETIRED: &[(u16, &str)]`; макросы `common::code_bail!(1204, "…")` и `common::code_ensure!(cond, 1204, "…")` (обёртки над `anyhow::bail!`/`ensure!`, текст прогоняется через `say`); `common::check::Check` получает поле `pub code: u16` (ноль — номера нет) и конструкторы `Check::fail_code(code: u16, what: &str, detail: impl Into<String>, hint: impl Into<String>) -> Check` и `Check::busy_code(…)` с той же подписью.

- [ ] **Step 1: Написать красный тест формата и областей**

```rust
#[test]
fn a_code_stands_before_the_words_and_belongs_to_a_known_area() {
    assert_eq!(say(1204, "каталога базы нет"), "1204 · каталога базы нет");
    assert!(AREAS.iter().any(|(a, _)| *a == 12));
    assert!(!AREAS.iter().any(|(a, _)| *a == 99));
}
```

- [ ] **Step 2: Прогон — тест падает**

Run: `cargo test --lib -p common code::` (из `setup/common`)
Expected: FAIL — модуля `code` нет.

- [ ] **Step 3: Написать `setup/common/src/code.rs`**

`say` склеивает `"{code} · {text}"`. `AREAS` — таблица из семи строк (10…16) с названием области словами, как в замысле. `RETIRED` — пустой массив той же формы с комментарием, как им пользоваться. Макросы объявить через `#[macro_export]`, текст собирать `format!` и прогонять через `say`.

- [ ] **Step 4: Прогон — тест проходит**

- [ ] **Step 5: Написать красный тест на то, что `Check` несёт номер, а старые конструкторы — нет**

```rust
#[test]
fn only_the_coded_constructors_carry_a_number() {
    assert_eq!(Check::fail_code(1501, "база", "нет каталога", "base_path").code, 1501);
    assert_eq!(Check::busy_code(1502, "база", "занята", "закройте 1С").code, 1502);
    assert_eq!(Check::fail("база", "нет каталога", "base_path").code, 0);
    assert_eq!(Check::pass("база", "C:\\Bases\\Buh").code, 0);
}
```

- [ ] **Step 6: Прогон — падает; добавить поле и конструкторы в `check.rs`; прогон — проходит**

Поле `code: u16` со значением `0` в существующих конструкторах. `line()` печатает номер перед именем факта, только когда он не ноль.

- [ ] **Step 7: Написать сторожа — тест, читающий исходники мастерской**

Файл тот же. Тест `the_numbers_do_not_collide_and_no_refusal_goes_without_one` обходит `CARGO_MANIFEST_DIR/../*/src/**/*.rs`, собирает литералы номеров из вызовов `say(`, `code_bail!(`, `code_ensure!(`, `fail_code(`, `busy_code(` и проверяет четыре правила: номер встречается один раз; номера нет в `RETIRED`; первые две цифры есть в `AREAS`; в файле, не входящем в список `UNCOVERED`, не осталось голых `bail!`, `ensure!`, `anyhow!`, `Check::fail(`, `Check::busy(` вне `mod tests`.

- [ ] **Step 8: Завести список `UNCOVERED`**

Константа в том же тесте: пути файлов, которые ещё не размечены — все файлы `1c-connect/src`, кроме уже покрытых, и весь `dev1c-setup/src`. Рядом комментарий: каждая следующая задача плана вычёркивает свои файлы, `dev1c-setup` остаётся навсегда и назван отдельной строкой.

- [ ] **Step 9: Прогон целиком — зелено, `dev1c-setup` собирается без правок**

Run: `cargo test --lib` в `setup/common`, затем `cargo build -p dev1c-setup` из `setup/`
Expected: оба зелёные; правок в `dev1c-setup` нет ни одной.

- [ ] **Step 10: Коммит**

```bash
git add setup/common/src
git commit -m "Механизм номеров отказов: формат, области, пенсия и сторож в тесте"
```

---

### Task 2: Метка сборки

**Files:**
- Modify: `setup/1c-connect/build.rs`, `setup/1c-connect/src/main.rs` (строка версии у `clap` и `--version`), `setup/1c-connect/src/gui/window.rs` (заголовок окна), `setup/1c-connect/src/gui/log.rs` (первая строка журнала сеанса)
- Test: `setup/1c-connect/build.rs` чистую часть вынести в `setup/common/src/code.rs` и проверить там

**Interfaces:**
- Consumes: `common::code` (задача 1).
- Produces: `common::code::stamp_short(date: &str, branch: &str) -> String` — «`22.11 пилот`»; `common::code::stamp_full(date: &str, hash: Option<&str>, branch: &str) -> String` — «`2026-11-22 a3f1c2d worktree-pilot-28-10`» или «`2026-11-22 без хеша worktree-pilot-28-10`»; переменные сборки `BUILD_STAMP_SHORT` и `BUILD_STAMP_FULL`, читаемые через `env!`.

- [ ] **Step 1: Написать красный тест на обе формы метки**

```rust
#[test]
fn a_build_without_git_still_has_an_honest_stamp() {
    assert_eq!(stamp_short("22.11", "worktree-pilot-28-10"), "22.11 пилот");
    assert_eq!(stamp_short("22.11", "main"), "22.11 main");
    assert_eq!(stamp_full("2026-11-22", Some("a3f1c2d"), "main"), "2026-11-22 a3f1c2d main");
    assert_eq!(stamp_full("2026-11-22", None, "main"), "2026-11-22 без хеша main");
}
```

Соответствие `worktree-pilot-28-10` → `пилот` — таблица в `code.rs`; ветка без строки в таблице называет себя своим именем.

- [ ] **Step 2: Прогон — падает; написать обе функции; прогон — проходит**

- [ ] **Step 3: Научить `build.rs` отдавать метку**

Дата — локальная дата сборки в виде `22.11` и `2026-11-22`. Хеш и ветка — `git rev-parse --short HEAD` и `git rev-parse --abbrev-ref HEAD`; команда не вышла или `.git` нет — `None` и имя ветки `сборка`. Обязательные `cargo:rerun-if-changed=../../.git/HEAD` и на файл текущей ветки, иначе метка застрянет на вчерашней. Отказ `git` сборку не валит.

- [ ] **Step 4: Проверить обе дороги руками**

Run: `cargo build --bin 1c-connect`, затем `1c-connect.exe --version`
Expected: печатается полная метка с хешем. Затем скопировать дерево без `.git` во временный каталог, собрать там и убедиться: сборка проходит, метка — «без хеша».

- [ ] **Step 5: Показать короткую метку в шапке окна и полную — в журнале и отчёте**

Заголовок окна: `1c-connect · 22.11 пилот`. Первая строка журнала сеанса и строка версии в отчёте «Проверить» (`common::check::render` — параметр `version`) несут полную форму.

- [ ] **Step 6: Прогон и коммит**

```bash
git add setup/1c-connect setup/common
git commit -m "Метка сборки: дата, ветка и хеш — в шапке окна, в журнале и в --version"
```

---

### Task 3: Область 15 — факты проверки

**Files:**
- Modify: `setup/1c-connect/src/verify.rs` (39 мест `Check::fail`/`Check::busy`)
- Test: там же

**Interfaces:**
- Consumes: `Check::fail_code`, `Check::busy_code` (задача 1).
- Produces: номера 1501…15NN; `verify::base_fact` несёт свой номер — его пропускают через себя ворота (задача 4).

- [ ] **Step 1: Написать красный тест на то, что факты различимы номерами**

```rust
#[test]
fn every_failing_fact_carries_its_own_number() {
    let c = base_fact("C:\\Bases\\Нет", false);
    assert!(!c.ok && c.code / 100 == 15, "{c:?}");
    assert_ne!(base_fact("C:\\Bases\\Нет", false).code, db_user_fact("  ").code);
}
```

- [ ] **Step 2: Прогон — падает (номера нулевые)**

- [ ] **Step 3: Разметить все 39 мест**

Номера по порядку заведения, с 1501. Текст фактов не трогать: эта работа номера расставляет, а слова правит отдельная.

- [ ] **Step 4: Вычеркнуть `verify.rs` из `UNCOVERED`, прогнать всё**

Expected: сторож зелёный — в `verify.rs` голых `Check::fail`/`Check::busy` не осталось.

- [ ] **Step 5: Коммит**

---

### Task 4: Область 12 — ворота

**Files:**
- Modify: `setup/1c-connect/src/gate.rs` (12 возвратов `cold_gate`), `setup/1c-connect/src/gui/panel.rs` (`foot_left_text`)
- Test: `gate.rs`, `gui/panel.rs`

**Interfaces:**
- Consumes: номера фактов (задача 3), `common::code::say`.
- Produces: `cold_gate` по-прежнему возвращает `Option<String>`, но строка начинается с номера.

- [ ] **Step 1: Написать красный тест: причина начинается с номера, а причина из факта несёт номер факта**

```rust
#[test]
fn a_locked_switch_says_its_number_first() {
    let mut c = crate::cfg::filled_for_test();
    c.host.base_path = "C:\\Bases\\Нет".into();
    let why = cold_gate(&c, true, false, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid).unwrap();
    assert!(why.starts_with(&format!("{} · ", crate::verify::base_fact(&c.host.base_path, false).code)), "{why}");
    // Пустое поле — своя причина ворот, не факта.
    c.host.base_path = "  ".into();
    let own = cold_gate(&c, true, false, &done(), Ok(Path::new("1cv8.exe")), true, &licence::State::Valid).unwrap();
    assert!(own.starts_with("12"), "{own}");
}
```

- [ ] **Step 2: Прогон — падает; разметить 12 возвратов; прогон — проходит**

Причины, пришедшие из факта или из `platform_choice`, номер не выдумывают, а несут чужой — одна причина, один номер.

- [ ] **Step 3: Написать тест на то, что номер переживает обрез подвала**

```rust
#[test]
fn the_number_survives_the_two_line_clamp() {
    let longest = "инструментов в комплекте нет (vrunner/, git/, httpd/) — скопируйте полный комплект заново";
    let (text, _) = foot_left_text(None, Some(&common::code::say(1207, longest)), None, "C:\\kit");
    assert!(text.starts_with("1207 · "), "{text}");
}
```

- [ ] **Step 4: Вычеркнуть `gate.rs` из `UNCOVERED`, прогон, коммит**

---

### Task 5: Область 13 — отказы 1С, и два номера в одной строке

**Files:**
- Modify: `setup/1c-connect/src/onec_error.rs` (8 причин в `TABLE`, `Cause`, `hint_line`), `setup/1c-connect/src/gui/mod.rs` (обработчик `Done`, ~996)
- Test: `onec_error.rs`

**Interfaces:**
- Consumes: `common::code::say`.
- Produces: `Cause` получает поле `pub code: u16` (1301…1308); `onec_error::explain_line(raw: &str) -> Option<String>` — готовая строка вида `1402·1305 · Превышено количество сеансов`, где первое число взято из текста отказа, если оно там есть, второе — номер причины.

- [ ] **Step 1: Написать красный тест на оба номера**

```rust
#[test]
fn a_recognised_refusal_keeps_the_number_of_the_place_it_came_from() {
    let raw = common::code::say(1402, "Превышено количество сеансов с клиентскими лицензиями");
    let line = explain_line(&raw).unwrap();
    assert!(line.starts_with("1402·13"), "{line}");
    // Нераспознанный отказ остаётся собой, номер места не теряет.
    assert_eq!(explain_line(&common::code::say(1402, "что-то новое")), None);
}
```

- [ ] **Step 2: Прогон — падает; разметить 8 причин, написать `explain_line`; прогон — проходит**

Номер места вынимается из начала строки отказа функцией `common::code::split` (заведена в задаче 1) — тем же форматом, которым его туда положили.

- [ ] **Step 3: Подвал и уведомление показывают строку из `explain_line`**

- [ ] **Step 4: Вычеркнуть `onec_error.rs` из `UNCOVERED`, прогон, коммит**

---

### Task 6: Область 14 — операции

**Files:**
- Modify: `setup/1c-connect/src/ops_host.rs` (21 отказ), `setup/1c-connect/src/publication.rs`, `setup/1c-connect/src/project.rs`, `setup/1c-connect/src/platform.rs`, `setup/1c-connect/src/licence.rs` — все `bail!`/`ensure!`/`anyhow!` вне тестов
- Test: `ops_host.rs`

**Interfaces:**
- Consumes: `code_bail!`, `code_ensure!` (задача 1).
- Produces: номера 1401…14NN.

- [ ] **Step 1: Написать красный тест на то, что отказ операции несёт номер**

```rust
#[test]
fn an_operation_refusal_says_its_number() {
    let e = run_mcp_for_test_without_kit().unwrap_err().to_string();
    assert!(common::code::split(&e).0.is_some_and(|c| c / 100 == 14), "{e}");
}
```

Если такой точки входа нет, взять любую `pub` функцию модуля, чей отказ достижим без 1С, и назвать её в тесте.

- [ ] **Step 2: Прогон — падает; заменить все голые отказы на `code_bail!`/`code_ensure!`; прогон — проходит**

- [ ] **Step 3: Вычеркнуть файлы из `UNCOVERED`, прогон, коммит**

---

### Task 7: Области 10, 11, 16 — параметры, запуск, мост

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs` (10 отказов `validate`), `setup/1c-connect/src/main.rs` (8), `setup/1c-connect/src/single_instance.rs`, `setup/1c-connect/src/gui/autostart.rs`, `setup/1c-connect/src/gui/tray.rs`, `setup/1c-connect/src/bridge.rs`, `setup/1c-connect/src/session.rs`, `setup/1c-connect/src/listen.rs`, `setup/1c-connect/src/tunnel.rs`
- Test: `cfg.rs`

**Interfaces:**
- Consumes: `code_bail!`, `code_ensure!`.
- Produces: номера 1001…10NN, 1101…11NN, 1601…16NN; `UNCOVERED` после этой задачи содержит одну строку — `dev1c-setup`.

- [ ] **Step 1: Написать красный тест на отказ разбора параметров**

```rust
#[test]
fn a_bad_parameter_file_says_its_number() {
    let mut c = Cfg::default();
    c.host.base_path = String::new();
    let e = c.validate().unwrap_err().to_string();
    assert!(common::code::split(&e).0.is_some_and(|n| n / 100 == 10), "{e}");
}
```

- [ ] **Step 2: Прогон — падает; разметить все места трёх областей; прогон — проходит**

- [ ] **Step 3: Очистить `UNCOVERED` до единственной строки `dev1c-setup`**

Сторож теперь красный на любом новом голом отказе в `1c-connect`.

- [ ] **Step 4: Прогон целиком, в двух сборках**

Run: `cargo test --lib`, затем `cargo build --no-default-features`
Expected: оба зелёные — механизм номеров живёт вне фичи `gui`.

- [ ] **Step 5: Коммит**

---

### Task 8: Живой прогон и замер обещаний

**Files:**
- Modify: ничего, кроме найденного по дороге
- Test: живой прогон окна

- [ ] **Step 1: Собрать и поднять окно с пустыми параметрами**

Run: `cargo build --bin 1c-connect`, запуск из каталога сборки
Expected: в шапке — `1c-connect · <дата> <ветка>`; в подвале — отказ, начинающийся с номера.

- [ ] **Step 2: Проверить, что номер попадает в одно место**

Взять номер из подвала и выполнить поиск по исходникам. Expected: ровно одно совпадение вне тестов.

- [ ] **Step 3: Проверить тот самый отказ, с которого всё началось**

Поставить базой `srv-1c:1541\buh` на старой сборке (`git stash` не использовать — отдельный каталог по `git worktree add`) и убедиться: номер называет ветку ворот, которая сработала, а не ту, о которой говорят слова.

- [ ] **Step 4: Записать в журнал работы, что обещания замерены, и коммитом закрыть ветку задач**

---

## Самопроверка плана

- **Покрытие замысла.** Номер и его формат — задача 1; пенсия и сторож — задача 1; метка сборки — задача 2; «одна причина — один номер» — задачи 3 и 4; два номера у отказа операции — задача 5; все области — задачи 3, 4, 5, 6, 7; «номер впереди, не в конце» — задача 4 (тест обреза); `dev1c-setup` без правок — задача 1 (шаг 9).
- **Типы сходятся.** `say`, `split`, `stamp_short`, `stamp_full`, `Check::fail_code`, `Check::busy_code`, `explain_line` названы в тех же подписях во всех задачах, где встречаются.
- **Review Focus разложен по задачам.** Обрез подвала — задача 4, шаг 3. Подмена текста `explain` — задача 5, шаг 1. Сборка без `.git` — задача 2, шаг 4. `rerun-if-changed` — задача 2, шаг 3. Поле в общем `Check` — задача 1, шаг 9.
- **Соразмерность.** План короче замысла по числу решений и не содержит тел функций там, где их определяют подпись и тест.
