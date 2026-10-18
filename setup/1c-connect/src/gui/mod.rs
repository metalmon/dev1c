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

/// Значок трея со статусом: кружок в правом нижнем углу, зелёный —
/// подключён; без связи значок обесцвечен и кружок серый.
pub fn badge(base: &image::RgbaImage, up: bool) -> image::RgbaImage {
    let (w, h) = base.dimensions();
    let mut out = base.clone();
    if !up {
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
    let colour: [u8; 4] = if up { [0x2f, 0x8f, 0x4e, 255] } else { [0x8d, 0x86, 0x7e, 255] };
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

pub fn web_url(cfg: &Cfg) -> String {
    let port = cfg.web_port();
    let host = if port == 80 { "localhost".to_string() } else { format!("localhost:{port}") };
    format!("http://{host}{}", cfg.publication())
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
            Which::Export => "— выгружаю в git —",
            Which::Import => "— загружаю из git —",
        }
    }
}

/// Операция на ходу: с чего начали, какая фаза сейчас и когда от неё
/// последний раз были вести (строка в журнал или `Event::Progress`).
pub struct Running {
    pub which: Which,
    pub started: Instant,
    pub phase: Phase,
    pub note: String,
    pub seen: Instant,
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

/// Прошло с начала операции, для карточки хода: секунды с ведущим нулём,
/// как на часах («05 с»); от минуты — минуты и секунды («12 мин 05 с»).
/// Не `took`: та не дополняет секунды нулём, а эта всегда с ним.
fn running_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs:02} с")
    } else {
        format!("{} мин {:02} с", secs / 60, secs % 60)
    }
}

/// Заголовок карточки хода (название фазы) и сколько она уже идёт.
pub fn progress_text(r: &Running, now: Instant) -> (String, String) {
    (r.phase.title().to_string(), running_elapsed(now.saturating_duration_since(r.started)))
}

/// Пять минут без вестей от потока — поток молчит, но операция ещё жива.
const QUIET: Duration = Duration::from_secs(5 * 60);

pub fn is_stale(r: &Running, now: Instant) -> bool {
    now.saturating_duration_since(r.seen) > QUIET
}

