# Публикация в карточке, чистое меню значка, удаление соединения — план

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Довести окно до состояния, обещанного спекой: публикация базы правится в карточке соединения, в меню значка её и «Режима работы» больше нет вовсе, а ненужную карточку можно удалить из окна — не трогая ни одного файла человека.

**Architecture:** Переключатели карточки уже ходят одним путём: точечная запись одного ключа у НЕтекущей карточки и полная последовательность трея (отказ под идущей операцией → вопрос про смену базы → запись → перецикл сеанса → публикация) у текущей. Публикация становится третьим переключателем на этом же пути; её словарь ответов переезжает туда, где живут остальные слова карточки (`gui::link`), и остаётся единственным — журнальная строка трея читает его же. Удаление — точечная правка текста файла (вырезание блока `[[connection]]`), по тому же доводу, по которому точечные писатели вообще появились: в соседних блоках лежат пароли, которых эта машина могла не открыть. Четыре точечных писателя сводятся к одному (`set_keys`), и это заодно делает пару «роль + режим» одной записью вместо двух.

**Tech Stack:** Rust 2021, workspace `setup/`; крейт `onec-connect` (двоичный `1c-connect`), крейт `common` (`toml_render`); окно на GPUI через `gpui-kit`; значок на `tray-icon`.

**Spec:** `docs/superpowers/specs/2026-11-01-interface-design.md` — решения владельца 1.11 (пункт 5 развилок: публикация — поле соединения, пункта меню нет вовсе) и 4.11 (раздел «удаление соединения из окна есть, и папку оно не трогает»).

## Global Constraints

- Обратная совместимость НЕ нужна: установок нигде нет (решение владельца 4.11, прошлая ветка).
- Слова на экране и в журнале — обращением к человеку, без жаргона кода: ни «toml», ни «блок», ни имён полей в тексте для человека.
- Программа не удаляет файлов человека нигде и ни в каком случае; удаление карточки удаляет запись в настройках и только её.
- Точечная запись правит ОДИН блок в тексте файла, прочитанном с диска, и не перепечатывает файл из памяти: `cfg::set_named`/`cfg::block_in` адресуют блок ПО ИМЕНИ.
- Один словарь на одно понятие: слова ответов переключателей живут в `gui::link`, и второй копии у них нет (урок задачи 16 прошлой ветки — три копии словаря ответов).
- `rustfmt.toml`: `max_width = 140`. Гейты: `cargo test -p onec-connect --lib`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`.
- Один `cargo` на машину в один момент: исполнитель гоняет ТОЛЬКО свои именованные тесты или модуль, ревьюер не запускает `cargo` вовсе, полный прогон делает ведущий.

## Review Focus

Пять входов, которые спека подразумевает, а задачи сами по себе не проверяют. Тест на каждый добавлен в задачу-владельца.

1. **Удаление карточки, чей блок в файле не один** (две карточки с одним именем правкой руками) — удаление обязано отказать словами, а не вырезать первый попавшийся блок. Тест в задаче 1.
2. **Удаление, когда файл на диске разошёлся с памятью** (человек правил toml, пока окно было открыто) — отказ словами, файл не тронут. Тест в задаче 1.
3. **Удаление последней карточки при живой своей публикации** — httpd гасится ДО записи, иначе он остаётся публиковать базу, которой в настройках больше нет. Тест в задаче 2.
4. **Смена публикации у НЕтекущей карточки, в чьём файле лежит выведенный нами адрес** — адрес обязан быть снят в той же записи, иначе карточка показывает пустое поле, а в файле лежит наша догадка, и она уедет в пробу. Тест в задаче 5.
5. **Неизвестная роль в файле, правленном руками** — список не вправе молча показать такую карточку под подписью «Принимаю подключения». Тест в задаче 7.

---

### Задача 1: вырезать блок из текста файла

**Files:**
- Modify: `setup/common/src/toml_render.rs` — новая `remove_at`
- Modify: `setup/1c-connect/src/cfg.rs` — новая `remove_named`, рядом с `set_named`/`block_in`
- Test: `setup/common/src/toml_render.rs` (`mod tests` в конце файла), `setup/1c-connect/src/cfg.rs` (`mod tests`)

**Interfaces:**
- Produces: `common::toml_render::remove_at(text: &str, section: &str, nth: usize) -> Option<String>` — текст без `nth`-го блока секции; `None` — такого блока нет.
- Produces: `cfg::remove_named(text: &str, name: &str) -> std::result::Result<String, String>` — текст файла без блока соединения `name`; отказ словами, если блока нет, или если блоков с этим именем больше одного (через `block_in`).

- [ ] **Шаг 1: падающие тесты `remove_at`**

```rust
#[test]
fn remove_at_cuts_one_block_and_leaves_the_rest_byte_for_byte() {
    let text = "current = \"a\"\n\n[[connection]]\nname = \"a\"\npassword = \"ШИФР-A\"\n\n[[connection]]\nname = \"b\"\npassword = \"ШИФР-B\"\n";
    let out = remove_at(text, "[[connection]]", 0).unwrap();
    assert_eq!(out, "current = \"a\"\n\n[[connection]]\nname = \"b\"\npassword = \"ШИФР-B\"\n");
    let out = remove_at(text, "[[connection]]", 1).unwrap();
    assert_eq!(out, "current = \"a\"\n\n[[connection]]\nname = \"a\"\npassword = \"ШИФР-A\"\n");
}

