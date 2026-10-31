//! Запись коннектора в настройки редактора на машине человека: у Zed правка
//! текстовая, по границам наших двух записей `context_servers` — его файл
//! несёт комментарии, и разобрать документ целиком и напечатать заново
//! нельзя. У pi (`merge_pi`) комментариев в `mcp.json` не бывает, терять
//! нечего — там честный разбор `serde_json` целиком и печать заново.

use crate::skills;
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

/// Первое из `names`, встреченное в `fields` дважды и более. Нужна там, где
/// мы собираемся ПИСАТЬ: одноимённые поля в одном объекте JSONC не запрещает,
/// а наш сканер и Zed читают их по-разному (первое против последнего), и
/// правка на угад тронула бы файл, чтобы потом отказать. На снятии такой
/// вопрос не встаёт — там уносятся все одноимённые сразу
/// (`remove_fields_by_name`) и только когда наши все.
fn duplicate_key(fields: &[Field], names: &[&str]) -> Option<String> {
    names.iter().find(|n| fields.iter().filter(|f| f.key == **n).count() > 1).map(|n| n.to_string())
}

/// Один текст отказа на оба случая одноимённых полей — и при записи, и при
/// снятии: причина у них одна, и человеку незачем читать её в двух
/// формулировках.
fn dup_refusal(key: &str) -> String {
    format!("в settings.json два поля «{key}» — какое из них увидит Zed, зависит от порядка, и угадывать мы не станем")
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
    // Одноимённые поля в одном объекте JSONC не запрещает, а наш сканер и
    // Zed читают их по-разному: `find` берёт первое, serde (и значит Zed) —
    // последнее. Правка первого поля при чужом последнем дала бы худшее из
    // двух: файл тронут, а `look` после записи честно скажет `Foreign`, то
    // есть человек получит «не удалось настроить» уже ПОСЛЕ правки своего
    // файла. Поэтому отказ — тем же правилом, что ниже у `context_servers`,
    // который не объект: угадывать, какое из двух полей человек считает
    // настоящим, мы не вправе (находка ревью целого 30.10, сомнение 2
    // четвёртого круга).
    if let Some(dup) = duplicate_key(&top_fields, &["context_servers"]) {
        return Err(vec![dup_refusal(&dup)]);
    }
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
            let names: Vec<&str> = OURS.iter().map(|e| e.name).collect();
            if let Some(dup) = duplicate_key(&cs_fields, &names) {
                return Err(vec![dup_refusal(&dup)]);
            }
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
/// `merge_zed`. Пустой текст (файла и не было) или текст без наших записей —
/// `Ok` с тем же текстом без изменений: состояние уже такое, какое просили,
/// это успех, а не повод что-то переписывать на диске.
///
/// Чужая запись с нашим именем здесь не отказ, а просто не наше дело: правило
/// владельца «чужую запись не трогаем никогда» в эту сторону и так выполнено
/// — снятие её НЕ касается. Прежде отказ был общим с записью, и человек,
/// подменивший одну запись из двух после настройки, не мог снять вторую, нашу,
/// больше никогда: галочка при этом стояла снятой, агент работал с нашей
/// записью, а окно говорило «не настроен ни один установленный агент» (находка
/// ревью целого 30.10). Что осталось нетронутым, человеку говорит строка
/// журнала по итоговому `Found::Foreign` (`gui::setup_line`).
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
    if let Some(dup) = duplicate_key(&top_fields, &["context_servers"]) {
        return Err(vec![dup_refusal(&dup)]);
    }
    let Some(cs_field) = top_fields.iter().find(|f| f.key == "context_servers") else {
        return Ok(text.to_string()); // ключа нет вовсе — нечего убирать
    };
    if body.as_bytes().get(cs_field.value_start) != Some(&b'{') {
        return Err(vec!["context_servers в settings.json — не объект".to_string()]);
    }
    let (cs_fields, _) = parse_fields(body, cs_field.value_start);
    // Тот же страж, что у `merge_zed`, и по той же причине — но здесь он
    // лечит не порчу файла, а ЛОЖЬ о нём: пропуск сомнительного имени (ниже)
    // оставлял наше поле жить у Zed, снимал галочку и говорил «наши записи
    // убраны». Случай: `"1c"` дважды, первое чужое, второе наше, и наш
    // `vrunner` рядом. Снятие уносило `vrunner`, после чего serde (и Zed)
    // видел в `1c` нашу запись — то есть агент работал с нашим мостом при
    // снятой галочке и молчаливом «убрано». До разворота правила это был
    // честный отказ; отказом и остаётся (находка 1 ре-ревью четвёртого круга).
    if let Some(dup) = duplicate_key(&cs_fields, &OURS.iter().map(|e| e.name).collect::<Vec<_>>()) {
        return Err(vec![dup_refusal(&dup)]);
    }
    // Убираем ровно свои записи; занятое чужой командой имя пропускаем, а не
    // отказываем всей правке.
    //
    // Снимаем имя только когда НАШИ все одноимённые поля, а не хоть одно:
    // `settings.json` — JSONC, и двух полей `"1c"` в одном объекте он не
    // запрещает, а `remove_fields_by_name` удаляет их по имени, то есть все
    // сразу. Условие «хоть одно наше» унесло бы с нашим полем и чужое
    // соседнее — та же беда, из-за которой пошёл этот круг, только в другом
    // месте. Сомнительный случай целиком пропускаем: чья запись победит у
    // Zed, из текста не решить (в JSON побеждает последний одноимённый ключ,
    // а наш сканер читает первый), и трогать мы такое не вправе.
    let remove_names: Vec<&str> = OURS
        .iter()
        .filter(|e| {
            let named: Vec<&Field> = cs_fields.iter().filter(|f| f.key == e.name).collect();
            !named.is_empty() && named.iter().all(|f| is_ours(body, f, exe))
        })
        .map(|e| e.name)
        .collect();
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

/// Расширения-кандидаты в порядке перебора: исполняемые для Windows —
/// ПЕРЕД пустым, потому что npm кладёт рядом с батником ещё и файл без
/// расширения — sh-шим, который Windows запустить не может. Замер 30.10:
/// `...\Roaming\npm\codex` — 421 байт, первая строка `#!/bin/sh`, запуск
/// отвечает «%1 не является приложением Win32» (os error 193), а
/// `...\Roaming\npm\codex.cmd` рядом — `codex-cli 0.145.0`. Пустое
/// расширение стояло первым, и находился именно шим: первый круг этой
/// задачи сменил только текст отказа («не найден на PATH» → «не является
/// приложением Win32»), а галочка Codex так и не вставала. На Linux порядок
/// безразличен — там совпадает одно пустое.
const EXE_EXTS: [&str; 4] = [".exe", ".cmd", ".bat", ""];

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
    find_in_dirs(std::env::split_paths(&path_var), name)
}