pub fn stale_text(r: &Running, now: Instant) -> String {
    format!("нет новостей {} мин", now.saturating_duration_since(r.seen).as_secs() / 60)
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

/// Сохранить параметры сервера. Списка инструментов, каталога выгрузки и
/// адреса публикации в окне нет (правятся в toml): берём их из текущих
/// параметров, иначе пустые значения из формы затёрли бы toml. Каталог
/// проекта — из формы.
pub fn save_host(ui: &mut Ui, mut host: Host) -> Result<(), String> {
    host.dump = ui.cfg.host.dump.clone();
    host.tools = ui.cfg.host.tools.clone();
    host.base_url = carried_base_url(&ui.cfg.host, &host.base_path);
    let cfg = Cfg { host };
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    write_text(&ui.kit.path(cfg::FILE), &cfg.render()).map_err(|e| format!("{e:#}"))?;
    ui.cfg = cfg;
    Ok(())
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

/// Переход сеанса: в состояние, в журнал, в трей, перерисовать окно.
fn set_link(cx: &mut App, l: State) {
    let host = cx.global::<Ui>().cfg.host.clone();
    let line = link_line(&l, &host);
    let ui = cx.global_mut::<Ui>();
    ui.link = l.clone();
    if let Some(line) = line {
        note_line(ui, line);
    }
    if let Some(tray) = &ui.tray {
        tray.set_state(l.is_up(), l.is_on());
    }
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
    // в том числе в окно между `session::start` и первым `Connecting`.
    if ui.session.is_some() && !matches!(ui.link, State::Failed(_)) {
        return Ok(());
    }
    // Новый сеанс — новое поколение: хвост от прежнего (если он всё ещё
    // договаривает) будет отбрасываться в `handle` по несовпадению.
    ui.session_gen += 1;
    let gen = ui.session_gen;
    let ctx = ctx_for(ui);
    let (cfg, kit, tx) = (ui.cfg.clone(), ui.kit.clone(), ui.tx.clone());
    let session = session::start(cfg, kit, ctx.sink, move |l| {
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

pub fn toggle_link(cx: &mut App) {
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
    ui.progress = Some(Running { which, started: now, phase: first_phase(which), note: String::new(), seen: now });
    ui.cancel = cancel.clone();
    ui.tab = Tab::Log;
    ui.unread = 0;
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
    let tx = ui.tx.clone();
    cx.refresh_windows();
    std::thread::spawn(move || {
        guarded(
            &tx,
            || {
                let state = link.is_up().then(|| link.label());
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

pub fn open_web(cx: &mut App) {
    let url = web_url(&cx.global::<Ui>().cfg);
    cx.open_url(&url);
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

/// `Ev::Progress`: фаза/заметка идут в карточку хода, не в журнал — тот
/// получил свою строку с заголовком фазы через `line_of` уже отдельно.
fn on_progress(ui: &mut Ui, p: Progress) {
    if let Some(r) = ui.progress.as_mut() {
        r.phase = p.phase;
        r.note = p.note;
        r.seen = Instant::now();
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

fn handle(ev: Ev, cx: &mut App) {
    match ev {
        Ev::Tray(e) => tray::on_tray(e, cx),
        Ev::Menu(e) => tray::on_menu(e, cx),
        Ev::Link(gen, l) => {
            if link_is_current(cx.global::<Ui>(), gen) {
                set_link(cx, l);
            }
        }
        Ev::Line(line) => {
            let ui = cx.global_mut::<Ui>();
            if let Some(r) = ui.progress.as_mut() {
                r.note = line.text;
                r.seen = Instant::now();
                ui.dirty = true;
            } else {
                note_line(ui, line);
                cx.refresh_windows();
            }
        }
        Ev::Progress(p) => on_progress(cx.global_mut::<Ui>(), p),
        Ev::Done { err, cancelled } => {
            let ui = cx.global_mut::<Ui>();
            let line = on_done(ui, err, cancelled);
            note_line(ui, line.clone());
            ui.status = Some(line);
            cx.refresh_windows();
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
    let autoconnect_on = autoconnect(&kit);
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
        // Трей, который не удалось построить, здесь смертелен (см. `fatal`):
        // глобал `Ui` ещё не выставлен, поэтому выходим через `cx.quit()`
        // напрямую, а не через `quit()` этого модуля.
        let tray = match tray::build(tx.clone(), theme::Choice::load(&kit), autostart_on, autoconnect_on) {
            Ok(t) => t,
            Err(e) => {
                fatal(&format!("Значок в трее не создался: {e:#}\nБез него программу нельзя закрыть, поэтому она завершается."));
                cx.quit();
                return;
            }
        };
        cx.set_global(Ui {
            kit,
            cfg,
            session: None,
            session_gen: 0,
            link: State::Off,
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
            tx,
        });
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(ev) = rx.next().await {
                let _ = cx.update(|cx| handle(ev, cx));
            }
        })
        .detach();
        refresh_ops(cx);
        if autoconnect_on {
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
            tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        autoconnect, badge, can_cancel, carried_base_url, check_lines, first_phase, is_stale, line_of, link_is_current,
        link_line, note_line, on_done, on_progress, progress_text, project_primary, save_host, stale_text, took, Dot,
        Kind, Line, OpView, Running, Tab, Ui, Which, LOG_CAP, LOG_FILE, TRAY_PNG,
    };
    use crate::cfg::{self, Cfg, Host};
    use crate::session::State;
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
        let up = badge(&base, true);
        assert_eq!(up.dimensions(), base.dimensions());
        let w = up.width() as usize;
        let at = |img: &image::RgbaImage, x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[..3].to_vec();
        let c = w - 1 - (w as f32 * 0.22) as usize;
        assert_eq!(at(&up, c, c), vec![0x2f, 0x8f, 0x4e]);
        let down = badge(&base, false);
        assert_eq!(at(&down, c, c), vec![0x8d, 0x86, 0x7e]);
        let p = at(&down, 2, 2);
        assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    }

    #[test]
    fn web_client_url_follows_the_publication() {
        let mut c = Cfg::default();
        assert_eq!(super::web_url(&c), "http://localhost/Buh/");
        c.host.base_url = "http://localhost:8081/InfoBase/hs/mcp-1c".into();
        assert_eq!(super::web_url(&c), "http://localhost:8081/InfoBase/");
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
        assert_eq!(Which::Export.title(), "— выгружаю в git —");
        assert_eq!(Which::Import.title(), "— загружаю из git —");
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
        let now = Instant::now();
        let r = Running { which: Which::Export, started: now - Duration::from_secs(725), phase: Phase::Decompile, note: String::new(), seen: now };
        let (title, elapsed) = progress_text(&r, now);
        assert_eq!(title, "Выгружаю конфигурацию");
        assert_eq!(elapsed, "12 мин 05 с");
        let r5 = Running { which: Which::Export, started: now - Duration::from_secs(5), phase: Phase::Decompile, note: String::new(), seen: now };
        assert_eq!(progress_text(&r5, now).1, "05 с");
    }

    #[test]
    fn stale_after_five_quiet_minutes() {
        let now = Instant::now();
        let fresh = Running { which: Which::Export, started: now, phase: Phase::Push, note: String::new(), seen: now - Duration::from_secs(299) };
        assert!(!is_stale(&fresh, now));
        let quiet = Running { which: Which::Export, started: now, phase: Phase::Push, note: String::new(), seen: now - Duration::from_secs(301) };
        assert!(is_stale(&quiet, now));
        assert_eq!(stale_text(&quiet, now), "нет новостей 5 мин");
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
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, note: String::new(), seen: now });
        let old_cancel = ui.cancel.clone();
        old_cancel.store(true, Ordering::Relaxed);
        on_done(&mut ui, None, false);
        assert!(ui.progress.is_none());
        assert!(!ui.cancel.load(Ordering::Relaxed));
        assert!(!Arc::ptr_eq(&old_cancel, &ui.cancel));
    }

    #[test]
    fn a_progress_event_moves_phase_note_and_seen() {
        let mut ui = Ui::for_test();
        let old_seen = Instant::now() - Duration::from_secs(10);
        ui.progress = Some(Running { which: Which::Export, started: old_seen, phase: Phase::Decompile, note: String::new(), seen: old_seen });
        on_progress(&mut ui, Progress { phase: Phase::Push, done: None, total: None, note: "x".into() });
        let r = ui.progress.as_ref().unwrap();
        assert_eq!(r.phase, Phase::Push);
        assert_eq!(r.note, "x");
        assert!(r.seen > old_seen);
        assert!(ui.dirty);
    }

    #[test]
    fn a_cancelled_done_is_a_warning_not_a_failure() {
        let mut ui = Ui::for_test();
        let now = Instant::now();
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, note: String::new(), seen: now });
        let line = on_done(&mut ui, Some("прервано".into()), true);
        note_line(&mut ui, line.clone());
        ui.status = Some(line);
        assert_eq!(ui.log.last(), Some(&Line { kind: Kind::Warn, text: "прервано".into() }));
        assert_eq!(ui.status, Some(Line { kind: Kind::Warn, text: "прервано".into() }));
    }
}
