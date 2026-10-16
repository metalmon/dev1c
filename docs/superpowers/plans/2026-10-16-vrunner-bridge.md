# vanessa-runner: мост и комплект (этап 6A) — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `1c-connect` отвечает на порту 778N процессом `vrunner-mcp` из комплекта: фильтрует его инструменты, добавляет три своих (git), держит дерево процессов в Job Object; комплект несёт OneScript, vanessa-runner и MinGit; туннель поднимается под учёткой рабочего места.

**Architecture:** Чистая часть моста — модуль `jsonrpc.rs` (рамка «строка на сообщение» с пределом 8 МиБ, маршрутизация запросов, переписывание ответа `tools/list`, сборка ответов MCP) с тестами на байтах и JSON; `bridge::pipe_jsonrpc` — асинхронная обёртка: процесс, один писатель за `mpsc`, свои инструменты отдельными задачами с отменой. Git-инструменты — модуль `project.rs` (один набор флагов git, мьютекс проекта, тесты на настоящем git с голым origin). Windows Job Object — `job.rs`. Операция `host.tools` копирует инструменты комплекта в `%LOCALAPPDATA%\dev1c`. `pack.sh` раскладывает `vrunner/oscript`, `vrunner/lib`, `git` по приколоченным хешам и манифесту. Образ: три учётки туннеля вместо `dev`.

**Tech Stack:** Rust 2021, `tokio` (уже: rt, process, io-util, sync), `serde_json`, `russh` 0.64, `windows-sys` 0.59 (новые features `Win32_System_JobObjects`, `Win32_System_Threading`), каркас `common` (`Op<Ctx>`, `OpState`, `docker::{exec,probe}`, `Kit`, `Check`); `sh` в `pack.sh`, `opm` в контейнере `evilbeaver/onescript:2.2.0`; CI GitHub Actions (`.github/workflows/build.yml`).

**Spec:** `docs/superpowers/specs/2026-10-16-setup-binaries-design.md`, раздел «vanessa-runner в комплекте разработчика (этап 6)», абзац «Два плана» (6A). Раздел «Что меняется в образе» и «Комплекты» там же.

## Global Constraints

- Язык сообщений, комментариев и документации — русский; идентификаторы — английские (как во всём `setup/`). Ответы агенту (`content[0].text`) — русские, с подсказкой, что делать.
- Порт 778N — **не** `pipe_process`: мост разбирает поток построчно; строки не похожие на JSON (без ведущего `{`) и байты сверх предела 8 МиБ идут как есть; в канал пишет один писатель.
- `tools/list` фильтруется по `[host] tools`; три инструмента моста (`project_status`, `project_pull`, `project_push`) добавляются только на странице без `nextCursor` и отдаются всегда, даже если ребёнок не ответил до выхода.
- `tools/call` с именем не из списка и не моста — `isError` «инструмент … выключен в 1c-connect.toml»; свои инструменты — отдельной задачей; `notifications/cancelled` на id моста снимает задачу.
- Один проект — один процесс за раз: `try_lock` мьютекса проекта, иначе «занято: идёт <что>»; `cf_load` и `infobase_update` отвергаются, пока запущен `1cv8.exe`.
- Все вызовы git — один набор флагов: `-c user.name=dev<N> -c user.email=dev<N>@dev1c.local -c safe.directory=* -c core.longpaths=true -c core.quotepath=false -c credential.helper= -c http.extraHeader=Authorization: Basic <base64 devN:devN>`; `GIT_TERMINAL_PROMPT=0`; пароля в URL remote нет.
- Затирка одна на всё, что уходит агенту и в журнал: `Basic <…>`, `/P<…>`, `db-pwd=…` → многоточие (в `common::docker::redact_text`).
- На Windows дерево `oscript.exe` → `1cv8.exe` живёт в Job Object с `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; `CREATE_NO_WINDOW`.
- Инструменты запускаются из `%LOCALAPPDATA%\dev1c\{vrunner,git}` (на Linux — `~/.local/share/dev1c`), не с носителя комплекта; операция `host.tools` копирует их и ставит отметку `tools.version`.
- Туннель — под `workstation` с `[host] password`; sshd — `AllowUsers dev1 dev2 dev3`, у каждого `Match User` с `ForceCommand /bin/false` и `PermitListen` трёх своих портов; учётка `dev` в образе исчезает.
- Архивы `pack.sh` — по SHA-256; дерево `vrunner/lib` — по манифесту `setup/1c-connect/vendor/vrunner-lib.sha256`; ассеты `mcp-1c` тоже по хешам. Линукс-сборка (`--no-default-features`) и тесты обязаны проходить без Windows и без Gitea.
- Коммит на задачу; `cargo test -p onec-connect` зелёный после каждой; прямо в `main` (решение заказчика), CI через Monitor; сборки — в CI.

## Review Focus

1. **Ответ `tools/list` ребёнка пришёл одной строкой больше 8 МиБ** — мост обязан пропустить байты как есть, не упасть и не зависнуть; агент получит список без инструментов моста, но живой. Тест `oversized_line_passes_through_unparsed` в Задаче 2.
2. **Zed открыл второе соединение на 778N, пока первое исполняет `project_push`** — второе получает «занято: идёт project_push», а не блокируется и не `index.lock`. Тест `second_caller_is_told_busy_not_queued` в Задаче 3.
3. **`notifications/cancelled` на чужой id** (запрос самого vrunner) — уходит ребёнку как есть, мост ничего не снимает. Тест `cancel_of_a_foreign_id_is_forwarded` в Задаче 2.
4. **Пароль базы с `"` или кириллицей в `VRUNNER_DBPWD`** — уходит процессу переменной окружения без экранирования (не через командную строку); тест `connection_env_is_passed_verbatim` в Задаче 6.
5. **Ребёнок умер, не ответив на `tools/list`** — агент всё равно получает три инструмента моста ответом на тот же id, а затем канал закрывается. Тест `child_death_still_answers_pending_list_with_our_tools` в Задаче 5.

---

### Task 1: Параметры проекта (`cfg.rs`, `1c-connect.example.toml`)

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs`
- Modify: `setup/1c-connect/1c-connect.example.toml`
- Modify: `setup/1c-connect/ui/app.js:7` (список `KEYS` — добавить `project`; поле в `index.html` появится в 6B, поэтому здесь только `KEYS` не трогаем — см. шаг 5)

**Interfaces:**
- Produces: `Host { project: String, tools: Vec<String>, … }`; `Host::default().password == "dev1"`; `Cfg::gitea_url(&self) -> String` (`http://<host>:<ssh − 2>`); `Cfg::dump_dir(&self) -> String` (пусто → `<project>\src`); `Cfg::number(&self) -> Option<u16>` становится `pub`; `pub const DEFAULT_TOOLS: [&str; 12]`.

- [ ] **Step 1: Написать падающие тесты**

В `mod tests` файла `setup/1c-connect/src/cfg.rs` добавить:

```rust
    #[test]
    fn gitea_sits_two_ports_below_ssh() {
        let mut c = sample();
        assert_eq!(c.gitea_url(), "http://192.168.1.10:3391");
        c.host.server = "10.0.0.5:4403".into();
        assert_eq!(c.gitea_url(), "http://10.0.0.5:4401");
    }

    #[test]
    fn dump_defaults_to_src_under_the_project() {
        let mut c = sample();
        c.host.project = "C:\\proj\\Buh".into();
        assert_eq!(c.dump_dir(), "C:\\proj\\Buh\\src");
        c.host.dump = "D:\\dump".into();
        assert_eq!(c.dump_dir(), "D:\\dump");
    }

    #[test]
    fn project_must_be_a_full_path_without_quotes_or_percent() {
        let mut c = sample();
        c.host.project = "proj".into();
        assert!(c.validate().unwrap_err().to_string().contains("project"));
        c.host.project = "C:\\proj\\\"x\"".into();
        assert!(c.validate().unwrap_err().to_string().contains("project"));
        c.host.project = "C:\\proj\\Buh".into();
        assert!(c.validate().is_ok());
        c.host.project = "/home/dev/proj".into();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn tools_default_to_the_working_set() {
        let c = sample();
        assert_eq!(c.host.tools, DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(c.host.tools.contains(&"cf_load".to_string()) && !c.host.tools.contains(&"xunit".to_string()));
    }

    #[test]
    fn password_is_the_workstation_login_by_default() {
        assert_eq!(Host::default().password, "dev1");
    }
```

- [ ] **Step 2: Запустить — убедиться, что падают на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features cfg::`
Expected: ошибка компиляции `no field project`, `no method gitea_url`.

- [ ] **Step 3: Реализовать**

В `setup/1c-connect/src/cfg.rs`:

```rust
/// Инструменты vrunner, видимые агенту по умолчанию: выгрузка/загрузка,
/// синтакс-контроль, тесты, фоновые задачи, кэш. Остальные 49 — по списку
/// в toml.
pub const DEFAULT_TOOLS: [&str; 12] = [
    "cf_decompile", "cf_load", "infobase_update", "validate_syntax_check", "test_xunit", "test_vanessa",
    "test_yaxunit", "task_status", "task_result", "task_cancel", "cache_show", "cache_clear",
];
```

В `Host` после `dump`: `pub project: String, pub tools: Vec<String>,`. В `Default for Host`: `password: "dev1".into()`, `project: "C:\\proj\\Buh".into()`, `tools: DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect()`.

В `impl Cfg`: `fn number` → `pub fn number`. Добавить:

```rust
    /// Gitea среды: на два порта ниже SSH (блок из четырёх подряд у dev1c-setup).
    pub fn gitea_url(&self) -> String {
        let (host, port) = self.host.server.rsplit_once(':').unwrap_or((&self.host.server, "3393"));
        let port: u32 = port.parse().unwrap_or(3393);
        format!("http://{host}:{}", port.saturating_sub(2))
    }

    /// Каталог выгрузки для поиска mcp-1c: `dump`, а пусто — `<project>/src`.
    pub fn dump_dir(&self) -> String {
        if self.host.dump.is_empty() {
            format!("{}{}src", self.host.project.trim_end_matches(['\\', '/']), std::path::MAIN_SEPARATOR)
        } else {
            self.host.dump.clone()
        }
    }
```

В `validate()` перед `Ok(())`:

```rust
        let p = &self.host.project;
        let full = p.len() > 2 && (p.as_bytes()[1] == b':' || p.starts_with('/') || p.starts_with("\\\\"));
        anyhow::ensure!(full && !p.contains('"') && !p.contains('%'), "[host]: project — полный путь к каталогу проекта без кавычек и %, например C:\\proj\\Buh");
```

(Тест ждёт платформонезависимого результата: `C:\proj\Buh` и `/home/dev/proj` проходят на обеих ОС — проверка по форме строки, не по файловой системе.)

В `render()` добавить ветки `"project" => format!("{:?}", h.project)` и `"tools" => format!("[{}]", h.tools.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>().join(", "))`.

- [ ] **Step 4: Образец**

`setup/1c-connect/1c-connect.example.toml` — заменить строки 7–8 и добавить после `dump`:

```toml
# Пароль вашего рабочего места в среде (в пилоте равен логину).
password = "dev1"
```

```toml
# Каталог проекта на этой машине: сюда ложатся исходники (src), здесь живёт
# git. Создаётся при install. Не каталог базы.
project = "C:\\proj\\Buh"
# Инструменты vrunner, которые видит агент. Полный список — vrunner-mcp,
# 61 инструмент; здесь рабочий набор.
tools = ["cf_decompile", "cf_load", "infobase_update", "validate_syntax_check", "test_xunit", "test_vanessa", "test_yaxunit", "task_status", "task_result", "task_cancel", "cache_show", "cache_clear"]
```

Комментарий у `dump` заменить на: `# Каталог выгрузки для поиска по коду. Пусто — <project>\src.`

- [ ] **Step 5: Окно пока не знает `project`**

`gui.rs` команда `save_cfg` принимает `Host` целиком из JS (`KEYS`); поле `project` без элемента в форме пришло бы пустым и затёрло бы значение. Проверить `grep -n 'save_cfg' setup/1c-connect/src/gui.rs`: если `save_cfg` строит `Host` из структуры с `#[serde(default)]`, пустое `project` затирает. Решение здесь: в `save_cfg` брать `project` и `tools` из текущего `cfg` (`let mut h = host; h.project = cur.host.project.clone(); h.tools = cur.host.tools.clone();`) — вкладка появится в 6B и снимет это. Записать в ledger как `Ruling`.

