# Скиллы агентам из комплекта — план работ

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** агент, которому в трее поставили галочку, получает вместе с записями MCP ещё и знание про 1С — скиллы из комплекта, в его собственный каталог скиллов.

**Architecture:** новый модуль `skills.rs` знает всё про скиллы: что лежит в комплекте, что мы положили человеку (список с суммами в состоянии машины), чем наше отличается от чужого (метка во фронтматтере). Реестр агентов (`editors::AGENTS`) получает поле «какой каталог скиллов читает этот агент», а установка и снятие галочки делают теперь два дела вместо одного. Состояние галочки — пара `Found`: по записям MCP и по скиллам, сводится одним правилом.

**Tech Stack:** Rust, только уже имеющиеся зависимости. Суммы файлов — `common::integrity::sha256_file` (та же функция, что считает `checksums.txt` комплекта; `sha2` уже в `common`). Манифест — `serde_json`, он уже прямая зависимость коннектора. Новых крейтов задача не требует.

**Spec:** `docs/superpowers/specs/2026-10-30-skills-for-agents-design.md`

## Global Constraints

- Каталоги: `~/.agents/skills/` читают Zed, pi, Codex; `~/.claude/skills/` — Claude Code; на Windows `~` это `%USERPROFILE%` (замерено 30.10).
- Метка нашего скилла — `metadata.installed-by: 1c-connect` во фронтматтере `SKILL.md`; папка без метки чужая всегда, как бы она ни называлась.
- Чужое не трогаем: ни чужой скилл, ни наш, изменённый человеком. Отказ говорится словами, в журнал.
- Чужая папка со скиллом НЕ мешает поставить записи MCP (решение владельца): руки важнее знания.
- Список установленного живёт в `ops_host::state_dir()`, не в комплекте.
- `skills/` в комплекте необязателен: нет папки — скиллы не ставятся, сказано словами, записи MCP работают.
- Комментарии и строки для человека — по-русски, абзац одной строкой; комментарий объясняет решение и чем оно вызвано.
- Фразу «не проверено» в факты набора вносить нельзя (тест `the_set_never_says_it_did_not_check`).

## Review Focus

- `SKILL.md` без фронтматтера или с битым YAML в чужой папке — чтение не вправе падать и не вправе счесть папку нашей (задача 2).
- Каталог скиллов существует, но недоступен на запись (права, занятый файл) — отказ словами, а не паника и не половина установки (задача 3).
- Человек удалил часть наших файлов руками: установка должна дополнить недостающее, снятие — унести остаток и не ругаться на отсутствующее (задача 3).
- Комплект сменился, а пользователь не менял тексты — обновление молча доставляет новое; менял — оставляет и называет (задача 6).
- Два агента с общим каталогом, галочки сняты по очереди — скиллы уходят ровно на второй (задача 4).

---

### Task 1: Скиллы в комплекте

**Files:**
- Create: `config/skills/1c-mcp-basics/SKILL.md`
- Modify: `pack.sh` (рядом с `mkdir -p "$CONNECT/mcp-1c"`)
- Test: `setup/1c-connect/src/skills.rs` (тест формата читает файл репозитория)

**Interfaces:**
- Produces: каталог `skills/` в комплекте коннектора; `skills::KIT_DIR: &str = "skills"`.

- [ ] **Step 1: Написать скилл** `config/skills/1c-mcp-basics/SKILL.md` с фронтматтером `name: 1c-mcp-basics`, `description` (что умеет и когда звать: работа с базой 1С через инструменты `1c` и `vrunner`, выгрузка и загрузка конфигурации, запуск тестов) и `metadata: installed-by: 1c-connect`. Тело: наши два инструмента и что ими делают; долгие операции только в фоне (`background=true`, ответ приносит `taskId`, дальше `task_status`/`task_result`); открытое «Предприятие» на учебной платформе занимает единственный сеанс, и база ответит отказом; где лежат исходники проекта. До 500 строк, подробности — в `references/`, если понадобятся.

- [ ] **Step 2: Тест на формат**

