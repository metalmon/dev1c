//! Чистая часть моста 778N: рамка «строка на сообщение» и правила, что
//! уходит ребёнку, что отвечает мост сам и как переписывается `tools/list`.
//! Байты и JSON, никаких процессов — всё проверяется тестами на строках.

use serde_json::{json, Value};

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
    let last_page = v.pointer("/result/nextCursor").is_none();
    let tools = v.pointer_mut("/result/tools")?.as_array_mut()?;
    tools.retain(|t| t.get("name").and_then(Value::as_str).map_or(false, |n| allowed.iter().any(|a| a == n)));
    if last_page {
        tools.extend(our_tools());
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
}
