# vanessa-runner: проект и окно (этап 6B) — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Разработчик нажимает «Выгрузить в git» — исходники базы уезжают в Gitea в его ветку; агент в контейнере правит и пушит; «Загрузить из git» возвращает правки в базу инкрементом. Проверки говорят, что из этого не готово. Образ знает организацию `dev1c` и пускает агента в git без пароля.

**Architecture:** Операция `host.project` (`project.rs::init` + `ops_host::ProjectOp`) кладёт каталог проекта: `.git` с веткой `dev<N>` и `origin` на Gitea среды, `.gitattributes`, `.gitignore`, `autumn-properties.json`. Модуль `local.rs` — локальный вызов `vrunner-mcp` без туннеля (процесс на время действия, `background: false`, под `project::LOCK`): `export` = `cf_decompile` → `project::push`, `import` = `project::pull` → `cf_load --increment`. CLI `export`/`import`, вкладка «Проект» в окне, пять новых фактов `check`. Образ: организация `dev1c` с тремя владельцами, `~/.git-credentials` и личность git у каждого рабочего места, раздел AGENTS.md. Последняя задача — прогон на этой машине против нового образа.

**Tech Stack:** Rust 2021, `std::process` (локальный вызов), `reqwest` blocking (есть), `serde_json`, `windows-sys` (`Win32_Storage_FileSystem` для типа носителя), Tauri 2 окно (`gui.rs`, `ui/`), sh в `gitea-start.sh`/`entrypoint.sh`, Gitea API v1, CI GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-16-setup-binaries-design.md`, раздел «vanessa-runner в комплекте разработчика (этап 6)», абзац «Два плана» (6B). Факты, проверенные 2026-10-17 на этой машине (ledger STATUS.md): `cf_decompile`/`cf_load` по умолчанию уходят в фон и требуют `oscript` в `PATH` (ответ «Запущено в фоне. taskId=cf_decompile-1…»); с `"background": false` отвечают синхронно `content[0].text` + `isError`; позиционные аргументы — `OUT` у `cf_decompile`, `SRC` у `cf_load`, флаг `increment`; `autumn-properties.json` читается из рабочего каталога, ключи `vrunner.cf.decompile.out`, `vrunner.cf.load.increment`.

## Global Constraints

- Язык сообщений и документации — русский; идентификаторы — английские. Ответы агенту и строки журнала — с подсказкой, что делать.
- Окно и CLI зовут **те же** инструменты, что агент, под тем же `project::LOCK`; отдельного пути через CLI vrunner нет. Локальный вызов — процесс `vrunner-mcp` на время действия, `"background": false`, с `cwd = project`.
- `cf_load`/`infobase_update` отвергаются, пока запущен `1cv8.exe`/`1cv8c.exe` («закройте Конфигуратор»); ненулевой результат `cf_load` сопровождается подсказкой «Конфигурация → Вернуться к конфигурации БД»; без `src/ConfigDumpInfo.xml` загрузка полная, об этом говорится текстом до запуска.
- `host.project`: отказ, если в каталоге лежит `1Cv8.1CD` или `*.dt` (каталог базы); `uninstall` проект и `%LOCALAPPDATA%\dev1c` не трогает.
- `.gitattributes` — `* -text`; `ConfigDumpInfo.xml` коммитится; `.gitignore` — `build/`, `*.log`, `autumn-properties.local.json`; `autumn-properties.json` без секретов.
- Пароль в URL remote не хранится; все вызовы git — через `project::git_flags`.
- Факты `check` в порядке: параметры, платформа, mcp-1c в комплекте, инструменты комплекта, **vrunner-mcp отвечает, проект**, база, веб-сервер, публикация и расширение, **Gitea, Конфигуратор, комплект на носителе**, туннель — последним.
- Образ: `gitea-start.sh` создаёт организацию `dev1c` (владельцы dev1–dev3) идемпотентно при каждом старте; `entrypoint.sh` кладёт каждому рабочему месту `~/.git-credentials` (`http://dev<N>:dev<N>@localhost:3000`, 0600) и `user.name`/`user.email` в `~/.gitconfig`, если их нет; `/etc/gitconfig` уже содержит `credential.helper = store`.
- Коммит на задачу; `cargo test -p onec-connect --no-default-features` зелёный после каждой; прямо в `main`; CI через Monitor; Linux-сборка без Windows и без Gitea обязана проходить.

## Review Focus

1. **Каталог проекта на сетевом диске или с кириллицей и пробелом** — `host.project` и `export` обязаны работать (пути идут в git и в oscript как есть, без кавычек в командной строке). Тест `init_lays_out_a_project_with_a_cyrillic_path` в Задаче 2.
2. **`export` при незакрытом Конфигураторе** — `cf_decompile` сам не отвергается (чтение), но `import` обязан отказать до `pull`, иначе ветка уже сдвинута, а база нет. Тест `import_refuses_before_pulling_while_the_platform_runs` в Задаче 3.
3. **`cf_load` упал на полпути** (платформа вернула ошибку после `pull`) — текст ответа содержит подсказку про возврат к конфигурации БД и ветка остаётся на новом коммите (человек повторяет загрузку руками). Тест `failed_load_names_the_way_back` в Задаче 3.
4. **Gitea отвечает, но организации нет** (старый образ в среде) — факт «Gitea» зелёный, а первый `project_push` падает на push-to-create с текстом git; факт «проект» обязан сказать «организации dev1c в среде нет — обновите образ», проверив `GET /api/v1/orgs/dev1c`. Тест `gitea_fact_distinguishes_no_server_from_no_org` в Задаче 4.
5. **Второй `export` из окна, пока агент держит `project_push`** — «занято: идёт project_push», без очереди; кнопки окна при этом не зависают. Тест `export_is_told_busy_like_any_tool` в Задаче 3.

---

### Task 1: `oscript` в `PATH` процесса и операция `host.project` (`bridge.rs`, `project.rs`, `ops_host.rs`)

**Files:**
- Modify: `setup/1c-connect/src/bridge.rs` (`vrunner_command`, тест `connection_env_is_passed_verbatim`)
- Modify: `setup/1c-connect/src/project.rs` (`init`, `layout_state`, константы файлов)
- Modify: `setup/1c-connect/src/ops_host.rs` (`ProjectOp`, `host()`)

**Interfaces:**
- Consumes: `Project::from_cfg`, `ensure_origin`, `git`/`must` (6A), `ops_host::{tools_dir, oscript_exe, git_exe}`.
- Produces:
  - `pub const GITATTRIBUTES: &str = "* -text\n"`, `pub const GITIGNORE: &str = "build/\n*.log\nautumn-properties.local.json\n"`, `pub const PROPERTIES: &str` (JSON из спеки, с переводом строки);
  - `pub enum Layout { Missing, BaseDir(String), Drifted(String), Ok }` и `pub fn layout_state(p: &Project) -> Layout`;
  - `pub fn init(p: &Project, log: &dyn Fn(String)) -> Result<(), String>` — каталог, `git init -b <branch>` при отсутствии `.git`, `ensure_origin`, три файла, если их нет (существующие не трогаются), `oscript -version` с `cwd = project` через `tools`, `git status`;
  - операция `host.project` («Каталог проекта») между `host.tools` и `host.extension`: `Blocked` для `BaseDir`, `Missing`, `Drifted(what)`, `Done("<dir>, ветка dev<N>")`; `revert` — `None`.

- [ ] **Step 1: Падающий тест `PATH`**

В `bridge.rs`, тест `connection_env_is_passed_verbatim`, заменить проверку `PATH`:

