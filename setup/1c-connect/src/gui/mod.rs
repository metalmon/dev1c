//! Окно и трей на GPUI (gpui-kit) + tray-icon. Глобал `Ui` живёт в
//! приложении GPUI, сеанс туннеля — в нём же, и переживает закрытие окна:
//! его опускает только «Выход». Трей и обработчики ОС кладут события в один
//! канал; задача-читатель канала — единственное, что меняет `Ui`.

use crate::cfg::{self, Cfg, Host};
use crate::flow;
use crate::ops_host::{self, Ctx};
use crate::project::{self, Layout, Project};
use crate::session::{self, Session, State};
use crate::verify;
use common::check::{self, Check};
use common::kit::{write_text, Kit};
use common::op::{CleanLevel, OpState};
use common::{run, AlwaysDefault, Event, Phase, Progress, Runner, Sink};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub mod autostart;
pub mod panel;
pub mod theme;
pub mod tray;
pub mod window;

pub const TRAY_PNG: &[u8] = include_bytes!("../../icons/tray.png");

/// Три состояния трея: занят (операция/проверка идёт) берёт верх над связью —
/// `tray_state` ниже решает, какое из них сейчас.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrayState {
    Off,
    Up,
    Busy,
}

/// Занято — всегда жёлтый (идёт операция, связь тут не важна); иначе зелёный
/// при поднятой связи, серый без неё.
pub fn tray_state(link: &State, busy: bool) -> TrayState {
    if busy {
        TrayState::Busy
    } else if link.is_up() {
        TrayState::Up
    } else {
        TrayState::Off
    }
}

/// Значок трея со статусом: кружок в правом нижнем углу — зелёный (подключён),
/// жёлтый (занят) или серый (без связи); обесцвечен значок только без связи —
/// занятость тоже заметна цветным значком.
pub fn badge(base: &image::RgbaImage, state: TrayState) -> image::RgbaImage {
    let (w, h) = base.dimensions();
    let mut out = base.clone();
    if state == TrayState::Off {
        for p in out.pixels_mut() {
            let [r, g, b, _] = p.0;
            let gr = ((r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100) as u8;
            p.0[0] = gr;
            p.0[1] = gr;
            p.0[2] = gr;
        }
    }
    let r = (w as f32 * 0.22).max(3.0);
    let (cx, cy) = (w as f32 - r - 1.0, h as f32 - r - 1.0);
    let colour: [u8; 4] = match state {
        TrayState::Busy => [0xfa, 0xcc, 0x1f, 255],
        TrayState::Up => [0x2f, 0x8f, 0x4e, 255],
        TrayState::Off => [0x8d, 0x86, 0x7e, 255],
    };
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d <= r {
                let rim = d > r - 1.5;
                out.put_pixel(x, y, image::Rgba(if rim { [255, 255, 255, 255] } else { colour }));
            }
        }
    }
    out
}

/// Состояние окна целиком. Глобал GPUI: один на приложение, живёт дольше
/// любого окна.
pub struct Ui {
    pub kit: Kit,
    pub cfg: Cfg,
    pub session: Option<Session>,
    /// Растёт на каждый новый сеанс и на каждую остановку: хвостовое
    /// событие от уже не текущего сеанса (умирающий поток ещё договаривает)
    /// несёт старое значение и отбрасывается в `handle`.
    pub session_gen: u64,
    pub link: State,
    /// Обслуживаются ли вызовы агента: живёт дольше любого сеанса (заводится
    /// один раз в `launch`, `start_session`/`stop_session`/«Сохранить» его не
    /// пересоздают), передаётся в `session::Serve` через `session::start_with_enabled` и
    /// читается мостом (`bridge::Rules`) на каждый вызов. В локальном режиме
    /// переключатель окна меняет только этот флаг — слушатели остаются
    /// поднятыми; в туннельном — как раньше, переключатель поднимает и
    /// опускает сеанс целиком.
    pub enabled: Arc<AtomicBool>,
    pub log: Vec<Line>,
    pub unread: usize,
    pub tab: Tab,
    /// Сколько фактов провалила последняя проверка — бейдж на «Журнале».
    /// Сами факты живут строками в журнале (`check_lines`), списка их нет.
    pub failed_checks: usize,
    pub ops: Vec<OpView>,
    /// Каталог проекта, перечитанный в своём потоке (`refresh_project`);
    /// `None` — ещё не спрашивали.
    pub project: Option<ProjectView>,
    /// Операция (`run_op`) на ходу: фаза, начало, последняя активность.
    /// Проверка (`run_check`) идёт без неё — у неё нет `Which`.
    pub progress: Option<Running>,
    /// Флаг отмены текущей операции; `run_op` заводит новый `Arc` на каждый
    /// запуск, чтобы устаревший клон в потоке прежней операции не мог
    /// отменить следующую.
    pub cancel: Arc<AtomicBool>,
    /// Пришли новые фаза/заметка после последней перерисовки карточки хода.
    pub dirty: bool,
    pub busy: Option<Busy>,
    /// Число живых потоков `refresh_ops`/`refresh_project` — растёт в момент
    /// `thread::spawn`, падает (насыщенно) в `Ev::Ops`/`Ev::Project`. Входит
    /// в `working()`: полоса хода в разделителе над подвалом.
    pub pending: usize,
    pub status: Option<Line>,
    pub tray: Option<tray::Tray>,
    /// `gpui_kit::open_window` оборачивает панель в свой корневой вид,
    /// поэтому ручка окна — безтиповая (`downcast::<Panel>()` дал бы `None`),
    /// а до самой панели ведёт отдельная сущность рядом.
    pub window: Option<AnyWindowHandle>,
    pub panel: Option<Entity<panel::Panel>>,
    /// Высота окна, подобранная под содержимое «Параметров» (`window::fit_height`);
    /// `None` — ещё не мерили, окно открывается наименьшим.
    pub height: Option<Pixels>,
    pub hidden_at: Option<Instant>,
    pub dialog_open: bool,
    /// Когда тик в последний раз подвинул подсказку трея на фазу операции —
    /// `tick` не чаще `TOOLTIP_EVERY`, иначе Win32 моргал бы подсказкой.
    pub tooltip_at: Option<Instant>,
    pub tx: UnboundedSender<Ev>,
}
impl Global for Ui {}

impl Ui {
    /// Что-то идёт: операция/проверка (`busy`), подключение туннеля, или
    /// фоновое обновление состояния (`pending` — `refresh_ops`/`refresh_project`
    /// ещё не ответили). Полоса хода в разделителе над подвалом читает это.
    pub fn working(&self) -> bool {
        self.busy.is_some() || matches!(self.link, State::Connecting) || self.pending > 0
    }
}

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

/// Три вкладки (решение заказчика 2026-10-18): «Расширение» ушло в «Проект»,
/// «Проверка» — в «Журнал». Пять подписей и по три кнопки в подвале в 380 px
/// не помещались.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Params,
    Project,
    Log,
}

impl Tab {
    /// Порядок вкладок в полосе; он же — порядок индексов `TabBar`.
    pub const ALL: [Tab; 3] = [Tab::Params, Tab::Project, Tab::Log];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Busy {
    Ext,
    Project,
    Check,
}

/// Что делает кнопка вкладки: расширение или git-обмен с проектом.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Install,
    Uninstall,
    Export,
    Import,
}

impl Which {
    /// Заголовок в журнал перед работой — тексты из `RUN_TITLES` в `app.js`.
    pub fn title(self) -> &'static str {
        match self {
            Which::Install => "— устанавливаю —",
            Which::Uninstall => "— снимаю расширение —",
            Which::Export => "— выгружаю из 1С —",
            Which::Import => "— загружаю в 1С —",
        }
    }
}

/// Операция на ходу: с чего начали, какая фаза сейчас и когда от неё
/// последний раз были вести (строка в журнал или `Event::Progress`).
/// `stale_noted` — застой уже отмечен строкой в журнал для этой операции
/// (не чаще раза: см. `stale_line`).
pub struct Running {
    pub which: Which,
    pub started: Instant,
    pub phase: Phase,
    pub seen: Instant,
    pub stale_noted: bool,
}

/// Фаза, с которой начинается операция — журнал и карточка хода встречают
/// её раньше первого `Event::Progress` движка.
pub fn first_phase(which: Which) -> Phase {
    match which {
        Which::Export => Phase::Decompile,
        Which::Import => Phase::Pull,
        Which::Install => Phase::Install,
        Which::Uninstall => Phase::Uninstall,
    }
}