#[test]
fn remove_at_keeps_comments_of_the_neighbours_and_the_root() {
    let text = "# шапка\ncurrent = \"a\"\n\n[[connection]]\n# про a\nname = \"a\"\n\n[[connection]]\n# про b\nname = \"b\"\n";
    assert_eq!(remove_at(text, "[[connection]]", 0).unwrap(), "# шапка\ncurrent = \"a\"\n\n[[connection]]\n# про b\nname = \"b\"\n");
}

#[test]
fn remove_at_without_such_a_block_changes_nothing() {
    assert_eq!(remove_at("current = \"a\"\n", "[[connection]]", 0), None);
    assert_eq!(remove_at("[[connection]]\nname = \"a\"\n", "[[connection]]", 1), None);
}
```

- [ ] **Шаг 2: прогнать, убедиться, что падают**

Run: `cargo test -p common --lib remove_at`
Expected: FAIL — `cannot find function remove_at`.

- [ ] **Шаг 3: написать `remove_at`**

Тем же разбором, что `set_in`: `text.split('\n')` (чтобы сохранить `\r` и наличие перевода строки в конце), заголовки узнаются `section_of`. Удаляются строки от заголовка нужного блока до строки перед следующим заголовком ЛЮБОЙ секции (или до конца текста), вместе с пустыми строками, что стояли внутри блока; пустая строка-разделитель перед заголовком удаляемого блока уходит с ним, чтобы в тексте не оставалось двух пустых строк подряд.

- [ ] **Шаг 4: прогнать — проходят**

Run: `cargo test -p common --lib remove_at`
Expected: PASS.

- [ ] **Шаг 5: падающие тесты `cfg::remove_named`** (Review Focus 1 и 2)

```rust
#[test]
fn remove_named_cuts_the_named_block_and_leaves_the_foreign_ciphertext_untouched() { /* два блока, в соседнем password = "ШИФР"; после удаления строка с ШИФР на месте */ }

#[test]
fn remove_named_refuses_a_name_that_is_not_in_the_file() { /* отказ содержит имя и слова «настройки на диске изменились» (stale_file) */ }