```rust
        let sep = std::path::MAIN_SEPARATOR;
        let bin = format!("T{sep}vrunner{sep}bin");
        let git = if cfg!(windows) { format!("T{sep}git{sep}cmd") } else { format!("T{sep}git{sep}bin") };
        // Background tasks of vrunner-mcp re-exec `oscript` from PATH; git is the bridge's own.
        assert!(env["PATH"].starts_with(&format!("{bin}{}{git}", if cfg!(windows) { ';' } else { ':' })), "{}", env["PATH"]);
```

- [ ] **Step 2: Запустить — падает**

Run: `cd setup && cargo test -p onec-connect --no-default-features connection_env`
Expected: FAIL (PATH начинается с `T\git\cmd`).

- [ ] **Step 3: Реализовать**

В `vrunner_command`:

```rust
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut path = oscript_exe(tools).parent().map(|p| p.as_os_str().to_owned()).unwrap_or_default();
    path.push(sep);
    path.push(git_exe(tools).parent().map(|p| p.as_os_str().to_owned()).unwrap_or_default());
    path.push(sep);
    path.push(std::env::var_os("PATH").unwrap_or_default());
    c.env("PATH", path);
```

(удалить прежние строки с `git_dir`). Комментарий: «фоновые задачи vrunner-mcp перезапускают `oscript` из PATH».

- [ ] **Step 4: Падающие тесты операции**

В `project.rs`, `mod tests`:

```rust
    #[test]
    fn init_lays_out_a_project_with_a_cyrillic_path() {
        let t = tempdir::Dir::new();
        let origin = t.path().join("o.git");
        git(t.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        let dir = t.path().join("Проект Бух");
        let p = Project { dir: dir.clone(), git: system_git(), branch: "dev3".into(), remote: origin.to_string_lossy().into(), login: "dev3".into() };
        assert!(matches!(layout_state(&p), Layout::Missing));
        let mut lines = Vec::new();
        init(&p, &|s| lines.push(s)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]), "dev3");
        assert_eq!(git(&dir, &["remote", "get-url", "origin"]), p.remote);
        assert_eq!(std::fs::read_to_string(dir.join(".gitattributes")).unwrap(), GITATTRIBUTES);
        assert!(std::fs::read_to_string(dir.join("autumn-properties.json")).unwrap().contains("\"increment\": true"));
        assert!(dir.join("src").is_dir());
        assert!(matches!(layout_state(&p), Layout::Ok));
        // A second init changes nothing and keeps a hand-edited file.
        std::fs::write(dir.join(".gitignore"), "mine\n").unwrap();
        init(&p, &|_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).unwrap(), "mine\n");
    }

    #[test]
    fn a_base_directory_is_refused_and_drift_is_named() {
        let t = tempdir::Dir::new();
        let dir = t.path().join("base");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1Cv8.1CD"), "").unwrap();
        let p = Project { dir: dir.clone(), git: system_git(), branch: "dev1".into(), remote: "x".into(), login: "dev1".into() };
        assert!(matches!(layout_state(&p), Layout::BaseDir(_)));
        assert!(init(&p, &|_| {}).unwrap_err().contains("каталог базы"));
        let (_t2, p2) = fixture(); // branch dev2, origin set, three files present
        git(&p2.dir, &["checkout", "-q", "-b", "main"]);
        assert!(matches!(layout_state(&p2), Layout::Drifted(d) if d.contains("ветка")));
    }
```

`init` в тесте использует `tools` для `oscript -version`: сигнатура `init(p, tools: &Path, log)` — в тесте передать `Path::new("nowhere")`; отсутствие `oscript` в тесте — предупреждение в `log`, не ошибка (на машине без инструментов `check` скажет сам). Поправить вызовы в тесте: `init(&p, Path::new("nowhere"), &|s| lines.push(s))`.

- [ ] **Step 5: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features project::`
Expected: `cannot find function init`.

- [ ] **Step 6: Реализовать**

В `project.rs`:

```rust
pub const GITATTRIBUTES: &str = "* -text\n";
pub const GITIGNORE: &str = "build/\n*.log\nautumn-properties.local.json\n";
/// Настройки vrunner без секретов: выгрузка в src, загрузка оттуда инкрементом.
pub const PROPERTIES: &str = "{\n  \"vrunner\": {\n    \"src\": \"src\",\n    \"cf\": {\n      \"decompile\": { \"out\": \"src\" },\n      \"load\": { \"increment\": true }\n    }\n  }\n}\n";

pub enum Layout {
    Missing,
    /// В каталоге лежит база, не проект.
    BaseDir(String),
    Drifted(String),
    Ok,
}

fn looks_like_a_base(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries.flatten().map(|e| e.file_name().to_string_lossy().to_string()).find(|n| n.eq_ignore_ascii_case("1Cv8.1CD") || n.to_lowercase().ends_with(".dt"))
}

pub fn layout_state(p: &Project) -> Layout {
    if let Some(f) = looks_like_a_base(&p.dir) {
        return Layout::BaseDir(f);
    }
    if !p.dir.join(".git").exists() {
        return Layout::Missing;
    }
    let branch = git(p, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|o| o.text).unwrap_or_default();
    if branch != p.branch {
        return Layout::Drifted(format!("ветка {branch}, нужна {}", p.branch));
    }
    let origin = git(p, &["remote", "get-url", "origin"]).map(|o| o.text).unwrap_or_default();
    if origin != p.remote {
        return Layout::Drifted(format!("origin {origin}, нужен {}", p.remote));
    }
    for f in [".gitattributes", ".gitignore", "autumn-properties.json"] {
        if !p.dir.join(f).exists() {
            return Layout::Drifted(format!("нет {f}"));
        }
    }
    Layout::Ok
}

