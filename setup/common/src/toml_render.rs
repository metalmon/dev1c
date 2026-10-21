//! Rewrite the values of a commented example TOML in place: `change` edits
//! a file the person reads, so the comments must survive.

/// `value(section, key)` returns the new right-hand side (already quoted for
/// strings) or `None` to keep the line as it is.
pub fn render(example: &str, value: impl Fn(&str, &str) -> Option<String>) -> String {
    let mut section = String::new();
    let mut out = String::new();
    for line in example.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t.to_string();
            out.push_str(line);
        } else if let Some((key, _)) = t.split_once(" = ").filter(|_| !t.starts_with('#')) {
            match value(&section, key) {
                Some(v) => out.push_str(&format!("{key} = {v}")),
                None => out.push_str(line),
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Правка одного значения **в тексте самого файла**: меняется только строка
/// этого ключа, всё остальное — комментарии человека, порядок ключей, пустые
/// строки и переводы строк — остаётся как есть. Ключа в секции нет — строка
/// дописывается в её конец (так же дописывает `seed` в скрипте стенда).
/// `value` — готовая правая часть (строки уже в кавычках).
///
/// Отличие от `render`: та перерисовывает файл по образцу, и свои строки
/// человека переживают правку только если образец — его же файл.
pub fn set(text: &str, section: &str, key: &str, value: &str) -> String {
    // split('\n'), а не lines(): так сохраняются и '\r' в концах строк, и
    // наличие/отсутствие перевода строки в конце файла.
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let cr = |l: &str| if l.ends_with('\r') { "\r" } else { "" };
    let mut cur = String::new();
    let mut tail = None; // последняя непустая строка нужной секции
    for i in 0..lines.len() {
        let t = lines[i].trim();
        if t.starts_with('[') && t.ends_with(']') {
            cur = t.to_string();
            if cur == section {
                tail = Some(i);
            }
            continue;
        }
        if cur != section {
            continue;
        }
        if !t.is_empty() {
            tail = Some(i);
        }
        if !t.starts_with('#') && t.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            lines[i] = format!("{key} = {value}{}", cr(&lines[i]));
            return lines.join("\n");
        }
    }
    match tail {
        Some(i) => lines.insert(i + 1, format!("{key} = {value}{}", cr(&lines[i]))),
        // Секции нет вовсе — дописываем её целиком в конец.
        None => {
            let end = lines.len().saturating_sub(1);
            lines.insert(end, section.to_string());
            lines.insert(end + 1, format!("{key} = {value}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EX: &str = "# top\n[a]\n# about x\nx = 1\nname = \"a\"\n\n[b]\nname = \"b\"\n";

    /// Правка на месте: своя строка человека и его порядок ключей целы, а
    /// отсутствующий ключ дописывается в конец своей секции, не чужой.
    #[test]
    fn set_edits_one_line_and_keeps_the_rest() {
        let mine = "[host]\n# порт 9000 занят Апачем, не менять\nhttp_port = 0\n";
        let out = set(mine, "[host]", "http_port", "9000");
        assert_eq!(out, "[host]\n# порт 9000 занят Апачем, не менять\nhttp_port = 9000\n");

        let added = set(mine, "[host]", "gitea", "\"http://localhost:3000\"");
        assert_eq!(added, "[host]\n# порт 9000 занят Апачем, не менять\nhttp_port = 0\ngitea = \"http://localhost:3000\"\n");

        // Ключ правится в своей секции, одноимённый в чужой — не трогается.
        assert_eq!(set(EX, "[b]", "name", "\"B\""), "# top\n[a]\n# about x\nx = 1\nname = \"a\"\n\n[b]\nname = \"B\"\n");
        // Концы строк CRLF переживают правку.
        assert_eq!(set("[host]\r\nx = 1\r\n", "[host]", "x", "2"), "[host]\r\nx = 2\r\n");
    }

    #[test]
    fn values_change_by_section_and_comments_stay() {
        let out = render(EX, |section, key| match (section, key) {
            ("[a]", "x") => Some("2".into()),
            ("[b]", "name") => Some("\"B\"".into()),
            _ => None,
        });
        assert_eq!(out, "# top\n[a]\n# about x\nx = 2\nname = \"a\"\n\n[b]\nname = \"B\"\n");
    }
}
