//! What answers on the other side of a forwarded channel: for 777N, a fresh
//! mcp-1c process on stdio; for 779N, the local web server. Nothing listens
//! on this machine — the channel is the connection.

use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A plain relay to a local TCP port (the web server behind 779N). A port
/// that does not answer is reported, not swallowed: from the container it
/// looks exactly like a tunnel fault, and only this side can tell.
pub async fn pipe_tcp<S>(stream: S, port: u16) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| format!("веб-сервер на порту {port} не отвечает ({e}) — публикация базы не поднята"))?;
    let mut stream = stream;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
    Ok(())
}

/// `mcp-1c.exe` as the bridge starts it: the kit's binary, the developer's base.
pub fn mcp_command(cfg: &crate::cfg::Cfg, kit: &common::kit::Kit) -> tokio::process::Command {
    let exe = kit.path(if cfg!(windows) { "mcp-1c-windows-amd64.exe" } else { "mcp-1c-linux-amd64" });
    let mut c = tokio::process::Command::new(exe);
    c.args(["--base", &cfg.host.base_url, "--user", &cfg.host.db_user, "--password", &cfg.host.db_password, "--quiet"]);
    if !cfg.host.dump.is_empty() {
        c.args(["--dump", &cfg.host.dump]);
    }
    c.current_dir(&kit.root);
    c
}

/// Процесс `vrunner-mcp` из инструментов на машине: cwd — проект, библиотеки
/// oscript находит сам (`lib.system = ../lib`), git — из `tools` через PATH,
/// подключение к базе — переменными (не в командной строке).
pub fn vrunner_command(cfg: &crate::cfg::Cfg, tools: &std::path::Path) -> tokio::process::Command {
    use crate::ops_host::{git_exe, ib_switch, oscript_exe, vrunner_lib};
    let lib = vrunner_lib(tools);
    let mut c = tokio::process::Command::new(oscript_exe(tools));
    c.arg(lib.join("vanessa-runner").join("src").join("mcp.os"));
    c.current_dir(&cfg.host.project);
    // oscript first: vrunner-mcp runs long commands as background tasks by
    // re-executing `oscript` from PATH; then the kit's git.
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut path = oscript_exe(tools).parent().map(|p| p.as_os_str().to_owned()).unwrap_or_default();
    path.push(sep);
    path.push(git_exe(tools).parent().map(|p| p.as_os_str().to_owned()).unwrap_or_default());
    path.push(sep);
    path.push(std::env::var_os("PATH").unwrap_or_default());
    c.env("PATH", path);
    c.env("VRUNNER_IBCONNECTION", ib_switch(&cfg.host.base_path));
    c.env("VRUNNER_DBUSER", &cfg.host.db_user);
    c.env("VRUNNER_DBPWD", &cfg.host.db_password);
    c.env("NO_PROXY", cfg.host.server.rsplit_once(':').map(|(h, _)| h).unwrap_or(&cfg.host.server));
    c.env("GIT_TERMINAL_PROMPT", "0");
    c
}

use crate::jsonrpc::{self, Frame, Framer, Route};
use common::{Event, Sink};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Текст отказа на вызов инструмента, пока переключатель «MCP» в окне выключен.
pub const DISABLED_TEXT: &str = "MCP выключены в 1c-connect: включите переключатель в окне";

/// Правила моста на одно соединение.
pub struct Rules {
    pub allowed: Vec<String>,
    pub project: Arc<crate::project::Project>,
    pub configurator_running: Arc<dyn Fn() -> usize + Send + Sync>,
    /// Аргументы подключения, подставляемые в каждый вызов инструмента vrunner
    /// (см. `connection_args`). У порта mcp-1c пустые: туда подставлять
    /// нечего, база и так передана процессу в командной строке.
    pub defaults: Value,
    /// Переключатель «MCP включены» окна — общий на оба порта одного сеанса.
    /// Выключен — каждый вызов инструмента получает отказ, не доходя до
    /// ребёнка; `initialize`, `tools/list`, уведомления и отмены идут как
    /// обычно, иначе Zed остался бы без ответа и счёл сервер упавшим.
    pub enabled: Arc<AtomicBool>,
    /// Подмешивать ли свои инструменты (`project_status`/`pull`/`push`) в
    /// список и исполнять их здесь же: `true` у vrunner-mcp (778N), `false`
    /// у mcp-1c (777N) — там нет ни белого списка инструментов, ни наших.
    pub inject: bool,
}

