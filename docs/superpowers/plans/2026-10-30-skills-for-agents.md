# Скиллы агентам из комплекта — план работ

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** агент, которому в трее поставили галочку, получает вместе с записями MCP ещё и знание про 1С — скиллы из комплекта, в его собственный каталог скиллов.

**Architecture:** новый модуль `skills.rs` знает всё про скиллы: что лежит в комплекте, что мы положили человеку (отпечаток содержимого на каждый скилл, в состоянии машины), чем наше отличается от чужого (отпечаток есть — наше). Правило переносится из точки входа контейнера, где работает с 14.10, а не изобретается заново. Реестр агентов (`editors::AGENTS`) получает поле «какой каталог скиллов читает этот агент», а установка и снятие галочки делают теперь два дела вместо одного. Состояние галочки — пара `Found`: по записям MCP и по скиллам, сводится одним правилом.

**Tech Stack:** Rust, только уже имеющиеся зависимости. Суммы файлов — `common::integrity::sha256_file` (та же функция, что считает `checksums.txt` комплекта; `sha2` уже в `common`). Манифест — `serde_json`, он уже прямая зависимость коннектора. Новых крейтов задача не требует.

**Spec:** `docs/superpowers/specs/2026-10-30-skills-for-agents-design.md`

## Global Constraints

- Каталоги: `~/.agents/skills/` читают Zed, pi, Codex; `~/.claude/skills/` — Claude Code; на Windows `~` это `%USERPROFILE%` (замерено 30.10).
- Наш скилл узнаётся по ОТПЕЧАТКУ его содержимого, который мы записали, когда клали; папка без отпечатка чужая всегда, как бы она ни называлась. Приём и правило трёх случаев перенесены из `config/entrypoint.sh` — там они работают с 14.10, и второго механизма на ту же работу мы не заводим.
- Чужое не трогаем: ни чужой скилл, ни наш, изменённый человеком. Отказ говорится словами, в журнал.
- Чужая папка со скиллом НЕ мешает поставить записи MCP (решение владельца): руки важнее знания.
- Отпечатки живут в `ops_host::state_dir()`, не в комплекте: комплект заменяют целиком, и он исчезает ровно тогда, когда снятие обязано помнить, что клало.
- `skills/` в комплекте необязателен: нет папки — скиллы не ставятся, сказано словами, записи MCP работают.
- Комментарии и строки для человека — по-русски, абзац одной строкой; комментарий объясняет решение и чем оно вызвано.
- Фразу «не проверено» в факты набора вносить нельзя (тест `the_set_never_says_it_did_not_check`).

## Review Focus

- Папка с нашим именем, которую мы не клали (отпечатка нет): не перезаписать, не унести, сказать словами (задача 3).
- Каталог скиллов недоступен на запись или на месте скилла ссылка — отказ словами, а не паника и не половина установки (задачи 2-3).
- Человек удалил часть наших файлов руками: установка должна дополнить недостающее, снятие — унести остаток и не ругаться на отсутствующее (задача 3).
- Комплект сменился, а человек не менял тексты — обновление молча доставляет новое; менял — оставляет и называет (задача 6).
- Два агента с общим каталогом, галочки сняты по очереди — скиллы уходят ровно на второй (задача 4).

---

### Task 1: Скиллы в комплекте коннектора

**Files:**
- Modify: `pack.sh` (рядом с `mkdir -p "$CONNECT/mcp-1c"`)
- Modify: `setup/1c-connect/src/skills.rs` (создаётся задачей 2; сюда — одна константа)

**Interfaces:**
- Produces: каталог `skills/` в комплекте коннектора — ТОТ ЖЕ `config/skills/`, что уезжает в образ (`Dockerfile:631`); `skills::KIT_DIR: &str = "skills"`.

- [ ] **Step 1: Положить скиллы в комплект** — в `pack.sh`, рядом с `mcp-1c`: копировать `config/skills/` в `$CONNECT/skills/`. Своего набора не заводить и тексты не править: один источник на образ и на комплект, иначе наборы разъедутся молча. Отсутствие каталога — не отказ сборки (`skills/` необязателен, см. Global Constraints).

- [ ] **Step 2: Проверить** `bash -n pack.sh`; собрать комплект, если это дёшево, и убедиться, что `skills/` в нём есть и попал в `checksums.txt`.