- [ ] **Step 6: Запустить тесты**

Run: `cd setup && cargo test -p onec-connect --no-default-features`
Expected: все зелёные, включая `example_parses_and_equals_defaults` и `render_keeps_comments_and_round_trips`.

- [ ] **Step 7: Коммит**

```bash
git add setup/1c-connect/src/cfg.rs setup/1c-connect/1c-connect.example.toml setup/1c-connect/src/gui.rs
git commit -m "1c-connect: [host] project and tools, gitea_url, dump defaults to <project>/src, password is the workstation's"
```

---

### Task 2: Рамка и маршрутизация JSON-RPC (`jsonrpc.rs`)

**Files:**
- Create: `setup/1c-connect/src/jsonrpc.rs`
- Modify: `setup/1c-connect/src/lib.rs` (добавить `pub mod jsonrpc;`)

**Interfaces:**
- Produces:
  - `pub const LINE_LIMIT: usize = 8 << 20;`
  - `pub struct Framer` с `Framer::new()`, `fn push(&mut self, chunk: &[u8]) -> Vec<Frame>`, `fn finish(&mut self) -> Option<Frame>`;
  - `pub enum Frame { Line(Vec<u8>), Raw(Vec<u8>) }` — `Line` без завершающего `\n`, `Raw` — байты как есть (сверх предела, с `\n` внутри как пришли);
  - `pub enum Route { Forward, ListRequest { id: Value }, OurCall { id: Value, tool: String, args: Value }, Disabled { id: Value, tool: String }, PlatformCall { id: Value, tool: String }, Cancel { id: Value } }`;
  - `pub fn route_request(line: &[u8], allowed: &[String]) -> Route`;
  - `pub fn rewrite_list_reply(line: &[u8], allowed: &[String]) -> Option<(Value, Vec<u8>)>` — `Some((id, новая строка))`, если строка — ответ с `result.tools`;
  - `pub fn our_tools() -> Vec<Value>`; `pub const OUR_TOOLS: [&str; 3] = ["project_status", "project_pull", "project_push"]`; `pub const PLATFORM_TOOLS: [&str; 2] = ["cf_load", "infobase_update"]`;
  - `pub fn text_reply(id: &Value, text: &str, is_error: bool) -> Vec<u8>` — строка с `\n` на конце;
  - `pub fn list_reply(id: &Value, tools: Vec<Value>) -> Vec<u8>`;
  - `pub fn is_response_with_id(line: &[u8]) -> Option<Value>`.

- [ ] **Step 1: Падающие тесты рамки**

Создать `setup/1c-connect/src/jsonrpc.rs` с заголовком и тестами (реализация — в шаге 3):

```rust
//! Чистая часть моста 778N: рамка «строка на сообщение» и правила, что
//! уходит ребёнку, что отвечает мост сам и как переписывается `tools/list`.
//! Байты и JSON, никаких процессов — всё проверяется тестами на строках.

use serde_json::{json, Value};

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(frames: Vec<Frame>) -> Vec<String> {
        frames
            .into_iter()
            .map(|f| match f {
                Frame::Line(b) => format!("L:{}", String::from_utf8_lossy(&b)),
                Frame::Raw(b) => format!("R:{}", String::from_utf8_lossy(&b)),
            })
            .collect()
    }

    #[test]
    fn a_line_split_across_reads_is_assembled() {
        let mut f = Framer::new();
        assert!(f.push(b"{\"a\":").is_empty());
        assert_eq!(lines(f.push(b"1}\n")), vec!["L:{\"a\":1}"]);
    }

    #[test]
    fn several_lines_in_one_read_come_out_separately() {
        let mut f = Framer::new();
        assert_eq!(lines(f.push(b"{\"a\":1}\n{\"b\":2}\nrest")), vec!["L:{\"a\":1}", "L:{\"b\":2}"]);
        assert_eq!(lines(f.finish().into_iter().collect()), vec!["L:rest"]);
    }

    #[test]
    fn crlf_is_stripped_like_lf() {
        let mut f = Framer::new();
        assert_eq!(lines(f.push(b"{\"a\":1}\r\n")), vec!["L:{\"a\":1}"]);
    }

    #[test]
    fn oversized_line_passes_through_unparsed() {
        // A validate_syntax_check report on ERP is megabytes; above the limit
        // the bridge stops buffering and streams the bytes as they come.
        let mut f = Framer::new();
        let big = vec![b'x'; LINE_LIMIT + 10];
        let out = f.push(&big);
        assert!(matches!(out.as_slice(), [Frame::Raw(b)] if b.len() == LINE_LIMIT + 10), "{}", out.len());
        assert_eq!(lines(f.push(b"tail\n")), vec!["R:tail\n"]);
        // After the newline the framer is back to lines.
        assert_eq!(lines(f.push(b"{\"ok\":1}\n")), vec!["L:{\"ok\":1}"]);
    }
}
```

- [ ] **Step 2: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features jsonrpc::`
Expected: `cannot find type Framer`.

- [ ] **Step 3: Реализовать рамку**

Над `mod tests`:

```rust
/// Предел одной строки: больше — не сообщение, а поток, и он идёт как есть.
pub const LINE_LIMIT: usize = 8 << 20;

pub enum Frame {
    /// Целая строка без `\n`/`\r\n` — кандидат на разбор.
    Line(Vec<u8>),
    /// Байты сверх предела — отдаются дальше без разбора, до конца строки.
    Raw(Vec<u8>),
}

#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
    /// Текущая строка переросла предел: остаток до `\n` идёт как `Raw`.
    overflow: bool,
}

