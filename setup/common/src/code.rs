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
        "1c-connect/src/gate.rs",
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
        "1c-connect/src/verify.rs",
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
    fn without_tests(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut lines = src.lines();
        while let Some(line) = lines.next() {
            if !line.trim_start().starts_with("#[cfg(test)]") {
                out.push_str(line);
                out.push('\n');
                continue;
            }
            out.push('\n');
            let Some(first) = lines.next() else { break };
            out.push('\n');
            if !first.contains('{') && first.trim_end().ends_with(';') {
                continue;
            }
            for l in lines.by_ref() {
                out.push('\n');
                if l == "}" {
                    break;
                }
            }
        }
        out
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

    /// Текст файла, в котором остался только код: ни тестов, ни комментариев.
    fn code_only(src: &str) -> String {
        without_comments(&without_tests(src))
    }

    /// Номера, выданные в этом тексте. Берём только безымянные
    /// четырёхзначные литералы среди аргументов вызова: `say(c.code, …)` и
    /// объявление `fail_code(code: u16, …)` номера не выдают.
    fn codes_in(src: &str) -> Vec<u16> {
        let mut out = Vec::new();
        for call in CALLS {
            let mut from = 0;
            while let Some(at) = src[from..].find(call) {
                let open = from + at + call.len();
                from = open;
                for arg in args_at(src, open) {
                    if arg.len() == 4 && arg.bytes().all(|b| b.is_ascii_digit()) {
                        if let Ok(n) = arg.parse::<u16>() {
                            out.push(n);
                        }
                    }
                }
            }
        }
        out
    }

    /// Аргументы вызова, уже разделённые по запятым верхнего уровня. Вложенные
    /// скобки и строковые литералы разделителями не считаем: иначе
    /// `format!("а, б")` развалил бы список.
    fn args_at(src: &str, open: usize) -> Vec<String> {
        let mut args = vec![String::new()];
        let mut depth = 0i32;
        let mut in_str = false;
        let mut escaped = false;
        for ch in src[open..].chars() {
            if in_str {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_str = false;
                }
                args.last_mut().expect("список аргументов не пуст").push(ch);
                continue;
            }
            match ch {
                '"' => in_str = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => return args.iter().map(|a| a.trim().to_string()).collect(),
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    args.push(String::new());
                    continue;
                }
                // Вызов часто разбит на строки (`Check::fail(` с аргументами
                // столбиком): перевод строки не конец вызова, а просто пробел.
                '\n' => {
                    args.last_mut().expect("список аргументов не пуст").push(' ');
                    continue;
                }
                _ => {}
            }
            args.last_mut().expect("список аргументов не пуст").push(ch);
        }
        Vec::new()
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

        for (path, text) in sources(&root) {
            // Отдельные файлы тестов (`gui/log/tests.rs`) — целиком не наше
            // дело: номера в них выдуманные, отказы — подложные.
            if path.ends_with("/tests.rs") {
                continue;
            }
            let code = code_only(&text);
            for n in codes_in(&code) {
                owner.entry(n).or_default().push(path.clone());
            }
            if UNCOVERED.iter().any(|u| path.starts_with(u)) {
                continue;
            }
            for found in bare_refusals_in(&code) {
                bare.push(format!("{path}: {found}"));
            }
        }

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
        // Сторож над пустым множеством зелен и слеп. Эти два утверждения
        // ловят именно слепоту: разбор номеров и поиск голых отказов должны
        // срабатывать на настоящих файлах мастерской, а не только в теории.
        let root = workshop();
        let ops = std::fs::read_to_string(root.join("1c-connect/src/ops_host.rs")).expect("ops_host.rs на месте");
        assert!(!bare_refusals_in(&code_only(&ops)).is_empty(), "в ops_host.rs голые отказы есть — значит, они и должны находиться");
        // Проза о механизме отказом не считается, а код — считается.
        assert!(bare_refusals_in(&code_only("/// как `anyhow::bail!`, но с номером")).is_empty());

        // Номер строки ведёт в настоящее место файла и после выброшенного
        // теста: иначе жалоба сторожа отправляет человека гадать.
        let file = "fn a() {}\n#[cfg(test)]\nmod t {\n    fn q() {}\n}\nfn b() -> R {\nbail!(\"x\")\n}\n";
        assert_eq!(bare_refusals_in(&code_only(file)), vec!["bail! в строке 7".to_string()]);

        let sample = "код: say(1204, format!(\"а, б\")) и code_ensure!(port == 1541, 1512, \"занято\") и fail_code(1501, \"база\")";
        assert_eq!(codes_in(sample), vec![1204, 1512, 1501]);

        // Условие с четырёхзначным числом (1541) номером не считается: оно не
        // стоит отдельным аргументом. А голый `anyhow::bail!` — считается.
        assert_eq!(bare_refusals_in("anyhow::bail!(\"x\")").len(), 1);
        assert!(bare_refusals_in("code_bail!(1204, \"x\")").is_empty());
    }
}
