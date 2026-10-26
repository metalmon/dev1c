//! Чистая часть моста 778N: рамка «строка на сообщение» и правила, что
//! уходит ребёнку, что отвечает мост сам и как переписывается `tools/list`.
//! Байты и JSON, никаких процессов — всё проверяется тестами на строках.
//! Сюда же вынесены две проверки общие для 778N и 777N (гейт `tools/call`
//! без `id`) и своя, для 777N, пока переключатель выключен (`mcp_gate_while_off`).

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
                Some(i) if self.buf.len() + i > LINE_LIMIT => {
                    // The line overflowed and its end is already here: raw
                    // through the newline, never a Line.
                    self.buf.extend_from_slice(&rest[..=i]);
                    out.push(Frame::Raw(std::mem::take(&mut self.buf)));
                    rest = &rest[i + 1..];
                }
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
    /// Запрос, который мост не пропускает: ответ — ошибка JSON-RPC с этим
    /// кодом и текстом (`id` null — разобрать не удалось).
    Reject { id: Value, code: i64, why: String },
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
    route_request_with(line, allowed, &Value::Null)
}

/// То же с аргументами подключения для вызовов, уходящих ребёнку: база и её
/// пользователь. vrunner-mcp берёт их из аргументов инструмента, не из
/// окружения (проверено 2026-10-18); агент их не знает и знать не должен.
/// Переданное агентом значение **заменяется** (ключи — без учёта регистра):
/// иначе строка подключения по выбору агента увела бы пароль базы на чужой
/// адрес.
pub fn route_request_with(line: &[u8], allowed: &[String], defaults: &Value) -> Route {
    const PARSE: i64 = -32700;
    const METHOD: i64 = -32601;
    const PARAMS: i64 = -32602;
    let Some(v) = parse(line) else { return Route::Reject { id: Value::Null, code: PARSE, why: "не JSON-объект".into() } };
    let Some(obj) = v.as_object() else { return Route::Reject { id: Value::Null, code: PARSE, why: "не JSON-объект".into() } };
    let Ok(id_field) = key_ci(obj, "id") else { return Route::Reject { id: Value::Null, code: PARSE, why: "id задан дважды".into() } };
    // Присутствие ключа `id`, а не только его значение: уведомление вообще
    // не несёт этого ключа, и ответ `"id":null` превратил бы его для
    // ребёнка в запрос, которого тот не посылал.
    let has_id = id_field.is_some();
    let id = id_field.cloned().unwrap_or(Value::Null);
    // Уведомление (без id) ответа не ждёт: неизвестное — молча мимо.
    let reject = |code: i64, why: &str| if id.is_null() { Route::Drop } else { Route::Reject { id: id.clone(), code, why: why.to_string() } };
    let method = match key_ci(obj, "method") {
        Ok(Some(Value::String(m))) => m.to_ascii_lowercase(),
        _ => return reject(METHOD, "нет метода"),
    };
    let Some(method) = METHODS.iter().find(|m| m.eq_ignore_ascii_case(&method)) else { return reject(METHOD, "нет такого метода") };
    let params = match key_ci(obj, "params") {
        Ok(p) => p.cloned().unwrap_or(json!({})),
        Err(()) => return reject(PARAMS, "params задан дважды"),
    };
    // Пересобранное сообщение: только проверенные поля, канонические имена —
    // но уведомление (нет ключа `id`, например `notifications/initialized`
    // или `notifications/cancelled`) уходит ребёнку байт в байт, как
    // пришло: добавленный ключ `"id":null` сделал бы из него запрос.
    let rebuilt = |params: Value| {
        if has_id {
            canonical(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        } else {
            let mut verbatim = line.to_vec();
            verbatim.push(b'\n');
            verbatim
        }
    };
    match *method {
        "tools/list" => Route::ListRequest { id: id.clone(), line: rebuilt(params) },
        "tools/call" => {
            let Some(p) = params.as_object() else { return reject(PARAMS, "params — не объект") };
            let (Ok(name), Ok(args)) = (key_ci(p, "name"), key_ci(p, "arguments")) else { return reject(PARAMS, "name или arguments заданы дважды") };
            let Some(tool) = name.and_then(Value::as_str) else { return reject(PARAMS, "tools/call без name") };
            let tool = tool.to_string();
            let args = args.cloned().unwrap_or(json!({}));
            if OUR_TOOLS.contains(&tool.as_str()) {
                Route::OurCall { id: id.clone(), tool, args }
            } else if !allowed.iter().any(|a| a == &tool) {
                Route::Disabled { id: id.clone(), tool }
            } else {
                let mut args = if args.is_object() { args } else { json!({}) };
                if let (Some(a), Some(d)) = (args.as_object_mut(), defaults.as_object()) {
                    a.retain(|k, _| !d.keys().any(|dk| dk.eq_ignore_ascii_case(k)) && !k.eq_ignore_ascii_case("db-pwd"));
                    for (k, v) in d {
                        a.insert(k.clone(), v.clone());
                    }
                }
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

/// `tools/call` без ключа `id` — ответить ему нечем (нет адреса для
/// ответа), а пропустить его к ребёнку — значит исполнить побочный эффект
/// (например, `cf_load`) без единого следа в ответах агенту. Поэтому такой
/// вызов не доходит до ребёнка никогда и ни на каком порту, независимо от
/// переключателя: безопаснее тихо отбросить, чем один раз обойти проверку
/// вызовом без идентификатора.
///
/// Ключи читаются через `key_ci` — тем же способом, что и `route_request_with`
/// чуть ниже. Это не новая проверка «на всякий случай» (её сознательно не
/// заводили для 777N, см. `mcp_gate_while_off`), а устранение несогласия
/// внутри одного файла: при точном сравнении имени ключа `{"Method":
/// "tools/call", ...}` без `id` не опознавался этой функцией как `tools/call`
/// вовсе, рассматривался дальше как обычная строка и там, где он на самом
/// деле `tools/call`, уходил ребёнку мимо этой проверки.
pub fn is_tools_call_without_id(line: &[u8]) -> bool {
    let Some(v) = parse(line) else { return false };
    let Some(obj) = v.as_object() else { return false };
    let Some(method) = key_ci(obj, "method").ok().flatten().and_then(Value::as_str) else { return false };
    method.eq_ignore_ascii_case("tools/call") && !matches!(key_ci(obj, "id"), Ok(Some(_)))
}

/// Куда идёт строка от агента на трубе mcp-1c (777N), пока переключатель
/// выключен. Этот порт не ведёт разбор JSON-RPC вовсе (голый проброс — см.
/// `Rules::for_mcp`), поэтому правило — не «пропускать всё, кроме
/// запрещённого», а наоборот: вперёд уходит только то, что уверенно опознано
/// безопасным (один JSON-объект со своим методом, и это не `tools/call`).
/// Всё прочее — не JSON-объект, батч, объект без понятного метода — закрыто
/// по умолчанию, иначе такая строка стала бы тихой лазейкой в обход
/// переключателя. `tools/call` без `id` сюда не долетает: он отбрасывается
/// раньше и везде, см. `is_tools_call_without_id`.
///
/// Ключи `method`/`id` читаются через `key_ci`, как у `is_tools_call_without_id`
/// и у `route_request_with`: не согласованные между собой способы читать один
/// и тот же ключ — это и была дыра, а не повод завести третий.
///
/// Дубликаты одного и того же ключа (двух `id` сразу) и прочие тонкости
/// разбора здесь не разбираются отдельно (решение заказчика) — это другой
/// вопрос, чем регистр имени ключа: `key_ci` сам возвращает «не нашёл»
/// (`Err`) при таком дубликате, и здесь это просто равносильно отсутствию
/// ключа. Пока с этой трубой говорит собственный агент пользователя в его же
/// контейнере, а не чужой недоверенный клиент — этого достаточно; цена
/// более строгой проверки (как самой `route_request_with` для 778N, которая
/// на такой дубликат отвечает явным отказом) здесь не оправдана.
pub enum McpOffGate {
    /// Метод известен и это не `tools/call` — пропустить как есть.
    Forward,
    /// `tools/call` с `id` — отказ по переключателю этим `id`.
    Refuse(Value),
    /// Распознать не удалось, но `id` в объекте есть — ответить им ошибкой.
    ParseError(Value),
    /// Распознать не удалось и ответить нечем — тихо отбросить (журнал — на
    /// совести вызывающего кода, здесь просто сигнал «не пропускать»).
    Drop,
}

pub fn mcp_gate_while_off(line: &[u8]) -> McpOffGate {
    let Some(v) = parse(line) else { return McpOffGate::Drop };
    let Some(obj) = v.as_object() else { return McpOffGate::Drop };
    let id = key_ci(obj, "id").ok().flatten().cloned();
    match key_ci(obj, "method").ok().flatten().and_then(Value::as_str) {
        Some(m) if m.eq_ignore_ascii_case("tools/call") => match id {
            Some(id) => McpOffGate::Refuse(id),
            // Перестраховка: is_tools_call_without_id должен был отбросить
            // это раньше, но на две проверки надёжнее положиться, чем на одну.
            None => McpOffGate::Drop,
        },
        Some(_) => McpOffGate::Forward,
        None => match id {
            Some(id) => McpOffGate::ParseError(id),
            None => McpOffGate::Drop,
        },
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
    tools.retain(|t| t.get("name").and_then(Value::as_str).is_some_and(|n| allowed.iter().any(|a| a == n)));
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
            "description": "Состояние проекта: ветка, последний коммит, неотправленные коммиты, изменённые файлы, есть ли ConfigDumpInfo.xml (индекс инкремента), а также имя репозитория в Gitea (dev1c/<имя>) и ветка рабочего места — их неоткуда больше узнать.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true}
        }),
        json!({
            "name": "project_pull",
            "description": "Забрать свою ветку из Gitea (git pull --rebase). Только на чистом дереве: сначала project_push. После pull — cf_load. Забирает то, что отправлено в Gitea; правки, лежащие у вас незакоммиченными, сюда не попадут.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "project_push",
            "description": "Закоммитить src и отправить свою ветку в Gitea. Перед этим — cf_decompile. Первый push создаёт репозиторий и ветку main. Пока не отправлено, человек правок не увидит и загрузить их в базу нельзя.",
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

    #[test]
    fn overflow_whose_newline_arrives_in_the_same_read_is_still_raw() {
        // 8 MiB of a line, then one read holding its tail, the newline and
        // the next line: the long line must come out as Raw, never as Line.
        let mut f = Framer::new();
        assert!(f.push(&vec![b'x'; LINE_LIMIT - 10]).is_empty());
        let out = lines(f.push(b"xxxxxxxxxxxxxxxxxxxx\n{\"ok\":1}\n"));
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[0].starts_with("R:") && out[0].ends_with('\n'), "{}", &out[0][..8]);
        assert_eq!(out[1], "L:{\"ok\":1}");
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
    fn a_notification_is_forwarded_byte_exact_without_an_id() {
        // No "id" key at all (a notification, not "id":null) must reach the
        // child exactly as it arrived — an added "id":null would turn it
        // into a request the child never sent, and mcp-1c may refuse that.
        let line: &[u8] = br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        match route_request(line, &allowed()) {
            Route::Forward(out) => {
                let mut want = line.to_vec();
                want.push(b'\n');
                assert_eq!(out, want);
            }
            _ => panic!("notifications/initialized must forward"),
        }
        // Same for notifications/cancelled without a matching running id: the
        // bridge does not own it, so it is forwarded, and still byte-exact.
        let line: &[u8] = br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9}}"#;
        match route_request(line, &allowed()) {
            Route::Cancel { line: out, .. } => {
                let mut want = line.to_vec();
                want.push(b'\n');
                assert_eq!(out, want);
            }
            _ => panic!("notifications/cancelled must carry the raw line"),
        }
    }

    #[test]
    fn tools_call_without_id_is_recognised_regardless_of_case_or_port() {
        assert!(is_tools_call_without_id(br#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"cf_load"}}"#));
        // Case does not matter — OneScript compares method names the same way.
        assert!(is_tools_call_without_id(br#"{"jsonrpc":"2.0","method":"Tools/Call"}"#));
        // An id, any method other than tools/call, or junk — not this rule's business.
        assert!(!is_tools_call_without_id(br#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#));
        assert!(!is_tools_call_without_id(br#"{"jsonrpc":"2.0","method":"initialize"}"#));
        assert!(!is_tools_call_without_id(b"not json"));
        assert!(!is_tools_call_without_id(b"[1,2]"));
    }

    #[test]
    fn tools_call_without_id_agrees_with_route_request_with_on_a_differently_cased_key() {
        // The bug the second review found: a differently-cased "method" or
        // "id" key must be read the same way here and in route_request_with,
        // or one half of the bridge sees a tools/call the other half misses.
        let no_id = br#"{"jsonrpc":"2.0","Method":"tools/call","params":{"name":"cf_load"}}"#;
        assert!(is_tools_call_without_id(no_id), "a differently-cased method must still read as tools/call");
        assert!(matches!(route_request(no_id, &["cf_load".to_string()]), Route::PlatformCall { .. }), "route_request_with already finds it via key_ci");

        let odd_id = br#"{"method":"tools/call","ID":5,"params":{"name":"cf_load"}}"#;
        assert!(!is_tools_call_without_id(odd_id), "an id spelled ID is still an id");
        assert!(matches!(route_request(odd_id, &["cf_load".to_string()]), Route::PlatformCall { id, .. } if id == json!(5)));
    }

    #[test]
    fn mcp_gate_while_off_forwards_only_a_recognised_non_tools_call_method() {
        assert!(matches!(mcp_gate_while_off(br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#), McpOffGate::Forward));
        assert!(matches!(mcp_gate_while_off(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), McpOffGate::Forward));
    }

    #[test]
    fn mcp_gate_while_off_refuses_a_tools_call_that_has_an_id() {
        assert!(matches!(mcp_gate_while_off(br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"x"}}"#), McpOffGate::Refuse(id) if id == json!(7)));
        assert!(matches!(mcp_gate_while_off(br#"{"jsonrpc":"2.0","id":"a","method":"Tools/Call"}"#), McpOffGate::Refuse(id) if id == json!("a")));
    }

    #[test]
    fn mcp_gate_while_off_fails_closed_on_anything_unclassifiable() {
        // A batch, junk, and an object without a usable method are not
        // positively known safe — none of them reach the child.
        assert!(matches!(mcp_gate_while_off(b"[1,2]"), McpOffGate::Drop));
        assert!(matches!(mcp_gate_while_off(b"not json"), McpOffGate::Drop));
        assert!(matches!(mcp_gate_while_off(br#"{"jsonrpc":"2.0","id":3}"#), McpOffGate::ParseError(id) if id == json!(3)));
        assert!(matches!(mcp_gate_while_off(br#"{"jsonrpc":"2.0"}"#), McpOffGate::Drop));
        // A tools/call without an id never reaches this far in the bridge
        // (handle_request drops it earlier for both ports), but the gate
        // itself still fails closed on it rather than forwarding.
        assert!(matches!(mcp_gate_while_off(br#"{"jsonrpc":"2.0","method":"tools/call"}"#), McpOffGate::Drop));
    }

    #[test]
    fn other_methods_are_forwarded_and_junk_is_answered_with_a_parse_error() {
        // Junk from the agent gets a JSON-RPC parse error (id null), not
        // silence: a client that sent something sees why nothing happened.
        assert!(matches!(route_request(b"warning: something", &allowed()), Route::Reject { id: Value::Null, .. }));
        assert!(matches!(route_request(b"[1,2]", &allowed()), Route::Reject { id: Value::Null, .. }));
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
    fn connection_defaults_are_injected_into_forwarded_calls_only_when_absent() {
        // vrunner-mcp reads the base user from tool arguments, not from the
        // environment: the bridge fills them in so the agent never has to.
        let defaults = json!({"ibconnection": "/FC:\\b", "db-user": "mcp", "db-pwd": "secret"});
        let line = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"task_status","arguments":{"taskId":"x"}}}"#;
        match route_request_with(line, &allowed(), &defaults) {
            Route::Forward(out) => {
                let v: Value = serde_json::from_slice(&out).unwrap();
                assert_eq!(v["params"]["arguments"]["taskId"], "x");
                assert_eq!(v["params"]["arguments"]["db-user"], "mcp");
                assert_eq!(v["params"]["arguments"]["ibconnection"], "/FC:\\b");
            }
            _ => panic!("forwarded"),
        }
        // The agent's own values are replaced, whatever their key case: a
        // connection string of the agent's choosing with the host's password
        // would send that password to an address the agent picked.
        let line = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cf_load","arguments":{"db-user":"other","IBConnection":"/Sevil\\base","Db-Pwd":"x"}}}"#;
        match route_request_with(line, &allowed(), &defaults) {
            Route::PlatformCall { line, .. } => {
                let v: Value = serde_json::from_slice(&line).unwrap();
                let a = v["params"]["arguments"].as_object().unwrap();
                assert_eq!(a["db-user"], "mcp");
                assert_eq!(a["db-pwd"], "secret");
                assert_eq!(a["ibconnection"], "/FC:\\b");
                assert_eq!(a.len(), 3, "{a:?}");
            }
            _ => panic!("platform call"),
        }
        // Our own tools and other methods are untouched.
        assert!(matches!(route_request_with(br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_status"}}"#, &allowed(), &defaults), Route::OurCall { args, .. } if args.get("db-pwd").is_none()));
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

