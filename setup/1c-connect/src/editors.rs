//! Запись коннектора в настройки редактора на машине человека: у Zed правка
//! текстовая, по границам наших двух записей `context_servers` — его файл
//! несёт комментарии, и разобрать документ целиком и напечатать заново
//! нельзя. У pi (`merge_pi`) комментариев в `mcp.json` не бывает, терять
//! нечего — там честный разбор `serde_json` целиком и печать заново.

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

/// Есть ли на этой машине Zed вообще — факт, отдельный от того, настроен ли
/// он нами (дефект живого прогона 29.10: `look` раньше судил «Zed есть» по
/// наличию `settings.json`, а у только что установленного и ни разу не
/// открытого Zed этого файла нет вовсе — каталог `%APPDATA%\Zed` появляется
/// при установке, файл настроек — только при первом открытии). Признак —
/// каталог настроек (родитель `zed_path()`) существует, ИЛИ `zed`/`zed.exe`
/// нашёлся на `PATH`; любой из двух достаточен — мы не можем знать заранее,
/// каким способом редактор поставлен.
pub fn zed_present() -> bool {
    zed_path().and_then(|p| p.parent().map(|d| d.exists())).unwrap_or(false) || on_path("zed")
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

/// Удаляет поля с именами из `names` внутри одного объекта, чьи прямые поля
/// — `fields` (порядок — порядок в исходном тексте, как отдаёт
/// `parse_fields`), возвращая список правок без пересечений — даже когда
/// удаляемые поля стоят в тексте друг за другом (иначе граница одного
/// „съедала" бы запятую, которую уже забрала граница соседнего). Соседние по
/// позиции имена из `names` обрабатываются одним куском: берём запятую ПОСЛЕ
/// последнего поля куска, если кусок не последний в объекте, иначе запятую
/// ПЕРЕД первым полем куска (кусок последний, но не единственный); кусок,
/// занимающий весь объект, уносит содержимое без запятых вовсе.
fn remove_fields_by_name(body: &str, fields: &[Field], names: &[&str]) -> Vec<Edit> {
    let mut idxs: Vec<usize> = fields.iter().enumerate().filter(|(_, f)| names.contains(&f.key.as_str())).map(|(i, _)| i).collect();
    idxs.sort_unstable();
    let mut edits = Vec::new();
    let mut k = 0;
    while k < idxs.len() {
        let (start_i, mut end_i) = (idxs[k], idxs[k]);
        while k + 1 < idxs.len() && idxs[k + 1] == end_i + 1 {
            k += 1;
            end_i = idxs[k];
        }
        let (first, last) = (&fields[start_i], &fields[end_i]);
        let (start, end) = if end_i + 1 < fields.len() {
            let after = skip_ws_and_comments(body, last.value_end);
            let comma_end = if body.as_bytes().get(after) == Some(&b',') { after + 1 } else { last.value_end };
            (first.key_start, comma_end)
        } else if start_i > 0 {
            (fields[start_i - 1].value_end, last.value_end)
        } else {
            (first.key_start, last.value_end)
        };
        edits.push(Edit { start, end, text: String::new() });
        k += 1;
    }
    edits
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

/// Снимаем галочку «Zed» в трее: убираем наши две записи `context_servers`
/// из текста `settings.json`, не трогая ничего чужого — обратное к
/// `merge_zed`. Чужая запись с нашим именем не удаляется, это отказ, как и
/// при записи (решение владельца, «чужую запись не правим и не
/// перезаписываем никогда»). Пустой текст (файла и не было) или текст без
/// наших записей — `Ok` с тем же текстом без изменений: состояние уже
/// такое, какое просили, это успех, а не повод что-то переписывать на диске.
pub fn remove_zed(text: &str, exe: &Path) -> Result<String, Vec<String>> {
    let (bom, body) = split_bom(text);
    if body.trim().is_empty() {
        return Ok(text.to_string());
    }
    let parsed: serde_json::Value = serde_json::from_str(&as_json(body))
        .map_err(|e| vec![format!("settings.json не разбирается как JSON (после снятия комментариев): {e}")])?;
    if !parsed.is_object() {
        return Err(vec!["верхний уровень settings.json — не объект".to_string()]);
    }
    let top_open = skip_ws_and_comments(body, 0);
    if body.as_bytes().get(top_open) != Some(&b'{') {
        return Err(vec!["не нашли границу верхнего объекта settings.json".to_string()]);
    }
    let (top_fields, _) = parse_fields(body, top_open);
    let Some(cs_field) = top_fields.iter().find(|f| f.key == "context_servers") else {
        return Ok(text.to_string()); // ключа нет вовсе — нечего убирать
    };
    if body.as_bytes().get(cs_field.value_start) != Some(&b'{') {
        return Err(vec!["context_servers в settings.json — не объект".to_string()]);
    }
    let (cs_fields, _) = parse_fields(body, cs_field.value_start);
    let mut occupied = Vec::new();
    let mut remove_names: Vec<&str> = Vec::new();
    for e in OURS.iter() {
        if let Some(f) = cs_fields.iter().find(|f| f.key == e.name) {
            if is_ours(body, f, exe) {
                remove_names.push(e.name);
            } else {
                occupied.push(e.name.to_string());
            }
        }
    }
    if !occupied.is_empty() {
        return Err(occupied);
    }
    if remove_names.is_empty() {
        return Ok(text.to_string());
    }
    let edited = apply_edits(body, remove_fields_by_name(body, &cs_fields, &remove_names));
    if serde_json::from_str::<serde_json::Value>(&as_json(&edited)).is_err() {
        return Err(vec!["правка дала невалидный JSON — отказ, файл не менялся".to_string()]);
    }
    Ok(format!("{bom}{edited}"))
}

/// Путь к `mcp.json` pi на этой машине: `~/.pi/agent/mcp.json`, на Windows —
/// под `%USERPROFILE%` (путь задан самим pi, не нами — спека «MCP в
/// редакторе»). `None` — переменной окружения, по которой путь строится, нет.
pub fn pi_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".pi").join("agent").join("mcp.json"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".pi").join("agent").join("mcp.json"))
    }
}

/// Есть ли на этой машине pi вообще — та же причина, что у `zed_present`
/// (дефект живого прогона 29.10: на машине владельца pi стоял — `~/.pi/agent`
/// с `settings.json`, `auth.json`, `bin/pi`, `sessions` — но `mcp.json` в нём
/// ни разу не создавался, и старый `look` читал это как «pi нет вовсе»).
/// Признак — любой из трёх: каталог `~/.pi`, каталог `~/.pi/agent` (родитель
/// `pi_path()`), или `pi`/`pi.exe` на `PATH`.
pub fn pi_present() -> bool {
    let home = if cfg!(windows) { std::env::var_os("USERPROFILE") } else { std::env::var_os("HOME") };
    if let Some(h) = home {
        let base = PathBuf::from(h).join(".pi");
        if base.exists() || base.join("agent").exists() {
            return true;
        }
    }
    on_path("pi")
}

/// Ищет `name` (с расширениями-кандидатами на Windows) в каждом каталоге
/// `PATH` — свой маленький `which`, без нового зависимого крейта: ровно
/// столько кода, сколько нужно для одного вопроса «исполняемый файл с таким
/// именем вообще есть на этой машине».
///
/// Отдаёт НАЙДЕННЫЙ путь, а не `bool`, потому что на оба вопроса — «агент на
/// машине есть» и «чем его запускать» — обязан отвечать один и тот же поиск.
/// Замер 30.10: `codex` на этой машине поставлен через npm
/// (`...\Roaming\npm\codex.cmd`, `.exe` нет вовсе), и
/// `Command::new("codex")` падал с «program not found» — на Windows
/// `CreateProcess` дописывает к имени только `.exe` и `PATHEXT` не читает.
/// Пункт меню при этом был доступен (`on_path` `.cmd` принимает нарочно), а
/// каждое включение падало: «видим» и «можем запустить» расходились. По
/// полному пути к `.cmd` тот же запуск работает — замерено
/// (`codex-cli 0.145.0`), батник std заворачивает в `cmd.exe` сам.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    // ponytail: расширения из `PATHEXT` не читаются — перебор не покроет `.ps1`/`.com`/`.py`/`.vbs`, только то, что здесь перечислено вручную. Признано некритичным в ре-ревью задачи 7: каталог настроек (`zed_present`) — первый, более надёжный признак, а `.cmd` уже закрывает установку pi через npm. Поднять до чтения `PATHEXT`, если найдётся редактор, ставящийся именно одним из непокрытых расширений.
    std::env::split_paths(&path_var)
        .flat_map(|dir| ["", ".exe", ".cmd", ".bat"].map(move |ext| dir.join(format!("{name}{ext}"))))
        .find(|p| p.is_file())
}

/// Тот же поиск, когда нужен только факт присутствия.
fn on_path(name: &str) -> bool {
    find_on_path(name).is_some()
}

/// Запись в `mcpServers` вышла бы нашей по тому же правилу, что у Zed: имя
/// файла в `command` совпадает с именем файла нашего исполняемого.
fn pi_entry_is_ours(entry: &serde_json::Value, exe: &Path) -> bool {
    let Some(want) = exe.file_name().map(|n| n.to_string_lossy().to_string()) else { return false };
    let Some(cmd) = entry.get("command").and_then(|c| c.as_str()) else { return false };
    Path::new(cmd).file_name().map(|n| n.to_string_lossy().to_string()).as_deref() == Some(want.as_str())
}

/// Значение одной нашей записи `mcpServers` для pi — те же поля, что у Zed
/// (`command`, `args`, `timeout`), без `enabled`: этого ключа в формате pi
/// нет (спека «MCP в редакторе», пример `config/pi/mcp.json`).
fn pi_entry(e: &Entry, exe: &Path) -> serde_json::Value {
    serde_json::json!({
        "command": exe.display().to_string(),
        "args": ["mcp", e.name],
        "timeout": e.timeout,
    })
}

