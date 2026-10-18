//! Решения окна без GPUI: что говорит карточка состояния, какая вкладка
//! открывается первой, что несёт плашка, что показывает «Доступ», есть ли
//! несохранённые правки, можно ли закрывать окно.
//!
//! Модуль собирается всегда, не только с флагом `gui`: GPUI на этой машине
//! не собирают (правило «сборка в CI»), а тесты логики должны идти локально.

use crate::cfg::{self, Cfg};
use common::check::Check;
use common::kit::Kit;
use common::op::OpState;
use common::Event;
use std::time::Duration;

/// Сколько строк журнала держим в окне.
pub const LOG_CAP: usize = 2000;
/// Журнал окна рядом с комплектом.
pub const LOG_FILE: &str = "dev1c-setup.log";
/// Текст в журнал, когда рабочий поток упал с паникой.
pub const PANIC_TEXT: &str = "внутренняя ошибка: операция прервана";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Ok,
    Warn,
    Fail,
    Say,
    /// Заголовок раздела журнала — название работы с часами.
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

impl Line {
    pub fn ok(text: impl Into<String>) -> Line {
        Line { kind: Kind::Ok, text: text.into() }
    }
    pub fn say(text: impl Into<String>) -> Line {
        Line { kind: Kind::Say, text: text.into() }
    }
    pub fn fail(text: impl Into<String>) -> Line {
        Line { kind: Kind::Fail, text: text.into() }
    }
}

/// Пять вкладок старого окна в том же порядке.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Ops,
    Params,
    Access,
    Check,
    Log,
}

impl Tab {
    /// Порядок вкладок в полосе; он же — порядок индексов `TabBar`.
    pub const ALL: [Tab; 5] = [Tab::Ops, Tab::Params, Tab::Access, Tab::Check, Tab::Log];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Ops => "Операции",
            Tab::Params => "Параметры",
            Tab::Access => "Доступ",
            Tab::Check => "Проверка",
            Tab::Log => "Журнал",
        }
    }
}

/// Что идёт прямо сейчас — от этого зависят кнопки и полоса хода.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Busy {
    Run,
    Check,
}

/// Работа, которую запускает кнопка.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Install,
    Change,
    CleanStack,
    CleanData,
    Check,
}

impl Which {
    /// Заголовок раздела в журнале перед работой.
    pub fn title(self) -> &'static str {
        match self {
            Which::Install => "— устанавливаю —",
            Which::Change => "— применяю изменения —",
            Which::CleanStack => "— снимаю контейнер —",
            Which::CleanData => "— удаляю данные —",
            Which::Check => "— проверяю —",
        }
    }
}

/// Кружок состояния: цвет говорит состояние, текст — подробность.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dot {
    /// Не сделано.
    Off,
    /// Сделано, но разошлось с параметрами.
    Busy,
    /// Сделано.
    Up,
    /// Заблокировано: сделать нельзя, пока не поправят.
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpView {
    pub id: String,
    pub title: String,
    pub dot: Dot,
    pub detail: String,
}

/// Карточка операции из её состояния.
pub fn op_view(id: String, title: String, st: OpState) -> OpView {
    let (dot, detail) = match st {
        OpState::Done(d) => (Dot::Up, d),
        OpState::Missing => (Dot::Off, "не сделано".into()),
        OpState::Drifted { from, to } => (Dot::Busy, format!("было: {from} → станет: {to}")),
        OpState::Blocked(why) => (Dot::Down, why),
    };
    OpView { id, title, dot, detail }
}

/// Параметры для окна: сломанный файл не повод не открыться — окно и есть
/// способ его поправить. Возвращает умолчания и причину, если файл не читается.
pub fn load_or_default(kit: &Kit) -> (Cfg, Option<String>) {
    match Cfg::load(kit) {
        Ok(cfg) => (cfg, None),
        Err(e) => (Cfg::default(), Some(format!("{e:#}"))),
    }
}

/// Какая вкладка открывается первой. Всегда «Операции»: незаполненное видно
/// там словами (операция отвечает `Blocked` с именем параметра). «Параметры» —
/// только если файла параметров нет вовсе, то есть его удалили: комплект
/// всегда едет с `dev1c.toml` (`pack.sh`).
pub fn start_tab(file_exists: bool) -> Tab {
    if file_exists {
        Tab::Ops
    } else {
        Tab::Params
    }
}

/// Карточка состояния в шапке: кружок, заголовок, подстрочник и главная
/// кнопка со своей надписью. Пусто в `primary` — делать нечего, кнопки нет.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub dot: Dot,
    pub state: String,
    pub sub: String,
    pub primary: Option<(Which, &'static str)>,
}