/// Чистая часть поиска: перебор переданных каталогов и `EXE_EXTS`. Отдельно
/// от `find_on_path` ради теста — `PATH` процесса один на весь набор, и
/// подменять его в тесте значило бы устроить гонку с соседними тестами
/// (находка ре-ревью: порядок расширений был непокрыт целиком, его можно
/// было переставить молча — что и случилось).
///
/// ponytail: расширения из `PATHEXT` не читаются — перебор не покроет `.ps1`/`.com`/`.py`/`.vbs`, только то, что перечислено в `EXE_EXTS` вручную. Признано некритичным в ре-ревью задачи 7: каталог настроек (`zed_present`) — первый, более надёжный признак, а `.cmd` уже закрывает установку через npm. Поднять до чтения `PATHEXT`, если найдётся агент, ставящийся именно одним из непокрытых расширений.
fn find_in_dirs(dirs: impl Iterator<Item = PathBuf>, name: &str) -> Option<PathBuf> {
    dirs.flat_map(|dir| EXE_EXTS.map(move |ext| dir.join(format!("{name}{ext}")))).find(|p| p.is_file())
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

    // ponytail: разбор и печать заново схлопывают одноимённые ключи во всём файле — serde_json оставляет последний и молча теряет остальные, так что файл с двумя `"1c"` (или двумя любыми ключами) вернётся человеку без одного из них. У Zed этот случай — отказ (`duplicate_key`), потому что там мы правим текст по границам и расходимся с serde в том, какое поле настоящее; здесь расхождения нет — и читаем, и пишем одним serde, — а дубликаты в `mcp.json` патологичны: комментариев формат не допускает, и писать его руками человеку незачем. Поднять до отказа, если найдётся живой файл pi с одноимёнными ключами (находка 7 ре-ревью четвёртого круга).
    Ok(serde_json::to_string_pretty(&top).expect("serde_json::Value всегда сериализуется") + "\n")
}

/// Снимаем галочку «pi» в трее — обратное к `merge_pi`, тот же честный
/// разбор и печать заново целиком: `mcp.json` комментариев не допускает,
/// терять нечего. Правило чужой записи то же, что у Zed и по той же причине:
/// занятое чужой командой имя мы просто не снимаем, отказа всей правке из
/// этого не делаем. Файла нет, или `mcpServers` нет, или наших записей там и
/// так нет — `Ok` без изменений текста: состояние уже такое, какое просили.
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
    let ours: Vec<&str> = OURS.iter().filter(|e| servers.get(e.name).is_some_and(|f| pi_entry_is_ours(f, exe))).map(|e| e.name).collect();
    let mut changed = false;
    for name in ours {
        changed |= servers.remove(name).is_some();
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

/// Галочка в трее несёт две половины — записи MCP и скиллы — и показывает
/// одно слово про обе сразу. Агента на машине нет — спрашивать половины не о
/// чём, и это перекрывает всё остальное, даже нечитаемую вторую половину или
/// чужую запись в ней: галочка всё равно недоступна, и это бывает каждый
/// день — общий `~/.agents/skills` может нести наш скилл (его туда положил
/// сосед) ровно тогда, когда САМ агент на машине не стоит вовсе. Нечитаемая
/// половина — отдельный исход, не «нет»: приравнять её к `Absent` значило бы
/// выдать непрочитанное состояние за пустое место, хотя там могло быть что
/// угодно, в том числе наше; из двух нечитаемых половин называем причину
/// MCP — она проверяется первой, порядок зафиксирован и закреплён тестом,
/// иначе человек увидит СВОЮ, а не ту причину, которую назвал бы код.
/// Чужое хоть в одной половине — `Foreign`, и называем ОБЕИХ: если чужие
/// записи заняты в MCP и в скиллах одновременно (разные причины, разные
/// имена), молчание про вторую означало бы, что после исправления первой
/// человек снова получит «чужое» без объяснения, о чём теперь речь. Обе
/// свои — `Ours`; иначе (хоть одна `Absent`, другая `Ours`) — всё ещё
/// `Absent`, та же логика, что у `look` для двух записей MCP одного агента.
pub fn both(mcp: &Result<Found, String>, skills: &Result<Found, String>) -> Result<Found, String> {
    if matches!(mcp, Ok(Found::Missing)) || matches!(skills, Ok(Found::Missing)) {
        return Ok(Found::Missing);
    }
    if let Err(e) = mcp {
        return Err(e.clone());
    }
    if let Err(e) = skills {
        return Err(e.clone());
    }
    let mcp_foreign = if let Ok(Found::Foreign(names)) = mcp { names.clone() } else { Vec::new() };
    let skills_foreign = if let Ok(Found::Foreign(names)) = skills { names.clone() } else { Vec::new() };
    if !mcp_foreign.is_empty() || !skills_foreign.is_empty() {
        let mut names = mcp_foreign;
        names.extend(skills_foreign);
        return Ok(Found::Foreign(names));
    }
    match (mcp, skills) {
        (Ok(Found::Ours), Ok(Found::Ours)) => Ok(Found::Ours),
        _ => Ok(Found::Absent),
    }
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
/// на диске ПОСЛЕ правки, а не наше предположение об успехе; `Found::Foreign`
/// в этом итоге — не провал снятия, а оставленная чужая запись с нашим именем
/// (`remove_zed`/`remove_pi`, то же решение, что у `teardown_cli`).
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
        // Пустой файл — то же «записей нет», что и отсутствующий, хотя разбор
        // пустого JSON отказал бы: чужой записи в пустом файле нет по
        // определению, терять нечего, а отказ запер бы галочку у человека,
        // поставившего агента и ни разу его не открывшего. У TOML того же
        // случая нет (пустой файл — законная пустая таблица), и различать два
        // агента на этом месте было бы несимметрично без причины.
        Ok(text) if text.trim().is_empty() => Ok(None),
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
/// Файл читается и разбирается ОДИН раз на весь `look`, а не по разу на имя:
/// `parse` отдаёт команды сразу обоих наших имён в порядке `OURS` (живой
/// `.claude.json` владельца — 102 КБ, и разбирать его дважды ради двух полей
/// незачем — находка ре-ревью). Агента нет на машине — `Missing` без похода
/// к диску вовсе.
fn look_cli_agent(
    exe: &Path,
    present: bool,
    path: Option<&Path>,
    parse: fn(&str) -> Result<Vec<Option<String>>, String>,
) -> Result<Found, String> {
    if !present {
        return Ok(Found::Missing);
    }
    let cmds = match read_agent_config(path)? {
        Some(text) => parse(&text)?,
        None => vec![None; OURS.len()],
    };
    Ok(found_via(exe, &cmds))
}

/// Файл, в который `claude mcp add --scope user` действительно пишет:
/// `$CLAUDE_CONFIG_DIR/.claude.json`, иначе `~/.claude.json`. Переменную
/// среды учитываем честно — там, где она задана, домашний файл не тот, в
/// который писал CLI; замер 30.10 сделан на отводном `CLAUDE_CONFIG_DIR`, и
/// CLI сам печатает «File modified: …» именно с этим путём.
fn claude_config_path() -> Option<PathBuf> {
    claude_config_path_of(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from), home_dir())
}

/// Та же развилка чистой функцией от своих двух входов — ветвь переменной
/// среды иначе непокрыта вовсе, и в имени файла можно было ошибиться молча
/// (находка ре-ревью).
fn claude_config_path_of(dir: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    dir.or(home).map(|d| d.join(".claude.json"))
}

/// Команды наших записей в `.claude.json`, в порядке `OURS`: записи лежат в
/// `mcpServers` корня файла, каждая — `{"type":"stdio","command":…,
/// "args":[…],"env":{}}` (образец замерен 30.10). Из записи нужно одно поле,
/// остальное для галочки без надобности. Не разобралось — отказ, а не
/// «записи нет»: файл есть, и судить о нём по неудачному разбору мы не вправе.
///
/// Запись ЕСТЬ, а `command` в ней нет или он не строка — всё равно `Some`, с
/// пустой командой: это ровно удалённый MCP-сервер (`{"type":"http","url":…}`),
/// то есть чужая запись, а не её отсутствие. Прежний `and_then(get("command"))`
/// отдавал на ней `None`, `found_via` читал это как `Absent`, и `setup_cli`
/// пускал `claude mcp remove 1c -s user` — чужой удалённый сервер исчезал
/// молча, без строки в журнале и без копии (находка ревью целого 30.10).
/// Пустая команда с именем нашего исполняемого не совпадёт никогда, значит
/// выйдет `Foreign`, и CLI не позовётся вовсе — тот же fail-closed, что у
/// файловых агентов (`is_ours` тоже требует строкового `command`).
fn parse_claude_config(text: &str) -> Result<Vec<Option<String>>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!(".claude.json не разбирается как JSON: {e}"))?;
    let servers = v.get("mcpServers");
    Ok(OURS
        .iter()
        .map(|e| servers.and_then(|s| s.get(e.name)).map(|x| x.get("command").and_then(|c| c.as_str()).unwrap_or_default().to_string()))
        .collect())
}

