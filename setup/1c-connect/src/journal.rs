//! Журнал: словарь строк, их вид и запись на диск — закон 2 спеки («книга, учёт разговоров и журнал — одни»). Файл один (`LOG_FILE`), и в него пишет и окно (через `gui::note_line`), и командная строка без окна (через `say`): второй таблицы значков или второго пути на диск этот модуль не заводит.
//!
//! Не за фичей `gui`: до задачи 12 плана «Сопряжение переезжает в бек» журнал писало только окно, и `serve`/отладочный режим командной строки не могли выполнить то, что обещают. `Kind` переехал сюда вместе с записью, потому что перевод разряда в значок (`Kind::mark`) — одна функция, и её зовут и файл, и экран (`gui::panel`); заводить в беке вторую меру значило бы разойтись с экраном.
//!
//! `Line` и перевод `common::Event → Line` (`gui::line_of`) остаются в окне: это решение, что показать на экране, а не что записать на диск, и второго перевода `Event` в строку заводить не стоит — `say` ниже делает тот же разбор для диска, что `line_of` для экрана.

use common::kit::Kit;

/// Разряд строки журнала — словарь ФАЙЛА, а не экрана: варианты называют форму строки на диске, а `mark()` отдаёт знак, который в этот файл и пишется. Экран берёт тот же разряд (реэкспорт `gui::log::Kind`) и красит строку по нему же — вторая мера на тот же вопрос здесь не нужна.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Ok,
    Warn,
    Fail,
    Say,
    /// Заголовок раздела журнала — название операции с часами в заголовке.
    Head,
}

impl Kind {
    /// Значок перед строкой журнала; у `Say` и `Head` строка идёт без значка.
    pub fn mark(self) -> &'static str {
        match self {
            Kind::Ok => "✓",
            Kind::Warn => "!",
            Kind::Fail => "✗",
            Kind::Say | Kind::Head => "",
        }
    }
}

/// Отметка дня и часа у строки журнала: не поле самой `Line` (та строится литералом `Line { kind, text }` в полутора сотнях мест по всему `gui/` — карточки, сохранение, трей, окно, — и часы там ни при чём), а обёртка (`Stamped`) в единственном месте, где строка входит в журнал окна (`gui::log::note_line_counting`). Неделя считается с понедельника (0) — обычный порядок для лестницы `group_label` и для «в понедельник» … «в воскресенье».
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
}

impl Stamp {
    /// Время строки для показа — тем же `HH:MM`, каким его уже печатает `clock()` в заголовке операции: двух разных начертаний часа на одном экране быть не должно.
    pub fn hhmm(self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

/// Строка журнала вместе с отметкой, когда она пришла. Это и есть элемент `gui::Ui::log`; `Deref` до `gui::Line` оставляет без изменений всё, что уже читает `l.kind`/`l.text` у строк журнала по всему коду и тестам.
///
/// За фичей `gui`, в отличие от остального модуля: держит поле типа `gui::Line`, а `Line` — вещь экрана (строится литералом в полутора сотнях мест по `gui/`) и без окна не существует вовсе. Без окна эта обёртка и не нужна — лента (`entries`/`LogEntry`) существует затем, чтобы её рисовать.
#[cfg(feature = "gui")]
#[derive(Clone, PartialEq, Debug)]
pub struct Stamped {
    pub line: crate::gui::Line,
    pub stamp: Stamp,
}

#[cfg(feature = "gui")]
impl std::ops::Deref for Stamped {
    type Target = crate::gui::Line;
    fn deref(&self) -> &crate::gui::Line {
        &self.line
    }
}

pub const LOG_CAP: usize = 2000;

/// Файл журнала рядом с комплектом: та же дозапись, что командная строка ведёт в `setup.log` (`Kit::journal`), но этим видом строк. Пишут сюда без различия окно и командная строка без окна — ровно закон 2 спеки.
pub const LOG_FILE: &str = "1c-connect.log";

/// Местное время `HH:MM` для заголовка раздела. Журнал не гейтится фичей `gui` (закон 2 спеки — он один и доступен из командной строки), и здесь единственный гейт — платформенный, тот же приём, что у `day_stamp` ниже.
#[cfg(windows)]
pub fn clock() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = SYSTEMTIME { wYear: 0, wMonth: 0, wDayOfWeek: 0, wDay: 0, wHour: 0, wMinute: 0, wSecond: 0, wMilliseconds: 0 };
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

#[cfg(not(windows))]
pub fn clock() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60)
}

