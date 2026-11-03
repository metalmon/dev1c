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
/// Секции нет вовсе — дописывается она целиком, в конец файла. `value` —
/// готовая правая часть (строки уже в кавычках).
///
/// Отличие от `render`: та перерисовывает файл по образцу, и свои строки
/// человека переживают правку только если образец — его же файл.
///
/// Повторяющаяся секция (`[[connection]]`) правится первым блоком — за
/// остальными и за отказом вместо дописывания идите в `set_in` ниже: писать
/// одному соединению ключ в чужой блок или в дописанный пустой нельзя.
pub fn set(text: &str, section: &str, key: &str, value: &str) -> String {
    if let Some(out) = set_in(text, section, 0, key, value) {
        return out;
    }
    // Секции нет вовсе — дописываем её целиком в конец.
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let end = lines.len().saturating_sub(1);
    lines.insert(end, section.to_string());
    lines.insert(end + 1, format!("{key} = {value}"));
    lines.join("\n")
}

/// `set` для `nth`-го блока ПОВТОРЯЮЩЕЙСЯ секции (`[[connection]]`, блоки
/// считаются от нуля) и для КОРНЯ файла (`section` пуст — всё до первой
/// `[таблицы]`, там живёт ключ `current`).
///
/// Корень — пустое имя секции, а не отдельная функция: разбор здесь и так
/// ведёт «последнюю встреченную `[...]`», и до первой таблицы это ровно
/// пустая строка — то есть корневой случай уже описан тем же состоянием, а
/// вторая функция держала бы вторую копию цикла. Корневой ключ, которого в
/// файле нет, дописывается ПЕРЕД первой таблицей: в TOML корневые ключи
/// обязаны стоять до неё, иначе ключ молча уехал бы в чужую таблицу.
///
/// `None` — такого блока в тексте нет, и дописывать его тут нечем: ключ
/// одного соединения в наспех дописанный пустой блок — это молча неверная
/// запись, а не запись. Решать, что с этим делать, — вызывающему
/// (`cfg::set_current` отказывает словами).
pub fn set_in(text: &str, section: &str, nth: usize, key: &str, value: &str) -> Option<String> {
    // split('\n'), а не lines(): так сохраняются и '\r' в концах строк, и
    // наличие/отсутствие перевода строки в конце файла.
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let cr = |l: &str| if l.ends_with('\r') { "\r" } else { "" };
    let root = section.is_empty();
    // Корень начинается с первой же строки файла — своего заголовка у него нет.
    let mut inside = root;
    let mut found = root;
    let mut seen = 0; // сколько блоков этой секции уже прошло
    let mut tail = None; // последняя непустая строка нужного блока
    for i in 0..lines.len() {
        let t = lines[i].trim();
        if t.starts_with('[') && t.ends_with(']') {
            inside = t == section && seen == nth;
            if t == section {
                seen += 1;
            }
            if inside {
                found = true;
                tail = Some(i);
            }
            continue;
        }
        if !inside {
            continue;
        }
        if !t.is_empty() {
            tail = Some(i);
        }
        if !t.starts_with('#') && t.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            lines[i] = format!("{key} = {value}{}", cr(&lines[i]));
            return Some(lines.join("\n"));
        }
    }
    if !found {
        return None;
    }
    match tail {
        Some(i) => lines.insert(i + 1, format!("{key} = {value}{}", cr(&lines[i]))),
        // Корень без единой строки до первой таблицы — ключ идёт в самое
        // начало файла (см. докстроку).
        None => lines.insert(0, format!("{key} = {value}")),
    }
    Some(lines.join("\n"))
}

/// Терпимое чтение одного значения из текста самого файла — **не разборщик
/// TOML**, и не притворяется им: без полного разбора, без оглядки на то,
/// проходят ли остальные строки свою проверку, без вложенных кавычек,
/// экранирования и многострочных значений. Для случая, когда файл целиком
/// не прошёл настоящий разбор (`toml::from_str`/`validate()`), а один
/// конкретный ключ всё равно нужно прочитать (`ops_host::watch_mcp`, круг
/// исправления 2 задачи 3 плана «тонкий срез ценности» — потолок записи
/// обязан дойти до сеанса даже из негодного файла).
///
/// Снимает одну пару внешних кавычек (двойных или одинарных) и хвостовой
/// комментарий (см. `unquote`); ключа, секции или строки-комментария —
/// `None`, тем же приёмом, что у `set` выше (секция узнаётся по последней
/// встреченной `[...]`). **Вызывающий обязан считать нераспознанное или не
/// точно совпавшее значение небезопасным случаем**, а не подставлять
/// умолчание наугад: это терпимое чтение одной строки для ветки, где
/// полноценный разбор уже провалился, а не его замена — заводить здесь
/// `toml::Value` означало бы тащить назад то, что уже не сработало.
pub fn get(text: &str, section: &str, key: &str) -> Option<String> {
    get_in(text, section, 0, key)
}

/// `get` для `nth`-го блока повторяющейся секции и для корня файла — те же
/// два случая и те же доводы, что у `set_in` выше.
pub fn get_in(text: &str, section: &str, nth: usize, key: &str) -> Option<String> {
    let root = section.is_empty();
    let mut inside = root;
    let mut seen = 0;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            inside = t == section && seen == nth;
            if t == section {
                seen += 1;
            }
            continue;
        }
        if !inside || t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if k.trim() == key {
                return Some(unquote(v.trim()));
            }
        }
    }
    None
}

