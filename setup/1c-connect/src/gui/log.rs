//! Журнал: строки, их вид и запись на диск.
//!
//! `note_line` — единственный путь строки в журнал окна, `note_file` — на
//! диск. Здесь же форматирование того, что человек читает.

use super::*;

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

#[derive(Clone, PartialEq, Debug)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}

/// Отметка дня и часа у строки журнала: не поле самой `Line` — её строят
/// литералом `Line { kind, text }` в полутора сотнях мест по всему `gui/`
/// (карточки, сохранение, трей, окно), и часы там ни при чём, — а обёртка
/// (`Stamped`) в единственном месте, где строка входит в журнал окна
/// (`note_line_counting`). Неделя считается с понедельника (0) — обычный
/// порядок для лестницы `group_label` и для «в понедельник» … «в
/// воскресенье».
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
    /// Время строки для показа — тем же `HH:MM`, каким его уже печатает
    /// `clock()` в заголовке операции: двух разных начертаний часа на одном
    /// экране быть не должно.
    pub fn hhmm(self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

/// Строка журнала вместе с отметкой, когда она пришла. Это и есть элемент
/// `Ui::log`; `Deref` до `Line` оставляет без изменений всё, что уже читает
/// `l.kind`/`l.text` у строк журнала по всему коду и тестам.
#[derive(Clone, PartialEq, Debug)]
pub struct Stamped {
    pub line: Line,
    pub stamp: Stamp,
}

impl std::ops::Deref for Stamped {
    type Target = Line;
    fn deref(&self) -> &Line {
        &self.line
    }
}

pub const LOG_CAP: usize = 2000;

/// Файл журнала рядом с комплектом: та же дозапись, что командная строка
/// ведёт в `setup.log` (`Kit::journal`), но для окна и своим текстом.
pub const LOG_FILE: &str = "1c-connect.log";

/// Местное время `HH:MM` для заголовка раздела. `gui` собирается под
/// Windows (feature `gui` на Linux в CI отключена, см. `Cargo.toml`), но
/// модуль всё равно огорожен по примеру `fatal` ниже, а не завязан на это.
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

/// Местная дата и час:минута для отметки строки журнала — тем же приёмом,
/// что `clock()` выше (тот же `GetLocalTime`, тот же запасной путь вне
/// Windows), с числом, месяцем, годом и днём недели вдобавок: без них
/// лестницу подписи дня (`group_label`) не свести. Нового зависимого пакета
/// для времени не заводим (решение владельца 9.11) — `chrono` и подобные
/// здесь не нужны.
#[cfg(windows)]
pub fn day_stamp() -> Stamp {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = SYSTEMTIME { wYear: 0, wMonth: 0, wDayOfWeek: 0, wDay: 0, wHour: 0, wMinute: 0, wSecond: 0, wMilliseconds: 0 };
    unsafe { GetLocalTime(&mut t) };
    // `wDayOfWeek`: 0 = воскресенье … 6 = суббота. У нас неделя с
    // понедельника (0) — см. докстроку `Stamp`.
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
        // Эпоха (1970-01-01) — четверг, то есть индекс 3 при неделе с
        // понедельника (0); отсюда и смещение `+3` ниже.
        weekday: (((days + 3) % 7 + 7) % 7) as u32,
        hour: ((secs / 3600) % 24) as u32,
        minute: ((secs / 60) % 60) as u32,
    }
}

/// Число дней от 1970-01-01 (алгоритм Хауарда Хиннанта: `days_from_civil`).
/// Новый зависимый пакет (`chrono`/`time`) под разницу двух дат не заводим —
/// это десяток строк известной арифметики, без нового кода для часовых
/// поясов и календарей, которые здесь и не нужны.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Обратное действие — нужно только запасному пути `day_stamp` вне Windows:
/// там нет `GetLocalTime`, который отдаёт дату сам, только секунды с эпохи.
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

/// Названия дней недели для лестницы (владелец 9.11): «в понедельник», «во
/// вторник», «в среду», «в четверг», «в пятницу», «в субботу», «в
/// воскресенье» — с большой, потому что подпись стоит сама по себе, не
/// серединой фразы. Индекс — `Stamp::weekday` (понедельник = 0).
const WEEKDAY_NAMES: [&str; 7] = ["В понедельник", "Во вторник", "В среду", "В четверг", "В пятницу", "В субботу", "В воскресенье"];

/// Месяцы в родительном падеже («3 ноября») для даты старше этой недели.
/// Индекс — месяц минус один (`Stamp::month` от 1 до 12).
const MONTHS_GENITIVE: [&str; 12] =
    ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];