impl Rules {
    /// Мост 778N: `vrunner-mcp`. Белый список из `[host] tools`, свои
    /// инструменты подмешиваются в `tools/list` и исполняются мостом.
    pub fn for_vrunner(
        allowed: Vec<String>,
        project: Arc<crate::project::Project>,
        configurator_running: Arc<dyn Fn() -> usize + Send + Sync>,
        defaults: Value,
        enabled: Arc<AtomicBool>,
    ) -> Rules {
        Rules { allowed, project, configurator_running, defaults, enabled, inject: true }
    }

    /// Мост 777N: `mcp-1c`. Голый проброс — ни белого списка инструментов,
    /// ни наших, ни аргументов подключения.
    pub fn for_mcp(enabled: Arc<AtomicBool>) -> Rules {
        Rules {
            allowed: Vec::new(),
            project: Arc::new(crate::project::Project {
                dir: std::path::PathBuf::new(),
                git: std::path::PathBuf::new(),
                branch: String::new(),
                remote: String::new(),
                login: String::new(),
            }),
            configurator_running: Arc::new(|| 0),
            defaults: Value::Null,
            enabled,
            inject: false,
        }
    }
}

/// Подключение к базе и её пользователь как аргументы инструмента vrunner:
/// `vrunner-mcp` читает их отсюда, а не из переменных окружения. Пустой
/// пароль не передаётся — иначе платформа получит `/P` с пустым значением.
pub fn connection_args(cfg: &crate::cfg::Cfg) -> Value {
    let mut a = serde_json::json!({"ibconnection": crate::ops_host::ib_switch(&cfg.host.base_path), "db-user": cfg.host.db_user});
    if !cfg.host.db_password.is_empty() {
        a["db-pwd"] = Value::String(cfg.host.db_password.clone());
    }
    a
}

/// Сколько процессов платформы запущено на машине: Конфигураторы и толстые
/// клиенты (`1cv8.exe`) и тонкие (`1cv8c.exe`) — файловую базу держит любой.
pub fn configurator_count() -> usize {
    if !cfg!(windows) {
        return 0;
    }
    let mut c = std::process::Command::new("tasklist");
    c.args(["/NH", "/FO", "CSV"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.output().map(|o| count_platform(&String::from_utf8_lossy(&o.stdout))).unwrap_or(0)
}

/// Строки `tasklist /FO CSV`, начинающиеся с процесса платформы.
fn count_platform(csv: &str) -> usize {
    csv.lines().filter(|l| l.starts_with("\"1cv8.exe\"") || l.starts_with("\"1cv8c.exe\"")).count()
}

type Pending = Arc<Mutex<HashMap<String, Value>>>;
type Running = Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>;

fn key(id: &Value) -> String {
    id.to_string()
}

/// Общий мост на оба порта: `stream` ↔ процесс (`vrunner-mcp` на 778N,
/// `mcp-1c` на 777N), построчно. У 778N `tools/list` переписывается, чужие
/// и наши инструменты перехватываются; у 777N (`Rules::for_mcp`) — голый
/// проброс. Переключатель «MCP» выключен — любой `tools/call` получает
/// отказ, не доходя до ребёнка. В канал пишет один писатель — за `out_tx`.
pub async fn pipe_jsonrpc<S>(stream: S, mut cmd: tokio::process::Command, rules: Rules, log: Sink, on_exit: impl FnOnce(i32, String))
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    crate::job::group(&mut cmd);
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
    // Bounded: a multi-megabyte report waits for the tunnel instead of piling
    // up in memory while it stalls.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    // Ответы ребёнка на tools/list, которых ещё ждут; ключ — id как JSON.
    let pending: Pending = Arc::default();
    let running: Running = Arc::default();
    let rules = Arc::new(rules);

    let writer = tokio::spawn(async move {
        while let Some(b) = out_rx.recv().await {
            if sw.write_all(&b).await.is_err() {
                break;
            }
        }
        let _ = sw.shutdown().await;
    });

    // Агент → ребёнок.
    let to_child = {
        let (out, pending, running, rules) = (out_tx.clone(), pending.clone(), running.clone(), rules.clone());
        tokio::spawn(async move {
            let mut f = Framer::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match sr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for fr in f.push(&buf[..n]) {
                    if handle_request(fr, &rules, &out, &pending, &running, &mut stdin).await.is_err() {
                        return;
                    }
                }
            }
            if let Some(fr) = f.finish() {
                let _ = handle_request(fr, &rules, &out, &pending, &running, &mut stdin).await;
            }
            let _ = stdin.shutdown().await;
        })
    };

    // Ребёнок → агент.
    let from_child = {
        let (out, pending, rules, log) = (out_tx.clone(), pending.clone(), rules.clone(), log.clone());
        tokio::spawn(async move {
            let mut f = Framer::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for fr in f.push(&buf[..n]) {
                    warn_on_session_limit(&fr, &log);
                    if out.send(rewrite_reply(fr, &rules.allowed, rules.inject, &pending)).await.is_err() {
                        return;
                    }
                }
            }
            if let Some(fr) = f.finish() {
                warn_on_session_limit(&fr, &log);
                let _ = out.send(rewrite_reply(fr, &rules.allowed, rules.inject, &pending)).await;
            }
        })
    };
    let err_tail = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        let lines: Vec<&str> = s.lines().collect();
        lines[lines.len().saturating_sub(5)..].join("\n")
    });
    // Канал закрылся, а ребёнок не вышел по EOF: несколько секунд на выход,
    // затем принудительно — иначе `oscript.exe` с Конфигуратором под ним
    // живут до перезапуска, а следующий вызов видит «закройте Конфигуратор».
    let mut to_child = to_child;
    let status = tokio::select! {
        s = child.wait() => {
            to_child.abort();
            s
        }
        _ = &mut to_child => match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
            Ok(s) => s,
            Err(_) => {
                let _ = child.start_kill();
                child.wait().await
            }
        },
    };
    let _ = from_child.await;
    // Ребёнок ушёл, не ответив на tools/list: наши инструменты агент получает
    // всё равно — но только у моста, который их вообще подмешивает.
    let unanswered: Vec<Value> = pending.lock().unwrap().drain().map(|(_, id)| id).collect();
    if rules.inject {
        for id in unanswered {
            let _ = out_tx.send(jsonrpc::list_reply(&id, jsonrpc::our_tools())).await;
        }
    }
    // Свои задачи дорабатывают: их ответы ещё могут дойти до агента.
    let ours: Vec<_> = running.lock().unwrap().drain().map(|(_, h)| h).collect();
    for h in ours {
        let _ = h.await;
    }
    drop(out_tx);
    let _ = writer.await;
    drop(job);
    let tail = err_tail.await.unwrap_or_default();
    on_exit(status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1), common::docker::redact_text(&tail));
}