/// Местная дата и час:минута для отметки строки журнала — тем же приёмом, что `clock()` выше (тот же `GetLocalTime`, тот же запасной путь вне Windows), с числом, месяцем, годом и днём недели вдобавок: без них лестницу подписи дня (`group_label`) не свести. Нового зависимого пакета для времени не заводим (решение владельца 9.11) — `chrono` и подобные здесь не нужны.
#[cfg(windows)]
pub fn day_stamp() -> Stamp {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = SYSTEMTIME { wYear: 0, wMonth: 0, wDayOfWeek: 0, wDay: 0, wHour: 0, wMinute: 0, wSecond: 0, wMilliseconds: 0 };
    unsafe { GetLocalTime(&mut t) };
    // `wDayOfWeek`: 0 = воскресенье … 6 = суббота. У нас неделя с понедельника (0) — см. докстроку `Stamp`.
    Stamp {
        year: t.wYear as i32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        weekday: (t.wDayOfWeek as u32 + 6) % 7,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
    }
}

#[cfg(not(windows))]
pub fn day_stamp() -> Stamp {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86400) as i64;
    let (year, month, day) = civil_from_days(days);
    Stamp {
        year,
        month,
        day,
        // Эпоха (1970-01-01) — четверг, то есть индекс 3 при неделе с понедельника (0); отсюда и смещение `+3` ниже.
        weekday: (((days + 3) % 7 + 7) % 7) as u32,
        hour: ((secs / 3600) % 24) as u32,
        minute: ((secs / 60) % 60) as u32,
    }
}

/// Число дней от 1970-01-01 (алгоритм Хауарда Хиннанта: `days_from_civil`). Новый зависимый пакет (`chrono`/`time`) под разницу двух дат не заводим — это десяток строк известной арифметики, без нового кода для часовых поясов и календарей, которые здесь и не нужны.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Обратное действие — нужно только запасному пути `day_stamp` вне Windows: там нет `GetLocalTime`, который отдаёт дату сам, только секунды с эпохи.
#[cfg(not(windows))]
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { (y + 1) as i32 } else { y as i32 }, m, d)
}

/// Названия дней недели для лестницы (владелец 9.11): «в понедельник», «во вторник», «в среду», «в четверг», «в пятницу», «в субботу», «в воскресенье» — с большой, потому что подпись стоит сама по себе, не серединой фразы. Индекс — `Stamp::weekday` (понедельник = 0).
const WEEKDAY_NAMES: [&str; 7] = ["В понедельник", "Во вторник", "В среду", "В четверг", "В пятницу", "В субботу", "В воскресенье"];

/// Месяцы в родительном падеже («3 ноября») для даты старше этой недели. Индекс — месяц минус один (`Stamp::month` от 1 до 12).
const MONTHS_GENITIVE: [&str; 12] =
    ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];

/// Подпись дня по лестнице, которую ведущий предложил и владелец согласовал 9.11: «Сегодня» → «Вчера» → название дня недели для остальных дней ЭТОЙ недели («В среду») → дата для всего, что старше («3 ноября»). Чистая функция от двух дат, без обращения к живым часам: отрисовку GPUI тестом не взять, а эту лестницу можно и нужно проверить без живого окна.
pub fn group_label(day: Stamp, today: Stamp) -> String {
    let diff = days_from_civil(today.year, today.month, today.day) - days_from_civil(day.year, day.month, day.day);
    match diff {
        0 => "Сегодня".to_string(),
        1 => "Вчера".to_string(),
        // `today.weekday` — число дней от понедельника ЭТОЙ недели до сегодня; день попадает в эту неделю, пока он не дальше понедельника.
        d if d >= 2 && d <= today.weekday as i64 => WEEKDAY_NAMES[day.weekday as usize].to_string(),
        _ => format!("{} {}", day.day, MONTHS_GENITIVE[day.month.saturating_sub(1).min(11) as usize]),
    }
}

/// Элемент готовой ленты журнала — то же разделение решения от отрисовки, что `panel::Body` у вкладки «Связь»: что показать решает чистая функция (`entries`), рисует уже готовый список, своего не решая. За фичей `gui` по той же причине, что `Stamped`: без окна ленту некому рисовать.
#[cfg(feature = "gui")]
#[derive(Clone, PartialEq, Debug)]
pub enum LogEntry {
    /// Подпись дня (`group_label`) по центру, между строк.
    Caption(String),
    /// Строка журнала ВМЕСТЕ с её отметкой времени (`Stamped`, не `Line`) и сколько раз подряд она повторилась (1 — без повтора, счётчик в хвосте строки тогда не рисуется).
    ///
    /// Отметку запись несёт затем, что иначе собранное время никуда не доходит: `Stamp::hour`/`minute` заполнялись с 9.11, а читателя у них не было ни одного, и за день работы все строки стояли под одной подписью «Сегодня», ничем не отличаясь друг от друга (находка ревью всей ветки). `Stamped`, а не третье поле кортежа: обёртка уже есть, `Deref` до `Line` оставляет без изменений всё, что читает `l.kind` и `l.text` у записи.
    Row(Stamped, usize),
}

