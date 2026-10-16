//! Локальный вызов `vrunner-mcp` без туннеля: окно и CLI зовут те же
//! инструменты, что агент, — процесс на время действия, синхронно
//! (`background: false`), под тем же мьютексом проекта (берёт вызывающий).

use crate::jsonrpc::{Frame, Framer};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::time::Duration;

#[derive(Debug)]
pub struct Reply {
    pub text: String,
    pub is_error: bool,
}

/// Результат вызова инструмента: тексты `content` склеены, `isError` как есть;
/// ошибка JSON-RPC — тоже `is_error`. Не ответ — `None`.
pub fn parse_reply(v: &Value) -> Option<Reply> {
    if let Some(e) = v.get("error") {
        return Some(Reply { text: e.get("message").and_then(Value::as_str).unwrap_or("ошибка").to_string(), is_error: true });
    }
    let r = v.get("result")?;
    let text = r
        .get("content")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    Some(Reply { text, is_error: r.get("isError").and_then(Value::as_bool).unwrap_or(false) })
}

fn line(v: Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(&v).unwrap();
    b.push(b'\n');
    b
}

/// Процесс на время одного вызова: `initialize` → `notifications/initialized`
/// → `tools/call`. Строки `notifications/message` идут в `on_log`; выход
/// процесса без ответа или таймаут — ошибка; процесс всегда добивается.
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
    // Reader thread: whole lines to the channel; this thread owns the timeout.
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
    let deadline = std::time::Instant::now() + timeout;
    let wait_for = |id: i64| -> Result<Value, String> {
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
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
        }
    };
    let result = (|| {
        send(
            &mut stdin,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"1c-connect","version":env!("CARGO_PKG_VERSION")}}}),
        )?;
        wait_for(1)?;
        send(&mut stdin, json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        send(&mut stdin, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":args}}))?;
        let v = wait_for(2)?;
        parse_reply(&v).ok_or_else(|| "vrunner-mcp ответил не результатом".to_string())
    })();
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    drop(job);
    result.map(|r| Reply { text: common::docker::redact_text(&r.text), is_error: r.is_error })
}

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
    /// BRIDGE_SCRIPT answers initialize and tools/call ("child ran <name>").
    fn scripted(env: (&str, &str)) -> std::process::Command {
        let mut c = std::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "bridge::tests::__echo_child", "--nocapture", "--quiet"]);
        c.env(env.0, env.1);
        c
    }

    #[test]
    fn a_local_call_initialises_calls_and_reaps_the_process() {
        let logged = std::cell::RefCell::new(Vec::new());
        let r = call(scripted(("BRIDGE_SCRIPT", "1")), "cf_decompile", json!({"background": false}), Duration::from_secs(30), &|s| logged.borrow_mut().push(s)).unwrap();
        assert_eq!(r.text, "child ran cf_decompile");
        assert!(!r.is_error);
    }

    #[test]
    fn a_process_that_dies_without_answering_is_an_error_not_a_hang() {
        let t0 = std::time::Instant::now();
        let e = call(scripted(("BRIDGE_DIE_AFTER_READ", "1")), "cf_load", json!({}), Duration::from_secs(30), &|_| {}).unwrap_err();
        assert!(e.contains("завершился") || e.contains("не ответил"), "{e}");
        assert!(t0.elapsed() < Duration::from_secs(10));
    }
}