#[test]
fn remove_named_refuses_two_blocks_with_the_same_name() { /* отказ — тот же duplicate_block, что у block_in; текст не изменён */ }
```

- [ ] **Шаг 6: прогнать, убедиться, что падают**

Run: `cargo test -p onec-connect --lib cfg::tests::remove_named`
Expected: FAIL — функции нет.

- [ ] **Шаг 7: написать `cfg::remove_named`**

`block_in(text, name)?` (он уже отказывает и на «нет такого», и на дубле имени), затем `toml_render::remove_at(text, CONNECTION, nth)`, `None` → тот же `stale_file(name)`. Корневой ключ `current` здесь НЕ трогается: решение, что делать с текущим, принимает вызывающий (задача 2), а у текста файла одна забота — вырезать блок.

- [ ] **Шаг 8: прогнать — проходят**

Run: `cargo test -p onec-connect --lib cfg::tests::remove_named`
Expected: PASS.

- [ ] **Шаг 9: коммит**

```bash
git add setup/common/src/toml_render.rs setup/1c-connect/src/cfg.rs
git commit -m "Вырезание блока соединения из текста файла: remove_at и remove_named"
```

---

### Задача 2: удаление карточки — предусловия, уборка, запись

**Files:**
- Modify: `setup/1c-connect/src/gui/save.rs` — новая `delete_connection`, рядом с `switch_current`
- Test: `setup/1c-connect/src/gui/save/tests.rs`

**Interfaces:**
- Consumes: `cfg::remove_named` (задача 1).
- Produces: `save::delete_connection(ui: &mut Ui, name: &str) -> Result<(), String>` — удаляет карточку из файла и из `ui.cfg`; говорит в журнал одну строку `Kind::Ok` («Соединение «X» удалено. Папка и база остались на месте»).
- Produces: `save::delete_blocked_reason(ui: &Ui, name: &str) -> Option<String>` — чистая проверка предусловий: `Some(слова отказа)`, если удалять нельзя. Её же зовёт проводка окна (задача 3), чтобы не спрашивать подтверждение у действия, которое всё равно откажет.

- [ ] **Шаг 1: падающие тесты предусловий**

```rust
#[test]
fn deleting_the_current_card_is_refused_while_other_cards_exist() {
    // два соединения, текущее — первое; отказ называет, что сделать:
    assert!(delete_blocked_reason(&ui, "Buh").unwrap().contains("сделайте текущим другое"));
}

#[test]
fn deleting_the_only_card_is_allowed_even_though_it_is_current() {
    assert_eq!(delete_blocked_reason(&ui, "Buh"), None);
}

#[test]
fn deleting_anything_is_refused_while_an_operation_is_running() {
    // ui.progress = Some(..) и отдельно ui.publishing = Some(..):
    // отказ называет, ЧТО идёт — тем же словом, что `tray::busy_now_word`
    assert!(delete_blocked_reason(&ui, "Ромашка").unwrap().contains("публикация базы"));
}

#[test]
fn deleting_a_card_that_is_not_in_the_list_is_refused_by_name() { /* отказ содержит имя */ }
```

- [ ] **Шаг 2: прогнать, убедиться, что падают**

Run: `cargo test -p onec-connect --lib gui::save::tests::deleting`
Expected: FAIL — функции нет.

- [ ] **Шаг 3: написать `delete_blocked_reason`**

Порядок проверок: нет такой карточки → идёт операция/проверка/публикация (`crate::gui::tray::busy_now_word` для слова) → карточка текущая И в списке есть другие. Третья проверка — последняя: человеку важнее услышать про идущую операцию.

- [ ] **Шаг 4: падающие тесты записи** (Review Focus 3)

```rust
#[test]
fn deleting_a_card_writes_the_file_without_its_block_and_keeps_the_others() { /* файл после: соседний блок и его шифртекст на месте, удалённого имени в тексте нет */ }

#[test]
fn deleting_the_only_card_leaves_a_file_with_no_connections_and_an_empty_current() {
    // текст после: `current = ""`, ни одного блока; `ui.cfg.connections` пуст
}

#[test]
fn deleting_the_only_card_puts_down_our_own_publication_before_writing() {
    // ui.raised_publication = true → после удачи признак снят
}

