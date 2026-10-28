//! Запись коннектора в настройки редактора на машине человека: сегодня
//! только Zed (pi — задача 5). Правка текстовая, по границам наших двух
//! записей `context_servers` — файл человека несёт комментарии, и разобрать
//! документ целиком и напечатать заново нельзя.

use std::path::{Path, PathBuf};

/// Одна из наших записей `context_servers`: имя сервера совпадает с
/// аргументом `1c-connect mcp <name>`, таймаут — тем, что заложен в мост
/// (`1c` ждёт mcp-1c, `vrunner` — сборку и тесты, отсюда разница).
pub struct Entry {
    pub name: &'static str,
    pub timeout: u32,
}

pub const OURS: [Entry; 2] = [Entry { name: "1c", timeout: 300 }, Entry { name: "vrunner", timeout: 3600 }];

/// Путь к `settings.json` Zed на этой машине: Windows — `%APPDATA%`, macOS —
/// `~/.config` (сам Zed так решил, не Apple), остальной unix — `XDG_CONFIG_HOME`
/// или `~/.config` (спека «MCP в редакторе», «Пути настроек»). `None` —
/// переменной окружения, по которой путь строится, нет.
pub fn zed_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Zed").join("settings.json"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/zed/settings.json"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        base.map(|b| b.join("zed").join("settings.json"))
    }
}

/// Снимает комментарии `//` **вне строк**, чтобы JSONC можно было разобрать
/// как обычный JSON. Сама правка (`merge_zed`/`set_enabled_zed`) её не
/// зовёт — она ищет границы записи напрямую в исходном тексте, пропуская
/// комментарии на лету (`skip_ws_and_comments`), чтобы не потерять их при
/// сборке результата. Эта функция нужна тестам: и здесь (проверить, что
/// получилось, разобрав как JSON), и в `cfg.rs` (`zed_json`) — там она жила
/// копией до этой задачи, теперь общее место одно.
///
/// ponytail: сегодня оба вызова — из тестов (`#[cfg(test)]`), и в обычной
/// сборке функция простаивает; `pub(crate) fn`, а не `#[cfg(test)]`, — по
/// решению задачи (эта правка — боевой код модуля, не только для тестов),
/// поэтому гасим предупреждение, а не прячем функцию за test-гейтом.
#[allow(dead_code)]
pub(crate) fn as_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        let (mut in_string, mut escaped, mut i) = (false, false, 0);
        while i < chars.len() {
            let c = chars[i];
            if escaped {
                escaped = false;
            } else if in_string && c == '\\' {
                escaped = true;
            } else if c == '\"' {
                in_string = !in_string;
            } else if !in_string && c == '/' && chars.get(i + 1) == Some(&'/') {
                break; // строка комментария — остаток не нужен
            }
            out.push(c);
            i += 1;
        }
        out.push('\n');
    }
    out
}

/// Экранирует `\` и `"` для JSON-строки — путь к программе кладём в
/// `command` как он есть, остальные байты (кириллица, пробелы) допустимы в
/// JSON без изменений.
fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

/// Обратное к `escape_json_string` — читаем значение `command`, которое уже
/// лежит в файле, чтобы сравнить его имя файла с нашим.
fn unescape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next) => out.push(next),
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Текст одной нашей записи `"name": {...}`, с полями на отступе в шесть
/// пробелов и закрывающей скобкой на четырёх — ровно так, как кладёт её
/// `context_servers_value`, вставляющая такие записи на отступе в четыре.
/// ponytail: отступ всегда наш собственный, а не подогнанный под чужой файл
/// — это косметика, которую ни один тест не проверяет.
fn entry_json(e: &Entry, exe: &Path, nl: &str) -> String {
    format!(
        "\"{}\": {{{nl}      \"command\": \"{}\",{nl}      \"args\": [\"mcp\", \"{}\"],{nl}      \"timeout\": {},{nl}      \"enabled\": true{nl}    }}",
        e.name,
        escape_json_string(&exe.display().to_string()),
        e.name,
        e.timeout,
    )
}

/// Значение всего ключа `"context_servers": {...}` с обеими записями — для
/// свежего файла и для вставки ключа, которого раньше не было.
fn context_servers_value(exe: &Path, nl: &str) -> String {
    let body = OURS.iter().map(|e| format!("    {}", entry_json(e, exe, nl))).collect::<Vec<_>>().join(&format!(",{nl}"));
    format!("{{{nl}{body}{nl}  }}")
}

