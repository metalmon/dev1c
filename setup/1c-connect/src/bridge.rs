//! What answers on the other side of a forwarded channel: for 777N, a fresh
//! mcp-1c process on stdio; for 779N, the local web server. Nothing listens
//! on this machine — the channel is the connection.

use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Start `cmd` with piped stdio and let `stream` speak to it: stream → stdin,
/// stdout → stream. When the child exits, the stream is shut down and
/// `on_exit` gets the exit code and the last lines of stderr.
pub async fn pipe_process<S>(stream: S, mut cmd: tokio::process::Command, on_exit: impl FnOnce(i32, String))
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            on_exit(-1, format!("не удалось запустить: {e}"));
            return;
        }
    };
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (mut sr, mut sw) = tokio::io::split(stream);
    let to_child = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut sr, &mut stdin).await;
        let _ = stdin.shutdown().await;
    });
    let from_child = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut stdout, &mut sw).await;
        let _ = sw.shutdown().await;
    });
    let err_tail = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        let lines: Vec<&str> = s.lines().collect();
        lines[lines.len().saturating_sub(5)..].join("\n")
    });
    let status = child.wait().await;
    let _ = from_child.await;
    to_child.abort();
    let tail = err_tail.await.unwrap_or_default();
    on_exit(status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1), tail);
}

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

/// Процесс `vrunner-mcp` — заглушка до операции `host.tools` (задача 6).
pub fn vrunner_command(_cfg: &crate::cfg::Cfg, _tools: &std::path::Path) -> tokio::process::Command {
    tokio::process::Command::new("oscript")
}

use crate::jsonrpc::{self, Frame, Framer, Route};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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

type Pending = Arc<Mutex<HashMap<String, Value>>>;
type Running = Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>;

fn key(id: &Value) -> String {
    id.to_string()
}