/// Разложить проект: каталог, git с веткой рабочего места и origin на Gitea,
/// три служебных файла (существующие не трогаются). Доказывает круг на
/// месте: oscript и git отвечают из этого каталога.
pub fn init(p: &Project, tools: &Path, log: &dyn Fn(String)) -> Result<(), String> {
    if let Some(f) = looks_like_a_base(&p.dir) {
        return Err(format!("{} — это каталог базы ({f}), не проекта; укажите другой каталог в project", p.dir.display()));
    }
    std::fs::create_dir_all(p.dir.join("src")).map_err(|e| format!("не создать {}: {e}", p.dir.display()))?;
    if !p.dir.join(".git").exists() {
        must(p, &["init", "-q", "-b", &p.branch])?;
        log(format!("git init, ветка {}", p.branch));
    } else {
        let branch = must(p, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
        if branch != p.branch {
            must(p, &["checkout", "-q", "-B", &p.branch])?;
            log(format!("ветка {}", p.branch));
        }
    }
    ensure_origin(p)?;
    for (name, body) in [(".gitattributes", GITATTRIBUTES), (".gitignore", GITIGNORE), ("autumn-properties.json", PROPERTIES)] {
        let f = p.dir.join(name);
        if !f.exists() {
            std::fs::write(&f, body).map_err(|e| format!("{name}: {e}"))?;
            log(format!("{name} создан"));
        }
    }
    must(p, &["status", "--porcelain"])?;
    let oscript = crate::ops_host::oscript_exe(tools);
    match Command::new(&oscript).arg("-version").current_dir(&p.dir).output() {
        Ok(o) if o.status.success() => log(format!("oscript {} из {}", String::from_utf8_lossy(&o.stdout).trim(), p.dir.display())),
        _ => log(format!("oscript в {} не запустился — выполните 1c-connect install (host.tools)", tools.display())),
    }
    Ok(())
}
```

(`Command::new(&oscript)` — с `CREATE_NO_WINDOW` на Windows, как в `git()`.)

В `ops_host.rs`:

```rust
struct ProjectOp;
impl Op<Ctx> for ProjectOp {
    fn id(&self) -> &'static str {
        "host.project"
    }
    fn title(&self) -> &'static str {
        "Каталог проекта"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        let p = crate::project::Project::from_cfg(&ctx.cfg, &tools_dir());
        match crate::project::layout_state(&p) {
            crate::project::Layout::BaseDir(f) => OpState::Blocked(format!("{} — каталог базы ({f}); укажите другой project", p.dir.display())),
            crate::project::Layout::Missing => OpState::Missing,
            crate::project::Layout::Drifted(d) => OpState::Drifted { from: d, to: format!("ветка {}, origin {}", p.branch, p.remote) },
            crate::project::Layout::Ok => OpState::Done(format!("{}, ветка {}", p.dir.display(), p.branch)),
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        let p = crate::project::Project::from_cfg(&ctx.cfg, &tools_dir());
        vec![Action::new(format!("{}: git init -b {}, origin {}, .gitattributes/.gitignore/autumn-properties.json", p.dir.display(), p.branch, p.remote))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        if ctx.dry_run {
            return Ok(());
        }
        let p = crate::project::Project::from_cfg(&ctx.cfg, &tools_dir());
        crate::project::init(&p, &tools_dir(), &|s| ctx.ok(s)).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }
}
```

`host()` → `vec![Box::new(Env), Box::new(Tools), Box::new(ProjectOp), Box::new(Extension)]`.

- [ ] **Step 7: Запустить**

Run: `cd setup && cargo test -p onec-connect --no-default-features`
Expected: все зелёные.

- [ ] **Step 8: Коммит**

```bash
git add setup/1c-connect/src/bridge.rs setup/1c-connect/src/project.rs setup/1c-connect/src/ops_host.rs
git commit -m "1c-connect: host.project lays out the git project; oscript on the bridge's PATH for vrunner's background tasks"
```

---

### Task 2: Локальный вызов `vrunner-mcp` (`local.rs`)

**Files:**
- Create: `setup/1c-connect/src/local.rs`
- Modify: `setup/1c-connect/src/lib.rs` (`pub mod local;`)
- Modify: `setup/1c-connect/src/job.rs` (`assign_std`)

**Interfaces:**
- Consumes: `bridge::vrunner_command(cfg, tools) -> tokio::process::Command` (`.into_std()`), `job::Job`, `jsonrpc::{Framer, Frame, is_response_with_id}`.
- Produces:
  - `pub struct Reply { pub text: String, pub is_error: bool }`;
  - `pub fn call(cmd: std::process::Command, tool: &str, args: Value, timeout: Duration, on_log: &dyn Fn(String)) -> Result<Reply, String>` — процесс на время вызова: `initialize` → `notifications/initialized` → `tools/call {name, arguments}`; `notifications/message` с `params.data` уходят в `on_log`; ответ с нашим id — результат; выход процесса без ответа или таймаут — `Err`; процесс всегда добивается (Job Object на Windows);
  - `pub fn parse_reply(v: &Value) -> Option<Reply>` — чистая часть: `result.content[*].text` склеиваются, `result.isError`; `error.message` → `Reply { is_error: true }`.
  - `Job::assign_std(&self, child: &std::process::Child) -> bool`.

- [ ] **Step 1: Падающие тесты**

В `local.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reply_is_the_text_and_the_error_flag() {
        let v = json!({"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}],"isError":true}});
        let r = parse_reply(&v).unwrap();
        assert_eq!((r.text.as_str(), r.is_error), ("a\nb", true));
        let v = json!({"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"нет метода"}});
        let r = parse_reply(&v).unwrap();
        assert!(r.is_error && r.text.contains("нет метода"));
        assert!(parse_reply(&json!({"jsonrpc":"2.0","method":"notifications/message"})).is_none());
    }

    /// The test binary as a scripted server (see bridge.rs __echo_child):
    /// BRIDGE_SCRIPT answers tools/call with "child ran <name>".
    fn scripted() -> std::process::Command {
        let mut c = std::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "bridge::tests::__echo_child", "--nocapture", "--quiet"]);
        c.env("BRIDGE_SCRIPT", "1");
        c
    }

    #[test]
    fn a_local_call_initialises_calls_and_reaps_the_process() {
        let mut logged = Vec::new();
        let r = call(scripted(), "cf_decompile", json!({"background": false}), std::time::Duration::from_secs(30), &|s| logged.push(s)).unwrap();
        assert_eq!(r.text, "child ran cf_decompile");
        assert!(!r.is_error);
    }

    #[test]
    fn a_process_that_dies_without_answering_is_an_error_not_a_hang() {
        let mut c = std::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "bridge::tests::__echo_child", "--nocapture", "--quiet"]);
        c.env("BRIDGE_DIE_AFTER_READ", "1");
        let t0 = std::time::Instant::now();
        let e = call(c, "cf_load", json!({}), std::time::Duration::from_secs(30), &|_| {}).unwrap_err();
        assert!(e.contains("завершился") || e.contains("не ответил"), "{e}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
    }
}
```

`__echo_child` в `bridge.rs` при `BRIDGE_SCRIPT` отвечает на `initialize`? Сейчас — `continue` для неизвестных методов. Добавить ветку: `Some("initialize") => json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"script","version":"0"}}})`.

- [ ] **Step 2: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features local::`
Expected: `cannot find function call`.

- [ ] **Step 3: Реализовать**

```rust
//! Локальный вызов `vrunner-mcp` без туннеля: окно и CLI зовут те же
//! инструменты, что агент, — процесс на время действия, синхронно
//! (`background: false`), под тем же мьютексом проекта (берёт вызывающий).

use crate::jsonrpc::{Frame, Framer};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::time::Duration;

pub struct Reply {
    pub text: String,
    pub is_error: bool,
}

pub fn parse_reply(v: &Value) -> Option<Reply> {
    if let Some(e) = v.get("error") {
        return Some(Reply { text: e.get("message").and_then(Value::as_str).unwrap_or("ошибка").to_string(), is_error: true });
    }
    let r = v.get("result")?;
    let text = r.get("content").and_then(Value::as_array).map(|c| c.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    Some(Reply { text, is_error: r.get("isError").and_then(Value::as_bool).unwrap_or(false) })
}

fn line(v: Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(&v).unwrap();
    b.push(b'\n');
    b
}

pub fn call(mut cmd: std::process::Command, tool: &str, args: Value, timeout: Duration, on_log: &dyn Fn(String)) -> Result<Reply, String> {
    use std::process::Stdio;
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let job = crate::job::Job::new();
    let mut child = cmd.spawn().map_err(|e| format!("vrunner-mcp не запустился: {e} — выполните 1c-connect install"))?;
    if let Some(j) = &job {
        j.assign_std(&child);
    }
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    // Reader thread: lines to the channel; the main thread owns the timeout.
    std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(stdout);
        let mut f = Framer::new();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    for fr in f.push(&buf) {
                        if let Frame::Line(l) = fr {
                            if tx.send(l).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        }
    });
    let send = |stdin: &mut std::process::ChildStdin, v: Value| stdin.write_all(&line(v)).and_then(|_| stdin.flush()).map_err(|e| format!("vrunner-mcp закрыл вход: {e}"));
    send(&mut stdin, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"1c-connect","version":env!("CARGO_PKG_VERSION")}}}))?;
    let deadline = std::time::Instant::now() + timeout;
    let mut wait_for = |id: i64, stdin: &mut std::process::ChildStdin| -> Result<Value, String> {
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(format!("vrunner-mcp не ответил за {} с", timeout.as_secs()));
            }
            let l = match rx.recv_timeout(left) {
                Ok(l) => l,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err(format!("vrunner-mcp не ответил за {} с", timeout.as_secs())),
                Err(_) => return Err("vrunner-mcp завершился, не ответив".into()),
            };
            let Ok(v) = serde_json::from_slice::<Value>(&l) else { continue };
            if v.get("id").and_then(Value::as_i64) == Some(id) {
                return Ok(v);
            }
            if v.get("method").and_then(Value::as_str) == Some("notifications/message") {
                if let Some(d) = v.pointer("/params/data").and_then(Value::as_str) {
                    on_log(common::docker::redact_text(d));
                }
            }
            let _ = stdin; // keep the borrow shape simple
        }
    };
    let result = (|| {
        wait_for(1, &mut stdin)?;
        send(&mut stdin, json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        send(&mut stdin, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":args}}))?;
        let v = wait_for(2, &mut stdin)?;
        parse_reply(&v).ok_or_else(|| "vrunner-mcp ответил не результатом".to_string())
    })();
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    drop(job);
    result.map(|r| Reply { text: common::docker::redact_text(&r.text), is_error: r.is_error })
}
```