/// Файл с нуля — абсента `settings.json` ещё не было, значит и чужого в нём
/// нет.
fn fresh_document(exe: &Path, nl: &str) -> String {
    format!("{{{nl}  \"context_servers\": {}{nl}}}{nl}", context_servers_value(exe, nl))
}

/// BOM отделяем от текста, а не стираем: входил — выйдет обратно таким же.
fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

/// Один фрагмент правки по байтовым границам исходного текста: `[start,
/// end)` заменяется на `text` (пустой диапазон — вставка).
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

/// Применяет фрагменты правки за один проход — байтовые смещения всегда
/// считаны от исходного `body`, поэтому несколько правок не портят друг
/// другу позиции (в отличие от правки по месту со сдвигом длины).
fn apply_edits(body: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| (e.start, e.end));
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0;
    for e in edits {
        out.push_str(&body[cursor..e.start]);
        out.push_str(&e.text);
        cursor = e.end;
    }
    out.push_str(&body[cursor..]);
    out
}

/// Пропускает пробелы и строчные комментарии `//`, возвращая индекс первого
/// значимого байта. Скобки/кавычки в комментариях этим и не видны счёту
/// глубины ниже — ради этого функция и существует отдельно от `as_json`:
/// там снятые комментарии печатаются заново, здесь нужны только смещения в
/// исходном тексте.
fn skip_ws_and_comments(t: &str, mut i: usize) -> usize {
    let b = t.as_bytes();
    loop {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        break;
    }
    i
}

/// Индекс первого байта после закрывающей кавычки строки, начинающейся в
/// `i` (байт `b[i]` — открывающая кавычка).
fn skip_string(t: &str, i: usize) -> usize {
    let b = t.as_bytes();
    let mut j = i + 1;
    let mut escaped = false;
    while j < b.len() {
        if escaped {
            escaped = false;
        } else if b[j] == b'\\' {
            escaped = true;
        } else if b[j] == b'"' {
            return j + 1;
        }
        j += 1;
    }
    j
}

