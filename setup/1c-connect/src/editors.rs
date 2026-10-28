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

/// Снимает комментарии `//` и `/* ... */`, затем висящую запятую перед `}`
/// или `]` — то, что `serde_json` (строгий JSON) не разбирает, а Zed
/// разбирает: его парсер настроек — JSONC, комментарии и висящая запятая
/// там свои, а не наша придумка. Боевой код, не помощник тестов: `merge_zed`
/// проверяет на нём и вход, и результат (`serde_json::from_str(&as_json(..))`)
/// — не разобрался вход, значит файл человека нам не по зубам, не разобрался
/// результат — значит где-то ошибся наш сканер границ; в обоих случаях
/// правильный ответ — отказ, а не тихая порча файла. Используется и в
/// `cfg.rs` (`zed_json`) — там она жила копией до этой задачи, теперь общее
/// место одно.
///
/// Сама хирургическая правка текста этой функции не видит вовсе: висящая
/// запятая человека остаётся в файле как есть, здесь только на минуту
/// снимается то, что мешало бы `serde_json` сказать «цел» или «не цел».
pub(crate) fn as_json(text: &str) -> String {
    strip_trailing_commas(&strip_comments(text))
}

/// Снимает комментарии `//` и `/* ... */` **вне строк**, чтобы JSONC можно
/// было разобрать как обычный JSON.
///
/// Байтовый скан, а не посимвольный: ASCII-пунктуация (`"`, `/`, `*`, `\`),
/// по которой идёт разбор, никогда не совпадает с продолжающим байтом
/// многобайтовой UTF-8 последовательности (кириллица и прочее — не ASCII,
/// то есть ≥ 0x80), поэтому резать строку по этим байтовым смещениям
/// безопасно: срез никогда не придётся на середину символа.
fn strip_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    let mut seg_start = 0usize; // начало копируемого "как есть" куска
    let mut i = 0usize;
    while i < b.len() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b[i] == b'\\' {
                escaped = true;
            } else if b[i] == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if b[i] == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            out.push_str(&text[seg_start..i]);
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            seg_start = i; // сам перевод строки (если есть) достанется следующему куску
            continue;
        }
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            out.push_str(&text[seg_start..i]);
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len()); // за `*/`, или до конца, если комментарий не закрыт
            seg_start = i;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[seg_start..]);
    out
}

/// Снимает запятую, за которой (через пробелы — комментариев тут уже нет,
/// их снял `strip_comments` раньше) идёт `}` или `]`, И перед которой (тем
/// же способом) стоит конец настоящего значения — закрывающая скобка,
/// кавычка или буква/цифра (хвост числа, `true`/`false`/`null`). Это и
/// отличает настоящую висящую запятую JSONC от голого мусора вроде `{,}`
/// или `{"a":,}`, у которого перед запятой нет значения вовсе — такой
/// мусор должен остаться отказом, а не превратиться в пустой объект.
fn strip_trailing_commas(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    let mut seg_start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b[i] == b'\\' {
                escaped = true;
            } else if b[i] == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if b[i] == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        if b[i] == b',' && is_a_real_trailing_comma(b, i) {
            out.push_str(&text[seg_start..i]);
            seg_start = i + 1;
        }
        i += 1;
    }
    out.push_str(&text[seg_start..]);
    out
}