#[test]
fn a_refused_delete_leaves_the_file_byte_for_byte() { /* сравнение всего текста до и после */ }
```

- [ ] **Шаг 5: прогнать, убедиться, что падают**

Run: `cargo test -p onec-connect --lib gui::save::tests::deleting`
Expected: FAIL.

- [ ] **Шаг 6: написать `delete_connection`**

Порядок шагов строгий, и он тот же, что у переключения текущего: `delete_blocked_reason` → (только для последней карточки) гасим сеанс и снимаем свою публикацию `crate::ops_host::stop_own_publication` со снимком СТАРОГО `Cfg` (`gui::quiet_ctx_with`, он в `gui/mod.rs` приватен для модуля — этого достаточно, `save.rs` внутри него), признак `ui.raised_publication` сбрасывается только по удаче → читаем файл с диска → `cfg::remove_named` → для последней карточки тем же текстом правим корневой `current` на пустую строку (`toml_render::set(&text, "", "current", "\"\"")`) → одна запись файла → правим `ui.cfg` (убираем карточку из списка; если удалили последнюю — `ui.cfg.current` пустой) → строка в журнал.

Гашение сеанса в этой функции НЕ делается: `Ui` без `App` сеансом не управляет. Последовательность для последней карточки собирает проводка (задача 3) — она гасит сеанс до вызова, тем же `gui::link::stop_session`, что у смены режима; `delete_connection` отвечает за уборку публикации и запись.

- [ ] **Шаг 7: прогнать — проходят**

Run: `cargo test -p onec-connect --lib gui::save::tests::deleting`
Expected: PASS.

- [ ] **Шаг 8: коммит**

```bash
git add setup/1c-connect/src/gui/save.rs setup/1c-connect/src/gui/save/tests.rs
git commit -m "Удаление карточки: предусловия словами, уборка публикации, одна запись файла"
```

---

### Задача 3: пункт «Удалить соединение» и вопрос перед удалением

**Files:**
- Modify: `setup/1c-connect/src/gui/panel.rs` — `LinkAct::Delete`, `link_menu`, `delete_question_text`, `run_link_act`, сброс `Panel::selected`
- Test: `setup/1c-connect/src/gui/panel.rs` (`mod tests`)

**Interfaces:**
- Consumes: `save::delete_connection`, `save::delete_blocked_reason` (задача 2).
- Produces: `panel::delete_question_text(name: &str) -> (String, String)` — заголовок и подробность вопроса; `LinkAct::Delete`.

- [ ] **Шаг 1: падающие тесты меню и слов**

```rust
#[test]
fn the_menu_of_a_card_ends_with_delete_and_the_whole_list_is_pinned() {
    // ПОЛНОЕ равенство списка, а не contains (урок задачи 10 прошлой ветки):
    assert_eq!(menu_labels(&ui_dial_current()), vec!["Подключиться", "Переименовать", "Удалить соединение"]);
    assert_eq!(menu_labels(&ui_other_card()), vec!["Переименовать", "Удалить соединение"]);
}