/// Вписывает наши две записи `mcpServers` в `mcp.json` pi (`text`), не трогая
/// ничего чужого. В отличие от `merge_zed`, здесь честный разбор целиком
/// через `serde_json` и печать заново, а не текстовая хирургия по границам:
/// `mcp.json` комментариев не допускает, терять нечего (решение владельца
/// №1). Чужие ключи верхнего уровня и внутри `mcpServers` сохраняются все,
/// но порядок ключей не обещан — `preserve_order` не включаем, цена фичи
/// ради вида файла не та (решение владельца №2). Правило чужой записи то
/// же, что у Zed: имя совпало, а команда чужая — отказ со списком имён, без
/// правки файла (решение владельца №3).
///
/// BOM в начале файла — тоже отказ, но не терпимость, как у Zed с висящей
/// запятой: Zed сам читает JSONC с комментарием и запятой, а pi сам такой
/// файл не прочтёт (BOM не входит в чистый JSON). Тихо переписать его без
/// BOM значило бы отрапортовать успех о файле, который pi всё равно не
/// подхватит молча — отказ называет причину по имени, чтобы человек понял,
/// что чинить (пересохранить файл в UTF-8 без метки), а не наткнулся на
/// общее «не разбирается как JSON».
pub fn merge_pi(text: &str, exe: &Path) -> Result<String, Vec<String>> {
    if text.starts_with('\u{feff}') {
        return Err(
            vec!["mcp.json начинается с BOM — pi сам не прочтёт такой файл; пересохраните его в UTF-8 без метки (BOM)".to_string()],
        );
    }
    let mut top: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|e| vec![format!("mcp.json не разбирается как JSON: {e}")])?
    };
    let Some(obj) = top.as_object_mut() else {
        return Err(vec!["верхний уровень mcp.json — не объект".to_string()]);
    };
    let servers_slot = obj.entry("mcpServers").or_insert_with(|| serde_json::json!({}));
    let Some(servers) = servers_slot.as_object_mut() else {
        return Err(vec!["mcpServers в mcp.json — не объект".to_string()]);
    };

    let occupied: Vec<String> =
        OURS.iter().filter(|e| servers.get(e.name).is_some_and(|f| !pi_entry_is_ours(f, exe))).map(|e| e.name.to_string()).collect();
    if !occupied.is_empty() {
        return Err(occupied);
    }
    for e in OURS.iter() {
        servers.insert(e.name.to_string(), pi_entry(e, exe));
    }

    Ok(serde_json::to_string_pretty(&top).expect("serde_json::Value всегда сериализуется") + "\n")
}

/// Снимаем галочку «pi» в трее — обратное к `merge_pi`, тот же честный
/// разбор и печать заново целиком: `mcp.json` комментариев не допускает,
/// терять нечего. Правило чужой записи то же, что у Zed: имя совпало, а
/// команда чужая — отказ со списком имён, файл не трогаем. Файла нет, или
/// `mcpServers` нет, или наших записей там и так нет — `Ok` без изменений
/// текста: состояние уже такое, какое просили.
pub fn remove_pi(text: &str, exe: &Path) -> Result<String, Vec<String>> {
    if text.starts_with('\u{feff}') {
        return Err(
            vec!["mcp.json начинается с BOM — pi сам не прочтёт такой файл; пересохраните его в UTF-8 без метки (BOM)".to_string()],
        );
    }
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }
    let mut top: serde_json::Value = serde_json::from_str(text).map_err(|e| vec![format!("mcp.json не разбирается как JSON: {e}")])?;
    let Some(obj) = top.as_object_mut() else {
        return Err(vec!["верхний уровень mcp.json — не объект".to_string()]);
    };
    let Some(servers_slot) = obj.get_mut("mcpServers") else {
        return Ok(text.to_string()); // ключа нет вовсе — нечего убирать
    };
    let Some(servers) = servers_slot.as_object_mut() else {
        return Err(vec!["mcpServers в mcp.json — не объект".to_string()]);
    };
    let occupied: Vec<String> =
        OURS.iter().filter(|e| servers.get(e.name).is_some_and(|f| !pi_entry_is_ours(f, exe))).map(|e| e.name.to_string()).collect();
    if !occupied.is_empty() {
        return Err(occupied);
    }
    let mut changed = false;
    for e in OURS.iter() {
        changed |= servers.remove(e.name).is_some();
    }
    if !changed {
        return Ok(text.to_string());
    }
    Ok(serde_json::to_string_pretty(&top).expect("serde_json::Value всегда сериализуется") + "\n")
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

/// Опознанное состояние файла настроек одного редактора — то, что раздел
/// «Редакторы» в окне говорит человеку (спека, «Как это выглядит у
/// человека»): не найден, найден без наших записей, настроен нами, или
/// хотя бы одна наша запись занята чужой командой (имена — словами).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Missing,
    Absent,
    Ours,
    Foreign(Vec<String>),
}

/// Опознание по уже прочитанному пути И по отдельному факту «редактор на
/// машине есть» (`present` — `zed_present`/`pi_present`). Эти два факта
/// РАЗНЫЕ, и путать их — дефект, найденный живым прогоном 29.10: на машине
/// владельца pi стоял (`~/.pi/agent`, свои `settings.json`/`auth.json`/`bin`),
/// но `mcp.json` у него ни разу не создавался, а старый `look` судил
/// «редактор есть» по наличию ИМЕННО ЭТОГО файла — и запирал кнопку там, где
/// её нажатие должно было создать файл с нуля (`merge_zed`/`merge_pi` на
/// пустом тексте это умеют). То же ждало только что установленный, но ни
/// разу не открытый Zed — самый частый первый запуск из всех.
///
/// `present=false` — редактора нет вовсе: `Found::Missing`, нажимать нечего.
/// `present=true` — редактор есть, и дальше смотрим файл настроек как обычно:
/// нет файла, не читается, не разбирается, нет нашего ключа, нет наших
/// записей — всё это `Found::Absent` («найден, серверы не настроены»), а не
/// `Missing`.
///
/// Генерична по обоим редакторам сразу: ищет контейнер записей по любому из
/// двух имён ключа — `context_servers` у Zed, `mcpServers` у pi; тот
/// редактор, которого это не касается, такого ключа и не заводит.
/// Комментарии и висящая запятая (Zed, JSONC) снимает `as_json` — на честном
/// JSON pi для неё нечего снимать, и вызов безвреден.
pub fn look(path: Option<&Path>, exe: &Path, present: bool) -> Found {
    if !present {
        return Found::Missing;
    }
    let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else { return Found::Absent };
    let (_, body) = split_bom(&text);
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&as_json(body)) else { return Found::Absent };
    let Some(obj) = parsed.as_object() else { return Found::Absent };
    let Some(servers) = obj.get("context_servers").or_else(|| obj.get("mcpServers")).and_then(|v| v.as_object()) else {
        return Found::Absent;
    };
    let mut foreign = Vec::new();
    // Important 2 ревью задачи 6: «настроен» — только когда на месте ВСЕ
    // наши записи, а не хоть одна. Половина (`1c` есть, `vrunner` нет) — это
    // всё ещё «серверы не настроены»: раздел обязан говорить о файле, а не
    // хвалить его за часть работы, которую сделает только следующее нажатие
    // «Настроить» (оно же допишет недостающую запись, `merge_zed`/`merge_pi`
    // это уже умеют).
    let mut ours_count = 0usize;
    for e in OURS.iter() {
        match servers.get(e.name) {
            Some(entry) if pi_entry_is_ours(entry, exe) => ours_count += 1,
            Some(_) => foreign.push(e.name.to_string()),
            None => {}
        }
    }
    if !foreign.is_empty() {
        return Found::Foreign(foreign);
    }
    if ours_count == OURS.len() {
        Found::Ours
    } else {
        Found::Absent
    }
}

/// Факт набора готовности в контейнере (задача 7, решение владельца 29.10): те же записи `context_servers`, что `look` ищет на машине человека, но опознаются по имени, а не по `command` — там команда `nc` на проброшенный порт, а не наш исполняемый, и это не чужая рука: имя `1c`/`vrunner` в контейнере наше по построению (та же посылка, что у `set_enabled_zed(text, on, None)`). Переиспользует текст, который уже прочитан ради правки `enabled` (`session::flip_with`) — второго похода по SSH ради одного этого факта быть не должно.
///
/// Только `Ours`/`Absent`: в контейнере «чужой» записи с нашим именем не бывает по построению, и различать `Missing` (файла вовсе нет) от `Absent` (файл есть, записей нет) здесь незачем — обеим одна и та же причина у человека: «обновите образ».
pub fn found_by_name(text: &str) -> Found {
    let (_, body) = split_bom(text);
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&as_json(body)) else { return Found::Absent };
    let Some(obj) = parsed.as_object() else { return Found::Absent };
    let Some(servers) = obj.get("context_servers").and_then(|v| v.as_object()) else { return Found::Absent };
    if OURS.iter().all(|e| servers.contains_key(e.name)) {
        Found::Ours
    } else {
        Found::Absent
    }
}

/// Путь рядом с файлом настроек, с добавленным хвостом — не заменяющим
/// расширение (`settings.json` → `settings.json.bak`, а не `settings.bak`).
fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// Жмём «Настроить»: читаем файл редактора (нет на диске — пустой текст,
/// это всё ещё создание свежего файла, как `merge_zed`/`merge_pi` и так
/// понимают), просим `merge` вписать наши две записи, и, если он согласился
/// (не занял чужого), пишем результат через временный файл рядом и
/// `rename` — тот же приём, которым раскладка настроек в образе кладёт свои
/// файлы (`config/entrypoint.sh`), и он здесь нужен ровно потому, что файл
/// чужой: один прерванный процесс записи не должен оставить человека с
/// половиной JSON. `.bak` исходника берём только на первой правке — был он
/// или нет, повторную правку уже снятой копией не перетираем (решение
/// владельца №1).
///
/// `Err` — ничего не пишем, причина отказа такая же, какую отдал `merge`
/// (имена занятых чужими записями, либо одна причина, почему файл не
/// разобрать). `Ok` несёт не «получилось», раз `merge` не вернул `Err`, а то,
/// что `look` увидел на диске ПОСЛЕ записи — раздел говорит о файле, а не о
/// нашем намерении (решение владельца №2).
pub fn setup(path: &Path, exe: &Path, merge: impl Fn(&str, &Path) -> Result<String, Vec<String>>) -> Result<Found, Vec<String>> {
    let existed = path.exists();
    let text = if existed {
        std::fs::read_to_string(path).map_err(|e| vec![format!("не прочитать {}: {e}", path.display())])?
    } else {
        String::new()
    };
    let merged = merge(&text, exe)?;
    // Important 3 ревью задачи 6: `path` может быть символьной ссылкой —
    // человек держит настройки в репозитории dotfiles, обычное дело. `rename`
    // прямо на такой путь заменил бы саму ссылку обычным файлом: настоящий
    // файл остался бы нетронутым и отвязанным, данные целы, а связь потеряна
    // без единого слова об этом. Канонизируем, чтобы писать СКВОЗЬ ссылку, в
    // настоящий файл; не вышло (файла ещё нет — свежее создание, либо путь
    // не существует вовсе) — пишем по тому пути, что дали, как и раньше.
    let target = if existed { std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) } else { path.to_path_buf() };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| vec![format!("не создать каталог {}: {e}", parent.display())])?;
    }
    if existed {
        let bak = sibling_path(&target, ".bak");
        if !bak.exists() {
            std::fs::copy(&target, &bak).map_err(|e| vec![format!("не сохранить резервную копию {}: {e}", bak.display())])?;
        }
    }
    let tmp = sibling_path(&target, ".tmp");
    if let Err(e) = write_and_rename(&tmp, &target, &merged) {
        // Minor 6 ревью задачи 6: осиротевший `.tmp` не должен остаться на
        // диске после отказа — ни от самой записи, ни от переименования.
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // Мы только что создали/переписали этот файл — редактор, очевидно, есть.
    Ok(look(Some(path), exe, true))
}