```rust
#[test]
fn the_kit_skill_has_the_fields_the_spec_requires() {
    let text = std::fs::read_to_string("../../config/skills/1c-mcp-basics/SKILL.md").unwrap();
    let front = front_matter(&text).expect("фронтматтер обязателен");
    assert_eq!(front.get("name").map(String::as_str), Some("1c-mcp-basics"), "имя обязано совпадать с именем папки");
    assert!(front.get("description").is_some_and(|d| d.len() > 40), "описание решает, позовёт ли агент скилл вообще");
    assert_eq!(mark_of(&text).as_deref(), Some(MARK_VALUE), "без метки мы не узнаем свой же скилл");
}
```

- [ ] **Step 3: Запустить — падает** (`front_matter`/`mark_of` ещё нет; они появятся в задаче 2). Пока тест помечен `#[ignore]` с причиной; снять пометку в задаче 2.

- [ ] **Step 4: Положить скиллы в комплект** — в `pack.sh`, рядом с `mcp-1c`: копировать `config/skills/` в `$CONNECT/skills/`, если каталог существует. Отсутствие каталога — не отказ сборки (`skills/` необязателен).

- [ ] **Step 5: Проверить** `bash -n pack.sh`, и что в `pack.sh` путь назван один раз.

- [ ] **Step 6: Commit**

---

### Task 2: Чтение скилла — метка и фронтматтер

**Files:**
- Create: `setup/1c-connect/src/skills.rs`
- Modify: `setup/1c-connect/src/lib.rs` (объявить модуль)

**Interfaces:**
- Produces: `pub const MARK_KEY: &str = "installed-by"`, `pub const MARK_VALUE: &str = "1c-connect"`, `pub fn front_matter(text: &str) -> Option<BTreeMap<String, String>>`, `pub fn mark_of(text: &str) -> Option<String>`, `pub fn is_ours(skill_dir: &Path) -> Result<bool, String>`.

- [ ] **Step 1: Тесты разбора**

```rust
#[test]
fn a_folder_without_our_mark_is_never_ours() {
    assert_eq!(mark_of("---\nname: x\ndescription: y\n---\nтело"), None);
    assert_eq!(mark_of("---\nname: x\nmetadata:\n  installed-by: 1c-connect\n---\n").as_deref(), Some("1c-connect"));
    // Битый и отсутствующий фронтматтер — не наш скилл и не паника.
    assert_eq!(mark_of("нет никакого фронтматтера"), None);
    assert_eq!(mark_of("---\n: : :\n---\n"), None);
    assert!(front_matter("---\nname: x\n---\n").is_some());
}
```

- [ ] **Step 2: Запустить — падает** («cannot find function `mark_of`»).

- [ ] **Step 3: Реализовать** `front_matter` и `mark_of` в `skills.rs`. Разбор — свой, построчный, без новой зависимости: фронтматтер открывается и закрывается строкой `---`, нас интересуют только `name`, `description` и вложенный `metadata.installed-by`. Всё, что не разобралось, — `None`, а не отказ: чужой файл мы не судим.

- [ ] **Step 4: Реализовать** `is_ours(skill_dir)`: читает `<skill_dir>/SKILL.md`, отдаёт `Ok(true)` при нашей метке, `Ok(false)` без неё, `Err` — только если каталог есть, а файл не читается.

- [ ] **Step 5: Снять `#[ignore]`** с теста формата из задачи 1 и прогнать оба.

- [ ] **Step 6: Commit**

---

### Task 3: Установка и снятие в каталоге скиллов

**Files:**
- Modify: `setup/1c-connect/src/skills.rs`

**Interfaces:**
- Consumes: `is_ours`, `MARK_VALUE` (задача 2).
- Produces:
  - `pub enum Home { Agents, Claude }` и `pub fn home_dir(h: Home) -> Option<PathBuf>` (`~/.agents/skills`, `~/.claude/skills`; `~` — `USERPROFILE` на Windows, `HOME` иначе);
  - `pub struct Manifest { pub files: BTreeMap<String, String> }` — путь файла относительно каталога скиллов → сумма, какой МЫ её записали;
  - `pub fn manifest_path(state: &Path, h: Home) -> PathBuf`, `pub fn read_manifest(..) -> Manifest`, `pub fn write_manifest(..) -> Result<(), String>`;
  - `pub fn look(kit: &Kit, state: &Path, h: Home) -> Result<editors::Found, String>`;
  - `pub fn install(kit: &Kit, state: &Path, h: Home) -> Result<editors::Found, Vec<String>>`;
  - `pub fn remove(state: &Path, h: Home) -> Result<editors::Found, Vec<String>>`.

