//! Номера отказов: четыре цифры, которые человек диктует по телефону.
//!
//! Первые две цифры — область (`AREAS`), последние две — место внутри неё.
//! Номер стоит ВПЕРЕДИ слов: левый слот подвала окна обрезан двумя строками с
//! многоточием, и номер в конце уезжал бы под обрез ровно на самых длинных
//! отказах — то есть тогда, когда он нужнее всего.
//!
//! Механизм живёт здесь, а не в окне: проверка собирается всегда, окно — по
//! фиче `gui`, и номер обязан быть в обеих сборках.

/// Разделитель между номером и словами. Пробел вокруг `·` — чтобы номер
/// читался отдельным словом и не слипался с текстом на узком слоте.
pub const SEP: &str = " · ";

/// Области номеров. Таблица одна на весь крейт: сторож в тестах сверяется с
/// ней, а не со своим списком — два списка разошлись бы молча.
///
/// Строкой на область, а не одной лентой: таблицу читают и правят люди, а
/// rustfmt свернул бы её в 143 знака в одну строку.
#[rustfmt::skip]
pub const AREAS: &[(u16, &str)] = &[
    (10, "параметры"),
    (11, "запуск"),
    (12, "ворота"),
    (13, "отказы 1С"),
    (14, "операции"),
    (15, "факты проверки"),
    (16, "мост и сеанс"),
];

/// Номера на пенсии: ветка убрана — её номер уходит сюда и соседу не
/// достаётся. Иначе старый бинарь у пилота и новый у нас говорят одно число о
/// разном, а это ровно та ловушка, против которой вся работа.
///
/// Как пользоваться: убрал точку отказа — перенеси её номер строкой
/// `(1204, "22.11.2026, каталог")`: номер, дата, одно слово о том, чем он был.
/// Сторож не даст выдать такой номер заново.
pub const RETIRED: &[(u16, &str)] = &[];

/// Строка отказа: номер, затем слова. Один формат на все поверхности — подвал
/// окна, журнал, отчёт, терминал, — чтобы человек диктовал одно и то же
/// независимо от того, где прочёл.
pub fn say(code: u16, text: impl AsRef<str>) -> String {
    format!("{code}{SEP}{}", text.as_ref())
}

/// Обратная к [`say`]: вынуть номер из начала строки и отдать остаток без него.
/// Номера нет — отдаём строку как есть.
///
/// Нужна там, где отказ уже стал строкой, а показать надо не его слова, а
/// разобранную причину 1С: номер места иначе до человека не доехал бы вовсе.
pub fn split(text: &str) -> (Option<u16>, &str) {
    let Some((head, rest)) = text.split_once(SEP) else {
        return (None, text);
    };
    // Ровно четыре цифры и ничего больше: `parse` пропустил бы «+123» и
    // «1402·1305» (двойной номер отказа операции) приняло бы за мусор молча.
    if head.len() == 4 && head.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(code) = head.parse::<u16>() {
            return (Some(code), rest);
        }
    }
    (None, text)
}

// ----------------------------------------------------------------------
// Метка сборки. Номер говорит, ГДЕ отказ; метка — в КАКОМ бинаре. По телефону
// спрашивают и то, и другое, поэтому вторая половина опознания живёт в этом же
// модуле: формат метки обязан быть один на шапку окна, журнал и `--version`.
// ----------------------------------------------------------------------

/// Как человек называет ветку. Шапку окна пилот читает вслух, а
/// `worktree-pilot-28-10` вслух не читается. Ветка без строки здесь называет
/// себя своим именем: таблица нужна только там, где машинное имя мешает.
pub const BRANCHES: &[(&str, &str)] = &[("worktree-pilot-28-10", "пилот")];

/// Чем метка называет неизвестное. Константами, а не литералами по месту: обе
/// строки ставит в метку `build.rs`, а сверяются с ними тесты — у значения два
/// потребителя, и разойтись им нельзя.
pub const NO_HASH: &str = "без хеша";
/// Ветка без имени: отсоединённая голова или сборка не из репозитория.
pub const NO_BRANCH: &str = "сборка";