/// Подпись дня по лестнице, которую ведущий предложил и владелец согласовал
/// 9.11: «Сегодня» → «Вчера» → название дня недели для остальных дней ЭТОЙ
/// недели («В среду») → дата для всего, что старше («3 ноября»). Чистая
/// функция от двух дат, без обращения к живым часам: отрисовку GPUI тестом
/// не взять, а эту лестницу можно и нужно проверить без живого окна.
pub fn group_label(day: Stamp, today: Stamp) -> String {
    let diff = days_from_civil(today.year, today.month, today.day) - days_from_civil(day.year, day.month, day.day);
    match diff {
        0 => "Сегодня".to_string(),
        1 => "Вчера".to_string(),
        // `today.weekday` — число дней от понедельника ЭТОЙ недели до
        // сегодня; день попадает в эту неделю, пока он не дальше понедельника.
        d if d >= 2 && d <= today.weekday as i64 => WEEKDAY_NAMES[day.weekday as usize].to_string(),
        _ => format!("{} {}", day.day, MONTHS_GENITIVE[day.month.saturating_sub(1).min(11) as usize]),
    }
}

/// Элемент готовой ленты журнала — то же разделение решения от отрисовки,
/// что `panel::Body` у вкладки «Связь»: что показать решает чистая функция
/// (`entries`), рисует уже готовый список, своего не решая.
#[derive(Clone, PartialEq, Debug)]
pub enum LogEntry {
    /// Подпись дня (`group_label`) по центру, между строк.
    Caption(String),
    /// Строка журнала ВМЕСТЕ с её отметкой времени (`Stamped`, не `Line`) и
    /// сколько раз подряд она повторилась (1 — без повтора, счётчик в хвосте
    /// строки тогда не рисуется).
    ///
    /// Отметку запись несёт затем, что иначе собранное время никуда не
    /// доходит: `Stamp::hour`/`minute` заполнялись с 9.11, а читателя у них
    /// не было ни одного, и за день работы все строки стояли под одной
    /// подписью «Сегодня», ничем не отличаясь друг от друга (находка ревью
    /// всей ветки). `Stamped`, а не третье поле кортежа: обёртка уже есть,
    /// `Deref` до `Line` оставляет без изменений всё, что читает `l.kind`
    /// и `l.text` у записи.
    Row(Stamped, usize),
}

/// Лента журнала целиком, готовая к отрисовке: повторы подряд (один вид,
/// один текст) сложены в одну запись со счётчиком — разделённые чужой
/// строкой НЕ складываются, — и перед первой строкой каждого дня (включая
/// самый первый) вставлена его подпись. Чистая функция от списка и
/// сегодняшней даты: это и единственное, что можно проверить тестом —
/// отрисовку GPUI тестом не взять.
///
/// У склеенного повтора остаётся отметка ПЕРВОГО вхождения: запись стоит в
/// ленте на месте первой строки, и время рядом с ней обязано отвечать на
/// «когда это началось», а не называть час, в который запись стоять не
/// может.
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

/// Отказ операции, меняющей машину, по истёкшему сроку: `Some` — начинать
/// нельзя, строка уходит в журнал. Кнопки окна ведут к тем же четырём
/// операциям, что `install`/`uninstall`/`export`/`import` в командной строке
/// (`Which` — ровно они), и ворота у них должны быть одни: окно и есть
/// главный путь человека к ним, запрет только в argv ничего не запрещает.
/// Пока срок идёт — даже в последний его день — операции работают как обычно.
pub fn op_refusal(state: crate::licence::State) -> Option<Line> {
    matches!(state, crate::licence::State::Expired).then(|| Line { kind: Kind::Fail, text: crate::licence::REFUSAL.to_string() })
}

/// Строка журнала о сроке заглушки лицензии: вышел — отказ его текстом,
/// скоро выйдет — сколько осталось, иначе молчим. Молча умирающая программа —
/// худшее, что можно сделать (спека, п. 9), поэтому окно открывается и
/// объясняет. Отдельная чистая функция — чтобы решение «что сказать» читалось
/// без живого окна.
pub fn licence_line(state: crate::licence::State) -> Option<Line> {
    use crate::licence::State;
    match state {
        State::Expired => op_refusal(state),
        State::Soon { days } => Some(Line {
            kind: Kind::Warn,
            text: format!("срок действия 1c-connect — до {}, осталось дней: {days}", crate::licence::until_text()),
        }),
        State::Valid => None,
    }
}