/// Проверка для `strip_trailing_commas`: запятая в `i` — настоящая висящая,
/// а не мусор, если после неё (через пробелы) `}`/`]`, а перед ней (тем же
/// способом) — конец значения.
fn is_a_real_trailing_comma(b: &[u8], i: usize) -> bool {
    let mut after = i + 1;
    while after < b.len() && b[after].is_ascii_whitespace() {
        after += 1;
    }
    if !matches!(b.get(after), Some(&b'}') | Some(&b']')) {
        return false;
    }
    let mut before = i;
    while before > 0 && b[before - 1].is_ascii_whitespace() {
        before -= 1;
    }
    before > 0 && {
        let c = b[before - 1];
        matches!(c, b'}' | b']' | b'"') || c.is_ascii_alphanumeric()
    }
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

/// Файл с нуля — `settings.json` раньше не было, значит и чужого в нём нет.
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

/// Пропускает пробелы и комментарии (`//` и `/* ... */`), возвращая индекс
/// первого значимого байта. Скобки/кавычки внутри комментария этим и не
/// видны счёту глубины ниже — без этого закомментированный человеком старый
/// `"context_servers": {...}` читался бы как настоящий ключ, и наши записи
/// легли бы внутрь чужого комментария (а не рядом с ним).
fn skip_ws_and_comments(t: &str, mut i: usize) -> usize {
    let b = t.as_bytes();
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len());
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
            Some(&c) if c == b'/' && b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
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
                if b[j] == b'/' && matches!(b.get(j + 1), Some(&b'/') | Some(&b'*')) {
                    break;
                }
                j += 1;
            }
            while j > i && b[j - 1].is_ascii_whitespace() {
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
/// ляжет в `command`. `Err` — список причин словами: либо имена, занятых
/// чужими записями с нашим именем (решение владельца, «чужую запись не
/// правим и не перезаписываем никогда»), либо одна причина, почему файл не
/// разобрать. Пустой (или только из пробелов) вход — это всё ещё создание
/// свежего файла; всё прочее, что не разобралось как JSON-объект, —
/// отказ, а не попытка угадать и затереть то, что человек не поймёт как наш
/// формат (Critical 1 ревью задачи).
pub fn merge_zed(text: &str, exe: &Path) -> Result<String, Vec<String>> {
    let (bom, body) = split_bom(text);
    let nl = if body.contains("\r\n") { "\r\n" } else { "\n" };
    if body.trim().is_empty() {
        return Ok(format!("{bom}{}", fresh_document(exe, nl)));
    }
    // Разбор целиком, а не только по скобкам: битый JSON (незакрытый объект,
    // голый токен, файл из одних комментариев) обязан быть отказом здесь, а
    // не наполовину написанным файлом (Critical 2 ревью задачи).
    let parsed: serde_json::Value = serde_json::from_str(&as_json(body))
        .map_err(|e| vec![format!("settings.json не разбирается как JSON (после снятия комментариев): {e}")])?;
    if !parsed.is_object() {
        return Err(vec!["верхний уровень settings.json — не объект".to_string()]);
    }
    let top_open = skip_ws_and_comments(body, 0);
    if body.as_bytes().get(top_open) != Some(&b'{') {
        // Разобралось как объект, но границу не нашли — наш сканер и
        // serde_json разошлись; не должно происходить, но отказ дешевле
        // паники или порчи файла.
        return Err(vec!["не нашли границу верхнего объекта settings.json".to_string()]);
    }
    let (top_fields, _) = parse_fields(body, top_open);
    let cs = top_fields.iter().find(|f| f.key == "context_servers");

    let mut edits = Vec::new();
    match cs {
        Some(cs_field) => {
            if body.as_bytes().get(cs_field.value_start) != Some(&b'{') {
                // Человек положил в context_servers не объект (строку,
                // массив) — вписать рядом свой второй ключ с тем же именем
                // значило бы молча проиграть чужому (в JSON побеждает
                // последний одноимённый ключ), поэтому отказ (Important 2
                // ревью задачи).
                return Err(vec!["context_servers в settings.json — не объект".to_string()]);
            }
            let (cs_fields, _) = parse_fields(body, cs_field.value_start);
            let mut occupied = Vec::new();
            let mut missing = Vec::new();
            for e in OURS.iter() {
                match cs_fields.iter().find(|f| f.key == e.name) {
                    Some(f) if is_ours(body, f, exe) => {
                        edits.push(Edit { start: f.key_start, end: f.value_end, text: entry_json(e, exe, nl) })
                    }
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
        }
        None => {
            let has_other = !top_fields.is_empty();
            let text = format!("{nl}  \"context_servers\": {}{}", context_servers_value(exe, nl), if has_other { "," } else { "" });
            edits.push(Edit { start: top_open + 1, end: top_open + 1, text });
        }
    }

    let edited = apply_edits(body, edits);
    // Страховка от собственной ошибки сканера: правка обязана остаться
    // валидным JSON, иначе Zed отвергнет файл целиком, а мы отрапортуем
    // успех (Critical 2 ревью задачи).
    if serde_json::from_str::<serde_json::Value>(&as_json(&edited)).is_err() {
        return Err(vec!["правка дала невалидный JSON — отказ, файл не менялся".to_string()]);
    }
    Ok(format!("{bom}{edited}"))
}

/// `"1c"`/`"vrunner"`, выставленные `merge_zed`, несут явный `"enabled":
/// true` (решение владельца №1) — здесь мы этот ключ заменяем, а не
/// вставляем: правка только своих двух записей, всё остальное в файле не
/// задето.
///
/// `exe` различает два случая, а не два отдельных имени функции:
/// `Some(exe)` — машина человека, и трогаем только записи, чей `command`
/// действительно указывает на этот исполняемый (та же проверка, что в
/// `merge_zed`) — иначе чужая запись с нашим именем (например, `nc` на
/// порту `1c`) гасилась бы вместе с нашей (Important 1 ревью задачи).
/// `None` — контейнер: там имя `1c`/`vrunner` наше по построению (эту
/// запись туда положила наша же правка по SSH, задача 7), а `command` —
/// `nc` на проброшенный порт, и сверять его с путём к `1c-connect` незачем
/// и нечем.
pub fn set_enabled_zed(text: &str, on: bool, exe: Option<&Path>) -> String {
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
        if let Some(exe) = exe {
            if !is_ours(body, f, exe) {
                continue;
            }
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

    /// Переводы строк — только `\r\n`, ни одного одиночного `\n`: слабая
    /// проверка `out.contains("\r\n")` проходит и за счёт нетронутых строк,
    /// если наша вставка потеряла перевод строки где-то в своём тексте,
    /// этого бы не заметили (находка ревью задачи).
    fn only_crlf_newlines(s: &str) -> bool {
        let b = s.as_bytes();
        (0..b.len()).filter(|&i| b[i] == b'\n').all(|i| i > 0 && b[i - 1] == b'\r')
    }

    #[test]
    fn line_endings_and_bom_survive_the_merge() {
        let before = "\u{feff}{\r\n  \"theme\": \"One\"\r\n}\r\n";
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.starts_with('\u{feff}') && out.contains("\r\n") && only_crlf_newlines(&out), "{out:?}");
    }

    #[test]
    fn enabled_flips_without_touching_anything_else() {
        let on = merge_zed("", Path::new("/opt/1c-connect")).unwrap();
        let off = set_enabled_zed(&on, false, Some(Path::new("/opt/1c-connect")));
        let v: serde_json::Value = serde_json::from_str(&as_json(&off)).unwrap();
        assert_eq!(v["context_servers"]["1c"]["enabled"], false);
        assert_eq!(v["context_servers"]["vrunner"]["enabled"], false);
        assert_eq!(set_enabled_zed(&off, true, Some(Path::new("/opt/1c-connect"))), on);
    }

    /// Important 1 ревью задачи: чужая запись с нашим именем (`nc` на
    /// порту, человек прописал руками) не гасится вместе с нашей, когда мы
    /// знаем, кто мы (`Some(exe)` — машина человека).
    #[test]
    fn set_enabled_zed_with_exe_skips_a_foreign_record_with_our_name() {
        let before = "{\"context_servers\":{\"1c\":{\"command\":\"nc\",\"args\":[\"-N\",\"localhost\",\"7771\"],\"enabled\":true}}}";
        let out = set_enabled_zed(before, false, Some(Path::new("/opt/1c-connect")));
        assert_eq!(out, before, "чужая запись с нашим именем не должна меняться");
    }

    /// Контейнерный случай (задача 7): `exe` неизвестен или не к месту,
    /// имени доверяем по построению — запись `1c`/`vrunner` туда положила
    /// только наша же правка.
    #[test]
    fn set_enabled_zed_without_exe_trusts_the_name_container_case() {
        let before = "{\"context_servers\":{\"1c\":{\"command\":\"nc\",\"args\":[\"-N\",\"localhost\",\"7771\"],\"enabled\":true},\"vrunner\":{\"command\":\"nc\",\"enabled\":true}}}";
        let out = set_enabled_zed(before, false, None);
        let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
        assert_eq!(v["context_servers"]["1c"]["enabled"], false);
        assert_eq!(v["context_servers"]["vrunner"]["enabled"], false);
    }

    /// Critical 1 ревью задачи: текст, который не разбирается как JSON
    /// вовсе, — отказ, а не наш файл вместо содержимого человека.
    #[test]
    fn garbage_that_is_not_json_is_a_refusal_not_a_silent_overwrite() {
        assert!(merge_zed("это вообще не json", Path::new("/opt/1c-connect")).is_err());
    }

    /// Файл из одних комментариев после их снятия — пустой текст, а не
    /// пустой ВХОД: в JSON это тоже не разбирается, и это тоже отказ, а не
    /// «файла как бы не было».
    #[test]
    fn a_comments_only_file_is_a_refusal() {
        assert!(merge_zed("// только комментарии\n", Path::new("/opt/1c-connect")).is_err());
    }

    /// Critical 2 ревью задачи: объект начат, но не закрыт и несёт голый
    /// токен — Zed отверг бы такой файл целиком, и мы обязаны отказом, а не
    /// наполовину написанным (и при этом как бы успешным) результатом.
    #[test]
    fn an_unparseable_object_is_a_refusal_not_a_half_written_file() {
        let before = "{\n  \"theme\": \"One\",\n  oops\n";
        assert!(merge_zed(before, Path::new("/opt/1c-connect")).is_err());
    }

    /// Important 2 ревью задачи: `context_servers` — строка, не объект.
    /// Вписать рядом свой второй ключ с тем же именем значило бы молча
    /// проиграть чужому (в JSON побеждает последний одноимённый ключ).
    #[test]
    fn context_servers_that_is_not_an_object_is_a_refusal() {
        let before = "{\"context_servers\": \"выключено руками\"}";
        assert!(merge_zed(before, Path::new("/opt/1c-connect")).is_err());
    }

    /// Висящая запятая в объекте — то, что Zed (его разбор — JSONC) читает
    /// как обычный файл. Отказ здесь был бы «программа не умеет» при
    /// исправно работающем редакторе, поэтому `as_json` её терпит на
    /// проверке — а сама правка текста её не трогает: запятая человека
    /// остаётся на месте, мы её не «исправляем».
    #[test]
    fn a_dangling_comma_in_an_object_is_tolerated_like_zed_tolerates_it() {
        let before = "{\n  // старый сервер\n  \"context_servers\": {\"other\": {\"command\": \"x\"},}\n}\n";
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("// старый сервер"), "{out}");
        assert!(out.contains("\"other\": {\"command\": \"x\"},"), "запятую человека не трогаем: {out}");
        assert!(out.contains("\"1c\""), "{out}");
    }

    /// То же для массива — `args` человека с висящей запятой переживает
    /// правку нетронутым.
    #[test]
    fn a_dangling_comma_in_an_array_is_tolerated_like_zed_tolerates_it() {
        let before = "{\"context_servers\":{\"other\":{\"command\":\"x\",\"args\":[\"a\",\"b\",]}}}";
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("[\"a\",\"b\",]"), "запятую в массиве не трогаем: {out}");
        assert!(out.contains("\"1c\""), "{out}");
    }

    /// Обратная сторона: терпимость к висящей запятой не должна проглотить
    /// настоящий мусор, где перед запятой нет значения вовсе.
    #[test]
    fn a_comma_with_no_preceding_value_stays_a_refusal() {
        assert!(merge_zed("{\"a\":,}", Path::new("/opt/1c-connect")).is_err());
        assert!(merge_zed("{,}", Path::new("/opt/1c-connect")).is_err());
    }

    /// Фигурные скобки и кавычки внутри строковых значений не должны
    /// сбивать счёт глубины у `skip_bracket`/`parse_fields` — иначе границу
    /// чужой записи (а за ней и свою, следом) нашли бы не там.
    #[test]
    fn braces_and_quotes_inside_string_values_do_not_confuse_the_scanner() {
        let before = r#"{"context_servers":{"weird":{"command":"/opt/{a}\"b\"","args":[]}}}"#;
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("weird"), "{out}");
        assert!(out.contains("\"1c\""), "{out}");
    }

    /// Экранированная обратная косая перед закрывающей кавычкой: два
    /// символа `\\` в JSON-тексте — это один настоящий `\`, и кавычка
    /// сразу после него закрывает строку, а не экранируется ей. Собьётся
    /// счёт — `merge_zed` вернёт `Err` (результат не разберётся как JSON),
    /// а не тихо сдвинутые границы.
    #[test]
    fn an_escaped_backslash_before_a_closing_quote_does_not_desync_the_scanner() {
        let before = r#"{"context_servers":{"other":{"command":"C:\\","args":[]}}}"#;
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("other") && out.contains("\"1c\""), "{out}");
    }

    /// Critical 3 ревью задачи: блочный комментарий ПЕРЕД `context_servers`
    /// — текст комментария переживает правку, запись садится в настоящий
    /// (не закомментированный) объект.
    #[test]
    fn a_block_comment_before_context_servers_survives_the_merge() {
        let before = "{\n  /* старый конфиг */\n  \"context_servers\": {}\n}\n";
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("/* старый конфиг */"), "{out}");
        assert!(out.contains("\"1c\""), "{out}");
    }

    /// Блочный комментарий ПОСЛЕ `context_servers`, перед закрывающей
    /// скобкой объекта — не мешает найти границу последнего поля.
    #[test]
    fn a_block_comment_after_context_servers_survives_the_merge() {
        let before = "{\n  \"context_servers\": {}\n  /* хвост */\n}\n";
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("/* хвост */"), "{out}");
        assert!(out.contains("\"1c\""), "{out}");
    }

    /// Самый опасный случай Critical 3: `context_servers` настоящего нет,
    /// есть только закомментированный блок с таким же текстом — человек так
    /// отключил старый конфиг. Наш ключ не должен слиться с чужим, и
    /// закомментированное имя `legacy` не должно всплыть как настоящая
    /// запись.
    #[test]
    fn a_fully_commented_out_context_servers_block_does_not_fool_the_scanner() {
        let before = "{\n  \"theme\": \"One\"\n  /* \"context_servers\": { \"legacy\": {\"command\": \"x\"} } */\n}\n";
        let out = merge_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("/* \"context_servers\""), "комментарий должен выжить: {out}");
        let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
        assert_eq!(v["context_servers"]["1c"]["timeout"], 300);
        assert!(v["context_servers"]["legacy"].is_null(), "закомментированное имя не должно всплыть: {out}");
    }
}
