# Локальный режим и переключатель MCP в `1c-connect` — план работ

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `1c-connect` сам слушает `777N`/`778N` на loopback, сам поднимает HTTP-сервис активной базы своим httpd, а переключатель в окне включает и выключает обслуживание вызовов MCP — одинаково на Windows и в образе.

**Architecture:** Сеанс получает второй транспорт: вместо SSH-туннеля `TcpListener` на петле, на каждое соединение — тот же `bridge::pipe_jsonrpc` с теми же командами мостов. Переключатель перестаёт управлять портами и начинает управлять обслуживанием вызовов (отказ на вызове, как уже сделано для запрещённых инструментов). Публикация базы переезжает из скрипта стенда в `ops_host` и работает своим переносимым httpd под пользователем на обеих ОС.

**Tech Stack:** Rust (tokio, gpui/gpui-component, tray-icon), POSIX sh для стенда, Apache httpd portable + модуль платформы 1С, OneScript/vanessa-runner.

**Spec:** `docs/superpowers/specs/2026-10-19-connect-local-mode-design.md`

## Global Constraints

- **Собирать только в CI.** Локально `cargo build` не запускать (у машины мало памяти и диска). Проверка синтаксиса локально — `rustfmt --emit stdout` по изменённым файлам; сборка и тесты — `gh workflow run build.yml --ref local-mode -f binaries_only=true`, затем ожидание прогона.
- **Ветка работы — `local-mode`** в рабочей копии `E:\dev1C\.worktrees\local-mode`. В `origin/main` не пушить.
- **Пароли не попадают в журнал и файл журнала** иначе как через `redact_text`.
- **Zed не трогаем.** В настройках Zed навсегда `nc localhost 777N` и `778N`; слушатели держатся всё время работы коннектора, переключатель решает только, обслуживать ли вызов.
- **Половинчатого состояния нет:** не встал второй слушатель — первый закрывается, сеанс в `Failed`.
- **Ничего не держать дольше вызова:** всё, что коннектор запустил, живёт в его группе процессов (Job Object на Windows, группа процессов на Linux) и гасится целиком; чужие процессы платформы не трогать никогда.
- **Язык интерфейса — язык 1С-разработчика.** «Выгрузить из 1С», «Загрузить в 1С», «MCP включены»; `git`, `cf_decompile`, `vrunner` — только в хвосте строки журнала.
- **Тексты и решения — из спеки дословно**, включая обязательные атрибуты `default.vrd` и набор модулей httpd.

## Review Focus

- **Порт занят чужим процессом**: коннектор обязан назвать держателя (имя и идентификатор процесса) и не повторять попытки бесконечно, иначе человек видит вечное «Включаю…».
- **Выключенные MCP при живом соединении Zed**: вызов должен получить читаемый отказ в формате JSON-RPC, а не обрыв соединения, иначе Zed покажет «сервер недоступен» и потребует ручного перезапуска.
- **Смена базы под работающим сеансом**: «Сохранить» во время операции обязано быть заблокировано, иначе вызов агента умирает без ответа.
- **Остановка сеанса с живым потомком**: закрытие слушателя не рвёт принятые соединения, поэтому забытый `oscript` или платформа продолжат держать единственный сеанс базы.
- **Публикация без обязательных атрибутов**: второй вызов агента подряд получает HTTP 500 от платформы; дескриптор обязан содержать `reuseSessions="dontuse"` и пул из одного соединения.

## Карта файлов

| Файл | Ответственность после работ |
|---|---|
| `setup/1c-connect/src/job.rs` | убийство дерева процессов на обеих ОС: Job Object на Windows, группа процессов на Linux |
| `setup/1c-connect/src/bridge.rs` | один путь «порт → процесс» для обоих мостов, отказ на вызове при выключённых MCP |
| `setup/1c-connect/src/listen.rs` | **новый**: локальный транспорт, два слушателя на петле, список живых мостов |
| `setup/1c-connect/src/holder.rs` | **новый**: кто слушает порт (имя и pid) — `/proc/net/tcp` и `GetExtendedTcpTable` |
| `setup/1c-connect/src/session.rs` | выбор транспорта по `cfg.host.mode`, состояния, тексты без слова «туннель» |
| `setup/1c-connect/src/tunnel.rs` | ограниченные повторы, когда отказано именно в пробросе порта |
| `setup/1c-connect/src/cfg.rs` | `publication`, необязательный порт httpd, `Cfg::base_url()` |
| `setup/1c-connect/src/ops_host.rs` | `host.publication` (свой httpd и дескриптор), кроссплатформенный поиск платформы |
| `setup/1c-connect/src/verify.rs` | факты по режиму, имена вместо позиций |
| `setup/1c-connect/src/main.rs` | подкоманда `serve` — сеанс без окна |
| `setup/1c-connect/src/gui/{mod,panel,tray}.rs` | слова и поля по режиму, подменю «Режим работы», строка публикации |
| `config/1c/stand.sh` | без `socat` и без своей публикации; запускает `serve` |
| `pack.sh` | строка `httpd/` в Windows-комплекте с закреплённой суммой |

---

### Task 1: Уборка дерева процессов на Linux (`job.rs`)

**Files:**
- Modify: `setup/1c-connect/src/job.rs` (весь файл: ветка `#[cfg(not(windows))]`, строки 62-104)
- Modify: `setup/1c-connect/Cargo.toml` (новая секция `[target.'cfg(unix)'.dependencies]`)
- Modify: `setup/1c-connect/src/bridge.rs:171-181` и `setup/1c-connect/src/local.rs` (места, где создаётся `Job`) — вызов нового помощника перед `spawn`
- Test: там же, inline `mod tests` в `job.rs`

**Interfaces:**
- Consumes: ничего.
- Produces: `job::Job::new() -> Option<Job>`, `Job::assign(&self, child: &tokio::process::Child) -> bool`, `Job::assign_std(&self, child: &std::process::Child) -> bool` (подписи не меняются), плюс новое:
  `job::group(cmd: &mut tokio::process::Command)` и `job::group_std(cmd: &mut std::process::Command)` — на Linux ставят `process_group(0)`, на Windows ничего не делают.

- [ ] **Шаг 1: зависимость.** В `setup/1c-connect/Cargo.toml` добавить

```toml
[target.'cfg(unix)'.dependencies]
# Убийство группы процессов: SIGTERM, через 5 с SIGKILL. libc уже в Cargo.lock
# (его тянет tokio), прямая зависимость ничего не докачивает.
libc = "0.2"
```

- [ ] **Шаг 2: падающий тест.** В `job.rs` добавить тест (только `#[cfg(all(test, not(windows)))]`):

```rust
#[test]
fn dropping_the_job_kills_the_whole_group() {
    // sh порождает внука: умереть должен и он.
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", "sleep 30 & echo $!; wait"]).stdout(std::process::Stdio::piped());
    group_std(&mut cmd);
    let job = Job::new().unwrap();
    let mut child = cmd.spawn().unwrap();
    assert!(job.assign_std(&child));
    let grandchild = read_first_line(child.stdout.as_mut().unwrap());
    drop(job);
    assert!(waits_for_death(child.id()), "sh жив");
    assert!(waits_for_death(grandchild), "внук жив");
}
```

  Помощники пишутся в том же `mod tests`: `read_first_line` читает одну строку, `waits_for_death(pid) -> bool` ждёт до 8 с, проверяя `/proc/<pid>` на существование.

- [ ] **Шаг 3: тест падает.** Прогон в CI на Linux (локально `cargo` не запускать): тест обязан упасть на `assert!(waits_for_death(...))`, потому что заглушка никого не убивает.
- [ ] **Шаг 4: реализация для Linux.** `pub struct Job(std::sync::Mutex<Vec<i32>>)`; `new()` — пустой список; `assign`/`assign_std` кладут `child.id() as i32` (он же идентификатор группы, потому что группу задаёт `group`/`group_std`) и возвращают `true`; `impl Drop` посылает каждой группе `libc::killpg(pgid, SIGTERM)`, затем отдельным потоком через 5 с `SIGKILL` (поток детачится, чтобы `drop` не ждал). Нулевые и отрицательные идентификаторы в список не попадают — иначе сигнал уйдёт всей своей группе.
- [ ] **Шаг 5: помощник группы.** `group`/`group_std` на unix — `std::os::unix::process::CommandExt::process_group(0)`; на Windows — пустое тело. Вызвать его в `bridge::pipe_jsonrpc` перед `spawn` и в `local::call`.
- [ ] **Шаг 6: тест проходит.** CI: `job.rs` тесты зелёные на Linux и на Windows.
- [ ] **Шаг 7: коммит.**

```bash
git add setup/1c-connect/src/job.rs setup/1c-connect/src/bridge.rs setup/1c-connect/src/local.rs setup/1c-connect/Cargo.toml
git commit -m "job: группа процессов на Linux вместо заглушки, уборка дерева на обеих ОС"
```

---

### Task 2: Отказ на вызове при выключённых MCP, один путь для двух мостов

**Files:**
- Modify: `setup/1c-connect/src/bridge.rs` (`Rules` L113, `pipe_jsonrpc` L164, `handle_request` L291, `rewrite_reply`)
- Modify: `setup/1c-connect/src/session.rs:138` (`serve`: порт `mcp` переводится с `pipe_process` на `pipe_jsonrpc`)
- Test: inline `mod tests` в `bridge.rs` (готовый стенд `scripted()` на самовызове тестового бинаря)