/// Состояние среды по состояниям операций — те же пять случаев, что считало
/// старое окно. Среда «работает», когда сделана операция `container`:
/// единственное имя операции, которое окно знает (состав списка задаёт
/// `dev1c.toml`, не окно).
pub fn status_line(ops: &[OpView], name: &str) -> Card {
    let container = ops.iter().find(|o| o.id == "container");
    let running = container.is_some_and(|o| o.dot == Dot::Up);
    let todo = ops.iter().filter(|o| o.dot == Dot::Off || o.dot == Dot::Busy).count();
    let drift = ops.iter().any(|o| o.dot == Dot::Busy);
    let done = ops.iter().filter(|o| o.dot == Dot::Up).count();
    let blocked = ops.iter().find(|o| o.dot == Dot::Down);
    // Доустановить или установить — одна и та же работа, разные надписи:
    // человек должен видеть, начинает он или продолжает.
    let finish = |label| Some((Which::Install, label));

    let (dot, state, sub, primary) = if !running && done == 0 {
        (Dot::Off, "Среда не установлена", format!("сделано 0 из {}", ops.len()), finish("Установить"))
    } else if !running {
        (Dot::Busy, "Среда установлена не полностью", format!("сделано {done} из {}", ops.len()), finish("Доустановить"))
    } else if drift {
        let detail = container.map(|o| o.detail.clone()).unwrap_or_default();
        (Dot::Busy, "Среда работает, параметры разошлись", format!("контейнер {name} · {detail}"), Some((Which::Change, "Применить изменения")))
    } else if let Some(b) = blocked {
        let primary = if todo > 0 { finish("Доустановить") } else { None };
        (Dot::Busy, "Среда работает, не всё настроено", format!("{}: {}", b.title, b.detail), primary)
    } else {
        let detail = container.map(|o| o.detail.clone()).unwrap_or_default();
        let primary = if todo > 0 { finish("Доустановить") } else { None };
        (Dot::Up, "Среда работает", format!("контейнер {name} · {detail}"), primary)
    };
    Card { dot, state: state.into(), sub, primary }
}

/// Плашка над вкладками: первая заблокированная операция своим текстом.
/// Недоделанное плашкой не считается — это обычная работа кнопки.
pub fn notice(ops: &[OpView]) -> Option<String> {
    ops.iter().find(|o| o.dot == Dot::Down).map(|o| o.detail.clone())
}

/// Рабочий поток обязан договорить: паника в теле всё равно шлёт конец
/// работы (иначе кнопки навсегда остались бы в «идёт работа») и строку об
/// этом в журнал. Отправитель — замыкание: так путь паники проверяется
/// тестом без канала GPUI.
pub fn guarded<E>(send: impl Fn(E), as_line: impl FnOnce(Line) -> E, body: impl FnOnce() -> E, on_panic: impl FnOnce() -> E) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(ev) => send(ev),
        Err(_) => {
            send(as_line(Line::fail(PANIC_TEXT)));
            send(on_panic());
        }
    }
}

/// Событие движка строкой журнала. `Command`/`Output` в окно не идут — им
/// место в журнале команд комплекта.
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

/// Факты проверки строками в журнал; подсказка провалившегося — второй строкой.
pub fn check_lines(checks: &[Check]) -> Vec<Line> {
    let mut out = Vec::with_capacity(checks.len());
    for c in checks {
        // Информационный факт — не приговор: обычная строка, а не провал.
        let kind = match (c.ok, c.info) {
            (true, false) => Kind::Ok,
            (true, true) => Kind::Say,
            _ => Kind::Fail,
        };
        out.push(Line { kind, text: format!("{}: {}", c.what, c.detail) });
        if kind == Kind::Fail && !c.hint.is_empty() {
            out.push(Line { kind: Kind::Warn, text: c.hint.clone() });
        }
    }
    out
}

/// Сколько шла работа, для «Готово за …»: меньше минуты — секунды, иначе
/// минуты и секунды. Секунды целые, без округления вверх.
pub fn took(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs} с")
    } else {
        format!("{} мин {} с", secs / 60, secs % 60)
    }
}

/// Местное время `HH:MM` для заголовка раздела журнала.
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

/// Заголовок раздела журнала: название работы и час её начала.
pub fn head_line(title: &str) -> Line {
    Line { kind: Kind::Head, text: format!("{title} · {}", clock()) }
}

