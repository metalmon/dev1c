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

/// Строка от агента: что с ней делать. Всё, что уходит ребёнку, несёт
/// канонический текст — JSON, как его разобрал мост (без BOM, пробелов и
/// повторов ключей): иначе ребёнок прочёл бы не то, что проверил фильтр.
pub enum Route {
    /// Не JSON-объект или уведомление с неизвестным методом — от агента такое не пересылается.
    Drop,
    /// Запрос, который мост не пропускает: ответ — ошибка JSON-RPC с этим текстом.
    Reject { id: Value, why: String },
    Forward(Vec<u8>),
    ListRequest { id: Value, line: Vec<u8> },
    OurCall { id: Value, tool: String, args: Value },
    Disabled { id: Value, tool: String },
    PlatformCall { id: Value, tool: String, line: Vec<u8> },
    Cancel { id: Value, line: Vec<u8> },
}

fn parse(line: &[u8]) -> Option<Value> {
    let s = std::str::from_utf8(line).ok()?;
    let s = s.trim_start_matches('\u{feff}').trim();
    if !s.starts_with('{') {
        return None;
    }
    serde_json::from_str(s).ok()
}

fn canonical(v: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec(v).unwrap_or_default();
    out.push(b'\n');
    out
}

/// Методы, которые мост пропускает ребёнку. Остальное — ошибка «нет такого
/// метода»: OneScript сравнивает строки без учёта регистра, и `Tools/Call`
/// для него — вызов инструмента, а для фильтра был бы «что-то другое».
const METHODS: [&str; 14] = [
    "initialize",
    "notifications/initialized",
    "ping",
    "tools/list",
    "tools/call",
    "notifications/cancelled",
    "notifications/progress",
    "prompts/list",
    "prompts/get",
    "resources/list",
    "resources/read",
    "resources/templates/list",
    "logging/setLevel",
    "completion/complete",
];

/// Ключ объекта без учёта регистра — и только если он один такой: два ключа,
/// различающихся регистром, читаются ребёнком и мостом по-разному.
fn key_ci<'a>(o: &'a serde_json::Map<String, Value>, key: &str) -> Result<Option<&'a Value>, ()> {
    let mut found = None;
    for (k, v) in o {
        if k.eq_ignore_ascii_case(key) {
            if found.is_some() {
                return Err(());
            }
            found = Some(v);
        }
    }
    Ok(found)
}