/// Снял галочку: просим `remove` убрать наши записи и, если текст
/// изменился, переписываем файл тем же атомарным приёмом, что и `setup`
/// (временный файл + `rename`) — без резервной копии: это обратное действие,
/// а не правка чужого содержимого, катастрофы восстанавливать не из чего.
/// Файла на диске не было — `remove` на пустом тексте и так отдаёт пустой
/// текст, запись на диск не нужна вовсе (решение владельца, «файла нет —
/// это успех, а не ошибка»). Итог, как и у `setup`, — то, что `look` увидел
/// на диске ПОСЛЕ правки, а не наше предположение об успехе.
pub fn teardown(path: &Path, exe: &Path, remove: impl Fn(&str, &Path) -> Result<String, Vec<String>>) -> Result<Found, Vec<String>> {
    let text = if path.exists() {
        std::fs::read_to_string(path).map_err(|e| vec![format!("не прочитать {}: {e}", path.display())])?
    } else {
        String::new()
    };
    let removed = remove(&text, exe)?;
    if removed != text {
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let tmp = sibling_path(&target, ".tmp");
        if let Err(e) = write_and_rename(&tmp, &target, &removed) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    Ok(look(Some(path), exe, true))
}

/// Пишет во временный файл и переименовывает его в целевой — вынесено одним
/// местом, чтобы `setup` убирал осиротевший `.tmp` на любой из двух ошибок
/// одной веткой, не повторяя чистку дважды.
///
/// ponytail: `File::sync_all` перед `rename` не зовём — пропавшее питание
/// между записью и переименованием оставило бы пустой временный файл, а
/// `rename` его не тронул бы вовсе (целевой файл и `.bak` целы); обратный
/// порядок (питание пропало между `rename` и возвратом) `.bak` всё равно
/// спасает. Потолок этого — пустой или укороченный `.tmp`, который никогда
/// не становится целевым файлом: поднять до `sync_all`, если это окажется
/// недостаточным (найдутся файловые системы, где незавершённая `rename`
/// видна снаружи раньше `fsync`).
fn write_and_rename(tmp: &Path, target: &Path, text: &str) -> Result<(), Vec<String>> {
    std::fs::write(tmp, text).map_err(|e| vec![format!("не записать {}: {e}", tmp.display())])?;
    std::fs::rename(tmp, target).map_err(|e| vec![format!("не переименовать {} в {}: {e}", tmp.display(), target.display())])
}

/// Слова для отказа `setup`: список занятых имён — старой фразой со всеми
/// именами через запятую; любая другая причина (битый JSON, BOM у pi, отказ
/// ввода-вывода) — как она есть, без подмены чужой причины своей догадкой
/// (Important 1 ревью задачи 6: причина — она одна, и это причина
/// `merge`/`setup`, а не придуманные нами слова про занятую запись).
///
/// Различать два случая по форме, а не заводить отдельный вариант ошибки:
/// список занятых имён — это и только это подмножество `OURS` целиком
/// (`merge_zed`/`merge_pi` никогда не возвращают вперемешку имя и прозу), всё
/// прочее — одна строка прозы.
pub fn refusal_text(reasons: &[String]) -> String {
    let occupied = !reasons.is_empty() && reasons.iter().all(|r| OURS.iter().any(|e| e.name == r));
    if occupied {
        format!("запись {} занята чужой командой — не меняем", reasons.join(", "))
    } else {
        reasons.join("; ")
    }
}

// Claude Code и Codex (решение владельца 29.10, «Четыре локальных агента, и
// настраиваются они по-разному»): у Zed и pi файл настроек только про MCP и
// больше ни про что, поэтому правим его сами. У Claude Code конфигурация
// живёт в `~/.claude.json` вместе со списком проектов и прочим хозяйством, у
// Codex — в `~/.codex/config.toml`, который человек правит руками;
// переписывать такие файлы целиком ради одной записи несоразмерно, и для
// обоих мы зовём их собственное CLI (`claude mcp ...`, `codex mcp ...`),
// проверенное на машине владельца 29.10 и лежащее на `PATH`.

#[cfg(windows)]
fn no_window(c: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn no_window(_c: &mut std::process::Command) {}

/// Запустить `claude`/`codex` и получить его `stdout`/`stderr`/код выхода —
/// одно место для обоих, и запуск по полному пути от того же `find_on_path`,
/// которым меню решает, доступен ли пункт: своё имя `CreateProcess` ищет
/// только с `.exe`, а `codex` на этой машине поставлен через npm как
/// `codex.cmd` — подробности и замер у `find_on_path`.
///
/// `CREATE_NO_WINDOW` — релиз собран с `windows_subsystem = "windows"`
/// (`main.rs:4`), и без флага каждый щелчок галочки мигал бы чёрным окном
/// консоли; тот же приём, что у `holder.rs`, `local.rs`, `project.rs` и
/// `bridge.rs`.
fn run_cli(bin: &str, args: &[String]) -> std::io::Result<std::process::Output> {
    let exe = find_on_path(bin).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{bin} не найден на PATH")))?;
    let mut c = std::process::Command::new(exe);
    c.args(args);
    no_window(&mut c);
    c.output()
}

/// Боевой раннер одного вызова CLI. Отдельной функцией потому, что
/// `claude_set`/`codex_set`/`*_unset` принимают раннер параметром: без этого
/// ни один тест не мог сказать, зовёт ли `claude_set` `remove` перед `add` —
/// находка ревью, эту пару можно было выпотрошить, и весь набор остался бы
/// зелёным.
fn cli(bin: &str, args: &[String]) -> Result<(), String> {
    let out = run_cli(bin, args).map_err(|e| format!("не запустить {bin}: {e}"))?;
    cli_result(out.status.success(), &out.stderr)
}

/// Код выхода CLI в наш общий язык отказа: `0` — успех, иначе его `stderr`
/// как есть, без своей формулировки (задача прямо требует не придумывать
/// слова за CLI — у файловых агентов причину тоже называет не наш текст, а
/// `merge`/`remove`). Пустой `stderr` — единственное исключение: пустая
/// причина дошла бы до журнала пустой строкой после двоеточия, и человек не
/// узнал бы даже того, что отказ был.
///
/// Берёт `ok` и `stderr`, а не готовый `Output`: `Output` в тесте не собрать
/// (`ExitStatus` своим кодом не создать), и ровно из-за этого ни один тест
/// прежде эту функцию не трогал — её можно было заменить на `Ok(())` и все
/// тесты остались бы зелёными (находка ревью). Читается именно `stderr`:
/// замерено 30.10, что `claude mcp add` на занятое имя печатает отказ в
/// `stderr`, а не в `stdout`.
fn cli_result(ok: bool, stderr: &[u8]) -> Result<(), String> {
    if ok {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(stderr).trim().to_string();
    if msg.is_empty() {
        Err("CLI отказал, не сказав причины".to_string())
    } else {
        Err(msg)
    }
}

/// Есть ли Claude Code на этой машине — признак тот же, что у Zed/pi:
/// исполняемый файл на `PATH`. Своего каталога настроек у него для этого не
/// попросить: `~/.claude.json` несёт в себе список проектов человека и
/// прочее хозяйство, и использовать его присутствие как признак — значило
/// бы путать «CLI поставлен» с «человек хоть раз его открыл».
pub fn claude_present() -> bool {
    on_path("claude")
}

/// Есть ли Codex на этой машине — тем же способом.
pub fn codex_present() -> bool {
    on_path("codex")
}

/// Аргументы `claude mcp add` для одной нашей записи: область —
/// пользовательская (`--scope user`), чтобы запись жила у человека, а не в
/// одном проекте (решение владельца) — CLI сам решает, в какой файл её
/// положить. Чистая функция ради теста: какие аргументы мы строим, не
/// зависит от того, что CLI сделает с ними.
fn claude_add_args(name: &str, exe: &str) -> Vec<String> {
    vec!["mcp".into(), "add".into(), name.into(), "--scope".into(), "user".into(), "--".into(), exe.into(), "mcp".into(), name.into()]
}

/// Аргументы `claude mcp remove` для одной нашей записи, той же области.
fn claude_remove_args(name: &str) -> Vec<String> {
    vec!["mcp".into(), "remove".into(), name.into(), "-s".into(), "user".into()]
}

/// Домашний каталог человека — одно место для обоих файлов настроек ниже.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Текст файла настроек CLI-агента, с главным различием, проведённым типом:
/// файла нет (`NotFound`) — `Ok(None)`, то есть «записей нет, настраивать
/// можно»; файл есть, а прочитать его не вышло — `Err`, и тогда ни
/// `setup_cli`, ни `teardown_cli` CLI не зовут вовсе. Образец —
/// `gui::save::save_tray_key` (критическая C2 ревью задачи 7: там «нет файла»
/// спутали с «файл не прочитался» и затёрли живые настройки человека). Цена
/// та же и здесь: приняв нечитаемый файл за пустой, мы не увидели бы
/// `Foreign` и молча перезаписали бы чужую запись — `codex mcp add` на
/// занятое имя именно переписывает.
fn read_agent_config(path: Option<&Path>) -> Result<Option<String>, String> {
    let Some(path) = path else {
        return Err("не узнать путь к его настройкам на этой машине".to_string());
    };
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("не прочитать {}: {e}", path.display())),
    }
}

/// Состояние CLI-агента — чтением его файла настроек, а не опросом его CLI.
/// Это отмена строки спеки «состояние спрашивается у CLI, не угадывается по
/// файлу», и причина — замер 30.10: `claude mcp get 1c` — 6309 и 8710 мс,
/// `codex mcp list --json` — 4423 и 4636 мс, а `tray::set_agents` зовёт
/// `look` каждого агента синхронно в главном потоке GPUI при запуске и на
/// каждой смене режима: 17–22 с замершего окна, и до минуты на один щелчок
/// галочки. Запись остаётся через CLI: запрет владельца был на запись в чужой
/// файл, чтением он не нарушается — и потому мы по-прежнему не пишем в эти
/// файлы сами.
///
/// Файл читается ОДИН раз на весь `look`, а не по разу на имя; агента нет на
/// машине — `Missing` без похода к диску вовсе.
fn look_cli_agent(
    exe: &Path,
    present: bool,
    path: Option<&Path>,
    parse: fn(&str, &str) -> Result<Option<String>, String>,
) -> Result<Found, String> {
    if !present {
        return Ok(Found::Missing);
    }
    let text = read_agent_config(path)?;
    found_via(exe, present, |name| match &text {
        Some(t) => parse(t, name),
        None => Ok(None),
    })
}

/// Файл, в который `claude mcp add --scope user` действительно пишет:
/// `$CLAUDE_CONFIG_DIR/.claude.json`, иначе `~/.claude.json`. Переменную
/// среды учитываем честно — там, где она задана, домашний файл не тот, в
/// который писал CLI; замер 30.10 сделан на отводном `CLAUDE_CONFIG_DIR`, и
/// CLI сам печатает «File modified: …» именно с этим путём.
fn claude_config_path() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => Some(PathBuf::from(dir).join(".claude.json")),
        None => home_dir().map(|h| h.join(".claude.json")),
    }
}