impl Framer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        let mut out = Vec::new();
        let mut rest = chunk;
        while !rest.is_empty() {
            if self.overflow {
                match rest.iter().position(|&b| b == b'\n') {
                    Some(i) => {
                        out.push(Frame::Raw(rest[..=i].to_vec()));
                        rest = &rest[i + 1..];
                        self.overflow = false;
                    }
                    None => {
                        out.push(Frame::Raw(rest.to_vec()));
                        rest = &[];
                    }
                }
                continue;
            }
            match rest.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    self.buf.extend_from_slice(&rest[..i]);
                    if self.buf.last() == Some(&b'\r') {
                        self.buf.pop();
                    }
                    out.push(Frame::Line(std::mem::take(&mut self.buf)));
                    rest = &rest[i + 1..];
                }
                None => {
                    self.buf.extend_from_slice(rest);
                    rest = &[];
                }
            }
            if self.buf.len() > LINE_LIMIT {
                out.push(Frame::Raw(std::mem::take(&mut self.buf)));
                self.overflow = true;
            }
        }
        out
    }

    /// Конец потока: что осталось без `\n` — тоже строка.
    pub fn finish(&mut self) -> Option<Frame> {
        if self.buf.is_empty() {
            None
        } else if self.overflow {
            self.overflow = false;
            Some(Frame::Raw(std::mem::take(&mut self.buf)))
        } else {
            Some(Frame::Line(std::mem::take(&mut self.buf)))
        }
    }
}
```

- [ ] **Step 4: Запустить тесты рамки**

Run: `cd setup && cargo test -p onec-connect --no-default-features jsonrpc::`
Expected: 4 passed.

- [ ] **Step 5: Падающие тесты маршрутизации**

Добавить в `mod tests`:

```rust
    fn allowed() -> Vec<String> {
        ["cf_load", "task_status"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn list_request_is_remembered_and_forwarded() {
        let r = route_request(br#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#, &allowed());
        assert!(matches!(r, Route::ListRequest { id } if id == json!(7)));
    }

    #[test]
    fn allowed_call_goes_to_the_child_as_is() {
        let r = route_request(br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"task_status","arguments":{}}}"#, &allowed());
        assert!(matches!(r, Route::Forward));
    }

    #[test]
    fn platform_calls_are_flagged_for_the_configurator_check() {
        let r = route_request(br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#, &allowed());
        assert!(matches!(r, Route::PlatformCall { tool, .. } if tool == "cf_load"));
    }

    #[test]
    fn disabled_tool_is_answered_by_the_bridge() {
        let r = route_request(br#"{"jsonrpc":"2.0","id":"a","method":"tools/call","params":{"name":"infobase_delete","arguments":{}}}"#, &allowed());
        assert!(matches!(r, Route::Disabled { id, tool } if id == json!("a") && tool == "infobase_delete"));
    }

    #[test]
    fn our_tool_is_ours_even_when_not_in_the_list() {
        let r = route_request(br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_push","arguments":{"message":"m"}}}"#, &allowed());
        assert!(matches!(r, Route::OurCall { id, tool, args } if id == json!(2) && tool == "project_push" && args["message"] == "m"));
    }

    #[test]
    fn cancel_carries_the_request_id() {
        let r = route_request(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}"#, &allowed());
        assert!(matches!(r, Route::Cancel { id } if id == json!(2)));
    }

    #[test]
    fn cancel_of_a_foreign_id_is_forwarded() {
        // Route only says "cancel of id 9"; whether 9 is ours is the bridge's
        // table — the bridge forwards what it does not own (tested in bridge.rs).
        let r = route_request(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9}}"#, &allowed());
        assert!(matches!(r, Route::Cancel { id } if id == json!(9)));
    }

    #[test]
    fn non_json_and_other_methods_are_forwarded() {
        assert!(matches!(route_request(b"warning: something", &allowed()), Route::Forward));
        assert!(matches!(route_request(br#"{"jsonrpc":"2.0","id":3,"method":"initialize","params":{}}"#, &allowed()), Route::Forward));
        assert!(matches!(route_request(br#"{"jsonrpc":"2.0","id":3,"method":"tools/call"}"#, &allowed()), Route::Forward));
    }

    #[test]
    fn list_reply_is_filtered_and_ours_are_appended_on_the_last_page() {
        let line = br#"{"jsonrpc":"2.0","id":7,"result":{"tools":[{"name":"cf_load"},{"name":"infobase_delete"},{"name":"task_status"}]}}"#;
        let (id, out) = rewrite_list_reply(line, &allowed()).unwrap();
        assert_eq!(id, json!(7));
        let v: Value = serde_json::from_slice(&out).unwrap();
        let names: Vec<&str> = v["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["cf_load", "task_status", "project_status", "project_pull", "project_push"]);
        assert_eq!(out.last(), Some(&b'\n'));
    }

    #[test]
    fn a_page_with_next_cursor_is_filtered_but_not_extended() {
        let line = br#"{"jsonrpc":"2.0","id":7,"result":{"tools":[{"name":"x"},{"name":"cf_load"}],"nextCursor":"p2"}}"#;
        let (_, out) = rewrite_list_reply(line, &allowed()).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(v["result"]["nextCursor"], "p2");
    }

    #[test]
    fn replies_without_tools_are_not_list_replies() {
        assert!(rewrite_list_reply(br#"{"jsonrpc":"2.0","id":7,"result":{"ok":1}}"#, &allowed()).is_none());
        assert!(rewrite_list_reply(b"not json", &allowed()).is_none());
        assert_eq!(is_response_with_id(br#"{"jsonrpc":"2.0","id":7,"result":{"ok":1}}"#), Some(json!(7)));
        assert_eq!(is_response_with_id(br#"{"jsonrpc":"2.0","id":7,"method":"x"}"#), None);
    }

    #[test]
    fn our_tools_describe_their_arguments() {
        let t = our_tools();
        assert_eq!(t.len(), 3);
        assert_eq!(t[2]["name"], "project_push");
        assert_eq!(t[2]["inputSchema"]["required"][0], "message");
        assert!(t[0]["description"].as_str().unwrap().contains("ветк"));
    }

    #[test]
    fn text_reply_is_mcp_shaped() {
        let v: Value = serde_json::from_slice(&text_reply(&json!(5), "занято", true)).unwrap();
        assert_eq!(v["id"], 5);
        assert_eq!(v["result"]["isError"], true);
        assert_eq!(v["result"]["content"][0]["text"], "занято");
        let v: Value = serde_json::from_slice(&list_reply(&json!(1), our_tools())).unwrap();
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 3);
    }
```

- [ ] **Step 6: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features jsonrpc::`
Expected: `cannot find function route_request`.

- [ ] **Step 7: Реализовать маршрутизацию**

Между рамкой и `mod tests`:

```rust
pub const OUR_TOOLS: [&str; 3] = ["project_status", "project_pull", "project_push"];
/// Вызовы, которым мешает открытый Конфигуратор (потеря его работы или отказ платформы).
pub const PLATFORM_TOOLS: [&str; 2] = ["cf_load", "infobase_update"];

pub enum Route {
    Forward,
    ListRequest { id: Value },
    OurCall { id: Value, tool: String, args: Value },
    Disabled { id: Value, tool: String },
    PlatformCall { id: Value, tool: String },
    Cancel { id: Value },
}

fn parse(line: &[u8]) -> Option<Value> {
    if line.first() != Some(&b'{') {
        return None;
    }
    serde_json::from_slice(line).ok()
}

/// Что делать со строкой от агента.
pub fn route_request(line: &[u8], allowed: &[String]) -> Route {
    let Some(v) = parse(line) else { return Route::Forward };
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    match v.get("method").and_then(Value::as_str) {
        Some("tools/list") => Route::ListRequest { id },
        Some("tools/call") => {
            let Some(tool) = v.pointer("/params/name").and_then(Value::as_str) else { return Route::Forward };
            let tool = tool.to_string();
            if OUR_TOOLS.contains(&tool.as_str()) {
                let args = v.pointer("/params/arguments").cloned().unwrap_or(json!({}));
                Route::OurCall { id, tool, args }
            } else if !allowed.iter().any(|a| a == &tool) {
                Route::Disabled { id, tool }
            } else if PLATFORM_TOOLS.contains(&tool.as_str()) {
                Route::PlatformCall { id, tool }
            } else {
                Route::Forward
            }
        }
        Some("notifications/cancelled") => match v.pointer("/params/requestId") {
            Some(id) => Route::Cancel { id: id.clone() },
            None => Route::Forward,
        },
        _ => Route::Forward,
    }
}

/// Ответ ребёнка на `tools/list`: чужие инструменты вырезаются, на последней
/// странице добавляются наши. `None` — это не такой ответ.
pub fn rewrite_list_reply(line: &[u8], allowed: &[String]) -> Option<(Value, Vec<u8>)> {
    let mut v = parse(line)?;
    let id = v.get("id")?.clone();
    let tools = v.pointer_mut("/result/tools")?.as_array_mut()?;
    tools.retain(|t| t.get("name").and_then(Value::as_str).map_or(false, |n| allowed.iter().any(|a| a == n)));
    if v.pointer("/result/nextCursor").is_none() {
        v.pointer_mut("/result/tools")?.as_array_mut()?.extend(our_tools());
    }
    let mut out = serde_json::to_vec(&v).ok()?;
    out.push(b'\n');
    Some((id, out))
}

/// id ответа (есть `id`, нет `method`), чтобы снять его со списка ожидания.
pub fn is_response_with_id(line: &[u8]) -> Option<Value> {
    let v = parse(line)?;
    if v.get("method").is_some() {
        return None;
    }
    v.get("id").cloned()
}

pub fn our_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "project_status",
            "description": "Состояние проекта: ветка, последний коммит, неотправленные коммиты, изменённые файлы, есть ли ConfigDumpInfo.xml (индекс инкремента).",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true}
        }),
        json!({
            "name": "project_pull",
            "description": "Забрать свою ветку из Gitea (git pull --rebase). Только на чистом дереве: сначала project_push. После pull — cf_load.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "project_push",
            "description": "Закоммитить src и отправить свою ветку в Gitea. Перед этим — cf_decompile. Первый push создаёт репозиторий и ветку main.",
            "inputSchema": {"type": "object", "properties": {"message": {"type": "string", "description": "Сообщение коммита"}}, "required": ["message"]}
        }),
    ]
}

pub fn text_reply(id: &Value, text: &str, is_error: bool) -> Vec<u8> {
    let v = json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": text}], "isError": is_error}});
    let mut out = serde_json::to_vec(&v).unwrap();
    out.push(b'\n');
    out
}

pub fn list_reply(id: &Value, tools: Vec<Value>) -> Vec<u8> {
    let v = json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools}});
    let mut out = serde_json::to_vec(&v).unwrap();
    out.push(b'\n');
    out
}
```

В `lib.rs` добавить `pub mod jsonrpc;` (по алфавиту после `gui`).

- [ ] **Step 8: Запустить**

Run: `cd setup && cargo test -p onec-connect --no-default-features jsonrpc::`
Expected: 17 passed.

- [ ] **Step 9: Коммит**

```bash
git add setup/1c-connect/src/jsonrpc.rs setup/1c-connect/src/lib.rs
git commit -m "1c-connect: JSON-RPC framing and routing for the vrunner bridge, tested on bytes"
```

---

### Task 3: Git-инструменты проекта и затирка (`project.rs`, `common::docker::redact_text`)

**Files:**
- Create: `setup/1c-connect/src/project.rs`
- Modify: `setup/common/src/docker.rs:54-67` (`redact_text`)
- Modify: `setup/1c-connect/src/lib.rs` (`pub mod project;`)

**Interfaces:**
- Consumes: `Cfg::number()`, `Cfg::gitea_url()` (Task 1).
- Produces:
  - `pub struct Project { pub dir: PathBuf, pub git: PathBuf, pub branch: String, pub remote: String, pub login: String }` и `Project::from_cfg(cfg: &Cfg, tools: &Path) -> Project` (`git = tools/git/cmd/git.exe` на Windows, `tools/git/bin/git` иначе; `remote = <gitea_url>/dev1c/<имя каталога project>.git`; `branch = login = workstation`);
  - `pub fn git_flags(login: &str) -> Vec<String>`;
  - `pub fn status(p: &Project) -> Result<String, String>`, `pub fn pull(p: &Project) -> Result<String, String>`, `pub fn push(p: &Project, message: &str) -> Result<String, String>` — `Ok(text)`/`Err(text)` = `isError`;
  - `pub static LOCK: Mutex<&'static str>` (что идёт), `pub fn call(p: &Project, tool: &str, args: &Value) -> (String, bool)` — берёт `try_lock`, при занятости `("занято: идёт <tool>", true)`;
  - `pub fn redact_text` в `common` затирает `Basic <…>`, `/P<…>`, `db-pwd=…`, `--password <…>`.

- [ ] **Step 1: Падающий тест затирки (common)**

В `setup/common/src/docker.rs`, `mod tests` (есть — проверить `grep -n 'mod tests' setup/common/src/docker.rs`; нет — создать):

```rust
    #[test]
    fn git_and_platform_secrets_are_blotted_out() {
        let t = "fatal: Authorization: Basic ZGV2MTpkZXYx rejected\n1cv8 DESIGNER /F C:\\b /N Admin /Pqwerty /DumpCfg\nvrunner --db-pwd=secret --db-user=Admin";
        let r = redact_text(t);
        assert!(!r.contains("ZGV2MTpkZXYx") && r.contains("Basic …"));
        assert!(!r.contains("qwerty") && r.contains("/P…"));
        assert!(!r.contains("secret") && r.contains("--db-pwd=…") && r.contains("--db-user=Admin"));
        assert_eq!(redact_text("API_KEY=abc"), "API_KEY=…");
    }
```

- [ ] **Step 2: Запустить — падает**

Run: `cd setup && cargo test -p common git_and_platform`
Expected: FAIL (assertion `Basic …`).

- [ ] **Step 3: Реализовать затирку**

Заменить `redact_text`:

```rust
pub fn redact_text(text: &str) -> String {
    text.lines()
        .map(|l| {
            let name = l.split('=').next().unwrap_or("");
            let upper = name.to_ascii_uppercase();
            if l.contains('=') && !l.contains(' ') && (upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("PASSWORD")) {
                return format!("{name}=…");
            }
            blot_words(l)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Секреты внутри строки: `Basic <base64>`, `/P<пароль>`, `--db-pwd=…`,
/// `--password <…>` — значение заменяется многоточием, остальное остаётся.
fn blot_words(line: &str) -> String {
    let mut out = Vec::new();
    let mut after_password = false;
    let mut after_basic = false;
    for w in line.split(' ') {
        let blotted = if after_basic {
            after_basic = false;
            "…".to_string()
        } else if after_password {
            after_password = false;
            "…".to_string()
        } else if let Some(k) = w.strip_suffix("Basic").filter(|_| w == "Basic") {
            after_basic = true;
            k.to_string() + "Basic"
        } else if w.starts_with("/P") && w.len() > 2 {
            "/P…".to_string()
        } else if let Some((k, _)) = w.split_once('=').filter(|(k, _)| k.ends_with("pwd") || k.ends_with("password")) {
            format!("{k}=…")
        } else if w == "--password" || w == "--db-pwd" {
            after_password = true;
            w.to_string()
        } else {
            w.to_string()
        };
        out.push(blotted);
    }
    out.join(" ")
}
```

Примечание: `/P…` ловит и `/Path` в выводе — цена принятая: это строки журнала, не команды.

- [ ] **Step 4: Запустить common**

Run: `cd setup && cargo test -p common`
Expected: все зелёные (в т.ч. прежние тесты `redact`).

- [ ] **Step 5: Падающие тесты git-инструментов**

Создать `setup/1c-connect/src/project.rs`:

```rust
//! Git-инструменты моста: состояние, pull, push — одним набором флагов, из
//! MinGit комплекта. Пароль не в URL, а в заголовке; вывод проходит затирку.
//! Мьютекс проекта: второй вызов получает «занято», не очередь.

use crate::cfg::Cfg;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

#[cfg(test)]
mod tests {
    use super::*;

    /// Настоящий git с машины — для тестов; в комплекте будет MinGit.
    fn system_git() -> PathBuf {
        PathBuf::from(if cfg!(windows) { "git.exe" } else { "git" })
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = Command::new(system_git()).args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "safe.directory=*"]).args(args).current_dir(dir).output().unwrap();
        assert!(o.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// Голый origin и проект с веткой dev2, как положит host.project (6B).
    fn fixture() -> (tempdir::Dir, Project) {
        let tmp = tempdir::Dir::new();
        let origin = tmp.path().join("origin.git");
        git(tmp.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        let dir = tmp.path().join("Проект с пробелом");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q", "-b", "dev2"]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        std::fs::write(dir.join(".gitattributes"), "* -text\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "build/\n*.log\n").unwrap();
        std::fs::write(dir.join("autumn-properties.json"), "{}").unwrap();
        let p = Project { dir: dir.clone(), git: system_git(), branch: "dev2".into(), remote: origin.to_string_lossy().into(), login: "dev2".into() };
        (tmp, p)
    }

    #[test]
    fn flags_carry_identity_and_auth_header_without_a_password_in_the_url() {
        let f = git_flags("dev2");
        assert!(f.iter().any(|a| a == "user.name=dev2"));
        assert!(f.iter().any(|a| a == "http.extraHeader=Authorization: Basic ZGV2MjpkZXYy"));
        assert!(f.iter().any(|a| a == "credential.helper="));
        let cfg = Cfg::default();
        let p = Project::from_cfg(&cfg, Path::new("/tools"));
        assert_eq!(p.remote, "http://192.168.1.10:3391/dev1c/Buh.git");
        assert_eq!(p.branch, "dev1");
        assert!(p.git.to_string_lossy().contains("git"));
    }

    #[test]
    fn first_push_creates_main_and_the_workstation_branch() {
        let (_t, p) = fixture();
        std::fs::write(p.dir.join("src").join("Module.bsl"), "Процедура А() КонецПроцедуры").unwrap();
        std::fs::write(p.dir.join("secret.log"), "x").unwrap();
        let text = push(&p, "первая выгрузка").unwrap();
        assert!(text.contains("dev2"), "{text}");
        let heads = git(&p.dir, &["ls-remote", "--heads", "origin"]);
        assert!(heads.contains("refs/heads/main") && heads.contains("refs/heads/dev2"), "{heads}");
        // .log is ignored, and only the four roots are added.
        assert!(!git(&p.dir, &["ls-files"]).contains("secret.log"));
        // Nothing new → push still succeeds and says so.
        let again = push(&p, "пусто").unwrap();
        assert!(again.contains("нечего"), "{again}");
    }

    #[test]
    fn pull_refuses_a_dirty_tree_and_reports_conflicts_by_file() {
        let (t, p) = fixture();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "a").unwrap();
        // A second clone pushes a conflicting change to dev2.
        let other = t.path().join("other");
        git(t.path(), &["clone", "-q", "-b", "dev2", &p.remote, other.to_str().unwrap()]);
        std::fs::write(other.join("src").join("M.bsl"), "2").unwrap();
        git(&other, &["commit", "-qam", "theirs"]);
        git(&other, &["push", "-q", "origin", "dev2"]);
        // Dirty tree here → refused before touching git.
        std::fs::write(p.dir.join("src").join("M.bsl"), "3").unwrap();
        let e = pull(&p).unwrap_err();
        assert!(e.contains("project_push"), "{e}");
        push(&p, "mine").unwrap_or_else(|e| panic!("{e}"));
        let e = pull(&p).unwrap_err();
        assert!(e.contains("конфликт") && e.contains("M.bsl"), "{e}");
        // The tree is left for a human, not half-rebased.
        assert!(!git(&p.dir, &["status", "--porcelain"]).is_empty());
    }

    #[test]
    fn pull_before_any_push_says_export_first() {
        let (_t, p) = fixture();
        let e = pull(&p).unwrap_err();
        assert!(e.contains("Выгрузить в git"), "{e}");
    }

    #[test]
    fn status_names_branch_unpushed_and_the_dump_index() {
        let (_t, p) = fixture();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "a").unwrap();
        std::fs::write(p.dir.join("src").join("ConfigDumpInfo.xml"), "<x/>").unwrap();
        let s = status(&p).unwrap();
        assert!(s.contains("ветка dev2") && s.contains("ConfigDumpInfo.xml: есть") && s.contains("изменено: 1"), "{s}");
    }

    #[test]
    fn second_caller_is_told_busy_not_queued() {
        let (_t, p) = fixture();
        let guard = LOCK.try_lock().unwrap();
        let (text, err) = call(&p, "project_status", &Value::Null);
        assert!(err && text.contains("занято"), "{text}");
        drop(guard);
        let (text, err) = call(&p, "project_status", &Value::Null);
        assert!(!err, "{text}");
    }
}
```

Модуль `tempdir` — свой, в этом же файле под `#[cfg(test)]` (без новой зависимости):

```rust
#[cfg(test)]
mod tempdir {
    pub struct Dir(std::path::PathBuf);
    impl Dir {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!("dev1c-proj-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
```

- [ ] **Step 6: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features project::`
Expected: `cannot find struct Project`.

- [ ] **Step 7: Реализовать**

Над тестами:

```rust
pub struct Project {
    pub dir: PathBuf,
    pub git: PathBuf,
    pub branch: String,
    pub remote: String,
    pub login: String,
}

impl Project {
    /// `tools` — каталог инструментов на машине (`%LOCALAPPDATA%\dev1c`).
    pub fn from_cfg(cfg: &Cfg, tools: &Path) -> Project {
        let dir = PathBuf::from(&cfg.host.project);
        let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
        let git = if cfg!(windows) { tools.join("git").join("cmd").join("git.exe") } else { tools.join("git").join("bin").join("git") };
        Project { dir, git, branch: cfg.host.workstation.clone(), remote: format!("{}/dev1c/{name}.git", cfg.gitea_url()), login: cfg.host.workstation.clone() }
    }
}

/// Один набор флагов на все вызовы: личность, длинные пути, кириллица в
/// именах, никакого менеджера паролей, пароль заголовком (в пилоте равен логину).
pub fn git_flags(login: &str) -> Vec<String> {
    let auth = base64(format!("{login}:{login}").as_bytes());
    [
        format!("user.name={login}"),
        format!("user.email={login}@dev1c.local"),
        "safe.directory=*".into(),
        "core.longpaths=true".into(),
        "core.quotepath=false".into(),
        "credential.helper=".into(),
        format!("http.extraHeader=Authorization: Basic {auth}"),
    ]
    .into_iter()
    .flat_map(|f| ["-c".to_string(), f])
    .collect()
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

struct Out {
    ok: bool,
    text: String,
}

fn git(p: &Project, args: &[&str]) -> Result<Out, String> {
    let mut c = Command::new(&p.git);
    c.args(git_flags(&p.login)).args(args).current_dir(&p.dir).env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let o = c.output().map_err(|e| format!("git не запускается ({}): {e} — выполните 1c-connect install", p.git.display()))?;
    let mut text = String::from_utf8_lossy(&o.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&o.stderr));
    Ok(Out { ok: o.status.success(), text: common::docker::redact_text(text.trim()) })
}

fn must(p: &Project, args: &[&str]) -> Result<String, String> {
    let o = git(p, args)?;
    if o.ok {
        Ok(o.text)
    } else {
        Err(format!("git {}: {}", args.join(" "), o.text))
    }
}

fn remote_has(p: &Project, branch: &str) -> Result<bool, String> {
    Ok(must(p, &["ls-remote", "--heads", p.remote.as_str(), branch])?.contains(&format!("refs/heads/{branch}")))
}

pub fn status(p: &Project) -> Result<String, String> {
    let branch = must(p, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|_| "(нет коммитов)".into());
    let last = must(p, &["log", "-1", "--format=%h %s (%cr)"]).unwrap_or_else(|_| "нет".into());
    let ahead = git(p, &["rev-list", "--count", &format!("origin/{}..HEAD", p.branch)]).map(|o| if o.ok { o.text } else { "все".into() })?;
    let changed = must(p, &["status", "--porcelain"])?.lines().count();
    let index = if p.dir.join("src").join("ConfigDumpInfo.xml").exists() { "есть" } else { "нет — первая загрузка будет полной" };
    Ok(format!("ветка {branch}\nпоследний коммит: {last}\nнеотправленных коммитов: {ahead}\nизменено: {changed}\nConfigDumpInfo.xml: {index}"))
}

pub fn pull(p: &Project) -> Result<String, String> {
    if !must(p, &["status", "--porcelain"])?.is_empty() {
        return Err("в каталоге проекта есть незакоммиченные изменения — сначала project_push".into());
    }
    if !remote_has(p, &p.branch)? {
        return Err(format!("ветки {} в Gitea ещё нет — сначала «Выгрузить в git» (cf_decompile, затем project_push)", p.branch));
    }
    let o = git(p, &["pull", "--rebase", "origin", &p.branch])?;
    if o.ok {
        return Ok(format!("{}\nтеперь cf_load", o.text));
    }
    let conflicted = must(p, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    if !conflicted.is_empty() {
        let _ = git(p, &["rebase", "--abort"]);
        return Err(format!("конфликт в {} — разрешите в каталоге проекта {}", conflicted.replace('\n', ", "), p.dir.display()));
    }
    Err(o.text)
}

pub fn push(p: &Project, message: &str) -> Result<String, String> {
    must(p, &["add", "--", "src", ".gitattributes", ".gitignore", "autumn-properties.json"])?;
    let staged = !must(p, &["diff", "--cached", "--name-only"])?.is_empty();
    let has_commits = git(p, &["rev-parse", "--verify", "HEAD"])?.ok;
    if staged || !has_commits {
        must(p, &["commit", "-q", "--allow-empty", "-m", if message.trim().is_empty() { "выгрузка" } else { message }])?;
    }
    let first = !remote_has(p, "main")?;
    if first {
        must(p, &["push", "-q", "origin", &format!("HEAD:refs/heads/main")])?;
    }
    must(p, &["push", "-q", "-u", "origin", &p.branch])?;
    let what = if staged { "отправлено" } else { "нечего коммитить, ветка отправлена" };
    Ok(format!("{what}: ветка {} в {}{}", p.branch, p.remote, if first { " (репозиторий создан, main тоже отправлен)" } else { "" }))
}

/// Что сейчас идёт, если проект занят.
pub static LOCK: Mutex<&'static str> = Mutex::new("");

/// Вызов инструмента моста: `(текст, isError)`.
pub fn call(p: &Project, tool: &str, args: &Value) -> (String, bool) {
    let Ok(mut guard) = LOCK.try_lock() else {
        return (format!("занято: идёт {}", LOCK.try_lock().map(|g| *g).unwrap_or("другая операция")), true);
    };
    *guard = match tool {
        "project_pull" => "project_pull",
        "project_push" => "project_push",
        _ => "project_status",
    };
    let r = match tool {
        "project_status" => status(p),
        "project_pull" => pull(p),
        "project_push" => push(p, args.get("message").and_then(Value::as_str).unwrap_or("")),
        other => Err(format!("нет инструмента {other}")),
    };
    *guard = "";
    match r {
        Ok(t) => (t, false),
        Err(t) => (t, true),
    }
}
```

Поправка к `call`: занятость сообщать без второго `try_lock` (он тоже провалится) — хранить «что идёт» в отдельном `static BUSY: Mutex<&'static str>`? Проще: `LOCK: Mutex<()>` плюс `static DOING: AtomicUsize` с индексом в `["", "project_status", "project_pull", "project_push"]`. Сделать так:

```rust
pub static LOCK: Mutex<()> = Mutex::new(());
static DOING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const DOING_NAMES: [&str; 4] = ["", "project_status", "project_pull", "project_push"];

pub fn doing() -> &'static str {
    DOING_NAMES[DOING.load(std::sync::atomic::Ordering::Relaxed)]
}

pub fn call(p: &Project, tool: &str, args: &Value) -> (String, bool) {
    use std::sync::atomic::Ordering::Relaxed;
    let Ok(_guard) = LOCK.try_lock() else {
        return (format!("занято: идёт {}", doing()), true);
    };
    DOING.store(DOING_NAMES.iter().position(|n| *n == tool).unwrap_or(1), Relaxed);
    let r = match tool { /* как выше */ };
    DOING.store(0, Relaxed);
    match r { Ok(t) => (t, false), Err(t) => (t, true) }
}
```

Тест `second_caller_is_told_busy_not_queued` держит `LOCK.try_lock().unwrap()` — с `Mutex<()>` это `MutexGuard<()>`; оставить как есть.

В `lib.rs` — `pub mod project;`.

- [ ] **Step 8: Запустить**

Run: `cd setup && cargo test -p onec-connect --no-default-features project::`
Expected: 6 passed (нужен `git` в PATH; на CI есть на обеих ОС).

- [ ] **Step 9: Коммит**

```bash
git add setup/common/src/docker.rs setup/1c-connect/src/project.rs setup/1c-connect/src/lib.rs
git commit -m "1c-connect: git tools of the bridge (status, pull, push) on a project mutex; secrets blotted in git and platform output"
```

---

### Task 4: Job Object на Windows (`job.rs`)

**Files:**
- Create: `setup/1c-connect/src/job.rs`
- Modify: `setup/1c-connect/Cargo.toml:41` (features `windows-sys`: добавить `"Win32_System_JobObjects"`, `"Win32_System_Threading"`)
- Modify: `setup/1c-connect/src/lib.rs` (`pub mod job;`)

**Interfaces:**
- Produces: `pub struct Job` (на не-Windows — пустая), `Job::new() -> Option<Job>`, `fn assign(&self, child: &tokio::process::Child) -> bool`; `Drop` закрывает описатель — с `KILL_ON_JOB_CLOSE` дерево гибнет.

- [ ] **Step 1: Падающий тест (только Windows)**

Создать `setup/1c-connect/src/job.rs`:

```rust
//! Дерево процессов моста (`oscript.exe` → `1cv8.exe`) в Job Object: закрылся
//! канал или вышел `1c-connect` — Конфигуратор не остаётся держать базу.

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn dropping_the_job_kills_the_tree() {
        let job = Job::new().expect("job object");
        let mut cmd = tokio::process::Command::new("cmd.exe");
        cmd.args(["/c", "ping -n 30 127.0.0.1 >nul"]);
        cmd.creation_flags(0x0800_0000 | 0x0000_0004); // CREATE_NO_WINDOW | CREATE_SUSPENDED is not available via tokio; CREATE_NO_WINDOW only
        let mut child = cmd.spawn().unwrap();
        assert!(job.assign(&child));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(child.try_wait().unwrap().is_none(), "still running before the drop");
        drop(job);
        let r = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await;
        assert!(r.is_ok(), "cmd and its ping must die with the job");
    }

    #[cfg(not(windows))]
    #[test]
    fn job_is_a_noop_elsewhere() {
        let job = Job::new();
        assert!(job.is_some());
    }
}
```

(Строку `cmd.creation_flags(0x0800_0000 | 0x0000_0004)` заменить на `cmd.creation_flags(0x0800_0000);` — без `CREATE_SUSPENDED`; тест и так проверяет гибель дерева.)

- [ ] **Step 2: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features job::`
Expected: `cannot find struct Job`.

- [ ] **Step 3: Реализовать**

```rust
#[cfg(windows)]
pub struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Job {
    pub fn new() -> Option<Job> {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
            if ok == 0 {
                windows_sys::Win32::Foundation::CloseHandle(h);
                return None;
            }
            Some(Job(h))
        }
    }

    pub fn assign(&self, child: &tokio::process::Child) -> bool {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        let Some(h) = child.raw_handle() else { return false };
        unsafe { AssignProcessToJobObject(self.0, h as _) != 0 }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(windows)]
unsafe impl Send for Job {}

#[cfg(not(windows))]
pub struct Job;

#[cfg(not(windows))]
impl Job {
    pub fn new() -> Option<Job> {
        Some(Job)
    }
    pub fn assign(&self, _child: &tokio::process::Child) -> bool {
        true
    }
}
```

`Cargo.toml` (1c-connect, строка 41): `features = ["Win32_Foundation", "Win32_UI_WindowsAndMessaging", "Win32_System_JobObjects", "Win32_System_Threading"]`. `lib.rs` — `pub mod job;`.

Если `windows-sys` 0.59 называет тип описателя `HANDLE = *mut c_void` — `h.is_null()` верно; если `isize` — заменить на `h == 0`. Проверить компиляцией.

- [ ] **Step 4: Запустить (здесь Windows)**

Run: `cd setup && cargo test -p onec-connect --no-default-features job::`
Expected: 1 passed, `dropping_the_job_kills_the_tree` за ~1 с.

- [ ] **Step 5: Коммит**

```bash
git add setup/1c-connect/src/job.rs setup/1c-connect/src/lib.rs setup/1c-connect/Cargo.toml setup/Cargo.lock
git commit -m "1c-connect: Job Object with KILL_ON_JOB_CLOSE for the bridge's process tree"
```

---

### Task 5: Мост `pipe_jsonrpc` (`bridge.rs`, `session.rs`)

**Files:**
- Modify: `setup/1c-connect/src/bridge.rs`
- Modify: `setup/1c-connect/src/session.rs:135-162` (`serve`)

**Interfaces:**
- Consumes: `jsonrpc::{Framer, Frame, Route, route_request, rewrite_list_reply, is_response_with_id, our_tools, text_reply, list_reply}` (Task 2); `project::{Project, call}` (Task 3); `job::Job` (Task 4).
- Produces:
  - `pub struct Rules { pub allowed: Vec<String>, pub project: Arc<Project>, pub configurator_running: Arc<dyn Fn() -> usize + Send + Sync> }`;
  - `pub async fn pipe_jsonrpc<S>(stream: S, cmd: tokio::process::Command, rules: Rules, on_exit: impl FnOnce(i32, String))` — та же форма, что `pipe_process`;
  - `pub fn configurator_count() -> usize` (`tasklist /FI "IMAGENAME eq 1cv8.exe" /NH` на Windows, 0 иначе);
  - `session::serve` — на `ports.vrunner` зовёт `pipe_jsonrpc` с `bridge::vrunner_command` (Task 6; здесь — заглушка `vrunner_command` возвращает `Command::new("oscript")`, заменяется в Task 6).

- [ ] **Step 1: Падающие тесты моста**

В `setup/1c-connect/src/bridge.rs`, `mod tests` — рядом с `__echo_child` добавить ветку «скриптового ребёнка»: читает строки, на `tools/list` отвечает списком, на `tools/call` — эхо имени; переменная `BRIDGE_SCRIPT=1`. И `BRIDGE_DIE_AFTER_READ=1`: читает одну строку и выходит без ответа.

```rust
    #[test]
    fn __echo_child() {
        if std::env::var("BRIDGE_ECHO").is_ok() { /* как было */ }
        if let Ok(msg) = std::env::var("BRIDGE_DIE") { /* как было */ }
        if std::env::var("BRIDGE_SCRIPT").is_ok() {
            use std::io::{BufRead, Write};
            let stdin = std::io::stdin();
            let mut out = std::io::stdout();
            for line in stdin.lock().lines() {
                let line = line.unwrap();
                let v: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
                let id = v["id"].clone();
                let reply = match v["method"].as_str() {
                    Some("tools/list") => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"cf_load"},{"name":"infobase_delete"},{"name":"task_status"}]}}),
                    Some("tools/call") => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":format!("child ran {}", v["params"]["name"].as_str().unwrap_or(""))}]}}),
                    Some("notifications/cancelled") => { writeln!(out, "child saw cancel {}", v["params"]["requestId"]).unwrap(); continue }
                    _ => continue,
                };
                writeln!(out, "{reply}").unwrap();
                out.flush().unwrap();
            }
            std::process::exit(0);
        }
        if std::env::var("BRIDGE_DIE_AFTER_READ").is_ok() {
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            std::process::exit(0);
        }
    }
```

Помощники и тесты (в том же `mod tests`):

```rust
    fn rules(allowed: &[&str], running: usize) -> Rules {
        let dir = std::env::temp_dir();
        let project = crate::project::Project { dir, git: "git".into(), branch: "dev1".into(), remote: "x".into(), login: "dev1".into() };
        Rules { allowed: allowed.iter().map(|s| s.to_string()).collect(), project: std::sync::Arc::new(project), configurator_running: std::sync::Arc::new(move || running) }
    }

    async fn talk(child_env: (&str, &str), rules: Rules, send: &[&str]) -> (Vec<serde_json::Value>, i32) {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, child(child_env), rules, move |code, err| { let _ = tx.send((code, err)); }));
        let (r, mut w) = tokio::io::split(ours);
        for l in send {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        let (code, _) = rx.await.unwrap();
        (got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect(), code)
    }

    #[tokio::test]
    async fn list_is_filtered_and_extended_through_the_bridge() {
        let (replies, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#]).await;
        let names: Vec<&str> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["cf_load", "project_status", "project_pull", "project_push"]);
    }

    #[tokio::test]
    async fn disabled_tool_never_reaches_the_child() {
        let (replies, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"infobase_delete","arguments":{}}}"#]).await;
        assert_eq!(replies[0]["result"]["isError"], true);
        assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("1c-connect.toml"));
    }

    #[tokio::test]
    async fn platform_call_is_refused_while_the_configurator_runs() {
        let req = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#;
        let (replies, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 2), &[req]).await;
        assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("закройте Конфигуратор"));
        let (replies, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[req]).await;
        assert_eq!(replies[0]["result"]["content"][0]["text"], "child ran cf_load");
    }

    #[tokio::test]
    async fn our_tool_runs_in_its_own_task_and_the_child_keeps_answering() {
        // project_status against a directory that is not a repo fails fast
        // with isError — enough to prove the path; git itself is tested in project.rs.
        let (replies, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["task_status"], 0), &[
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"project_status","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"task_status","arguments":{}}}"#,
        ]).await;
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert!(replies.iter().any(|r| r["id"] == 5 && r["result"]["content"][0]["text"] == "child ran task_status"));
        assert!(replies.iter().any(|r| r["id"] == 4 && r["result"]["content"][0]["type"] == "text"));
    }

    #[tokio::test]
    async fn cancel_of_our_task_is_swallowed_and_of_a_foreign_one_forwarded() {
        let (replies, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&[], 0), &[
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"project_status","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":6}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":99}}"#,
        ]).await;
        // The child echoes only the foreign cancel as a plain line (not JSON → not in replies);
        // ours either finished first or was cancelled — in both cases no second answer for 6.
        assert!(replies.iter().filter(|r| r["id"] == 6).count() <= 1);
    }

    #[tokio::test]
    async fn child_death_still_answers_pending_list_with_our_tools() {
        let (replies, code) = talk(("BRIDGE_DIE_AFTER_READ", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#]).await;
        assert_eq!(code, 0);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["id"], 7);
        let names: Vec<&str> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["project_status", "project_pull", "project_push"]);
    }
```

Проверка чужого cancel в выводе: `talk` отбрасывает не-JSON; добавить второй возврат — сырой текст — и в тесте `cancel_of_our_task…` проверить `raw.contains("child saw cancel 99") && !raw.contains("child saw cancel 6")`. Расширить `talk` до `(Vec<Value>, String, i32)` и поправить деструктуризацию в остальных тестах.

- [ ] **Step 2: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features bridge::`
Expected: `cannot find struct Rules` / `pipe_jsonrpc`.

- [ ] **Step 3: Реализовать мост**

В `bridge.rs` после `pipe_process`:

```rust
use crate::jsonrpc::{self, Frame, Framer, Route};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Правила моста на одно соединение.
pub struct Rules {
    pub allowed: Vec<String>,
    pub project: Arc<crate::project::Project>,
    pub configurator_running: Arc<dyn Fn() -> usize + Send + Sync>,
}

/// Сколько `1cv8.exe` запущено на машине (открытые Конфигураторы и клиенты).
pub fn configurator_count() -> usize {
    if !cfg!(windows) {
        return 0;
    }
    let mut c = std::process::Command::new("tasklist");
    c.args(["/FI", "IMAGENAME eq 1cv8.exe", "/NH", "/FO", "CSV"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.output().map(|o| String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.starts_with("\"1cv8.exe\"")).count()).unwrap_or(0)
}

/// Мост 778N: `stream` ↔ процесс `vrunner-mcp`, построчно. `tools/list`
/// переписывается, чужие и наши инструменты перехватываются, остальное идёт
/// как есть. Один писатель в канал — `out_tx`.
pub async fn pipe_jsonrpc<S>(stream: S, mut cmd: tokio::process::Command, rules: Rules, on_exit: impl FnOnce(i32, String))
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let job = crate::job::Job::new();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            on_exit(-1, format!("не удалось запустить: {e}"));
            return;
        }
    };
    if let Some(j) = &job {
        j.assign(&child);
    }
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (mut sr, mut sw) = tokio::io::split(stream);
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    // Ответы ребёнка на tools/list, которых ещё ждут; ключ — id как JSON.
    let pending: Arc<std::sync::Mutex<HashMap<String, Value>>> = Arc::default();
    let running: Arc<std::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> = Arc::default();

    let writer = tokio::spawn(async move {
        while let Some(b) = out_rx.recv().await {
            if sw.write_all(&b).await.is_err() {
                break;
            }
        }
        let _ = sw.shutdown().await;
    });

```

Продолжение тела после `writer` (тот же блок кода):

```rust
    let rules = Arc::new(rules);
    let to_child = {
        let (out, pending, running, rules) = (out_tx.clone(), pending.clone(), running.clone(), rules.clone());
        tokio::spawn(async move {
            let mut f = Framer::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match sr.read(&mut buf).await { Ok(0) | Err(_) => break, Ok(n) => n };
                for fr in f.push(&buf[..n]) {
                    if handle_request(fr, &rules, &out, &pending, &running, &mut stdin).await.is_err() { return; }
                }
            }
            if let Some(fr) = f.finish() {
                let _ = handle_request(fr, &rules, &out, &pending, &running, &mut stdin).await;
            }
            let _ = stdin.shutdown().await;
        })
    };
    let from_child = {
        let (out, pending, rules) = (out_tx.clone(), pending.clone(), rules.clone());
        tokio::spawn(async move {
            let mut f = Framer::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match stdout.read(&mut buf).await { Ok(0) | Err(_) => break, Ok(n) => n };
                for fr in f.push(&buf[..n]) {
                    let _ = out.send(rewrite_reply(fr, &rules.allowed, &pending));
                }
            }
            if let Some(fr) = f.finish() {
                let _ = out.send(rewrite_reply(fr, &rules.allowed, &pending));
            }
        })
    };
    let err_tail = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        let lines: Vec<&str> = s.lines().collect();
        lines[lines.len().saturating_sub(5)..].join("\n")
    });
    let status = child.wait().await;
    let _ = from_child.await;
    // Ребёнок ушёл, не ответив на tools/list: наши инструменты агент получает всё равно.
    for (_, id) in pending.lock().unwrap().drain() {
        let _ = out_tx.send(jsonrpc::list_reply(&id, jsonrpc::our_tools()));
    }
    to_child.abort();
    for (_, h) in running.lock().unwrap().drain() {
        h.abort();
    }
    drop(out_tx);
    let _ = writer.await;
    drop(job);
    let tail = err_tail.await.unwrap_or_default();
    on_exit(status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1), common::docker::redact_text(&tail));
```

Вспомогательные функции (модульного уровня):

```rust
type Pending = Arc<std::sync::Mutex<HashMap<String, Value>>>;
type Running = Arc<std::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>;

fn key(id: &Value) -> String {
    id.to_string()
}

/// Одна строка от агента: кому она. `Err` — ребёнок закрыл stdin.
async fn handle_request(fr: Frame, rules: &Arc<Rules>, out: &mpsc::UnboundedSender<Vec<u8>>, pending: &Pending, running: &Running, stdin: &mut tokio::process::ChildStdin) -> Result<(), ()> {
    let line = match fr {
        Frame::Raw(b) => return stdin.write_all(&b).await.map_err(|_| ()),
        Frame::Line(l) => l,
    };
    match jsonrpc::route_request(&line, &rules.allowed) {
        Route::ListRequest { id } => {
            pending.lock().unwrap().insert(key(&id), id);
            forward(stdin, &line).await
        }
        Route::Disabled { id, tool } => {
            let _ = out.send(jsonrpc::text_reply(&id, &format!("инструмент {tool} выключен в 1c-connect.toml ([host] tools)"), true));
            Ok(())
        }
        Route::PlatformCall { id, tool } => {
            let n = (rules.configurator_running)();
            if n > 0 {
                let _ = out.send(jsonrpc::text_reply(&id, &format!("{tool}: на машине запущен 1cv8.exe ({n}) — закройте Конфигуратор и клиентов 1С, затем повторите"), true));
                Ok(())
            } else {
                forward(stdin, &line).await
            }
        }
        Route::OurCall { id, tool, args } => {
            let (out, project, running2) = (out.clone(), rules.project.clone(), running.clone());
            let k = key(&id);
            let k2 = k.clone();
            let h = tokio::spawn(async move {
                let (text, is_error) = tokio::task::spawn_blocking(move || crate::project::call(&project, &tool, &args)).await.unwrap_or_else(|e| (format!("сорвалось: {e}"), true));
                let _ = out.send(jsonrpc::text_reply(&id, &text, is_error));
                running2.lock().unwrap().remove(&k2);
            });
            running.lock().unwrap().insert(k, h);
            Ok(())
        }
        Route::Cancel { id } => match running.lock().unwrap().remove(&key(&id)) {
            Some(h) => {
                h.abort();
                Ok(())
            }
            None => forward(stdin, &line).await,
        },
        Route::Forward => forward(stdin, &line).await,
    }
}

async fn forward(stdin: &mut tokio::process::ChildStdin, line: &[u8]) -> Result<(), ()> {
    stdin.write_all(line).await.map_err(|_| ())?;
    stdin.write_all(b"\n").await.map_err(|_| ())
}

/// Одна строка от ребёнка: ответ на tools/list переписывается, остальное как есть.
fn rewrite_reply(fr: Frame, allowed: &[String], pending: &Pending) -> Vec<u8> {
    match fr {
        Frame::Raw(b) => b,
        Frame::Line(l) => {
            let is_pending = jsonrpc::is_response_with_id(&l).map_or(false, |id| pending.lock().unwrap().remove(&key(&id)).is_some());
            if is_pending {
                if let Some((_, out)) = jsonrpc::rewrite_list_reply(&l, allowed) {
                    return out;
                }
            }
            let mut l = l;
            l.push(b'\n');
            l
        }
    }
}
```

Отмена нашей задачи через `abort()` снимает ожидание, но `spawn_blocking` с git дорабатывает в фоне — мьютекс проекта освободится по его завершении; записать в ledger как известный предел (`ponytail`-комментарий у `Route::Cancel`).

`session.rs` `serve`: добавить ветку

```rust
        } else if inc.port == ports.vrunner {
            let cmd = bridge::vrunner_command(&cfg, &tools_dir);
            let rules = bridge::Rules { allowed: cfg.host.tools.clone(), project: project.clone(), configurator_running: std::sync::Arc::new(bridge::configurator_count) };
            let log = log.clone();
            tokio::spawn(bridge::pipe_jsonrpc(stream, cmd, rules, move |code, err| {
                if code != 0 {
                    log(Event::Warn(format!("vrunner-mcp завершился с кодом {code}: {err}")));
                }
            }));
        }
```

где до замыкания: `let tools_dir = crate::ops_host::tools_dir(); let project = std::sync::Arc::new(crate::project::Project::from_cfg(&cfg, &tools_dir));`. До Task 6 `vrunner_command` и `tools_dir` — временные заглушки в этом коммите: `pub fn vrunner_command(_cfg: &Cfg, _tools: &Path) -> Command { Command::new("oscript") }` в `bridge.rs` и `pub fn tools_dir() -> PathBuf { PathBuf::from(".") }` в `ops_host.rs`; Task 6 их заменяет. Комментарий модуля `serve` («Остальные порты (vrunner, этап 5) пока молчат») обновить: «на порту vrunner — мост `pipe_jsonrpc`».

В `tunnel.rs::run` пробросы: `&[ports.mcp, ports.vrunner, ports.web]` и текст события `"туннель поднят: {} → порты {} (mcp-1c), {} (vrunner) и {} (веб-клиент)"`. Проверить тесты `tunnel.rs`/`session.rs` на этот текст (`grep -n 'туннель поднят' setup/1c-connect/src`).

- [ ] **Step 4: Запустить**

Run: `cd setup && cargo test -p onec-connect --no-default-features`
Expected: все зелёные, в `bridge::` — 6 новых.

- [ ] **Step 5: Коммит**

```bash
git add setup/1c-connect/src/bridge.rs setup/1c-connect/src/session.rs setup/1c-connect/src/tunnel.rs setup/1c-connect/src/ops_host.rs
git commit -m "1c-connect: pipe_jsonrpc — the vrunner bridge on 778N: filtered tools/list, own git tools as tasks, cancel, Job Object"
```

---

### Task 6: Инструменты на машине: `host.tools` и команда `vrunner-mcp` (`ops_host.rs`, `bridge.rs`)

**Files:**
- Modify: `setup/1c-connect/src/ops_host.rs`
- Modify: `setup/1c-connect/src/bridge.rs` (`vrunner_command`)
- Modify: `setup/1c-connect/src/verify.rs:9` (`ORDER` — добавить «инструменты комплекта» после «mcp-1c в комплекте»; остальные факты — 6B)

**Interfaces:**
- Consumes: `Cfg` (Task 1), заглушки Task 5.
- Produces:
  - `pub fn tools_dir() -> PathBuf` — `%LOCALAPPDATA%\dev1c` (Windows) или `$XDG_DATA_HOME/dev1c` → `~/.local/share/dev1c`;
  - `pub fn oscript_exe(tools: &Path) -> PathBuf` (`vrunner/oscript/oscript.exe` | `oscript`), `pub fn vrunner_lib(tools: &Path) -> PathBuf` (`vrunner/lib`), `pub fn git_exe(tools: &Path) -> PathBuf`;
  - `pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64>` (число файлов);
  - операция `host.tools` («Инструменты комплекта на машине»): `Done(версия)`, если `tools.version` на машине равен `tools.version` комплекта; `Missing` — нет; `Drifted` — отличается; `Blocked` — в комплекте нет `vrunner/` или `git/`;
  - `pub fn ib_connection(base_path: &str) -> String` (`/F<path>` | `/S<server\base>`) — уже есть `ib_switch`, использовать его;
  - `bridge::vrunner_command(cfg: &Cfg, tools: &Path) -> tokio::process::Command`.

- [ ] **Step 1: Падающие тесты**

В `ops_host.rs` `mod tests`:

```rust
    #[test]
    fn tools_live_under_the_local_app_data_not_the_kit() {
        let t = tools_dir();
        assert!(t.ends_with("dev1c"), "{}", t.display());
        assert!(oscript_exe(Path::new("T")).ends_with(if cfg!(windows) { "vrunner\\oscript\\oscript.exe" } else { "vrunner/oscript/oscript" }));
        assert!(vrunner_lib(Path::new("T")).ends_with("lib"));
    }

    #[test]
    fn copy_tree_copies_nested_files_and_counts_them() {
        let tmp = std::env::temp_dir().join(format!("dev1c-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src/a/b")).unwrap();
        std::fs::write(tmp.join("src/a/b/f.txt"), "1").unwrap();
        std::fs::write(tmp.join("src/g.txt"), "2").unwrap();
        let n = copy_tree(&tmp.join("src"), &tmp.join("dst")).unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read_to_string(tmp.join("dst/a/b/f.txt")).unwrap(), "1");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tools_op_state_follows_the_version_stamp() {
        let tmp = std::env::temp_dir().join(format!("dev1c-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let kit = tmp.join("kit");
        let home = tmp.join("home");
        std::fs::create_dir_all(kit.join("vrunner")).unwrap();
        std::fs::create_dir_all(kit.join("git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(kit.join("tools.version"), "OneScript 2.2.0; vanessa-runner 3.0.2; MinGit 2.56.0.2\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Missing));
        std::fs::write(home.join("tools.version"), "старая\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Drifted(_)));
        std::fs::write(home.join("tools.version"), "OneScript 2.2.0; vanessa-runner 3.0.2; MinGit 2.56.0.2\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Done(d) if d.contains("3.0.2")));
        std::fs::remove_dir_all(kit.join("git")).unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Blocked(_)));
        let _ = std::fs::remove_dir_all(&tmp);
    }
```

В `bridge.rs` `mod tests`:

```rust
    #[test]
    fn connection_env_is_passed_verbatim() {
        let mut cfg = crate::cfg::Cfg::default();
        cfg.host.base_path = "C:\\Bases\\Бух".into();
        cfg.host.db_password = "па\"роль".into();
        cfg.host.project = "C:\\proj\\Бух".into();
        cfg.host.server = "10.0.0.5:3393".into();
        let c = vrunner_command(&cfg, Path::new("T")).into_std();
        let env: std::collections::HashMap<_, _> = c.get_envs().filter_map(|(k, v)| v.map(|v| (k.to_string_lossy().to_string(), v.to_string_lossy().to_string()))).collect();
        assert_eq!(env["VRUNNER_IBCONNECTION"], "/FC:\\Bases\\Бух");
        assert_eq!(env["VRUNNER_DBUSER"], "Администратор");
        assert_eq!(env["VRUNNER_DBPWD"], "па\"роль");
        assert_eq!(env["NO_PROXY"], "10.0.0.5");
        assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
        assert!(env["OSCRIPT_LIB"].ends_with("lib"));
        assert!(env["PATH"].starts_with(&format!("T{0}git{0}cmd", std::path::MAIN_SEPARATOR)) || env["PATH"].starts_with(&format!("T{0}git{0}bin", std::path::MAIN_SEPARATOR)));
        assert_eq!(c.get_current_dir().unwrap(), Path::new("C:\\proj\\Бух"));
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(args[0].ends_with("mcp.os"), "{args:?}");
    }
```

- [ ] **Step 2: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features tools_ copy_tree connection_env`
Expected: `cannot find function tools_dir` / `vrunner_command` has wrong signature.

- [ ] **Step 3: Реализовать**

`ops_host.rs`:

```rust
/// Каталог инструментов на машине: с носителя комплекта (флешка, FAT32,
/// Defender на каждый запуск) 48 МБ .NET не запускают.
pub fn tools_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into())).join("dev1c")
    } else {
        match std::env::var_os("XDG_DATA_HOME") {
            Some(d) => std::path::PathBuf::from(d).join("dev1c"),
            None => std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".local/share/dev1c"),
        }
    }
}