/// Сказать о сроке, если он сменился с того, о чём уже сказано. Зовут и при
/// запуске окна (запомнено `Valid`, и `Soon`/`Expired` сразу ложатся строкой),
/// и на каждом тике: окно может простоять открытым через полночь срока, и
/// тогда оно молча расходилось бы с мостом, который уже отказывает.
pub fn note_licence_change(cx: &mut App) {
    note_licence_change_ui(cx.global_mut::<Ui>());
}

/// То же без `cx`: смена срока — такое же событие, меняющее почву, как
/// «Сохранить» или конец операции, и кончается тем же пересчётом.
pub fn note_licence_change_ui(ui: &mut Ui) {
    let state = crate::licence::state_now();
    if ui.licence == state {
        return;
    }
    ui.licence = state;
    if let Some(line) = licence_line(state) {
        note_line(ui, line);
    }
    // Строки мало (находка S4): истёкший срок мост уже знает — он спрашивает
    // часы на каждый вызов, — а окно смотрело в запомненное и показывало
    // зелёное «MCP включены», пока агенту отказывали. Пересчёт запирает
    // ворота (`cold_gate` срок знает первым), гасит обслуживание и двигает
    // значок — как любое другое событие.
    run_readiness(ui);
}

/// Технические имена, которые пишутся строчными сами по себе: поднять в них
/// первую букву — соврать про имя команды, каталога или программы. Список
/// наш и короткий; проверяется по ПЕРВОМУ СЛОВУ целиком, а не по началу
/// строки, иначе «гитом» и «питон» попали бы под `git` и `pi`.
const LOWER_NAMES: [&str; 7] = ["mcp-1c", "vrunner", "vrunner-mcp", "oscript", "git", "httpd", "pi"];

/// Начало фразы — с заглавной, но только при ПОКАЗЕ. Имена фактов
/// (`verify::TOOLS` и прочие константы) править нельзя: они идут и
/// заголовком карточки, и серединой составленной фразы («не всё готово:
/// порт веб-сервера: …»), где заглавная была бы ошибкой.
///
/// Владелец 31.10 прислал снимок, где рядом стоят карточки «репозиторий» и
/// «Не зафиксировано»: орфография обязана быть одинаковой, «с большой
/// наверно правильнее будет».
///
/// Чего не поднимаем:
/// * первое слово — техническое имя из `LOWER_NAMES`;
/// * первое слово похоже на путь, файл, версию, отпечаток или команду — в
///   нём есть `\`, `/`, `:`, `.` или цифра. Цифра ВНУТРИ слова, а не только
///   в начале (поправка к брифу): подробность карточки «Последний коммит»
///   начинается с отпечатка (`a1b2c3 выгрузка (2 часа назад)`), и «A1b2c3»
///   было бы выдумкой про имя коммита. Слов русского языка с цифрой внутри
///   у нас нет, а версии, идентификаторы и отпечатки — сплошь такие;
/// * первая буква уже заглавная (или это вовсе не буква — значок, тире).
pub fn capitalized(s: &str) -> String {
    // Знак в хвосте слова — не часть имени: «план: …» — обычное начало
    // фразы, а не путь, и оставлять его строчным было бы ровно той же
    // разнобойностью, из-за которой правка и затеяна. Внутренние `:` и `.`
    // (`C:\…`, `setup.log`, `2.2.0`) обрезка не трогает.
    let word = s.split_whitespace().next().unwrap_or("").trim_end_matches([':', '.', ',', ';']);
    let techy = LOWER_NAMES.contains(&word) || word.contains(|c: char| c.is_numeric() || matches!(c, '\\' | '/' | ':' | '.'));
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if !techy && c.is_lowercase() => c.to_uppercase().collect::<String>() + cs.as_str(),
        _ => s.to_string(),
    }
}

/// Заголовок раздела журнала: название операции и час её начала.
pub fn head_line(title: &str) -> Line {
    Line { kind: Kind::Head, text: format!("{title} · {}", clock()) }
}