/// Снимает одну пару внешних кавычек (двойных или одинарных) и хвостовой
/// комментарий у значения строки TOML. В кавычках — всё до закрывающей
/// кавычки ТОГО ЖЕ вида, остальное (в том числе `# комментарий`) отбрасывается
/// как есть, без разбора; без кавычек — всё до первого `#`, если он есть.
/// Не разборщик: экранированная кавычка внутри строки, многострочные
/// значения и прочие тонкости настоящего TOML здесь не предусмотрены.
fn unquote(v: &str) -> String {
    let mut chars = v.chars();
    match chars.next() {
        Some(q @ ('"' | '\'')) => {
            let rest = &v[1..];
            match rest.find(q) {
                Some(end) => rest[..end].to_string(),
                None => rest.to_string(),
            }
        }
        _ => v.split('#').next().unwrap_or(v).trim().to_string(),
    }
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

    /// Терпимое чтение одного ключа: без кавычек, по секции, мимо
    /// комментариев и одноимённых ключей в чужой секции; отсутствующий
    /// ключ или секция — `None`.
    #[test]
    fn get_reads_one_key_tolerantly() {
        let mine = "[host]\n# комментарий\nwrite = \"read_only\"\nbase_path = \"\"\n\n[other]\nwrite = \"read_write\"\n";
        assert_eq!(get(mine, "[host]", "write"), Some("read_only".into()));
        assert_eq!(get(mine, "[host]", "base_path"), Some("".into()));
        assert_eq!(get(mine, "[host]", "missing"), None);
        assert_eq!(get(mine, "[missing]", "write"), None);
        // Чужая секция под тем же именем ключа не подменяет свою.
        assert_eq!(get(mine, "[other]", "write"), Some("read_write".into()));
    }

    /// Круг исправления 4 задачи 3: хвостовой комментарий и одинарные
    /// кавычки — не повод отдать мусор вместо значения.
    #[test]
    fn get_handles_a_trailing_comment_and_single_quotes() {
        let text = "[host]\nwrite = \"read_write\" # комментарий человека\n";
        assert_eq!(get(text, "[host]", "write"), Some("read_write".into()));
        let text = "[host]\nwrite = 'read_only'\n";
        assert_eq!(get(text, "[host]", "write"), Some("read_only".into()));
        let text = "[host]\nwrite = read_write # без кавычек вовсе\n";
        assert_eq!(get(text, "[host]", "write"), Some("read_write".into()));
    }

    /// Блок повторяющейся секции адресуется номером: ключ правится в том
    /// блоке, который попросили, а соседний не трогается вовсе. Это и есть
    /// адрес, которым пишет текущее соединение (`cfg::set_current`).
    #[test]
    fn set_in_edits_the_asked_block_and_leaves_its_neighbours_alone() {
        let two = "[[connection]]\nname = \"Buh\"\nwrite = \"read_only\"\n\n[[connection]]\nname = \"Ромашка\"\nwrite = \"read_only\"\n";
        let out = set_in(two, "[[connection]]", 1, "write", "\"read_write\"").unwrap();
        assert_eq!(
            out,
            "[[connection]]\nname = \"Buh\"\nwrite = \"read_only\"\n\n[[connection]]\nname = \"Ромашка\"\nwrite = \"read_write\"\n"
        );
        // Ключа в блоке нет — дописывается в конец СВОЕГО блока, не чужого.
        let out = set_in(two, "[[connection]]", 0, "dump", "\"C:\\\\src\"").unwrap();
        assert_eq!(
            out,
            "[[connection]]\nname = \"Buh\"\nwrite = \"read_only\"\ndump = \"C:\\\\src\"\n\n[[connection]]\nname = \"Ромашка\"\nwrite = \"read_only\"\n"
        );
        assert_eq!(get_in(two, "[[connection]]", 1, "name"), Some("Ромашка".into()));
        // Блока с таким номером нет — `None`, а не дописанный пустой блок.
        assert_eq!(set_in(two, "[[connection]]", 2, "write", "\"read_write\""), None);
        assert_eq!(get_in(two, "[[connection]]", 2, "write"), None);
    }

    /// Корневой ключ (`current` у настроек коннектора): секции у него нет
    /// вовсе, и адресуется он пустым именем секции. Отсутствующий —
    /// дописывается ПЕРЕД первой таблицей, иначе он уехал бы внутрь неё.
    #[test]
    fn a_root_key_is_addressed_by_an_empty_section() {
        let text = "# шапка\ncurrent = \"Buh\"\n\n[[connection]]\nname = \"Buh\"\ncurrent = \"чужой\"\n";
        assert_eq!(get(text, "", "current"), Some("Buh".into()), "корень читается, а не одноимённый ключ блока");
        let out = set(text, "", "current", "\"Ромашка\"");
        assert_eq!(out, "# шапка\ncurrent = \"Ромашка\"\n\n[[connection]]\nname = \"Buh\"\ncurrent = \"чужой\"\n");
        // Ключа в корне нет — встаёт до первой таблицы, а не в её конец.
        let no_root = "# шапка\n[[connection]]\nname = \"Buh\"\n";
        assert_eq!(set(no_root, "", "current", "\"Buh\""), "# шапка\ncurrent = \"Buh\"\n[[connection]]\nname = \"Buh\"\n");
        // Файл без таблиц вовсе — ключ просто дописывается.
        assert_eq!(set("# шапка\n", "", "current", "\"Buh\""), "# шапка\ncurrent = \"Buh\"\n");
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