/// Строка журнала на диск, тем же текстом, что рисует окно, но без цвета:
/// значок и пробел перед строкой, у заголовка раздела — пустая строка перед
/// ним. Ошибки записи молча игнорируются: журнал на диске не обязателен.
pub fn write_log_file(kit: &Kit, line: &Line) {
    use std::io::Write;
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(kit.path(LOG_FILE)) else { return };
    let text = if line.kind == Kind::Head { format!("\n{}\n", line.text) } else { format!("{:<2}{}\n", line.kind.mark(), line.text) };
    let _ = f.write_all(text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit_with(text: Option<&str>) -> Kit {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("dev1c-setup-ui-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(t) = text {
            std::fs::write(dir.join(cfg::FILE), t).unwrap();
        }
        Kit::at(&dir)
    }

    fn op(id: &str, st: OpState) -> OpView {
        op_view(id.into(), id.into(), st)
    }

    #[test]
    fn a_broken_config_file_yields_defaults_and_the_reason() {
        let kit = kit_with(Some("[server]\nname =\n"));
        let (cfg, err) = load_or_default(&kit);
        assert_eq!(cfg.server.name, Cfg::default().server.name);
        assert!(err.unwrap().contains(cfg::FILE), "в тексте ошибки должно быть имя файла");
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn a_good_config_file_yields_no_reason() {
        let kit = kit_with(Some("[server]\nname = \"other\"\n"));
        let (cfg, err) = load_or_default(&kit);
        assert_eq!(cfg.server.name, "other");
        assert!(err.is_none());
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn a_missing_config_file_is_not_an_error() {
        let kit = kit_with(None);
        let (_, err) = load_or_default(&kit);
        assert!(err.is_none());
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn the_notice_carries_the_text_of_the_first_blocked_operation() {
        let ops = vec![
            op("env", OpState::Done("docker 27".into())),
            op("model", OpState::Blocked("модель не выбрана: [model] file".into())),
        ];
        assert_eq!(notice(&ops).unwrap(), "модель не выбрана: [model] file");
        assert!(notice(&[op("env", OpState::Missing)]).is_none());
    }

    #[test]
    fn the_first_tab_is_operations_unless_the_file_is_gone() {
        assert_eq!(start_tab(true), Tab::Ops);
        assert_eq!(start_tab(false), Tab::Params);
    }

    #[test]
    fn a_panicking_worker_still_reports_the_end_of_work_and_a_line() {
        let sent = std::sync::Mutex::new(Vec::<String>::new());
        guarded(
            |e: String| sent.lock().unwrap().push(e),
            |l: Line| l.text,
            || panic!("boom"),
            || "done".to_string(),
        );
        assert_eq!(*sent.lock().unwrap(), vec![PANIC_TEXT.to_string(), "done".to_string()]);
    }

    #[test]
    fn a_working_worker_sends_only_its_own_event() {
        let sent = std::sync::Mutex::new(Vec::<String>::new());
        guarded(|e: String| sent.lock().unwrap().push(e), |l: Line| l.text, || "ok".to_string(), || "done".to_string());
        assert_eq!(*sent.lock().unwrap(), vec!["ok".to_string()]);
    }

    /// Карточка состояния коротко: заголовок и надпись главной кнопки.
    fn card(ops: &[OpView]) -> (String, Option<&'static str>) {
        let c = status_line(ops, "dev1c");
        (c.state, c.primary.map(|(_, label)| label))
    }

    #[test]
    fn the_status_card_names_the_state_and_the_next_step() {
        let missing = |id: &str| op(id, OpState::Missing);
        let done = |id: &str| op(id, OpState::Done("готово".into()));
        let drift = |id: &str| op(id, OpState::Drifted { from: "3390".into(), to: "4000".into() });
        let blocked = |id: &str| op(id, OpState::Blocked("модель не выбрана".into()));

        assert_eq!(card(&[missing("container"), missing("image")]), ("Среда не установлена".into(), Some("Установить")));
        assert_eq!(card(&[done("image"), missing("container")]), ("Среда установлена не полностью".into(), Some("Доустановить")));
        assert_eq!(card(&[done("container"), drift("image")]), ("Среда работает, параметры разошлись".into(), Some("Применить изменения")));
        assert_eq!(card(&[done("container"), blocked("model")]), ("Среда работает, не всё настроено".into(), None));
        assert_eq!(card(&[done("container"), done("image")]), ("Среда работает".into(), None));
    }

    #[test]
    fn a_blocked_environment_with_work_left_still_offers_to_finish_it() {
        let ops = vec![
            op("container", OpState::Done("запущен".into())),
            op("model", OpState::Blocked("модель не выбрана".into())),
            op("image", OpState::Missing),
        ];
        let c = status_line(&ops, "dev1c");
        assert_eq!(c.primary.map(|(w, l)| (w, l)), Some((Which::Install, "Доустановить")));
        assert_eq!(c.sub, "model: модель не выбрана");
    }

    #[test]
    fn the_subline_counts_what_is_done_until_the_container_runs() {
        let ops = vec![op("image", OpState::Done("загружен".into())), op("container", OpState::Missing)];
        assert_eq!(status_line(&ops, "dev1c").sub, "сделано 1 из 2");
    }

    #[test]
    fn a_running_environment_shows_the_container_name_and_its_detail() {
        let ops = vec![op("container", OpState::Done("запущен".into()))];
        assert_eq!(status_line(&ops, "dev1c").sub, "контейнер dev1c · запущен");
    }

    #[test]
    fn the_journal_line_of_an_event_keeps_the_kind_and_drops_the_command() {
        assert_eq!(line_of(&Event::Ok("готово".into())), Some(Line::ok("готово")));
        assert_eq!(line_of(&Event::Command("docker ps".into())), None);
    }

    #[test]
    fn how_long_it_took_reads_in_russian() {
        assert_eq!(took(Duration::from_secs(9)), "9 с");
        assert_eq!(took(Duration::from_secs(61)), "1 мин 1 с");
    }
}