/// Заголовок раздела на экране, разобранный на две половины: название
/// операции слева (полужирным) и час справа (мелким приглушённым, заказчик
/// 2026-10-18). Час живёт ВНУТРИ текста (`head_line` его туда и кладёт),
/// поэтому разбор — по последнему « · »: само название операции точку-разделитель
/// содержать может, час — нет.
///
/// Счётчик повтора приписывается к НАЗВАНИЮ, а не к часу: справа стоит
/// время, и «×2» рядом с ним читалось бы как часть часа. Без него (Minor 2
/// ревью всей ветки) отрисовка `count` у заголовков не читала вовсе, и два
/// одинаковых заголовка подряд склеивались в один молча — одно событие
/// просто исчезало с экрана.
///
/// Чистая функция затем, что это и есть всё решение этой ветви отрисовки:
/// саму отрисовку GPUI тестом не взять, а разбор и счётчик — можно.
pub fn head_parts(text: &str, count: usize) -> (String, String) {
    let (title, time) = text.rsplit_once(" · ").map(|(t, c)| (t.to_string(), c.to_string())).unwrap_or((text.to_string(), String::new()));
    let title = if count > 1 { format!("{title} ×{count}") } else { title };
    (title, time)
}

/// Сколько шла операция, для «Готово за …»: меньше минуты — секунды,
/// иначе минуты и секунды. Секунды целые, без округления вверх.
pub fn took(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs} с")
    } else {
        format!("{} мин {} с", secs / 60, secs % 60)
    }
}

/// Прошло с начала операции, для карточки хода, в виде часов: `m:ss`
/// («12:05»), а от часа и больше — `h:mm:ss» («1:02:05»). Минуты и секунды
/// всегда с ведущим нулём, кроме самих часов. Не `took`: та словами
/// («12 мин 05 с»), эта — цифрами, чтобы заголовок карточки не растягивался.
pub fn running_clock(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Заголовок карточки хода (название фазы) и часы — сколько она уже идёт
/// (`m:ss`/`h:mm:ss`, см. `running_clock`).
pub fn progress_text(r: &Running, now: Instant) -> (String, String) {
    (r.phase.title().to_string(), running_clock(now.saturating_duration_since(r.started)))
}

/// Пять минут без вестей от потока — поток молчит, но операция ещё жива.
pub const QUIET: Duration = Duration::from_secs(5 * 60);

pub fn is_stale(r: &Running, now: Instant) -> bool {
    now.saturating_duration_since(r.seen) > QUIET
}

pub fn stale_text(r: &Running, now: Instant) -> String {
    format!("нет новостей {} мин", now.saturating_duration_since(r.seen).as_secs() / 60)
}

/// Застой уходит из карточки (заказчик 2026-10-19) в журнал — одной строкой
/// `Kind::Warn`, в момент, когда операция впервые застряла, не чаще раза на
/// операцию: взводит `stale_noted`, второй вызов при том же застое отдаёт
/// `None`.
pub fn stale_line(r: &mut Running, now: Instant) -> Option<Line> {
    if is_stale(r, now) && !r.stale_noted {
        r.stale_noted = true;
        Some(Line { kind: Kind::Warn, text: stale_text(r, now) })
    } else {
        None
    }
}

/// Строка журнала на диск, тем же текстом, что рисует окно, но без цвета:
/// значок и пробел перед строкой (у `Say` — два пробела вместо значка),
/// у заголовка раздела — пустая строка перед ним. Ошибки записи (диск занят,
/// прав нет) молча игнорируются — журнал на диске не обязателен для работы.
pub fn write_log_file(kit: &Kit, line: &Line) {
    use std::io::Write;
    let path = kit.state_path(LOG_FILE);
    // Каталог состояния создаёт `ops_host::ensure_state` на запуске; здесь
    // на всякий случай ещё раз — журнал открывается напрямую, без
    // `kit::write_text`, который родителя создаёт сам.
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&path) else { return };
    let text = if line.kind == Kind::Head { format!("\n{}\n", line.text) } else { format!("{:<2}{}\n", line.kind.mark(), line.text) };
    let _ = f.write_all(text.as_bytes());
}

/// `Command`/`Output` — в журнал команд на диске (см. `ctx_for`), не в окно.
pub fn line_of(e: &Event) -> Option<Line> {
    let (kind, text) = match e {
        Event::Ok(t) => (Kind::Ok, t.clone()),
        Event::Warn(t) => (Kind::Warn, t.clone()),
        Event::Fail(t) => (Kind::Fail, t.clone()),
        Event::Say(t) => (Kind::Say, t.clone()),
        Event::Progress(p) => (Kind::Say, p.phase.title().into()),
        Event::Command(_) | Event::Output { .. } => return None,
    };
    Some(Line { kind, text })
}