/// Одна строка от агента: кому она. `Err` — ребёнок закрыл stdin.
async fn handle_request(
    fr: Frame,
    rules: &Arc<Rules>,
    out: &mpsc::Sender<Vec<u8>>,
    pending: &Pending,
    running: &Running,
    stdin: &mut tokio::process::ChildStdin,
) -> Result<(), ()> {
    // A line over the Framer's cap is never passed on, on either port: a
    // line the bridge did not buffer whole is not one it can gate or route.
    // The agent hears why (a parse error with id null) instead of silence.
    let line = match fr {
        Frame::Raw(b) => {
            if b.ends_with(b"\n") {
                let _ = out.send(jsonrpc::error_reply(&Value::Null, -32700, "мост 1c-connect: строка длиннее 8 МиБ отброшена")).await;
            }
            return Ok(());
        }
        Frame::Line(l) => l,
    };
    let off = !rules.enabled.load(Ordering::Relaxed);
    if !rules.inject {
        // 777N (mcp-1c): голая труба. Единственная проверка на всём порту —
        // переключатель на tools/call; всё остальное уходит ребёнку как
        // есть, какой бы ни была строка (не JSON, неизвестный метод, чужое
        // уведомление, батч) — так было устроено у бывшего `pipe_process`,
        // и здесь остаётся так же: этот порт не ведёт свой разбор JSON-RPC.
        if off {
            if let Some(id) = jsonrpc::tools_call_id(&line) {
                let _ = out.send(jsonrpc::text_reply(&id, DISABLED_TEXT, true)).await;
                return Ok(());
            }
        }
        let mut raw = line;
        raw.push(b'\n');
        return forward(stdin, &raw).await;
    }
    // 778N (vrunner-mcp): полный разбор — белый список, наши инструменты.
    // Переключатель — тем же механизмом, что обычный белый список: пустой
    // список на время разбора строки проводит любой tools/call через
    // Route::Disabled/Route::OurCall, и обе ветки ниже тут же отвечают
    // отказом по переключателю вместо обычного текста.
    let empty: Vec<String> = Vec::new();
    let allowed: &[String] = if off { &empty } else { &rules.allowed };
    match jsonrpc::route_request_with(&line, allowed, &rules.defaults) {
        Route::Drop => Ok(()),
        Route::Reject { id, code, why } => {
            let _ = out.send(jsonrpc::error_reply(&id, code, &format!("мост 1c-connect: {why}"))).await;
            Ok(())
        }
        Route::ListRequest { id, line } => {
            pending.lock().unwrap().insert(key(&id), id);
            forward(stdin, &line).await
        }
        Route::Disabled { id, tool } => {
            let text = if off { DISABLED_TEXT.to_string() } else { format!("инструмент {tool} выключен в 1c-connect.toml ([host] tools)") };
            let _ = out.send(jsonrpc::text_reply(&id, &text, true)).await;
            Ok(())
        }
        Route::PlatformCall { id, tool, line } => {
            let n = (rules.configurator_running)();
            if n > 0 {
                let _ = out
                    .send(jsonrpc::text_reply(
                        &id,
                        &format!("{tool}: на машине запущен 1cv8.exe ({n}) — закройте Конфигуратор и клиентов 1С, затем повторите"),
                        true,
                    ))
                    .await;
                Ok(())
            } else {
                forward(stdin, &line).await
            }
        }
        Route::OurCall { id, tool, args } => {
            if off {
                let _ = out.send(jsonrpc::text_reply(&id, DISABLED_TEXT, true)).await;
                return Ok(());
            }
            let (out, project) = (out.clone(), rules.project.clone());
            let k = key(&id);
            let h = tokio::spawn(async move {
                let (text, is_error) = tokio::task::spawn_blocking(move || crate::project::call(&project, &tool, &args))
                    .await
                    .unwrap_or_else(|e| (format!("сорвалось: {e}"), true));
                let _ = out.send(jsonrpc::text_reply(&id, &text, is_error)).await;
            });
            // Kept for the life of the connection, finished or not: a cancel
            // for an id the bridge answered must never reach the child.
            running.lock().unwrap().insert(k, h);
            Ok(())
        }
        // ponytail: abort drops the waiter; the blocking git call itself runs
        // to its end and frees the project mutex then. Enough for a pilot.
        Route::Cancel { id, line } => {
            let ours = {
                let map = running.lock().unwrap();
                map.get(&key(&id)).map(|h| h.abort()).is_some()
            };
            if ours {
                Ok(())
            } else {
                forward(stdin, &line).await
            }
        }
        Route::Forward(line) => forward(stdin, &line).await,
    }
}