**Interfaces:**
- Consumes: `job::group` (задача 1).
- Produces:
  - `bridge::Rules { allowed: Vec<String>, project: Arc<Project>, configurator_running: Arc<dyn Fn() -> usize + Send + Sync>, defaults: Value, enabled: Arc<AtomicBool>, inject: bool }` — два новых поля;
  - `Rules::for_mcp(enabled: Arc<AtomicBool>) -> Rules` и `Rules::for_vrunner(...) -> Rules` (конструкторы, чтобы вызывающий не собирал структуру руками);
  - текст отказа ровно такой: `"MCP выключены в 1c-connect: включите переключатель в окне"`.

- [ ] **Шаг 1: падающие тесты.** В `bridge.rs`:

```rust
#[tokio::test]
async fn a_tool_call_is_refused_while_mcp_are_off() {
    // enabled = false: ответ приходит без запуска дочернего процесса
    // и содержит текст отказа, а initialize и tools/list работают.
}

#[tokio::test]
async fn flipping_the_switch_mid_connection_starts_serving() {
    // На одном соединении: отказ, затем enabled.store(true), затем
    // тот же вызов доходит до дочернего процесса и возвращает его ответ.
}

#[tokio::test]
async fn the_mcp_port_does_not_get_our_tools_injected() {
    // Rules::for_mcp: в ответе tools/list нет project_status/pull/push,
    // а аргументы вызова уходят дочернему процессу без connection_args.
}
```

- [ ] **Шаг 2: тесты падают** (в CI): `Rules` не собирается, полей нет.
- [ ] **Шаг 3: гейт.** В `handle_request` перед разбором маршрута: если `!enabled.load(Relaxed)` и запрос — `tools/call`, вернуть `jsonrpc::text_reply(&id, "MCP выключены в 1c-connect: включите переключатель в окне", true)`. `initialize`, `tools/list`, уведомления и отмены проходят как сейчас: соединение Zed остаётся живым.
- [ ] **Шаг 4: `inject`.** `rewrite_reply`/`rewrite_list_reply` добавляют `our_tools()` только при `inject == true`; `OurCall` при `inject == false` невозможен (инструментов нет в списке), на всякий случай отвечает тем же текстом отказа с именем инструмента. `defaults`/`connection_args` для порта mcp пустые.
- [ ] **Шаг 5: один путь.** В `session::serve` порт `ports.mcp` идёт через `pipe_jsonrpc` с `Rules::for_mcp`; `pipe_process` остаётся в коде (им пользуется `ops_host`/`local`), но из `serve` уходит. Строки, не являющиеся JSON-RPC (баннеры дочернего процесса), должны проходить насквозь — это уже поведение `Forward`.
- [ ] **Шаг 6: лимит сеансов строкой в журнал.** Ответ инструмента уходит агенту как есть (это уже так), но если в тексте ответа есть «превышено предельное количество подключений», мост добавляет в журнал `Warn`: «агент не вошёл в базу: <текст платформы>; если 1С открыта у вас — закройте её». Тест `a_session_limit_answer_adds_one_journal_line`: фиктивный дочерний процесс отвечает этим текстом, в журнале ровно одна такая строка, а ответ агенту не изменён.
- [ ] **Шаг 7: тесты проходят** в CI (Windows и Linux).
- [ ] **Шаг 8: коммит.**

```bash
git add setup/1c-connect/src/bridge.rs setup/1c-connect/src/session.rs
git commit -m "bridge: выключенные MCP отвечают отказом на вызове; порт mcp-1c идёт тем же разбором строк"
```

---

### Task 3: Локальный транспорт — два слушателя на петле

**Files:**
- Create: `setup/1c-connect/src/listen.rs`
- Modify: `setup/1c-connect/src/session.rs` (`serve` L138 превращается в тип `Serve`, `start` L104 выбирает транспорт по `cfg.host.mode`)
- Modify: `setup/1c-connect/src/lib.rs` (объявить модуль)
- Test: inline `mod tests` в `listen.rs`

**Interfaces:**
- Consumes: `bridge::Rules::for_mcp`/`for_vrunner` (задача 2), `job::group` (задача 1).
- Produces:
  - `session::Serve` — общий разбор «порт → мост», не зависящий от вида потока:
    `Serve::new(cfg: &Cfg, kit: &Kit, log: Sink, enabled: Arc<AtomicBool>) -> Serve`,
    `Serve::dispatch<S>(&self, port: u16, stream: S) -> Option<tokio::task::JoinHandle<()>> where S: AsyncRead + AsyncWrite + Unpin + Send + 'static`;
    туннель зовёт её как `move |inc| { serve.dispatch(inc.port, inc.channel.into_stream()); }`;
  - `listen::run(serve: Arc<session::Serve>, ports: cfg::Ports, on_event: impl Fn(common::Event), stop: tokio::sync::watch::Receiver<bool>) -> anyhow::Result<()>`.

- [ ] **Шаг 1: падающие тесты** в `listen.rs`:

```rust
#[tokio::test]
async fn both_listeners_come_up_and_a_connection_reaches_the_bridge() {
    // Свободные порты, фиктивный Serve, подключение на порт mcp
    // доходит до dispatch (счётчик вызовов), событие Ok про включённые MCP.
}

#[tokio::test]
async fn a_busy_port_fails_the_whole_session_and_frees_the_first_listener() {
    // Занять порт vrunner заранее: run возвращает Err с номером порта,
    // а порт mcp после возврата снова свободен (повторный bind удаётся).
}

#[tokio::test]
async fn stop_closes_accepted_connections() {
    // Принятое соединение живо; после stop оно закрыто (чтение даёт 0 байт).
}
```

- [ ] **Шаг 2: тесты падают** в CI: модуля нет.
- [ ] **Шаг 3: реализация.** `run` биндит `TcpListener` на `127.0.0.1:ports.mcp`, затем на `127.0.0.1:ports.vrunner`; ошибка второго бинда закрывает первый и возвращает `Err` с номером порта. Дальше `select!` по двум `accept()` и `stop.changed()`. Каждое принятое соединение уходит в `serve.dispatch(port, stream)`, полученный `JoinHandle` кладётся в список; по `stop` список обходится, задачи снимаются, `run` возвращает `Ok(())`. Порт `web` в локальном режиме не слушается.
- [ ] **Шаг 4: выбор транспорта.** В `session::start`: при `cfg.host.mode == "local"` вместо `tunnel::run` зовётся `listen::run`; события те же (`Ok`/`Fail`), состояние `Retrying` в этом режиме не возникает. Текст события при подъёме — «MCP включены: порты 777N и 778N», без слова «туннель».
- [ ] **Шаг 5: тесты проходят** в CI.
- [ ] **Шаг 6: коммит.**

```bash
git add setup/1c-connect/src/listen.rs setup/1c-connect/src/session.rs setup/1c-connect/src/lib.rs
git commit -m "listen: локальный транспорт на петле, один разбор порт-мост для обоих транспортов"
```

---

### Task 4: Один владелец рабочего места

**Files:**
- Create: `setup/1c-connect/src/holder.rs`
- Modify: `setup/1c-connect/src/listen.rs` (текст занятого порта)
- Modify: `setup/1c-connect/src/tunnel.rs` (ограниченные повторы именно для отказа в пробросе)
- Modify: `setup/1c-connect/src/lib.rs`
- Test: inline в `holder.rs` (разбор образцов вывода) и в `tunnel.rs` (счётчик попыток)

**Interfaces:**
- Produces:
  - `holder::of_port(port: u16) -> Option<String>` — строка вида `1c-connect (pid 812)` или `None`; на Linux зовёт `ss -ltnp`, на Windows `netstat -ano` плюс `tasklist /FI "PID eq N"` (тот же приём, что уже применяет `bridge::configurator_count`);
  - `holder::parse_ss(out: &str, port: u16) -> Option<String>`;
  - `holder::parse_netstat(out: &str, port: u16) -> Option<u32>`.

- [ ] **Шаг 1: падающие тесты.** Образцы вывода берутся в тесте как строковые константы: строка `ss` вида `LISTEN 0 4096 127.0.0.1:7771 0.0.0.0:* users:(("1c-connect",pid=812,fd=9))` даёт `Some("1c-connect (pid 812)")` для порта 7771 и `None` для 7772; строка `netstat` вида `TCP 127.0.0.1:7771 0.0.0.0:0 LISTENING 812` даёт `Some(812)` для 7771 и `None` для 7781. Третий тест — `forwarding_refused_stops_after_three_tries`: фиктивная функция проброса, всегда отказывающая, даёт ровно три попытки и терминальный `Fail` с текстом про занятое рабочее место.
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: `holder`.** Разбор по строкам: на Linux искать `127.0.0.1:<порт>` и вытаскивать имя и `pid=N`; на Windows найти строку `LISTENING` с нужным портом, взять последнее поле как идентификатор процесса, затем спросить имя у `tasklist`. Пустой или неожидаемый вывод — `None`, без паники.
- [ ] **Шаг 4: текст занятого порта.** `listen::run` при `AddrInUse` зовёт `holder::of_port`: с именем — «порт N занят процессом <имя> (pid N)», без имени — «порт N уже занят другим процессом».
- [ ] **Шаг 5: повторы в туннеле.** Отказ именно проброса считается отдельно от обрыва связи: не больше трёх попыток с паузой, затем терминальный `Fail` с текстом спеки — «рабочее место <devN> занято: другой коннектор (в среде или на другой машине) либо старый проброс спящей машины, который сам отпустится за три минуты». Обрыв связи повторяется как сейчас, без ограничения.
- [ ] **Шаг 6: тесты проходят** в CI.
- [ ] **Шаг 7: коммит.**