/// `command` нашей записи `<name>` в `.claude.json`: записи лежат в
/// `mcpServers` корня файла, каждая — `{"type":"stdio","command":…,
/// "args":[…],"env":{}}` (образец замерен 30.10). Из записи нужно одно поле,
/// остальное для галочки без надобности. Не разобралось — отказ, а не
/// «записи нет»: файл есть, и судить о нём по неудачному разбору мы не вправе.
fn parse_claude_config(text: &str, name: &str) -> Result<Option<String>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!(".claude.json не разбирается как JSON: {e}"))?;
    Ok(v.get("mcpServers").and_then(|s| s.get(name)).and_then(|e| e.get("command")).and_then(|c| c.as_str()).map(str::to_string))
}

/// Состояние наших записей у Claude Code — чтением `.claude.json`; правило
/// «наша/чужая» и развёртка те же, что у остальных агентов (`found_via`).
pub fn look_claude(exe: &Path, present: bool) -> Result<Found, String> {
    look_cli_agent(exe, present, claude_config_path().as_deref(), parse_claude_config)
}

/// Ставит одну нашу запись у Claude Code: сперва `remove` (его отказ
/// отбрасываем — записи может и не быть), затем `add`. `remove` перед `add`
/// — не хирургия на всякий случай: замерено, что `claude mcp add` при
/// `--scope user` отказывает на уже существующем имени кодом 1 («MCP server
/// 1c already exists in user config»), даже если это наша же запись со старым
/// путём к исполняемому, — значит обновить её можно только «снять и
/// добавить».
///
/// `run` — раннер параметром ради теста: на подставном видно, что `remove`
/// действительно зовётся перед `add`, и выпотрошить эту пару молча больше
/// нельзя.
fn claude_set_with(name: &str, exe: &Path, run: impl Fn(&[String]) -> Result<(), String>) -> Result<(), String> {
    let _ = run(&claude_remove_args(name));
    run(&claude_add_args(name, &exe.display().to_string()))
}

pub fn claude_set(name: &str, exe: &Path) -> Result<(), String> {
    claude_set_with(name, exe, |args| cli("claude", args))
}

/// Снимает одну нашу запись у Claude Code.
pub fn claude_unset(name: &str) -> Result<(), String> {
    cli("claude", &claude_remove_args(name))
}

/// Аргументы `codex mcp add`/`remove` для одной нашей записи: у Codex нет
/// ключа области — `config.toml` человека один на всех проектов, и это уже
/// то, что нам нужно (решение владельца).
fn codex_add_args(name: &str, exe: &str) -> Vec<String> {
    vec!["mcp".into(), "add".into(), name.into(), "--".into(), exe.into(), "mcp".into(), name.into()]
}

fn codex_remove_args(name: &str) -> Vec<String> {
    vec!["mcp".into(), "remove".into(), name.into()]
}

/// Файл, в который пишет `codex mcp add`: `$CODEX_HOME/config.toml`, иначе
/// `~/.codex/config.toml` — та же честность к переменной среды, что у
/// `claude_config_path`.
fn codex_config_path() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) => Some(PathBuf::from(dir).join("config.toml")),
        None => home_dir().map(|h| h.join(".codex").join("config.toml")),
    }
}

/// `command` нашей записи `<name>` в `config.toml` Codex: таблица
/// `[mcp_servers.<name>]` с полями `command` и `args` (образец замерен
/// 30.10). `toml` уже в зависимостях — нового крейта разбор не требует.
fn parse_codex_config(text: &str, name: &str) -> Result<Option<String>, String> {
    let v: toml::Value = toml::from_str(text).map_err(|e| format!("config.toml не разбирается как TOML: {e}"))?;
    Ok(v.get("mcp_servers").and_then(|s| s.get(name)).and_then(|e| e.get("command")).and_then(|c| c.as_str()).map(str::to_string))
}

/// Состояние наших записей у Codex — тем же правилом и тем же чтением файла,
/// что у Claude Code; разница только в формате файла.
pub fn look_codex(exe: &Path, present: bool) -> Result<Found, String> {
    look_cli_agent(exe, present, codex_config_path().as_deref(), parse_codex_config)
}

/// Ставит одну нашу запись у Codex: просто `add` — замерено отдельно, что
/// повторный `add` на занятое имя не отказывает, а переписывает запись (в
/// отличие от Claude Code), значит «снять и добавить» здесь не нужно. Ровно
/// поэтому `setup_cli` обязан отказывать на нечитаемом состоянии: не увидев
/// `Foreign`, мы перезаписали бы чужую запись молча.
fn codex_set_with(name: &str, exe: &Path, run: impl Fn(&[String]) -> Result<(), String>) -> Result<(), String> {
    run(&codex_add_args(name, &exe.display().to_string()))
}

pub fn codex_set(name: &str, exe: &Path) -> Result<(), String> {
    codex_set_with(name, exe, |args| cli("codex", args))
}

/// Снимает одну нашу запись у Codex.
pub fn codex_unset(name: &str) -> Result<(), String> {
    cli("codex", &codex_remove_args(name))
}

/// Общая развёртка `Found` для CLI-агентов по двум нашим именам: `get`
/// отдаёт `command`, стоящий сейчас в записи под этим именем (`Ok(None)` —
/// записи нет), правило «наша/чужая» то же, что у файловых агентов —
/// совпало имя файла `command` с нашим исполняемым, остальное чужое.
///
/// `Result` насквозь, а не `Option`: «записи нет» и «состояние прочитать не
/// удалось» — разные вещи, и спутать их значит перезаписать чужую запись
/// (критическая находка ревью: прежний геттер отдавал `None` на любой сбой, а
/// `setup_cli` читал это как `Absent` и звал `add`).
fn found_via(exe: &Path, present: bool, get: impl Fn(&str) -> Result<Option<String>, String>) -> Result<Found, String> {
    if !present {
        return Ok(Found::Missing);
    }
    let want = exe.file_name().map(|n| n.to_string_lossy().to_string());
    let mut foreign = Vec::new();
    let mut ours_count = 0usize;
    for e in OURS.iter() {
        let Some(cmd) = get(e.name)? else { continue };
        let got = Path::new(&cmd).file_name().map(|n| n.to_string_lossy().to_string());
        if want.is_some() && got == want {
            ours_count += 1;
        } else {
            foreign.push(e.name.to_string());
        }
    }
    if !foreign.is_empty() {
        return Ok(Found::Foreign(foreign));
    }
    if ours_count == OURS.len() {
        Ok(Found::Ours)
    } else {
        Ok(Found::Absent)
    }
}

/// Жмём «Настроить» для CLI-агента: сперва смотрим, не занято ли одно из
/// наших имён чужой командой (`look`) — та же защита, что у файловых
/// агентов (решение владельца, «чужую запись не перезаписываем никогда»), и
/// только если не занято — зовём `set` на обоих именах. `Ok` несёт то, что
/// увидел повторный `look` ПОСЛЕ вызова CLI, не наше предположение об успехе
/// — тот же приём, что у `setup` для файловых агентов.
/// Состояние прочитать не удалось — ни одного вызова CLI: защита чужого
/// обязана быть fail-closed. Не зная, что лежит в файле, мы не видим
/// `Foreign`, а `add` у Codex перезаписывает занятое имя молча — галочка не
/// встаёт, и человек читает в журнале, почему (критическая находка ревью).
pub fn setup_cli(
    exe: &Path,
    present: bool,
    look: impl Fn(&Path, bool) -> Result<Found, String>,
    set: impl Fn(&str, &Path) -> Result<(), String>,
) -> Result<Found, Vec<String>> {
    if let Found::Foreign(names) = look(exe, present).map_err(|msg| vec![msg])? {
        return Err(names);
    }
    for e in OURS.iter() {
        set(e.name, exe).map_err(|msg| vec![msg])?;
    }
    look(exe, present).map_err(|msg| vec![msg])
}