(Упростить `wait_for`, если заимствования мешают: `stdin` ему не нужен — убрать параметр.) В `job.rs`:

```rust
#[cfg(windows)]
impl Job {
    pub fn assign_std(&self, child: &std::process::Child) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as _) != 0 }
    }
}
#[cfg(not(windows))]
impl Job {
    pub fn assign_std(&self, _child: &std::process::Child) -> bool {
        true
    }
}
```

(встроить в существующие `impl` блоки). `lib.rs`: `pub mod local;`.

- [ ] **Step 4: Запустить**

Run: `cd setup && cargo test -p onec-connect --no-default-features`
Expected: все зелёные, `local::` — 3.

- [ ] **Step 5: Коммит**

```bash
git add setup/1c-connect/src/local.rs setup/1c-connect/src/lib.rs setup/1c-connect/src/job.rs setup/1c-connect/src/bridge.rs
git commit -m "1c-connect: local synchronous call of vrunner-mcp for the window and the CLI"
```

---

### Task 3: `export` и `import` (`flow.rs`, `main.rs`)

**Files:**
- Create: `setup/1c-connect/src/flow.rs`
- Modify: `setup/1c-connect/src/lib.rs` (`pub mod flow;`)
- Modify: `setup/1c-connect/src/main.rs` (`Cmd::Export`, `Cmd::Import`)

**Interfaces:**
- Consumes: `local::call`, `bridge::vrunner_command`, `bridge::configurator_count`, `project::{Project, LOCK, push, pull, status}`, `ops_host::tools_dir`.
- Produces:
  - `pub struct Deps<'a> { pub call: &'a dyn Fn(&str, Value) -> Result<local::Reply, String>, pub platform_running: &'a dyn Fn() -> usize, pub push: &'a dyn Fn(&str) -> Result<String, String>, pub pull: &'a dyn Fn() -> Result<String, String>, pub dump_index_exists: bool }` — для тестов без платформы;
  - `pub fn export_with(d: &Deps, message: &str, log: &dyn Fn(String)) -> Result<String, String>`, `pub fn import_with(d: &Deps, log: &dyn Fn(String)) -> Result<String, String>` — чистая логика; оба сначала `project::LOCK.try_lock()` → «занято: идёт <что>»;
  - `pub fn export(ctx: &Ctx, message: &str) -> Result<String, String>`, `pub fn import(ctx: &Ctx) -> Result<String, String>` — сборка `Deps` из настоящих частей (`local::call(vrunner_command(..).into_std(), tool, args, 3600 s, log)`), лог в `ctx.sink` (`Event::Say`).

- [ ] **Step 1: Падающие тесты**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<String>>,
        reply: RefCell<Vec<(String, bool)>>, // answers in order
        running: usize,
        pushed: RefCell<bool>,
        pulled: RefCell<bool>,
        index: bool,
    }
    impl Fake {
        fn new(replies: &[(&str, bool)]) -> Fake {
            Fake { calls: RefCell::new(vec![]), reply: RefCell::new(replies.iter().map(|(t, e)| (t.to_string(), *e)).collect()), running: 0, pushed: RefCell::new(false), pulled: RefCell::new(false), index: true }
        }
        fn run<R>(&self, f: impl FnOnce(&Deps) -> R) -> R {
            let call = |tool: &str, args: Value| {
                self.calls.borrow_mut().push(format!("{tool} {args}"));
                let (t, e) = self.reply.borrow_mut().remove(0);
                Ok(crate::local::Reply { text: t, is_error: e })
            };
            let platform_running = || self.running;
            let push = |m: &str| { *self.pushed.borrow_mut() = true; Ok(format!("отправлено: {m}")) };
            let pull = || { *self.pulled.borrow_mut() = true; Ok("pulled".to_string()) };
            f(&Deps { call: &call, platform_running: &platform_running, push: &push, pull: &pull, dump_index_exists: self.index })
        }
    }

    #[test]
    fn export_decompiles_then_pushes() {
        let f = Fake::new(&[("разобрано", false)]);
        let r = f.run(|d| export_with(d, "выгрузка", &|_| {})).unwrap();
        assert!(r.contains("отправлено"), "{r}");
        assert_eq!(f.calls.borrow()[0], r#"cf_decompile {"OUT":"src","background":false}"#);
        assert!(*f.pushed.borrow());
    }

    #[test]
    fn a_failed_decompile_does_not_push() {
        let f = Fake::new(&[("платформа: ошибка", true)]);
        let e = f.run(|d| export_with(d, "x", &|_| {})).unwrap_err();
        assert!(e.contains("платформа") && !*f.pushed.borrow(), "{e}");
    }

    #[test]
    fn import_refuses_before_pulling_while_the_platform_runs() {
        let mut f = Fake::new(&[]);
        f.running = 1;
        let e = f.run(|d| import_with(d, &|_| {})).unwrap_err();
        assert!(e.contains("закройте Конфигуратор") && !*f.pulled.borrow(), "{e}");
    }

    #[test]
    fn import_pulls_then_loads_incrementally_and_warns_without_the_index() {
        let mut f = Fake::new(&[("загружено", false)]);
        f.index = false;
        let mut log = Vec::new();
        let r = f.run(|d| import_with(d, &|s| log.push(s))).unwrap();
        assert!(*f.pulled.borrow() && r.contains("загружено"), "{r}");
        assert_eq!(f.calls.borrow()[0], r#"cf_load {"SRC":"src","background":false,"increment":true}"#);
        assert!(log.iter().any(|l| l.contains("ConfigDumpInfo.xml")), "{log:?}");
    }

    #[test]
    fn failed_load_names_the_way_back() {
        let f = Fake::new(&[("Конфигуратор: ошибка загрузки", true)]);
        let e = f.run(|d| import_with(d, &|_| {})).unwrap_err();
        assert!(e.contains("Вернуться к конфигурации БД") && *f.pulled.borrow(), "{e}");
    }

    #[test]
    fn export_is_told_busy_like_any_tool() {
        let _g = crate::project::LOCK.try_lock().unwrap_or_else(|e| match e {
            std::sync::TryLockError::Poisoned(g) => g.into_inner(),
            e => panic!("{e}"),
        });
        let f = Fake::new(&[]);
        let e = f.run(|d| export_with(d, "x", &|_| {})).unwrap_err();
        assert!(e.contains("занято"), "{e}");
    }
}
```

(Тест `export_is_told_busy…` гоняется с другими тестами `project::` параллельно — те тоже берут `LOCK`; `try_lock` у них отвечает «занято» вместо работы и тест `second_caller_is_told_busy_not_queued` мог бы увидеть чужой замок. Пометить оба теста `#[serial]`? Зависимостей не добавляем: в `flow` тест держит замок < 1 мс; принять.)

- [ ] **Step 2: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features flow::`
Expected: `cannot find`.

- [ ] **Step 3: Реализовать**

```rust
//! Выгрузка и загрузка: те же инструменты, что у агента, в том же порядке
//! и под тем же мьютексом. Чистая часть — через `Deps`, чтобы проверять
//! порядок шагов без платформы.

use crate::local::Reply;
use crate::ops_host::Ctx;
use serde_json::{json, Value};

pub struct Deps<'a> {
    pub call: &'a dyn Fn(&str, Value) -> Result<Reply, String>,
    pub platform_running: &'a dyn Fn() -> usize,
    pub push: &'a dyn Fn(&str) -> Result<String, String>,
    pub pull: &'a dyn Fn() -> Result<String, String>,
    pub dump_index_exists: bool,
}