#[test]
fn the_delete_question_names_the_card_and_promises_the_folder_stays() {
    let (message, detail) = delete_question_text("Ромашка");
    assert!(message.contains("«Ромашка»"), "{message}");
    assert!(detail.contains("папка") && detail.contains("останут"), "{detail}");
}
```

- [ ] **Шаг 2: прогнать, убедиться, что падают**

Run: `cargo test -p onec-connect --lib gui::panel::tests::delete`
Expected: FAIL.

- [ ] **Шаг 3: добавить `LinkAct::Delete`, пункт в `link_menu` и `delete_question_text`**

Пункт стоит ПОСЛЕДНИМ в обеих ветвях `link_menu` (у текущей карточки и у чужой): необратимое действие не встаёт выше обратимых. Текст вопроса: заголовок «Удалить соединение «X»?», подробность — одно предложение о том, что удаляется только запись в настройках, а папка, база и выгруженные файлы остаются на месте.

- [ ] **Шаг 4: провести щелчок в `run_link_act`**

`LinkAct::Delete`: `save::delete_blocked_reason` → отказ строкой в журнал (`Kind::Fail`) и выход; иначе `gui::ask_yes_no` с текстом из `delete_question_text`, кнопка согласия «Да, удалить». В согласии: если удаляется ПОСЛЕДНЯЯ карточка — `gui::link::stop_session(cx)` до записи; затем `save::delete_connection`; затем сбросить выбор панели (`Panel::select` на пустое имя) и `cx.refresh_windows()`.

- [ ] **Шаг 5: прогнать тесты задачи и модуль целиком**

Run: `cargo test -p onec-connect --lib gui::panel`
Expected: PASS, прежние тесты модуля тоже.

- [ ] **Шаг 6: коммит**

```bash
git add setup/1c-connect/src/gui/panel.rs
git commit -m "Меню «Связи»: «Удалить соединение» последним пунктом, с вопросом о сохранности папки"
```

---

### Задача 4: один точечный писатель вместо четырёх

**Files:**
- Modify: `setup/1c-connect/src/gui/save.rs` — новая `set_keys`, через неё `set_profile`/`set_role`/`set_mode`/`set_base_here`
- Modify: `setup/1c-connect/src/gui/panel.rs` — `set_card_topology_kind` пишет роль и режим ОДНОЙ записью, когда уходит из локального
- Test: `setup/1c-connect/src/gui/save/tests.rs`

**Interfaces:**
- Produces: `save::set_keys(ui: &mut Ui, name: &str, keys: &[(&str, &str)]) -> Result<(), String>` — читает файл один раз, правит перечисленные ключи одного блока и пишет один раз; годность задуманной правки считается до записи тем же `breaks_connection`, что у четырёх прежних писателей.

- [ ] **Шаг 1: падающий тест атомарности**

```rust
#[test]
fn set_keys_writes_every_key_in_one_pass() {
    set_keys(&mut ui, "Buh", &[("role", "dial"), ("mode", "tunnel")]).unwrap();
    let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
    assert!(text.contains("role = \"dial\"") && text.contains("mode = \"tunnel\""));
    assert_eq!(ui.cfg.by_name("Buh").unwrap().role, "dial");
    assert_eq!(ui.cfg.by_name("Buh").unwrap().mode, "tunnel");
}