```bash
git add setup/1c-connect/src/holder.rs setup/1c-connect/src/listen.rs setup/1c-connect/src/tunnel.rs setup/1c-connect/src/lib.rs
git commit -m "holder: называем держателя порта; отказ в пробросе больше не повторяется бесконечно"
```

---

### Task 5: Переключатель включает MCP, а не порты

**Files:**
- Modify: `setup/1c-connect/src/gui/mod.rs` (`Ui` L87, `start_session`/`stop_session`/`toggle_link` L622-671, `save_host` L556)
- Modify: `setup/1c-connect/src/gui/panel.rs` (`card` L88, перезапуск сеанса L650)
- Modify: `setup/1c-connect/src/session.rs` (`State::label` L53 — слова по режиму)
- Test: inline в `gui/mod.rs` (фикстура `Ui::for_test`) и в `session.rs`

**Interfaces:**
- Consumes: `session::Serve::new(..., enabled)` (задача 3).
- Produces: `Ui.enabled: Arc<AtomicBool>` — живёт дольше сеанса; `gui::toggle_link` в локальном режиме меняет только флаг, в туннельном — как сейчас (поднимает и опускает сеанс), и флаг следует за `link.is_on()`.

- [ ] **Шаг 1: падающие тесты.** `in_local_mode_the_switch_only_flips_the_flag`: при `mode = "local"` и поднятом сеансе двойной `toggle_link` не меняет `session_gen`, а флаг проходит `false` и снова `true`. `card_titles_speak_about_mcp_not_the_tunnel`: `Off` даёт «MCP выключены», `Up` — «MCP включены», `Failed` — причину, и ни в одном заголовке нет слова «туннель». `saving_a_new_base_restarts_the_session_and_writes_one_journal_line`: `save_host` с другим `base_path` при поднятом сеансе увеличивает `session_gen` и пишет ровно одну строку «MCP перезапущены: база <путь>». `save_is_blocked_while_an_operation_runs`: при `ui.busy.is_some()` или `ui.progress.is_some()` «Сохранить» недоступна и `cfg` не меняется.
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: флаг.** `Ui.enabled` создаётся при старте приложения и передаётся в `Serve::new`; `stop_session`/`start_session` его не пересоздают. В локальном режиме сеанс поднимается при старте окна независимо от переключателя (слушатели живут всегда), а «подключаться при запуске» решает лишь начальное значение флага.
- [ ] **Шаг 4: слова.** `State::label` и заголовки карточки берут слова по `cfg.host.mode`: в локальном — «MCP выключены / Включаю… / MCP включены», в туннельном — как сейчас, но без слова «туннель» в заголовке карточки (в журнале подробность остаётся). Пункт трея — «Включить MCP» и «Выключить MCP».
- [ ] **Шаг 5: перезапуск по «Сохранить».** Как сегодня, плюс строка журнала «MCP перезапущены: база <путь>». Кнопка «Сохранить» заблокирована, пока `ui.busy.is_some()` или `ui.progress.is_some()`.
- [ ] **Шаг 6: тесты проходят** в CI.
- [ ] **Шаг 7: коммит.**

```bash
git add setup/1c-connect/src/gui/mod.rs setup/1c-connect/src/gui/panel.rs setup/1c-connect/src/session.rs
git commit -m "Переключатель означает обслуживание вызовов; слова карточки и трея говорят про MCP"
```

---

### Task 6: Поля «Параметров» по режиму

**Files:**
- Modify: `setup/1c-connect/src/gui/panel.rs` (`Fields` L42, `host_of` L128, `fields_differ` L147, отрисовка `Tab::Params` L491-547)
- Test: inline в `panel.rs`

**Interfaces:**
- Produces: `panel::shown_fields(mode: &str) -> &'static [&'static str]`; отрисовка и сравнение «изменилось ли» ходят только через него.

- [ ] **Шаг 1: падающие тесты.** `local_mode_hides_the_environment_fields`: `shown_fields("local")` равно `["base_path", "db_user", "db_password", "project"]`, а `shown_fields("tunnel")` имеет длину 7. `changing_a_hidden_field_does_not_enable_save`: в локальном режиме правка адреса среды, пароля или рабочего места не включает «Сохранить».
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: реализация.** `shown_fields` плюс отрисовка рядов по этому списку; скрытые поля переносятся из текущего `cfg` (как уже делает `save_host` для `dump`, `tools` и `mode`). Рабочее место остаётся в toml: от него считаются порты.
- [ ] **Шаг 4: тесты проходят** в CI.
- [ ] **Шаг 5: коммит.**

```bash
git add setup/1c-connect/src/gui/panel.rs
git commit -m "Параметры: в локальном режиме видны только поля базы и проекта"
```

---

### Task 7: Поиск платформы на обеих ОС

**Files:**
- Modify: `setup/1c-connect/src/ops_host.rs` (`find_1cv8` L205)
- Test: inline в `ops_host.rs`

**Interfaces:**
- Produces: `ops_host::find_1cv8() -> Option<PathBuf>` (подпись та же) и чистая функция
  `ops_host::pick_version(found: Vec<(String, PathBuf)>) -> Option<PathBuf>` — числовая сортировка версий, её и проверяют тесты.

- [ ] **Шаг 1: падающие тесты.** `versions_sort_as_numbers_not_as_text`: из `8.3.9.1`, `8.3.27.1786`, `8.5.1.1150` выбирается последняя (при текстовой сортировке победила бы `8.3.9.1`). `the_same_tree_seen_through_a_symlink_appears_once`: два пути с одинаковым `canonicalize` дают один результат.
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: реализация.** Переменная `ONEC_PLATFORM` перекрывает поиск целиком. Иначе на Windows — как сейчас (`C:\Program Files\1cv8`, `C:\Program Files (x86)\1cv8`, внутри `<версия>/bin/1cv8.exe`), на Linux — `/opt/1cv8/x86_64/<версия>/1cv8` и `/opt/1cv8t/x86_64/<версия>/1cv8t`; версии сравниваются покомпонентно как числа; пути с одинаковым `std::fs::canonicalize` складываются в один (в образе `/opt/1cv8` — ссылка на `/opt/1cv8t`).
- [ ] **Шаг 4: тесты проходят** в CI.
- [ ] **Шаг 5: коммит.**

```bash
git add setup/1c-connect/src/ops_host.rs
git commit -m "find_1cv8: Linux и учебная платформа, числовая сортировка версий, ONEC_PLATFORM"
```

---

### Task 8: Факты проверки по режиму

**Files:**
- Modify: `setup/1c-connect/src/verify.rs` (`ORDER` L12, `run` L105)
- Modify: `setup/1c-connect/src/gui/mod.rs` (читатель `verify::ORDER`, если он позиционный)
- Test: inline в `verify.rs`

**Interfaces:**
- Produces: `verify::order(mode: &str) -> Vec<&'static str>` вместо константы длиной 13; `run(ctx, link: Option<&str>)` собирает факты в этом порядке.

- [ ] **Шаг 1: падающие тесты.** `facts_follow_the_mode`: в туннельном режиме есть «туннель» и «Конфигуратор», в локальном вместо «туннеля» — «порты рабочего места». `every_fact_has_a_name_in_order`: длина вывода `run` равна длине `order(mode)` для обоих режимов.
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: реализация.** Общие факты — параметры, платформа, комплект, инструменты, vrunner, проект, база, публикация и расширение, Gitea, носитель. По режиму: туннельные — «туннель», «Конфигуратор»; локальные — «порты рабочего места» (слушает этот процесс либо свободны) и «в базе открыт сеанс» как сведение, не провал.
- [ ] **Шаг 4: тесты проходят** в CI.
- [ ] **Шаг 5: коммит.**

```bash
git add setup/1c-connect/src/verify.rs setup/1c-connect/src/gui/mod.rs
git commit -m "verify: набор фактов зависит от режима, порядок задаётся именами"
```

---

### Task 9: Сеанс без окна (`serve`) и стенд без своих мостов

**Files:**
- Modify: `setup/1c-connect/src/main.rs` (новая подкоманда рядом с `Connect`, L50-75)
- Modify: `config/1c/stand.sh` (убрать строки `socat` L329-332; `start` запускает `serve` под рабочим местом)
- Modify: `config/1c/test-stand.sh` (сам поднимает `serve` перед проверкой портов)
- Test: inline в `main.rs` (разбор аргументов) плюс живой прогон смоук-теста стенда

**Interfaces:**
- Consumes: `listen::run` через `session::start` (задача 3).
- Produces: `1c-connect serve` — поднимает сеанс и держит его до `Ctrl+C`/`SIGTERM`, печатая события в стандартный вывод; ничего не рисует и не требует рабочего стола.