/// Индекс первого байта после скобки, парной той, что в `open` (`{`/`}` или
/// `[`/`]`) — считает только свой тип скобки, другой тип и содержимое строк
/// внутри пропускает как обычные байты (классический приём: вложенный `[`
/// внутри `{...}` не трогает глубину объекта, а в строке `"{"` скобка не
/// считается вовсе).
fn skip_bracket(t: &str, open: usize) -> usize {
    let b = t.as_bytes();
    let (openc, closec) = if b[open] == b'{' { (b'{', b'}') } else { (b'[', b']') };
    let mut depth = 0i32;
    let mut i = open;
    loop {
        match b.get(i) {
            None => return i,
            Some(&b'"') => i = skip_string(t, i),
            Some(&c) if c == b'/' && b.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            Some(&c) if c == openc => {
                depth += 1;
                i += 1;
            }
            Some(&c) if c == closec => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
}

/// Индекс первого байта после значения, начинающегося в `start` (после
/// пробелов/комментариев) — строка, объект, массив или голый литерал
/// (число/`true`/`false`/`null`), который обрывается на запятой, скобке или
/// переводе строки.
fn skip_value(t: &str, start: usize) -> usize {
    let b = t.as_bytes();
    let i = skip_ws_and_comments(t, start);
    match b.get(i) {
        Some(b'"') => skip_string(t, i),
        Some(b'{') | Some(b'[') => skip_bracket(t, i),
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && b[j] != b'\n' {
                if b[j] == b'/' && b.get(j + 1) == Some(&b'/') {
                    break;
                }
                j += 1;
            }
            while j > i && (b[j - 1] as char).is_whitespace() {
                j -= 1;
            }
            j
        }
    }
}

/// Прямой потомок JSON-объекта: имя ключа и байтовые границы его значения в
/// исходном тексте (кавычки значения-строки входят в границы).
struct Field {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
}

/// Поля объекта, начинающегося на `{` в `open`, и индекс первого байта
/// после его закрывающей `}`. Вложенные объекты/массивы не разбирает —
/// только прямых детей, этого достаточно: вглубь мы спускаемся повторным
/// вызовом (`context_servers`, затем запись `1c`/`vrunner`, затем её
/// `command`/`enabled`).
fn parse_fields(t: &str, open: usize) -> (Vec<Field>, usize) {
    let b = t.as_bytes();
    let mut i = open + 1;
    let mut fields = Vec::new();
    loop {
        i = skip_ws_and_comments(t, i);
        match b.get(i) {
            None => return (fields, i),
            Some(&b'}') => return (fields, i + 1),
            Some(&b'"') => {
                let key_start = i;
                let key_end = skip_string(t, i);
                let key = t[key_start + 1..key_end - 1].to_string();
                let after_colon = skip_ws_and_comments(t, key_end);
                let after_colon = if b.get(after_colon) == Some(&b':') { after_colon + 1 } else { after_colon };
                let value_start = skip_ws_and_comments(t, after_colon);
                let value_end = skip_value(t, value_start);
                fields.push(Field { key, key_start, value_start, value_end });
                i = skip_ws_and_comments(t, value_end);
                if b.get(i) == Some(&b',') {
                    i += 1;
                }
            }
            // Не ключ и не закрывающая скобка — защита от бесконечного цикла
            // на неожиданном байте, не должна срабатывать на валидном JSONC.
            Some(_) => i += 1,
        }
    }
}

/// Запись в `context_servers` — наша, если имя файла в её `command`
/// совпадает с именем файла нашего исполняемого (`1c-connect`/
/// `1c-connect.exe`); остальное — чужое (спека, решение владельца №3).
fn is_ours(body: &str, field: &Field, exe: &Path) -> bool {
    let Some(want) = exe.file_name().map(|n| n.to_string_lossy().to_string()) else { return false };
    if body.as_bytes().get(field.value_start) != Some(&b'{') {
        return false;
    }
    let (fields, _) = parse_fields(body, field.value_start);
    let Some(cmd) = fields.iter().find(|f| f.key == "command") else { return false };
    if body.as_bytes().get(cmd.value_start) != Some(&b'"') {
        return false;
    }
    let raw = &body[cmd.value_start + 1..cmd.value_end - 1];
    let got = Path::new(&unescape_json_string(raw)).file_name().map(|n| n.to_string_lossy().to_string());
    got.as_deref() == Some(want.as_str())
}

/// Вписывает наши две записи `context_servers` в текст `settings.json` Zed
/// (`text`), не трогая ничего чужого. `exe` — путь к программе, который
/// ляжет в `command`. `Err` — список имён, занятых чужими записями с нашим
/// именем: ни одна правка в этом случае не применяется (решение владельца,
/// «чужую запись не правим и не перезаписываем никогда»).
pub fn merge_zed(text: &str, exe: &Path) -> Result<String, Vec<String>> {
    let (bom, body) = split_bom(text);
    let nl = if body.contains("\r\n") { "\r\n" } else { "\n" };
    if body.trim().is_empty() {
        return Ok(format!("{bom}{}", fresh_document(exe, nl)));
    }
    let top_open = skip_ws_and_comments(body, 0);
    if body.as_bytes().get(top_open) != Some(&b'{') {
        // Не похоже на объект — не наш случай (битый файл), но и падать
        // незачем: заводим с нуля, как если бы файла не было вовсе.
        return Ok(format!("{bom}{}", fresh_document(exe, nl)));
    }
    let (top_fields, _) = parse_fields(body, top_open);
    let cs = top_fields.iter().find(|f| f.key == "context_servers");

    let mut edits = Vec::new();
    if let Some(cs_field) = cs.filter(|f| body.as_bytes().get(f.value_start) == Some(&b'{')) {
        let (cs_fields, _) = parse_fields(body, cs_field.value_start);
        let mut occupied = Vec::new();
        let mut missing = Vec::new();
        for e in OURS.iter() {
            match cs_fields.iter().find(|f| f.key == e.name) {
                Some(f) if is_ours(body, f, exe) => edits.push(Edit { start: f.key_start, end: f.value_end, text: entry_json(e, exe, nl) }),
                Some(_) => occupied.push(e.name.to_string()),
                None => missing.push(e),
            }
        }
        if !occupied.is_empty() {
            return Err(occupied);
        }
        if !missing.is_empty() {
            let has_other = !cs_fields.is_empty();
            let block = missing.iter().map(|e| entry_json(e, exe, nl)).collect::<Vec<_>>().join(&format!(",{nl}    "));
            let text = format!("{nl}    {block}{}", if has_other { "," } else { "" });
            edits.push(Edit { start: cs_field.value_start + 1, end: cs_field.value_start + 1, text });
        }
    } else {
        let has_other = !top_fields.is_empty();
        let text = format!("{nl}  \"context_servers\": {}{}", context_servers_value(exe, nl), if has_other { "," } else { "" });
        edits.push(Edit { start: top_open + 1, end: top_open + 1, text });
    }

    Ok(format!("{bom}{}", apply_edits(body, edits)))
}

/// `"1c"`/`"vrunner"`, выставленные `merge_zed`, несут явный `"enabled":
/// true` (решение владельца №1) — здесь мы этот ключ заменяем, а не
/// вставляем: правка только своих двух записей, всё остальное в файле не
/// задето.
pub fn set_enabled_zed(text: &str, on: bool) -> String {
    let (bom, body) = split_bom(text);
    let top_open = skip_ws_and_comments(body, 0);
    if body.as_bytes().get(top_open) != Some(&b'{') {
        return text.to_string();
    }
    let (top_fields, _) = parse_fields(body, top_open);
    let Some(cs) = top_fields.iter().find(|f| f.key == "context_servers") else { return text.to_string() };
    if body.as_bytes().get(cs.value_start) != Some(&b'{') {
        return text.to_string();
    }
    let (cs_fields, _) = parse_fields(body, cs.value_start);
    let mut edits = Vec::new();
    for e in OURS.iter() {
        let Some(f) = cs_fields.iter().find(|f| f.key == e.name) else { continue };
        if body.as_bytes().get(f.value_start) != Some(&b'{') {
            continue;
        }
        let (inner, _) = parse_fields(body, f.value_start);
        if let Some(en) = inner.iter().find(|x| x.key == "enabled") {
            edits.push(Edit { start: en.value_start, end: en.value_end, text: if on { "true" } else { "false" }.to_string() });
        }
    }
    format!("{bom}{}", apply_edits(body, edits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn an_absent_zed_file_is_created_with_only_our_entries() {
        let out = merge_zed("", Path::new("/opt/1c-connect")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
        assert_eq!(v["context_servers"]["1c"]["command"], "/opt/1c-connect");
        assert_eq!(v["context_servers"]["1c"]["args"], serde_json::json!(["mcp", "1c"]));
        assert_eq!(v["context_servers"]["1c"]["timeout"], 300);
        assert_eq!(v["context_servers"]["vrunner"]["timeout"], 3600);
    }

    #[test]
    fn foreign_keys_and_comments_survive_the_merge() {
        let before = "{\n  // моя тема\n  \"theme\": \"One Dark\",\n  \"context_servers\": {\n    \"bsl-reference\": {\"command\": \"/opt/bsl-analyzer\"}\n  }\n}\n";
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("// моя тема"), "{out}");
        assert!(out.contains("bsl-reference"), "{out}");
        assert!(out.contains("\"1c\""), "{out}");
    }

    #[test]
    fn a_foreign_entry_with_our_name_is_reported_not_overwritten() {
        let before = "{\"context_servers\":{\"1c\":{\"command\":\"nc\",\"args\":[\"-N\",\"localhost\",\"7771\"]}}}";
        assert_eq!(merge_zed(before, Path::new("/opt/1c-connect")).unwrap_err(), vec!["1c".to_string()]);
    }

    #[test]
    fn our_own_entry_is_updated_in_place() {
        let before = "{\"context_servers\":{\"1c\":{\"command\":\"/old/1c-connect\",\"args\":[\"mcp\",\"1c\"]}}}";
        let out = merge_zed(before, Path::new("/new/1c-connect")).unwrap();
        assert!(out.contains("/new/1c-connect") && !out.contains("/old/1c-connect"), "{out}");
    }

    #[test]
    fn a_windows_path_is_escaped_and_parses_back() {
        let out = merge_zed("", Path::new(r"C:\Users\Иван Петров\1c-connect.exe")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
        assert_eq!(v["context_servers"]["1c"]["command"], r"C:\Users\Иван Петров\1c-connect.exe");
    }

    #[test]
    fn line_endings_and_bom_survive_the_merge() {
        let before = "\u{feff}{\r\n  \"theme\": \"One\"\r\n}\r\n";
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.starts_with('\u{feff}') && out.contains("\r\n") && !out.contains("\n\n"), "{out:?}");
    }

    #[test]
    fn enabled_flips_without_touching_anything_else() {
        let on = merge_zed("", Path::new("/opt/1c-connect")).unwrap();
        let off = set_enabled_zed(&on, false);
        let v: serde_json::Value = serde_json::from_str(&as_json(&off)).unwrap();
        assert_eq!(v["context_servers"]["1c"]["enabled"], false);
        assert_eq!(v["context_servers"]["vrunner"]["enabled"], false);
        assert_eq!(set_enabled_zed(&off, true), on);
    }
}
