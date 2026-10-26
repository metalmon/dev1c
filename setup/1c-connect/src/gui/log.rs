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
        State::Soon { days } => {
            Some(Line { kind: Kind::Warn, text: format!("срок действия 1c-connect — до {}, осталось дней: {days}", crate::licence::until_text()) })
        }
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

/// Заголовок раздела журнала: название операции и час её начала.
pub fn head_line(title: &str) -> Line {
    Line { kind: Kind::Head, text: format!("{title} · {}", clock()) }
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

/// Строка в журнал, с потолком в `LOG_CAP`. Непрочитанные растут, если
/// журнал сейчас не на экране — вкладка другая или окно спрятано.
pub fn note_line(ui: &mut Ui, line: Line) {
    write_log_file(&ui.kit, &line);
    ui.log.push(line);
    if ui.log.len() > LOG_CAP {
        let extra = ui.log.len() - LOG_CAP;
        ui.log.drain(0..extra);
    }
    if ui.tab != Tab::Log || ui.window.is_none() {
        ui.unread += 1;
    }
}

/// Строка только в файл журнала (`1c-connect.log`), мимо окна: диагностика
/// для разбора по снимку, которую человеку читать незачем (геометрия, обмеры).
/// В журнале окна — только то, что человеку надо прочесть; пометка `Kind::Say`
/// сама по себе не повод показывать строку, выбирает канал тот, кто пишет.
pub fn note_file(kit: &Kit, line: &Line) {
    write_log_file(kit, line);
}

/// Итог последнего действия в подвал — вместе с подсказкой к нему. Одной
/// функцией, чтобы подсказка прошлого итога не осталась висеть на следующем.
pub fn set_status(ui: &mut Ui, line: Line, tip: Option<String>) {
    ui.status = Some(line);
    ui.status_tip = tip;
    ui.status_at = Some(Instant::now());
}

#[cfg(test)]
mod tests;