fn lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    match crate::project::LOCK.try_lock() {
        Ok(g) => Ok(g),
        Err(std::sync::TryLockError::Poisoned(p)) => Ok(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => {
            let what = crate::project::doing();
            Err(format!("занято: идёт {}", if what.is_empty() { "другая операция" } else { what }))
        }
    }
}

/// cf_decompile в src, затем project_push.
pub fn export_with(d: &Deps, message: &str, log: &dyn Fn(String)) -> Result<String, String> {
    let _g = lock()?;
    log("cf_decompile → src (минуты; Конфигуратор в фоне)".into());
    let r = (d.call)("cf_decompile", json!({"OUT": "src", "background": false}))?;
    if r.is_error {
        return Err(format!("выгрузка не удалась: {}", r.text));
    }
    log(r.text);
    (d.push)(message)
}

/// project_pull, затем cf_load инкрементом.
pub fn import_with(d: &Deps, log: &dyn Fn(String)) -> Result<String, String> {
    let _g = lock()?;
    let n = (d.platform_running)();
    if n > 0 {
        return Err(format!("на машине запущен 1cv8.exe ({n}) — закройте Конфигуратор и клиентов 1С, затем повторите"));
    }
    log((d.pull)()?);
    if !d.dump_index_exists {
        log("src/ConfigDumpInfo.xml нет — загрузка будет полной, не инкрементом (первая после клонирования)".into());
    }
    log("cf_load ← src (минуты)".into());
    let r = (d.call)("cf_load", json!({"SRC": "src", "background": false, "increment": true}))?;
    if r.is_error {
        return Err(format!("загрузка не удалась: {} — в Конфигураторе: Конфигурация → Вернуться к конфигурации БД, затем повторите; ветка уже обновлена", r.text));
    }
    Ok(r.text)
}

fn deps_for<'a>(ctx: &'a Ctx, project: &'a crate::project::Project, log: &'a dyn Fn(String)) -> (impl Fn(&str, Value) -> Result<Reply, String> + 'a, impl Fn(&str) -> Result<String, String> + 'a, impl Fn() -> Result<String, String> + 'a) {
    let tools = crate::ops_host::tools_dir();
    let cfg = ctx.cfg.clone();
    let call = move |tool: &str, args: Value| {
        let cmd = crate::bridge::vrunner_command(&cfg, &tools).into_std();
        crate::local::call(cmd, tool, args, std::time::Duration::from_secs(3600), log)
    };
    let push = move |m: &str| crate::project::push(project, m);
    let pull = move || crate::project::pull(project);
    (call, push, pull)
}

pub fn export(ctx: &Ctx, message: &str) -> Result<String, String> {
    let p = crate::project::Project::from_cfg(&ctx.cfg, &crate::ops_host::tools_dir());
    let log = |s: String| ctx.say(s);
    let (call, push, pull) = deps_for(ctx, &p, &log);
    let d = Deps { call: &call, platform_running: &crate::bridge::configurator_count, push: &push, pull: &pull, dump_index_exists: p.dir.join("src").join("ConfigDumpInfo.xml").exists() };
    export_with(&d, message, &log)
}