- [ ] **Шаг 1: падающий тест.** В `main.rs`: `serve_is_a_subcommand` — `Cli::try_parse_from(["1c-connect", "serve"])` даёт `Cmd::Serve`; `serve_takes_no_flags_of_its_own` — лишний аргумент даёт ошибку разбора.
- [ ] **Шаг 2: тест падает** в CI.
- [ ] **Шаг 3: реализация.** `Cmd::Serve` повторяет тело `connect` (L222-247), но без вопросов и без вывода плана: `session::start`, затем ожидание `ctrl_c` либо `s.wait()`; на Linux дополнительно слушается `SIGTERM` (у tokio он есть в наборе `signal`), чтобы `docker stop` закрывал мосты штатно и уборка группы процессов срабатывала.
- [ ] **Шаг 4: стенд.** Из `stand.sh` уходят ровно четыре строки с `socat` (L329-332) и остаются обе подкоманды `mcp`/`vrunner` (их теперь зовёт мост коннектора). В `start` вместо `socat` под каждым рабочим местом запускается `su - dev$n -c "$HOME/dev1c/1c-connect serve"` с журналом в `$LOG/$u-connect.log`. Публикация и Apache на этом шаге **не трогаются**.
- [ ] **Шаг 5: смоук-тест стенда.** `test-stand.sh` перед проверками сам поднимает `serve`, если порт свободен, и гасит его в конце; проверки портов `777N`/`778N` остаются как есть.
- [ ] **Шаг 6: проверка живьём.** В контейнере: `docker exec dev1c test-stand` проходит; `ss -ltnp` показывает слушателя `1c-connect`, а не `socat`.
- [ ] **Шаг 7: коммит.**

```bash
git add setup/1c-connect/src/main.rs config/1c/stand.sh config/1c/test-stand.sh
git commit -m "serve: сеанс без окна; порты рабочего места у коннектора, socat из стенда убран"
```

---

### Task 10: Параметры публикации в `cfg`

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs` (`Host` L37-52, `validate` L100, `EXAMPLE`, `publication()` L150)
- Modify: все шесть читателей `host.base_url` (их перечисляет спека: мост, установка и снятие расширения, два факта проверки, веб-порт туннеля, перенос значения при сохранении)
- Test: inline в `cfg.rs`

**Interfaces:**
- Produces:
  - `Host.publication: String` — `"auto"` (умолчание) либо `"webserver"`;
  - `Host.http_port: Option<u16>` — необязательный порт своего httpd, умолчание `8080 + N`;
  - `Cfg::base_url(&self) -> String` — в `"auto"` считает `http://localhost:<порт>/hs/mcp-1c`, в `"webserver"` отдаёт поле;
  - `Cfg::http_port(&self) -> u16`;
  - существующий `Cfg::publication()` переименован в `Cfg::publication_path()` (он про путь в адресе, а не про режим публикации — иначе два разных смысла под одним именем).

- [ ] **Шаг 1: падающие тесты.** `auto_computes_the_base_url`: при `publication = "auto"` и рабочем месте `dev2` `base_url()` равен `http://localhost:8082/hs/mcp-1c`, а поле `host.base_url` при этом не меняется. `webserver_keeps_the_field`: при `publication = "webserver"` `base_url()` равен полю. `an_explicit_port_wins`: `http_port = 9000` даёт `http://localhost:9000/hs/mcp-1c`. `validate_rejects_an_unknown_publication`: значение `"iis"` отвергается с внятным текстом.
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: реализация.** Поля, `Default` (`publication = "auto"`, `http_port = None`), проверка значения, строки в `EXAMPLE` с пояснением на русском, `base_url()`/`http_port()`, переименование `publication()` в `publication_path()` вместе с его тестами.
- [ ] **Шаг 4: читатели.** Все шесть переводятся на `Cfg::base_url()`; поле `base_url` в toml остаётся обязательным и проверяется как сейчас (спека: у него шесть читателей, и пустая строка молча сломала бы их).
- [ ] **Шаг 5: тесты проходят** в CI.
- [ ] **Шаг 6: коммит.**

```bash
git add setup/1c-connect/src/cfg.rs setup/1c-connect/src
git commit -m "cfg: publication и http_port, Cfg::base_url по режиму публикации"
```

---

### Task 11: `host.publication` — свой httpd и дескриптор

**Files:**
- Create: `setup/1c-connect/src/publication.rs` (чистая сборка текстов и путей)
- Modify: `setup/1c-connect/src/ops_host.rs` (новая операция в списке `host()` L368 между `ProjectOp` и `Extension`)
- Modify: `setup/1c-connect/src/lib.rs`
- Test: inline в `publication.rs` (сравнение с эталоном) и в `ops_host.rs` (состояние операции)

**Interfaces:**
- Consumes: `Cfg::base_url`/`http_port` (задача 10), `job::group_std` (задача 1), `ops_host::find_1cv8` (задача 7).
- Produces:
  - `publication::vrd(cfg: &Cfg) -> String` — текст `default.vrd`;
  - `publication::httpd_conf(p: &Paths) -> String` — текст конфигурации httpd;
  - `publication::Paths { root: PathBuf, pub_dir: PathBuf, conf: PathBuf, pid: PathBuf, log: PathBuf, module: PathBuf, port: u16 }` и `Paths::of(cfg: &Cfg, kit: &Kit) -> Result<Paths, String>`;
  - `publication::start(p: &Paths) -> Result<(), String>` / `stop(p: &Paths) -> Result<(), String>`.

- [ ] **Шаг 1: падающие тесты.** `vrd_carries_the_mandatory_attributes`: в тексте есть `publishExtensionsByDefault="true"`, `reuseSessions="dontuse"`, `sessionMaxAge="1"`, `poolSize="1"`, `poolTimeout="1"` и `rootUrl="mcp-1c"`; `base="/"`. `httpd_conf_matches_the_spiked_sample`: текст совпадает с эталоном дословно (эталон — константа в тесте, взята со спайка). `httpd_listens_on_loopback_only`: в тексте ровно одна строка `Listen` и она начинается с `Listen 127.0.0.1:`. `only_the_five_modules_plus_the_platform`: в тексте пять строк `LoadModule` плюс `_1cws_module`.
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: тексты.** `default.vrd` (кодировка UTF-8, перевод строки `\n`):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<point xmlns="http://v8.1c.ru/8.2/virtual-resource-system"
		base="/" ib="File=&quot;<путь к базе>&quot;;Usr=&quot;<пользователь>&quot;;Pwd=&quot;<пароль>&quot;;"
		enable="true">
	<httpServices publishByDefault="true" publishExtensionsByDefault="true">
		<service name="MCPService" rootUrl="mcp-1c" enable="true"
				reuseSessions="dontuse" sessionMaxAge="1" poolSize="1" poolTimeout="1"/>
	</httpServices>
</point>
```

  Конфигурация httpd — ровно как в спайке, с подстановкой путей и порта:

```apache
ServerRoot "<корень httpd>"
ServerName localhost
Listen 127.0.0.1:<порт>
PidFile "<pid>"
ErrorLog "<журнал>"
LogLevel warn
LoadModule authz_core_module modules/mod_authz_core.so
LoadModule authz_host_module modules/mod_authz_host.so
LoadModule alias_module modules/mod_alias.so
LoadModule mime_module modules/mod_mime.so
LoadModule dir_module modules/mod_dir.so
LoadModule _1cws_module "<модуль платформы>"
ThreadsPerChild 16
DocumentRoot "<каталог публикации>"
<Directory "<каталог публикации>">
    AllowOverride All
    Options None
    Require all granted
    SetHandler 1c-application
    ManagedApplicationDescriptor "<каталог публикации>/default.vrd"