- [ ] **Step 1: Тесты на временных каталогах** (приём тот же, что у соседних тестов: `project::tempdir::Dir`)

```rust
#[test]
fn install_puts_the_kit_skills_and_remembers_what_it_wrote() { /* look → Absent; install → Ours; манифест знает каждый файл */ }

#[test]
fn a_foreign_folder_with_our_name_is_foreign_and_survives() {
    // Папка `1c-mcp-basics` без нашей метки: look → Foreign(["1c-mcp-basics"]),
    // install отказывает и НЕ трогает её текст, remove её не уносит.
}

#[test]
fn removal_keeps_what_the_person_changed_and_says_so() {
    // install; человек дописал строку в SKILL.md; remove → файл на месте,
    // в отказе названо его имя, остальные наши файлы убраны.
}

#[test]
fn a_half_deleted_install_is_completed_not_refused() {
    // install; удалить один наш файл руками; look → Absent; install → Ours.
}

#[test]
fn an_unwritable_home_is_a_refusal_in_words() {
    // home_dir указывает на файл, а не каталог: install → Err со словами, без паники.
}
```

- [ ] **Step 2: Запустить — падают.**

- [ ] **Step 3: Реализовать** `home_dir`, манифест (формат — JSON через `serde_json`; имя файла `skills-agents.json` / `skills-claude.json`; суммы — `common::integrity::sha256_file`), `look`, `install`, `remove` по правилам спеки: наш файл = совпал с суммой в манифесте; чужая папка = нет метки; `install` дополняет недостающее и не трогает чужое; `remove` уносит только совпавшее, остальное называет в отказе.

- [ ] **Step 4: Прогнать тесты — зелено.**

- [ ] **Step 5: Commit**

---

### Task 4: Реестр агентов знает про скиллы

**Files:**
- Modify: `setup/1c-connect/src/editors.rs` (`AgentDef`, `AGENTS`, `Setup::enable`/`disable` не трогаем — склейка выше)
- Modify: `setup/1c-connect/src/skills.rs`

**Interfaces:**
- Consumes: `skills::{Home, look, install, remove}`.
- Produces:
  - поле `pub skills: skills::Home` в `AgentDef` (`Zed`, `pi`, `Codex` → `Home::Agents`; `Claude Code` → `Home::Claude`);
  - `pub fn both(mcp: &Result<Found, String>, skills: &Result<Found, String>) -> Result<Found, String>` в `editors.rs`;
  - `pub fn others_hold(home: Home, exe: &Path) -> bool` — есть ли у ДРУГИХ агентов того же каталога свои записи MCP.

- [ ] **Step 1: Тесты правил склейки и счёта**

```rust
#[test]
fn the_checkbox_is_on_only_when_both_halves_are_ours() {
    // Ours+Ours → Ours; Ours+Absent → Absent; Absent+Ours → Absent;
    // Foreign в любой половине → Foreign с её именами;
    // Missing агента перекрывает всё → Missing;
    // Err в любой половине → Err (нечитаемое состояние не выдаём за «нет»).
}

#[test]
fn the_shared_folder_waits_for_the_last_agent() {
    // Zed и Codex делят `~/.agents/skills`: снятие у Zed, пока записи MCP
    // Codex наши, скиллы не уносит; после снятия у обоих — уносит.
    // Счёт идёт по записям MCP соседей: скиллы у них общие, и спрашивать
    // про них значило бы спрашивать себя.
}
```

- [ ] **Step 2: Запустить — падают.**

- [ ] **Step 3: Реализовать** `both` и `others_hold`; добавить поле в `AgentDef` и заполнить его в `AGENTS`.