/// Общее тело `note_line`/`note_line_seen`: запись на диск, в список окна и
/// потолок `LOG_CAP` — одно и то же в обоих случаях, расходится только то,
/// растут ли непрочитанные.
fn note_line_counting(ui: &mut Ui, mut line: Line, counts_as_unread: bool) {
    line.text = capitalized(&line.text);
    write_log_file(&ui.kit, &line);
    let stamp = day_stamp();
    // Склеится ли строка с предыдущей — считаем ДО записи, тем же правилом,
    // каким ленту собирает `entries`: один вид, один текст и один день
    // (подпись дня, что встаёт на смене суток, склейку разрывает). Склеенный
    // повтор непрочитанного не прибавляет — иначе бейдж обещает пять новых
    // сообщений, а лента показывает одну строку «×5», то есть экран
    // расходится сам с собой (Important 5 ревью всей ветки, решение
    // ведущего: счёт по ЗАПИСЯМ, а не по строкам).
    let glued = ui.log.last().is_some_and(|p| {
        p.line.kind == line.kind
            && p.line.text == line.text
            && (p.stamp.year, p.stamp.month, p.stamp.day) == (stamp.year, stamp.month, stamp.day)
    });
    ui.log.push(Stamped { stamp, line });
    if ui.log.len() > LOG_CAP {
        let extra = ui.log.len() - LOG_CAP;
        ui.log.drain(0..extra);
    }
    if counts_as_unread && !glued && (ui.tab != Tab::Log || ui.window.is_none()) {
        ui.unread += 1;
    }
}

/// Строка в журнал, с потолком в `LOG_CAP`. Непрочитанные растут, если
/// журнал сейчас не на экране — вкладка другая или окно спрятано, — и
/// ровно на одну ЗАПИСЬ ленты: повтор, который склеится в «×N», второй
/// цифры бейджу не добавляет (см. `note_line_counting`).
///
/// Заглавная (`capitalized`) ставится ЗДЕСЬ, до записи на диск: это
/// единственная дорога строки в журнал окна, и так окно с файлом
/// `1c-connect.log` говорят одним текстом.
pub fn note_line(ui: &mut Ui, line: Line) {
    note_line_counting(ui, line, true);
}

/// То же самое, но без прибавки к непрочитанному: для строки, что повторяет
/// состояние, которое человек и так видит в другом месте экрана — замок
/// запуска (серая точка, запертый переключатель), — а значит не новость,
/// которую ему ещё предстоит заметить. Строка всё равно идёт на диск и во
/// вкладку «Журнал»: разбирать её по снимку или искать глазами там можно,
/// просто вкладка не кричит цифрой о том, что человек уже видит. Находка
/// владельца 9.11 на живом прогоне: свежий комплект показывал «Журнал 1»
/// раньше, чем человек сделал хоть что-то.
///
/// Сюда же — УДАЧА своего действия человека: «Сохранено в 1c-connect.toml»,
/// «Имя карточки сохранено». Он только что нажал «Сохранить» и в ту же
/// секунду видит ответ на экране — кнопка перестала предлагаться; бейдж
/// «Журнал 1» за это обесценивает сам бейдж, который обещает НОВОЕ
/// (решение ведущего по Important 5 ревью всей ветки). ОТКАЗЫ остаются
/// громкими (`note_line`): их человек по погасшей кнопке не прочтёт — она
/// не погасла, — и не заметить их нельзя.
pub fn note_line_seen(ui: &mut Ui, line: Line) {
    note_line_counting(ui, line, false);
}

/// Строка только в файл журнала (`1c-connect.log`), мимо окна: диагностика
/// для разбора по снимку, которую человеку читать незачем (геометрия, обмеры).
/// В журнале окна — только то, что человеку надо прочесть; пометка `Kind::Say`
/// сама по себе не повод показывать строку, выбирает канал тот, кто пишет.
pub fn note_file(kit: &Kit, line: &Line) {
    write_log_file(kit, line);
}

/// Строка про очередь к httpd, когда баз больше одной. Однополосный
/// веб-сервер упирается первым — раньше памяти и портов — и кончается
/// ожиданием. Без строки в журнале эту медлительность будут искать в агенте,
/// в сети и в чём угодно, кроме места, где она есть. Возвращает `None`, когда
/// база одна (очередь не образуется), и `Some` с сообщением про задержку, когда
/// соединений больше одного.
pub fn httpd_queue_note(cfg: &crate::cfg::Cfg) -> Option<String> {
    if cfg.connections.len() > 1 {
        Some(
            "Баз на этой машине больше одной: веб-сервер обслуживает их по очереди, \
             и ответы могут приходить медленнее."
                .to_string(),
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