/// Лента журнала целиком, готовая к отрисовке: повторы подряд (один вид, один текст) сложены в одну запись со счётчиком — разделённые чужой строкой НЕ складываются, — и перед первой строкой каждого дня (включая самый первый) вставлена его подпись. Чистая функция от списка и сегодняшней даты: это и единственное, что можно проверить тестом — отрисовку GPUI тестом не взять.
///
/// У склеенного повтора остаётся отметка ПЕРВОГО вхождения: запись стоит в ленте на месте первой строки, и время рядом с ней обязано отвечать на «когда это началось», а не называть час, в который запись стоять не может.
#[cfg(feature = "gui")]
pub fn entries(lines: &[Stamped], today: Stamp) -> Vec<LogEntry> {
    let mut out: Vec<LogEntry> = Vec::new();
    let mut last_day: Option<(i32, u32, u32)> = None;
    for s in lines {
        let day = (s.stamp.year, s.stamp.month, s.stamp.day);
        if last_day != Some(day) {
            out.push(LogEntry::Caption(group_label(s.stamp, today)));
            last_day = Some(day);
        }
        match out.last_mut() {
            Some(LogEntry::Row(prev, count)) if prev.kind == s.line.kind && prev.text == s.line.text => *count += 1,
            _ => out.push(LogEntry::Row(s.clone(), 1)),
        }
    }
    out
}

/// Строка журнала на диск: значок и пробел перед строкой, добитые до двух знаков (у `Say` — два пробела вместо значка), у заголовка раздела (`Kind::Head`) — пустая строка перед ним и без значка. Ошибки записи (диск занят, прав нет) молча игнорируются — журнал на диске не обязателен для работы. Единственный путь строки на диск — и для окна (`gui::note_line` зовёт отсюда), и для командной строки без него (`say` ниже).
pub fn write(kit: &Kit, kind: Kind, text: &str) {
    use std::io::Write;
    let path = kit.state_path(LOG_FILE);
    // Каталог состояния создаёт `ops_host::ensure_state` на запуске; здесь на всякий случай ещё раз — журнал открывается напрямую, без `kit::write_text`, который родителя создаёт сам.
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&path) else { return };
    let line = if kind == Kind::Head { format!("\n{text}\n") } else { format!("{:<2}{text}\n", kind.mark()) };
    let _ = f.write_all(line.as_bytes());
}

/// Удобство для бека, который держит в руках `common::Event`, а не `Kind`: переводит одно в другое и зовёт `write`. Разбор — тот же, что `gui::line_of` делает для экрана (второй мере на один вопрос здесь не бывать): `Command`/`Output` в журнал на диске не идут — это журнал команд (`Kit::journal`, `setup.log`), не этот файл, — а фаза долгой операции (`Progress`) идёт `Kind::Say` её названием.
pub fn say(kit: &Kit, e: &common::Event) {
    let (kind, text) = match e {
        common::Event::Ok(t) => (Kind::Ok, t.clone()),
        common::Event::Warn(t) => (Kind::Warn, t.clone()),
        common::Event::Fail(t) => (Kind::Fail, t.clone()),
        common::Event::Say(t) => (Kind::Say, t.clone()),
        common::Event::Progress(p) => (Kind::Say, p.phase.title().into()),
        common::Event::Command(_) | common::Event::Output { .. } => return,
    };
    write(kit, kind, &text);
}

/// Паника — в журнал на диске, а не в никуда.
///
/// Релиз окна собран `windows_subsystem = "windows"` (`main.rs`): консоли нет, и всё, что паника печатает в `stderr`, пропадает. Для человека это выглядит как «окно замерло и вылетело», а для нас — как пустое место там, где должна быть причина: обвал 12.11 на home-gpu пришлось доставать из системного журнала Windows по коду `c0000409`, и имени виноватой строки там нет вовсе.
///
/// Прежний обработчик зовётся следом, а не заменяется: он печатает то же самое в `stderr`, и под консолью (отладка, `--console`) человек по-прежнему видит привычное сообщение.
pub fn record_panics(kit: &Kit) {
    let path = kit.state_path(LOG_FILE);
    let was = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let trace = std::backtrace::Backtrace::force_capture();
        let text = format!("\n!! обвал {} — {info}\n{trace}\n", clock());
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&path) {
            let _ = f.write_all(text.as_bytes());
        }
        was(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::tempdir::Dir;

    /// Журнал один: строка, записанная беком без окна, ложится в тот же файл и тем же видом.
    #[test]
    fn the_journal_is_one_and_the_back_writes_to_it_without_a_window() {
        let dir = Dir::new();
        let kit = Kit::at(dir.path());
        say(&kit, &common::Event::Fail("не дозвонился".into()));
        write(&kit, Kind::Head, "Проверка");
        let text = std::fs::read_to_string(kit.state_path(LOG_FILE)).unwrap();
        // `say` — значок `Kind::Fail` и тот же текст; `write(Head, ..)` — прежняя форма заголовка: пустая строка, без значка.
        assert_eq!(text, "✗ не дозвонился\n\nПроверка\n");
    }
}