#[test]
fn set_keys_refuses_a_patch_that_breaks_the_card_and_writes_nothing() {
    // профиль developer при пустой папке — тот же отказ, что был у set_profile;
    // текст файла до и после совпадает побайтово
}
```

- [ ] **Шаг 2: прогнать, убедиться, что падает**

Run: `cargo test -p onec-connect --lib gui::save::tests::set_keys`
Expected: FAIL.

- [ ] **Шаг 3: написать `set_keys` и свести четырёх писателей на неё**

Тело — то же, что у нынешнего `set_mode`, но ключей несколько: `connection_named` → применить правки к копии карточки → `breaks_connection` → прочитать файл → `cfg::set_named` по каждому ключу подряд на одном и том же тексте → одна запись → обновить `ui.cfg`. `set_profile`/`set_role`/`set_mode`/`set_base_here` остаются с теми же подписями и становятся однострочными обёртками: их собственные тесты обязаны пройти БЕЗ правок — это и есть доказательство, что поведение не изменилось.

- [ ] **Шаг 4: прогнать весь модуль записи**

Run: `cargo test -p onec-connect --lib gui::save`
Expected: PASS, включая все прежние тесты четырёх писателей.

- [ ] **Шаг 5: убрать двойную запись из `set_card_topology_kind`**

Там, где карточка уходит из локального режима, роль и режим пишутся одним `set_keys(ui, name, &[("role", answer), ("mode", "tunnel")])` вместо двух записей подряд. Исключение остаётся одно: если карточка ТЕКУЩАЯ, режим по-прежнему идёт через `tray::request_current_mode_change` (сеанс, замок, значок), и тогда роль пишется отдельно — это не две записи одной правки, а две разные последовательности.

- [ ] **Шаг 6: прогнать затронутые модули**

Run: `cargo test -p onec-connect --lib gui::panel gui::save`
Expected: PASS.

- [ ] **Шаг 7: коммит**

```bash
git add setup/1c-connect/src/gui/save.rs setup/1c-connect/src/gui/save/tests.rs setup/1c-connect/src/gui/panel.rs
git commit -m "Один точечный писатель вместо четырёх; роль и режим — одной записью"
```

---

### Задача 5: публикация — переключатель карточки

**Files:**
- Modify: `setup/1c-connect/src/gui/link.rs` — `publication_answers()`, единственный словарь слов публикации
- Modify: `setup/1c-connect/src/gui/save.rs` — `set_publication`
- Modify: `setup/1c-connect/src/gui/tray.rs` — `request_current_publication_change`, журнальная строка читает словарь из `link`
- Modify: `setup/1c-connect/src/gui/panel.rs` — третий переключатель карточки и его проводка `set_card_publication`
- Test: `setup/1c-connect/src/gui/save/tests.rs`, `setup/1c-connect/src/gui/panel.rs`, `setup/1c-connect/src/gui/tray.rs`

**Interfaces:**
- Consumes: `save::set_keys` (задача 4), `tray::mode_change_blocked`/`busy_now_word`, `panel::sync_card_topology`, `tray::apply_publication`.
- Produces: `link::publication_answers() -> [(&'static str, &'static str); 2]` — пары «значение, слова»: `("auto", "Публикует коннектор")`, `("webserver", "Публикует платформа 1С")`.
- Produces: `save::set_publication(ui: &mut Ui, name: &str, publication: &str) -> Result<(), String>`.
- Produces: `tray::request_current_publication_change(cx: &mut App, publication: &'static str) -> bool` — та же форма, что `request_current_mode_change`: `false` — не состоялось из-за идущей операции.

- [ ] **Шаг 1: падающие тесты точечной записи** (Review Focus 4)

```rust
#[test]
fn set_publication_writes_one_key_of_a_foreign_card() { /* publication = "webserver" в блоке «Ромашка», блок «Buh» не тронут */ }

#[test]
fn switching_a_card_to_webserver_drops_the_address_we_derived_ourselves() {
    // base_path = C:\Bases\Buh, base_url — ровно cfg::derived_base_url(base_path);
    // после set_publication(.., "webserver") в файле base_url = ""
}

#[test]
fn switching_a_card_to_webserver_keeps_an_address_the_person_typed() {
    // base_url = "http://localhost:8081/Buh/hs/mcp-1c" остаётся как есть
}
```

- [ ] **Шаг 2: прогнать, убедиться, что падают**

Run: `cargo test -p onec-connect --lib gui::save::tests::set_publication gui::save::tests::switching_a_card`
Expected: FAIL.

- [ ] **Шаг 3: написать `save::set_publication`**

Через `set_keys`: при переходе в `webserver` и выведенном адресе (`cfg::is_derived_address`) в тот же набор ключей попадает `base_url = ""` — довод тот же, что в `tray::apply_publication`: адрес, который написали мы, не ответ человека, и проба по нему ушла бы на чужой веб-сервер.

- [ ] **Шаг 4: падающий тест последовательности у текущей карточки**

```rust
#[test]
fn a_publication_change_of_the_current_card_is_refused_while_anything_is_running() {
    // ui.publishing = Some(..): request_current_publication_change отдаёт false
}
```

- [ ] **Шаг 5: написать `tray::request_current_publication_change`**

Та же форма и тот же порядок, что у `request_current_mode_change`: `mode_change_blocked` → совпадение со нынешним значением (ничего не делаем, `true`) → `save_needs_confirm`/`confirm_database_change` → `apply_publication`. Своей второй копии уборки публикации здесь нет: её делает `apply_publication`.

- [ ] **Шаг 6: падающий тест переключателя карточки**

```rust
#[test]
fn the_card_shows_a_publication_switch_only_where_the_base_is_here() {
    // base_here = false → переключателя нет вовсе (поля базы приходят с той стороны)
    // base_here = true → два ответа словами из link::publication_answers()
}
```

- [ ] **Шаг 7: нарисовать и провести переключатель**

Третий переключатель карточки, под «Где база», рисуется только при `base_here`. Проводка `set_card_publication` — та же развилка, что у `set_card_mode`: текущая карточка идёт через `tray::request_current_publication_change` (а на `false` говорит в журнал словами `busy_now_word`), прочие — `save::set_publication` + `sync_card_topology`.

- [ ] **Шаг 8: прогнать затронутые модули**

Run: `cargo test -p onec-connect --lib gui::panel gui::save gui::tray gui::link`
Expected: PASS.

- [ ] **Шаг 9: коммит**

```bash
git add setup/1c-connect/src/gui
git commit -m "Публикация базы — переключатель карточки, с единственным словарём слов"
```

---

### Задача 6: из меню значка уходят «Режим работы» и «Публикация базы»

**Files:**
- Modify: `setup/1c-connect/src/gui/tray.rs` — удаление подменю, их пунктов, `MenuAction::Mode`/`Publication`, `Tray::set_mode`/`set_publication`, полей `modes`/`publications`
- Modify: `setup/1c-connect/src/gui/mod.rs:2088-2089` — снятие двух вызовов отметок
- Test: `setup/1c-connect/src/gui/tray.rs` (`mod tests`)

**Interfaces:**
- Consumes: словарь `link::publication_answers()` (задача 5) — журнальная строка `apply_publication` берёт слова оттуда; для режима остаётся таблица слов в `tray.rs` без идентификаторов пунктов.

- [ ] **Шаг 1: падающий тест — прежних пунктов нет**

```rust
#[test]
fn the_tray_knows_nothing_about_mode_and_publication_items_any_more() {
    assert_eq!(menu_action("mode:tunnel"), None);
    assert_eq!(menu_action("mode:local"), None);
    assert_eq!(menu_action("publication:auto"), None);
    assert_eq!(menu_action("publication:webserver"), None);
}
```

- [ ] **Шаг 2: прогнать, убедиться, что падает**

Run: `cargo test -p onec-connect --lib gui::tray::tests`
Expected: FAIL — пункты ещё разбираются.

- [ ] **Шаг 3: вырезать подменю и всё, что их держало**

Удаляются: константы `MODES`-с-идентификаторами и `PUBLICATIONS`, `PUBLICATION_SUBMENU`, сборка `mode_items`/`publication_items`/подменю и их место в меню, поля `Tray::modes`/`publications`, методы `set_mode`/`set_publication`, варианты `MenuAction::Mode`/`Publication` и их обработчики, прежние тесты про эти идентификаторы и отметки. `apply_mode`/`apply_publication` остаются: их зовёт карточка. Слова для журнальной строки режима остаются таблицей `[(&str, &str); 2]` в `tray.rs`, слова публикации приходят из `link::publication_answers()`.

- [ ] **Шаг 4: прогнать модуль и собрать всё**

Run: `cargo test -p onec-connect --lib gui::tray`
Expected: PASS.
Run: `cargo clippy -p onec-connect --all-targets -- -D warnings`
Expected: ноль предупреждений (мёртвого кода после удаления не остаётся).

- [ ] **Шаг 5: коммит**

```bash
git add setup/1c-connect/src/gui/tray.rs setup/1c-connect/src/gui/mod.rs
git commit -m "Меню значка: «Режим работы» и «Публикация базы» убраны — это поля соединения"
```

---

### Задача 7: отложенные мелочи, которые стоят починки

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs` — `Connection::validate` проверяет роль
- Modify: `setup/1c-connect/src/gui/link.rs` — четвёртая группа списка для нераспознанной роли
- Modify: `setup/1c-connect/src/gui/save.rs` — `rename` с `from == to` не пишет; «пароли» во множественном числе
- Modify: `setup/1c-connect/src/gui/tray.rs` — `apply_switch_current` перепроверяет предусловие
- Modify: `setup/1c-connect/src/gui/panel.rs` — переименование помощника теста `ui_link_current`
- Test: рядом с каждым изменением

- [ ] **Шаг 1: падающие тесты** (Review Focus 5)

```rust
#[test]
fn validate_rejects_an_unknown_role() { /* role = "сервер" → отказ называет допустимые значения */ }

#[test]
fn a_card_with_an_unreadable_role_does_not_stand_under_the_accepting_caption() {
    // список: подпись «Роль не распознана» и строка под ней, а не под «Принимаю подключения»
}

#[test]
fn renaming_a_card_to_its_own_name_touches_no_file() { /* текст до и после совпадает */ }

#[test]
fn switching_current_rechecks_the_precondition_at_apply_time() {
    // операция началась между вопросом и ответом: применение отказывает словами, current не изменился
}

#[test]
fn switching_to_an_invalid_card_is_refused_with_words() { /* отложенная мелочь задачи 6 прошлой ветки */ }

#[test]
fn an_unknown_profile_refuses_and_leaves_the_file_alone() { /* отложенная мелочь задачи 7 прошлой ветки */ }
```

- [ ] **Шаг 2: прогнать, убедиться, что падают**

Run: `cargo test -p onec-connect --lib`
Expected: перечисленные шесть падают, прочие проходят.

- [ ] **Шаг 3: починить**

- `Connection::validate`: `if let Err(e) = self.role() { anyhow::bail!("{at}: {e}") }` — тем же приёмом, что уже стоит у `configuration()`/`toolset()`; неизвестная роль перестаёт быть законным значением файла.
- `link::list`: `category` отдаёт четвёртую группу для `role()` с ошибкой, `CAPTIONS` получает четвёртую подпись «Роль не распознана»; правило «подписи только когда групп больше одной» остаётся прежним.
- `save::rename`: `from == to` после `trim` — ранний `Ok(())` без чтения и записи файла.
- Текст отказа «добавить карточку сейчас нельзя» в `add_connection` — «пароли» во множественном числе, когда карточек больше одной (там уже выбирается слово `карточек`/`карточки`).
- `tray::apply_switch_current`: перед записью ещё раз `mode_change_blocked` — между вопросом и ответом человек мог начать проверку или публикацию; отказ словами (`busy_now_word`), `current` не меняется.
- `panel::tests::ui_link_current` → имя, говорящее правду о том, что помощник строит (например `ui_current_card`).

- [ ] **Шаг 4: прогнать весь набор**

Run: `cargo test -p onec-connect --lib`
Expected: PASS.

- [ ] **Шаг 5: коммит**

```bash
git add setup/1c-connect/src
git commit -m "Отложенные мелочи: роль проверяется, нераспознанная роль не прячется, предусловие перепроверяется"
```

---

## Чего этот план НЕ делает, и почему

- **Мелочи, оставленные как есть** (из журнала прошлой ветки): `select()` кладёт в `self.selected` сырое имя до разрешения; `id("row-{i}")` по номеру; `main.subject.clone()` трижды; `recycle_session` зовёт `run_readiness` лишний раз; `ui.link.is_up()` в `main_button` отражает текущее соединение, а не открытую карточку; две формулировки «передай код на ту сторону». Ни одна не меняет того, что человек видит или получает, а правка каждой — это правка проверенного кода ради вкуса. `ui.link.is_up()` ждёт протокола: пока связь одна на машину, другого честного значения у него нет.
- **Протокольное** — код для передачи, диалог разрешения, «отклонено»/«отозвать разрешение», число зачисленных сторон, список «кто подключён». Этого в продукте нет вовсе, и окно про это по-прежнему говорит словами, а не рисует пустые органы.