/// Снимаем галочку для CLI-агента — обратное к `setup_cli`: занято чужим или
/// состояние не прочиталось — отказ без вызова CLI, агента нет на машине —
/// успех без вызова, иначе `unset` на ОБОИХ именах и свежий `look` ради
/// итога.
///
/// `Absent` здесь — не повод уйти успехом: `Absent` значит «обеих наших
/// записей нет», а половина (одна наша, второй нет) даёт тот же `Absent` —
/// прежний код на нём выходил успехом, оставив нашу запись в чужом конфиге
/// (находка ревью). Поэтому зовём `unset` на оба имени, не разбирая,
/// половина там или пусто: два лишних вызова на пустом месте дешевле
/// забытой записи.
///
/// Отказ отдельного `unset` провалом не считаем: замерено, что `claude mcp
/// remove` на несуществующем имени выходит кодом 1 (у Codex — 0), и принимать
/// это за провал значило бы отказывать ровно там, где работа и так сделана.
/// Итог решает чтение ПОСЛЕ записи — тот же приём, что в `cfg::write_checked`.
pub fn teardown_cli(
    exe: &Path,
    present: bool,
    look: impl Fn(&Path, bool) -> Result<Found, String>,
    unset: impl Fn(&str) -> Result<(), String>,
) -> Result<Found, Vec<String>> {
    match look(exe, present).map_err(|msg| vec![msg])? {
        Found::Foreign(names) => return Err(names),
        Found::Missing => return Ok(Found::Missing),
        Found::Absent | Found::Ours => {}
    }
    for e in OURS.iter() {
        let _ = unset(e.name);
    }
    match look(exe, present).map_err(|msg| vec![msg])? {
        after @ (Found::Missing | Found::Absent) => Ok(after),
        Found::Foreign(names) => Err(names),
        Found::Ours => Err(vec!["наши записи остались на месте — снять их не удалось".to_string()]),
    }
}

/// Честное различение двух способов настройки одного локального агента —
/// не больше сущностей, чем способов сегодня: правкой файла настроек,
/// который только про MCP и больше ни про что (Zed, pi), — у него есть путь
/// и `merge`/`remove` правят его текст; или вызовом собственного CLI агента
/// (Claude Code, Codex) — у него нет файла, который мы бы трогали, только
/// `status`/`set`/`unset`.
pub enum Setup {
    File {
        path: fn() -> Option<PathBuf>,
        present: fn() -> bool,
        merge: fn(&str, &Path) -> Result<String, Vec<String>>,
        remove: fn(&str, &Path) -> Result<String, Vec<String>>,
    },
    Cli {
        present: fn() -> bool,
        status: fn(&Path, bool) -> Result<Found, String>,
        set: fn(&str, &Path) -> Result<(), String>,
        unset: fn(&str) -> Result<(), String>,
    },
}

impl Setup {
    /// Есть ли агент на машине вообще.
    pub fn present(&self) -> bool {
        match self {
            Setup::File { present, .. } => present(),
            Setup::Cli { present, .. } => present(),
        }
    }

    /// Текущее состояние наших записей — у всех четырёх агентов чтением
    /// файла, в который пишут настройки; угадывания нет нигде.
    ///
    /// `Err` — состояние прочитать не удалось: файл есть, а разобрать или
    /// открыть его не вышло. Это НЕ `Absent` и не `Ours`, и именно поэтому
    /// отдельный `Err`, а не пятый вариант `Found`: файловые агенты такого
    /// случая не знают (их `look` читает файл, который только про MCP, и
    /// непонятный текст там равен «записей нет»), а у CLI-агентов файл чужой,
    /// и принять его молчание за пустоту значит перезаписать чужую запись.
    pub fn look(&self, exe: &Path) -> Result<Found, String> {
        match self {
            Setup::File { path, present, .. } => Ok(look(path().as_deref(), exe, present())),
            Setup::Cli { present, status, .. } => status(exe, present()),
        }
    }

    /// Ставит галочку: пишет наши две записи (идемпотентно — повторный вызов
    /// на уже своей записи не плодит конфликт).
    pub fn enable(&self, exe: &Path) -> Result<Found, Vec<String>> {
        match self {
            Setup::File { path, merge, .. } => match path() {
                Some(p) => setup(&p, exe, merge),
                None => Err(vec!["не узнать путь к его настройкам на этой машине".to_string()]),
            },
            Setup::Cli { present, status, set, .. } => setup_cli(exe, present(), status, set),
        }
    }

    /// Снимает галочку: убирает наши записи, не трогая чужие.
    pub fn disable(&self, exe: &Path) -> Result<Found, Vec<String>> {
        match self {
            Setup::File { path, remove, .. } => match path() {
                Some(p) => teardown(&p, exe, remove),
                None => Err(vec!["не узнать путь к его настройкам на этой машине".to_string()]),
            },
            Setup::Cli { present, status, unset, .. } => teardown_cli(exe, present(), status, unset),
        }
    }
}

/// Один локальный агент в разделе трея «Локальные агенты»: `id` — он же
/// хвост `1c-connect mcp <id>` для записи в настройках и суффикс id пункта
/// меню (`agent:<id>`), `label` — подпись человеку. Порядок в массиве — он
/// же порядок в меню: оба файловых агента, как было, затем оба CLI-агента
/// (решение владельца 29.10, «Четыре локальных агента»).
pub struct AgentDef {
    pub id: &'static str,
    pub label: &'static str,
    pub setup: Setup,
}