- [ ] **Step 4: Прогнать тесты.**

- [ ] **Step 5: Commit**

---

### Task 5: Щелчок галочки ставит и то, и другое

**Files:**
- Modify: `setup/1c-connect/src/gui/mod.rs` (`agent_work`, `setup_line`)
- Modify: `setup/1c-connect/src/gui/tray.rs` (`set_agents` — состояние галочки)
- Modify: `setup/1c-connect/src/verify.rs` (`local_editor_mcp_fact` и его вызывающий)

**Interfaces:**
- Consumes: `editors::both`, `skills::{install, remove, look}`, `editors::AgentDef::skills`.

- [ ] **Step 1: Тесты слов и состояния**

```rust
#[test]
fn a_refused_skill_does_not_cost_the_agent_its_mcp_entries() {
    // Записи MCP поставлены, скилл отказал (чужая папка): строка журнала
    // называет ОБА исхода, галочка снята, и в тексте есть имя чужой папки.
}

#[test]
fn the_card_counts_an_agent_configured_only_with_its_skills() {
    // local_editor_mcp_fact: агент с записями, но без скиллов — не «настроен».
}
```

- [ ] **Step 2: Запустить — падают.**

- [ ] **Step 3: Провести скиллы через щелчок**: `agent_work` ставит записи MCP, затем скиллы (`install`/`remove` по `def.skills`), и отдаёт человеку одну строку про оба исхода — словами, которые вернули обе половины. Отказ скилла не отменяет записей MCP.

- [ ] **Step 4: Состояние галочки** — `both(...)` вместо одного `look`; тот же `Found` уходит и в факт набора.

- [ ] **Step 5: Прогнать весь набор пакета.**

- [ ] **Step 6: Commit**

---

### Task 6: Обновление и тихая доставка при запуске

**Files:**
- Modify: `setup/1c-connect/src/skills.rs`
- Modify: `setup/1c-connect/src/gui/mod.rs` (там, где окно просыпается и считает первый набор)

**Interfaces:**
- Produces: `pub fn refresh(kit: &Kit, state: &Path, h: Home) -> Vec<Line>` — доставляет недостающее и обновляет устаревшее; отдаёт строки для журнала (пусто — говорить нечего).

- [ ] **Step 1: Тесты**

```rust
#[test]
fn a_new_kit_updates_the_untouched_files_and_leaves_the_changed_one() {
    // install; подменить текст в комплекте (новая версия); один файл изменён
    // человеком. refresh: неизменённые переписаны новым, изменённый на месте
    // и назван в строке журнала.
}

#[test]
fn the_first_run_after_the_release_delivers_skills_to_a_configured_agent() {
    // Записи MCP наши, скиллов нет, манифеста нет: refresh кладёт скиллы и
    // говорит об этом строкой (решение владельца — молча доставить тем, кто
    // галочкой уже разрешил).
}

#[test]
fn refresh_touches_nothing_when_the_kit_has_no_skills_folder() {}
```

- [ ] **Step 2: Запустить — падают.**

- [ ] **Step 3: Реализовать `refresh`** и позвать её при запуске окна для каждого агента, у которого записи MCP наши.

- [ ] **Step 4: Прогнать набор.**

- [ ] **Step 5: Commit**

---

### Task 7: Перечень живой проверки

**Files:**
- Modify: `config/kit/УСТАНОВКА.md` (раздел про агентов)
- Create: `docs/superpowers/plans/2026-10-30-skills-handoff.md`

- [ ] **Step 1: Выписать перечень** того, что проверяется только на машине со всеми четырьмя агентами: видит ли каждый наш скилл своим способом показать список (`/skills` у Codex, `@`-упоминание у Zed, список у pi, `/` у Claude Code); подхватывается ли он без перезапуска агента; что видит человек, если папка занята чужим скиллом; что остаётся после снятия галочки у одного из трёх, делящих каталог.

- [ ] **Step 2: Сказать прямо**, что задача не может сама объявить себя проверенной: другой машины с полным набором агентов у проекта нет.

- [ ] **Step 3: Commit**