/// Вкладка «Проект»: раскладка каталога и, если она в порядке, состояние
/// ветки строками `project::status`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectView {
    pub dir: String,
    pub branch: String,
    pub lines: Vec<String>,
    pub ok: bool,
    pub why: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dot {
    Off,
    Busy,
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub struct OpView {
    pub id: String,
    pub title: String,
    pub dot: Dot,
    pub detail: String,
}

pub enum Ev {
    Tray(tray_icon::TrayIconEvent),
    Menu(tray_icon::menu::MenuEvent),
    /// Переход сеанса, помеченный поколением, что был текущим в момент
    /// отправки — старое поколение значит сеанс уже не тот.
    Link(u64, State),
    Line(Line),
    /// Фаза/заметка долгой операции — для карточки хода (журналу своей
    /// строки хватает, её шлёт `line_of`).
    Progress(Progress),
    Done { err: Option<String>, cancelled: bool },
    Checks(Vec<Check>),
    /// Состояние операций, перечитанное в своём потоке (`refresh_ops`).
    Ops(Vec<OpView>),
    /// Каталог проекта, перечитанный в своём потоке (`refresh_project`).
    Project(ProjectView),
}

/// Сколько ждать опускания туннеля на «Выходе».
pub const STOP_WAIT: Duration = Duration::from_secs(5);

/// Единый тик приложения (замена прежнего тика панели): раз в секунду, пока
/// идёт операция, иначе раз в полминуты — только на перерисовку «N мин» в
/// карточке хода, когда окно открыто. Подсказка трея обновляется не чаще
/// `TOOLTIP_EVERY`.
pub const TICK_BUSY: Duration = Duration::from_secs(1);
pub const TICK_IDLE: Duration = Duration::from_secs(30);
pub const TOOLTIP_EVERY: Duration = Duration::from_secs(60);

/// Текст в журнал, когда рабочий поток упал с паникой.
pub const PANIC_TEXT: &str = "внутренняя ошибка: операция прервана";

/// Рабочий поток обязан договорить: паника в теле всё равно шлёт конец
/// работы (иначе кнопки навсегда остались бы в «идёт работа») и строку об
/// этом в журнал.
fn guarded(tx: &UnboundedSender<Ev>, body: impl FnOnce() -> Ev, on_panic: impl FnOnce() -> Ev) {
    let ev = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(ev) => ev,
        Err(_) => {
            let _ = tx.unbounded_send(Ev::Line(Line { kind: Kind::Fail, text: PANIC_TEXT.into() }));
            on_panic()
        }
    };
    let _ = tx.unbounded_send(ev);
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
fn running_clock(d: Duration) -> String {
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

/// Подсказка трея: во время операции — фаза строчными и сколько она идёт
/// («выгружаю конфигурацию, 12 мин»; меньше минуты — «меньше минуты»); без
/// операции — просто про связь.
pub fn tray_tooltip(progress: Option<&Running>, link: &State, now: Instant) -> String {
    match progress {
        Some(r) => {
            let mut chars = r.phase.title().chars();
            let lower: String = match chars.next() {
                Some(c) => c.to_lowercase().chain(chars).collect(),
                None => String::new(),
            };
            let mins = now.saturating_duration_since(r.started).as_secs() / 60;
            let when = if mins == 0 { "меньше минуты".to_string() } else { format!("{mins} мин") };
            format!("1c-connect: {lower}, {when}")
        }
        None => format!("1c-connect: {}", if link.is_up() { "подключён" } else { "не подключён" }),
    }
}

/// Пять минут без вестей от потока — поток молчит, но операция ещё жива.
const QUIET: Duration = Duration::from_secs(5 * 60);

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
fn stale_line(r: &mut Running, now: Instant) -> Option<Line> {
    if is_stale(r, now) && !r.stale_noted {
        r.stale_noted = true;
        Some(Line { kind: Kind::Warn, text: stale_text(r, now) })
    } else {
        None
    }
}

/// Отменить можно только git-обмен (`Push`/`Pull`) — он единственный умеет
/// слушать флаг (`flow::export`/`import`); `cf_decompile`/`cf_load` на
/// стороне платформы не прерываются.
pub fn can_cancel(phase: Phase) -> bool {
    matches!(phase, Phase::Push | Phase::Pull)
}

/// Кнопка «Отменить» карточки хода — поток сам решает, когда заметить флаг
/// и договорить `Ev::Done { cancelled: true, .. }`.
pub fn cancel_op(cx: &mut App) {
    cx.global::<Ui>().cancel.store(true, Ordering::Relaxed);
}

/// Строка журнала на диск, тем же текстом, что рисует окно, но без цвета:
/// значок и пробел перед строкой (у `Say` — два пробела вместо значка),
/// у заголовка раздела — пустая строка перед ним. Ошибки записи (диск занят,
/// прав нет) молча игнорируются — журнал на диске не обязателен для работы.
fn write_log_file(kit: &Kit, line: &Line) {
    use std::io::Write;
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(kit.path(LOG_FILE)) else { return };
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

/// Строка перехода сеанса — тексты как в сегодняшнем `gui.rs::set_link`.
/// Начало и конец видны и в журнале: туннель сам пишет только про поднятую
/// связь и обрывы, а отказ возвращает ошибкой.
pub fn link_line(l: &State, host: &Host) -> Option<Line> {
    match l {
        State::Connecting => Some(Line { kind: Kind::Say, text: format!("подключаюсь к {} как {}", host.server, host.workstation) }),
        State::Failed(why) => Some(Line { kind: Kind::Fail, text: why.clone() }),
        State::Off => Some(Line { kind: Kind::Say, text: "отключено".into() }),
        State::Up { .. } | State::Retrying(_) => None,
    }
}

/// `Ev::Link(gen, ..)` стоит применять только если `gen` — поколение сеанса,
/// текущее прямо сейчас; иначе это хвост уже остановленного или уже
/// замещённого сеанса.
fn link_is_current(ui: &Ui, gen: u64) -> bool {
    gen == ui.session_gen
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

/// Карточка операции из состояния `apply_all`/`status`.
pub fn op_view(id: String, title: String, st: OpState) -> OpView {
    let (dot, detail) = match st {
        OpState::Done(d) => (Dot::Up, d),
        OpState::Missing => (Dot::Off, "не установлено".into()),
        OpState::Drifted { from, to } => (Dot::Busy, format!("было: {from} → станет: {to}")),
        OpState::Blocked(why) => (Dot::Down, why),
    };
    OpView { id, title, dot, detail }
}

/// Заголовок строки `host.publication` в «Проекте»: слова 1С-разработчика
/// вместо «публикация»/реверс-прокси — база отвечает по HTTP, вот и всё.
pub const PUBLICATION_TITLE: &str = "HTTP-сервис базы";

/// Подробность той же строки: адрес, на который смотреть браузером
/// (`auto` — свой httpd на 127.0.0.1:<порт>, как в `ops_host::Publication`),
/// либо прямое указание, что публикацию настроил человек (`webserver`).
/// Дескрипторы, httpd.conf и прочее — в журнал (`ops_host`/`publication`),
/// не в эту строку: окно не должно объяснять реверс-прокси.
pub fn publication_detail(cfg: &Cfg) -> String {
    if cfg.host.publication == "webserver" {
        "публикация настроена вручную".into()
    } else {
        format!("база отвечает по HTTP: 127.0.0.1:{}", cfg.http_port())
    }
}

/// Главное действие вкладки «Проект»: пока расширение не стоит — «Установить»,
/// дальше человеку нужен обмен с git, и главной становится «Выгрузить».
/// Остальное — в меню «…».
pub fn project_primary(ops: &[OpView]) -> Which {
    match ops.iter().find(|o| o.id == "host.extension") {
        Some(o) if o.dot == Dot::Up => Which::Export,
        _ => Which::Install,
    }
}

/// Факты проверки строками в журнал: отдельного списка на экране больше нет.
/// Значок перед строкой даёт `Kind::mark` при отрисовке — в тексте его больше
/// нет. Подсказка провалившегося факта идёт второй строкой без ведущих
/// пробелов: отступ ей даёт колонка значка, а не сам текст.
pub fn check_lines(checks: &[Check]) -> Vec<Line> {
    let mut out = Vec::with_capacity(checks.len());
    for c in checks {
        // Информационный факт — не приговор: как серый кружок прежней
        // вкладки «Проверка», обычная строка, а не провал.
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

/// Файл или каталог средствами системы: на Windows — `explorer`, иначе
/// `xdg-open`. Не открылось — строкой в журнал, молча не теряем.
pub fn open_path(cx: &mut App, path: &std::path::Path) {
    let mut c = std::process::Command::new(if cfg!(windows) { "explorer" } else { "xdg-open" });
    if let Err(e) = c.arg(path).spawn() {
        note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: format!("не открыть {}: {e}", path.display()) });
        cx.refresh_windows();
    }
}

/// Адрес публикации для записи: умолчание идёт за каталогом базы, правка в
/// toml переживает его смену.
pub fn carried_base_url(saved: &Host, new_base_path: &str) -> String {
    if saved.base_url.is_empty() || saved.base_url == cfg::derived_base_url(&saved.base_path) {
        cfg::derived_base_url(new_base_path)
    } else {
        saved.base_url.clone()
    }
}

/// Сохранить параметры сервера. Списка инструментов, каталога выгрузки,
/// режима публикации, её порта и адреса публикации в окне нет (правятся в
/// toml): берём их из текущих параметров, иначе пустые значения из формы
/// затёрли бы toml. Каталог проекта — из формы.
pub fn save_host(ui: &mut Ui, mut host: Host) -> Result<(), String> {
    let old_host = ui.cfg.host.clone();
    host.dump = ui.cfg.host.dump.clone();
    host.tools = ui.cfg.host.tools.clone();
    host.mode = ui.cfg.host.mode.clone();
    host.publication = ui.cfg.host.publication.clone();
    host.http_port = ui.cfg.host.http_port;
    host.base_url = carried_base_url(&ui.cfg.host, &host.base_path);
    let cfg = Cfg { host };
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    write_text(&ui.kit.path(cfg::FILE), &cfg.render()).map_err(|e| format!("{e:#}"))?;
    let base_path = cfg.host.base_path.clone();
    // То же сравнение, что решает, светится ли «Сохранить» (`panel::fields_differ`):
    // поля, которых окно не показывает (в т.ч. `mode`), сеанс не перезапускают
    // здесь — смена режима в трее поднимает его сама, отдельным путём
    // (`tray::on_menu`), и лишний перезапуск в довесок к тому был бы только
    // лишней строкой в журнал и лишним миганием портов.
    let changed = panel::fields_differ(&cfg.host, &old_host, &cfg.host.mode);
    ui.cfg = cfg;
    if changed && ui.session.is_some() {
        restart_session(ui, base_path);
    }
    Ok(())
}

/// Перезапустить сеанс после удачной записи параметров: старому потоку
/// просят остановиться (в своём потоке, не блокируя этот), новый поднимают
/// сразу с уже записанными параметрами — тем же флагом `enabled`, он живёт
/// дольше сеанса. Переход в «подключаюсь»/«подключён» придёт позже обычным
/// `Ev::Link`, здесь — только одна строка в журнал про сам факт перезапуска.
fn restart_session(ui: &mut Ui, base_path: String) {
    ui.session_gen += 1;
    let gen = ui.session_gen;
    if let Some(s) = ui.session.take() {
        std::thread::spawn(move || {
            let _ = s.stop();
        });
    }
    let ctx = ctx_for(ui);
    let (cfg, kit, tx, enabled) = (ui.cfg.clone(), ui.kit.clone(), ui.tx.clone(), ui.enabled.clone());
    let session = session::start_with_enabled(cfg, kit, ctx.sink, enabled, move |l| {
        let _ = tx.unbounded_send(Ev::Link(gen, l));
    });
    ui.session = Some(session);
    note_line(ui, Line { kind: Kind::Say, text: format!("MCP перезапущены: база {base_path}") });
}

/// Контекст одного прогона: события идут в журнал на диске (команды и их
/// сырой вывод) или в окно — остальное.
fn ctx_for(ui: &Ui) -> Ctx {
    let kit = ui.kit.clone();
    let tx = ui.tx.clone();
    let sink: Sink = Arc::new(move |e: Event| {
        if kit.journal(&e) {
            return;
        }
        if let Event::Progress(p) = &e {
            let _ = tx.unbounded_send(Ev::Progress(p.clone()));
        }
        if let Some(line) = line_of(&e) {
            let _ = tx.unbounded_send(Ev::Line(line));
        }
    });
    Ctx { kit: ui.kit.clone(), cfg: ui.cfg.clone(), dry_run: false, sink, prompt: Box::new(AlwaysDefault) }
}

/// Синхронизировать трей с `Ui`: состояние (занято — жёлтый, иначе по
/// связи) и подсказка на текущий момент. Общий конец всех мест, что меняют
/// `busy`/`progress`/`link` — `set_link`, `run_op`, `Ev::Done`, `Ev::Checks`;
/// один из них забыл это раньше (S1) — значок трея застревал жёлтым после
/// проверки.
fn sync_tray(ui: &Ui, now: Instant) {
    let busy = ui.progress.is_some() || ui.busy.is_some();
    let (state, on) = (tray_state(&ui.link, busy), ui.link.is_on());
    let tooltip = tray_tooltip(ui.progress.as_ref(), &ui.link, now);
    if let Some(tray) = &ui.tray {
        tray.set_state(state, on);
        tray.set_tooltip(&tooltip);
    }
}

/// Переход сеанса: в состояние, в журнал, в трей, перерисовать окно.
fn set_link(cx: &mut App, l: State) {
    let host = cx.global::<Ui>().cfg.host.clone();
    let line = link_line(&l, &host);
    let ui = cx.global_mut::<Ui>();
    ui.link = l.clone();
    if let Some(line) = line {
        note_line(ui, line);
    }
    sync_tray(ui, Instant::now());
    cx.refresh_windows();
}

/// Автоподключение при запуске: ключ состояния комплекта `autoconnect`,
/// нет записи — выключено.
pub fn autoconnect(kit: &Kit) -> bool {
    kit.state_get("autoconnect").as_deref() == Some("1")
}

/// Тексты ошибок — дословно из сегодняшнего `gui.rs::start_session`.
pub fn start_session(cx: &mut App) -> Result<(), String> {
    let ui = cx.global_mut::<Ui>();
    if !ui.kit.path(cfg::FILE).exists() {
        return Err("в комплекте нет 1c-connect.toml — заполните параметры и нажмите «Сохранить»".into());
    }
    // Сеанс уже есть и не сдался (`Failed`) — второй поверх него не нужен,
    // в том числе в окно между `session::start_with_enabled` и первым `Connecting`.
    if ui.session.is_some() && !matches!(ui.link, State::Failed(_)) {
        return Ok(());
    }
    // Новый сеанс — новое поколение: хвост от прежнего (если он всё ещё
    // договаривает) будет отбрасываться в `handle` по несовпадению.
    ui.session_gen += 1;
    let gen = ui.session_gen;
    let ctx = ctx_for(ui);
    let (cfg, kit, tx, enabled) = (ui.cfg.clone(), ui.kit.clone(), ui.tx.clone(), ui.enabled.clone());
    let session = session::start_with_enabled(cfg, kit, ctx.sink, enabled, move |l| {
        let _ = tx.unbounded_send(Ev::Link(gen, l));
    });
    cx.global_mut::<Ui>().session = Some(session);
    Ok(())
}

/// Остановить сеанс (блокирующе — в своём потоке) и сразу показать «отключено».
pub fn stop_session(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    // Хвостовой `Ev::Link` от останавливаемого сеанса (тот всё ещё может
    // договаривать в своём потоке) должен отбрасываться — он уже не про
    // текущее поколение.
    ui.session_gen += 1;
    if let Some(s) = ui.session.take() {
        std::thread::spawn(move || {
            let _ = s.stop();
        });
    }
    set_link(cx, State::Off);
}

/// Переключатель в локальном режиме не трогает сеанс — слушатели уже подняты
/// и остаются поднятыми (см. `launch`), меняется только то, отвечают ли
/// вызовы агента. Возвращает новое состояние флага.
fn flip_enabled_only(ui: &mut Ui) -> bool {
    let on = !ui.enabled.load(Ordering::Relaxed);
    ui.enabled.store(on, Ordering::Relaxed);
    on
}

/// В локальном режиме слушатели поднимаются при старте и остаются поднятыми
/// независимо от переключателя — настоящее состояние сеанса почти всегда
/// `Up`/`Connecting`. Экран обязан при этом показывать «MCP выключены», пока
/// флаг снят: подменяем `Connecting`/`Up` на `Off`. Отказ (`Failed`) виден
/// всегда — порт занят независимо от переключателя.
fn local_display(mode: &str, enabled: bool, l: State) -> State {
    if mode == "local" && !enabled {
        match l {
            State::Up { .. } | State::Connecting => State::Off,
            other => other,
        }
    } else {
        l
    }
}

pub fn toggle_link(cx: &mut App) {
    if cx.global::<Ui>().cfg.host.mode == "local" {
        let ui = cx.global_mut::<Ui>();
        let on = flip_enabled_only(ui);
        let l = if on { State::Up { since: Instant::now() } } else { State::Off };
        set_link(cx, l);
        return;
    }
    let on = cx.global::<Ui>().link.is_on();
    if on {
        stop_session(cx);
        return;
    }
    if let Err(e) = start_session(cx) {
        window::show_window(cx);
        note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
        cx.refresh_windows();
    }
}

/// Расширение (установить/снять) или обмен с git (выгрузить/загрузить);
/// ход — событиями из `ctx_for`, конец — `Ev::Done`. Заголовок работы ложится
/// в журнал, и журнал же показывается: за ходом смотрят там.
pub fn run_op(cx: &mut App, which: Which) {
    let ui = cx.global_mut::<Ui>();
    ui.busy = Some(match which {
        Which::Export | Which::Import => Busy::Project,
        _ => Busy::Ext,
    });
    note_line(ui, head_line(which.title()));
    let now = Instant::now();
    let cancel = Arc::new(AtomicBool::new(false));
    ui.progress = Some(Running { which, started: now, phase: first_phase(which), seen: now, stale_noted: false });
    ui.cancel = cancel.clone();
    ui.tab = Tab::Log;
    ui.unread = 0;
    // Подсказка фазы сразу, а не через TOOLTIP_EVERY: иначе первые 60 с
    // новой операции висела бы подсказка от прежней (или «подключён»).
    ui.tooltip_at = Some(now);
    sync_tray(ui, now);
    let ctx = ctx_for(ui);
    let tx = ui.tx.clone();
    cx.refresh_windows();
    std::thread::spawn(move || {
        guarded(
            &tx,
            || {
                let r: Result<(), String> = match which {
                    Which::Install => run::apply_all(&ctx, &ops_host::host(), None, false).map(|_| ()).map_err(|e| format!("{e:#}")),
                    Which::Uninstall => ops_host::host()
                        .iter()
                        .find(|o| o.id() == "host.extension")
                        .map(|ext| ext.revert_apply(&ctx, CleanLevel::Stack))
                        .unwrap_or(Ok(()))
                        .map_err(|e| format!("{e:#}")),
                    // Те же инструменты, что зовёт агент, под тем же замком проекта.
                    Which::Export => flow::export(&ctx, "выгрузка из 1c-connect", &cancel).map(|t| ctx.ok(t)),
                    Which::Import => flow::import(&ctx, &cancel).map(|t| ctx.ok(t)),
                };
                match r {
                    Ok(()) => Ev::Done { err: None, cancelled: false },
                    Err(e) => Ev::Done { err: Some(e), cancelled: cancel.load(Ordering::Relaxed) },
                }
            },
            || Ev::Done { err: Some(PANIC_TEXT.into()), cancelled: false },
        );
    });
}

/// Перечитать каталог проекта в своём потоке: `layout_state` и `status` ходят
/// по диску и к git, на главном потоке подвесили бы окно.
pub fn refresh_project(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    let (cfg, tx) = (ui.cfg.clone(), ui.tx.clone());
    ui.pending += 1;
    std::thread::spawn(move || {
        guarded(
            &tx,
            || {
                let p = Project::from_cfg(&cfg, &ops_host::tools_dir());
                let (ok, why) = match project::layout_state(&p) {
                    Layout::Ok => (true, String::new()),
                    Layout::Missing => (false, "каталог проекта ещё не разложен".into()),
                    Layout::BaseDir(f) => (false, format!("это каталог базы ({f})")),
                    Layout::Drifted(d) => (false, d),
                };
                // Тот же инструмент, что зовёт агент, под тем же замком
                // проекта: во время выгрузки вкладка говорит «занято», а не
                // читает полузаписанный индекс (правка 6B на main).
                let lines = if ok {
                    let (text, _) = project::call(&p, "project_status", &serde_json::Value::Null);
                    text.lines().map(str::to_string).collect()
                } else {
                    Vec::new()
                };
                Ev::Project(ProjectView { dir: p.dir.display().to_string(), branch: p.branch, lines, ok, why })
            },
            || Ev::Project(ProjectView { dir: String::new(), branch: String::new(), lines: Vec::new(), ok: false, why: PANIC_TEXT.into() }),
        );
    });
}

/// Семь фактов от основания к вершине; отчёт пишется в `last-check.txt` тем
/// же текстом, что печатает командная строка.
pub fn run_check(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    ui.busy = Some(Busy::Check);
    note_line(ui, head_line("Проверка"));
    ui.tab = Tab::Log;
    ui.unread = 0;
    let ctx = ctx_for(ui);
    let link = ui.link.clone();
    let mode = ui.cfg.host.mode.clone();
    let tx = ui.tx.clone();
    cx.refresh_windows();
    std::thread::spawn(move || {
        guarded(
            &tx,
            || {
                let state = link.is_up().then(|| link.label(&mode));
                let checks = verify::run(&ctx, state.as_deref());
                let text = check::render("1c-connect", env!("CARGO_PKG_VERSION"), &verify::cfg_for_report(&ctx.cfg), &checks);
                let _ = write_text(&ctx.kit.path("last-check.txt"), &text);
                Ev::Checks(checks)
            },
            || Ev::Checks(Vec::new()),
        );
    });
}

/// Перечитать состояние операций на машине (без изменений) в `ui.ops`.
/// В своём потоке: `run::status` ходит по сети (у `ops_host` запрос с
/// минутным сроком) и на главном потоке подвесил бы окно.
pub fn refresh_ops(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    let ctx = ctx_for(ui);
    let tx = ui.tx.clone();
    ui.pending += 1;
    std::thread::spawn(move || {
        guarded(
            &tx,
            || Ev::Ops(run::status(&ctx, &ops_host::host()).into_iter().map(|(id, title, st)| op_view(id, title, st)).collect()),
            || Ev::Ops(Vec::new()),
        );
    });
}

/// Выход: ждём, пока сеанс действительно опустится (поток собственного
/// рантайма сеанса), и только потом закрываем приложение — иначе туннель
/// обрывается не чисто. Ждём не дольше `STOP_WAIT`: застрявший `stop` не
/// должен держать «Выход» навсегда.
pub fn quit(cx: &mut App) {
    if let Some(s) = cx.global_mut::<Ui>().session.take() {
        let (done, stopped) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = s.stop();
            let _ = done.send(());
        });
        let _ = stopped.recv_timeout(STOP_WAIT);
    }
    cx.quit();
}

/// `Ev::Progress`: фаза и `seen` идут в карточку хода, не в журнал — тот
/// получил свою строку с заголовком фазы через `line_of` уже отдельно.
/// `now` — параметр, а не свой `Instant::now()`, иначе тест не отличил бы
/// «`seen` подвинулся» от «часы сами не стоят на месте».
/// `Ev::Line`: журнал получает каждую строку всегда (в т.ч. на диск,
/// `1c-connect.log`); во время операции `seen` карточки хода, вдобавок,
/// подвигается этой строкой — но без `refresh_windows()` на каждую строку, по
/// спеке «Троттлинг»: `dirty` подхватит `tick` раз в `TICK_BUSY`. Вызывающий
/// сам решает про `refresh_windows()` вне операции — здесь нет `cx`.
fn on_line(ui: &mut Ui, line: Line, now: Instant) {
    note_line(ui, line.clone());
    if let Some(r) = ui.progress.as_mut() {
        r.seen = now;
        ui.dirty = true;
    }
}

fn on_progress(ui: &mut Ui, p: Progress, now: Instant) {
    if let Some(r) = ui.progress.as_mut() {
        r.phase = p.phase;
        r.seen = now;
    }
    ui.dirty = true;
}

/// Конец операции: строка в журнал (провал/успех/отмена — отмена не
/// провал, `Kind::Warn`), и сброс карточки хода с новым флагом отмены —
/// старый, даже уже взведённый, никакую следующую операцию не отменит.
fn on_done(ui: &mut Ui, err: Option<String>, cancelled: bool) -> Line {
    ui.busy = None;
    let running = ui.progress.take();
    let line = if cancelled {
        Line { kind: Kind::Warn, text: err.unwrap_or_else(|| "прервано".into()) }
    } else {
        match err {
            Some(e) => Line { kind: Kind::Fail, text: e },
            None => match running {
                Some(r) => Line { kind: Kind::Ok, text: format!("Готово за {}", took(r.started.elapsed())) },
                None => Line { kind: Kind::Ok, text: "Готово.".into() },
            },
        }
    };
    ui.cancel = Arc::new(AtomicBool::new(false));
    line
}

/// Перерисовать на тике: окно открыто — карточка связи считает минуты от
/// `since.elapsed()` сама на каждой отрисовке и обязана не стоять на месте,
/// даже в покое; окно спрятано — только когда есть, что показать (новые
/// данные или операция на ходу).
fn tick_redraw(window_open: bool, dirty: bool, running: bool) -> bool {
    window_open || dirty || running
}

/// Единый тик приложения (`launch` зовёт его раз в `TICK_BUSY`/`TICK_IDLE`):
/// перерисовывает окно (см. `tick_redraw`), один раз на операцию отмечает
/// застой строкой в журнал (`stale_line`) и, не чаще `TOOLTIP_EVERY`, двигает
/// подсказку трея на фазу текущей операции.
fn tick(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    let redraw = tick_redraw(ui.window.is_some(), ui.dirty, ui.progress.is_some());
    ui.dirty = false;
    if redraw {
        cx.refresh_windows();
    }
    let now = Instant::now();
    let ui = cx.global_mut::<Ui>();
    if ui.progress.is_some() {
        if let Some(line) = ui.progress.as_mut().and_then(|r| stale_line(r, now)) {
            note_line(ui, line);
        }
        let due = ui.tooltip_at.map(|at| now.saturating_duration_since(at) >= TOOLTIP_EVERY).unwrap_or(true);
        if due {
            let text = tray_tooltip(ui.progress.as_ref(), &ui.link, now);
            ui.tooltip_at = Some(now);
            if let Some(tray) = &ui.tray {
                tray.set_tooltip(&text);
            }
        }
    }
}

fn handle(ev: Ev, cx: &mut App) {
    match ev {
        Ev::Tray(e) => tray::on_tray(e, cx),
        Ev::Menu(e) => tray::on_menu(e, cx),
        Ev::Link(gen, l) => {
            if link_is_current(cx.global::<Ui>(), gen) {
                let ui = cx.global::<Ui>();
                let l = local_display(&ui.cfg.host.mode, ui.enabled.load(Ordering::Relaxed), l);
                set_link(cx, l);
            }
        }
        Ev::Line(line) => {
            let now = Instant::now();
            let ui = cx.global_mut::<Ui>();
            let running = ui.progress.is_some();
            on_line(ui, line, now);
            if !running {
                cx.refresh_windows();
            }
        }
        Ev::Progress(p) => on_progress(cx.global_mut::<Ui>(), p, Instant::now()),
        Ev::Done { err, cancelled } => {
            let ui = cx.global_mut::<Ui>();
            let line = on_done(ui, err, cancelled);
            note_line(ui, line.clone());
            ui.status = Some(line.clone());
            // Подсказка без операции: `on_done` уже опустил `progress`/`busy`.
            // `tooltip_at` тоже назад в `None` — следующая операция не должна
            // наследовать свежий штамп и ждать до TOOLTIP_EVERY своей подсказки.
            let now = Instant::now();
            ui.tooltip_at = None;
            sync_tray(ui, now);
            let hidden = ui.window.is_none();
            cx.refresh_windows();
            if hidden {
                cx.show_system_notification(SystemNotification {
                    tag: "op".into(),
                    title: "1c-connect".into(),
                    body: line.text.clone().into(),
                    actions: vec![],
                });
            }
            refresh_ops(cx);
            refresh_project(cx);
        }
        Ev::Checks(checks) => {
            let failed = checks.iter().filter(|c| !c.ok).count();
            let lines = check_lines(&checks);
            let ui = cx.global_mut::<Ui>();
            ui.failed_checks = failed;
            for line in lines {
                note_line(ui, line);
            }
            ui.busy = None;
            let status = if failed > 0 {
                Line { kind: Kind::Fail, text: format!("Проблем: {failed}. Отчёт — last-check.txt в комплекте") }
            } else {
                Line { kind: Kind::Ok, text: "Проверка пройдена. Отчёт — last-check.txt".into() }
            };
            note_line(ui, status.clone());
            ui.status = Some(status);
            // Трей (S1): проверка — другое место, где `busy` возвращается в
            // `None`, и значок/подсказку надо подвинуть, как у `Ev::Done` —
            // иначе жёлтый («занят») может застрять до следующего перехода.
            sync_tray(ui, Instant::now());
            cx.refresh_windows();
        }
        Ev::Ops(ops) => {
            let ui = cx.global_mut::<Ui>();
            ui.ops = ops;
            ui.pending = ui.pending.saturating_sub(1);
            cx.refresh_windows();
        }
        Ev::Project(v) => {
            let ui = cx.global_mut::<Ui>();
            ui.project = Some(v);
            ui.pending = ui.pending.saturating_sub(1);
            cx.refresh_windows();
        }
    }
}

/// Нативное сообщение об ошибке (на Windows — `MessageBoxW`, иначе `stderr`)
/// перед принудительным выходом: без трея программа беззвучно зависла бы —
/// окно без трея не закрыть (ни кнопки на всплывающем окне снаружи, ни
/// пункта «Выход» вне его меню), а `QuitMode::Explicit` не уходит по закрытию
/// окна. `eprintln!` здесь не увидят: обычный запуск без консоли, а релиз —
/// `windows_subsystem = "windows"`.
#[cfg(windows)]
fn fatal(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (text, title) = (wide(text), wide("1c-connect"));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(windows))]
fn fatal(text: &str) {
    eprintln!("{text}");
}

pub fn launch(kit: Kit, hidden: bool) -> anyhow::Result<()> {
    let cfg = Cfg::load(&kit)?;
    // ponytail: усечение сбросом, ротация — когда кому-то понадобится история
    let log_path = kit.path(LOG_FILE);
    if std::fs::metadata(&log_path).map(|m| m.len() > 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&log_path);
    }
    let autostart_on = autostart::is_on();
    // «Подключаться при запуске»: в туннельном режиме — решает, поднимать ли
    // сеанс при старте, как раньше; в локальном — сеанс поднимается всегда
    // (слушатели должны жить, пока жив процесс — см. `launch` ниже), а эта
    // галочка решает только начальное значение переключателя «MCP».
    let autoconnect_on = autoconnect(&kit);
    let local_mode = cfg.host.mode == "local";
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        // До `tray::build`: `install` красит и меню трея (`apply_menu_theme`),
        // а Win32 берёт режим в момент создания меню.
        theme::install(&kit, cx);
        // Окно закрыто — процесс живёт, пока «Выход» в трее.
        cx.set_quit_mode(QuitMode::Explicit);
        // Без опроса: обработчики трея (их зовёт WndProc на этом же потоке)
        // кладут событие в канал, задача GPUI просыпается по нему.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Ev>();
        // AppUserModelID до первого окна/уведомления — Windows привязывает
        // уведомления к процессу по нему.
        cx.set_app_identity("OktaPlus.1c-connect", "1c-connect");
        // Трей, который не удалось построить, здесь смертелен (см. `fatal`):
        // глобал `Ui` ещё не выставлен, поэтому выходим через `cx.quit()`
        // напрямую, а не через `quit()` этого модуля.
        let tray = match tray::init().and_then(|()| tray::build(tx.clone(), theme::Choice::load(&kit), autostart_on, autoconnect_on)) {
            Ok(t) => t,
            Err(e) => {
                fatal(&format!("Значок в трее не создался: {e:#}\nБез него программу нельзя закрыть, поэтому она завершается."));
                cx.quit();
                return;
            }
        };
        // Пункт подменю «Режим работы» строился без ведома сохранённого
        // режима и всегда отмечал «Через туннель» первым — здесь сразу
        // поправляется на настоящий, тем же методом, что потом зовёт смена
        // режима в трее (`tray::on_menu`).
        tray.set_mode(&cfg.host.mode);
        cx.set_global(Ui {
            kit,
            cfg,
            session: None,
            session_gen: 0,
            link: State::Off,
            enabled: Arc::new(AtomicBool::new(autoconnect_on)),
            log: Vec::new(),
            unread: 0,
            tab: Tab::default(),
            failed_checks: 0,
            ops: Vec::new(),
            project: None,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            dirty: false,
            busy: None,
            pending: 0,
            status: None,
            tray: Some(tray),
            window: None,
            panel: None,
            height: None,
            hidden_at: None,
            dialog_open: false,
            tooltip_at: None,
            tx,
        });
        // Единый тик приложения: сам решает свой срок (занято — секунда,
        // иначе полминуты) и заменяет прежний тик панели `Panel::new`, тот
        // жил только пока было открыто окно.
        // Один сон на `TICK_BUSY`, занятость — не до него, а после: иначе
        // операция, начавшаяся в окне простоя, молчала бы до следующего
        // тика (до 30 с) — именно первые полминуты, на которые смотрит
        // человек. Счётчик держит прежний ритм покоя (раз в `TICK_IDLE`).
        cx.spawn(async move |cx| {
            let mut idle = Duration::ZERO;
            loop {
                cx.background_executor().timer(TICK_BUSY).await;
                let busy = cx.update(|cx| {
                    let ui = cx.global::<Ui>();
                    ui.progress.is_some() || ui.busy.is_some()
                });
                idle = if busy { Duration::ZERO } else { idle + TICK_BUSY };
                if busy || idle >= TICK_IDLE {
                    idle = Duration::ZERO;
                    cx.update(|cx| tick(cx));
                }
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(ev) = rx.next().await {
                let _ = cx.update(|cx| handle(ev, cx));
            }
        })
        .detach();
        // Linux: значок трея живёт в GTK, и его главный цикл крутим мы сами,
        // на этом же потоке (`tray::pump`). Отдельная задача с коротким сном,
        // а не общий тик: тот просыпается раз в секунду и реже, и меню
        // отвечало бы с задержкой.
        #[cfg(target_os = "linux")]
        cx.spawn(async move |cx| loop {
            cx.background_executor().timer(tray::PUMP).await;
            cx.update(|_| tray::pump());
        })
        .detach();
        refresh_ops(cx);
        // В локальном режиме слушатели держат порты весь срок жизни
        // процесса (Zed не переподключается сам — см. заголовок модуля),
        // поэтому сеанс поднимается здесь безусловно; «подключаться при
        // запуске» в этом режиме уже отдано начальному значению `enabled`
        // чуть выше. В туннельном режиме — как раньше, по той же галочке.
        if local_mode || autoconnect_on {
            if let Err(e) = start_session(cx) {
                note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
            }
        }
        if !hidden {
            window::show_window(cx);
        }
    });
    Ok(())
}