pub fn oscript_exe(tools: &Path) -> PathBuf {
    tools.join("vrunner").join("oscript").join(if cfg!(windows) { "oscript.exe" } else { "oscript" })
}

pub fn vrunner_lib(tools: &Path) -> PathBuf {
    tools.join("vrunner").join("lib")
}

pub fn git_exe(tools: &Path) -> PathBuf {
    if cfg!(windows) { tools.join("git").join("cmd").join("git.exe") } else { tools.join("git").join("bin").join("git") }
}

pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            n += copy_tree(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), &dst)?;
            n += 1;
        }
    }
    Ok(n)
}

const VERSION_FILE: &str = "tools.version";

/// Состояние по отметке версии: комплект пишет `tools.version` (pack.sh),
/// копия на машине несёт ту же строку.
pub fn tools_state(kit_root: &Path, tools: &Path) -> OpState {
    if !kit_root.join("vrunner").is_dir() || !kit_root.join("git").is_dir() {
        return OpState::Blocked("в комплекте нет vrunner/ или git/ — комплект собран без инструментов (pack.sh)".into());
    }
    let want = std::fs::read_to_string(kit_root.join(VERSION_FILE)).unwrap_or_default();
    let have = std::fs::read_to_string(tools.join(VERSION_FILE)).unwrap_or_default();
    if have.trim().is_empty() {
        OpState::Missing
    } else if have.trim() == want.trim() {
        OpState::Done(format!("{} в {}", want.trim(), tools.display()))
    } else {
        OpState::Drifted(format!("на машине {}, в комплекте {}", have.trim(), want.trim()))
    }
}