/// Состояние наших записей у Claude Code — чтением `.claude.json`; правило
/// «наша/чужая» и развёртка те же, что у остальных агентов (`found_via`).
pub fn look_claude(exe: &Path, present: bool) -> Result<Found, String> {
    look_claude_at(exe, present, claude_config_path().as_deref())
}

/// То же при заданном пути — ради теста на образце настоящего файла: путь
/// `claude_config_path` берёт из переменной среды, а подменять её в тесте
/// нельзя (она общая на весь набор, гонка с соседними тестами реальна).
/// Нужен этот шов затем, что связка «файл Claude Code — разборщик
/// `.claude.json`» иначе непокрыта вовсе: разборщики двух CLI-агентов можно
/// было поменять местами, и ни один тест не покраснел бы (находка
/// ре-ревью), а живьём встала бы вечная «не разбирается» у обоих.
fn look_claude_at(exe: &Path, present: bool, path: Option<&Path>) -> Result<Found, String> {
    look_cli_agent(exe, present, path, parse_claude_config)
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

/// Снимает одну нашу запись у Claude Code — раннером параметром по той же
/// причине, что `claude_set_with`: тело без него можно было заменить на
/// `Ok(())`, и ни один тест не покраснел бы (находка ре-ревью).
fn claude_unset_with(name: &str, run: impl Fn(&[String]) -> Result<(), String>) -> Result<(), String> {
    run(&claude_remove_args(name))
}

pub fn claude_unset(name: &str) -> Result<(), String> {
    claude_unset_with(name, |args| cli("claude", args))
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
    codex_config_path_of(std::env::var_os("CODEX_HOME").map(PathBuf::from), home_dir())
}

/// Та же развилка чистой функцией, что у Claude Code, и с той же причиной.
/// `$CODEX_HOME` — это и есть каталог `.codex`, поэтому домашняя ветвь
/// дописывает `.codex` сама, а переменная — нет.
fn codex_config_path_of(dir: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    dir.or_else(|| home.map(|h| h.join(".codex"))).map(|d| d.join("config.toml"))
}

/// Команды наших записей в `config.toml` Codex, в порядке `OURS`: таблица
/// `[mcp_servers.<имя>]` с полями `command` и `args` (образец замерен
/// 30.10). `toml` уже в зависимостях — нового крейта разбор не требует.
///
/// Таблица есть, а `command` в ней нет — `Some` с пустой командой, по той же
/// причине и с той же ценой ошибки, что у `parse_claude_config`: у Codex это
/// `[mcp_servers.1c]` с одним `url`, и `codex mcp add` на занятое имя не
/// отказывает, а переписывает запись молча (замерено 30.10) — приняв такую
/// запись за отсутствующую, мы бы её и затёрли.
fn parse_codex_config(text: &str) -> Result<Vec<Option<String>>, String> {
    let v: toml::Value = toml::from_str(text).map_err(|e| format!("config.toml не разбирается как TOML: {e}"))?;
    let servers = v.get("mcp_servers");
    Ok(OURS
        .iter()
        .map(|e| servers.and_then(|s| s.get(e.name)).map(|x| x.get("command").and_then(|c| c.as_str()).unwrap_or_default().to_string()))
        .collect())
}

/// Состояние наших записей у Codex — тем же правилом и тем же чтением файла,
/// что у Claude Code; разница только в формате файла.
pub fn look_codex(exe: &Path, present: bool) -> Result<Found, String> {
    look_codex_at(exe, present, codex_config_path().as_deref())
}

/// То же при заданном пути — тот же шов и та же причина, что у
/// `look_claude_at`.
fn look_codex_at(exe: &Path, present: bool, path: Option<&Path>) -> Result<Found, String> {
    look_cli_agent(exe, present, path, parse_codex_config)
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

/// Снимает одну нашу запись у Codex — тем же приёмом и по той же причине.
fn codex_unset_with(name: &str, run: impl Fn(&[String]) -> Result<(), String>) -> Result<(), String> {
    run(&codex_remove_args(name))
}

pub fn codex_unset(name: &str) -> Result<(), String> {
    codex_unset_with(name, |args| cli("codex", args))
}

/// Общая развёртка `Found` для CLI-агентов по двум нашим именам: `cmds` —
/// команды, стоящие сейчас в записях, по одной на имя в порядке `OURS`
/// (`None` — записи нет, пустая строка — запись есть, а `command` в ней нет:
/// удалённый MCP-сервер, см. `parse_claude_config`). Правило «наша/чужая» то
/// же, что у файловых агентов: совпало имя файла `command` с нашим
/// исполняемым — наша, остальное чужое, и пустая команда попадает именно в
/// «чужое» — `Path::new("").file_name()` не даёт ничего, совпасть не с чем.
///
/// Готовым срезом, а не замыканием-геттером: разбор файла — дело
/// `look_cli_agent`, и он разбирает его один раз на оба имени, а не по разу
/// на имя. Отказ разбора («состояние прочитать не удалось» — не то же, что
/// «записи нет»: спутать их значит перезаписать чужую запись) до сюда не
/// доходит вовсе, он остаётся `Err` у вызывающего.
fn found_via(exe: &Path, cmds: &[Option<String>]) -> Found {
    // Срез короче реестра дал бы `Found::Absent` навсегда и ни одного отказа:
    // `ours_count == OURS.len()` стало бы недостижимо. Оба `parse_*` отдают
    // ровно `OURS.len()` по построению — здесь это закреплено, чтобы правка в
    // них не ушла в молчаливое «записей нет».
    debug_assert_eq!(cmds.len(), OURS.len(), "состояние даётся по одной записи на имя реестра");
    let want = exe.file_name().map(|n| n.to_string_lossy().to_string());
    let mut foreign = Vec::new();
    let mut ours_count = 0usize;
    for (e, cmd) in OURS.iter().zip(cmds) {
        let Some(cmd) = cmd else { continue };
        let got = Path::new(cmd).file_name().map(|n| n.to_string_lossy().to_string());
        if want.is_some() && got == want {
            ours_count += 1;
        } else {
            foreign.push(e.name.to_string());
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
    match look(exe, present).map_err(|msg| vec![msg])? {
        Found::Foreign(names) => return Err(names),
        // Агента нет на машине — звать нечего и настраивать нечего:
        // симметрично `teardown_cli`. Сегодня сюда не придти (пункт меню у
        // отсутствующего агента недоступен — `tray::agent_available`), но
        // защита обязана держаться на себе, а не на одном меню: без этой
        // ветви `Missing` проваливался в цикл записи ниже (находка
        // ре-ревью).
        Found::Missing => return Ok(Found::Missing),
        Found::Absent | Found::Ours => {}
    }
    for e in OURS.iter() {
        set(e.name, exe).map_err(|msg| vec![msg])?;
    }
    look(exe, present).map_err(|msg| vec![msg])
}

/// Снимаем галочку для CLI-агента — обратное к `setup_cli`: состояние не
/// прочиталось — отказ без вызова CLI (fail-closed, как и при записи), агента
/// нет на машине — успех без вызова, иначе `unset` на наших именах и свежий
/// `look` ради итога.
///
/// Занятое чужой командой имя (`Found::Foreign`) здесь не отказ, а пропуск
/// ровно этого имени: правило «чужую запись не трогаем» в сторону снятия и так
/// соблюдено — `unset` по такому имени как раз и стёр бы чужую запись. Отказ
/// был общим с записью, и это запирало нашу запись навсегда: подменили `1c`
/// после настройки — и снять нашу `vrunner` галочкой нельзя больше никогда
/// (находка ревью целого 30.10). Итог `Found::Foreign` при снятии значит
/// «наши записи убраны, чужая осталась»; что именно осталось, человеку
/// говорит строка журнала (`gui::setup_line`), и то же решение одинаково у
/// файловых агентов (`remove_zed`/`remove_pi`) — они делят один `Found`, и
/// расхождение здесь стоило бы человеку понимания.
///
/// `Absent` — не повод уйти успехом: `Absent` значит «обеих наших записей
/// нет», а половина (одна наша, второй нет) даёт тот же `Absent` — прежний
/// код на нём выходил успехом, оставив нашу запись в чужом конфиге (находка
/// ревью). Поэтому зовём `unset` на все не занятые чужим имена, не разбирая,
/// половина там или пусто: лишний вызов на пустом месте дешевле забытой
/// записи.
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
    let foreign = match look(exe, present).map_err(|msg| vec![msg])? {
        Found::Missing => return Ok(Found::Missing),
        Found::Foreign(names) => names,
        Found::Absent | Found::Ours => Vec::new(),
    };
    for e in OURS.iter().filter(|e| !foreign.iter().any(|n| n == e.name)) {
        let _ = unset(e.name);
    }
    match look(exe, present).map_err(|msg| vec![msg])? {
        after @ (Found::Missing | Found::Absent | Found::Foreign(_)) => Ok(after),
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
/// `skills` — чей дом скиллов (`skills::Home`) достаётся этому агенту: имя
/// только задаёт подпапку, путь строит `skills::home_dir` в вызывающем коде
/// (докстрока `skills::Home`). Zed, pi и Codex делят один дом (`~/.agents`,
/// решение владельца — общий каталог для всех трёх), у Claude Code свой
/// (`~/.claude`), потому что так зовётся его собственный каталог скиллов.
pub struct AgentDef {
    pub id: &'static str,
    pub label: &'static str,
    pub setup: Setup,
    pub skills: skills::Home,
}

pub const AGENTS: [AgentDef; 4] = [
    AgentDef {
        id: "zed",
        label: "Zed",
        setup: Setup::File { path: zed_path, present: zed_present, merge: merge_zed, remove: remove_zed },
        skills: skills::Home::Agents,
    },
    AgentDef {
        id: "pi",
        label: "pi",
        setup: Setup::File { path: pi_path, present: pi_present, merge: merge_pi, remove: remove_pi },
        skills: skills::Home::Agents,
    },
    AgentDef {
        id: "claude",
        label: "Claude Code",
        setup: Setup::Cli { present: claude_present, status: look_claude, set: claude_set, unset: claude_unset },
        skills: skills::Home::Claude,
    },
    AgentDef {
        id: "codex",
        label: "Codex",
        setup: Setup::Cli { present: codex_present, status: look_codex, set: codex_set, unset: codex_unset },
        skills: skills::Home::Agents,
    },
];

/// Есть ли у ДРУГИХ агентов, делящих с нами тот же дом скиллов, свои записи
/// MCP — вопрос задаём именно про MCP, не про скиллы: в общем доме
/// (`~/.agents/skills`) файлы у Zed, pi и Codex одни и те же, и спрашивать
/// про скиллы соседа значило бы спрашивать про самого себя, не узнавая
/// ничего нового (докстрока модуля `skills`, решение хозяина). `id` —
/// СПРАШИВАЮЩИЙ, его самого из счёта исключаем явно, а не надеждой на
/// порядок вызовов: круг 1 ревью нашёл, что без этого своя же `Ours`-запись
/// заставила бы функцию держать общий дом вечно (спроси до `Setup::disable`
/// — и он ещё не снят; спроси после `teardown_cli`, закончившегося `Foreign`
/// — и чужая запись на месте нашей тоже сочтена «держит»; а у Claude Code,
/// у которого соседей в его каталоге нет вовсе, функция без исключения себя
/// превращалась в отчёт о самой себе). Нечитаемая половина соседа (`Err`) и
/// чужая запись на его месте (`Foreign`) считаются «настроен»: не унести
/// лишнего важнее, чем прибраться, и ошибка чтения не повод решить, что
/// сосед свободен. Агента физически нет на машине (`Missing`) — повод
/// противоположный: спрашивать нечего, сосед не держит, и без этой ветки
/// дом, где реально стоит только один из троих общих агентов, не
/// освобождался бы никогда.
pub fn others_hold(id: &str, home: skills::Home, exe: &Path) -> bool {
    any_agent_holds(&AGENTS, id, home, exe)
}

/// Та же проверка, но над явным списком агентов — так тест строит маленький
/// свой набор с управляемыми `Setup::Cli` без похода в настоящую файловую
/// систему и переменные среды (которые этот модуль трогать не должен, они
/// общие на весь прогон тестов).
fn any_agent_holds(agents: &[AgentDef], id: &str, home: skills::Home, exe: &Path) -> bool {
    agents.iter().filter(|a| a.skills == home && a.id != id).any(|a| !matches!(a.setup.look(exe), Ok(Found::Absent) | Ok(Found::Missing)))
}

/// Живут ли локальные агенты на ЭТОЙ машине — один предикат на всех, кто про
/// это спрашивает, и их трое: доступность подменю и отметки в нём
/// (`gui::tray::set_agents`), ветвь факта набора `EDITOR_MCP`
/// (`verify::cheap`) и тихая доставка навыков при запуске окна
/// (`gui::delivery_needed`, которую зовёт `gui::refresh_skills`). Третьего
/// добавило ревью целого 31.10: доставка не смотрела режим вовсе и в
/// туннельном режиме молча обновляла каталоги навыков в доме человека — там,
/// где самого раздела «Локальные агенты» нет и отменить это галочкой нельзя.
/// Жил он в `tray.rs`
/// приватным, а факт выбирал ветвь своим `mode == "local"` — и в серверном
/// режиме, под который предикат и назван, подменю открылось бы с живыми
/// галочками, а карточка в окне говорила бы «в контейнере записи MCP не
/// проверяем» (находка ревью целого 30.10). Отсюда и дом в `editors`: это
/// знание про агентов, а не про трей.
///
/// Назван по смыслу, а не по режиму, нарочно: уточнение владельца 30.10 —
/// следующий шаг — серверный режим коннектора, где он стоит на рабочем месте
/// разработчика и ПРИНИМАЕТ подключение из туннеля, а не звонит сам; агенты
/// там настраиваются на этой же машине, то есть «агенты здесь» станет истиной
/// ещё при одном значении режима, помимо `"local"`. Когда серверный режим
/// появится, этот предикат — единственное место, которое расширяется.
pub fn agents_live_here(mode: &str) -> bool {
    mode == "local"
}

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

    /// Сомнение 2 четвёртого круга: одноимённые поля наш сканер читает
    /// первым, а Zed — последним, и правка первого при чужом последнем
    /// тронула бы файл, чтобы потом отказать. Отказ обязан случиться ДО
    /// правки, и текст файла остаться тем же — проверяем оба имени и сам
    /// `context_servers`.
    #[test]
    fn two_fields_with_one_name_are_a_refusal_before_any_edit() {
        let exe = Path::new("/opt/1c-connect");
        let ours = r#"{"command":"/opt/1c-connect","args":["mcp","1c"]}"#;
        let twice_inside = format!(r#"{{"context_servers":{{"1c":{ours},"1c":{{"command":"nc"}}}}}}"#);
        let reasons = merge_zed(&twice_inside, exe).unwrap_err();
        assert!(refusal_text(&reasons).contains("два поля «1c»"), "{}", refusal_text(&reasons));
        let twice_top = r#"{"context_servers":{},"context_servers":{}}"#;
        let reasons = merge_zed(twice_top, exe).unwrap_err();
        assert!(refusal_text(&reasons).contains("два поля «context_servers»"), "{}", refusal_text(&reasons));
        // Одно поле каждого имени — обычный путь, отказа быть не должно.
        assert!(merge_zed(r#"{"context_servers":{"1c":{"command":"nc"}}}"#, exe).is_err(), "чужая запись — другой отказ, но отказ");
        assert!(merge_zed(r#"{"other":1}"#, exe).is_ok());
    }

    /// Находка 1 ре-ревью четвёртого круга: снятие на одноимённых полях
    /// пропускало имя, уносило вторую нашу запись и говорило «убрано», а
    /// наша запись при этом оставалась жить у Zed — serde (и значит Zed)
    /// читает последнее поле. Теперь отказ, и файл не тронут.
    #[test]
    fn removal_refuses_two_fields_with_one_name_instead_of_lying() {
        let exe = Path::new("/opt/1c-connect");
        let ours = r#"{"command":"/opt/1c-connect","args":["mcp","1c"]}"#;
        let ours_vr = r#"{"command":"/opt/1c-connect","args":["mcp","vrunner"]}"#;
        let text = format!(r#"{{"context_servers":{{"1c":{{"command":"nc"}},"1c":{ours},"vrunner":{ours_vr}}}}}"#);
        let reasons = remove_zed(&text, exe).unwrap_err();
        assert!(refusal_text(&reasons).contains("два поля «1c»"), "{}", refusal_text(&reasons));
        // Два `context_servers` — то же самое: правили бы первый, Zed читает последний.
        let two_cs = r#"{"context_servers":{},"context_servers":{}}"#;
        assert!(remove_zed(two_cs, exe).is_err());
        // Обычный путь снятия отказом не стал.
        let one = format!(r#"{{"context_servers":{{"1c":{ours},"vrunner":{ours_vr}}}}}"#);
        let left = remove_zed(&one, exe).unwrap();
        assert!(!left.contains("1c-connect"), "наши записи должны уйти: {left}");
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

    /// Пункт 3 ревью целого: снятие убирает НАШИ записи и не трогает чужие —
    /// половинное состояние (`1c` подменён чужой командой, `vrunner` наш)
    /// прежде отказывало целиком, и нашу запись нельзя было снять галочкой
    /// больше никогда.
    #[test]
    fn remove_zed_takes_ours_and_leaves_a_foreign_entry_with_our_name() {
        let before = r#"{"context_servers":{"1c":{"command":"nc"},"vrunner":{"command":"/opt/1c-connect","args":["mcp","vrunner"]}}}"#;
        let out = remove_zed(before, Path::new("/opt/1c-connect")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&as_json(&out)).unwrap();
        assert_eq!(v["context_servers"]["1c"]["command"], "nc", "чужую запись снятие тронуло: {out}");
        assert!(v["context_servers"]["vrunner"].is_null(), "нашу запись снятие не убрало: {out}");
        // Все наши имена заняты чужим — убирать нечего, и файл не переписываем.
        let only_foreign = "{\"context_servers\":{\"1c\":{\"command\":\"nc\"}}}";
        assert_eq!(remove_zed(only_foreign, Path::new("/opt/1c-connect")).unwrap(), only_foreign);
        // Два поля с одним именем (JSONC такого не запрещает) — отказ, а не
        // молчаливый пропуск: пропуск уносил вторую нашу запись и говорил
        // «убрано», оставляя нашу запись жить у Zed (находка 1 ре-ревью
        // четвёртого круга, подробнее —
        // `removal_refuses_two_fields_with_one_name_instead_of_lying`).
        let twice = r#"{"context_servers":{"1c":{"command":"/opt/1c-connect"},"1c":{"command":"nc"}}}"#;
        assert!(remove_zed(twice, Path::new("/opt/1c-connect")).is_err(), "сомнительное имя обязано быть отказом");
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

    /// То же решение, что у Zed, и тем же тестом: расхождение файловых агентов
    /// между собой на защите чужого человеку стоило бы понимания.
    #[test]
    fn remove_pi_takes_ours_and_leaves_a_foreign_entry_with_our_name() {
        let before = r#"{"mcpServers":{"vrunner":{"command":"nc"},"1c":{"command":"/opt/1c-connect","args":["mcp","1c"]}}}"#;
        let out = remove_pi(before, Path::new("/opt/1c-connect")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["vrunner"]["command"], "nc", "чужую запись снятие тронуло: {out}");
        assert!(v["mcpServers"]["1c"].is_null(), "нашу запись снятие не убрало: {out}");
        let only_foreign = r#"{"mcpServers":{"vrunner":{"command":"nc"}}}"#;
        assert_eq!(remove_pi(only_foreign, Path::new("/opt/1c-connect")).unwrap(), only_foreign);
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

    /// Снятие при занятой чужим `1c`: итог — `Foreign` (успех снятия, а не
    /// отказ, пункт 3 ревью целого), а файл на диске не меняется вовсе —
    /// убирать там было нечего, и писать незачем.
    #[test]
    fn teardown_leaves_a_foreign_record_untouched_on_disk() {
        let d = Dir::new();
        let p = d.path().join("settings.json");
        let exe = Path::new("/opt/1c-connect");
        let before = "{\"context_servers\":{\"1c\":{\"command\":\"nc\"}}}";
        std::fs::write(&p, before).unwrap();
        assert_eq!(teardown(&p, exe, remove_zed).unwrap(), Found::Foreign(vec!["1c".to_string()]));
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
        // Порядок — `OURS`: сперва `1c`, затем `vrunner` (его в образце нет).
        assert_eq!(parse_claude_config(CLAUDE_SAMPLE).unwrap(), vec![Some(r"E:\dev 1C\1c-connect.exe".to_string()), None]);
        // Файл человека без наших записей вовсе (у владельца там 17 проектов) — «записей нет», не отказ.
        assert_eq!(parse_claude_config(r#"{"projects":{}}"#).unwrap(), vec![None, None]);
    }

    #[test]
    fn reads_our_command_out_of_a_real_codex_config_toml() {
        assert_eq!(parse_codex_config(CODEX_SAMPLE).unwrap(), vec![Some(r"E:\dev 1C\1c-connect.exe".to_string()), None]);
        assert_eq!(parse_codex_config("model = \"gpt-5\"\n").unwrap(), vec![None, None]);
    }

    /// Главная находка ревью целого: запись ЕСТЬ, а `command` в ней нет — это
    /// удалённый MCP-сервер человека, и прежде оба парсера отдавали на нём
    /// `None`, то есть `Absent`; дальше `claude mcp remove 1c -s user` уносил
    /// чужую запись, а `codex mcp add` переписывал её молча. Образцы
    /// настоящие: `claude mcp add --transport http 1c https://…` и
    /// `[mcp_servers.1c]` с одним `url`. Итог обязан быть `Foreign` — тогда ни
    /// `setup_cli`, ни `teardown_cli` CLI по этому имени не позовут.
    #[test]
    fn a_remote_mcp_server_without_a_command_is_foreign_not_absent() {
        let exe = Path::new(r"E:\dev 1C\1c-connect.exe");
        let claude_http = r#"{"mcpServers":{"1c":{"type":"http","url":"https://mcp.example.org/sse"}}}"#;
        assert_eq!(parse_claude_config(claude_http).unwrap(), vec![Some(String::new()), None]);
        assert_eq!(found_via(exe, &parse_claude_config(claude_http).unwrap()), Found::Foreign(vec!["1c".to_string()]));
        let codex_url = "[mcp_servers.1c]\nurl = \"https://mcp.example.org/sse\"\n";
        assert_eq!(parse_codex_config(codex_url).unwrap(), vec![Some(String::new()), None]);
        assert_eq!(found_via(exe, &parse_codex_config(codex_url).unwrap()), Found::Foreign(vec!["1c".to_string()]));
        // `command` есть, но не строка (человек правил файл руками) — тот же
        // случай: запись есть, чья она — не понять, значит не наша.
        assert_eq!(
            found_via(exe, &parse_claude_config(r#"{"mcpServers":{"vrunner":{"command":["sh","-c"]}}}"#).unwrap()),
            Found::Foreign(vec!["vrunner".to_string()])
        );
    }

    /// Критическая находка ревью: «не смог прочитать» не должно значить
    /// «записи нет» — битый файл обязан быть отказом, иначе `setup_cli` не
    /// увидит `Foreign` и молча перезапишет чужую запись.
    #[test]
    fn a_broken_agent_config_is_a_refusal_not_an_absence() {
        assert!(parse_claude_config("{это не json").is_err());
        assert!(parse_codex_config("[mcp_servers.1c\ncommand =").is_err());
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
        // Пустой файл — «записей нет», а не отказ: чужого в нём нет, и
        // отказ запер бы галочку у человека, ни разу не открывшего агента.
        std::fs::write(&p, "   \n").unwrap();
        assert_eq!(look_cli_agent(exe, true, Some(&p), parse_claude_config).unwrap(), Found::Absent);
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
    /// совпало — чужая, обоих нет — отсутствует. «Агента нет вовсе» сюда уже
    /// не доходит — это решает `look_cli_agent` до чтения файла.
    #[test]
    fn found_via_tells_ours_from_foreign_from_absent() {
        let exe = Path::new("/opt/1c-connect");
        let ours = || Some("/opt/1c-connect".to_string());
        assert_eq!(found_via(exe, &[None, None]), Found::Absent);
        assert_eq!(found_via(exe, &[ours(), ours()]), Found::Ours);
        assert_eq!(found_via(exe, &[ours(), None]), Found::Absent, "одна запись из двух — не «настроен»");
        assert_eq!(
            found_via(exe, &[Some("nc".to_string()), Some("nc".to_string())]),
            Found::Foreign(vec!["1c".to_string(), "vrunner".to_string()])
        );
    }

    /// Порядок расширений: в каталоге, где лежат и `x`, и `x.cmd`, находиться
    /// обязан `x.cmd`. Замер 30.10: npm кладёт рядом с `codex.cmd` ещё и
    /// `codex` — sh-шим, который Windows отвергает (os error 193), и пустое
    /// расширение первым оставляло Codex неработающим. `PATH` процесса тест
    /// не трогает — он общий на весь набор.
    #[test]
    fn an_executable_extension_wins_over_the_bare_shim_next_to_it() {
        let d = Dir::new();
        std::fs::write(d.path().join("codex"), "#!/bin/sh\n").unwrap();
        std::fs::write(d.path().join("codex.cmd"), "@echo off\n").unwrap();
        assert_eq!(find_in_dirs([d.path().to_path_buf()].into_iter(), "codex").unwrap(), d.path().join("codex.cmd"));

        // Единственный файл без расширения всё же находится — на Linux это
        // единственный случай, который вообще бывает.
        let d2 = Dir::new();
        std::fs::write(d2.path().join("pi"), "#!/bin/sh\n").unwrap();
        assert_eq!(find_in_dirs([d2.path().to_path_buf()].into_iter(), "pi").unwrap(), d2.path().join("pi"));

        // Ничего похожего в каталоге нет — ничего и не нашлось.
        assert!(find_in_dirs([d2.path().to_path_buf()].into_iter(), "zed").is_none());
    }

    /// Путь к настройкам CLI-агента: переменная среды задана — её каталог,
    /// не задана — домашний. `$CODEX_HOME` — это и есть `.codex`, поэтому
    /// `.codex` дописывает только домашняя ветвь. Обе ветви были непокрыты
    /// целиком, и в имени файла можно было ошибиться молча (находка ре-ревью).
    #[test]
    fn the_agent_config_path_prefers_the_environment_over_the_home_directory() {
        let home = PathBuf::from("/home/u");
        assert_eq!(claude_config_path_of(None, Some(home.clone())).unwrap(), PathBuf::from("/home/u/.claude.json"));
        assert_eq!(
            claude_config_path_of(Some(PathBuf::from("/tmp/cfg")), Some(home.clone())).unwrap(),
            PathBuf::from("/tmp/cfg/.claude.json")
        );
        assert_eq!(codex_config_path_of(None, Some(home.clone())).unwrap(), PathBuf::from("/home/u/.codex/config.toml"));
        assert_eq!(codex_config_path_of(Some(PathBuf::from("/tmp/codex")), Some(home)).unwrap(), PathBuf::from("/tmp/codex/config.toml"));
        // Домашнего каталога не узнать и переменной нет — пути нет, и
        // `read_agent_config` отдаст отказ, а не «записей нет».
        assert!(claude_config_path_of(None, None).is_none());
        assert!(codex_config_path_of(None, None).is_none());
    }

    /// Каждый CLI-агент разбирает СВОЙ файл: образец настоящего
    /// `.claude.json` у `look_claude`, настоящего `config.toml` у
    /// `look_codex`. Поменять разборщики местами молча больше нельзя —
    /// прежде оба теста звали только `look_cli_agent` с явным разборщиком.
    #[test]
    fn each_cli_agent_reads_its_own_file_format() {
        let d = Dir::new();
        let exe = Path::new(r"E:\dev 1C\1c-connect.exe");
        let claude = d.path().join(".claude.json");
        std::fs::write(&claude, CLAUDE_SAMPLE).unwrap();
        assert_eq!(look_claude_at(exe, true, Some(&claude)).unwrap(), Found::Absent, "в образце одна запись из двух");
        let codex = d.path().join("config.toml");
        std::fs::write(&codex, CODEX_SAMPLE).unwrap();
        assert_eq!(look_codex_at(exe, true, Some(&codex)).unwrap(), Found::Absent);
        // Файл чужого формата — отказ, а не «записей нет»: ровно это
        // случилось бы живьём, если разборщики перепутать.
        assert!(look_claude_at(exe, true, Some(&codex)).is_err());
        assert!(look_codex_at(exe, true, Some(&claude)).is_err());
    }

    /// `*_unset` зовёт именно `mcp remove` своего CLI: тела обеих функций
    /// можно было заменить на `Ok(())`, и набор остался бы зелёным (находка
    /// ре-ревью). Подставной раннер — тот же приём, что у `*_set_with`.
    #[test]
    fn unset_calls_mcp_remove_of_its_own_cli() {
        let calls = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let run = |args: &[String]| {
            calls.borrow_mut().push(args.to_vec());
            Ok(())
        };
        claude_unset_with("1c", run).unwrap();
        codex_unset_with("vrunner", run).unwrap();
        assert_eq!(*calls.borrow(), vec![claude_remove_args("1c"), codex_remove_args("vrunner")]);
        // Отказ CLI доходит до вызывающего словами, а не теряется.
        assert_eq!(claude_unset_with("1c", |_: &[String]| Err("no such server".to_string())).unwrap_err(), "no such server");
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

    /// Агента нет на машине — `set` не зовём вовсе, симметрично
    /// `teardown_cli`. Сегодня этот случай недостижим (пункт меню недоступен),
    /// и защита держалась ровно на меню: `Missing` проваливался в цикл записи
    /// (находка ре-ревью).
    #[test]
    fn setup_cli_calls_nothing_when_the_agent_is_missing() {
        let calls = std::cell::RefCell::new(0usize);
        let look = |_: &Path, _: bool| Ok(Found::Missing);
        let set = |_: &str, _: &Path| {
            *calls.borrow_mut() += 1;
            Ok(())
        };
        assert_eq!(setup_cli(Path::new("/opt/1c-connect"), false, look, set).unwrap(), Found::Missing);
        assert_eq!(*calls.borrow(), 0);
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

    /// Пункт 3 ревью целого: снятие убирает наши записи и не трогает чужие.
    /// `1c` занят чужой командой, `vrunner` наш — `unset` зовётся только по
    /// `vrunner`, а итог `Foreign(["1c"])` — успех снятия, не отказ: прежде
    /// отказывали целиком, и нашу `vrunner` нельзя было снять галочкой больше
    /// никогда. Нечитаемое состояние при этом по-прежнему отказ без вызовов:
    /// fail-closed обязан держаться в обе стороны.
    #[test]
    fn teardown_cli_unsets_ours_and_leaves_a_foreign_name_alone() {
        let calls = std::cell::RefCell::new(Vec::<String>::new());
        let look = |_: &Path, _: bool| Ok(Found::Foreign(vec!["1c".to_string()]));
        let unset = |name: &str| {
            calls.borrow_mut().push(name.to_string());
            Ok(())
        };
        assert_eq!(teardown_cli(Path::new("/opt/1c-connect"), true, look, unset).unwrap(), Found::Foreign(vec!["1c".to_string()]));
        assert_eq!(*calls.borrow(), vec!["vrunner".to_string()], "по занятому чужим имени CLI звать нельзя");

        calls.borrow_mut().clear();
        let unreadable = |_: &Path, _: bool| Err("не прочитать .claude.json: отказано в доступе".to_string());
        let err = teardown_cli(Path::new("/opt/1c-connect"), true, unreadable, unset).unwrap_err();
        assert!(err[0].contains("не прочитать"), "{err:?}");
        assert!(calls.borrow().is_empty(), "не зная состояния, CLI не зовём вовсе");
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

    /// Сегодня «агенты здесь» ровно `mode == "local"` — именованный предикат
    /// существует, чтобы расшириться без переименования условия, когда
    /// появится серверный режим (уточнение владельца 30.10): там коннектор
    /// стоит на машине разработчика и принимает подключение из туннеля, а
    /// агенты всё равно здесь же. Живёт он в `editors`, а не в трее, потому
    /// что спрашивают его двое — подменю и факт набора `EDITOR_MCP`.
    #[test]
    fn agents_are_here_only_in_local_mode_for_now() {
        assert!(agents_live_here("local"));
        assert!(!agents_live_here("tunnel"));
        assert!(!agents_live_here("something-else"));
    }

    /// Реестр `AGENTS` не рассыпается по длине под тестом `tray::AGENTS`
    /// (четыре имени, порядок — файловые агенты, затем CLI-агенты) — тот же
    /// контракт, по которому меню строит свои id пунктов `agent:<id>`.
    #[test]
    fn the_agent_registry_has_four_agents_in_the_documented_order() {
        assert_eq!(AGENTS.iter().map(|a| a.id).collect::<Vec<_>>(), ["zed", "pi", "claude", "codex"]);
        assert_eq!(AGENTS.iter().map(|a| a.label).collect::<Vec<_>>(), ["Zed", "pi", "Claude Code", "Codex"]);
        // Общий дом скиллов у трёх, свой — у Claude Code (докстрока `AgentDef::skills`).
        assert_eq!(
            AGENTS.iter().map(|a| a.skills).collect::<Vec<_>>(),
            [skills::Home::Agents, skills::Home::Agents, skills::Home::Claude, skills::Home::Agents]
        );
    }

    #[test]
    fn the_checkbox_is_on_only_when_both_halves_are_ours() {
        let ours: Result<Found, String> = Ok(Found::Ours);
        let absent: Result<Found, String> = Ok(Found::Absent);
        let missing: Result<Found, String> = Ok(Found::Missing);
        let foreign_mcp: Result<Found, String> = Ok(Found::Foreign(vec!["1c".to_string()]));
        let foreign_skills: Result<Found, String> = Ok(Found::Foreign(vec!["cf-edit".to_string()]));
        // Две РАЗНЫЕ строки, не одна на обе половины: иначе подмена,
        // возвращающая всегда один и тот же жёсткий литерал вместо настоящей
        // причины, прошла бы незамеченной (круг 1 ревью, пункт 3).
        let err_mcp: Result<Found, String> = Err("не прочитать ~/.codex/config.toml".to_string());
        let err_skills: Result<Found, String> = Err("не прочитать отпечаток скилла".to_string());

        assert_eq!(both(&ours, &ours), Ok(Found::Ours));
        assert_eq!(both(&ours, &absent), Ok(Found::Absent));
        assert_eq!(both(&absent, &ours), Ok(Found::Absent));

        // Чужое в одной половине — Foreign с ИМЕННО её именами, не перепутанными.
        assert_eq!(both(&foreign_mcp, &ours), Ok(Found::Foreign(vec!["1c".to_string()])));
        assert_eq!(both(&ours, &foreign_skills), Ok(Found::Foreign(vec!["cf-edit".to_string()])));
        // Чужое в ОБЕИХ половинах разом — называем обе, не теряем вторую
        // (круг 1 ревью, пункт 4): человек правит записи MCP, жмёт снова, и
        // не должен услышать «чужое» без упоминания скилла, о котором
        // промолчали в прошлый раз.
        assert_eq!(both(&foreign_mcp, &foreign_skills), Ok(Found::Foreign(vec!["1c".to_string(), "cf-edit".to_string()])));

        // Агента нет на машине — перекрывает всё, даже нечитаемую другую
        // половину и даже чужую запись в ней: это происходит каждый день —
        // агента вовсе не ставили, а общий `~/.agents/skills` уже несёт
        // (чужой или наш) скилл, положенный туда соседом (круг 1 ревью,
        // пункт 5 — зеркало того случая, которого не бывает).
        assert_eq!(both(&missing, &ours), Ok(Found::Missing));
        assert_eq!(both(&ours, &missing), Ok(Found::Missing));
        assert_eq!(both(&missing, &err_skills), Ok(Found::Missing));
        assert_eq!(both(&missing, &foreign_skills), Ok(Found::Missing));

        // Нечитаемая половина — Err с ЕЁ ПРИЧИНОЙ, а не «нет» и не причина
        // другой половины.
        assert_eq!(both(&err_mcp, &ours), err_mcp.clone());
        assert_eq!(both(&ours, &err_skills), err_skills.clone());
        // Обе половины нечитаемы разом — доходит причина MCP: порядок
        // проверок в `both` содержательный (докстрока выше), а не случайная
        // расстановка двух одинаковых по весу `if let`, и без этого теста
        // перестановка блоков местами осталась бы незамеченной (круг 2
        // ре-ревью, пункт 2).
        assert_eq!(both(&err_mcp, &err_skills), err_mcp.clone());
    }

    /// Простые заглушки под `Setup::Cli`, чтобы собрать свой маленький реестр
    /// агентов без файловой системы и переменных среды — они общие на весь
    /// прогон тестов, трогать их здесь нельзя (правило проекта).
    fn always_present() -> bool {
        true
    }
    fn noop_set(_: &str, _: &Path) -> Result<(), String> {
        Ok(())
    }
    fn noop_unset(_: &str) -> Result<(), String> {
        Ok(())
    }
    fn status_ours(_: &Path, _: bool) -> Result<Found, String> {
        Ok(Found::Ours)
    }
    fn status_absent(_: &Path, _: bool) -> Result<Found, String> {
        Ok(Found::Absent)
    }
    fn status_missing(_: &Path, _: bool) -> Result<Found, String> {
        Ok(Found::Missing)
    }
    fn status_err(_: &Path, _: bool) -> Result<Found, String> {
        Err("не прочитать".to_string())
    }
    fn status_foreign(_: &Path, _: bool) -> Result<Found, String> {
        Ok(Found::Foreign(vec!["vrunner".to_string()]))
    }
    /// Подпись НАРОЧНО не равна идентификатору (круг 2 ре-ревью, пункт 1):
    /// фильтр самоисключения в `any_agent_holds` сравнивает именно `id`, и
    /// если подставной агент давал `label == id`, подмена `a.id != id` на
    /// `a.label != id` проходила бы тестами незамеченной — в настоящем
    /// реестре подпись («Zed», «Claude Code») и идентификатор («zed»,
    /// «claude») расходятся всегда, а в подставном это должно быть видно
    /// тоже.
    fn fake_agent(id: &'static str, home: skills::Home, status: fn(&Path, bool) -> Result<Found, String>) -> AgentDef {
        AgentDef {
            id,
            label: "подставная подпись",
            setup: Setup::Cli { present: always_present, status, set: noop_set, unset: noop_unset },
            skills: home,
        }
    }

    /// Своя же запись не повод ждать самого себя — `others_hold`/
    /// `any_agent_holds` спрашивают про СОСЕДЕЙ, а спрашивающий передан
    /// отдельно и исключается явно (круг 1 ревью, пункт 1). Без исключения
    /// единственный агент в списке, спрашивающий сам про себя с живой
    /// `Ours`-записью, держал бы общий дом вечно.
    #[test]
    fn asking_about_yourself_does_not_count_as_a_neighbor_holding() {
        let exe = Path::new("/opt/1c-connect");
        let only_self = [fake_agent("zed", skills::Home::Agents, status_ours)];
        assert!(!any_agent_holds(&only_self, "zed", skills::Home::Agents, exe));
    }

    /// Zed и Codex делят `~/.agents/skills`: пока у Codex свои записи MCP —
    /// общий дом занят, снимать скиллы рано. Нужны ровно два агента с разным
    /// статусом: одного было бы не отличить от «общий дом всегда свободен».
    /// Спрашивает Zed — он в списке тоже есть, но свой статус не должен
    /// влиять на ответ про соседей.
    #[test]
    fn the_shared_folder_waits_for_the_last_other_agent() {
        let exe = Path::new("/opt/1c-connect");

        let codex_still_holds =
            [fake_agent("zed", skills::Home::Agents, status_absent), fake_agent("codex", skills::Home::Agents, status_ours)];
        assert!(any_agent_holds(&codex_still_holds, "zed", skills::Home::Agents, exe), "Codex ещё держит записи — общий дом не свободен");

        let both_let_go =
            [fake_agent("zed", skills::Home::Agents, status_absent), fake_agent("codex", skills::Home::Agents, status_absent)];
        assert!(!any_agent_holds(&both_let_go, "zed", skills::Home::Agents, exe), "оба сняли записи — общий дом свободен");
    }

    /// У кого из трёх общих агентов стоит только Zed, Codex и pi физически
    /// нет на машине (`Missing`) — и спрашивать не о чём: без этой ветки
    /// (круг 1 ревью, пункт 2) дом с единственным реально установленным
    /// агентом из троих не освобождался бы никогда, даже когда у этого
    /// единственного своя запись давно снята.
    #[test]
    fn an_uninstalled_neighbor_does_not_count_as_holding() {
        let exe = Path::new("/opt/1c-connect");
        let neighbor_not_installed =
            [fake_agent("zed", skills::Home::Agents, status_absent), fake_agent("codex", skills::Home::Agents, status_missing)];
        assert!(!any_agent_holds(&neighbor_not_installed, "zed", skills::Home::Agents, exe));
    }

    /// Нечитаемая запись соседа и чужая запись на его месте — обе «настроен»,
    /// не «нет»: решение хозяина «не унести лишнего важнее, чем прибраться».
    /// И дом, который не совпадает с запрошенным (`Home::Claude` при вопросе
    /// про `Home::Agents`), в счёт не идёт — иначе Claude Code держал бы
    /// общий каталог Zed/pi/Codex, которым не пользуется вовсе.
    #[test]
    fn unreadable_or_foreign_neighbors_count_as_held_but_other_homes_do_not() {
        let exe = Path::new("/opt/1c-connect");
        assert!(any_agent_holds(&[fake_agent("codex", skills::Home::Agents, status_err)], "zed", skills::Home::Agents, exe));
        assert!(any_agent_holds(&[fake_agent("codex", skills::Home::Agents, status_foreign)], "zed", skills::Home::Agents, exe));
        assert!(!any_agent_holds(&[fake_agent("claude", skills::Home::Claude, status_ours)], "zed", skills::Home::Agents, exe));
    }

    /// Проверка самого `others_hold` (не только `any_agent_holds`, которого
    /// здесь не хватало — круг 1 ревью, пункт 6), на настоящем `AGENTS`.
    ///
    /// Первое утверждение — на предпосылке: `Home::Claude` в реестре несёт
    /// только самого Claude Code (докстрока `AgentDef::skills`), и, спрашивая
    /// от его же имени, фильтр по `id` отбрасывает единственного кандидата ДО
    /// того, как до него дойдёт `.any()` — значит `setup.look` ни разу не
    /// позовётся, и результат не зависит от того, что в реальности лежит в
    /// `~/.claude.json` на этой машине. Предпосылка («ровно один агент с этим
    /// домом») закреплена явно: появится второй агент на `Home::Claude`, или
    /// сменится id — и без этой страховки тест тихо начал бы ходить в диск
    /// (круг 2 ре-ревью, пункт 4).
    ///
    /// Второе утверждение — дифференциальное (круг 2 ре-ревью, пункт 3):
    /// `others_hold` обязана отвечать ТО ЖЕ, что `any_agent_holds(&AGENTS, ..)`
    /// на тех же аргументах — равенство верно на любой машине, поскольку
    /// сравнивает её с собственным определением, а не с заранее знаемым
    /// значением. Но ловит оно не любую подмену, а только тогда, когда у
    /// соседей по общему дому (Zed, pi, Codex) есть НАСТОЯЩИЕ записи MCP под
    /// этот `exe`: тогда правая сторона — `true`, и подмена тела `others_hold`
    /// на жёсткую `false` или на пустой реестр (`&[]` вместо `&AGENTS`)
    /// разводит левую и правую стороны. На машине, где у соседей таких
    /// записей нет (замерено прямо на этой — см. отчёт задачи, круг 2),
    /// правая сторона сама вычисляется в `false`, и обе названные подмены
    /// проходят незамеченными: `false == false` держится без всякой логики.
    /// Надёжно этим утверждением ловится только подмена тела на жёсткую
    /// `true` — но и её ловит первое утверждение (`!others_hold("claude", ..)`)
    /// раньше и без зависимости от соседей. Защита самой строки `others_hold`
    /// от «тело заменили на константу» держится не на тесте, а на её
    /// краткости (одна строка, видна при чтении диффа). Чтение настоящих
    /// файлов агентов Zed/pi/Codex при этом происходит — это разрешено
    /// («только читаем»), запрещена только правка переменных среды и чужих
    /// файлов в тесте.
    #[test]
    fn others_hold_asks_about_neighbors_through_the_real_registry_too() {
        let exe = Path::new("/opt/1c-connect");
        assert_eq!(AGENTS.iter().filter(|a| a.skills == skills::Home::Claude).count(), 1);
        assert!(!others_hold("claude", skills::Home::Claude, exe));
        assert_eq!(others_hold("zed", skills::Home::Agents, exe), any_agent_holds(&AGENTS, "zed", skills::Home::Agents, exe));
    }
}