- [ ] **Step 3: Commit**

---

### Task 2: Отпечаток и три случая — как в точке входа образа

**Files:**
- Create: `setup/1c-connect/src/skills.rs`
- Modify: `setup/1c-connect/src/lib.rs` (объявить модуль)

**Первоисточник:** `config/entrypoint.sh`, цикл по `/etc/skel/.agents/skills` (около строки 368) — там это правило работает с 14.10. Прочти его перед тем, как писать: мы переносим обкатанное, а не придумываем похожее.

**Interfaces:**
- Produces:
  - `pub enum Home { Agents, Claude }`, `pub fn home_dir(h: Home) -> Option<PathBuf>` (`~/.agents/skills`, `~/.claude/skills`; `~` — `USERPROFILE` на Windows, `HOME` иначе);
  - `pub fn dir_sum(dir: &Path) -> Result<String, String>` — отпечаток содержимого каталога скилла (имена файлов относительно него плюс суммы, `common::integrity::sha256_file`); порядок обхода обязан быть устойчивым, иначе отпечаток «плавает»;
  - `pub fn stamp_path(state: &Path, h: Home, skill: &str) -> PathBuf`, `read_stamp`, `write_stamp`;
  - `pub enum Verdict { New, Untouched, Edited, Link }` и `pub fn verdict(dst: &Path, stamp: Option<&str>) -> Result<Verdict, String>`.

- [ ] **Step 1: Тесты правила**

```rust
#[test]
fn the_three_cases_are_the_ones_the_container_uses() {
    // Нет каталога → New; есть и сумма совпала с отпечатком → Untouched;
    // есть и не совпала → Edited; на месте скилла ссылка → Link.
}

#[test]
fn the_fingerprint_follows_the_content_not_the_order_of_the_walk() {
    // Два одинаковых по содержимому каталога дают одну сумму;
    // правка одного байта в любом файле её меняет.
}
```

- [ ] **Step 2: Запустить — падают.**

- [ ] **Step 3: Реализовать** `home_dir`, `dir_sum`, отпечатки и `verdict`.

- [ ] **Step 4: Прогнать тесты.**

- [ ] **Step 5: Commit**

---

### Task 3: Установка, обновление и снятие

**Files:**
- Modify: `setup/1c-connect/src/skills.rs`

**Interfaces:**
- Consumes: `Home`, `dir_sum`, `verdict`, отпечатки (задача 2).
- Produces:
  - `pub fn look(kit: &Kit, state: &Path, h: Home) -> Result<editors::Found, String>`;
  - `pub fn install(kit: &Kit, state: &Path, h: Home) -> Result<editors::Found, Vec<String>>`;
  - `pub fn remove(kit: &Kit, state: &Path, h: Home) -> Result<editors::Found, Vec<String>>`.

- [ ] **Step 1: Тесты на временных каталогах** (`project::tempdir::Dir`; `home_dir` подменяется параметром, переменные среды в тестах не трогаем — этот урок уже стоил нам падающего теста)

```rust
#[test]
fn install_copies_the_kit_skills_and_writes_their_stamps() {}

#[test]
fn a_skill_we_never_put_there_is_left_alone() {
    // Папка с нашим именем, но без отпечатка: look → Foreign([имя]),
    // install её не перезаписывает, remove не уносит.
}

#[test]
fn an_edited_skill_survives_both_update_and_removal_and_is_named() {}

#[test]
fn a_new_kit_replaces_the_untouched_skills() {}

#[test]
fn a_symlink_in_place_of_a_skill_is_refused_in_words() {}

#[test]
fn a_half_deleted_install_is_completed_not_refused() {}
```

- [ ] **Step 2: Запустить — падают.**

- [ ] **Step 3: Реализовать** `look`, `install`, `remove` по трём случаям задачи 2. `install` служит и обновлением: разницы между «поставить» и «обновить» в правиле нет — ровно так и устроен контейнер.

- [ ] **Step 4: Прогнать тесты.**

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
    // человеком. refresh: неизменённые заменены новыми, изменённый на месте
    // и назван в строке журнала.
}

#[test]
fn the_first_run_after_the_release_delivers_skills_to_a_configured_agent() {
    // Записи MCP наши, скиллов нет, отпечатков нет: refresh кладёт скиллы и
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