struct Tools;
impl Op<Ctx> for Tools {
    fn id(&self) -> &'static str {
        "host.tools"
    }
    fn title(&self) -> &'static str {
        "Инструменты комплекта на машине"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        tools_state(&ctx.kit.root, &tools_dir())
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!("скопировать vrunner/ и git/ из {} в {} (≈ 100 МБ)", ctx.kit.root.display(), tools_dir().display()))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let to = tools_dir();
        if ctx.dry_run {
            return Ok(());
        }
        for d in ["vrunner", "git"] {
            let _ = std::fs::remove_dir_all(to.join(d));
            let n = copy_tree(&ctx.kit.root.join(d), &to.join(d)).with_context(|| format!("копирование {d} в {}", to.display()))?;
            ctx.ok(format!("{d}: {n} файлов"));
        }
        let o = docker::probe(ctx, &oscript_exe(&to).to_string_lossy(), &["--version"])?;
        anyhow::ensure!(o.ok(), "oscript не запускается из {}: {}", to.display(), o.tail(2).join(" "));
        ctx.ok(format!("oscript {}", o.text.trim()));
        let o = docker::probe(ctx, &git_exe(&to).to_string_lossy(), &["--version"])?;
        anyhow::ensure!(o.ok(), "git не запускается из {}: {}", to.display(), o.tail(2).join(" "));
        ctx.ok(o.text.trim().to_string());
        std::fs::copy(ctx.kit.root.join(VERSION_FILE), to.join(VERSION_FILE)).context("отметка версии")?;
        Ok(())
    }
    fn revert(&self, _ctx: &Ctx, level: CleanLevel) -> Option<Vec<Action>> {
        matches!(level, CleanLevel::Data).then(|| vec![Action::new(format!("удалить {}", tools_dir().display()))])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        let _ = std::fs::remove_dir_all(tools_dir());
        ctx.ok("инструменты удалены с машины");
        Ok(())
    }
}
```

Проверить имена вариантов `CleanLevel` и сигнатуру `revert` в `common/src/op.rs` (`grep -n 'enum CleanLevel' -A4 setup/common/src/op.rs`) и подставить точные. `host()` → `vec![Box::new(Env), Box::new(Tools), Box::new(Extension)]`. `use anyhow::Context;` и `use std::path::{Path, PathBuf};` вверху.

`bridge.rs`:

```rust
/// Процесс `vrunner-mcp` из инструментов на машине: cwd — проект, библиотека
/// и git — из `tools`, подключение к базе — переменными (не в командной строке).
pub fn vrunner_command(cfg: &crate::cfg::Cfg, tools: &Path) -> tokio::process::Command {
    use crate::ops_host::{git_exe, oscript_exe, vrunner_lib};
    let lib = vrunner_lib(tools);
    let mut c = tokio::process::Command::new(oscript_exe(tools));
    c.arg(lib.join("vanessa-runner").join("src").join("mcp.os"));
    c.current_dir(&cfg.host.project);
    let git_dir = git_exe(tools).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path = git_dir.into_os_string();
    new_path.push(if cfg!(windows) { ";" } else { ":" });
    new_path.push(path);
    c.env("PATH", new_path);
    c.env("OSCRIPT_LIB", &lib);
    c.env("VRUNNER_IBCONNECTION", crate::ops_host::ib_switch(&cfg.host.base_path));
    c.env("VRUNNER_DBUSER", &cfg.host.db_user);
    c.env("VRUNNER_DBPWD", &cfg.host.db_password);
    c.env("NO_PROXY", cfg.host.server.rsplit_once(':').map(|(h, _)| h).unwrap_or(&cfg.host.server));
    c.env("GIT_TERMINAL_PROMPT", "0");
    c
}
```

`tokio::process::Command::into_std` есть с tokio 1.x? Если нет — `c.as_std()`; проверить компиляцией и подправить тест.

`verify.rs`: `ORDER` → 8 элементов, вставить `"инструменты комплекта"` после `"mcp-1c в комплекте"`; в `run` после факта `ORDER[2]` добавить:

```rust
    let tools = tools_dir();
    v.push(match tools_state(&ctx.kit.root, &tools) {
        OpState::Done(d) => Check::pass(ORDER[3], d),
        OpState::Missing => Check::fail(ORDER[3], format!("в {} ничего нет", tools.display()), "1c-connect install"),
        OpState::Drifted(d) => Check::fail(ORDER[3], d, "1c-connect install — обновит копию"),
        OpState::Blocked(d) => Check::fail(ORDER[3], d, "соберите комплект заново: pack.sh"),
    });