#[cfg(test)]
impl Ui {
    fn for_test() -> Ui {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("onec-connect-gui-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        Ui {
            kit: Kit::at(dir),
            cfg: Cfg::default(),
            session: None,
            session_gen: 0,
            link: State::Off,
            enabled: Arc::new(AtomicBool::new(true)),
            log: Vec::new(),
            unread: 0,
            tab: Tab::default(),
            failed_checks: 0,
            ops: Vec::new(),
            project: None,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            dirty: false,
            busy: None,
            pending: 0,
            status: None,
            tray: None,
            window: None,
            panel: None,
            height: None,
            hidden_at: None,
            dialog_open: false,
            tooltip_at: None,
            tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        autoconnect, badge, can_cancel, carried_base_url, check_lines, first_phase, flip_enabled_only, is_stale, line_of,
        link_is_current, link_line, local_display, note_line, on_done, on_line, on_progress, progress_text, project_primary,
        publication_detail, save_host, stale_line, stale_text, took, tick_redraw, tray_state, tray_tooltip, Dot, Kind, Line,
        OpView, Running, Tab, TrayState, Ui, Which, LOG_CAP, LOG_FILE, PUBLICATION_TITLE, TRAY_PNG,
    };
    use crate::cfg::{self, Cfg, Host};
    use crate::session::{self, State};
    use common::check::Check;
    use common::{Event, Phase, Progress};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn op(id: &str, dot: Dot) -> OpView {
        OpView { id: id.into(), title: id.into(), dot, detail: String::new() }
    }

    #[test]
    fn three_tabs_in_the_order_the_window_draws_them() {
        assert_eq!(Tab::ALL, [Tab::Params, Tab::Project, Tab::Log]);
        assert_eq!(Tab::ALL.len(), 3);
        assert_eq!(Tab::default(), Tab::Params);
        for (i, t) in Tab::ALL.iter().enumerate() {
            assert_eq!(Tab::ALL.iter().position(|x| x == t), Some(i));
        }
    }

    #[test]
    fn the_project_button_installs_until_the_extension_is_up_then_exports() {
        // Ответа от `refresh_ops` ещё нет — ставить.
        assert_eq!(project_primary(&[]), Which::Install);
        assert_eq!(project_primary(&[op("host.tools", Dot::Up)]), Which::Install);
        assert_eq!(project_primary(&[op("host.extension", Dot::Off)]), Which::Install);
        assert_eq!(project_primary(&[op("host.extension", Dot::Down)]), Which::Install);
        // Расширение стоит — главная кнопка про обмен с git.
        assert_eq!(project_primary(&[op("host.tools", Dot::Off), op("host.extension", Dot::Up)]), Which::Export);
    }

    #[test]
    fn checks_print_into_the_log_as_facts_with_a_hint_on_its_own_line() {
        let lines = check_lines(&[
            Check::pass("база", "C:\\Bases\\Buh"),
            Check::note("туннель", "не поднят"),
            Check::fail("расширение", "не установлено", "нажмите «Установить»"),
        ]);
        assert_eq!(lines.len(), 4);
        // Значки из текста ушли — их теперь даёт `Kind::mark` при отрисовке.
        assert_eq!(lines[0], Line { kind: Kind::Ok, text: "база: C:\\Bases\\Buh".into() });
        assert_eq!(lines[1], Line { kind: Kind::Say, text: "туннель: не поднят".into() });
        assert_eq!(lines[2], Line { kind: Kind::Fail, text: "расширение: не установлено".into() });
        assert_eq!(lines[3], Line { kind: Kind::Warn, text: "нажмите «Установить»".into() });
        // Пустая подсказка второй строки не добавляет.
        assert_eq!(check_lines(&[Check::fail("а", "б", "")]).len(), 1);
    }

    #[test]
    fn kind_marks_are_icons_say_and_head_have_none() {
        assert_eq!(Kind::Ok.mark(), "✓");
        assert_eq!(Kind::Warn.mark(), "!");
        assert_eq!(Kind::Fail.mark(), "✗");
        assert_eq!(Kind::Say.mark(), "");
        assert_eq!(Kind::Head.mark(), "");
    }

    #[test]
    fn took_prints_seconds_under_a_minute_then_minutes_and_seconds() {
        assert_eq!(took(Duration::from_secs(12)), "12 с");
        assert_eq!(took(Duration::from_secs(72)), "1 мин 12 с");
        assert_eq!(took(Duration::from_secs(0)), "0 с");
    }

    #[test]
    fn note_line_appends_marks_and_head_lines_to_the_log_file() {
        let mut ui = Ui::for_test();
        note_line(&mut ui, Line { kind: Kind::Head, text: "— устанавливаю — · 12:34".into() });
        note_line(&mut ui, Line { kind: Kind::Ok, text: "готово".into() });
        let text = std::fs::read_to_string(ui.kit.path(LOG_FILE)).unwrap();
        assert_eq!(text, "\n— устанавливаю — · 12:34\n✓ готово\n");
    }

    #[test]
    fn the_badge_sits_in_the_corner_and_says_up_or_down() {
        let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
        let up = badge(&base, TrayState::Up);
        assert_eq!(up.dimensions(), base.dimensions());
        let w = up.width() as usize;
        let at = |img: &image::RgbaImage, x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[..3].to_vec();
        let c = w - 1 - (w as f32 * 0.22) as usize;
        assert_eq!(at(&up, c, c), vec![0x2f, 0x8f, 0x4e]);
        let down = badge(&base, TrayState::Off);
        assert_eq!(at(&down, c, c), vec![0x8d, 0x86, 0x7e]);
        let p = at(&down, 2, 2);
        assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    }

    #[test]
    fn the_busy_badge_is_yellow_and_not_greyed() {
        let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
        let busy = badge(&base, TrayState::Busy);
        let w = busy.width() as usize;
        let at = |img: &image::RgbaImage, x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[..3].to_vec();
        let c = w - 1 - (w as f32 * 0.22) as usize;
        assert_eq!(at(&busy, c, c), vec![0xfa, 0xcc, 0x1f]);
        // Занято — не обесцвечивается, в отличие от «без связи» (та же точка
        // вдали от кружка остаётся такой же, как в исходном PNG).
        assert_eq!(at(&busy, 2, 2), at(&base, 2, 2));
    }

    #[test]
    fn busy_wins_over_the_link_state() {
        let up = State::Up { since: Instant::now() };
        assert_eq!(tray_state(&up, true), TrayState::Busy);
        assert_eq!(tray_state(&up, false), TrayState::Up);
        assert_eq!(tray_state(&State::Off, false), TrayState::Off);
        assert_eq!(tray_state(&State::Connecting, false), TrayState::Off);
    }

    #[test]
    fn tooltip_names_the_phase_and_minutes() {
        let now = Instant::now() + Duration::from_secs(3600);
        let r = Running {
            which: Which::Export,
            started: now - Duration::from_secs(12 * 60),
            phase: Phase::Decompile,
            seen: now,
            stale_noted: false,
        };
        assert_eq!(tray_tooltip(Some(&r), &State::Up { since: now }, now), "1c-connect: выгружаю конфигурацию из 1С, 12 мин");
        let fresh = Running { started: now, seen: now, ..r };
        assert_eq!(tray_tooltip(Some(&fresh), &State::Off, now), "1c-connect: выгружаю конфигурацию из 1С, меньше минуты");
        assert_eq!(tray_tooltip(None, &State::Up { since: now }, now), "1c-connect: подключён");
        assert_eq!(tray_tooltip(None, &State::Off, now), "1c-connect: не подключён");
    }

    #[test]
    fn tick_redraws_when_the_window_is_open_or_something_changed() {
        // Окно открыто и в покое — минуты в карточке связи всё равно не
        // должны стоять, та считает их сама от `since.elapsed()`.
        assert!(tick_redraw(true, false, false));
        // Окно спрятано и тихо — перерисовывать нечего.
        assert!(!tick_redraw(false, false, false));
        // Спрятано, но пришли новые данные или идёт операция — всё равно да.
        assert!(tick_redraw(false, true, false));
        assert!(tick_redraw(false, false, true));
    }

    #[test]
    fn command_and_output_never_reach_the_window() {
        assert_eq!(line_of(&Event::Command("x".into())), None);
        assert_eq!(line_of(&Event::Output { command: "x".into(), text: "y".into() }), None);
        assert_eq!(line_of(&Event::Warn("w".into())), Some(Line { kind: Kind::Warn, text: "w".into() }));
    }

    #[test]
    fn the_log_keeps_the_last_two_thousand_lines_and_counts_unread() {
        let mut ui = Ui::for_test();
        for i in 0..LOG_CAP + 5 {
            note_line(&mut ui, Line { kind: Kind::Say, text: i.to_string() });
        }
        assert_eq!(ui.log.len(), LOG_CAP);
        assert_eq!(ui.log[0].text, "5");
        assert_eq!(ui.unread, LOG_CAP + 5);
        ui.tab = Tab::Log;
        ui.unread = 0; // окна нет — строки всё равно непрочитанные
        note_line(&mut ui, Line { kind: Kind::Say, text: "x".into() });
        assert_eq!(ui.unread, 1);
    }

    #[test]
    fn a_bad_server_leaves_the_toml_alone_and_says_why() {
        let mut ui = Ui::for_test();
        let before = std::fs::read_to_string(ui.kit.path(cfg::FILE)).ok();
        let mut host = ui.cfg.host.clone();
        host.server = "no-port".into();
        let err = save_host(&mut ui, host).unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(std::fs::read_to_string(ui.kit.path(cfg::FILE)).ok(), before);
    }

    #[test]
    fn carried_base_url_keeps_an_override_and_follows_the_default() {
        // Поля «Адрес публикации» в окне нет: умолчание идёт за каталогом
        // базы, а правка в toml переживает смену базы.
        let mut saved = Host { base_path: "C:\\Bases\\Buh".into(), base_url: cfg::derived_base_url("C:\\Bases\\Buh"), ..Host::default() };
        assert_eq!(carried_base_url(&saved, "C:\\Bases\\Torg"), "http://localhost/Torg/hs/mcp-1c");
        saved.base_url = "http://localhost:8081/InfoBase/hs/mcp-1c".into();
        assert_eq!(carried_base_url(&saved, "C:\\Bases\\Torg"), "http://localhost:8081/InfoBase/hs/mcp-1c");
        saved.base_url = String::new();
        assert_eq!(carried_base_url(&saved, "C:\\Bases\\Torg"), "http://localhost/Torg/hs/mcp-1c");
    }

    #[test]
    fn link_lines_say_where_and_as_whom() {
        let h = Host { server: "s:1".into(), workstation: "dev2".into(), ..Default::default() };
        assert_eq!(link_line(&State::Connecting, &h).unwrap().text, "подключаюсь к s:1 как dev2");
        assert_eq!(link_line(&State::Off, &h).unwrap().text, "отключено");
        assert!(link_line(&State::Up { since: Instant::now() }, &h).is_none());
    }

    #[test]
    fn a_panicking_worker_still_reports_the_end_of_the_work() {
        use super::{guarded, Ev, PANIC_TEXT};
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        guarded(&tx, || -> Ev { Ev::Done { err: None, cancelled: false } }, || -> Ev { unreachable!("тело не паниковало") });
        assert!(matches!(rx.try_recv().unwrap(), Ev::Done { err: None, cancelled: false }));

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        guarded(&tx, || -> Ev { panic!("ой") }, || Ev::Done { err: Some("сломалось".into()), cancelled: false });
        std::panic::set_hook(prev);
        match rx.try_recv().unwrap() {
            Ev::Line(l) => assert_eq!((l.kind, l.text.as_str()), (Kind::Fail, PANIC_TEXT)),
            _ => panic!("первой должна идти строка в журнал"),
        }
        // Без этого `busy` остался бы навсегда и кнопки — заблокированными.
        assert!(matches!(rx.try_recv().unwrap(), Ev::Done { err: Some(_), cancelled: false }));
    }

    #[test]
    fn which_titles_are_the_old_log_lines() {
        assert_eq!(Which::Install.title(), "— устанавливаю —");
        assert_eq!(Which::Uninstall.title(), "— снимаю расширение —");
        assert_eq!(Which::Export.title(), "— выгружаю из 1С —");
        assert_eq!(Which::Import.title(), "— загружаю в 1С —");
    }

    #[test]
    fn autoconnect_reads_the_kit_state_with_no_record_meaning_off() {
        let ui = Ui::for_test();
        assert!(!autoconnect(&ui.kit));
        ui.kit.state_set("autoconnect", "1").unwrap();
        assert!(autoconnect(&ui.kit));
        ui.kit.state_set("autoconnect", "0").unwrap();
        assert!(!autoconnect(&ui.kit));
    }

    #[test]
    fn working_is_true_while_busy_connecting_or_a_background_refresh_is_in_flight() {
        use super::Busy;
        let mut ui = Ui::for_test();
        assert!(!ui.working(), "покой");
        ui.busy = Some(Busy::Check);
        assert!(ui.working(), "операция/проверка идёт");
        ui.busy = None;
        ui.link = State::Connecting;
        assert!(ui.working(), "подключение туннеля идёт");
        ui.link = State::Off;
        assert!(!ui.working());
        ui.pending = 1;
        assert!(ui.working(), "фоновое обновление состояния идёт");
        ui.pending = 0;
        assert!(!ui.working());
    }

    #[test]
    fn a_stale_session_generation_is_not_current() {
        let mut ui = Ui::for_test();
        ui.session_gen = 2;
        assert!(!link_is_current(&ui, 1), "поколение 1 устарело, текущее — 2");
        assert!(link_is_current(&ui, 2));
    }

    #[test]
    fn first_phase_of_each_operation() {
        assert_eq!(first_phase(Which::Export), Phase::Decompile);
        assert_eq!(first_phase(Which::Import), Phase::Pull);
        assert_eq!(first_phase(Which::Install), Phase::Install);
        assert_eq!(first_phase(Which::Uninstall), Phase::Uninstall);
    }

    #[test]
    fn progress_text_shows_title_and_elapsed() {
        // Anchored an hour into the future: a fresh CI runner can have under
        // an hour of uptime, and `Instant::now() - Duration::from_secs(3725)`
        // panics (underflow) once the subtracted duration outruns it.
        let now = Instant::now() + Duration::from_secs(3600);
        let at = |secs| Running { which: Which::Export, started: now - Duration::from_secs(secs), phase: Phase::Decompile, seen: now, stale_noted: false };
        let (title, clock) = progress_text(&at(725), now);
        assert_eq!(title, "Выгружаю конфигурацию из 1С");
        assert_eq!(clock, "12:05");
        assert_eq!(progress_text(&at(5), now).1, "0:05");
        assert_eq!(progress_text(&at(65), now).1, "1:05");
        assert_eq!(progress_text(&at(3725), now).1, "1:02:05");
    }

    #[test]
    fn stale_after_five_quiet_minutes() {
        // Same underflow risk as above: anchor `now` well past any real
        // clock value before subtracting minutes from it.
        let now = Instant::now() + Duration::from_secs(3600);
        let fresh = Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(299), stale_noted: false };
        assert!(!is_stale(&fresh, now));
        let quiet = Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(301), stale_noted: false };
        assert!(is_stale(&quiet, now));
        assert_eq!(stale_text(&quiet, now), "нет новостей 5 мин");
    }

    #[test]
    fn stale_line_fires_once_per_operation() {
        // Карточка не показывает застой (заказчик 2026-10-19) — он уходит в
        // журнал одной строкой `Kind::Warn`, не чаще раза на операцию.
        let now = Instant::now() + Duration::from_secs(3600);
        let mut fresh = Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false };
        assert_eq!(stale_line(&mut fresh, now), None);
        let mut quiet = Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(301), stale_noted: false };
        assert_eq!(stale_line(&mut quiet, now), Some(Line { kind: Kind::Warn, text: "нет новостей 5 мин".into() }));
        assert!(quiet.stale_noted);
        // Второй вызов на том же застое — ничего не шлёт.
        assert_eq!(stale_line(&mut quiet, now), None);
    }

    #[test]
    fn only_git_phases_can_be_cancelled() {
        assert!(can_cancel(Phase::Push));
        assert!(can_cancel(Phase::Pull));
        for p in [Phase::Decompile, Phase::Load, Phase::Install, Phase::Uninstall, Phase::Check] {
            assert!(!can_cancel(p), "{p:?}");
        }
    }

    #[test]
    fn done_clears_progress_and_renews_cancel() {
        let mut ui = Ui::for_test();
        let now = Instant::now();
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false });
        let old_cancel = ui.cancel.clone();
        old_cancel.store(true, Ordering::Relaxed);
        on_done(&mut ui, None, false);
        assert!(ui.progress.is_none());
        assert!(!ui.cancel.load(Ordering::Relaxed));
        assert!(!Arc::ptr_eq(&old_cancel, &ui.cancel));
    }

    #[test]
    fn a_progress_event_moves_phase_and_seen() {
        let mut ui = Ui::for_test();
        // `on_progress` now takes `now` as a parameter instead of reading
        // the real clock itself: a plain `assert!(r.seen >= old_seen)`
        // against the real clock would hold even if the function never
        // touched `seen` at all, since real time never runs backwards.
        // Anchored, so the `- 10s` below can't underflow a short-lived
        // runner's clock either.
        let now = Instant::now() + Duration::from_secs(3600);
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Decompile, seen: now - Duration::from_secs(10), stale_noted: false });
        on_progress(&mut ui, Progress { phase: Phase::Push, done: None, total: None, note: "x".into() }, now);
        let r = ui.progress.as_ref().unwrap();
        assert_eq!(r.phase, Phase::Push);
        assert_eq!(r.seen, now);
        assert!(ui.dirty);
    }

    #[test]
    fn a_line_during_an_operation_reaches_the_journal_and_moves_seen() {
        // B1: `Ev::Line` while `progress.is_some()` used to move the line
        // into `r.note` only — `note_line` (the sole caller of
        // `write_log_file`) was never reached, so the journal (window and
        // `1c-connect.log`) stayed empty for the whole operation.
        let mut ui = Ui::for_test();
        let now = Instant::now() + Duration::from_secs(3600);
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Decompile, seen: now - Duration::from_secs(10), stale_noted: false });
        let line = Line { kind: Kind::Say, text: "cf_decompile → src (минуты…)".into() };
        on_line(&mut ui, line.clone(), now);
        assert_eq!(ui.log.last(), Some(&line));
        let r = ui.progress.as_ref().unwrap();
        assert_eq!(r.seen, now);
        assert!(ui.dirty);
    }

    #[test]
    fn a_cancelled_done_is_a_warning_not_a_failure() {
        let mut ui = Ui::for_test();
        let now = Instant::now();
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false });
        let line = on_done(&mut ui, Some("прервано".into()), true);
        note_line(&mut ui, line.clone());
        ui.status = Some(line);
        assert_eq!(ui.log.last(), Some(&Line { kind: Kind::Warn, text: "прервано".into() }));
        assert_eq!(ui.status, Some(Line { kind: Kind::Warn, text: "прервано".into() }));
    }

    /// Сеанс, которому заведомо некуда дозвониться (порт только что закрыт) —
    /// тот же приём, что в `session.rs`: тест не ждёт настоящей связи, ему
    /// важно лишь то, что `ui.session` не пусто.
    fn doomed_session(ui: &Ui) -> crate::session::Session {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let mut cfg = ui.cfg.clone();
        cfg.host.server = format!("127.0.0.1:{port}");
        session::start_with_enabled(cfg, ui.kit.clone(), Arc::new(|_| {}), ui.enabled.clone(), |_| {})
    }

    #[test]
    fn in_local_mode_the_switch_only_flips_the_flag() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.session = Some(doomed_session(&ui));
        let gen_before = ui.session_gen;
        assert!(ui.enabled.load(Ordering::Relaxed), "фикстура заводит флаг включённым");
        assert!(!flip_enabled_only(&mut ui));
        assert!(!ui.enabled.load(Ordering::Relaxed));
        assert!(flip_enabled_only(&mut ui));
        assert!(ui.enabled.load(Ordering::Relaxed));
        // Локальный режим: слушатели живут своей жизнью, переключатель их не
        // трогает — поколение сеанса ровно то же, что и до двух щелчков.
        assert_eq!(ui.session_gen, gen_before);
        let _ = ui.session.take().unwrap().stop();
    }

    #[test]
    fn local_mode_hides_up_behind_the_flag_but_shows_a_failure_regardless() {
        let now = Instant::now();
        assert_eq!(local_display("local", false, State::Up { since: now }), State::Off);
        assert_eq!(local_display("local", false, State::Connecting), State::Off);
        assert_eq!(local_display("local", true, State::Up { since: now }), State::Up { since: now });
        let failed = State::Failed("порт занят".into());
        assert_eq!(local_display("local", false, failed.clone()), failed);
        // Туннельный режим — состояние сеанса идёт как есть, флаг тут ни при чём.
        assert_eq!(local_display("tunnel", false, State::Up { since: now }), State::Up { since: now });
    }

    #[test]
    fn saving_a_new_base_restarts_the_session_and_writes_one_journal_line() {
        let mut ui = Ui::for_test();
        ui.session = Some(doomed_session(&ui));
        let gen_before = ui.session_gen;
        let log_before = ui.log.len();
        let mut host = ui.cfg.host.clone();
        host.base_path = "C:\\Bases\\Other".into();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.session_gen, gen_before + 1, "сеанс перезапущен один раз");
        let new_lines = &ui.log[log_before..];
        assert_eq!(new_lines.len(), 1, "{new_lines:?}");
        assert_eq!(new_lines[0].text, "MCP перезапущены: база C:\\Bases\\Other");
        assert!(ui.session.is_some());
        let _ = ui.session.take().unwrap().stop();
    }

    #[test]
    fn saving_unchanged_fields_does_not_restart_a_live_session() {
        // Смена режима в трее меняет `cfg.host.mode` и зовёт `save_host`
        // напрямую (не через панель), не трогая поля, которые показывает
        // окно — `tray::on_menu` перезапускает сеанс сама следом, отдельным
        // путём; лишнего перезапуска (и лишней строки «MCP перезапущены») на
        // стороне `save_host` здесь быть не должно.
        let mut ui = Ui::for_test();
        ui.session = Some(doomed_session(&ui));
        let gen_before = ui.session_gen;
        let log_before = ui.log.len();
        let mut host = ui.cfg.host.clone();
        host.mode = "local".into();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.session_gen, gen_before);
        assert_eq!(ui.log.len(), log_before);
        let _ = ui.session.take().unwrap().stop();
    }

    #[test]
    fn project_tab_shows_the_publication_row() {
        // Заголовок и адрес понятны 1С-разработчику независимо от режима
        // подключения («local»/«tunnel») — публикация не про туннель.
        assert_eq!(PUBLICATION_TITLE, "HTTP-сервис базы");
        for mode in ["local", "tunnel"] {
            let mut cfg = Cfg { host: Host { mode: mode.into(), publication: "auto".into(), ..Host::default() } };
            let want = format!("127.0.0.1:{}", cfg.http_port());
            let detail = publication_detail(&cfg);
            assert!(detail.contains(&want), "{mode}: {detail:?}");
            cfg.host.publication = "webserver".into();
            assert_eq!(publication_detail(&cfg), "публикация настроена вручную");
        }
    }

    #[test]
    fn save_host_without_a_live_session_writes_no_restart_line() {
        // Поднятого сеанса нет — перезаписать toml, и всё; журнал не трогаем.
        let mut ui = Ui::for_test();
        let log_before = ui.log.len();
        let mut host = ui.cfg.host.clone();
        host.base_path = "C:\\Bases\\Other".into();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.log.len(), log_before);
    }
}