/// Что делать со строкой от агента.
pub fn route_request(line: &[u8], allowed: &[String]) -> Route {
    let Some(v) = parse(line) else { return Route::Drop };
    let Some(obj) = v.as_object() else { return Route::Drop };
    let Ok(id) = key_ci(obj, "id") else { return Route::Drop };
    let id = id.cloned().unwrap_or(Value::Null);
    let reject = |why: &str| if id.is_null() { Route::Drop } else { Route::Reject { id: id.clone(), why: why.to_string() } };
    let method = match key_ci(obj, "method") {
        Ok(Some(Value::String(m))) => m.to_ascii_lowercase(),
        _ => return reject("нет метода"),
    };
    let Some(method) = METHODS.iter().find(|m| m.eq_ignore_ascii_case(&method)) else { return reject("нет такого метода") };
    let params = match key_ci(obj, "params") {
        Ok(p) => p.cloned().unwrap_or(json!({})),
        Err(()) => return reject("params задан дважды"),
    };
    // Пересобранное сообщение: только проверенные поля, канонические имена.
    let rebuilt = |params: Value| canonical(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    match *method {
        "tools/list" => Route::ListRequest { id: id.clone(), line: rebuilt(params) },
        "tools/call" => {
            let Some(p) = params.as_object() else { return reject("params — не объект") };
            let (Ok(name), Ok(args)) = (key_ci(p, "name"), key_ci(p, "arguments")) else { return reject("name или arguments заданы дважды") };
            let Some(tool) = name.and_then(Value::as_str) else { return reject("tools/call без name") };
            let tool = tool.to_string();
            let args = args.cloned().unwrap_or(json!({}));
            if OUR_TOOLS.contains(&tool.as_str()) {
                Route::OurCall { id: id.clone(), tool, args }
            } else if !allowed.iter().any(|a| a == &tool) {
                Route::Disabled { id: id.clone(), tool }
            } else {
                let line = rebuilt(json!({"name": tool, "arguments": args}));
                if PLATFORM_TOOLS.contains(&tool.as_str()) {
                    Route::PlatformCall { id: id.clone(), tool, line }
                } else {
                    Route::Forward(line)
                }
            }
        }
        "notifications/cancelled" => match params.as_object().and_then(|p| key_ci(p, "requestId").ok().flatten()) {
            Some(rid) => Route::Cancel { id: rid.clone(), line: rebuilt(params) },
            None => Route::Forward(rebuilt(params)),
        },
        _ => Route::Forward(rebuilt(params)),
    }
}

/// Ошибка JSON-RPC на запрос, который мост не пропустил.
pub fn error_reply(id: &Value, code: i64, message: &str) -> Vec<u8> {
    let v = json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
    let mut out = serde_json::to_vec(&v).unwrap();
    out.push(b'\n');
    out
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
        assert!(matches!(r, Route::ListRequest { id, .. } if id == json!(7)));
    }

    #[test]
    fn allowed_call_goes_to_the_child_as_is() {
        let r = route_request(br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"task_status","arguments":{}}}"#, &allowed());
        assert!(matches!(r, Route::Forward(_)));
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
        assert!(matches!(r, Route::Cancel { id, .. } if id == json!(2)));
    }

    #[test]
    fn cancel_of_a_foreign_id_is_forwarded() {
        // Route only says "cancel of id 9"; whether 9 is ours is the bridge's
        // table — the bridge forwards what it does not own (tested in bridge.rs).
        let r = route_request(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9}}"#, &allowed());
        assert!(matches!(r, Route::Cancel { id, .. } if id == json!(9)));
    }

    #[test]
    fn other_methods_are_forwarded_and_junk_is_dropped() {
        assert!(matches!(route_request(b"warning: something", &allowed()), Route::Drop));
        assert!(matches!(route_request(b"[1,2]", &allowed()), Route::Drop));
        assert!(matches!(route_request(br#"{"jsonrpc":"2.0","id":3,"method":"initialize","params":{}}"#, &allowed()), Route::Forward(_)));
        assert!(matches!(route_request(br#"{"jsonrpc":"2.0","id":3,"method":"tools/call"}"#, &allowed()), Route::Reject { .. }));
    }

    #[test]
    fn method_and_key_case_cannot_dodge_the_whitelist() {
        // OneScript compares strings case-insensitively: "Tools/Call" with a
        // "Name" key is a tools/call to the child. It must be one to us too.
        let line = br#"{"jsonrpc":"2.0","id":1,"method":"Tools/Call","params":{"Name":"infobase_delete","arguments":{}}}"#;
        assert!(matches!(route_request(line, &allowed()), Route::Disabled { tool, .. } if tool == "infobase_delete"));
        let line = br#"{"jsonrpc":"2.0","id":1,"method":"TOOLS/CALL","params":{"NAME":"task_status","Arguments":{"a":1},"extra":"x"}}"#;
        match route_request(line, &allowed()) {
            Route::Forward(out) => {
                let v: Value = serde_json::from_slice(&out).unwrap();
                assert_eq!(v["method"], "tools/call");
                assert_eq!(v["params"]["name"], "task_status");
                assert_eq!(v["params"]["arguments"]["a"], 1);
                assert_eq!(v["params"].as_object().unwrap().len(), 2, "{v}");
                assert_eq!(v.as_object().unwrap().len(), 4, "{v}");
            }
            _ => panic!("task_status is allowed"),
        }
        // Two keys that differ only by case: ambiguous, refused.
        let line = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"task_status","NAME":"infobase_delete"}}"#;
        assert!(matches!(route_request(line, &allowed()), Route::Reject { .. }));
    }

    #[test]
    fn unknown_methods_and_nameless_calls_are_refused_not_forwarded() {
        let line = br#"{"jsonrpc":"2.0","id":3,"method":"tools/call"}"#;
        assert!(matches!(route_request(line, &allowed()), Route::Reject { id, .. } if id == json!(3)));
        let line = br#"{"jsonrpc":"2.0","id":4,"method":"tools/execute","params":{"name":"infobase_delete"}}"#;
        assert!(matches!(route_request(line, &allowed()), Route::Reject { id, .. } if id == json!(4)));
        // A notification with an unknown method has no id to answer: dropped.
        let line = br#"{"jsonrpc":"2.0","method":"notifications/whatever"}"#;
        assert!(matches!(route_request(line, &allowed()), Route::Drop));
        // Known methods pass, in canonical spelling.
        for m in ["initialize", "Ping", "notifications/initialized", "prompts/list", "resources/list"] {
            let line = format!(r#"{{"jsonrpc":"2.0","id":5,"method":"{m}","params":{{}}}}"#);
            assert!(matches!(route_request(line.as_bytes(), &allowed()), Route::Forward(_)), "{m}");
        }
        let v: Value = serde_json::from_slice(&error_reply(&json!(4), -32601, "нет такого метода")).unwrap();
        assert_eq!(v["error"]["code"], -32601);
        assert_eq!(v["id"], 4);
    }

    #[test]
    fn agent_lines_are_canonicalised_before_forwarding() {
        // Leading whitespace, a BOM and duplicate keys must not let a call slip
        // past the whitelist: what goes to the child is what the bridge parsed.
        let line = "\u{feff}  {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"infobase_delete\"},\"params\":{\"name\":\"task_status\"}}";
        match route_request(line.as_bytes(), &allowed()) {
            Route::Forward(out) => {
                let s = String::from_utf8(out).unwrap();
                assert!(s.starts_with('{') && s.ends_with('\n'), "{s}");
                assert!(!s.contains("infobase_delete"), "{s}");
            }
            _ => panic!("task_status is allowed"),
        }
        let line = " {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"infobase_delete\"}}";
        assert!(matches!(route_request(line.as_bytes(), &allowed()), Route::Disabled { .. }));
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