/// Мост 778N: `stream` ↔ процесс `vrunner-mcp`, построчно. `tools/list`
/// переписывается, чужие и наши инструменты перехватываются, остальное идёт
/// как есть. В канал пишет один писатель — за `out_tx`.
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
        let (out, pending, rules) = (out_tx.clone(), pending.clone(), rules.clone());
        tokio::spawn(async move {
            let mut f = Framer::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
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
    // Свои задачи дорабатывают: их ответы ещё могут дойти до агента.
    let ours: Vec<_> = running.lock().unwrap().drain().map(|(_, h)| h).collect();
    for h in ours {
        let _ = h.await;
    }
    to_child.abort();
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
    out: &mpsc::UnboundedSender<Vec<u8>>,
    pending: &Pending,
    running: &Running,
    stdin: &mut tokio::process::ChildStdin,
) -> Result<(), ()> {
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
                let _ = out.send(jsonrpc::text_reply(
                    &id,
                    &format!("{tool}: на машине запущен 1cv8.exe ({n}) — закройте Конфигуратор и клиентов 1С, затем повторите"),
                    true,
                ));
                Ok(())
            } else {
                forward(stdin, &line).await
            }
        }
        Route::OurCall { id, tool, args } => {
            let (out, project) = (out.clone(), rules.project.clone());
            let k = key(&id);
            let h = tokio::spawn(async move {
                let (text, is_error) = tokio::task::spawn_blocking(move || crate::project::call(&project, &tool, &args))
                    .await
                    .unwrap_or_else(|e| (format!("сорвалось: {e}"), true));
                let _ = out.send(jsonrpc::text_reply(&id, &text, is_error));
            });
            // Kept for the life of the connection, finished or not: a cancel
            // for an id the bridge answered must never reach the child.
            running.lock().unwrap().insert(k, h);
            Ok(())
        }
        // ponytail: abort drops the waiter; the blocking git call itself runs
        // to its end and frees the project mutex then. Enough for a pilot.
        Route::Cancel { id } => {
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
        Frame::Line(mut l) => {
            let is_pending = jsonrpc::is_response_with_id(&l).map_or(false, |id| pending.lock().unwrap().remove(&key(&id)).is_some());
            if is_pending {
                if let Some((_, out)) = jsonrpc::rewrite_list_reply(&l, allowed) {
                    return out;
                }
            }
            l.push(b'\n');
            l
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The test binary itself as the child: `__echo_child` below copies stdin
    /// to stdout when BRIDGE_ECHO is set, or dies with a message when
    /// BRIDGE_DIE is set — a portable `cat` and a portable failure.
    fn child(env: (&str, &str)) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "bridge::tests::__echo_child", "--nocapture", "--quiet"]);
        c.env(env.0, env.1);
        c
    }

    #[test]
    fn __echo_child() {
        if std::env::var("BRIDGE_ECHO").is_ok() {
            let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::stdout());
            std::process::exit(0);
        }
        if let Ok(msg) = std::env::var("BRIDGE_DIE") {
            eprintln!("{msg}");
            std::process::exit(3);
        }
        // A scripted MCP server: lists three tools, echoes the called tool's
        // name, prints a plain line on a cancel notification.
        if std::env::var("BRIDGE_SCRIPT").is_ok() {
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
                let reply = match v["method"].as_str() {
                    Some("tools/list") => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"cf_load"},{"name":"infobase_delete"},{"name":"task_status"}]}}),
                    Some("tools/call") => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":format!("child ran {}", v["params"]["name"].as_str().unwrap_or(""))}]}}),
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
        // Reads one line and leaves without answering.
        if std::env::var("BRIDGE_DIE_AFTER_READ").is_ok() {
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            std::process::exit(0);
        }
    }

    fn rules(allowed: &[&str], running: usize) -> Rules {
        let dir = std::env::temp_dir();
        let project = crate::project::Project { dir, git: "git".into(), branch: "dev1".into(), remote: "x".into(), login: "dev1".into() };
        Rules {
            allowed: allowed.iter().map(|s| s.to_string()).collect(),
            project: std::sync::Arc::new(project),
            configurator_running: std::sync::Arc::new(move || running),
        }
    }

    /// Drive the bridge: send the lines, wait, close; return the JSON replies,
    /// the raw output and the child's exit code.
    async fn talk(child_env: (&str, &str), rules: Rules, send: &[&str]) -> (Vec<serde_json::Value>, String, i32) {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, child(child_env), rules, move |code, err| {
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
    async fn child_death_still_answers_pending_list_with_our_tools() {
        let (replies, raw, code) = talk(("BRIDGE_DIE_AFTER_READ", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#]).await;
        assert_eq!(code, 0);
        assert_eq!(replies.len(), 1, "{raw}");
        assert_eq!(replies[0]["id"], 7);
        let names: Vec<&str> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["project_status", "project_pull", "project_push"]);
    }

    #[tokio::test]
    async fn process_sees_the_stream_and_answers_through_it() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_process(theirs, child(("BRIDGE_ECHO", "1")), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (mut r, mut w) = tokio::io::split(ours);
        w.write_all(b"{\"jsonrpc\":\"2.0\"}\n").await.unwrap();
        w.shutdown().await.unwrap();
        let mut got = String::new();
        r.read_to_string(&mut got).await.unwrap();
        // The test harness prints its own lines before the child echoes.
        assert!(got.ends_with("{\"jsonrpc\":\"2.0\"}\n"), "{got:?}");
        let (code, _) = rx.await.unwrap();
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn a_child_that_dies_at_once_closes_the_stream_and_reports_stderr() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_process(theirs, child(("BRIDGE_DIE", "база недоступна")), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (mut r, _w) = tokio::io::split(ours);
        let mut got = Vec::new();
        r.read_to_end(&mut got).await.unwrap(); // EOF: the stream closed
        let (code, err) = rx.await.unwrap();
        assert_eq!(code, 3);
        assert!(err.contains("база недоступна"), "{err:?}");
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