```

и сдвинуть индексы `ORDER[3..6]` → `ORDER[4..7]` ниже. Тест `web_server_fact_comes_before_tunnel_fact` (есть) — проверить, что индексы в нём не захардкожены числами; поправить при необходимости.

- [ ] **Step 4: Запустить**

Run: `cd setup && cargo test -p onec-connect --no-default-features`
Expected: все зелёные.

- [ ] **Step 5: Коммит**

```bash
git add setup/1c-connect/src/ops_host.rs setup/1c-connect/src/bridge.rs setup/1c-connect/src/verify.rs setup/1c-connect/src/session.rs
git commit -m "1c-connect: host.tools copies oscript, vanessa-runner and MinGit to LOCALAPPDATA; vrunner-mcp command with connection by env"
```

---

### Task 7: Туннель под учёткой рабочего места (`sshd-setup.sh`, `Dockerfile`, `tunnel.rs`, факты)

**Files:**
- Modify: `config/sshd-setup.sh:40-70`
- Modify: `Dockerfile:505-510` (убрать `useradd dev`)
- Modify: `setup/1c-connect/src/tunnel.rs:45,294`
- Modify: `setup/1c-connect/tests/tunnel_live.rs` (пароль/логин, если захардкожены: `grep -n '"dev"' setup/1c-connect/tests/tunnel_live.rs`)
- Modify: `docs/superpowers/specs/2026-10-16-setup-binaries-design.md` раздел «Что меняется в образе» (строка про учётку `dev` → три учётки)
- Modify: `setup/dev1c-setup/src/*` — `grep -rn 'dev/dev\|учётк[аи] dev\|password = "dev"' setup/dev1c-setup` и поправить текст подсказок (если есть)

**Interfaces:**
- Consumes: `Cfg::host.workstation`, `Cfg::host.password` (Task 1).
- Produces: `Tunnel::connect(server, login, password, known_hosts, forwards)` — новый параметр `login: &str`.

- [ ] **Step 1: Падающий тест**

В `tunnel.rs` `mod tests` (рядом с `errors_name_the_field_to_fix`):

```rust
    #[test]
    fn auth_error_names_the_workstation_not_a_shared_account() {
        let s = TunnelError::Auth.to_string();
        assert!(s.contains("рабочего места") && !s.contains("учётки dev "), "{s}");
    }
```

- [ ] **Step 2: Запустить — падает**

Run: `cd setup && cargo test -p onec-connect --no-default-features auth_error_names`
Expected: FAIL.

- [ ] **Step 3: Реализовать**

`tunnel.rs`: строка 45 → `write!(f, "сервер не принял пароль рабочего места — проверьте workstation и password в 1c-connect.toml")`; `connect(server: &str, login: &str, password: &str, …)` и `handle.authenticate_password(login, password)`; в `run`: `Tunnel::connect(&cfg.host.server, &cfg.host.workstation, &cfg.host.password, &known_hosts, &[ports.mcp, ports.vrunner, ports.web])`. Комментарий модуля: «Password as `dev`» → «логин и пароль рабочего места». `tunnel_live.rs` — тот же порядок аргументов.

`config/sshd-setup.sh`: `AllowUsers dev` → `AllowUsers dev1 dev2 dev3`; блок `Match User dev` заменить на три:

```sh
# Each workstation forwards only its own three ports: 778N runs the
# Configurator on that developer's machine, so a typo in `workstation` must
# fail at the request instead of binding a neighbour's tunnel. dev1 -> 7771
# (mcp-1c), 7781 (vrunner-mcp), 7791 (web client); dev2 -> 7772/7782/7792;
# dev3 -> 7773/7783/7793. ForceCommand: forwarding only, never a shell.
for n in 1 2 3; do
    cat >>/etc/ssh/sshd_config.d/tunnel.conf <<EOF
Match User dev$n
    ForceCommand /bin/false
    PermitListen localhost:777$n localhost:778$n localhost:779$n
EOF
done
```

(Подставить реальный путь файла конфигурации из скрипта — `grep -n 'cat >' config/sshd-setup.sh`.) Абзацы комментариев про «общую учётку dev» переписать: учётки туннеля — те же системные dev1–dev3, пароль равен логину, sudo у них есть по RDP-сценарию, но sshd даёт только проброс.

`Dockerfile`: удалить `RUN useradd -m -s /bin/sh dev && echo 'dev:dev' | chpasswd` и его комментарий; заменить комментарием «Туннель — под учётками рабочих мест (dev1–dev3), см. sshd-setup».

Спека, раздел «Что меняется в образе»: заменить упоминание учётки `dev` на три учётки с `PermitListen` своих портов. `grep -n 'dev/dev\|учётк[аи] dev\|AllowUsers' docs/superpowers/specs/2026-10-16-setup-binaries-design.md`.

`dev1c-setup`: `grep -rn '"dev"\|dev/dev\|учётк[аи] dev' setup/dev1c-setup/src` — подсказки вида «пароль dev» → «логин и пароль рабочего места (dev1/dev1)».

- [ ] **Step 4: Запустить**

Run: `cd setup && cargo test --workspace --no-default-features`
Expected: все зелёные. `sh -n config/sshd-setup.sh` — без ошибок.

- [ ] **Step 5: Коммит**

```bash
git add config/sshd-setup.sh Dockerfile setup/1c-connect/src/tunnel.rs setup/1c-connect/tests/tunnel_live.rs docs/superpowers/specs/2026-10-16-setup-binaries-design.md setup/dev1c-setup
git commit -m "Tunnel under the workstation's account: sshd Match User dev1..dev3 with their own PermitListen; the shared dev account is gone"
```

---

### Task 8: Комплект: `pack.sh` с OneScript, vanessa-runner и MinGit; манифест `lib`

**Files:**
- Modify: `pack.sh`
- Create: `setup/1c-connect/vendor/vrunner-lib.sha256` (генерируется один раз локально через docker, коммитится)
- Create: `setup/1c-connect/vendor/README.md` (откуда манифест и как обновить)
- Modify: `.github/workflows/build.yml` (шаг `pack.sh` — docker уже есть на раннере; убедиться, что `opm` в контейнере имеет доступ в сеть: раннер ubuntu — да)
- Modify: `docs/superpowers/specs/2026-10-16-setup-binaries-design.md` раздел «Комплекты» (раскладка `vrunner/oscript`, `vrunner/lib`, `git`, `tools.version`)

**Interfaces:**
- Produces: в `dist/1c-connect`: `vrunner/oscript/oscript.exe`, `vrunner/lib/vanessa-runner/src/mcp.os`, `git/cmd/git.exe`, `tools.version` = `OneScript 2.2.0; vanessa-runner 3.0.2; MinGit 2.56.0.2`.

- [ ] **Step 1: Узнать хеши и URL (один раз, здесь)**

Run (Bash):

```bash
cd "$SCRATCH" && for u in \
  https://github.com/EvilBeaver/OneScript/releases/download/v2.2.0/OneScript-2.2.0-win-x64.zip \
  https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.2/MinGit-2.56.0.2-64-bit.zip; do
  n=$(basename "$u"); [ -f "$n" ] || curl -fsSL -o "$n" "$u"; sha256sum "$n"; done
```

Expected: две строки с хешами; записать их в `pack.sh` (`sha_of`). Если URL OneScript отличается (имя ассета релиза 2.2.0), взять точное имя из `gh release view v2.2.0 -R EvilBeaver/OneScript --json assets -q '.assets[].name'`. Также хеши ассетов mcp-1c v1.22.3: `sha256sum mcp-1c-windows-amd64.exe MCP_HTTPService.cfe` после скачивания.

- [ ] **Step 2: Манифест библиотеки (один раз, здесь)**

Run (Bash, `MSYS_NO_PATHCONV=1`):

```bash
cd "$SCRATCH" && rm -rf lib && mkdir lib && MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD/lib:/lib" evilbeaver/onescript:2.2.0 sh -c 'opm install -d /lib vanessa-runner@3.0.2 vanessa-add >/dev/null && cd /lib && find . -type f | sort | xargs sha256sum' > vrunner-lib.sha256 && wc -l vrunner-lib.sha256 && du -sh lib && ls lib && find lib -name 'xddTestRunner.epf' -o -name mcp.os
```

Expected: сотни строк, `lib/vanessa-runner/src/mcp.os` и `xddTestRunner.epf` найдены; размер (записать в ledger). Скопировать `vrunner-lib.sha256` в `setup/1c-connect/vendor/`. Если `opm install vanessa-add` не ставит `xddTestRunner.epf` — записать `Ruling`: `test_xunit` требует отдельного `vanessa-add` в 6B.

- [ ] **Step 3: `pack.sh`**

После блока `mcp-1c` (строки 41–47) заменить его и добавить ряд инструментов:

```sh
# The developer's kit: mcp-1c for the machine with the platform, the same
# release the image was designed against, pinned by hash like everything else.
MCP_1C_VERSION=v1.22.3
for f in mcp-1c-windows-amd64.exe MCP_HTTPService.cfe; do
    fetch "https://github.com/feenlace/mcp-1c/releases/download/$MCP_1C_VERSION/$f" "$CONNECT/$f"
done

# Tools for vrunner on the developer's machine: a self-contained OneScript
# runtime, the vanessa-runner library tree as opm resolves it today — checked
# against the committed manifest so two "3.0.2" kits cannot differ silently —
# and MinGit (the workstations have no git, or an ancient one). 1c-connect
# copies these into %LOCALAPPDATA%\dev1c at install (see host.tools).
ONESCRIPT_VERSION=2.2.0
VRUNNER_VERSION=3.0.2
MINGIT_VERSION=2.56.0.2
tools_row() {
    tmp=$(mktemp -d)
    mkdir -p "$CONNECT/vrunner/oscript" "$CONNECT/vrunner/lib" "$CONNECT/git"
    fetch "https://github.com/EvilBeaver/OneScript/releases/download/v$ONESCRIPT_VERSION/OneScript-$ONESCRIPT_VERSION-win-x64.zip" "$tmp/os.zip"
    unzip -qo "$tmp/os.zip" -d "$CONNECT/vrunner/oscript"
    [ -f "$CONNECT/vrunner/oscript/oscript.exe" ] || { echo "OneScript: oscript.exe not at the top of the archive" >&2; exit 1; }
    fetch "https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.2/MinGit-$MINGIT_VERSION-64-bit.zip" "$tmp/git.zip"
    unzip -qo "$tmp/git.zip" -d "$CONNECT/git"
    [ -f "$CONNECT/git/cmd/git.exe" ] || { echo "MinGit: cmd/git.exe missing" >&2; exit 1; }
    # The library: opm inside the project's own OneScript image; the result is
    # compared file by file with the manifest.
    docker run --rm -v "$CONNECT/vrunner/lib:/lib" "evilbeaver/onescript:$ONESCRIPT_VERSION" \
        sh -c "opm install -d /lib vanessa-runner@$VRUNNER_VERSION vanessa-add >/dev/null && chmod -R a+rX /lib"
    (cd "$CONNECT/vrunner/lib" && sha256sum -c --quiet "$DIR/setup/1c-connect/vendor/vrunner-lib.sha256") \
        || { echo "vrunner lib differs from setup/1c-connect/vendor/vrunner-lib.sha256 — hub.oscript.io moved; review and regenerate the manifest" >&2; exit 1; }
    [ "$(cd "$CONNECT/vrunner/lib" && find . -type f | wc -l)" = "$(wc -l <"$DIR/setup/1c-connect/vendor/vrunner-lib.sha256")" ] \
        || { echo "vrunner lib has files the manifest does not list" >&2; exit 1; }
    printf 'OneScript %s; vanessa-runner %s; MinGit %s\n' "$ONESCRIPT_VERSION" "$VRUNNER_VERSION" "$MINGIT_VERSION" >"$CONNECT/tools.version"
    rm -rf "$tmp"
    echo "tools: $(du -sh "$CONNECT/vrunner" | cut -f1) vrunner, $(du -sh "$CONNECT/git" | cut -f1) git"
}
tools_row
```

Переместить определения `sha_of`/`check_sha`/`fetch` выше первого использования (блок `mcp-1c` сейчас стоит раньше них) и добавить в `sha_of` пять строк: два ассета mcp-1c, OneScript, MinGit (хеши из шага 1). `LLAMA_CACHE` переименовывать не нужно — кэш общий для всех архивов (поправить комментарий вверху: «keeps the downloaded archives»).

На Windows-раннере `docker` нет, но `pack.sh` идёт на Linux-джобе — проверить по `build.yml` (`grep -n 'pack.sh' -B6 .github/workflows/build.yml`), что шаг на `ubuntu`. На этом ноутбуке `pack.sh` целиком не гонять (образ 10 ГБ); проверить функцию `tools_row` отдельно: `OUT_DIR=$SCRATCH/dist CONNECT=$SCRATCH/dist/1c-connect sh -c '. ./pack.sh'` не подходит — вместо этого выполнить тело `tools_row` вручную в Bash с теми же переменными и убедиться, что манифест сходится и `tools.version` записан.

- [ ] **Step 4: Документация**

`setup/1c-connect/vendor/README.md`:

```markdown
# vendor

`vrunner-lib.sha256` — манифест дерева `vrunner/lib` комплекта разработчика:
`opm install -d lib vanessa-runner@3.0.2 vanessa-add` в `evilbeaver/onescript:2.2.0`,
снят 2026-10-16. hub.oscript.io отдаёт зависимости «какие сегодня», поэтому
`pack.sh` сверяет результат с этим списком и падает при расхождении.

Обновить (сознательно, с ревью диффа):

    docker run --rm -v "$PWD/lib:/lib" evilbeaver/onescript:2.2.0 \
      sh -c 'opm install -d /lib vanessa-runner@3.0.2 vanessa-add >/dev/null && cd /lib && find . -type f | sort | xargs sha256sum' > vrunner-lib.sha256
```

Спека, раздел «Комплекты» — раскладка `dist/1c-connect`: `1c-connect.exe`, `1c-connect.toml`, `mcp-1c-windows-amd64.exe`, `MCP_HTTPService.cfe`, `vrunner/oscript/`, `vrunner/lib/`, `git/`, `tools.version`, `checksums.txt`.

- [ ] **Step 5: Проверка**

Run: `sh -n pack.sh` и ручной прогон `tools_row` (шаг 3).
Expected: без ошибок; `tools.version` содержит три версии; `sha256sum -c` молчит.

- [ ] **Step 6: Коммит**

```bash
git add pack.sh setup/1c-connect/vendor/vrunner-lib.sha256 setup/1c-connect/vendor/README.md docs/superpowers/specs/2026-10-16-setup-binaries-design.md
git commit -m "pack.sh: OneScript, vanessa-runner (manifest-checked) and MinGit in the developer's kit; mcp-1c assets pinned by hash"
```

---

### Task 9: CI и ledger

**Files:**
- Modify: `.github/workflows/build.yml` (если шаг `pack.sh` на ubuntu — ничего; если `docker` там недоступен после `dockerd` restart — добавить `sudo systemctl start docker` перед `pack.sh`; проверить по логу первого прогона)
- Modify: `docs/STATUS.md` (строка таблицы «vrunner-мост», «Verified», «Traps», «Next»)

- [ ] **Step 1: Push и CI**

```bash
git push origin main
```

Следить через Monitor за `gh run watch` (как в этапе 5). Expected: Windows и Linux джобы зелёные; `pack.sh` печатает `tools: … vrunner, … git`; артефакт `1c-connect` содержит `vrunner/`, `git/`, `tools.version`.

- [ ] **Step 2: Локальная проверка моста на этой машине (без Gitea)**

Скачать артефакт CI в `E:\dev1C\dist\ci\1c-connect`, выполнить `1c-connect --kit E:\dev1C\dist\ci\1c-connect install --dry-run`, затем `install` (на `host.tools` — копия в `%LOCALAPPDATA%\dev1c`), `1c-connect check` → факт «инструменты комплекта» зелёный. Затем прямой тест моста без туннеля: написать в scratchpad `bridge_probe.py`, который запускает `oscript.exe <lib>\vanessa-runner\src\mcp.os` с теми же переменными, шлёт `initialize` + `tools/list` и печатает число инструментов — ожидается 61. (Сам мост с туннелем — в 6B на живой среде.)

- [ ] **Step 3: Ledger**

`docs/STATUS.md`: таблица — строка `vrunner bridge (6A)` = done; «Verified by running it» — тесты моста на потоках, Job Object убивает дерево, `tools_row` сходится с манифестом, `tools/list` = 61 локально; «Known traps» — `opm` отдаёт зависимости по дню (манифест), `/P…` затирка ловит `/Path`, abort нашей задачи не прерывает git (мьютекс освобождается по завершении); «Next» — 6B. Коммит:

```bash
git add docs/STATUS.md .github/workflows/build.yml
git commit -m "Ledger: vrunner bridge and kit (6A) done; 6B next"
git push origin main
```

---

## Self-review

- Spec coverage (6A): `cfg` (`project`, `tools`, `gitea_url`, `dump` умолчание) — T1; рамка/страницы/фильтр/свои инструменты/задача/отмена — T2+T5; git-инструменты и затирка — T3; Job Object — T4; `host.tools` — T6; `pack.sh` + манифест + хеши mcp-1c — T8; sshd три учётки + `tunnel.rs` под `workstation` — T7; `.gitattributes`/`autumn-properties.json`/операция `host.project` — 6B (не здесь); проверки кроме «инструменты комплекта» — 6B; «один проект — один процесс» (мьютекс, `1cv8.exe`) — T3/T5.
- Placeholder scan: чисто — тело `pipe_jsonrpc` в T5 дано целиком двумя подряд идущими блоками.
- Type consistency: `Project { dir, git, branch, remote, login }` одинаков в T3/T5/T6; `Rules { allowed, project, configurator_running }` в T5; `tools_dir()`/`oscript_exe`/`git_exe`/`vrunner_lib` в T6 используются `vrunner_command` и `Project::from_cfg` (T3 строит путь git сам — заменить в T6 на `git_exe(tools)`, одно место). `Tunnel::connect` с `login` — T7, вызов в `run` там же.
- Review Focus → тесты: 1 → T2 `oversized_line_passes_through_unparsed`; 2 → T3 `second_caller_is_told_busy_not_queued`; 3 → T2 `cancel_of_a_foreign_id_is_forwarded` + T5 `cancel_of_our_task…`; 4 → T6 `connection_env_is_passed_verbatim`; 5 → T5 `child_death_still_answers_pending_list_with_our_tools`.