/// Короткая форма метки — шапке окна. Самого хеша в ней нет нарочно: шапку
/// читают вслух, а семь знаков по телефону не диктуют.
///
/// Но знать про хеш она обязана. Хеша нет — сборка идёт из копии без `.git`,
/// опознать её нечем, и короткая форма говорит об этом прямо: имя ветки на
/// этом месте обещало бы опознание, которого нет.
pub fn stamp_short(date: &str, hash: Option<&str>, branch: &str) -> String {
    if hash.is_none() {
        return format!("{date} {NO_HASH}");
    }
    let human = BRANCHES.iter().find(|(name, _)| *name == branch).map_or(branch, |(_, human)| *human);
    format!("{date} {human}")
}

/// Полная форма — в журнал, в отчёт «Проверить» и в `--version`.
///
/// Хеша нет (собирали из копии без `.git`) — так и сказано словами. Отсутствие
/// `.git` сборку не валит, но и выдавать такую сборку за опознанную нельзя:
/// пустое место на месте хеша человек прочёл бы как «всё в порядке».
pub fn stamp_full(date: &str, hash: Option<&str>, branch: &str) -> String {
    format!("{date} {} {branch}", hash.unwrap_or(NO_HASH))
}

/// Дата сборки в двух видах, которые носит метка: `22.11` для шапки окна и
/// `2026-11-22` для полной формы.
pub fn stamp_dates(y: u64, m: u64, d: u64) -> (String, String) {
    (format!("{d:02}.{m:02}"), format!("{y:04}-{m:02}-{d:02}"))
}

/// Дата из `YYYY-MM-DD` — ровно то, что печатают `date +%F` и `Get-Date`,
/// которых `build.rs` и спрашивает про местный день.
///
/// `None` на всём остальном: пустой вывод, чужая локаль, `22.11.2026`. Строгий
/// вид, а не «что разобралось»: день из чужого формата попал бы в метку
/// перевёрнутым, и человек диктовал бы неправильную дату, не зная об этом.
///
/// Разбор стоит здесь, а не в `build.rs`: там его не проверить ни одним тестом.
pub fn parse_ymd(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || (y.len(), m.len(), d.len()) != (4, 2, 2) {
        return None;
    }
    let ymd = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    ((1..=12).contains(&ymd.1) && (1..=31).contains(&ymd.2)).then_some(ymd)
}

/// Год, месяц и день по секундам эпохи, в UTC.
///
/// Запасная дорога `build.rs`: местный день он спрашивает у системы, а это —
/// на случай, когда команда не вышла. Тогда ночная сборка подпишется вчерашним
/// днём, но подпишется: без даты метка хуже, чем со сдвинутой на день.
///
/// Пересчёт стоит здесь, а не в `build.rs`: там его не проверить ни одним
/// тестом, а ошибается именно он — на 29 февраля и на границе суток.
pub fn ymd_utc(unix_secs: u64) -> (u64, u64, u64) {
    ymd(unix_secs / 86_400)
}

/// Год, месяц и день по числу суток от 1970-01-01. Пересчёт Хиннанта: счёт
/// идёт эрами по 400 лет и от марта, поэтому високосный день оказывается
/// последним днём года и отдельной ветки под него не нужно.
fn ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (era * 400 + yoe + u64::from(m <= 2), m, d)
}

/// Отказ с номером: как `anyhow::bail!`, но текст проходит через [`say`].
///
/// Раскрывается без имён `bail!`/`anyhow!`: сторож ищет голые отказы по этим
/// самым словам, и упомяни их здесь — он ослеп бы в собственном доме.
#[macro_export]
macro_rules! code_bail {
    ($code:expr, $($arg:tt)*) => {
        return ::core::result::Result::Err(::anyhow::Error::msg($crate::code::say($code, ::std::format!($($arg)*))))
    };
}