</Directory>
```

  Модуль платформы: `wsap24.dll`/`wsap24t.dll` рядом с найденным `1cv8.exe` на Windows, `wsap24t.so` в образе. Каталог публикации — `<каталог состояния коннектора>/pub` (на Windows `%LOCALAPPDATA%\dev1c\pub`, в образе `$HOME/dev1c/pub`): каталог базы может быть только для чтения, а публикации нужен свой.
- [ ] **Шаг 4: запуск.** `start` пишет оба файла, затем `httpd -f <conf> -k start` (Windows — `httpd.exe` из комплекта, Linux — системный `apache2`), процесс заводится через `job::group_std`, чтобы выключение MCP и выход гасили его вместе с остальным. `stop` — `-k stop` плюс проверка, что порт освободился. Пароль базы в журнал не печатается: строка операции говорит «HTTP-сервис базы поднят на 127.0.0.1:<порт>».
- [ ] **Шаг 5: операция.** `host.publication`: `state()` — отвечает ли `GET <base_url>/version` (в режиме `"webserver"` только проверка, без запуска); `plan()` — что будет сделано; `apply()` — записать дескриптор и поднять httpd. Операция зовётся при включении MCP, при «Сохранить» с изменившейся базой и из «Установить» — до `host.extension`, иначе расширение ставить некуда.
- [ ] **Шаг 6: тесты проходят** в CI.
- [ ] **Шаг 7: коммит.**

```bash
git add setup/1c-connect/src/publication.rs setup/1c-connect/src/ops_host.rs setup/1c-connect/src/lib.rs
git commit -m "host.publication: дескриптор с обязательными атрибутами и свой httpd под пользователем"
```

---

### Task 12: httpd в комплекте Windows

**Files:**
- Modify: `pack.sh` (`sha_of` L101-114 — новая сумма; строки комплекта `1c-connect` L139-147; рядом с `tools_row` L164-192 — новая `httpd_row`)
- Modify: `setup/1c-connect/src/ops_host.rs:176` (список каталогов комплекта, которые переносятся в `%LOCALAPPDATA%\dev1c`)
- Test: прогон `pack.sh` в CI (джоба `build`) плюс тест на список каталогов в `ops_host.rs`

**Interfaces:**
- Produces: строка `httpd/` в Windows-комплекте `1c-connect` (`bin`, `modules` — только пять нужных, `conf/mime.types`, `LICENSE`, `NOTICE`).

- [ ] **Шаг 1: падающий тест.** В `ops_host.rs`: `the_kit_dirs_include_httpd` — список переносимых каталогов содержит `httpd` рядом с `vrunner` и `git`.
- [ ] **Шаг 2: тест падает** в CI.
- [ ] **Шаг 3: `httpd_row`.** Скачать архив Apache Lounge, проверить SHA-256 закреплённой суммой (как `sha_of` делает для MinGit), распаковать, оставить `bin/*`, пять модулей (`mod_authz_core`, `mod_authz_host`, `mod_alias`, `mod_mime`, `mod_dir`), `conf/mime.types`, `LICENSE`, `NOTICE` — остальное выбросить (123 модуля, openssl, curl, lua не нужны). Неудача скачивания или несовпадение суммы — **падение сборки с внятным текстом**, а не молчаливый комплект без httpd.
- [ ] **Шаг 4: перенос в состояние.** В `ops_host` список каталогов комплекта дополняется `httpd`.
- [ ] **Шаг 5: проверка.** В CI джоба `build` собирает Windows-комплект: в манифесте есть `httpd/bin/httpd.exe`, `httpd/LICENSE`, `httpd/conf/mime.types`, и нет `httpd/modules/mod_ssl.so`.
- [ ] **Шаг 6: коммит.**

```bash
git add pack.sh setup/1c-connect/src/ops_host.rs
git commit -m "pack: переносимый httpd в комплекте Windows, пять модулей, лицензия, падение при недоступном архиве"
```

---

### Task 13: Публикация стенда переезжает к коннектору

**Files:**
- Modify: `config/1c/stand.sh` (`publish` L229-290 и его вызов из `start` L312-346)
- Modify: `Dockerfile.1c` (проверка демо-базы на этапе сборки, L258-268)
- Modify: `config/1c/test-stand.sh` (проверка HTTP-сервиса идёт по адресу коннектора)
- Test: живой прогон смоук-теста стенда и сборка образа в CI

- [ ] **Шаг 1: что остаётся.** `publish` в стенде нужен **только** сборке образа (проверка шаблона демо-базы на порту 8089) — там он и остаётся. Из `start` вызов публикации уходит: её ведёт `serve` коннектора через `host.publication`.
- [ ] **Шаг 2: `base_url` в toml стенда.** Строка `base_url` в `1c-connect.toml` (L159-168) приводится к виду, который даёт `Cfg::base_url()` в режиме `"auto"`: `http://localhost:808N/hs/mcp-1c` (без `/base`), и добавляется `publication = "auto"`.
- [ ] **Шаг 3: каталог публикации.** Коннектор пишет публикацию в `$HOME/dev1c/pub` (не в `/run`), поэтому `start` больше не готовит `$RUN/$u/pub` для рабочих мест.
- [ ] **Шаг 4: смоук-тест.** `test-stand.sh` проверяет `http://127.0.0.1:808N/hs/mcp-1c/version` (новый адрес) и то, что httpd поднял именно коннектор.
- [ ] **Шаг 5: проверка живьём.** `docker exec dev1c test-stand` проходит целиком; сборка образа в CI зелёная.
- [ ] **Шаг 6: коммит.**

```bash
git add config/1c/stand.sh config/1c/test-stand.sh Dockerfile.1c
git commit -m "Стенд больше не публикует базу рабочих мест: это делает коннектор"
```

---

### Task 14: Строка публикации в окне

**Files:**
- Modify: `setup/1c-connect/src/gui/mod.rs` (сбор `ops`/`ProjectView` для вкладки «Проект»)
- Modify: `setup/1c-connect/src/gui/panel.rs` (отрисовка строки)
- Test: inline в `gui/mod.rs`

- [ ] **Шаг 1: падающий тест.** `project_tab_shows_the_publication_row`: в обоих режимах при `publication = "auto"` среди строк вкладки есть «HTTP-сервис базы» с адресом `127.0.0.1:<порт>`; при `"webserver"` строка говорит «публикация задана вручную».
- [ ] **Шаг 2: тест падает** в CI.
- [ ] **Шаг 3: реализация.** Строка рядом с расширением, состояние берётся у операции `host.publication`, кнопка «Установить» её выполняет.
- [ ] **Шаг 4: тест проходит** в CI.
- [ ] **Шаг 5: коммит.**

```bash
git add setup/1c-connect/src/gui/mod.rs setup/1c-connect/src/gui/panel.rs
git commit -m "Проект: строка HTTP-сервиса базы рядом с расширением"
```

---

### Task 15: Подменю «Режим работы» и правка спеки окна

**Files:**
- Modify: `setup/1c-connect/src/gui/tray.rs` (`MenuAction` L33-46, `menu_action` L50, `build` L120-144, `on_menu` L198)
- Modify: `docs/superpowers/specs/2026-10-16-gpui-connect-design.md` (раздел про режим: переключателя в локальном режиме нет — неверно, он есть и означает MCP)
- Test: inline в `tray.rs`

- [ ] **Шаг 1: падающий тест.** `mode_submenu_has_two_checked_items`: в меню есть подменю «Режим работы» с пунктами «Через туннель» и «Локально», отмечен текущий; `choosing_a_mode_writes_the_toml_and_restarts_the_session`: выбор пишет `mode` и поднимает сеанс заново.
- [ ] **Шаг 2: тест падает** в CI.
- [ ] **Шаг 3: реализация.** `MenuAction::Mode(&'static str)`, подменю после «Проверить», запись через `gui::save_host` (режим — поле, которого нет в форме, поэтому пишется отдельно), затем перезапуск сеанса и строка журнала «режим работы: <какой>».
- [ ] **Шаг 4: правка спеки окна** тем же коммитом.
- [ ] **Шаг 5: тесты проходят** в CI.
- [ ] **Шаг 6: коммит.**

```bash
git add setup/1c-connect/src/gui/tray.rs docs/superpowers/specs/2026-10-16-gpui-connect-design.md
git commit -m "Трей: подменю «Режим работы»; спека окна приведена к решению про переключатель"
```

---

### Task 16: Теневой каталог платформы для vrunner на Windows

**Files:**
- Modify: `setup/1c-connect/src/ops_host.rs` (операция `host.tools` L157 — после раскладки инструментов)
- Test: inline в `ops_host.rs`

**Interfaces:**
- Produces: `ops_host::shadow_platform(platform: &Path, to: &Path) -> Result<PathBuf, String>` — жёсткие ссылки на все файлы каталога `bin` платформы с именами `1cv8.exe` и `1cv8c.exe` для учебных `1cv8t.exe`/`1cv8ct.exe`; при отказе ссылок (другой том) — копия. Плюс запись пути в `%APPDATA%\1C\1CEStart\1CEStart.cfg` строкой `InstalledLocation=`.

- [ ] **Шаг 1: падающие тесты.** `shadow_renames_the_training_binaries`: в теневом каталоге есть `1cv8.exe` и `1cv8c.exe`, и они те же файлы (по размеру и содержимому), что `1cv8t.exe`/`1cv8ct.exe`. `a_commercial_platform_needs_no_shadow`: если `1cv8.exe` уже есть, теневой каталог не создаётся. `1cestart_line_is_added_once`: повторный вызов не плодит строк.
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: реализация.** Только Windows (в образе это решено симлинками `/opt/1cv8`), только когда найденная платформа учебная.
- [ ] **Шаг 4: тесты проходят** в CI.
- [ ] **Шаг 5: коммит.**

```bash
git add setup/1c-connect/src/ops_host.rs
git commit -m "host.tools: теневой каталог платформы, чтобы vrunner нашёл учебную 1С без прав администратора"
```

---

### Task 17: Доставка ветки в контейнер (туннельный режим)

**Files:**
- Modify: `setup/1c-connect/src/project.rs` (`status` — две новые строки)
- Modify: `setup/1c-connect/src/jsonrpc.rs:264` (описание `project_status`)
- Modify: `config/1c/stand.sh` (новая подкоманда `project`)
- Test: inline в `project.rs` и живой прогон в контейнере

**Interfaces:**
- Produces: `1c-stand project <имя-репозитория> [status|pull|push <сообщение>]` — своя сторона круга внутри образа: знает `http://127.0.0.1:3000`, учётные данные рабочего места и ветку `devN`.

**Почему это не только про доставку.** В туннельном режиме правка агента
существует для человека **только после отправки**: `cf_load` грузит в базу
то, что лежит в каталоге проекта **на машине человека**, а туда файлы
попадают лишь через Gitea. Значит незакоммиченные и неотправленные правки
в контейнере загрузить в базу нельзя никаким вызовом — и агент обязан это
понимать, а не узнавать по молчанию базы (замечание заказчика 2026-10-20).
Поэтому правило говорится в трёх местах: в выводе `project_status`, в
описаниях инструментов и в скилле (задача 18).

- [ ] **Шаг 1: падающие тесты.** `status_names_the_repository_and_the_branch`: вывод `project::status` содержит строки «репозиторий: dev1c/<имя>» и «ветка рабочего места: devN». `status_warns_that_unsent_work_cannot_reach_the_base`: когда есть неотправленные коммиты или изменённые файлы, в выводе есть строка «эти правки в базу загрузить нельзя, пока они не отправлены»; на чистом дереве этой строки нет.
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: вывод инструмента.** Новые строки в `status`; описания в `jsonrpc.rs` дополняются: `project_status` — про имя репозитория, `project_pull` — «забирает то, что **отправлено** в Gitea; правки, лежащие у вас незакоммиченными, сюда не попадут», `project_push` — «пока не отправлено, человек правок не увидит и загрузить их в базу нельзя».
- [ ] **Шаг 4: подкоманда стенда.** `project`: при отсутствии `.git` в `$HOME/project` делает `clone --depth 1 -b dev$n http://dev$n:dev$n@127.0.0.1:3000/dev1c/<имя>.git`; дальше `pull --rebase` либо `commit -am <сообщение>` плюс `push`. Подкоманда `status` печатает ветку, число неотправленных коммитов и число изменённых файлов, а при любом ненулевом — ту же строку предупреждения. Пароль не печатается. Тексты — на языке 1С: «забираю файлы из Gitea», «отправляю файлы в Gitea».
- [ ] **Шаг 5: проверка живьём.** В контейнере: `1c-stand project UNF pull` приносит дерево (`ls src | head` непусто), правка файла и `project status` печатает предупреждение, `project push "проба"` отправляет ветку, Gitea показывает коммит, после чего `project status` предупреждения не печатает.
- [ ] **Шаг 6: коммит.**

```bash
git add setup/1c-connect/src/project.rs setup/1c-connect/src/jsonrpc.rs config/1c/stand.sh
git commit -m "project_status называет репозиторий и ветку; у стенда есть своя сторона круга"
```

---

### Task 18: Скилл про круг работы с базой

**Files:**
- Create: `config/skills/1c-project-round/SKILL.md` (каталог скиллов образа — положить рядом с существующими 39)
- Modify: сборка образа, если список скиллов перечислен явно
- Test: проверка, что скилл попадает в образ, и живой вопрос агенту

- [ ] **Шаг 1: куда кладём.** Найти, где лежат 39 скиллов образа и как они попадают внутрь (`grep -rn "skills" Dockerfile.1c config/`), и положить новый тем же способом.
- [ ] **Шаг 2: текст скилла.** По спеке (четвёртый шаг работ), ровно эти разделы: начало задачи (посмотреть состояние проекта, забрать ветку, работать с файлами, а не с базой); **главное правило — правка существует для человека только после отправки**; **и второе правило — из учебной базы образа не выгружать конфигурацию в общее дерево**: формат выгрузки станет новее, и рабочая база человека его не примет (разбор в спеке): пока файлы не закоммичены и не отправлены, загрузить их в базу нельзя никаким вызовом, потому что в базу грузится каталог проекта на машине человека; конец задачи (отправить ветку и загрузить конфигурацию в базу, иначе работа не сделана); разница режимов (в образе каталог общий, отправка — для истории, загрузка в базу всё равно нужна; через туннель отправка ещё и доставка, забирать ветку своей стороной круга — `1c-stand project`); отказы, которые агент встретит («занято», «закройте Конфигуратор», лимит сеансов учебной платформы) и что делать; цена шагов (выгрузка — минуты и десятки тысяч файлов, первая отправка — минуты; мерено на УНФ 2026-10-20: 66 958 файлов).
- [ ] **Шаг 3: проверка.** Скилл виден в образе; агент, спрошенный «как отдать правку человеку», отвечает по нему.
- [ ] **Шаг 4: коммит.**

```bash
git add config/skills/1c-project-round/SKILL.md
git commit -m "Скилл: круг работы с базой — откуда файлы и как правка попадает в 1С"
```

---

---

### Task 19: Отметка о платформе рядом с выгрузкой

**Files:**
- Modify: `setup/1c-connect/src/flow.rs` (выгрузка пишет отметку, загрузка её сверяет)
- Modify: `setup/1c-connect/src/ops_host.rs` (версия платформы из найденного пути — вспомогательная функция)
- Test: inline в `flow.rs`

**Interfaces:**
- Consumes: `ops_host::find_1cv8` и версию платформы из её пути (задача 7).
- Produces:
  - `flow::dump_mark(project: &Path) -> Option<(String, String)>` — чтение отметки `.dev1c-dump` (версия платформы, версия формата);
  - `flow::write_dump_mark(project: &Path, platform: &str, format: &str) -> Result<(), String>`;
  - `flow::dump_format(project: &Path) -> Option<String>` — атрибут `version` из `ConfigDumpInfo.xml`;
  - `flow::check_mark(tree_format: Option<&str>, ours: Option<&str>) -> Mark` где `enum Mark { Same, Older(String), Newer(String) }` — чистая функция сравнения, её и проверяют тесты.

- [ ] **Шаг 1: падающие тесты.**
  `same_version_loads_silently`: отметка и платформа совпадают — `Mark::Same`.
  `older_files_load_with_a_warning`: формат в дереве `2.20`, формат нашей платформы `2.21` — `Mark::Older`, и текст предупреждения содержит обе версии формата и слова про то, что после выгрузки из этой базы старая платформа дерево не примет.
  `newer_files_are_refused`: формат в дереве `2.21`, формат нашей платформы `2.20` — `Mark::Newer`, загрузка не начинается (это тот отказ, который платформа даёт текстом «Неизвестная версия формата 2.21», но до запуска конфигуратора).
  `format_versions_compare_as_numbers`: `2.9` старее `2.20`, то есть сравнение покомпонентное, а не текстовое.
  `a_missing_mark_is_not_a_failure`: отметки нет (дерево пришло из Gitea от другой машины) — загрузка идёт, в журнале строка «отметки о платформе нет, проверить формат не могу».
- [ ] **Шаг 2: тесты падают** в CI.
- [ ] **Шаг 3: запись отметки.** В конце «Выгрузить из 1С» (после `cf_decompile`, до отправки) пишется `.dev1c-dump` — две строки `platform=<версия>` и `format=<version из ConfigDumpInfo.xml>`. Файл попадает в репозиторий (он мал и нужен второй стороне), поэтому в `.gitignore` проекта его не добавлять.
- [ ] **Шаг 4: сверка перед загрузкой.** В начале «Загрузить в 1С» (до `cf_load`) сверка по `check_mark`: `Same` — молча; `Older` — строка `Warn` в журнал и загрузка продолжается; `Newer` — операция прекращается с текстом «эти файлы выгружены платформой <X>, ваша база работает на <Y> — загрузить нельзя».
- [ ] **Шаг 5: тесты проходят** в CI.
- [ ] **Шаг 6: коммит.**

```bash
git add setup/1c-connect/src/flow.rs setup/1c-connect/src/ops_host.rs
git commit -m "Отметка о платформе рядом с выгрузкой: новое в старую платформу не грузим, старое грузим с предупреждением"
```

---

### Task 20: Комплект в образе — один общий, только для чтения

**Files:**
- Modify: `Dockerfile.1c` (сборка комплекта по пути `/opt/dev1c-kit`)
- Modify: `config/1c/stand.sh` (`seed` перестаёт связывать файлы; рабочее место смотрит на общий комплект)
- Modify: `setup/1c-connect/src/ops_host.rs` (отметка версии инструментов — в состояние пользователя, не в комплект)
- Modify: `setup/1c-connect/src/single_instance.rs` (замок — в состояние пользователя)
- Test: inline там же плюс живой прогон на чистом томе

**Interfaces:**
- Consumes: `ops_host::tools_dir()` — состояние пользователя (на Linux `~/.local/share/dev1c`, на Windows `%LOCALAPPDATA%\dev1c`).
- Produces: правило «комплект только читают, состояние пишут», одинаковое для обеих ОС.

- [x] **Шаг 1: падающие тесты.** `the_lock_lives_in_the_state_not_in_the_kit`: путь замка начинается с каталога состояния и не лежит внутри комплекта. `the_tools_stamp_lives_in_the_state_not_in_the_kit`: то же для отметки версии. `a_read_only_kit_still_starts`: при комплекте, открытом только для чтения, запуск не падает (на Unix проверяется правами каталога во временной папке).
- [x] **Шаг 2: тесты падают** в CI.
- [x] **Шаг 3: состояние отделяется от комплекта.** Замок и отметка версии переезжают в `tools_dir()`; всё, что коннектор пишет, живёт там же, где публикация. В комплекте не создаётся ни одного файла.
- [x] **Шаг 4: комплект собирается в образ.** В `Dockerfile.1c` появляется `/opt/dev1c-kit` с теми именами, которых ждёт мост: `mcp-1c-linux-amd64`, `vrunner/bin/oscript`, `vrunner/lib`, `git/bin/git`, расширение. Связывание делается один раз на этапе сборки, а не при каждом запуске.
- [x] **Шаг 5: стенд упрощается.** Из `seed` уходит создание ссылок; рабочее место получает `ONEC_CONNECT_KIT=/opt/dev1c-kit` (в окружении входа и в запуске `serve`). Остальное в `seed` — toml, база, проект — остаётся.
- [x] **Шаг 6: проверка живьём.** На чистом томе: контейнер поднят, рабочее место ничего не связывает, `serve` поднимает порты и публикацию, вызов агента доходит до базы. Отдельно: комплект, смонтированный только для чтения, не ломает ни замок, ни отметку версии. — сделано частично: комплект и состояние проверены в живом контейнере (комплект собран руками теми же ссылками и закрыт на запись, бинарник из CI run 39427051112); на чистом томе после пересборки образа остаётся прогнать `test-stand` и вызов агента через туннель.
- [x] **Шаг 7: коммит.**

```bash
git add Dockerfile.1c config/1c/stand.sh setup/1c-connect/src/ops_host.rs setup/1c-connect/src/single_instance.rs
git commit -m "Комплект в образе: один общий только для чтения, состояние у пользователя"
```

---

### Task 21: Один вердикт на карточку и «Проверить», расчёт молча

**Files:**
- Modify: `setup/1c-connect/src/verify.rs` (`readiness`, новый `facts_for_readiness`)
- Modify: `setup/1c-connect/src/gui/mod.rs` (`run_readiness`, `Ev::Ready`, `Ev::Checks`, `Readiness`)
- Test: inline в обоих файлах

**Interfaces:**
- Produces: `verify::readiness(ctx: &Ctx, tunnel_state: Option<&str>) -> Vec<Check>` — тот же набор, что `verify::run`, но без `ctx.progress(Phase::Check)` и без строк в журнале.
- Produces: `verify::first_fail(checks: &[Check]) -> Option<Check>` — первый `!ok && !info`.
- Produces: `Ev::Ready(u64, Option<Check>)` — поколение и вердикт; `Ui::ready_gen: u64`, `Ui::ready_pending: bool`.

- [ ] **Шаг 1: падающие тесты.** `verify`: `readiness_has_the_same_facts_as_check` (имена фактов `readiness(&ctx, None)` равны именам `run(&ctx, None)`); `first_fail_skips_notes` (сведение между двумя провалами не выигрывает). `gui`: `a_finished_check_becomes_the_verdict` (`Ev::Checks` с провалом → `ui.ready` стал `Warn` с тем же `what`); `a_stale_generation_answer_is_dropped` (`Ev::Ready(1, …)` после `Ev::Ready(2, …)` не меняет `ui.ready`); `a_second_recompute_waits_for_the_first` (`run_readiness` при `ready_pending` не плодит поток, а помечает повтор).
- [ ] **Шаг 2: тесты падают** (`cargo test -p onec-connect --lib`).
- [ ] **Шаг 3: состав и молчание.** `verify::run` разбивается на сбор фактов и печать: сбор переезжает в `facts_for_readiness`, `run` остаётся обёрткой с `Phase::Check` и печатью, `readiness` зовёт сбор без фазы.
- [ ] **Шаг 4: вердикт один.** `Ev::Checks` кладёт `first_fail` в `ui.ready`; `run_readiness` считает поколение и пропускает расчёт, если один уже идёт.
- [ ] **Шаг 4а: сроки и события.** Каждому факту короткий срок (инструменты 15 с, сетевые 5 с), общий бюджет 20 с; не уложившийся факт — «не проверено за 20 с», не выдуманный провал. Инструмент пробы остаётся без сеанса «Предприятия» (`cache_show`). К событиям пересчёта добавляется показ окна, и там же перечитывается toml — правку файла руками иначе никто не заметит.
- [ ] **Шаг 5: тесты проходят.**
- [ ] **Шаг 6: коммит.** `git commit -m "Готовность и проверка считают одно и то же, расчёт молчит"`

---

### Task 22: Пустой пользователь базы — собственный факт

**Files:**
- Modify: `setup/1c-connect/src/verify.rs` (`db_user_fact`, место в наборе)
- Test: inline там же

**Interfaces:**
- Produces: `verify::db_user_fact(user: &str) -> Check` — чистая функция, без сети: пусто → `Check::fail("пользователь базы", "не задан", "заполните поле «Пользователь базы» и нажмите «Сохранить»")`.

- [ ] **Шаг 1: падающий тест.** `an_empty_db_user_fails_before_any_request`: `db_user_fact("")` не `ok`; `db_user_fact("Администратор")` `ok`; и `readiness` с пустым `db_user` отдаёт первым провалом именно его (а не публикацию).
- [ ] **Шаг 2: тест падает.**
- [ ] **Шаг 3: факт в наборе** — до фактов публикации и расширения, чтобы причина была верной.
- [ ] **Шаг 4: тест проходит.**
- [ ] **Шаг 5: коммит.** `git commit -m "Пустой пользователь базы — провал проверки, а не терпимость базы"`

---

### Task 23: Замок переключателя, трея и автозапуска

**Files:**
- Create: `setup/1c-connect/src/gui/gate.rs` (`cold_gate`)
- Modify: `setup/1c-connect/src/gui/panel.rs` (`Switch::disabled`, подсказка)
- Modify: `setup/1c-connect/src/gui/tray.rs` (пункт «Включить MCP»)
- Modify: `setup/1c-connect/src/gui/mod.rs` (`launch` — автозапуск, `save_host` — подтверждение)
- Modify: `setup/1c-connect/src/bridge.rs` (`DISABLED_TEXT` с причиной)
- Test: inline в `gate.rs`, `panel.rs`, `tray.rs`, `mod.rs`

**Interfaces:**
- Produces: `gui::gate::cold_gate(cfg: &Cfg, tools: &OpState, platform: Option<&Path>, mcp_in_kit: bool) -> Option<String>` — `None` = можно включать, `Some(причина)` = заперто. Чистая функция, без сети и без файловой системы: существование каталога базы и наличие платформы передаются аргументами.
- Produces: `bridge::disabled_text(reason: Option<&str>) -> String` — к «MCP выключены в 1c-connect» добавляет причину замка.

- [ ] **Шаг 1: падающие тесты.** `gate`: по тесту на каждое условие (`пустой db_user`, `файловая база без каталога`, `серверная база — не запирает`, `нет платформы`, `нет mcp-1c`, `инструменты не разложены`, `туннельный режим без адреса среды`, `всё на месте → None`). `panel`: `the_switch_is_locked_while_the_gate_holds`. `tray`: `the_tray_item_follows_the_same_gate`. `mod`: `autostart_does_not_enable_while_the_gate_holds`; `saving_away_a_cold_condition_asks_first`.
- [ ] **Шаг 2: тесты падают.**
- [ ] **Шаг 3: ворота.** `cold_gate` по разделу спеки; причина — короткая строка для подсказки.
- [ ] **Шаг 4: читатели.** Переключатель и пункт трея запираются; автозапуск не включает обслуживание; `save_host` при живом обслуживании и пропавшем условии идёт через тот же диалог, что смена базы; отказ агенту несёт причину.
- [ ] **Шаг 5: тесты проходят.**
- [ ] **Шаг 6: коммит.** `git commit -m "Переключатель заперт, пока включать нечего"`

---

### Task 24: Точка и заголовок по порядку разрешения

**Files:**
- Modify: `setup/1c-connect/src/gui/panel.rs` (`card`)
- Test: inline там же

**Interfaces:**
- Consumes: `gui::gate::cold_gate` (Task 23), `Readiness` (Task 21).
- Produces: `panel::card(progress: Option<&Running>, link: &State, mode: &str, ready: &Readiness, gate: Option<&str>, now: Instant) -> Card` — порядок разрешения: операция → `Failed` → замок → `Unknown` → провал → ок.

- [ ] **Шаг 1: падающие тесты.** `unknown_is_never_green`; `the_gate_wins_over_an_idle_state`; `a_running_operation_wins_over_the_gate`; `a_failed_bind_wins_over_the_gate`; `the_title_never_carries_the_reason` (заголовок не содержит текста причины ни в одном состоянии); `off_ignores_hot_facts` (выключено + `Warn` → серая «MCP выключены»).
- [ ] **Шаг 2: тесты падают.**
- [ ] **Шаг 3: карточка** по таблице спеки; причина уходит в подсказку и подвал, в заголовке только состояние.
- [ ] **Шаг 3а: трей тем же порядком.** `TrayState` получает четвёртое состояние (провал вердикта); при идущей операции побеждает «занято». Причина замка попадает в подсказку значка наравне с фазой.
- [ ] **Шаг 4: тесты проходят.**
- [ ] **Шаг 5: коммит.** `git commit -m "Заголовок называет состояние, причина живёт рядом"`

---

### Task 25: Адрес среды — поле в toml

**Files:**
- Modify: `setup/1c-connect/src/cfg.rs` (`Host::gitea`, `gitea_url`, `EXAMPLE`)
- Modify: `setup/1c-connect/src/ops_host.rs` (правка `origin` при расхождении)
- Modify: `config/1c/stand.sh` (`seed` дописывает `gitea` в существующий toml)
- Test: inline в `cfg.rs`, `project.rs`

**Interfaces:**
- Produces: `Cfg::gitea_url()` — `host.gitea`, если непусто; иначе прежний вывод «порт SSH минус два».

- [ ] **Шаг 1: падающие тесты.** `cfg`: `an_explicit_gitea_wins_over_the_derived_one`; `an_empty_gitea_keeps_the_old_derivation`; `example_prints_gitea_with_its_rule`. `project`: `the_remote_follows_the_explicit_gitea`.
- [ ] **Шаг 2: тесты падают.**
- [ ] **Шаг 3: поле и вывод.**
- [ ] **Шаг 4: стенд.** `seed` дописывает `gitea` в существующий toml (идемпотентно, не трогая остальное); у разложенного проекта правится `origin`.
- [ ] **Шаг 5: тесты проходят**, `bash -n config/1c/stand.sh`.
- [ ] **Шаг 6: коммит.** `git commit -m "Адрес среды задаётся полем, а не выводится из порта SSH"`

---

### Task 26: Подвал — одна надпись в две строки

**Files:**
- Modify: `setup/1c-connect/src/gui/panel.rs` (левый слот подвала)
- Modify: `setup/1c-connect/src/gui/mod.rs` (строка итога проверки — короткая)
- Modify: `docs/superpowers/specs/2026-10-18-dev1c-setup-window-design.md` (подвал всегда две строки)
- Test: inline в `panel.rs`

**Interfaces:**
- Consumes: `panel::foot_left_text(status, hint, kit_path) -> (String, bool)` — подпись та же.
- Produces: слот подвала фиксированной высоты 32 px: `line_clamp(2)`, `text_size(13)`, `line_height(16)`.

- [ ] **Шаг 1: падающий тест.** `the_check_status_line_stays_short`: строка итога проверки не содержит пути отчёта (путь уходит в журнал и подсказку), длина не больше 80 знаков на числах до 99.
- [ ] **Шаг 2: тест падает.**
- [ ] **Шаг 3: текст и слот.** Итог проверки — «Проблем: N» / «Проверка пройдена»; путь отчёта остаётся строкой журнала. Слот: две строки всегда, третья под многоточие, полный текст подсказкой.
- [ ] **Шаг 4: тест проходит**, спека окна поправлена (подвал не растёт по тексту).
- [ ] **Шаг 5: живьём.** Снимок из образа: обе строки целиком, высота подвала не прыгает.
- [ ] **Шаг 6: коммит.** `git commit -m "Подвал: две строки постоянной высоты, путь отчёта — в журнал"`

---

### Task 27: Журнал — для человека, не для отладки

**Files:**
- Modify: `setup/1c-connect/src/gui/mod.rs` (запись «только в файл»)
- Modify: `setup/1c-connect/src/gui/window.rs` (строки про геометрию и обмеры)
- Test: inline в `mod.rs`

**Interfaces:**
- Produces: `gui::note_file(kit: &Kit, line: &Line)` — пишет в `1c-connect.log` и не трогает список окна (`note_line` остаётся для человеческих строк).

- [ ] **Шаг 1: падающий тест.** `note_file_writes_the_file_and_not_the_window`: после вызова файл содержит строку, `ui.log` пуст.
- [ ] **Шаг 2: тест падает.**
- [ ] **Шаг 3: канал и переезд.** Диагностика геометрии (`work_area`/`flyout_origin`, низ флайаута за рабочей областью) уходит через `note_file`.
- [ ] **Шаг 4: тест проходит.**
- [ ] **Шаг 5: живьём.** Открыть и закрыть окно десять раз: в журнале окна нет ни одной служебной строки, в файле диагностика есть.
- [ ] **Шаг 6: коммит.** `git commit -m "Служебные строки — в файл журнала, окно их не показывает"`

---

### Task 28: «Выбрать…» становится значком

**Files:**
- Create: `setup/1c-connect/icons/folder.svg`
- Modify: `setup/1c-connect/src/gui/panel.rs` (`pick`)
- Test: inline там же

**Interfaces:**
- Consumes: приём `menu.svg` — `include_bytes!` плюс `Icon::default().data(...)`.

- [ ] **Шаг 1: падающий тест.** `the_picker_button_has_no_label_and_keeps_its_tooltip` — чистая функция подписи/подсказки кнопки выбора (`pick_tooltip() -> &str`), чтобы проверяемое не зависело от рисования.
- [ ] **Шаг 2: тест падает.**
- [ ] **Шаг 3: значок.** `folder.svg` в тех же пропорциях, что `menu.svg`; кнопка — квадрат высоты `h_8`, подпись «Выбрать…» уходит в подсказку.
- [ ] **Шаг 4: тест проходит.**
- [ ] **Шаг 5: живьём.** Снимок из образа: путь в поле «База» виден целиком.
- [ ] **Шаг 6: коммит.** `git commit -m "Кнопки выбора каталога — значком, ширина уходит полю"`

---

### Task 29: На Linux углы прямые

**Files:**
- Modify: `setup/1c-connect/src/gui/panel.rs` (корень панели)
- Modify: `setup/1c-connect/src/gui/window.rs` (`window_background`)
- Modify: `docs/superpowers/specs/2026-10-18-dev1c-setup-window-design.md` (про Linux)
- Test: inline в `panel.rs`

**Interfaces:**
- Produces: `panel::window_radius() -> Option<Pixels>` — `None` на Linux, прежний радиус на Windows; общая тема не трогается (её носит окно `dev1c-setup`).

- [ ] **Шаг 1: падающий тест.** `the_window_has_no_radius_on_linux` — `window_radius()` пуст на Linux и непуст на Windows.
- [ ] **Шаг 2: тест падает.**
- [ ] **Шаг 3: прямые углы.** Корень панели без радиуса на Linux; `window_background` там снова `Opaque` — прозрачность нужна была только ради углов. Рамка `border_1` остаётся.
- [ ] **Шаг 4: тест проходит.**
- [ ] **Шаг 5: живьём.** Снимок из образа: прямые углы без стеклянных пятен; окно `dev1c-setup` не изменилось.
- [ ] **Шаг 6: коммит.** `git commit -m "На Linux окно без скруглений: прозрачность в углах там не работает"`


---

### Task 30: Экран для конфигуратора даёт коннектор

**Files:**
- Modify: `setup/1c-connect/src/ops_host.rs` (`with_display`, факт «экран для конфигуратора»)
- Modify: `setup/1c-connect/src/bridge.rs` (`vrunner_command`)
- Modify: `setup/1c-connect/src/verify.rs` (факт в наборе на Linux)
- Test: inline в `ops_host.rs` и `verify.rs`

**Interfaces:**
- Produces: `ops_host::display_prefix(has_display: bool, xvfb: Option<&Path>) -> Option<PathBuf>` — чистая функция: `None`, если дисплей уже есть или оборачивать нечем; путь к `xvfb-run`, если нужен и он найден. На Windows всегда `None`.
- Produces: `ops_host::with_display(cmd: Command) -> Command` — переписывает команду в `xvfb-run -a <программа> <аргументы>`, когда `display_prefix` сказал «надо».
- Produces: `verify::display_fact(prefix_needed: bool, xvfb: Option<&Path>) -> Check` — провал только когда дисплея нет и `xvfb-run` не найден.

- [ ] **Шаг 1: падающие тесты.** `ops_host`: `display_prefix_is_none_when_display_is_set`; `display_prefix_names_xvfb_when_display_is_missing`; `display_prefix_is_none_on_windows`; `with_display_keeps_program_and_args_in_order` (после обёртки первым идёт `xvfb-run`, затем `-a`, затем прежняя программа и её аргументы без перестановки). `verify`: `display_fact_fails_only_without_display_and_without_xvfb`.
- [ ] **Шаг 2: тесты падают** (`cargo test -p onec-connect --lib`).
- [ ] **Шаг 3: обёртка.** `with_display` применяется там, где поднимается конфигуратор: `bridge::vrunner_command` (через него идут `cf_decompile`/`cf_load`) и прямые вызовы `1cv8 DESIGNER` в `ops_host` (установка и снятие расширения).
- [ ] **Шаг 4: факт.** На Linux в набор проверки добавляется «экран для конфигуратора»: есть `DISPLAY` — ok; нет, но есть `xvfb-run` — ok с названием способа; нет ни того, ни другого — провал с подсказкой поставить `xvfb`.
- [ ] **Шаг 5: тесты проходят.**
- [ ] **Шаг 6: живьём.** В контейнере под dev3: `env -u DISPLAY 1c-connect export --dry-run` и настоящий `export` без `DISPLAY` в окружении проходят; в журнале видно, каким способом дан экран.
- [ ] **Шаг 7: коммит.** `git commit -m "Экран для конфигуратора на Linux даёт коннектор, а не вызывающий"`

---

### Task 31: Разовая команда не воюет со своим же коннектором

**Files:**
- Modify: `setup/1c-connect/src/verify.rs` (`ports_fact`, признак «свой»)
- Modify: `setup/1c-connect/src/ops_host.rs` (`holder` — образ процесса)
- Test: inline в `verify.rs`

**Interfaces:**
- Produces: `verify::ports_fact(mcp: u16, vrunner: u16, holders: &[(u16, Holder)]) -> Check`, где `Holder` различает «мы сами», «наш коннектор этого рабочего места» и «чужой процесс». Провал — только на третьем.

- [ ] **Шаг 1: падающие тесты.** `our_own_tray_holding_the_ports_is_not_a_failure`; `a_stranger_on_the_same_port_still_fails`; `a_free_port_passes`.
- [ ] **Шаг 2: тесты падают.**
- [ ] **Шаг 3: признак «свой».** Держатель считается своим, если это тот же исполняемый файл коннектора (как уже отличает свою копию `single_instance::same_image`), а не только если его `pid` равен нашему.
- [ ] **Шаг 4: тесты проходят.**
- [ ] **Шаг 5: живьём.** При работающем трее на dev3: `install -y`, `check` и `export --dry-run` проходят; факт портов зелёный и называет способ («заняты коннектором этого рабочего места»).
- [ ] **Шаг 6: коммит.** `git commit -m "Свои же порты разовой команде не мешают"`

## Что остаётся исполнителю

- Как делить `session.rs`: оставить `Serve` в нём или вынести в свой модуль — по размеру кода после задачи 3.
- Точный вид `Paths` для публикации на Linux (каталог состояния в образе) и способ найти `apache2`.
- Формат строки журнала при подборе хвостов прошлого запуска.
- Нужен ли `--depth 1` при клоне в контейнере, если агенту понадобится история (по умолчанию — нужен).

## Не-цели этого плана

- Развязка встречных правок человека и агента («моя версия» против «версии агента») — отдельное решение заказчика, в этот план не входит.
- Кнопка «Открыть Zed», списки баз, профили проектов, серверные базы.