pub fn import(ctx: &Ctx) -> Result<String, String> {
    let p = crate::project::Project::from_cfg(&ctx.cfg, &crate::ops_host::tools_dir());
    let log = |s: String| ctx.say(s);
    let (call, push, pull) = deps_for(ctx, &p, &log);
    let d = Deps { call: &call, platform_running: &crate::bridge::configurator_count, push: &push, pull: &pull, dump_index_exists: p.dir.join("src").join("ConfigDumpInfo.xml").exists() };
    import_with(&d, &log)
}
```

`ctx.say` — проверить в `common::Runner` (`grep -n 'fn say\|fn ok\|fn warn' setup/common/src/lib.rs`); если нет `say`, использовать `ctx.ok`. Если возврат трёх `impl Fn` из одной функции не компилируется — собрать `Deps` прямо в `export`/`import` (дублирование пяти строк допустимо).

`main.rs`: `Cmd::Export { /// Сообщение коммита. #[arg(long, default_value = "выгрузка")] message: String }`, `Cmd::Import`; ветки:

```rust
        Cmd::Export { message } => {
            let t = flow::export(&ctx, &message).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{t}");
        }
        Cmd::Import => {
            let t = flow::import(&ctx).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{t}");
        }
```

Справка команд: «Выгрузить конфигурацию в src и отправить в Gitea», «Забрать ветку из Gitea и загрузить в базу». Текст «Установить расширение: … Подключиться: …» в `Status` дополнить «Выгрузить в git: 1c-connect export».

- [ ] **Step 4: Запустить**

Run: `cd setup && cargo test -p onec-connect --no-default-features`
Expected: все зелёные.

- [ ] **Step 5: Коммит**

```bash
git add setup/1c-connect/src/flow.rs setup/1c-connect/src/lib.rs setup/1c-connect/src/main.rs
git commit -m "1c-connect: export (cf_decompile → push) and import (pull → cf_load --increment) in the CLI, same tools and mutex as the agent"
```

---

### Task 4: Проверки (`verify.rs`, `ops_host.rs`)

**Files:**
- Modify: `setup/1c-connect/src/verify.rs`
- Modify: `setup/1c-connect/src/ops_host.rs` (`gitea_probe`, `media_kind`)
- Modify: `setup/1c-connect/Cargo.toml` (`windows-sys` + `"Win32_Storage_FileSystem"`)

**Interfaces:**
- Produces:
  - `ORDER: [&str; 13]` = `["параметры", "платформа 1С", "mcp-1c в комплекте", "инструменты комплекта", "vrunner-mcp отвечает", "проект", "база", "веб-сервер", "публикация и расширение", "Gitea", "Конфигуратор", "комплект на носителе", "туннель"]`;
  - `pub enum Gitea { Down(String), NoOrg, Ok(String) }`, `pub fn gitea_probe(url: &str) -> Gitea` (`GET <url>/api/v1/version` 5 с → `Down`; `GET <url>/api/v1/orgs/dev1c` 404 → `NoOrg`; иначе `Ok(version)`); `pub fn gitea_fact(g: &Gitea) -> Check`;
  - `pub fn media_kind(path: &Path) -> Option<String>` — `Some("съёмный носитель")`/`Some("FAT32")`/`Some("exFAT")` когда плохо, `None` когда нормально (Windows: `GetDriveTypeW` == `DRIVE_REMOVABLE`, `GetVolumeInformationW` имя ФС; не Windows — `None`);
  - `pub fn vrunner_fact(ctx) -> Check` — `local::call(.., "ping", {}, 60 s)`? У `vrunner-mcp` `ping` в списке методов моста есть; проще: `initialize` сам по себе — `local::call` с инструментом `task_status`? Нет: вызвать `local::call(cmd, "cache_show", json!({}), 60 s)` — безвредный инструмент; `Err`/`is_error` → fail «vrunner-mcp не отвечает из <project>: <текст>» с подсказкой `1c-connect install`; иначе pass «AutumnMCP, <N> с»;
  - факт «проект» — по `project::layout_state` плюс `Gitea::NoOrg` → fail «организации dev1c в среде нет — обновите образ (dev1c-setup install)»;
  - факт «Конфигуратор» — `configurator_count()`: 0 → pass «не запущен», N → fail «запущено N процессов 1cv8 — закройте перед загрузкой» (hint «cf_load и import откажут, пока он открыт»);
  - факт «комплект на носителе» — `media_kind(kit.root)`: `None` → pass «<диск>: обычный диск», `Some(k)` → note «<k> — инструменты работают из %LOCALAPPDATA%, комплект можно вынуть после install»;
  - сетевой путь базы (`\\сервер\…` в `base_path`) — note у факта «база»: «сетевая база — одна на двоих, cf_load второго затирает работу первого».

- [ ] **Step 1: Падающие тесты**

В `verify.rs` `mod tests`:

```rust
    #[test]
    fn order_has_thirteen_facts_with_the_tunnel_last() {
        assert_eq!(ORDER.len(), 13);
        assert_eq!(ORDER[12], "туннель");
        assert_eq!(ORDER[4], "vrunner-mcp отвечает");
        assert_eq!(ORDER[9], "Gitea");
    }

    #[test]
    fn gitea_fact_distinguishes_no_server_from_no_org() {
        let down = gitea_fact(&crate::ops_host::Gitea::Down("timeout".into()));
        assert!(!down.ok && down.hint.contains("dev1c-setup"));
        let noorg = gitea_fact(&crate::ops_host::Gitea::NoOrg);
        assert!(!noorg.ok && noorg.detail.contains("организации dev1c"), "{}", noorg.detail);
        let ok = gitea_fact(&crate::ops_host::Gitea::Ok("1.22".into()));
        assert!(ok.ok && ok.detail.contains("1.22"));
    }

    #[test]
    fn configurator_and_media_facts_say_what_to_do() {
        let c = configurator_fact(2);
        assert!(!c.ok && c.detail.contains("2") && c.hint.contains("загруз"));
        assert!(configurator_fact(0).ok);
        let m = media_fact(Some("FAT32".into()), "E:\\");
        assert!(m.ok && m.info && m.detail.contains("FAT32"));
        assert!(!media_fact(None, "C:\\").info);
    }

    #[test]
    fn a_network_base_is_flagged_as_shared() {
        let c = base_fact("\\\\srv\\bases\\Buh", true);
        assert!(c.ok && c.info && c.detail.contains("одна на двоих"), "{}", c.detail);
    }
```

(`Check` поля: `what, detail, hint, ok, info` — проверить `setup/common/src/check.rs` и подставить точные имена.) В `ops_host.rs` `mod tests`:

```rust
    #[test]
    fn gitea_probe_on_a_closed_port_is_down() {
        assert!(matches!(gitea_probe("http://127.0.0.1:1"), Gitea::Down(_)));
    }

    #[test]
    fn media_kind_of_the_system_drive_is_plain() {
        let root = if cfg!(windows) { std::path::Path::new("C:\\") } else { std::path::Path::new("/") };
        assert_eq!(media_kind(root), None);
    }
```

- [ ] **Step 2: Запустить — падает на компиляции**

Run: `cd setup && cargo test -p onec-connect --no-default-features verify:: ops_host::`
Expected: `cannot find`.

- [ ] **Step 3: Реализовать**

`ops_host.rs`:

```rust
pub enum Gitea {
    Down(String),
    NoOrg,
    Ok(String),
}

/// Gitea среды напрямую (без туннеля): версия и организация dev1c.
pub fn gitea_probe(url: &str) -> Gitea {
    let c = match reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(5)).build() {
        Ok(c) => c,
        Err(e) => return Gitea::Down(e.to_string()),
    };
    let version = match c.get(format!("{url}/api/v1/version")).send() {
        Ok(r) if r.status().is_success() => r.text().unwrap_or_default(),
        Ok(r) => return Gitea::Down(format!("HTTP {}", r.status())),
        Err(e) => return Gitea::Down(e.to_string()),
    };
    let version = version.split('"').nth(3).unwrap_or("?").to_string();
    match c.get(format!("{url}/api/v1/orgs/dev1c")).send() {
        Ok(r) if r.status().as_u16() == 404 => Gitea::NoOrg,
        _ => Gitea::Ok(version),
    }
}

/// Что не так с носителем комплекта: съёмный или FAT/exFAT. Ничего — `None`.
#[cfg(windows)]
pub fn media_kind(path: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW, DRIVE_REMOVABLE};
    let root: Vec<u16> = {
        let s = path.to_string_lossy();
        let drive = s.get(..3).unwrap_or("C:\\");
        drive.encode_utf16().chain(std::iter::once(0)).collect()
    };
    unsafe {
        if GetDriveTypeW(root.as_ptr()) == DRIVE_REMOVABLE {
            return Some("съёмный носитель".into());
        }
        let mut fs = [0u16; 64];
        if GetVolumeInformationW(root.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32) != 0 {
            let name = String::from_utf16_lossy(&fs).trim_end_matches('\0').to_string();
            if name.starts_with("FAT") || name == "exFAT" {
                return Some(name);
            }
        }
    }
    None
}
#[cfg(not(windows))]
pub fn media_kind(_path: &Path) -> Option<String> {
    None
}
```

`verify.rs`: новый `ORDER`, помощники `gitea_fact`, `configurator_fact(n: usize)`, `media_fact(kind: Option<String>, root: &str)`, `base_fact(path: &str, exists: bool)` (сетевой путь — `starts_with("\\\\")` → `Check::note`), `vrunner_fact(ctx)` и `project_fact(ctx, &Gitea)`; в `run` — порядок по `ORDER`, индексы прежних фактов сдвинуть. `vrunner_fact` запускает процесс (до 60 с): в `run` он идёт после факта инструментов, только если тот прошёл (иначе `Check::fail(ORDER[4], "инструментов нет", "1c-connect install")`).

Cargo: `windows-sys` features 1c-connect + `"Win32_Storage_FileSystem"`.

- [ ] **Step 4: Запустить**

Run: `cd setup && cargo test -p onec-connect --no-default-features`
Expected: все зелёные. На этой машине: `1c-connect --kit <scratch>/dist/1c-connect check` — тринадцать фактов, «vrunner-mcp отвечает» зелёный, «Gitea» красный (среда из образца).

- [ ] **Step 5: Коммит**

```bash
git add setup/1c-connect/src/verify.rs setup/1c-connect/src/ops_host.rs setup/1c-connect/Cargo.toml setup/Cargo.lock
git commit -m "1c-connect check: vrunner-mcp answers, project layout, Gitea with the dev1c org, running platform, kit media; a network base is flagged"
```

---

### Task 5: Окно: вкладка «Проект» и параметры проекта (`gui.rs`, `ui/`)

**Files:**
- Modify: `setup/1c-connect/src/gui.rs` (`save_cfg` без подмены, команды `project`, `run` с `export|import`, `open_dir`)
- Modify: `setup/1c-connect/ui/index.html` (вкладка и панель «Проект», группа «Проект» в параметрах)
- Modify: `setup/1c-connect/ui/app.js` (`KEYS` + `project`, `ACTIONS.project`, `refreshProject`, `run('export'|'import')`)
- Modify: `setup/1c-connect/ui/app.css` (стили для `.kv` списка, если нет)

**Interfaces:**
- Consumes: `project::status`, `flow::{export, import}`, `project::layout_state`.
- Produces: команда `project(ui) -> ProjectView { dir, branch, lines: Vec<String>, ok: bool, why: String }` (в `spawn_blocking`; `layout_state` → `ok`/`why`, `status` → `lines`); `run(which)` принимает `"export"`/`"import"` (поток, `flow::*`, `conn:done` с ошибкой или без, итог в `conn:line`); `open_dir(ui)` — `explorer <project>` на Windows (`xdg-open` иначе).

- [ ] **Step 1: Разметка**

`index.html`: вкладка `<button class="tab" data-tab="project">Проект</button>` после «Расширение»; панель:

```html
  <!-- Проект: каталог с git, состояние ветки; действия — в нижней плашке -->
  <section class="panel" id="panel-project" hidden>
    <div class="progress" id="p-project"></div>
    <div class="body">
      <ul class="list" id="project"><li class="empty">Каталог проекта задаётся в «Параметрах»; install раскладывает git.</li></ul>
    </div>
  </section>
```

В параметрах после группы «База 1С на этой машине» — группа:

```html
      <p class="group-title">Проект</p>
      <div class="field">
        <label for="f-project">Каталог проекта</label>
        <div class="row">
          <input id="f-project" data-key="project" placeholder="C:\proj\Buh" spellcheck="false">
          <button class="icon-btn browse" data-browse="dir" data-for="f-project" title="Выбрать каталог проекта"><svg …та же иконка папки…></svg></button>
        </div>
        <div class="hint">Исходники в src, git с веткой рабочего места. Не каталог базы.</div>
      </div>
```

Подпись «Семь фактов» в панели проверки → «Тринадцать фактов».

- [ ] **Step 2: Скрипт**

`app.js`: `KEYS` + `'project'`; `ACTIONS.project = [{ do: 'open-dir', label: 'Открыть каталог' }, { do: 'import', label: 'Загрузить из git' }, { do: 'export', label: 'Выгрузить в git', accent: true }]`; `setBusy` — `$('p-project').classList.toggle('on', on && which === 'project')`;

```js
async function refreshProject() {
  const v = await invoke('project');
  const ul = $('project');
  ul.replaceChildren();
  if (!v.ok) { ul.append(row('warn', v.dir, v.why, 'Параметры → Проект, затем Расширение → Установить')); return; }
  ul.append(row('ok', v.dir, `ветка ${v.branch}`));
  for (const l of v.lines) { const [k, d] = l.split(/:\s(.+)/); ul.append(row('dim', k, d || '')); }
}
```

`showTab('project')` → `refreshProject()`. В `run(which)`: подписи журнала `export` → «— выгружаю в git —», `import` → «— загружаю из git —», `setBusy(true, which === 'install' || which === 'uninstall' ? 'ext' : 'project')`. В `act`: `case 'export': run('export')`, `case 'import': run('import')`, `case 'open-dir': await invoke('open_dir')`. `conn:done` → `refreshOps()` и `refreshProject()`.

- [ ] **Step 3: Rust**

`gui.rs`: убрать подмену `project`/`tools` в `save_cfg` (оставить `tools` — поля в окне нет: `host.tools = cur.host.tools.clone()`); команда:

```rust
#[derive(Serialize)]
struct ProjectView {
    dir: String,
    branch: String,
    lines: Vec<String>,
    ok: bool,
    why: String,
}

#[tauri::command]
async fn project(ui: State<'_, Ui>) -> Result<ProjectView, String> {
    let cfg = ui.cfg.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let p = crate::project::Project::from_cfg(&cfg, &ops_host::tools_dir());
        let (ok, why) = match crate::project::layout_state(&p) {
            crate::project::Layout::Ok => (true, String::new()),
            crate::project::Layout::Missing => (false, "каталог проекта ещё не разложен".into()),
            crate::project::Layout::BaseDir(f) => (false, format!("это каталог базы ({f})")),
            crate::project::Layout::Drifted(d) => (false, d),
        };
        let lines = if ok { crate::project::status(&p).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default() } else { vec![] };
        ProjectView { dir: p.dir.display().to_string(), branch: p.branch, lines, ok, why }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_dir(ui: State<Ui>) -> Result<(), String> {
    let dir = ui.cfg.lock().unwrap().host.project.clone();
    let mut c = if cfg!(windows) { let mut c = std::process::Command::new("explorer"); c.arg(&dir); c } else { let mut c = std::process::Command::new("xdg-open"); c.arg(&dir); c };
    c.spawn().map(|_| ()).map_err(|e| format!("не открыть {dir}: {e}"))
}
```

В `run`: ветки `"export"` → `flow::export(&ctx, "выгрузка из окна").map(|t| ctx.ok(t))`, `"import"` → `flow::import(&ctx).map(|t| ctx.ok(t))`, ошибки в `conn:done`. `generate_handler!` + `project, open_dir`.

- [ ] **Step 4: Собрать с окном, посмотреть**

Run: `cd setup && CARGO_BUILD_JOBS=3 cargo build -p onec-connect` (с `gui`), запустить `target/debug/1c-connect.exe` с `ONEC_CONNECT_KIT=<scratch>/dist/1c-connect`, снять скриншоты вкладок «Параметры» (группа «Проект») и «Проект» драйвером `scratchpad/drive.py`, посмотреть глазами: поля по одной линии, действия в плашке, нет скролла.
Expected: вкладка показывает «каталог проекта ещё не разложен» до install; после `install` — ветка и строки состояния.

- [ ] **Step 5: Тесты и коммит**

Run: `cd setup && cargo test -p onec-connect --no-default-features`
Expected: зелёные.

```bash
git add setup/1c-connect/src/gui.rs setup/1c-connect/ui/index.html setup/1c-connect/ui/app.js setup/1c-connect/ui/app.css
git commit -m "1c-connect window: the Проект tab (branch, state, export/import/open) and the project directory in the parameters"
```

---

### Task 6: Образ: организация `dev1c`, учётные данные git, AGENTS.md (`gitea-start.sh`, `entrypoint.sh`, `agents-global.md`, CI)

**Files:**
- Modify: `config/gitea-start.sh` (после ожидания HTTP — организация, при каждом старте)
- Modify: `config/entrypoint.sh` (per-user блок: `.git-credentials`, `.gitconfig`)
- Modify: `config/agents-global.md` (раздел «Как работать с конфигурацией через git»)
- Modify: `.github/workflows/build.yml` (шаг «1C tooling works»: организация есть, credentials у dev1, пуш от dev2 создаёт репозиторий в организации)

- [ ] **Step 1: `gitea-start.sh`**

После цикла ожидания HTTP (он сейчас только при `FIRST_RUN`; вынести ожидание из-под `FIRST_RUN` — оно нужно и для организации):

```sh
# --- The developers' organisation ------------------------------------------
# One repository per base lives under dev1c/<base>, every workstation an
# owner: push-to-create then works for any of them. Idempotent: the org is
# created once, owners re-added every start (PUT is a no-op for a member).
api() { # $1 = method, $2 = path, $3 = json body (optional)
    curl -sf -o /dev/null -w '%{http_code}' -X "$1" -u "${GITEA_ADMIN}:${GITEA_PASSWORD}" \
        -H 'Content-Type: application/json' ${3:+-d "$3"} "http://localhost:3000/api/v1$2" 2>/dev/null || true
}
if [ "$(api GET /orgs/dev1c)" != "200" ]; then
    api POST /orgs '{"username":"dev1c","full_name":"Разработка 1С","visibility":"public","repo_admin_change_team_access":true}' >/dev/null
    echo "organisation dev1c created" >>"$GITEA_LOG"
fi
owners=$(curl -sf -u "${GITEA_ADMIN}:${GITEA_PASSWORD}" "http://localhost:3000/api/v1/orgs/dev1c/teams" 2>/dev/null \
    | tr ',' '\n' | grep -B1 '"name":"Owners"' | grep -o '"id":[0-9]*' | head -1 | cut -d: -f2)
for u in $GITEA_USERS; do
    [ -n "$owners" ] && api PUT "/teams/$owners/members/$u" >/dev/null
done
```

(`jq` в образе есть — `grep -q jq Dockerfile`; если есть, брать id через `jq -r '.[] | select(.name=="Owners") | .id'` вместо `tr/grep`.)

- [ ] **Step 2: `entrypoint.sh`**

В per-user цикле (`for u in dev1 dev2 dev3 dev` — убрать `dev`, учётки больше нет), после проверки владельца:

```sh
    # git without a password prompt and with a real identity: the credential
    # helper is `store` in /etc/gitconfig, this is the file it reads. Written
    # only when absent — a person may have changed the password or the name.
    cred="$home/.git-credentials"
    if [ ! -e "$cred" ]; then
        printf 'http://%s:%s@localhost:3000\n' "$u" "$u" >"$cred"
        chown "$u:$u" "$cred" && chmod 600 "$cred"
    fi
    if [ ! -e "$home/.gitconfig" ]; then
        printf '[user]\n\tname = %s\n\temail = %s@dev1c.local\n' "$u" "$u" >"$home/.gitconfig"
        chown "$u:$u" "$home/.gitconfig"
    fi
```

(Символические ссылки: как в блоке настроек Zed — пропускать, если `$cred` или `$home/.gitconfig` — ссылка.)

- [ ] **Step 3: AGENTS.md**

В `config/agents-global.md` после «Остальные инструменты» — раздел:

```markdown
## Как работать с конфигурацией через git

Конфигурация базы живёт в Gitea: репозиторий `dev1c/<база>`, у каждого
рабочего места своя ветка — `dev1`, `dev2`, `dev3` (твоя — по логину).
`main` — общая, в неё сливают руками.

1. Начало: `git clone -b dev<N> http://localhost:3000/dev1c/<база>.git`
   (вход уже настроен) или `git pull` в уже склонированном.
2. Правки — в `src/`, обычные коммиты и `git push` в свою ветку.
3. Загрузка в базу идёт на машине разработчика инструментами MCP `vrunner`
   (работает при поднятом туннеле): сначала `project_pull`, затем `cf_load`
   (инкрементом по `src/ConfigDumpInfo.xml` — этот файл не править руками),
   потом `validate_syntax_check`, потом `test_xunit`. Выгрузка обратно:
   `cf_decompile`, затем `project_push`.
4. Длинные команды уходят в фон: ответ с `taskId`, дальше `task_status` и
   `task_result`; не повторяй команду, пока задача выполняется.
5. «занято: идёт …» — на машине уже идёт операция; подожди и повтори.
   «закройте Конфигуратор» — у разработчика открыт Конфигуратор; скажи ему.
```

- [ ] **Step 4: CI**

`.github/workflows/build.yml`, шаг «1C tooling works», в конец:

```yaml
          docker exec dev1c sh -c 'curl -sf http://localhost:3000/api/v1/orgs/dev1c | grep -q "\"username\":\"dev1c\""' \
            && echo "ok: organisation dev1c"
          docker exec dev1c sh -c 'test -s /home/dev1/.git-credentials && test "$(stat -c %a /home/dev1/.git-credentials)" = 600' \
            && echo "ok: git credentials for dev1"
          # Push-to-create as a non-admin owner: the first project_push does this.
          docker exec -u dev2 dev1c sh -c 'cd $(mktemp -d) && git init -q -b dev2 && echo x > f && git add f && git commit -qm x && git push -q http://localhost:3000/dev1c/ci-probe.git dev2' \
            && docker exec dev1c sh -c 'curl -sf http://localhost:3000/api/v1/repos/dev1c/ci-probe >/dev/null' \
            && echo "ok: push-to-create in the organisation as dev2"
```

- [ ] **Step 5: Проверка и коммит**

Run: `sh -n config/gitea-start.sh && sh -n config/entrypoint.sh` → без ошибок.

```bash
git add config/gitea-start.sh config/entrypoint.sh config/agents-global.md .github/workflows/build.yml
git commit -m "Image: the dev1c organisation with the workstations as owners, git credentials and identity per home, agent rules for git"
```

Push и CI (Monitor): образ, проверки организации и push-to-create, живой туннель, `pack.sh`.

---

### Task 7: Образ на этой машине и прогон круга

**Files:**
- Modify: `docs/STATUS.md`, `docs/superpowers/specs/2026-10-16-setup-binaries-design.md` (если найдутся расхождения)

- [ ] **Step 1: Новый образ локально**

Скачать артефакт `dev1c-kit` зелёного прогона (`gh run download <id> -n dev1c-kit -D E:\dev1C\dist\ci`), `dev1c-setup --kit E:\dev1C\dist\ci\dev1c-kit status` → образ `Drifted`; `install` пересоздаёт контейнер (`serve = "remote"` в локальном `dev1c.toml`, чтобы не трогать `volt-llama`). `dev1c-setup check` → sshd пускает `dev1`, Gitea отвечает, организация есть (`curl http://localhost:3391/api/v1/orgs/dev1c`).

- [ ] **Step 2: Проект и выгрузка**

`1c-connect.toml` (копия в `<scratch>/dist/1c-connect`): `server = "127.0.0.1:3393"`, `workstation = "dev1"`, `password = "dev1"`, `base_path = "C:\Users\bucher\Documents\InfoBase"`, `project = "C:\proj\InfoBase"`, `base_url` как в прошлых этапах. `1c-connect install` → `host.project` разложил каталог; `1c-connect check` → 13 фактов, красный только туннель (не поднят). `1c-connect export --message "первая выгрузка"` → `cf_decompile` (минуты), push → `http://localhost:3391/dev1c/InfoBase` с ветками `main` и `dev1`; `ls C:\proj\InfoBase\src\ConfigDumpInfo.xml`.

- [ ] **Step 3: Правка в контейнере и загрузка**

`docker exec -u dev1 dev1c sh -c 'cd $(mktemp -d) && git clone -q -b dev1 http://localhost:3000/dev1c/InfoBase.git . && f=$(ls src/CommonModules/*/Ext/Module.bsl | head -1) && printf "\n// правка из контейнера %s\n" "$(date)" >> "$f" && git commit -qam "правка из контейнера" && git push -q'` → `1c-connect import` → `pull` + `cf_load --increment` (ожидается один модуль). Открыть Конфигуратор, убедиться, что комментарий в модуле есть; закрыть.

- [ ] **Step 4: Через туннель и Zed**

`1c-connect connect` (консоль) → в контейнере `nc -z localhost 7781`; из RDP-сессии `dev1` в Zed: агент зовёт `project_status` и `validate_syntax_check` (`vrunner` сервер уже в настройках). Обрыв: закрыть `1c-connect` во время `cf_decompile` из агента → `tasklist` без `1cv8.exe`.

- [ ] **Step 5: Ledger**

`docs/STATUS.md`: таблица (`project`, образ локально обновлён), «Verified by running it» (круг export → правка → import, туннель 778N, Job Object на живом Конфигураторе), ловушки, «Next» (6B done; дальше — PRISM/ERP по списку). Коммит, push, CI.

---

## Self-review

- Spec coverage (6B): `host.project` — T1; `export`/`import` тем же путём, что агент — T2+T3; вкладка «Проект» и параметры — T5; проверки (vrunner-mcp отвечает, проект, Gitea, Конфигуратор, носитель, сетевая база) — T4; образ (организация, credentials, AGENTS.md) — T6; прогон на этой машине и обрыв во время `cf_decompile` — T7; `uninstall` проект не трогает — `revert = None` в T1.
- Placeholder scan: иконка папки в T5 помечена «та же иконка» — скопировать SVG из поля `f-base_path` (одна разметка на три кнопки); `ctx.say`/`Check` поля — проверить по месту, указано как.
- Type consistency: `Layout::{Missing, BaseDir(String), Drifted(String), Ok}` в T1/T4/T5; `local::Reply { text, is_error }` в T2/T3; `Deps` в T3; `Gitea::{Down, NoOrg, Ok}` в T4; `init(p, tools, log)` в T1 и `ProjectOp::apply`.
- Review Focus → тесты: 1 → T1 `init_lays_out_a_project_with_a_cyrillic_path`; 2 → T3 `import_refuses_before_pulling_while_the_platform_runs`; 3 → T3 `failed_load_names_the_way_back`; 4 → T4 `gitea_fact_distinguishes_no_server_from_no_org`; 5 → T3 `export_is_told_busy_like_any_tool`.