/// Канонический текст уже с `\n` на конце.
async fn forward(stdin: &mut tokio::process::ChildStdin, line: &[u8]) -> Result<(), ()> {
    stdin.write_all(line).await.map_err(|_| ())
}

/// Одна строка от ребёнка: ответ на tools/list переписывается, но только у
/// моста, который подмешивает свои инструменты; остальное как есть.
fn rewrite_reply(fr: Frame, allowed: &[String], inject: bool, pending: &Pending) -> Vec<u8> {
    match fr {
        Frame::Raw(b) => b,
        Frame::Line(mut l) => {
            let is_pending = jsonrpc::is_response_with_id(&l).map_or(false, |id| pending.lock().unwrap().remove(&key(&id)).is_some());
            if inject && is_pending {
                if let Some((_, out)) = jsonrpc::rewrite_list_reply(&l, allowed) {
                    return out;
                }
            }
            l.push(b'\n');
            l
        }
    }
}

/// Текст лимита сеансов учебной (и не только) платформы — он же идёт агенту
/// как есть, но человек должен узнать об этом из журнала: забытый сеанс
/// «Предприятия» держит единственную лицензию файловой базы.
const SESSION_LIMIT_TEXT: &str = "превышено предельное количество подключений";

/// Если строка от ребёнка — ответ инструмента с текстом лимита сеансов,
/// пишет строку в журнал; иначе ничего не делает. Ответ агенту эта функция
/// не трогает и не меняет. Разбор как JSON — не бесплатный (ответы с
/// деревом метаданных бывают мегабайтами), поэтому сначала строка
/// проверяется на фразу в сырых байтах, и только если она нашлась — парсится.
fn warn_on_session_limit(fr: &Frame, log: &Sink) {
    let Frame::Line(l) = fr else { return };
    if !bytes_contain(l, SESSION_LIMIT_TEXT.as_bytes()) {
        return;
    }
    let Ok(v) = serde_json::from_slice::<Value>(l) else { return };
    let Some(text) = v.pointer("/result/content/0/text").and_then(Value::as_str) else { return };
    if !text.contains(SESSION_LIMIT_TEXT) {
        return;
    }
    // Текст платформы может нести строку подключения целиком (File=…;Usr=…;
    // Pwd=…) — в журнал она идёт без пароля, тем же способом, что и хвост
    // stderr дочернего процесса.
    let text = common::docker::redact_text(text);
    log(Event::Warn(format!("агент не вошёл в базу: {text}; если 1С открыта у вас — закройте её")));
}