/// Условие с номером: как `anyhow::ensure!`, но текст проходит через [`say`].
#[macro_export]
macro_rules! code_ensure {
    ($cond:expr, $code:expr, $($arg:tt)*) => {
        if !$cond {
            $crate::code_bail!($code, $($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_code_stands_before_the_words_and_belongs_to_a_known_area() {
        assert_eq!(say(1204, "каталога базы нет"), "1204 · каталога базы нет");
        assert!(AREAS.iter().any(|(a, _)| *a == 12));
        assert!(!AREAS.iter().any(|(a, _)| *a == 99));
    }

    #[test]
    fn a_build_without_git_still_has_an_honest_stamp() {
        assert_eq!(stamp_short("22.11", Some("a3f1c2d"), "worktree-pilot-28-10"), "22.11 пилот");
        assert_eq!(stamp_short("22.11", Some("a3f1c2d"), "main"), "22.11 main");
        // Отсоединённая голова: хеш есть, имени ветки нет.
        assert_eq!(stamp_short("22.11", Some("a3f1c2d"), NO_BRANCH), "22.11 сборка");
        // Копия без `.git`: опознавать нечем, и короткая форма это говорит —
        // её человек диктует вслух, и она не вправе обещать опознание.
        assert_eq!(stamp_short("22.11", None, NO_BRANCH), "22.11 без хеша");
        assert_eq!(stamp_full("2026-11-22", Some("a3f1c2d"), "main"), "2026-11-22 a3f1c2d main");
        assert_eq!(stamp_full("2026-11-22", None, "main"), "2026-11-22 без хеша main");
    }

    #[test]
    fn the_stamp_date_holds_on_leap_years_and_at_the_turn_of_the_epoch() {
        assert_eq!(stamp_dates(2026, 11, 22), ("22.11".to_string(), "2026-11-22".to_string()));
        // Местный день приходит от системы строкой, и разбирать её надо
        // строго: «22.11.2026» разобранное «как получится» дало бы в метке
        // 2022-й год и 11-е число.
        assert_eq!(parse_ymd("2026-11-22"), Some((2026, 11, 22)));
        assert_eq!(parse_ymd("2026-11-22\r\n"), Some((2026, 11, 22)));
        assert_eq!(parse_ymd("22.11.2026"), None);
        assert_eq!(parse_ymd("2026-11-22-01"), None);
        assert_eq!(parse_ymd("2026-13-01"), None);
        assert_eq!(parse_ymd(""), None);
        // Запасная дорога — пересчёт от секунд эпохи. Он пишется руками
        // (крейтов не добавляем) и ошибётся именно здесь: 29 февраля, начало
        // отсчёта и последняя секунда суток.
        assert_eq!(ymd_utc(1_795_305_600), (2026, 11, 22));
        assert_eq!(ymd_utc(951_782_400), (2000, 2, 29));
        assert_eq!(ymd_utc(0), (1970, 1, 1));
        assert_eq!(ymd_utc(1_795_305_600 + 86_399), (2026, 11, 22));
    }

    #[test]
    fn split_undoes_say_and_leaves_unnumbered_lines_alone() {
        let line = say(1204, "каталога базы нет");
        assert_eq!(split(&line), (Some(1204), "каталога базы нет"));
        assert_eq!(split("Нельзя включить: каталога базы нет"), (None, "Нельзя включить: каталога базы нет"));
        // Трёхзначное и двойной номер — не наш формат: отдаём строку целиком,
        // чтобы никто не выдал «140» за область.
        assert_eq!(split("140 · текст"), (None, "140 · текст"));
        assert_eq!(split("1402·1305 · текст"), (None, "1402·1305 · текст"));
    }

    #[test]
    fn the_coded_macros_put_the_number_in_front() {
        fn refuse() -> anyhow::Result<()> {
            code_bail!(1204, "каталога базы {} нет", "C:\\Bases");
        }
        fn demand(ok: bool) -> anyhow::Result<()> {
            code_ensure!(ok, 1001, "сервер не задан");
            Ok(())
        }
        assert_eq!(refuse().unwrap_err().to_string(), "1204 · каталога базы C:\\Bases нет");
        assert_eq!(demand(false).unwrap_err().to_string(), "1001 · сервер не задан");
        assert!(demand(true).is_ok());
    }

    // ------------------------------------------------------------------
    // Сторож. 110 точек отказа руками не удержать, поэтому номера считает
    // машина: тест читает исходники мастерской обычным `std::fs` (новая
    // зависимость ради реестра номеров не окупается).
    // ------------------------------------------------------------------

    /// Чем зовут номер в коде. У всех пяти номер — аргумент без имени, одно
    /// четырёхзначное число среди прочих, поэтому разбирать позицию не нужно.
    const CALLS: &[&str] = &["say(", "code_bail!(", "code_ensure!(", "fail_code(", "busy_code("];

    /// Голый отказ — отказ без номера. Три правила про сами номера проходят и
    /// тогда, когда у новой точки номера просто нет, а дрейф начинается именно
    /// так: это правило — главное.
    const BARE: &[&str] = &["bail!", "ensure!", "anyhow!", "Check::fail(", "Check::busy("];

    /// Файлы, где голые отказы пока разрешены. Каждая следующая задача плана
    /// вычёркивает отсюда свои файлы; когда список опустеет до одной строки
    /// про `dev1c-setup`, в крейте не останется ни одного отказа без номера.
    ///
    /// Запись — начало пути: файл целиком или каталог целиком.
    const UNCOVERED: &[&str] = &[
        // `dev1c-setup` ждёт своей очереди: про него ещё не звонят, и номера он
        // получит, когда начнут. Эта строка остаётся НАВСЕГДА — она здесь одна
        // и названа поимённо, чтобы её нельзя было забыть и нельзя было
        // расширить молча.
        "dev1c-setup/",
        // Внутренний сговор, а не отказ человеку: «нет операции {id}» значит,
        // что программа позвала несуществующий шаг. Такое не диктуют по
        // телефону, номера тут не будет.
        "common/src/run.rs",
        // Разметка номеров по областям — задачи 3…7 плана.
        "1c-connect/src/allow.rs",
        "1c-connect/src/analyzer.rs",
        "1c-connect/src/bridge.rs",
        "1c-connect/src/cfg.rs",
        "1c-connect/src/flow.rs",
        "1c-connect/src/gui/autostart.rs",
        "1c-connect/src/gui/facts.rs",
        "1c-connect/src/gui/gate.rs",
        "1c-connect/src/gui/link.rs",
        "1c-connect/src/gui/log.rs",
        "1c-connect/src/gui/mod.rs",
        "1c-connect/src/gui/panel.rs",
        "1c-connect/src/gui/save.rs",
        "1c-connect/src/gui/theme.rs",
        "1c-connect/src/gui/tray.rs",
        "1c-connect/src/gui/window.rs",
        "1c-connect/src/holder.rs",
        "1c-connect/src/job.rs",
        "1c-connect/src/jsonrpc.rs",
        "1c-connect/src/lib.rs",
        "1c-connect/src/licence.rs",
        "1c-connect/src/listen.rs",
        "1c-connect/src/local.rs",
        "1c-connect/src/main.rs",
        "1c-connect/src/onec_error.rs",
        "1c-connect/src/ops_host.rs",
        "1c-connect/src/platform.rs",
        "1c-connect/src/project.rs",
        "1c-connect/src/publication.rs",
        "1c-connect/src/session.rs",
        "1c-connect/src/single_instance.rs",
        "1c-connect/src/tunnel.rs",
    ];

    /// Корень мастерской: рядом с `common` лежат остальные крейты.
    fn workshop() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("у common есть каталог-родитель").to_path_buf()
    }

    /// Все `*/src/**/*.rs` мастерской, путём от её корня и всегда со слэшем:
    /// список `UNCOVERED` обязан читаться одинаково на обеих осях.
    fn sources(root: &Path) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut crates: Vec<PathBuf> = std::fs::read_dir(root)
            .expect("корень мастерской читается")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("src").is_dir())
            .collect();
        crates.sort();
        for c in crates {
            walk(&c.join("src"), root, &mut out);
        }
        out.sort();
        out
    }

    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<PathBuf> =
            std::fs::read_dir(dir).expect("каталог исходников читается").filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, root, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let rel = p.strip_prefix(root).expect("путь внутри мастерской").to_string_lossy().replace('\\', "/");
                let text = std::fs::read_to_string(&p).expect("исходник читается");
                out.push((rel, text));
            }
        }
    }

    /// Выбросить всё, что собирается только под тестом. Обрезать файл по
    /// первому `#[cfg(test)]` нельзя: он стоит и на отдельных помощниках
    /// посреди файла (`1c-connect/src/cfg.rs:280`), и за ним идёт настоящий
    /// код. Пропускаем ровно один следующий элемент: либо одну строку с `;`,
    /// либо блок до закрывающей скобки в нулевой колонке — элементы в этом
    /// крейте начинаются с неё.
    ///
    /// Выброшенное заменяем пустой строкой, а не удаляем: номер строки в
    /// жалобе сторожа должен вести в настоящее место файла, иначе он отправит
    /// человека гадать — ровно то, против чего вся работа.
    ///
    /// Не нашлась закрывающая скобка — паника, а не молчаливое проглатывание
    /// остатка файла: проглоченное зеленит сторожа, а сторож, зелёный от
    /// слепоты, хуже отсутствующего.
    fn without_tests(path: &str, src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut lines = src.lines();
        let mut at = 0;
        while let Some(line) = lines.next() {
            at += 1;
            if !line.trim_start().starts_with("#[cfg(test)]") {
                out.push_str(line);
                out.push('\n');
                continue;
            }
            out.push('\n');
            let Some(first) = lines.next() else { break };
            at += 1;
            out.push('\n');
            if !first.contains('{') && first.trim_end().ends_with(';') {
                continue;
            }
            let opened = at;
            let mut closed = false;
            for l in lines.by_ref() {
                at += 1;
                out.push('\n');
                if l == "}" {
                    closed = true;
                    break;
                }
            }
            assert!(closed, "{path}: блок под `#[cfg(test)]` со строки {opened} не закрылся `}}` в нулевой колонке — сторож не знает, где кончается тест");
        }
        out
    }

    /// Текст файла, в котором остался только код: ни тестов, ни комментариев.
    /// `path` нужен лишь затем, чтобы жалоба называла файл.
    fn code_only(path: &str, src: &str) -> String {
        without_comments(&without_tests(path, src))
    }

    /// Выбросить строчные комментарии. Без этого сторож ловил бы прозу:
    /// «как `anyhow::bail!`, но с номером» — слова о механизме, а не отказ. И
    /// первым пострадал бы собственный дом механизма, где про `bail!` и надо
    /// рассказывать. Строки с кавычками учитываем, чтобы `"http://…"` не
    /// съел остаток строки.
    fn without_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        for line in src.lines() {
            let mut in_str = false;
            let mut escaped = false;
            let mut prev_slash = false;
            for ch in line.chars() {
                if in_str {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        in_str = false;
                    }
                    out.push(ch);
                    continue;
                }
                if ch == '/' && prev_slash {
                    out.pop();
                    break;
                }
                prev_slash = ch == '/';
                if ch == '"' {
                    in_str = true;
                }
                out.push(ch);
            }
            out.push('\n');
        }
        out
    }

    /// Номера, выданные в этом тексте, и жалобы на то, чего сторож не разобрал.
    /// Номер — безымянный числовой литерал среди аргументов вызова: `say(c.code,
    /// …)` и объявление `fail_code(code: u16, …)` номера не выдают.
    ///
    /// Жалоба, а не пропуск, — потому что молча-зелёный сторож хуже отсутствия:
    /// выпавший из реестра номер не проверяется на столкновение, а «ровно
    /// четыре цифры» — первое ограничение замысла, и кроме как здесь его никто
    /// не проверяет.
    fn codes_in(src: &str) -> (Vec<u16>, Vec<String>) {
        let mut out = Vec::new();
        let mut gripes = Vec::new();
        for call in CALLS {
            let mut from = 0;
            while let Some(at) = src[from..].find(call) {
                let start = from + at;
                let open = start + call.len();
                from = open;
                let line = src[..start].matches('\n').count() + 1;
                let Some(args) = args_at(src, open) else {
                    gripes.push(format!("строка {line}: `{call}` — аргументы не разобрались, номер не попал в реестр"));
                    continue;
                };
                for arg in args {
                    if arg.is_empty() || !arg.bytes().all(|b| b.is_ascii_digit()) {
                        continue;
                    }
                    match arg.parse::<u16>() {
                        Ok(n) if arg.len() == 4 => out.push(n),
                        _ => gripes.push(format!("строка {line}: `{call}` с номером {arg} — номер это ровно четыре цифры")),
                    }
                }
            }
        }
        (out, gripes)
    }

    /// Длина символьного литерала, начинающегося здесь, или `None`, если это
    /// время жизни (`'a`). Без различения `'"'` открывал бы строку, разбор
    /// срывался бы до конца файла, и номер выпадал из реестра — а таких
    /// аргументов в крейте полно (`s.trim_matches('"')`).
    fn char_lit(cs: &[char], i: usize) -> Option<usize> {
        if cs.get(i) != Some(&'\'') {
            return None;
        }
        if cs.get(i + 1) == Some(&'\\') {
            // Экранированный: `'\n'`, `'\\'`, `'\u{41}'` — закрывающая кавычка
            // недалеко, дальше девяти знаков символьных литералов не бывает.
            return (i + 2..=i + 9).find(|&j| cs.get(j) == Some(&'\'')).map(|j| j - i + 1);
        }
        if cs.get(i + 2) == Some(&'\'') {
            Some(3)
        } else {
            None
        }
    }

    /// Аргументы вызова, уже разделённые по запятым верхнего уровня. Вложенные
    /// скобки, строковые и символьные литералы разделителями не считаем: иначе
    /// `format!("а, б")` развалил бы список.
    ///
    /// `None` значит «не разобрал»: вызов не закрылся или строка не закрыта.
    /// Отдельный ответ нужен затем, чтобы `codes_in` пожаловался — тихо
    /// выпавший из реестра номер это несработавшая проверка столкновений, то
    /// есть дыра ровно там, где её не видно.
    fn args_at(src: &str, open: usize) -> Option<Vec<String>> {
        let cs: Vec<char> = src[open..].chars().collect();
        let mut args = vec![String::new()];
        let mut depth = 0i32;
        let mut i = 0;
        while let Some(&ch) = cs.get(i) {
            let arg = args.last_mut().expect("список аргументов не пуст");
            match ch {
                '"' => {
                    // Строковый литерал целиком: запятые и скобки внутри — текст.
                    let mut j = i + 1;
                    let end = loop {
                        match cs.get(j) {
                            None => return None, // строка не закрыта
                            Some('\\') => j += 2,
                            Some('"') => break j,
                            Some(_) => j += 1,
                        }
                    };
                    arg.extend(&cs[i..=end]);
                    i = end + 1;
                    continue;
                }
                '\'' if char_lit(&cs, i).is_some() => {
                    let n = char_lit(&cs, i).expect("только что проверили");
                    arg.extend(&cs[i..i + n]);
                    i += n;
                    continue;
                }
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => return Some(args.iter().map(|a| a.trim().to_string()).collect()),
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    args.push(String::new());
                    i += 1;
                    continue;
                }
                // Вызов часто разбит на строки (`Check::fail(` с аргументами
                // столбиком): перевод строки не конец вызова, а просто пробел.
                '\n' => {
                    arg.push(' ');
                    i += 1;
                    continue;
                }
                _ => {}
            }
            args.last_mut().expect("список аргументов не пуст").push(ch);
            i += 1;
        }
        None
    }

    /// Голые отказы этого текста. `code_bail!`/`code_ensure!` содержат `bail!`
    /// и `ensure!` как часть имени — отличаем по приставке `code_`, а не по
    /// соседнему символу: `anyhow::bail!` обязан ловиться.
    fn bare_refusals_in(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        for bare in BARE {
            let mut from = 0;
            while let Some(at) = src[from..].find(bare) {
                let start = from + at;
                from = start + bare.len();
                if src[..start].ends_with("code_") {
                    continue;
                }
                // По переводам строки, а не по `lines().count()`: тот теряет
                // единицу, когда отказ начинается с самой колонки, и сторож
                // указывал бы на строку выше.
                let line = src[..start].matches('\n').count() + 1;
                out.push(format!("{bare} в строке {line}"));
            }
        }
        out
    }

    #[test]
    fn the_numbers_do_not_collide_and_no_refusal_goes_without_one() {
        let root = workshop();
        let mut owner: BTreeMap<u16, Vec<String>> = BTreeMap::new();
        let mut bare: Vec<String> = Vec::new();
        let mut unparsed: Vec<String> = Vec::new();

        for (path, text) in sources(&root) {
            // Отдельные файлы тестов (`gui/log/tests.rs`) — целиком не наше
            // дело: номера в них выдуманные, отказы — подложные.
            if path.ends_with("/tests.rs") {
                continue;
            }
            let code = code_only(&path, &text);
            let (codes, gripes) = codes_in(&code);
            for n in codes {
                owner.entry(n).or_default().push(path.clone());
            }
            unparsed.extend(gripes.into_iter().map(|g| format!("{path}: {g}")));
            if UNCOVERED.iter().any(|u| path.starts_with(u)) {
                continue;
            }
            for found in bare_refusals_in(&code) {
                bare.push(format!("{path}: {found}"));
            }
        }

        // Сперва неразобранное: пока сторож чего-то не понял, его молчание об
        // остальном ничего не значит.
        assert!(unparsed.is_empty(), "сторож не разобрал номера — он не вправе быть зелёным:\n{}", unparsed.join("\n"));

        let twice: Vec<String> =
            owner.iter().filter(|(_, w)| w.len() > 1).map(|(n, w)| format!("{n} выдан {} раз: {}", w.len(), w.join(", "))).collect();
        assert!(twice.is_empty(), "один номер — одно место:\n{}", twice.join("\n"));

        let retired: Vec<String> = owner
            .keys()
            .filter_map(|n| RETIRED.iter().find(|(r, _)| r == n).map(|(_, was)| format!("{n} на пенсии ({was}), соседу не достаётся")))
            .collect();
        assert!(retired.is_empty(), "{}", retired.join("\n"));

        let stray: Vec<String> = owner
            .keys()
            .filter(|n| !AREAS.iter().any(|(a, _)| *a == **n / 100))
            .map(|n| format!("{n}: области {} нет в AREAS", n / 100))
            .collect();
        assert!(stray.is_empty(), "{}", stray.join("\n"));

        assert!(bare.is_empty(), "отказ без номера (или впиши номер, или назови файл в UNCOVERED):\n{}", bare.join("\n"));
    }

    #[test]
    fn the_guard_sees_what_it_looks_for() {
        // Сторож над пустым множеством зелен и слеп, а настоящих номеров в
        // мастерской пока ноль. Этот тест ловит именно слепоту.
        //
        // Привязки к имени размечаемого файла здесь нет нарочно: задачи 3…7
        // разметят его, утверждение покраснело бы с обманчивым текстом, и
        // очевидной «починкой» было бы выкинуть единственную живую проверку.
        // Вместо имени — инвариант обхода, который стереть нельзя.
        let root = workshop();
        let seen = sources(&root);
        assert!(seen.len() >= 30, "обход мастерской нашёл {} исходников — столько мало, обход сломан", seen.len());
        for u in UNCOVERED {
            assert!(root.join(u).exists(), "в UNCOVERED есть `{u}`, а на диске его нет: список разошёлся с мастерской");
        }

        // Проза о механизме отказом не считается, а код — считается.
        assert!(bare_refusals_in(&code_only("образец", "/// как `anyhow::bail!`, но с номером")).is_empty());
        assert_eq!(bare_refusals_in("anyhow::bail!(\"x\")").len(), 1);
        assert!(bare_refusals_in("code_bail!(1204, \"x\")").is_empty());

        // Номер строки ведёт в настоящее место файла и после выброшенного
        // теста: иначе жалоба сторожа отправляет человека гадать.
        let file = "fn a() {}\n#[cfg(test)]\nmod t {\n    fn q() {}\n}\nfn b() -> R {\nbail!(\"x\")\n}\n";
        assert_eq!(bare_refusals_in(&code_only("образец", file)), vec!["bail! в строке 7".to_string()]);

        // Условие с четырёхзначным числом (1541) номером не считается: оно не
        // стоит отдельным аргументом.
        let sample = "код: say(1204, format!(\"а, б\")) и code_ensure!(port == 1541, 1512, \"занято\") и fail_code(1501, \"база\")";
        assert_eq!(codes_in(sample), (vec![1204, 1512, 1501], vec![]));

        // Символьный литерал в аргументах разбор не срывает: `'"'` когда-то
        // открывал строку, и номер выпадал из реестра молча.
        let quoted = "code_bail!(1204, \"нет {}\", s.trim_matches('\"'))";
        assert_eq!(codes_in(quoted), (vec![1204], vec![]));

        // А если разбор всё же срывается (вызов не закрыт) — жалоба, не пропуск.
        let torn = "code_bail!(1204, \"нет\n";
        // `is_empty`, а не сравнение с `vec![]`: под фичей `gui` в дереве
        // сборки появляются `http` и `serde_json` со своими `PartialEq` для
        // `u16`, и тип пустого литерала перестаёт выводиться.
        assert!(codes_in(torn).0.is_empty(), "{:?}", codes_in(torn).0);
        assert_eq!(codes_in(torn).1.len(), 1, "{:?}", codes_in(torn).1);

        // Номер не из четырёх цифр не проходит молча: «ровно четыре цифры» —
        // первое ограничение замысла, и кроме как здесь его никто не проверяет.
        assert!(codes_in("say(140, \"мало\")").0.is_empty());
        assert_eq!(codes_in("say(140, \"мало\")").1.len(), 1);
        assert_eq!(codes_in("fail_code(12345, \"много\")").1.len(), 1);
    }

    #[test]
    #[should_panic(expected = "не закрылся")]
    fn an_unclosed_test_block_is_red_not_swallowed() {
        // Проглоченный остаток файла зеленит сторожа. Зелёный от слепоты он
        // хуже отсутствующего, поэтому здесь паника, а не тишина.
        code_only("образец", "#[cfg(test)]\nmod t {\n    fn q() {}\nbail!(\"x\")\n");
    }
}