pub const AGENTS: [AgentDef; 4] = [
    AgentDef { id: "zed", label: "Zed", setup: Setup::File { path: zed_path, present: zed_present, merge: merge_zed, remove: remove_zed } },
    AgentDef { id: "pi", label: "pi", setup: Setup::File { path: pi_path, present: pi_present, merge: merge_pi, remove: remove_pi } },
    AgentDef {
        id: "claude",
        label: "Claude Code",
        setup: Setup::Cli { present: claude_present, status: look_claude, set: claude_set, unset: claude_unset },
    },
    AgentDef {
        id: "codex",
        label: "Codex",
        setup: Setup::Cli { present: codex_present, status: look_codex, set: codex_set, unset: codex_unset },
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn pi_gets_the_same_two_servers_under_its_own_key() {
        let out = merge_pi("", Path::new("/opt/1c-connect")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["1c"]["args"], serde_json::json!(["mcp", "1c"]));
        assert_eq!(v["mcpServers"]["vrunner"]["timeout"], 3600);
    }

    #[test]
    fn pi_keeps_foreign_servers_and_foreign_keys() {
        let before = r#"{"_comment":"мой файл","mcpServers":{"bsl-reference":{"command":"/opt/bsl-analyzer"}}}"#;
        let out = merge_pi(before, Path::new("/opt/1c-connect")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["_comment"], "мой файл");
        assert!(v["mcpServers"]["bsl-reference"].is_object());
        assert!(v["mcpServers"]["1c"].is_object());
    }

    #[test]
    fn a_foreign_pi_entry_with_our_name_is_reported() {
        let before = r#"{"mcpServers":{"vrunner":{"command":"nc","args":["-N","localhost","7781"]}}}"#;
        assert_eq!(merge_pi(before, Path::new("/opt/1c-connect")).unwrap_err(), vec!["vrunner".to_string()]);
    }

    /// Симметрия с `merge_zed`: битый не-JSON текст — отказ, а не наш файл
    /// вместо содержимого человека (ревью задачи 5, Important 1).
    #[test]
    fn garbage_that_is_not_json_is_a_refusal_for_pi_too() {
        assert!(merge_pi("это вообще не json", Path::new("/opt/1c-connect")).is_err());
    }

    /// Та же симметрия: объект начат, но не закрыт — отказ, а не
    /// наполовину написанный файл.
    #[test]
    fn an_unclosed_object_is_a_refusal_for_pi_too() {
        let before = "{\n  \"mcpServers\": {\n  oops\n";
        assert!(merge_pi(before, Path::new("/opt/1c-connect")).is_err());
    }

    /// `mcpServers` — не объект (решение, зеркальное проверке
    /// `context_servers` у Zed): вписать рядом свой ключ значило бы молча
    /// проиграть чужому значению (в JSON побеждает последний одноимённый
    /// ключ), поэтому отказ для всех форм "не объект".
    #[test]
    fn mcp_servers_that_is_not_an_object_is_a_refusal() {
        for bad in [r#"{"mcpServers": null}"#, r#"{"mcpServers": "выключено руками"}"#, r#"{"mcpServers": []}"#] {
            assert!(merge_pi(bad, Path::new("/opt/1c-connect")).is_err(), "{bad}");
        }
    }

    /// Оба наших имени заняты чужими записями разом — список `occupied`
    /// несёт оба имени, а не только первое найденное.
    #[test]
    fn both_our_names_occupied_at_once_are_reported_together() {
        let before = r#"{"mcpServers":{"1c":{"command":"nc"},"vrunner":{"command":"nc"}}}"#;
        let mut err = merge_pi(before, Path::new("/opt/1c-connect")).unwrap_err();
        err.sort();
        assert_eq!(err, vec!["1c".to_string(), "vrunner".to_string()]);
    }

    /// Симметрия с `our_own_entry_is_updated_in_place` у Zed: наша же
    /// запись (имя файла в `command` совпадает) переписывается на новый
    /// путь исполняемого, а не копится рядом вторым конфликтом.
    #[test]
    fn pi_updates_our_own_entry_in_place_on_a_new_exe_path() {
        let before = r#"{"mcpServers":{"1c":{"command":"/old/1c-connect","args":["mcp","1c"],"timeout":300}}}"#;
        let out = merge_pi(before, Path::new("/new/1c-connect")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["1c"]["command"], "/new/1c-connect");
    }

    /// Симметрия с `a_windows_path_is_escaped_and_parses_back` у Zed:
    /// путь с пробелом и кириллицей переживает экранирование `serde_json`
    /// и возвращается тем же текстом после разбора (ревью задачи 5,
    /// Important 2).
    #[test]
    fn a_windows_path_with_spaces_and_cyrillic_round_trips_for_pi() {
        let out = merge_pi("", Path::new(r"C:\Users\Иван Петров\1c-connect.exe")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["1c"]["command"], r"C:\Users\Иван Петров\1c-connect.exe");
    }

    /// Minor ревью задачи 5: BOM называется по имени, а не тонет в общем
    /// «не разбирается как JSON» — человек должен понять, что чинить.
    #[test]
    fn a_bom_is_named_by_name_not_folded_into_a_generic_parse_error() {
        let before = "\u{feff}{}";
        let err = merge_pi(before, Path::new("/opt/1c-connect")).unwrap_err();
        assert!(err.iter().any(|m| m.contains("BOM")), "{err:?}");
    }

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

    use crate::project::tempdir::Dir;

    #[test]
    fn an_absent_editor_is_missing_regardless_of_the_path() {
        // Критический дефект живого прогона 29.10: `present=false` — Zed/pi
        // на машине нет вовсе, и это `Missing` независимо от того, можем ли
        // мы вообще назвать путь к несуществующим настройкам.
        assert_eq!(look(None, Path::new("/opt/1c-connect"), false), Found::Missing);
        let d = Dir::new();
        let p = d.path().join("settings.json");
        assert_eq!(look(Some(&p), Path::new("/opt/1c-connect"), false), Found::Missing);
    }

    /// Ровно сценарий владельца 29.10: редактор на машине ЕСТЬ (`present =
    /// true`), а файла настроек с нашими записями ни разу не было — это
    /// `Found::Absent` («найден, серверы не настроены»), а НЕ `Missing`.
    /// Старая версия `look` путала эти два факта и запирала кнопку там, где
    /// нажатие должно было создать файл с нуля.
    #[test]
    fn a_present_editor_without_a_settings_file_is_absent_not_missing() {
        let d = Dir::new();
        let p = d.path().join("settings.json"); // каталог редактора есть, этого файла в нём нет
        assert_eq!(look(Some(&p), Path::new("/opt/1c-connect"), true), Found::Absent);
    }

    #[test]
    fn look_of_a_file_without_our_key_is_absent() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        std::fs::write(&p, "{\"theme\": \"One\"}").unwrap();
        assert_eq!(look(Some(&p), Path::new("/opt/1c-connect"), true), Found::Absent);
    }

    #[test]
    fn look_finds_our_own_zed_entries() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        std::fs::write(&p, merge_zed("", exe).unwrap()).unwrap();
        assert_eq!(look(Some(&p), exe, true), Found::Ours);
    }

    #[test]
    fn look_finds_our_own_pi_entries() {
        let d = Dir::new();
        let p = d.path().join("mcp.json");
        let exe = Path::new("/opt/1c-connect");
        std::fs::write(&p, merge_pi("", exe).unwrap()).unwrap();
        assert_eq!(look(Some(&p), exe, true), Found::Ours);
    }

    #[test]
    fn look_reports_a_foreign_record_by_name() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        std::fs::write(&p, "{\"context_servers\":{\"1c\":{\"command\":\"nc\"}}}").unwrap();
        assert_eq!(look(Some(&p), Path::new("/opt/1c-connect"), true), Found::Foreign(vec!["1c".to_string()]));
    }

    #[test]
    fn setup_writes_a_fresh_file_with_no_backup() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        assert_eq!(setup(&p, exe, merge_zed).unwrap(), Found::Ours);
        assert!(p.exists());
        assert!(!sibling_path(&p, ".bak").exists(), "файла раньше не было — нечего резервировать");
        assert!(look(Some(&p), exe, true) == Found::Ours);
    }

    /// Первая правка снимает `.bak` с исходным текстом; вторая правку того же
    /// файла копию не перетирает (решение владельца №1).
    #[test]
    fn setup_backs_up_only_on_the_first_edit() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        let original = "{\n  \"theme\": \"One\"\n}\n";
        std::fs::write(&p, original).unwrap();
        setup(&p, exe, merge_zed).unwrap();
        let bak = sibling_path(&p, ".bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), original);
        // Вторая правка — другим путём к исполняемому, бэкап не трогаем.
        setup(&p, Path::new("/new/1c-connect"), merge_zed).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), original, "вторая правка не должна перетирать резервную копию");
    }

    /// Занятая чужой записью `1c` — отказ, и файл на диске не меняется вовсе.
    #[test]
    fn setup_refuses_and_leaves_a_foreign_record_untouched() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        let before = "{\"context_servers\":{\"1c\":{\"command\":\"nc\"}}}";
        std::fs::write(&p, before).unwrap();
        assert_eq!(setup(&p, exe, merge_zed).unwrap_err(), vec!["1c".to_string()]);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
        assert!(!sibling_path(&p, ".bak").exists());
    }

    /// Important 2 ревью задачи 6: половина наших записей на месте (`1c`
    /// есть и наша, `vrunner` отсутствует вовсе) — это ещё не «настроен»,
    /// а «серверы не настроены»: хвалить файл за часть работы раздел не
    /// должен, и кнопка «Настроить» обязана остаться открытой, чтобы
    /// дописать недостающую запись.
    #[test]
    fn look_requires_every_one_of_our_entries_not_just_one() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        std::fs::write(&p, format!(r#"{{"context_servers":{{"1c":{{"command":"{}"}}}}}}"#, exe.display())).unwrap();
        assert_eq!(look(Some(&p), exe, true), Found::Absent);
    }

    /// Important 4 ревью задачи 6: ревьюер подменил перечит на диске
    /// константой `Ok(Found::Ours)` — ни один тест тогда не упал. Этот тест
    /// обязан упасть на такой подмене: `merge`, который вернул `Ok`, пишет
    /// текст, где одна из двух записей — ЧУЖАЯ (`vrunner`: команда `nc`);
    /// если бы `setup` отвечал «получилось» по самому факту `Ok`, не читая
    /// диск заново, здесь был бы `Found::Ours`. Настоящий перечит обязан
    /// увидеть чужую команду и вернуть `Foreign`.
    #[test]
    fn setup_returns_what_look_actually_sees_not_an_assumed_success() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        let suspicious_merge = |_: &str, _: &Path| -> Result<String, Vec<String>> {
            Ok(r#"{"context_servers":{"1c":{"command":"/opt/1c-connect"},"vrunner":{"command":"nc"}}}"#.to_string())
        };
        let got = setup(&p, exe, suspicious_merge).unwrap();
        assert_eq!(got, Found::Foreign(vec!["vrunner".to_string()]));
        // И независимый перечит того же файла видит то же самое: одна правда.
        assert_eq!(look(Some(&p), exe, true), got);
    }

    /// Та же мысль с другой стороны: файл, который мы записали, подменили на
    /// диске МИМО нас (другой процесс, человек) — следующий перечит обязан
    /// говорить о том, что лежит на диске сейчас, а не о том, что мы когда-то
    /// туда положили.
    #[test]
    fn look_reflects_the_disk_even_after_an_outside_edit_past_our_own_write() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        assert_eq!(setup(&p, exe, merge_zed).unwrap(), Found::Ours);
        std::fs::write(&p, r#"{"context_servers":{"1c":{"command":"nc"}}}"#).unwrap();
        assert_eq!(look(Some(&p), exe, true), Found::Foreign(vec!["1c".to_string()]));
    }

    /// Important 1 ревью задачи 6: причина прозой (битый JSON) идёт в текст
    /// как есть, а не тонет в «занята чужой командой» — чужая фраза про
    /// отказ, которого не было.
    #[test]
    fn refusal_text_passes_a_single_prose_reason_through_unchanged() {
        let reasons = merge_zed("это не json", Path::new("/opt/1c-connect")).unwrap_err();
        let text = refusal_text(&reasons);
        assert!(text.contains("не разбирается как JSON"), "{text}");
        assert!(!text.contains("занята чужой командой"), "{text}");
    }

    /// Список занятых имён — старая фраза, со всеми именами через запятую.
    #[test]
    fn refusal_text_names_occupied_entries_with_the_old_phrase() {
        let reasons = vec!["1c".to_string(), "vrunner".to_string()];
        assert_eq!(refusal_text(&reasons), "запись 1c, vrunner занята чужой командой — не меняем");
    }

    /// Important 3 ревью задачи 6: символьную ссылку в тесте на этой машине
    /// создать нельзя (обычной учётке Windows прав не дают — тот же потолок,
    /// что у `the_same_tree_seen_through_a_symlink_appears_once` в
    /// `ops_host.rs`). Доказываем механизм канонизации, а не весь сценарий
    /// целиком: два разных написания одного файла (с `.` посередине пути)
    /// дают один канонический путь, и `.bak`/содержимое обязаны оказаться
    /// при НАСТОЯЩЕМ файле, а не при написании-с-точкой.
    #[test]
    fn setup_canonicalizes_an_existing_path_before_writing() {
        let d = Dir::new();
        let real = d.path().join("settings.json");
        std::fs::write(&real, "{}").unwrap();
        let dirty = d.path().join(".").join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        assert_eq!(setup(&dirty, exe, merge_zed).unwrap(), Found::Ours);
        assert_eq!(look(Some(&real), exe, true), Found::Ours);
        assert!(sibling_path(&real, ".bak").exists(), "резервная копия обязана лежать при настоящем файле");
    }

    /// Задача 7: обе записи на месте — по имени, хотя `command` там `nc`, а не наш исполняемый (контейнерный случай, имя наше по построению).
    #[test]
    fn found_by_name_sees_our_entries_by_name_even_with_a_foreign_command() {
        let text = r#"{"context_servers":{"1c":{"command":"nc","enabled":true},"vrunner":{"command":"nc","enabled":true}}}"#;
        assert_eq!(found_by_name(text), Found::Ours);
    }

    /// Хотя бы одной записи нет — «отсутствует», а не «наполовину настроено».
    #[test]
    fn found_by_name_is_absent_when_one_of_our_names_is_missing() {
        let text = r#"{"context_servers":{"1c":{"command":"nc"}}}"#;
        assert_eq!(found_by_name(text), Found::Absent);
    }

    /// Нет ключа, нет файла по смыслу (пустой текст), мусор — всё то же «отсутствует»: различать причины здесь незачем, совет человеку один.
    #[test]
    fn found_by_name_is_absent_on_garbage_or_empty_text() {
        assert_eq!(found_by_name(""), Found::Absent);
        assert_eq!(found_by_name("это не json"), Found::Absent);
        assert_eq!(found_by_name(r#"{"theme":"One Dark"}"#), Found::Absent);
    }

    /// Снятая галочка не трогает чужие ключи верхнего уровня, чужие записи
    /// `context_servers` и комментарии человека — ровно то же обещание, что
    /// у `merge_zed`, в обратную сторону.
    #[test]
    fn remove_zed_keeps_foreign_keys_comments_and_servers() {
        let before = "{\n  // моя тема\n  \"theme\": \"One Dark\",\n  \"context_servers\": {\n    \"bsl-reference\": {\"command\": \"/opt/bsl-analyzer\"},\n    \"1c\": {\"command\": \"/opt/1c-connect\", \"args\": [\"mcp\", \"1c\"]},\n    \"vrunner\": {\"command\": \"/opt/1c-connect\", \"args\": [\"mcp\", \"vrunner\"]}\n  }\n}\n";
        let out = remove_zed(before, Path::new("/opt/1c-connect")).unwrap();
        assert!(out.contains("// моя тема"), "{out}");
        assert!(out.contains("\"theme\": \"One Dark\""), "{out}");
        let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
        assert!(v["context_servers"]["1c"].is_null(), "{out}");
        assert!(v["context_servers"]["vrunner"].is_null(), "{out}");
        assert_eq!(v["context_servers"]["bsl-reference"]["command"], "/opt/bsl-analyzer");
    }

    /// Наши две записи могут оказаться не рядом в файле — чужая запись между
    /// ними не должна остаться без своей запятой и не должна слиться с
    /// соседним удалённым куском (границы двух несмежных удалений независимы).
    #[test]
    fn remove_zed_handles_our_entries_separated_by_a_foreign_one() {
        let before = r#"{"context_servers":{"1c":{"command":"/opt/1c-connect","args":["mcp","1c"]},"bsl-reference":{"command":"/opt/bsl-analyzer"},"vrunner":{"command":"/opt/1c-connect","args":["mcp","vrunner"]}}}"#;
        let out = remove_zed(before, Path::new("/opt/1c-connect")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
        assert!(v["context_servers"]["1c"].is_null(), "{out}");
        assert!(v["context_servers"]["vrunner"].is_null(), "{out}");
        assert_eq!(v["context_servers"]["bsl-reference"]["command"], "/opt/bsl-analyzer");
    }

    /// Файл без наших записей (или без файла вовсе, пустой текст) — успех
    /// без единой правки: состояние уже такое, какое просили.
    #[test]
    fn remove_zed_on_a_file_without_our_entries_is_a_success_noop() {
        let before = "{\"theme\":\"One\"}";
        assert_eq!(remove_zed(before, Path::new("/opt/1c-connect")).unwrap(), before);
        assert_eq!(remove_zed("", Path::new("/opt/1c-connect")).unwrap(), "");
    }

    /// Чужая запись с нашим именем — отказ, как и при записи; файл не трогаем.
    #[test]
    fn remove_zed_refuses_a_foreign_entry_with_our_name() {
        let before = "{\"context_servers\":{\"1c\":{\"command\":\"nc\"}}}";
        assert_eq!(remove_zed(before, Path::new("/opt/1c-connect")).unwrap_err(), vec!["1c".to_string()]);
    }

    #[test]
    fn remove_pi_keeps_foreign_keys_and_servers() {
        let before = r#"{"_comment":"мой файл","mcpServers":{"bsl-reference":{"command":"/opt/bsl-analyzer"},"1c":{"command":"/opt/1c-connect","args":["mcp","1c"]}}}"#;
        let out = remove_pi(before, Path::new("/opt/1c-connect")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["_comment"], "мой файл");
        assert!(v["mcpServers"]["bsl-reference"].is_object());
        assert!(v["mcpServers"]["1c"].is_null());
    }

    #[test]
    fn remove_pi_on_a_file_without_our_entries_is_a_success_noop() {
        let before = r#"{"mcpServers":{"bsl-reference":{"command":"/opt/bsl-analyzer"}}}"#;
        assert_eq!(remove_pi(before, Path::new("/opt/1c-connect")).unwrap(), before);
        assert_eq!(remove_pi("", Path::new("/opt/1c-connect")).unwrap(), "");
    }

    #[test]
    fn remove_pi_refuses_a_foreign_entry_with_our_name() {
        let before = r#"{"mcpServers":{"vrunner":{"command":"nc"}}}"#;
        assert_eq!(remove_pi(before, Path::new("/opt/1c-connect")).unwrap_err(), vec!["vrunner".to_string()]);
    }

    /// `teardown` на отсутствующем файле — успех без записи на диск: нечего
    /// убирать — нечего и писать.
    #[test]
    fn teardown_on_a_missing_file_is_a_success_noop_and_does_not_create_it() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        assert_eq!(teardown(&p, exe, remove_zed).unwrap(), Found::Absent);
        assert!(!p.exists());
    }

    /// Сам круг «снял — поставил»: файл после второго `setup` снова рабочий
    /// (`look` видит обе записи), независимо от того, что `teardown` мог
    /// оставить пустой `context_servers` на месте бывших записей.
    #[test]
    fn switching_off_then_on_again_returns_a_working_file() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        assert_eq!(setup(&p, exe, merge_zed).unwrap(), Found::Ours);
        assert_eq!(teardown(&p, exe, remove_zed).unwrap(), Found::Absent);
        assert_eq!(setup(&p, exe, merge_zed).unwrap(), Found::Ours);
        assert_eq!(look(Some(&p), exe, true), Found::Ours);
    }

    /// Занятая чужой записью `1c` — отказ, и файл на диске не меняется вовсе.
    #[test]
    fn teardown_refuses_and_leaves_a_foreign_record_untouched() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        let before = "{\"context_servers\":{\"1c\":{\"command\":\"nc\"}}}";
        std::fs::write(&p, before).unwrap();
        assert_eq!(teardown(&p, exe, remove_zed).unwrap_err(), vec!["1c".to_string()]);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
    }

    // Claude Code и Codex: тесты не зовут настоящий CLI (то же правило, что
    // у DPAPI). Состояние проверяется на образцах НАСТОЯЩИХ файлов, пойманных
    // живым прогоном `claude mcp add`/`codex mcp add` в отводном домике
    // (`CLAUDE_CONFIG_DIR`/`CODEX_HOME` во временном каталоге — живые
    // настройки владельца не тронуты), а запись — на подставном раннере.

    /// Образец — буквальный корень `.claude.json` после `claude mcp add 1c
    /// --scope user -- "E:\dev 1C\1c-connect.exe" mcp 1c` (замер 30.10): путь
    /// нарочно взят с пробелом, и он обязан доехать до нас целым.
    const CLAUDE_SAMPLE: &str =
        r#"{"mcpServers":{"1c":{"type":"stdio","command":"E:\\dev 1C\\1c-connect.exe","args":["mcp","1c"],"env":{}}}}"#;

    /// Образец — буквальный `config.toml` после `codex mcp add 1c -- "E:\dev
    /// 1C\1c-connect.exe" mcp 1c` тем же замером.
    const CODEX_SAMPLE: &str = r#"[mcp_servers.1c]
command = 'E:\dev 1C\1c-connect.exe'
args = ["mcp", "1c"]
"#;

    #[test]
    fn reads_our_command_out_of_a_real_claude_json() {
        assert_eq!(parse_claude_config(CLAUDE_SAMPLE, "1c").unwrap(), Some(r"E:\dev 1C\1c-connect.exe".to_string()));
        assert_eq!(parse_claude_config(CLAUDE_SAMPLE, "vrunner").unwrap(), None);
        // Файл человека без наших записей вовсе (у владельца там 17 проектов) — «записей нет», не отказ.
        assert_eq!(parse_claude_config(r#"{"projects":{}}"#, "1c").unwrap(), None);
    }

    #[test]
    fn reads_our_command_out_of_a_real_codex_config_toml() {
        assert_eq!(parse_codex_config(CODEX_SAMPLE, "1c").unwrap(), Some(r"E:\dev 1C\1c-connect.exe".to_string()));
        assert_eq!(parse_codex_config(CODEX_SAMPLE, "vrunner").unwrap(), None);
        assert_eq!(parse_codex_config("model = \"gpt-5\"\n", "1c").unwrap(), None);
    }

    /// Критическая находка ревью: «не смог прочитать» не должно значить
    /// «записи нет» — битый файл обязан быть отказом, иначе `setup_cli` не
    /// увидит `Foreign` и молча перезапишет чужую запись.
    #[test]
    fn a_broken_agent_config_is_a_refusal_not_an_absence() {
        assert!(parse_claude_config("{это не json", "1c").is_err());
        assert!(parse_codex_config("[mcp_servers.1c\ncommand =", "1c").is_err());
    }

    /// Файла нет вовсе — «записей нет», настраивать можно (`Absent`); файл
    /// есть и не разбирается — отказ, галочка не встанет. То же различие, что
    /// `gui::save::save_tray_key` держит ради сохранности чужих настроек.
    #[test]
    fn a_missing_agent_config_is_absent_and_an_unreadable_one_is_a_refusal() {
        let d = Dir::new();
        let exe = Path::new("/opt/1c-connect");
        let p = d.path().join(".claude.json");
        assert_eq!(look_cli_agent(exe, true, Some(&p), parse_claude_config).unwrap(), Found::Absent);
        std::fs::write(&p, "{это не json").unwrap();
        assert!(look_cli_agent(exe, true, Some(&p), parse_claude_config).is_err());
        // Агента нет на машине — `Missing`, и файл не читается вовсе.
        assert_eq!(look_cli_agent(exe, false, None, parse_claude_config).unwrap(), Found::Missing);
    }

    /// Правило «все или ничего» то же, что у файловых агентов: образец с
    /// одной записью из двух — всё ещё `Absent`, обе наши — `Ours`, чужая
    /// команда под нашим именем — `Foreign`.
    #[test]
    fn look_of_a_cli_agent_tells_ours_from_half_from_foreign() {
        let d = Dir::new();
        let exe = Path::new(r"E:\dev 1C\1c-connect.exe");
        let p = d.path().join(".claude.json");
        std::fs::write(&p, CLAUDE_SAMPLE).unwrap();
        assert_eq!(look_cli_agent(exe, true, Some(&p), parse_claude_config).unwrap(), Found::Absent, "одна запись из двух — не «настроен»");
        let both = r#"{"mcpServers":{"1c":{"command":"E:\\dev 1C\\1c-connect.exe"},"vrunner":{"command":"E:\\dev 1C\\1c-connect.exe"}}}"#;
        std::fs::write(&p, both).unwrap();
        assert_eq!(look_cli_agent(exe, true, Some(&p), parse_claude_config).unwrap(), Found::Ours);
        std::fs::write(&p, r#"{"mcpServers":{"1c":{"command":"nc"}}}"#).unwrap();
        assert_eq!(look_cli_agent(exe, true, Some(&p), parse_claude_config).unwrap(), Found::Foreign(vec!["1c".to_string()]));
    }

    /// `cli_result` — три случая, которые ревью верно назвало непокрытыми:
    /// функцию можно было заменить на `Ok(())`, и весь набор остался бы
    /// зелёным.
    #[test]
    fn cli_result_passes_success_and_carries_the_stderr_of_a_refusal() {
        assert!(cli_result(true, b"").is_ok());
        assert_eq!(
            cli_result(false, "MCP server 1c already exists in user config\n".as_bytes()).unwrap_err(),
            "MCP server 1c already exists in user config"
        );
        // Пустой stderr не должен дойти до журнала пустой строкой после двоеточия.
        assert!(!cli_result(false, b"").unwrap_err().is_empty());
    }

    /// Claude зовёт `remove` ПЕРЕД `add` (замер: `claude mcp add` при
    /// `--scope user` отказывает на занятом имени), Codex — только `add`
    /// (повторный `add` там переписывает). На подставном раннере: убрать
    /// `remove` из `claude_set` молча больше нельзя.
    #[test]
    fn claude_set_removes_before_adding_and_codex_set_only_adds() {
        let exe = Path::new("/opt/1c-connect");
        let calls = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let run = |args: &[String]| {
            calls.borrow_mut().push(args.to_vec());
            Ok(())
        };
        claude_set_with("1c", exe, run).unwrap();
        assert_eq!(*calls.borrow(), vec![claude_remove_args("1c"), claude_add_args("1c", "/opt/1c-connect")]);

        calls.borrow_mut().clear();
        codex_set_with("1c", exe, run).unwrap();
        assert_eq!(*calls.borrow(), vec![codex_add_args("1c", "/opt/1c-connect")]);
    }

    /// Отказ `remove` перед `add` у Claude провалом не считается — записи
    /// может и не быть (замерено, код 1), — а отказ самого `add` доходит до
    /// вызывающего словами.
    #[test]
    fn claude_set_ignores_a_failing_remove_but_not_a_failing_add() {
        let exe = Path::new("/opt/1c-connect");
        let seen = std::cell::RefCell::new(0usize);
        let run = |_: &[String]| {
            *seen.borrow_mut() += 1;
            if *seen.borrow() == 1 {
                Err("no such server".to_string())
            } else {
                Ok(())
            }
        };
        assert!(claude_set_with("1c", exe, run).is_ok());
        assert_eq!(*seen.borrow(), 2, "add обязан состояться после отказавшего remove");
        assert_eq!(claude_set_with("1c", exe, |_: &[String]| Err("add упал".to_string())).unwrap_err(), "add упал");
    }

    /// Аргументы `claude mcp add`/`remove` — область пользовательская
    /// (`--scope user`/`-s user`), команда — наш исполняемый с `mcp <name>`,
    /// как у Zed/pi, но без ключа предела: его у `claude mcp add` нет.
    #[test]
    fn claude_add_and_remove_arguments_use_the_user_scope() {
        assert_eq!(
            claude_add_args("1c", "/opt/1c-connect"),
            ["mcp", "add", "1c", "--scope", "user", "--", "/opt/1c-connect", "mcp", "1c"].map(String::from)
        );
        assert_eq!(claude_remove_args("vrunner"), ["mcp", "remove", "vrunner", "-s", "user"].map(String::from));
    }

    /// Аргументы `codex mcp add`/`remove` — без ключа области вовсе: у
    /// Codex `config.toml` один на всех проектов, и это и есть нужная нам
    /// область (решение владельца).
    #[test]
    fn codex_add_and_remove_arguments_have_no_scope_flag() {
        assert_eq!(codex_add_args("1c", "/opt/1c-connect"), ["mcp", "add", "1c", "--", "/opt/1c-connect", "mcp", "1c"].map(String::from));
        assert_eq!(codex_remove_args("vrunner"), ["mcp", "remove", "vrunner"].map(String::from));
    }

    /// `found_via` — то же правило, что у файловых агентов (`is_ours`): имя
    /// файла `command` совпало с нашим исполняемым — наша запись, не
    /// совпало — чужая, обоих нет — отсутствует, агента нет вовсе — `Missing`.
    #[test]
    fn found_via_tells_ours_from_foreign_from_absent_from_missing() {
        let exe = Path::new("/opt/1c-connect");
        assert_eq!(found_via(exe, false, |_| Ok(None)).unwrap(), Found::Missing);
        assert_eq!(found_via(exe, true, |_| Ok(None)).unwrap(), Found::Absent);
        assert_eq!(found_via(exe, true, |_| Ok(Some("/opt/1c-connect".to_string()))).unwrap(), Found::Ours);
        assert_eq!(
            found_via(exe, true, |_| Ok(Some("nc".to_string()))).unwrap(),
            Found::Foreign(vec!["1c".to_string(), "vrunner".to_string()])
        );
        // Отказ чтения доходит до вызывающего, а не оседает в `Absent`.
        assert!(found_via(exe, true, |_| Err("не прочитать".to_string())).is_err());
    }

    /// `setup_cli`/`teardown_cli` — та же защита чужой записи, что у
    /// файловых агентов, и тот же итог «то, что увидел повторный `look`»,
    /// без единого настоящего вызова CLI: `look`/`set`/`unset` здесь —
    /// поддельные замыкания над общей ячейкой, не `claude`/`codex`.
    #[test]
    fn setup_cli_refuses_a_foreign_name_without_calling_set() {
        let calls = std::cell::RefCell::new(Vec::<String>::new());
        let look = |_: &Path, _: bool| Ok(Found::Foreign(vec!["1c".to_string()]));
        let set = |name: &str, _: &Path| {
            calls.borrow_mut().push(name.to_string());
            Ok(())
        };
        let err = setup_cli(Path::new("/opt/1c-connect"), true, look, set).unwrap_err();
        assert_eq!(err, vec!["1c".to_string()]);
        assert!(calls.borrow().is_empty(), "занятое имя не должно звать set вовсе");
    }

    /// Критическая находка ревью: нечитаемое состояние — не «записей нет».
    /// `setup_cli` на отказе чтения не зовёт CLI ни разу и говорит причину
    /// словами.
    #[test]
    fn setup_cli_refuses_an_unreadable_state_without_calling_set() {
        let calls = std::cell::RefCell::new(0usize);
        let look = |_: &Path, _: bool| Err("не прочитать .claude.json: отказано в доступе".to_string());
        let set = |_: &str, _: &Path| {
            *calls.borrow_mut() += 1;
            Ok(())
        };
        let err = setup_cli(Path::new("/opt/1c-connect"), true, look, set).unwrap_err();
        assert!(err[0].contains("не прочитать"), "{err:?}");
        assert_eq!(*calls.borrow(), 0, "не зная состояния, CLI не зовём вовсе");
    }

    #[test]
    fn setup_cli_calls_set_for_both_names_and_reports_what_look_sees_after() {
        let state = std::cell::RefCell::new(Found::Absent);
        let look = |_: &Path, _: bool| Ok(state.borrow().clone());
        let set = |_: &str, _: &Path| {
            *state.borrow_mut() = Found::Ours;
            Ok(())
        };
        assert_eq!(setup_cli(Path::new("/opt/1c-connect"), true, look, set).unwrap(), Found::Ours);
    }

    /// Находка ревью: половина состояния (одна наша запись есть, второй нет)
    /// даёт `Absent`, и прежний `teardown_cli` на нём уходил успехом, оставив
    /// нашу запись в чужом конфиге. Теперь `unset` зовётся на оба имени.
    #[test]
    fn teardown_cli_unsets_both_names_even_when_the_state_looks_absent() {
        let calls = std::cell::RefCell::new(Vec::<String>::new());
        let look = |_: &Path, _: bool| Ok(Found::Absent);
        let unset = |name: &str| {
            calls.borrow_mut().push(name.to_string());
            Ok(())
        };
        assert_eq!(teardown_cli(Path::new("/opt/1c-connect"), true, look, unset).unwrap(), Found::Absent);
        assert_eq!(*calls.borrow(), OURS.iter().map(|e| e.name.to_string()).collect::<Vec<_>>());
    }

    /// Агента нет на машине — `unset` не зовём вовсе: снимать нечего и нечем.
    #[test]
    fn teardown_cli_calls_nothing_when_the_agent_is_missing() {
        let calls = std::cell::RefCell::new(0usize);
        let look = |_: &Path, _: bool| Ok(Found::Missing);
        let unset = |_: &str| {
            *calls.borrow_mut() += 1;
            Ok(())
        };
        assert_eq!(teardown_cli(Path::new("/opt/1c-connect"), true, look, unset).unwrap(), Found::Missing);
        assert_eq!(*calls.borrow(), 0);
    }

    /// Отказ отдельного `unset` провалом не считается (`claude mcp remove` на
    /// несуществующем имени выходит кодом 1 — замерено), но запись, которая
    /// после этого осталась на месте, — считается: галочка не должна погаснуть
    /// на невыполненной работе.
    #[test]
    fn teardown_cli_tolerates_a_failing_unset_but_not_a_record_left_behind() {
        let state = std::cell::RefCell::new(Found::Ours);
        let look = |_: &Path, _: bool| Ok(state.borrow().clone());
        let gone = |_: &str| {
            *state.borrow_mut() = Found::Absent;
            Err("no such server".to_string())
        };
        assert_eq!(teardown_cli(Path::new("/opt/1c-connect"), true, look, gone).unwrap(), Found::Absent);

        *state.borrow_mut() = Found::Ours;
        let err = teardown_cli(Path::new("/opt/1c-connect"), true, look, |_: &str| Ok(())).unwrap_err();
        assert!(err[0].contains("остались на месте"), "{err:?}");
    }

    /// Реестр `AGENTS` не рассыпается по длине под тестом `tray::AGENTS`
    /// (четыре имени, порядок — файловые агенты, затем CLI-агенты) — тот же
    /// контракт, по которому меню строит свои id пунктов `agent:<id>`.
    #[test]
    fn the_agent_registry_has_four_agents_in_the_documented_order() {
        assert_eq!(AGENTS.iter().map(|a| a.id).collect::<Vec<_>>(), ["zed", "pi", "claude", "codex"]);
        assert_eq!(AGENTS.iter().map(|a| a.label).collect::<Vec<_>>(), ["Zed", "pi", "Claude Code", "Codex"]);
    }
}