/// Подстрока в сырых байтах — без разбора кодировки: фраза либо есть в
/// строке как есть, либо нет.
fn bytes_contain(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The test binary itself as the child: `__echo_child` below runs one of
    /// the scripted shapes picked by an env var — a portable fake mcp-1c/
    /// vrunner-mcp without a real build.
    fn child(env: (&str, &str)) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "bridge::tests::__echo_child", "--nocapture", "--quiet"]);
        c.env(env.0, env.1);
        c
    }

    #[test]
    fn __echo_child() {
        // A scripted MCP server: lists three tools, echoes the called tool's
        // name, prints a plain line on a cancel notification. NOISY: first
        // 200 KB of stderr, the way OneScript logs a long command.
        if std::env::var("BRIDGE_SCRIPT_NOISY").is_ok() {
            use std::io::Write;
            let line = "предупреждение OneScript, строка журнала номер N, ничего страшного\n".repeat(3000);
            std::io::stderr().write_all(line.as_bytes()).unwrap();
            std::io::stderr().flush().unwrap();
        }
        if std::env::var("BRIDGE_SCRIPT").is_ok() || std::env::var("BRIDGE_SCRIPT_NOISY").is_ok() || std::env::var("BRIDGE_SCRIPT_RECORD").is_ok() {
            use std::io::{BufRead, Write};
            let stdin = std::io::stdin();
            let mut out = std::io::stdout();
            for line in stdin.lock().lines() {
                let line = line.unwrap();
                let v: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let id = v["id"].clone();
                // A test wants proof the child never saw a particular call: a
                // marker line reaches the agent as plain (non-JSON-RPC) text,
                // exactly like a banner from a real child would.
                if v["method"].as_str() == Some("tools/call") && std::env::var("BRIDGE_SCRIPT_RECORD").is_ok() {
                    writeln!(out, "CALL {}", v["params"]["name"].as_str().unwrap_or("")).unwrap();
                    out.flush().unwrap();
                }
                let reply = match v["method"].as_str() {
                    Some("initialize") => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"script","version":"0"}}}),
                    Some("tools/list") => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"cf_load"},{"name":"infobase_delete"},{"name":"task_status"}]}}),
                    Some("tools/call") => {
                        let mut text = format!("child ran {}", v["params"]["name"].as_str().unwrap_or(""));
                        if std::env::var("BRIDGE_SCRIPT_ARGS").is_ok() {
                            text = format!("{text} args={}", v["params"]["arguments"]);
                        }
                        if std::env::var("BRIDGE_SCRIPT_LIMIT").is_ok() {
                            text = "ошибка сервера: превышено предельное количество подключений".into();
                        }
                        serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}})
                    }
                    Some("notifications/cancelled") => {
                        writeln!(out, "child saw cancel {}", v["params"]["requestId"]).unwrap();
                        out.flush().unwrap();
                        continue;
                    }
                    _ => continue,
                };
                writeln!(out, "{reply}").unwrap();
                out.flush().unwrap();
            }
            std::process::exit(0);
        }
        // Ignores EOF on stdin: a server that does not exit when the client goes.
        if std::env::var("BRIDGE_LINGER").is_ok() {
            let mut s = String::new();
            while std::io::stdin().read_line(&mut s).map(|n| n > 0).unwrap_or(false) {}
            std::thread::sleep(std::time::Duration::from_secs(60));
            std::process::exit(0);
        }
        // Reads one line and leaves without answering.
        if std::env::var("BRIDGE_DIE_AFTER_READ").is_ok() {
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            std::process::exit(0);
        }
    }

    fn rules(allowed: &[&str], running: usize) -> Rules {
        rules_with(allowed, running, Arc::new(AtomicBool::new(true)))
    }

    /// Как `rules`, но с заданным переключателем — чтобы тест мог выключать
    /// и включать MCP прямо посреди разговора.
    fn rules_with(allowed: &[&str], running: usize, enabled: Arc<AtomicBool>) -> Rules {
        let dir = std::env::temp_dir();
        let project = crate::project::Project { dir, git: "git".into(), branch: "dev1".into(), remote: "x".into(), login: "dev1".into() };
        Rules::for_vrunner(
            allowed.iter().map(|s| s.to_string()).collect(),
            std::sync::Arc::new(project),
            std::sync::Arc::new(move || running),
            Value::Null,
            enabled,
        )
    }

    /// Журнал, который ничего не делает — тестам, которым нужны только ответы агенту.
    fn no_log() -> Sink {
        Arc::new(|_: Event| {})
    }

    /// Drive the bridge: send the lines, wait, close; return the JSON replies,
    /// the raw output and the child's exit code.
    async fn talk(child_env: (&str, &str), rules: Rules, send: &[&str]) -> (Vec<serde_json::Value>, String, i32) {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, child(child_env), rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        for l in send {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        let (code, _) = rx.await.unwrap();
        (got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect(), got, code)
    }

    #[test]
    fn connection_env_is_passed_verbatim() {
        let mut cfg = crate::cfg::Cfg::default();
        cfg.host.base_path = "C:\\Bases\\Бух".into();
        cfg.host.db_password = "па\"роль".into();
        cfg.host.project = "C:\\proj\\Бух".into();
        cfg.host.server = "10.0.0.5:3393".into();
        let c = vrunner_command(&cfg, std::path::Path::new("T")).into_std();
        let env: std::collections::HashMap<_, _> =
            c.get_envs().filter_map(|(k, v)| v.map(|v| (k.to_string_lossy().to_string(), v.to_string_lossy().to_string()))).collect();
        assert_eq!(env["VRUNNER_IBCONNECTION"], "/FC:\\Bases\\Бух");
        assert_eq!(env["VRUNNER_DBUSER"], "Администратор");
        assert_eq!(env["VRUNNER_DBPWD"], "па\"роль");
        assert_eq!(env["NO_PROXY"], "10.0.0.5");
        assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
        let sep = std::path::MAIN_SEPARATOR;
        let bin = format!("T{sep}vrunner{sep}bin");
        let git = if cfg!(windows) { format!("T{sep}git{sep}cmd") } else { format!("T{sep}git{sep}bin") };
        // Background tasks of vrunner-mcp re-exec `oscript` from PATH; git is the bridge's own.
        assert!(env["PATH"].starts_with(&format!("{bin}{}{git}", if cfg!(windows) { ';' } else { ':' })), "{}", env["PATH"]);
        assert_eq!(c.get_current_dir().unwrap(), std::path::Path::new("C:\\proj\\Бух"));
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(args[0].ends_with("mcp.os"), "{args:?}");
    }

    #[test]
    fn connection_args_carry_the_base_and_its_user() {
        let mut cfg = crate::cfg::Cfg::default();
        cfg.host.base_path = "C:\\Bases\\Бух".into();
        cfg.host.db_user = "mcp".into();
        cfg.host.db_password = "p w".into();
        let a = connection_args(&cfg);
        assert_eq!(a["ibconnection"], "/FC:\\Bases\\Бух");
        assert_eq!(a["db-user"], "mcp");
        assert_eq!(a["db-pwd"], "p w");
        cfg.host.db_password = String::new();
        assert!(connection_args(&cfg).get("db-pwd").is_none(), "an empty password is not sent as a value");
    }

    #[tokio::test]
    async fn list_is_filtered_and_extended_through_the_bridge() {
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#]).await;
        assert_eq!(replies.len(), 1, "{raw}");
        let names: Vec<&str> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["cf_load", "project_status", "project_pull", "project_push"]);
    }

    #[tokio::test]
    async fn disabled_tool_never_reaches_the_child() {
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"infobase_delete","arguments":{}}}"#]).await;
        assert_eq!(replies.len(), 1, "{raw}");
        assert_eq!(replies[0]["result"]["isError"], true);
        assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("1c-connect.toml"));
        assert!(!raw.contains("child ran"));
    }

    #[tokio::test]
    async fn platform_call_is_refused_while_the_configurator_runs() {
        let req = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#;
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 2), &[req]).await;
        assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("закройте Конфигуратор"), "{raw}");
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[req]).await;
        assert_eq!(replies[0]["result"]["content"][0]["text"], "child ran cf_load", "{raw}");
    }

    #[tokio::test]
    async fn a_tool_call_is_refused_while_mcp_are_off() {
        // Ребёнок запускается сразу на соединении, даже при выключенном
        // переключателе (решение заказчика: простаивающий ребёнок не держит
        // сеанс 1С и умирает вместе с сеансом) — и отвечает на initialize/
        // tools/list. Единственное, что до него не должно дойти, — сам
        // вызов инструмента; "CALL cf_load" — метка, которую ребёнок пишет,
        // только когда и если увидел tools/call (см. BRIDGE_SCRIPT_RECORD).
        let enabled = Arc::new(AtomicBool::new(false));
        let (replies, raw, _) = talk(
            ("BRIDGE_SCRIPT_RECORD", "1"),
            rules_with(&["cf_load"], 0, enabled),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#,
            ],
        )
        .await;
        assert!(!raw.contains("CALL cf_load"), "ребёнок не должен был увидеть вызов: {raw}");
        let call = replies.iter().find(|r| r["id"] == 3).unwrap();
        assert_eq!(call["result"]["isError"], true);
        assert_eq!(call["result"]["content"][0]["text"], DISABLED_TEXT, "{raw}");
        assert!(replies.iter().any(|r| r["id"] == 1), "initialize должен был ответить: {raw}");
        assert!(replies.iter().any(|r| r["id"] == 2), "tools/list должен был ответить: {raw}");
    }

    #[tokio::test]
    async fn the_mcp_port_gates_only_tools_call_while_off() {
        // inject == false (777N, mcp-1c) + переключатель выключен: гейт на
        // tools/call — единственное, что этот порт делает. Неизвестный
        // метод и уведомление проходят насквозь, как и раньше у
        // pipe_process, и ответ на tools/call — только отказ, который
        // никогда не превращается в Forward/PlatformCall (их тут вообще
        // нет: этот путь не вызывает route_request_with).
        let rules = Rules::for_mcp(Arc::new(AtomicBool::new(false)));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, child(("BRIDGE_SCRIPT_RECORD", "1")), rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        for l in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            r#"{"jsonrpc":"2.0","method":"кто-то/вендорский"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#,
        ] {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        assert!(!got.contains("CALL cf_load"), "гейт должен был поймать tools/call: {got}");
        assert!(!got.contains("-32601"), "неизвестный метод не должен получать ответ моста: {got}");
        let replies: Vec<Value> = got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        assert!(replies.iter().any(|r| r["id"] == 1), "initialize должен был пройти насквозь: {got}");
        let call = replies.iter().find(|r| r["id"] == 2).unwrap();
        assert_eq!(call["result"]["content"][0]["text"], DISABLED_TEXT, "{got}");
    }

    #[tokio::test]
    async fn flipping_the_switch_mid_connection_starts_serving() {
        let enabled = Arc::new(AtomicBool::new(false));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, child(("BRIDGE_SCRIPT", "1")), rules_with(&["cf_load"], 0, enabled.clone()), no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        let call = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#;
        w.write_all(call).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        enabled.store(true, Ordering::Relaxed);
        w.write_all(call).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        let replies: Vec<Value> = got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        assert_eq!(replies.len(), 2, "{got}");
        assert_eq!(replies[0]["result"]["content"][0]["text"], DISABLED_TEXT, "{got}");
        assert_eq!(replies[1]["result"]["content"][0]["text"], "child ran cf_load", "{got}");
    }

    #[tokio::test]
    async fn the_mcp_port_does_not_get_our_tools_injected() {
        let mut cmd = child(("BRIDGE_SCRIPT", "1"));
        cmd.env("BRIDGE_SCRIPT_ARGS", "1");
        let rules = Rules::for_mcp(Arc::new(AtomicBool::new(true)));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, cmd, rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        for l in [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cf_load","arguments":{"x":1}}}"#,
        ] {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        let replies: Vec<Value> = got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let list = replies.iter().find(|r| r["id"] == 1).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(!names.iter().any(|n| n.starts_with("project_")), "{names:?}");
        let call = replies.iter().find(|r| r["id"] == 2).unwrap();
        let text = call["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"x\":1"), "{text}");
        assert!(!text.contains("ibconnection") && !text.contains("db-user"), "{text}");
    }

    #[tokio::test]
    async fn a_session_limit_answer_adds_one_journal_line() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let log: Sink = Arc::new(move |e: Event| {
            if let Event::Warn(w) = e {
                seen2.lock().unwrap().push(w);
            }
        });
        let mut cmd = child(("BRIDGE_SCRIPT", "1"));
        cmd.env("BRIDGE_SCRIPT_LIMIT", "1");
        let (replies, raw, _) = {
            let (ours, theirs) = tokio::io::duplex(1 << 16);
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(pipe_jsonrpc(theirs, cmd, rules(&["task_status"], 0), log, move |code, err| {
                let _ = tx.send((code, err));
            }));
            let (r, mut w) = tokio::io::split(ours);
            w.write_all(br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"task_status","arguments":{}}}"#).await.unwrap();
            w.write_all(b"\n").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            w.shutdown().await.unwrap();
            let mut got = String::new();
            tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
            let _ = rx.await;
            (got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect::<Vec<Value>>(), got, 0)
        };
        assert_eq!(replies.len(), 1, "{raw}");
        let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(SESSION_LIMIT_TEXT), "{text}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].starts_with("агент не вошёл в базу") && seen[0].contains(SESSION_LIMIT_TEXT), "{}", seen[0]);
    }

    #[tokio::test]
    async fn our_tool_runs_in_its_own_task_and_the_child_keeps_answering() {
        // project_status against a directory that is not a repo fails fast
        // with isError — enough to prove the path; git itself is tested in project.rs.
        let (replies, raw, _) = talk(
            ("BRIDGE_SCRIPT", "1"),
            rules(&["task_status"], 0),
            &[
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"project_status","arguments":{}}}"#,
                r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"task_status","arguments":{}}}"#,
            ],
        )
        .await;
        assert_eq!(replies.len(), 2, "{raw}");
        assert!(replies.iter().any(|r| r["id"] == 5 && r["result"]["content"][0]["text"] == "child ran task_status"));
        assert!(replies.iter().any(|r| r["id"] == 4 && r["result"]["content"][0]["type"] == "text"));
    }

    #[tokio::test]
    async fn cancel_of_our_task_is_swallowed_and_of_a_foreign_one_forwarded() {
        let (replies, raw, _) = talk(
            ("BRIDGE_SCRIPT", "1"),
            rules(&[], 0),
            &[
                r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"project_status","arguments":{}}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":6}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":99}}"#,
            ],
        )
        .await;
        assert!(raw.contains("child saw cancel 99"), "{raw}");
        assert!(!raw.contains("child saw cancel 6"), "{raw}");
        // Ours either finished first or was cancelled — never two answers for 6.
        assert!(replies.iter().filter(|r| r["id"] == 6).count() <= 1);
    }

    #[tokio::test]
    async fn a_child_that_ignores_eof_is_killed_when_the_channel_closes() {
        let t0 = std::time::Instant::now();
        let (_replies, _raw, code) = talk(("BRIDGE_LINGER", "1"), rules(&[], 0), &[r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#]).await;
        assert_ne!(code, 0, "a killed child does not exit 0");
        assert!(t0.elapsed() < std::time::Duration::from_secs(20), "killed within the grace period, not after 60 s");
    }

    #[test]
    fn thin_client_counts_as_a_running_platform() {
        let csv = "\"1cv8c.exe\",\"1234\",\"Console\",\"1\",\"100 K\"\n\"1cv8.exe\",\"77\",\"Console\",\"1\",\"1 K\"\n\"notepad.exe\",\"1\",\"Console\",\"1\",\"1 K\"\n";
        assert_eq!(count_platform(csv), 2);
        assert_eq!(count_platform("INFO: No tasks are running which match the specified criteria.\n"), 0);
    }

    #[tokio::test]
    async fn oversized_and_junk_agent_lines_get_an_error_reply() {
        let big = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"pad\":\"{}\"}}", "x".repeat(crate::jsonrpc::LINE_LIMIT + 1));
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&[], 0), &[&big, "junk"]).await;
        assert!(!raw.contains("child ran"), "{}", &raw[..raw.len().min(200)]);
        let errors: Vec<_> = replies.iter().filter(|r| r["error"]["code"] == -32700).collect();
        assert_eq!(errors.len(), 2, "{replies:?}");
        assert!(errors.iter().all(|r| r["id"].is_null()));
    }

    #[tokio::test]
    async fn child_death_still_answers_pending_list_with_our_tools() {
        let (replies, raw, code) = talk(("BRIDGE_DIE_AFTER_READ", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#]).await;
        assert_eq!(code, 0);
        assert_eq!(replies.len(), 1, "{raw}");
        assert_eq!(replies[0]["id"], 7);
        let names: Vec<&str> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["project_status", "project_pull", "project_push"]);
    }

    #[tokio::test]
    async fn a_dead_web_server_is_reported_not_swallowed() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l); // nothing listens there any more
        let (_ours, theirs) = tokio::io::duplex(64);
        let r = pipe_tcp(theirs, port).await;
        assert!(r.is_err(), "connect to a closed port must be an error");
        assert!(r.unwrap_err().contains(&port.to_string()));
    }

    #[tokio::test]
    async fn tcp_side_is_a_plain_relay() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut b = [0u8; 5];
            s.read_exact(&mut b).await.unwrap();
            s.write_all(b"pong!").await.unwrap();
        });
        let (ours, theirs) = tokio::io::duplex(1024);
        tokio::spawn(pipe_tcp(theirs, port));
        let (mut r, mut w) = tokio::io::split(ours);
        w.write_all(b"ping!").await.unwrap();
        let mut b = [0u8; 5];
        r.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong!");
    }
}
