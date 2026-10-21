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
pub mod gate;
pub mod panel;
pub mod theme;
pub mod tray;
pub mod window;

pub const TRAY_PNG: &[u8] = include_bytes!("../../icons/tray.png");

/// Четыре состояния трея. Своего мнения у них нет: какое сейчас — решает
/// точка карточки (`tray_state_now` ниже переводит `Dot` в бейдж), поэтому
/// значок и окно расходиться не могут и порядок разрешения описан в одном
/// месте — `panel::card`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrayState {
    Off,
    Up,
    /// Операция идёт (или связь поднимается) — жёлтый.
    Busy,
    /// Вердикт готовности провалился — оранжевый: «не всё готово», а не
    /// «занято» и не «всё хорошо». Зелёным это состояние не бывает — ради
    /// этого и заведено (спека, «Трей живёт по тем же правилам»).
    Warn,
}

/// Что значок показывает: бейдж, положение переключателя в меню, заперт ли
/// пункт «Включить MCP» и подсказка. Собирается в `sync_tray` одним местом —
/// и отдаётся значку, и кладётся в `Ui`, чтобы было чем сторожить проводку.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrayView {
    pub state: TrayState,
    pub on: bool,
    pub locked: bool,
    pub tooltip: String,
}

/// Значок трея со статусом: кружок в правом нижнем углу — зелёный (подключён),
/// жёлтый (занят), оранжевый (не всё готово) или серый (без связи); обесцвечен
/// значок только без связи — занятость и провал вердикта тоже заметны цветным
/// значком.
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
        // Провал вердикта — оранжевый: рядом с жёлтым «занято» он различим,
        // а с зелёным «всё хорошо» не спутаешь и краем глаза.
        TrayState::Warn => [0xe8, 0x7a, 0x14, 255],
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
    /// Настоящий отказ bind локального слушателя («порт занят» и т.п.),
    /// независимый от переключателя: слушатели в локальном режиме не
    /// поднимаются заново ни «включить», ни «выключить» (см. `launch`), так
    /// что ни один клик его не чинит — только заводится (`Ev::Link` с
    /// `Failed`) и снимается (`Ev::Link` с `Up`) настоящим сеансом. Без
    /// отдельного поля `toggle_link` стирал бы `Failed` в `Off`/`Up` первым
    /// же щелчком, и второй клик уже не видел бы отказа (находка повторного
    /// ревью: прежний `keep_failure_on_enable` ловил лишь один клик).
    pub bind_failed: Option<String>,
    /// Обслуживаются ли вызовы агента: живёт дольше любого сеанса (заводится
    /// один раз в `launch`, `start_session`/`stop_session`/«Сохранить» его не
    /// пересоздают), передаётся в `session::Serve` через `session::start_with_enabled` и
    /// читается мостом (`bridge::Rules`) на каждый вызов. В локальном режиме
    /// переключатель окна меняет только этот флаг — слушатели остаются
    /// поднятыми; в туннельном — как раньше, переключатель поднимает и
    /// опускает сеанс целиком.
    pub enabled: Arc<AtomicBool>,
    /// Холодный замок (`gate::cold_gate`): `None` — включать можно,
    /// `Some(причина)` — включать нечего. Считается не на каждую отрисовку, а
    /// по тем же событиям, что и вердикт готовности (`refresh_gate` в
    /// `run_readiness`): за ним стоит диск — платформа, комплект, инструменты.
    pub gate: Option<String>,
    /// Итог последнего расчёта готовности — см. `Readiness` и
    /// `run_readiness`.
    pub ready: Readiness,
    /// На какой час верен вердикт (`gui::clock`, «15:54»); `None` — вердикта
    /// нет (`Readiness::Unknown`). Вердикт пересчитывается по событиям, а не
    /// на таймере (спека, «Чем за это платим»), значит бывает устаревшим —
    /// и человек должен видеть, на какой момент он верен. Пишется там же,
    /// где вердикт становится известен: `apply_ready` и `verdict_from_checks`.
    pub ready_at: Option<String>,
    /// Поколение вердикта: растёт на каждый начатый расчёт и на каждый итог
    /// «Проверить». Ответ чужого поколения отбрасывается — так же, как
    /// хвостовое событие прежнего сеанса (`session_gen`): набор считается
    /// десяток секунд, и окно для гонки широкое.
    pub ready_gen: u64,
    /// Поколение расчёта, который прямо сейчас держит защёлку
    /// `ready_pending`. Не то же, что `ready_gen`: тот двигает и
    /// «Проверить», ничего не запуская, — а защёлку обязан снимать
    /// только свой поток (находка N1 повторного ревью).
    pub ready_run: u64,
    /// Расчёт готовности идёт прямо сейчас — второй поток не заводится.
    pub ready_pending: bool,
    /// Когда начался идущий расчёт. Нужен сторожу `readiness_lost`: поток
    /// может не ответить никогда (бюджет проверяется **до** факта, а не
    /// прерывает его, и `holder::of_port` поднимает `ss`/`netstat` вовсе без
    /// срока), а залипшая защёлка `ready_pending` означала бы вердикт,
    /// который больше не посчитается ни разу, и точку, которая никогда не
    /// станет зелёной (находка S10).
    pub ready_since: Option<Instant>,
    /// Пока поток считал, почва сдвинулась ещё раз: пересчитать один раз по
    /// его возвращении. Иначе цепочка «Сохранить → публикация → операция»
    /// поднимала бы три пробы подряд.
    pub ready_again: bool,
    /// Состояние заглушки лицензии, о котором уже сказано в журнале. Решений
    /// на нём не принимают — и кнопки (`run_op`), и мост спрашивают часы
    /// заново; поле нужно только чтобы заметить смену под открытым окном и
    /// сказать о ней один раз, а не каждый тик.
    pub licence: crate::licence::State,
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
    /// Что подвал говорит по наведению сверх самого итога — путь отчёта
    /// после проверки. Сам итог короткий (`check_status`): длинный путь
    /// рядом с кнопкой не помещается, а потерять его нельзя. Ставится и
    /// снимается только вместе со `status` (`set_status`), иначе подсказка
    /// пережила бы итог, к которому её написали.
    pub status_tip: Option<String>,
    pub tray: Option<tray::Tray>,
    /// Что значок показывает прямо сейчас — последнее, что ему отдала
    /// `sync_tray`. Нужно не значку (он помнит сам), а тесту: без этого
    /// «дошло ли до трея» не отличить от «не дошло», и проводка остаётся
    /// непокрытой (находки S1/S2 итогового ревью). `None` — `sync_tray` ещё
    /// ни разу не звали.
    pub tray_shown: Option<TrayView>,
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
    /// MCP включены, но быстрая проверка (`verify::readiness`) нашла
    /// провалившийся факт — карточка жёлтая, а не зелёная или красная:
    /// обслуживание идёт, просто не всё готово. Провалиться может и то, что
    /// базы не касается (проект не разложен, среда не отвечает), поэтому
    /// заголовок говорит общее «Не всё готово», а что именно — говорит
    /// причина рядом (`panel::reason`).
    Warn,
}

/// Итог быстрой проверки «дойдёт ли агент до базы» (`verify::readiness`),
/// пересчитывается только по событию, что могло сдвинуть почву — включение
/// MCP, автозапуск с уже включённым переключателем, конец «Установить» и
/// «Сохранить» (см. `run_readiness` и её вызовы), не на таймере.
#[derive(Clone, Debug)]
pub enum Readiness {
    /// Переключатель выключен либо поток ещё считает. Зелёным это не бывает:
    /// при включённом обслуживании карточка показывает «Проверяю…» с точкой
    /// «занято» (спека: «Зелёной точки до вердикта готовности не бывает»), а
    /// при выключенном — обычное «MCP выключены», потому что вердикт тогда и
    /// не считается.
    Unknown,
    Ok,
    /// Первый провалившийся факт — его `what`/`detail`/`hint` и есть причина,
    /// которую видит человек в карточке.
    Warn(Check),
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
    /// Ответ `ensure_publication`, поднятой в своём потоке: проба (до 3 с),
    /// проверка расширения в режиме «webserver» и ожидание после spawn не
    /// должны держать главный поток окна (находка повторного ревью).
    Publish(Result<(), String>),
    /// Поколение расчёта и его вердикт — первый провалившийся факт
    /// `verify::readiness`, посчитанный в своём потоке (`run_readiness`);
    /// `None` — пройдено.
    Ready(u64, Option<Check>),
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

/// Отказ операции, меняющей машину, по истёкшему сроку: `Some` — начинать
/// нельзя, строка уходит в журнал. Кнопки окна ведут к тем же четырём
/// операциям, что `install`/`uninstall`/`export`/`import` в командной строке
/// (`Which` — ровно они), и ворота у них должны быть одни: окно и есть
/// главный путь человека к ним, запрет только в argv ничего не запрещает.
/// Пока срок идёт — даже в последний его день — операции работают как обычно.
fn op_refusal(state: crate::licence::State) -> Option<Line> {
    matches!(state, crate::licence::State::Expired).then(|| Line { kind: Kind::Fail, text: crate::licence::REFUSAL.to_string() })
}

/// Строка журнала о сроке заглушки лицензии: вышел — отказ его текстом,
/// скоро выйдет — сколько осталось, иначе молчим. Молча умирающая программа —
/// худшее, что можно сделать (спека, п. 9), поэтому окно открывается и
/// объясняет. Отдельная чистая функция — чтобы решение «что сказать» читалось
/// без живого окна.
fn licence_line(state: crate::licence::State) -> Option<Line> {
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
fn note_licence_change(cx: &mut App) {
    note_licence_change_ui(cx.global_mut::<Ui>());
}

/// То же без `cx`: смена срока — такое же событие, меняющее почву, как
/// «Сохранить» или конец операции, и кончается тем же пересчётом.
fn note_licence_change_ui(ui: &mut Ui) {
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
/// Причина замка попадает в подсказку значка наравне с фазой операции; идущая
/// операция побеждает — тот же порядок разрешения, что в карточке.
pub fn tray_tooltip(progress: Option<&Running>, link: &State, gate: Option<&str>, now: Instant) -> String {
    if let (None, Some(why)) = (progress, gate) {
        return format!("1c-connect: включать нечего — {why}");
    }
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

/// Строка перехода сеанса — тексты как в сегодняшнем `gui.rs::set_link`.
/// Начало и конец видны и в журнале: туннель сам пишет только про поднятую
/// связь и обрывы, а отказ возвращает ошибкой. `Connecting` — по режиму:
/// адрес среды, пароль и имя рабочего места в локальном режиме скрыты в
/// «Параметрах» (`panel::shown_fields`) и ничего не значат — печатать их в
/// журнал всё равно было бы находкой целостного ревью (строка говорила про
/// туннель, которого там нет).
pub fn link_line(l: &State, host: &Host, mode: &str) -> Option<Line> {
    match l {
        State::Connecting => {
            let text = if mode == "local" { "включаю MCP".to_string() } else { format!("подключаюсь к {} как {}", host.server, host.workstation) };
            Some(Line { kind: Kind::Say, text })
        }
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

/// Строка только в файл журнала (`1c-connect.log`), мимо окна: диагностика
/// для разбора по снимку, которую человеку читать незачем (геометрия, обмеры).
/// В журнале окна — только то, что человеку надо прочесть; пометка `Kind::Say`
/// сама по себе не повод показывать строку, выбирает канал тот, кто пишет.
pub fn note_file(kit: &Kit, line: &Line) {
    write_log_file(kit, line);
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
        // вкладки «Проверка», обычная строка, а не провал. «Не проверено»
        // (`!ok` при `info`, см. `verify::not_checked`) — посередине:
        // предупреждение, а не красное, иначе сорванный срок читался бы как
        // поломка.
        let kind = match (c.ok, c.info) {
            (true, false) => Kind::Ok,
            (true, true) => Kind::Say,
            (false, true) => Kind::Warn,
            _ => Kind::Fail,
        };
        out.push(Line { kind, text: format!("{}: {}", c.what, c.detail) });
        if kind == Kind::Fail && !c.hint.is_empty() {
            out.push(Line { kind: Kind::Warn, text: c.hint.clone() });
        }
    }
    out
}

/// Итог проверки одной строкой — в подвал и последней строкой раздела
/// журнала. Только числа: путь отчёта сюда не идёт (заказчик 2026-10-21 —
/// на снимке строка обрывалась на «Проблем: 2. Подробности — отчё…»), его
/// называет отдельная строка журнала (`report_line`), она же — подсказка
/// подвала.
pub fn check_status(failed: usize) -> Line {
    if failed > 0 {
        Line { kind: Kind::Fail, text: format!("Проблем: {failed}") }
    } else {
        Line { kind: Kind::Ok, text: "Проверка пройдена".into() }
    }
}

/// Итог последнего действия в подвал — вместе с подсказкой к нему. Одной
/// функцией, чтобы подсказка прошлого итога не осталась висеть на следующем.
pub fn set_status(ui: &mut Ui, line: Line, tip: Option<String>) {
    ui.status = Some(line);
    ui.status_tip = tip;
}

/// Где лежит отчёт проверки — строкой журнала и подсказкой подвала. «В
/// комплекте» он лежит только на Windows: на Linux комплект общий и только
/// для чтения, отчёт уходит в состояние, поэтому путь называется целиком.
pub fn report_line(report: &str) -> Line {
    Line { kind: Kind::Say, text: format!("Отчёт — {report}") }
}

/// Причина, по которой карточка жёлтая (`Dot::Warn`): первый провалившийся
/// факт `verify::readiness`, одной строкой — что не так и что сделать, без
/// «факт N провален» (заказчик 2026-10-20 читает это как 1С-разработчик).
/// Подсказка не дублируется, когда она уже вписана в само `detail` (так у
/// TOOLS-факта «в комплекте нет … — скопируйте заново»), иначе человек видел
/// бы один совет дважды.
pub fn readiness_reason(check: &Check) -> String {
    if check.hint.is_empty() || check.detail.contains(check.hint.as_str()) {
        format!("{}: {}", check.what, check.detail)
    } else {
        format!("{}: {} — {}", check.what, check.detail, check.hint)
    }
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

/// Поля, что решают, какую базу обслуживает публикация/MCP — путь, логин и
/// пароль дескриптора `publication::vrd`. Единый список: `save_host` смотрит
/// на него, не поднять ли публикацию заново, а подтверждение перед
/// «Сохранить» (`save_needs_confirm`) — не сменилась ли сама база — читает
/// тот же список, а не второй, отдельно придуманный.
fn database_changed(a: &Host, b: &Host) -> bool {
    a.base_path != b.base_path || a.db_user != b.db_user || a.db_password != b.db_password
}

/// Сохранить параметры сервера. Списка инструментов, каталога выгрузки,
/// режима публикации, её порта, адреса Gitea среды и адреса публикации в окне
/// нет (правятся в toml): берём их из текущих параметров, иначе пустые
/// значения из формы затёрли бы toml. Каталог проекта — из формы.
pub fn save_host(ui: &mut Ui, mut host: Host) -> Result<(), String> {
    let old_host = ui.cfg.host.clone();
    host.gitea = ui.cfg.host.gitea.clone();
    host.dump = ui.cfg.host.dump.clone();
    host.tools = ui.cfg.host.tools.clone();
    host.mode = ui.cfg.host.mode.clone();
    host.publication = ui.cfg.host.publication.clone();
    host.http_port = ui.cfg.host.http_port;
    host.base_url = carried_base_url(&ui.cfg.host, &host.base_path);
    let cfg = Cfg { host };
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    write_text(&ui.kit.state_path(cfg::FILE), &cfg.render()).map_err(|e| format!("{e:#}"))?;
    let base_path = cfg.host.base_path.clone();
    // Дескриптор публикации (`publication::vrd`) несёт путь базы, логин и
    // пароль — переподнять его нужно при смене любого из трёх, не только
    // пути: иначе смена пароля/логина оставляла бы старый дескриптор живым,
    // и публикация продолжала бы открывать базу под старыми учётными
    // данными (находка повторного ревью).
    let descriptor_changed = database_changed(&cfg.host, &old_host);
    // То же сравнение, что решает, светится ли «Сохранить» (`panel::fields_differ`):
    // поля, которых окно не показывает (в т.ч. `mode`), сеанс не перезапускают
    // здесь — смена режима в трее поднимает его сама, отдельным путём
    // (`tray::on_menu`), и лишний перезапуск в довесок к тому был бы только
    // лишней строкой в журнал и лишним миганием портов.
    let changed = panel::fields_differ(&cfg.host, &old_host, &cfg.host.mode);
    ui.cfg = cfg;
    // Публикация не должна молчать про старую базу: иначе агент отвечает из
    // прежней, пока кто-нибудь не перезапустит HTTP-сервис руками (находка
    // целостного ревью) — до следующего вызова она обязана смотреть на
    // новую. `ensure_publication` сама решает «auto»/«webserver» и когда уже
    // «сделано» — здесь только зовём её, не повторяя эту логику.
    if descriptor_changed {
        ensure_publication_now(ui);
    }
    if changed && ui.session.is_some() {
        if ui.cfg.host.mode == "local" {
            // Локальный режим: слушатели подняты на весь срок жизни процесса
            // и не трогаются переключателем/«Сохранить» (см. `launch`) —
            // спека прямо требует, чтобы смена базы была незаметна для Zed:
            // слушатели и уже принятые соединения не рвутся, следующий вызов
            // агента просто уходит в новую базу. `Serve::dispatch` берёт её
            // из этой же общей ячейки на каждое новое соединение — полный
            // перезапуск сеанса (`restart_session`) здесь не нужен и раньше
            // был блокирующей находкой целостного ревью (человеку пришлось
            // бы переподключать два сервера в Zed руками).
            if let Some(s) = &ui.session {
                s.update_cfg(ui.cfg.clone());
            }
            note_line(ui, Line { kind: Kind::Say, text: format!("MCP: параметры обновлены, база {base_path}") });
        } else {
            restart_session(ui, base_path);
        }
    }
    // Почва могла сдвинуться (база, логин, пароль, порт публикации) —
    // вердикт «дойдёт ли агент до базы» устарел, пересчитываем. Здесь же
    // пересчитывается замок, и если правка убрала холодное условие (стёрли
    // пользователя базы), `refresh_gate` сам гасит обслуживание и говорит об
    // этом в журнал — своей ветки «Сохранить» для этого больше не держит.
    // Спросили об этом раньше, до записи (`save_needs_confirm`): молчать
    // нельзя, агент остаётся без обслуживания.
    run_readiness(ui);
    Ok(())
}

/// Чистое ядро вопроса «погасит ли „Сохранить“ обслуживание»: замок был
/// открыт, обслуживание живо, а на новых параметрах условия не хватает.
/// Заперто и до правки — гасить нечего, вопрос не нужен.
pub fn locks_gate(gate_before: Option<&str>, gate_after: Option<String>, serving: bool) -> Option<String> {
    if gate_before.is_some() || !serving {
        return None;
    }
    gate_after
}

/// То же на живых параметрах. Полей, которых окно не показывает, в форме нет —
/// пустой режим берём из текущих параметров, как это делает сама `save_host`:
/// иначе туннельная проверка адреса среды применилась бы к локальному рабочему
/// месту. Непустой режим — это смена режима из трея, и спрашивать надо как
/// раз про новый: замки у режимов разные (туннелю нужны адрес среды, пароль
/// и имя рабочего места, локальному — нет). «Было» берём из кэша `ui.gate`
/// (он пересчитывается по тем же событиям и его же читает `toggle_link`), на
/// диск ходим ровно раз — за «станет»: нажатие «Сохранить» не должно стоить
/// шести обходов `Program Files` на главном потоке окна.
pub fn save_locks_gate(ui: &Ui, new_host: &Host) -> Option<String> {
    let serving = ui.enabled.load(Ordering::Relaxed) && ui.session.is_some();
    if !serving || ui.gate.is_some() {
        return None;
    }
    let mut host = new_host.clone();
    if host.mode.is_empty() {
        host.mode = ui.cfg.host.mode.clone();
    }
    locks_gate(ui.gate.as_deref(), gate_now(&ui.kit, &Cfg { host }), serving)
}

/// О чём спросить перед «Сохранить», и спрашивать ли вообще: готовый текст
/// диалога или `None`. Один расчёт на нажатие — и «нужен ли вопрос», и «какой
/// текст» отвечает он же, а не два одинаковых похода на диск подряд.
/// Погасшее обслуживание важнее смены базы: оно оставляет агента без ответа
/// вовсе.
fn save_question(ui: &Ui, new_host: &Host) -> Option<(String, String)> {
    if let Some(why) = save_locks_gate(ui, new_host) {
        return Some(confirm_gate_text(&why));
    }
    let live = ui.enabled.load(Ordering::Relaxed) && ui.session.is_some();
    if database_changed(new_host, &ui.cfg.host) && live {
        return Some(confirm_text(&ui.cfg.host.base_path, &new_host.base_path));
    }
    None
}

/// Нужно ли спросить перед «Сохранить»: база меняется (`database_changed`
/// выше — тот же список полей, что у публикации, не второй, отдельно
/// придуманный), агент прямо сейчас может отвечать (`enabled` — его читает
/// мост на каждый вызов, в обоих режимах, не только в локальном), и сеанс
/// жив (`ui.session`). Любая другая правка — каталог проекта, режим,
/// рабочее место — сохраняется без вопроса, как и было задумано (заказчик
/// просил вопрос именно и только в этом сочетании, 2026-10-20).
/// Второй повод спросить — тот же диалог: правка убирает холодное условие и
/// тем гасит обслуживание (`save_locks_gate`). Заказчик требовал вопроса
/// именно в этом сочетании: цель спеки — чтобы смена параметров была
/// незаметной для Zed, а тут агент остаётся без обслуживания.
pub fn save_needs_confirm(ui: &Ui, new_host: &Host) -> bool {
    save_question(ui, new_host).is_some()
}

/// Имя базы для диалога — последний компонент пути («Buh» для C:\Bases\Buh),
/// как 1С-разработчик называет базу в разговоре; полный путь уходит в
/// detail, не в это имя. Пустой/корневой путь без компонента — путь целиком,
/// лишь бы в диалоге не было пустой строки.
fn base_label(path: &str) -> &str {
    // Режем по обоим разделителям сами, а не средствами системы: на Linux
    // `Path::file_name` не считает `\` разделителем, и из `сервер\база`
    // (такую базу коннектор принимает на обеих системах) имя не выделилось
    // бы вовсе — в диалоге оказалась бы вся строка. Нашлось, когда тесты
    // окна впервые пошли на Linux (CI, 2026-10-21).
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path)
}

/// Текст диалога подтверждения: два предложения (что сейчас, что станет
/// после, и сам вопрос) и деталь с обоими полными путями.
pub fn confirm_text(old_path: &str, new_path: &str) -> (String, String) {
    let message =
        format!("Агент сейчас работает с базой «{}», после сохранения перейдёт на «{}». Сохранить?", base_label(old_path), base_label(new_path));
    (message, format!("{old_path} → {new_path}"))
}

/// Текст того же диалога, когда правка не меняет базу, а гасит обслуживание:
/// называем последствие и причину, а не одно «Сохранить?».
pub fn confirm_gate_text(why: &str) -> (String, String) {
    ("После сохранения MCP выключатся — агент перестанет отвечать. Сохранить?".to_string(), format!("включать будет нечего: {why}"))
}

/// Диалог перед «Сохранить», когда база меняется под живым и включённым MCP
/// (см. `save_needs_confirm`) — иначе `proceed` зовётся сразу, без вопроса.
/// Платформенный диалог (`Window::prompt`), тот же приём, что у выбора
/// каталога (`Panel::pick_dir`): `dialog_open` не даёт окну спрятаться под
/// диалогом (`Panel::new`'s `observe_window_activation`) и не даёт открыть
/// его второй раз; ответ приходит в своём потоке задачи — главный поток
/// ничего не ждёт. `proceed` зовётся ровно раз («да», или вопроса не было)
/// либо никогда (отказ — поля окна остаются как человек их оставил, здесь
/// ничего не трогаем).
pub fn confirm_database_change(window: &mut Window, cx: &mut App, new_host: &Host, proceed: impl FnOnce(&mut Window, &mut App) + 'static) {
    // Один диалог на оба повода (спека: «тот же диалог подтверждения, что уже
    // спрашивает при смене базы»), и один расчёт на нажатие: текст приходит
    // вместе с ответом «спрашивать ли».
    let Some((message, detail)) = save_question(cx.global::<Ui>(), new_host) else {
        proceed(window, cx);
        return;
    };
    if cx.global::<Ui>().dialog_open {
        return;
    }
    cx.global_mut::<Ui>().dialog_open = true;
    let rx = window.prompt(
        PromptLevel::Warning,
        &message,
        Some(&detail),
        &[PromptButton::ok("Да, сохранить"), PromptButton::cancel("Нет, не сохранять")],
        cx,
    );
    cx.spawn(async move |cx| {
        let yes = matches!(rx.await, Ok(0));
        let _ = cx.update(|cx| {
            cx.global_mut::<Ui>().dialog_open = false;
            if !yes {
                return;
            }
            let Some(w) = cx.global::<Ui>().window else { return };
            let _ = w.update(cx, |_, window, cx| {
                window.activate_window();
                proceed(window, cx);
            });
        });
    })
    .detach();
}

/// Поднять/проверить HTTP-сервис на уже записанных `ui.cfg`: после смены
/// базы («Сохранить», см. выше), при включении MCP (`toggle_link`) и при
/// старте с «подключаться при запуске» (`launch`). `ensure_publication` сама
/// идемпотентна и решает режим, но пробует связь с таймаутом в 3 с, в режиме
/// «webserver» ходит в сеть за версией расширения, а после запуска своего
/// httpd ждёт его готовности — всё это в своём потоке, а не на главном
/// потоке окна (находка повторного ревью: окно подвисало на эти секунды).
/// Ответ идёт `Ev::Publish`: журнал на отказе и карточка — в `handle`, не
/// здесь, иначе карточка соврала бы, что всё в порядке, пока агент отвечает
/// с ошибкой.
fn ensure_publication_now(ui: &mut Ui) {
    let ctx = ctx_for(ui);
    let tx = ui.tx.clone();
    std::thread::spawn(move || {
        let r = ops_host::ensure_publication(&ctx).map_err(|e| format!("{e:#}"));
        let _ = tx.unbounded_send(Ev::Publish(r));
    });
}

/// Холодный замок на текущих параметрах: всё, за чем надо на диск, спрашиваем
/// здесь, решает `gate::cold_gate`. Отдельно от `refresh_gate` ниже, потому
/// что `launch` считает замок до того, как `Ui` вообще появился — начальное
/// значение переключателя уже обязано его слушаться.
pub fn gate_now(kit: &Kit, cfg: &Cfg) -> Option<String> {
    let tools = ops_host::tools_state(&kit.root, &ops_host::tools_dir());
    // Существование спрашиваем только у файловой базы (`gate::wants_directory`,
    // правило самого факта «база»): `exists()` по недоступной сетевой шаре
    // висит на таймауте SMB, а считается это на главном потоке окна.
    let base = &cfg.host.base_path;
    let base_exists = gate::wants_directory(base) && std::path::Path::new(base).exists();
    gate::cold_gate(
        cfg,
        kit.state_path(cfg::FILE).exists(),
        base_exists,
        &tools,
        ops_host::find_1cv8().as_deref(),
        ops_host::mcp_exe(kit).exists(),
        // Срок спрашиваем у часов, а не у запомненного при запуске: окно может
        // простоять открытым через полночь срока (так же поступает `run_op`).
        &crate::licence::state_now(),
    )
}

/// Пересчитать замок и донести причину до моста: пока замок заперт, отказ
/// агенту несёт её текстом (`bridge::disabled_text`), иначе ни агент, ни
/// человек не узнают, что править.
pub fn refresh_gate(ui: &mut Ui) {
    // Замок именно **защёлкивается**: был открыт — стал заперт. Замок,
    // запертый и до пересчёта, обслуживание уже погасил этим же местом, и
    // гасить второй раз (и говорить второй раз) незачем.
    let was_open = ui.gate.is_none();
    ui.gate = gate_now(&ui.kit, &ui.cfg);
    // Защёлкнувшийся замок гасит обслуживание — здесь, в единственном месте,
    // где он защёлкивается, а не у каждого вызывающего по-своему. Иначе
    // замок правил бы одни слова в отказе: человек правит toml руками,
    // условие пропадает, окно на показе перечитывает файл и пересчитывает
    // ворота — карточка говорит «MCP выключены», а агента продолжают
    // обслуживать, потому что флаг никто не снял (находка ревью задачи 24).
    //
    // Обратное ворота не делают: условие вернулось — переключатель
    // отпирается сам (`panel::switch_locked` смотрит на `ui.gate`), но
    // обслуживание включает человек, а не пересчёт за его спиной.
    if was_open {
        if let Some(why) = ui.gate.clone() {
            if ui.enabled.load(Ordering::Relaxed) {
                douse_serving(ui);
                note_line(ui, Line { kind: Kind::Warn, text: format!("MCP выключены: {why}") });
            }
        }
    }
    // Замок доезжает до трея всегда, а не только когда что-то погасил
    // (находка S2): пункт «Включить MCP» обязан запираться и отпираться
    // ровно тогда же, когда переключатель в окне. Раньше `sync_tray` стояла
    // только внутри `douse_serving`, и открывшийся замок (вписали
    // пользователя базы) оставлял пункт серым до следующей операции, а
    // защёлкнувшийся при выключенных MCP — нажимаемым.
    sync_tray(ui, Instant::now());
}

/// Погасить обслуживание вызовов, не спрашивая никого: правка убрала холодное
/// условие. Снять один флаг мало — карточка и переключатель читают `ui.link`, и
/// без этой строки человек видел бы зелёное «MCP включены» и переключатель,
/// который нарисован включённым, но на щелчок не отвечает (находка ревью).
///
/// В локальном режиме слушатели остаются поднятыми (они живут весь срок жизни
/// процесса), гаснет только обслуживание — ровно то же, что делает ручное
/// выключение переключателя. В туннельном обслуживание и есть сеанс, поэтому
/// он опускается целиком, тем же приёмом, что `stop_session` (ей нужен `cx`,
/// а здесь его нет).
fn douse_serving(ui: &mut Ui) {
    ui.enabled.store(false, Ordering::Relaxed);
    if ui.cfg.host.mode == "local" {
        ui.link = local_display("local", false, ui.link.clone());
    } else {
        ui.session_gen += 1;
        if let Some(s) = ui.session.take() {
            std::thread::spawn(move || {
                let _ = s.stop();
            });
        }
        ui.link = State::Off;
    }
    sync_tray(ui, Instant::now());
}

/// Включать ли обслуживание при запуске: галочка «подключаться при запуске»
/// замок не обходит (спека: «Автозапуск не обходит замок»). В локальном
/// режиме это начальное значение переключателя, в туннельном — ещё и
/// поднимать ли сеанс.
pub fn autostart_enabled(gate: Option<&str>, autoconnect_on: bool) -> bool {
    autoconnect_on && gate.is_none()
}

/// Отказ публикации: гасит обслуживание и говорит, что показать. Спека
/// называет этот ряд красным («Отказ занять порт или поднять публикацию» →
/// «Не подключён»), поэтому выбран первый из двух выходов: карточка остаётся
/// красной, а настоящее обслуживание подводится под показанное, а не
/// наоборот.
///
/// Раньше отказ только красил карточку: `enabled` оставался поднятым, мост
/// продолжал обслуживать вызовы под красным «Не подключён», а первый щелчок
/// по переключателю **выключал** обслуживание вместо того, чтобы включить, —
/// показанное и настоящее разошлись (находка S6 итогового ревью). Теперь
/// щелчок после отказа означает «попробовать снова»: `toggle_link` поднимет
/// флаг и позовёт публикацию заново.
///
/// Цена названа прямо: инструменты, которым публикация не нужна
/// (`cf_decompile`/`cf_load` через `vrunner-mcp`), на время отказа тоже не
/// обслуживаются. Это следствие обещания карточки: красное значит «не
/// работает», а не «работает наполовину».
///
/// **Красное не лечится само, и это нарочно** (ревью N5). Удавшаяся
/// публикация (скажем, после «Сохранить») карточку не зеленит: `enabled`
/// снят здесь, а поднимать обслуживание за спиной человека программа не
/// умеет нигде — тем же правилом живёт замок («обратное ворота не делают:
/// условие вернулось — переключатель отпирается сам, но обслуживание
/// включает человек», см. `refresh_gate`). Красная карточка в это время не
/// врёт: вызовы правда не обслуживаются. Выход один и он на виду —
/// переключатель нарисован выключенным, причина в подвале и в подсказке, и
/// один щелчок означает «попробовать снова». Не надо «чинить» это
/// самозеленением: оно вернёт ровно тот дефект, против которого писан
/// раздел спеки «Зелёный значит „работает“» — экран, обещающий работу,
/// которую никто не подтверждал.
fn publication_failure(ui: &mut Ui, why: &str) -> State {
    ui.enabled.store(false, Ordering::Relaxed);
    State::Failed(format!("публикация: {why}"))
}

/// Обслуживаются ли вызовы агента прямо сейчас — по тому самому флагу,
/// который мост читает на каждый вызов. В туннельном режиме нужен ещё и
/// поднятый сеанс: без него вызову некуда ехать.
///
/// Раньше туннель смотрел только на сеанс, и флаг с ним расходился: замок,
/// защёлкнувшийся при смене режима, гасил флаг, а сеанс поднимался следом
/// безусловно — карточка показывала зелёное «Подключён», мост отказывал
/// каждому вызову, и первый щелчок по переключателю не включал обслуживание,
/// а останавливал туннель (находка S5 итогового ревью).
fn serving(ui: &Ui) -> bool {
    ui.enabled.load(Ordering::Relaxed) && (ui.cfg.host.mode == "local" || ui.link.is_on())
}

/// Расчёт готовности (`verify::readiness` — тот же набор фактов, что
/// печатает «Проверить») в своём потоке: он ходит по сети и поднимает пробу
/// инструментов, и не должен держать главный поток окна — тем же приёмом,
/// что `ensure_publication_now` выше. Зовётся, когда почва могла сдвинуться:
/// карточка стала «MCP включены» (включили переключатель, либо сеанс пришёл
/// уже включённым — автозапуск), кончилась любая операция (дешевле
/// перепроверить лишний раз, чем отличать операции друг от друга),
/// «Сохранить» и показ окна. Не на таймере — между этими событиями
/// `ui.ready` просто не пересчитывается.
pub fn run_readiness(ui: &mut Ui) {
    // Замок — по тем же событиям, что и вердикт (почва для них одна), но без
    // потока: он считается по диску и отвечает сразу, а переключатель обязан
    // слушаться его уже на ближайшей отрисовке.
    refresh_gate(ui);
    // Выключено — считать нечего (спека: «при выключенном обслуживании
    // горячие факты цвет не меняют, и вердикт в это время не считается»).
    // Карточка его и не покажет, а набор стоит 20–40 с, пробы инструментов и
    // поднятого на Linux виртуального экрана — всё ради ответа, который
    // некому показать (находка S9). Включат — посчитается само: включение и
    // есть одно из событий пересчёта.
    if !serving(ui) {
        return;
    }
    let Some(gen) = start_readiness(ui, Instant::now()) else { return };
    // Молча: `quiet_ctx`, а не `ctx_for` — ни строки в журнале, ни фазы в
    // карточке хода (`verify::readiness` фазу и не объявляет).
    let ctx = quiet_ctx(ui);
    let mode = ui.cfg.host.mode.clone();
    let link = ui.link.clone();
    let tx = ui.tx.clone();
    // `guarded`, как у всех прочих фоновых потоков окна: паника внутри сбора
    // фактов (подпроцессы, reqwest, чтение путей) обязана договорить ответом,
    // иначе `ready_pending` остался бы взведённым навсегда и вердикт не
    // считался бы больше ни разу — а `Unknown` карточка рисует зелёным.
    // На панике отдаём не `None` («всё хорошо»), а причину: она и покажется
    // в карточке.
    std::thread::spawn(move || {
        guarded(
            &tx,
            || {
                let state = link.is_up().then(|| link.label(&mode));
                Ev::Ready(gen, verify::first_fail(&verify::readiness(&ctx, state.as_deref())))
            },
            || Ev::Ready(gen, Some(Check::fail("готовность", PANIC_TEXT, "нажмите «Проверить»"))),
        );
    });
}

/// Принять перечитанный с диска `Cfg` (`window::show_window`): он должен
/// доехать не только до окна, но и до живого сеанса — иначе карточка,
/// вердикт и «Проверить» говорят про новую базу, а мост на каждое соединение
/// берёт старую (находка ревью). Приём тот же, что у «Сохранить» в локальном
/// режиме (`save_host`): `update_cfg` кладёт параметры в общую ячейку, и
/// следующий вызов агента уходит уже в новую базу, не разрывая поднятых
/// слушателей и принятых соединений. В туннельном режиме подменой параметров
/// не обойтись (порты живут внутри SSH — там `restart_session`), а рвать
/// туннель под человеком на показе окна нельзя: правку подхватит «Сохранить».
pub fn adopt_cfg(ui: &mut Ui, cfg: Cfg) {
    ui.cfg = cfg;
    if ui.cfg.host.mode == "local" {
        if let Some(s) = &ui.session {
            s.update_cfg(ui.cfg.clone());
        }
    }
}

/// Сколько ждать поток готовности, прежде чем считать его потерянным. Бюджет
/// набора — 20 с, и сторож нарочно взят много больше: он не инструмент
/// точности, а предохранитель от залипшей защёлки. Пока он не сработал,
/// второй поток не заводится, так что ошибиться в меньшую сторону хуже —
/// это две пробы инструментов разом.
const READINESS_LOST: Duration = Duration::from_secs(5 * 60);

/// Потерян ли идущий расчёт: защёлка стоит дольше всякого разумного срока.
/// `None` у начала — защёлку взвели где-то мимо `start_readiness`; считаем
/// потерянным, иначе она осталась бы навсегда.
fn readiness_lost(since: Option<Instant>, now: Instant) -> bool {
    match since {
        Some(t) => now.saturating_duration_since(t) > READINESS_LOST,
        None => true,
    }
}

/// Ядро `run_readiness` без потока: сброс устаревшего вердикта, поколение и
/// отметка повтора. `None` — считать не надо: поток уже считает, пересчёт
/// помечен и пойдёт один раз по его возвращении.
///
/// Исключение — потерянный поток (`readiness_lost`): ответа от него нет и,
/// похоже, не будет, а `ready_pending` без этого остаётся взведённым
/// навсегда — вердикт не посчитается больше ни разу, и карточка застрянет на
/// «Проверяю…» (находка S10). Тогда расчёт начинается заново, а ответ
/// пропавшего потока, если он всё-таки придёт, отбросит поколение
/// (`apply_ready`). Человеку об этом не говорим: он видит, что следующий
/// пересчёт прошёл, и этого довольно.
fn start_readiness(ui: &mut Ui, now: Instant) -> Option<u64> {
    // Старый вердикт не должен пережить смену почвы — сброс синхронный,
    // ещё до похода по сети (`drop_stale_readiness`, тестируется отдельно
    // от самого похода).
    drop_stale_readiness(ui);
    if ui.ready_pending && !readiness_lost(ui.ready_since, now) {
        ui.ready_again = true;
        return None;
    }
    // Этот расчёт и есть обещанный повтор — отметку снимаем здесь, а не в
    // ответе: иначе она пережила бы собственное исполнение и дала лишний
    // проход.
    ui.ready_again = false;
    ui.ready_pending = true;
    ui.ready_since = Some(now);
    ui.ready_gen += 1;
    ui.ready_run = ui.ready_gen;
    Some(ui.ready_gen)
}

/// Ответ потока готовности; `pending` снимается всегда — ответ пришёл, поток
/// кончился. Возвращает `true`, когда набор надо посчитать заново — ровно
/// один раз.
///
/// Чужое поколение значит, что вердикт моложе этого ответа: его поставил итог
/// «Проверить». Тогда не трогаем ни вердикт, ни отметку повтора — съесть её
/// здесь означало бы обнулить вердикт в `Unknown` (то есть снова в зелёное)
/// сразу после того, как проверка нашла провалы. Отметку исполнит и снимет
/// тот расчёт, который её и прочтёт (`start_readiness`).
fn apply_ready(ui: &mut Ui, gen: u64, first_fail: Option<Check>) -> bool {
    // Защёлку снимает только тот расчёт, который её держит (`ready_run`), —
    // его ответ и значит «поток кончился». Раньше её снимал любой ответ, и
    // опоздавший ответ потерянного потока отпирал защёлку живого: следующее
    // событие поднимало вторую пробу инструментов поверх первой (находка
    // N1). Ответ не-владельца защёлку не трогает вовсе; если владелец не
    // ответит никогда, её подберёт сторож `readiness_lost`.
    //
    // Поколение вердикта (`ready_gen`) — отдельный вопрос: его двигает и
    // «Проверить» (`verdict_from_checks`), ничего при этом не запуская.
    // Поэтому ответ владельца защёлки вполне может оказаться устаревшим для
    // вердикта — защёлку он снимет, а вердикт уже не его.
    if gen == ui.ready_run {
        ui.ready_pending = false;
        ui.ready_since = None;
    }
    if gen != ui.ready_gen {
        return false;
    }
    let again = std::mem::take(&mut ui.ready_again);
    if !again {
        let was = warn_reason(&ui.ready);
        ui.ready = match first_fail {
            Some(c) => Readiness::Warn(c),
            None => Readiness::Ok,
        };
        // Вердикт стал известен прямо сейчас — отсюда и час, который подвал
        // называет рядом с причиной.
        ui.ready_at = Some(clock());
        // Причина жёлтого вердикта — строкой в журнал (спека называет три
        // канала: подсказка, подвал и журнал; журнал молчал — находка S3).
        // Только на смене причины: вердикт пересчитывается по каждому
        // событию, и повторять одну и ту же строку значит засыпать журнал.
        let now = warn_reason(&ui.ready);
        if let Some(why) = now {
            if Some(&why) != was.as_ref() {
                note_line(ui, Line { kind: Kind::Warn, text: format!("не всё готово: {why}") });
            }
        }
    }
    // Значок считается по вердикту (`tray_state_now` → `panel::card`): без
    // этой строки карточка желтела, а значок оставался прежним — зелёным или
    // «занято» (находка S1 итогового ревью).
    sync_tray(ui, Instant::now());
    again
}

/// Причина жёлтого вердикта человеческими словами, `None` — вердикт не
/// жёлтый. Одна формулировка на журнал и на подвал (`panel::reason` берёт
/// ту же `readiness_reason`).
fn warn_reason(ready: &Readiness) -> Option<String> {
    match ready {
        Readiness::Warn(c) => Some(readiness_reason(c)),
        _ => None,
    }
}

/// Итог «Проверить» и есть вердикт (спека): полный набор в этот момент уже
/// посчитан, первый провал — причина в карточке. Без этой связки живой
/// случай заказчика 2026-10-20 повторяется дословно: вердикт посчитан
/// раньше, проверка нашла провалы, точка осталась зелёной. Поколение растёт
/// заодно — идущий расчёт готовности этот свежий вердикт не затрёт. Пустой
/// набор — упавший поток проверки (`guarded`), а не «всё хорошо»: прежний
/// вердикт тоже не годится (он мог быть `Ok`, и точка осталась бы зелёной
/// при упавшей проверке), поэтому там честнее «не считано».
fn verdict_from_checks(ui: &mut Ui, checks: &[Check]) {
    ui.ready_gen += 1;
    if checks.is_empty() {
        drop_stale_readiness(ui);
        return;
    }
    ui.ready = match verify::first_fail(checks) {
        Some(c) => Readiness::Warn(c),
        None => Readiness::Ok,
    };
    ui.ready_at = Some(clock());
}

/// Сброс вердикта в `Readiness::Unknown`: сама почва ещё не проверена заново,
/// но и старый ответ — уже не про неё. Ядро `run_readiness` без её похода по
/// сети, тестируется само по себе (см. тесты модуля).
fn drop_stale_readiness(ui: &mut Ui) {
    ui.ready = Readiness::Unknown;
    // Час вердикта уходит вместе с вердиктом: «проверено в 15:54» без
    // самого вердикта — обещание того, чего больше нет.
    ui.ready_at = None;
}

/// Перезапустить сеанс после удачной записи параметров (только туннельный
/// режим — там порты живут внутри SSH, и подменить базу под уже идущим
/// сеансом нечем): старому потоку просят остановиться (в своём потоке, не
/// блокируя этот), новый поднимают сразу с уже записанными параметрами —
/// тем же флагом `enabled`, он живёт дольше сеанса. Переход в
/// «подключаюсь»/«подключён» придёт позже обычным `Ev::Link`, здесь —
/// только одна строка в журнал про сам факт перезапуска.
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

/// `Ctx` без голоса — для расчёта готовности: его события никуда не идут, ни
/// строкой в журнал окна, ни фазой в карточку хода. Спека требует этого
/// прямо: готовность считается молча, иначе окно принимает расчёт за
/// начавшуюся операцию (запираются переключатель и «Сохранить», значок трея
/// желтеет, а журнал заполняется строками пробы). Всё прочее — то же, что у
/// `ctx_for`.
fn quiet_ctx(ui: &Ui) -> Ctx {
    Ctx { kit: ui.kit.clone(), cfg: ui.cfg.clone(), dry_run: false, sink: Arc::new(|_: Event| {}), prompt: Box::new(AlwaysDefault) }
}

/// Состояние значка — это точка карточки, переведённая в бейдж
/// (`tray::tray_state_of`). Порядок разрешения живёт в одном месте
/// (`panel::card`: операция → отказ → замок → вердикт не посчитан →
/// провалившийся факт → всё в порядке), и значок не может разойтись с окном:
/// раньше у него было своё правило («поднята связь — зелёный»), и он
/// оставался зелёным при непосчитанном и провалившемся вердикте — ровно та
/// ложь, против которой писан раздел спеки «Зелёный значит „работает“».
///
/// Идущая проверка — единственное, чего карточка не видит: «Проверить»
/// держит `ui.busy`, но не `ui.progress` (карточки хода у проверки нет).
/// Для значка это та же занятость, что и операция.
fn tray_state_now(ui: &Ui, now: Instant) -> TrayState {
    if ui.busy.is_some() {
        return TrayState::Busy;
    }
    tray::tray_state_of(panel::card(ui.progress.as_ref(), &ui.link, &ui.cfg.host.mode, &ui.ready, ui.gate.as_deref(), now).dot)
}

/// Синхронизировать трей с `Ui`: состояние (по точке карточки) и подсказка
/// на текущий момент. Общий конец всех мест, что меняют почву под значком —
/// `set_link`, `run_op`, `Ev::Done`, `Ev::Checks`, `Ev::Ready` (вердикт),
/// `refresh_gate` (замок); каждый забытый вызов — это значок, застрявший на
/// прежнем состоянии, и ровно так жили находки S1 и S2 итогового ревью.
///
/// Показанное запоминается в `ui.tray_shown`: иначе «значок узнал» — побочное
/// действие, которого тесту не видно (настоящего `Tray` в тестах нет), и
/// проводку приходится сторожить сверкой чистых функций между собой. Три
/// таких зелёных теста при сломанном поведении эта работа уже пережила.
fn sync_tray(ui: &mut Ui, now: Instant) {
    let view = TrayView {
        state: tray_state_now(ui, now),
        on: ui.link.is_on(),
        locked: ui.gate.is_some(),
        tooltip: tray_tooltip(ui.progress.as_ref(), &ui.link, ui.gate.as_deref(), now),
    };
    if let Some(tray) = &ui.tray {
        tray.set_state(view.state, view.on, view.locked);
        tray.set_tooltip(&view.tooltip);
    }
    ui.tray_shown = Some(view);
    // Заодно и мост: причина отказа агенту меняется ровно тогда же, когда
    // состояние значка (замок, отказ, выключили), и считать её в каждом из
    // этих мест по-своему — значит снова разойтись. Раньше её писала одна
    // `refresh_gate`, и после отказа публикации агент слышал «включите
    // переключатель» про то, чего никто не выключал (находка N2).
    crate::bridge::set_off_reason(off_reason(ui));
}

/// Почему обслуживание не идёт — одной строкой для отказа агенту. Порядок
/// тот же, что у карточки: сперва замок (включать нечего), потом отказ,
/// который обслуживание погасил (занят порт, не поднялась публикация).
/// Обслуживание идёт — причины нет; выключил человек — причины тоже нет, и
/// отказ советует прежнее «включите переключатель», что в этом случае и
/// правда (`bridge::disabled_text`).
fn off_reason(ui: &Ui) -> Option<String> {
    if serving(ui) {
        return None;
    }
    ui.gate.clone().or_else(|| match &ui.link {
        State::Failed(why) => Some(why.clone()),
        _ => None,
    })
}

/// Переход сеанса: в состояние, в журнал, в трей, перерисовать окно.
fn set_link(cx: &mut App, l: State) {
    let host = cx.global::<Ui>().cfg.host.clone();
    let line = link_line(&l, &host, &host.mode);
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
    let params = ui.kit.state_path(cfg::FILE);
    if !params.exists() {
        return Err(format!("нет файла параметров {} — заполните поля и нажмите «Сохранить»", params.display()));
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

/// Отображение настоящего отказа bind (`ui.bind_failed`) — независимо от
/// того, включили или выключили переключатель: слушатели в локальном режиме
/// не поднимаются заново ни тем, ни другим кликом (см. `launch`), так что
/// «включить» после провала ничего не чинит, и карточка обязана продолжать
/// говорить правду, а не «MCP включены». Решает по сохранённому отказу, а не
/// по текущему показанному состоянию — иначе один клик «выключить» стирал
/// бы его раньше, чем следующий клик «включить» успел бы его увидеть
/// (находка повторного ревью: прежний `keep_failure_on_enable` ловил лишь
/// один клик).
fn bind_failure_display(bind_failed: &Option<String>) -> Option<State> {
    bind_failed.clone().map(State::Failed)
}

pub fn toggle_link(cx: &mut App) {
    // Замок — один на окно и на трей, и держится он здесь, в общем месте: и
    // переключатель, и пункт меню приходят сюда. Запирается только включение —
    // выключить обслуживание можно всегда (замок мог защёлкнуться под живым
    // сеансом, например правкой toml руками).
    if !serving(cx.global::<Ui>()) {
        if let Some(why) = cx.global::<Ui>().gate.clone() {
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Warn, text: format!("включать нечего: {why}") });
            cx.refresh_windows();
            return;
        }
    }
    if cx.global::<Ui>().cfg.host.mode == "local" {
        let ui = cx.global_mut::<Ui>();
        let on = flip_enabled_only(ui);
        // Вердикт прошлого состояния переключателя устарел в любую сторону —
        // следующий зелёный «MCP включены» подтвердит себя сам (`run_readiness`
        // ниже/в `Ev::Publish`), а не унаследует чужую проверку.
        drop_stale_readiness(ui);
        if let Some(l) = bind_failure_display(&ui.bind_failed) {
            set_link(cx, l);
            return;
        }
        if on {
            // «Включаю…», пока публикация (проба до 3 с, ожидание httpd)
            // идёт в своём потоке — иначе карточка сказала бы «MCP включены»
            // раньше, чем проба успела ответить (находка повторного ревью:
            // карточка оставалась зелёной даже после отказа публикации).
            set_link(cx, State::Connecting);
            ensure_publication_now(cx.global_mut::<Ui>());
        } else {
            set_link(cx, State::Off);
        }
        return;
    }
    let on = cx.global::<Ui>().link.is_on();
    // Флаг обслуживания идёт за сеансом и в туннельном режиме. Раньше он
    // писался только в `launch` (начальным значением), и это было терпимо,
    // пока в «выключено» попадал лишь тот, кто сам снял «подключаться при
    // запуске». С замком туда попадает и обычное рабочее место — а выйти
    // оттуда было нечем: человек чинил условие, поднимал туннель, карточка
    // говорила «Подключён», а каждый вызов агента получал отказ с причиной,
    // которой уже нет (находка ревью).
    cx.global::<Ui>().enabled.store(!on, Ordering::Relaxed);
    if on {
        stop_session(cx);
        return;
    }
    match start_session(cx) {
        Ok(()) => ensure_publication_now(cx.global_mut::<Ui>()),
        Err(e) => {
            window::show_window(cx);
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
            cx.refresh_windows();
        }
    }
}

/// Расширение (установить/снять) или обмен с git (выгрузить/загрузить);
/// ход — событиями из `ctx_for`, конец — `Ev::Done`. Заголовок работы ложится
/// в журнал, и журнал же показывается: за ходом смотрят там.
pub fn run_op(cx: &mut App, which: Which) {
    // Срок спрашиваем у часов здесь, на нажатие, а не берём запомненное при
    // запуске: окно может простоять открытым через полночь срока.
    if let Some(line) = op_refusal(crate::licence::state_now()) {
        let ui = cx.global_mut::<Ui>();
        ui.tab = Tab::Log;
        ui.unread = 0;
        note_line(ui, line);
        cx.refresh_windows();
        return;
    }
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
                // Порты рабочего места занимает само окно: занятый кем-то
                // ещё порт для него — настоящий провал, а не сведение
                // (спека, «порты важны там, где мы их занимаем»).
                let checks = verify::run(&ctx, state.as_deref(), verify::PortUse::WeListen);
                let text = check::render("1c-connect", env!("CARGO_PKG_VERSION"), &verify::cfg_for_report(&ctx.cfg), &checks);
                let _ = write_text(&ctx.kit.state_path("last-check.txt"), &text);
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
            let text = tray_tooltip(ui.progress.as_ref(), &ui.link, ui.gate.as_deref(), now);
            ui.tooltip_at = Some(now);
            if let Some(tray) = &ui.tray {
                tray.set_tooltip(&text);
            }
        }
    }
    note_licence_change(cx);
}

fn handle(ev: Ev, cx: &mut App) {
    match ev {
        Ev::Tray(e) => tray::on_tray(e, cx),
        Ev::Menu(e) => tray::on_menu(e, cx),
        Ev::Link(gen, l) => {
            if link_is_current(cx.global::<Ui>(), gen) {
                let ui = cx.global_mut::<Ui>();
                // Настоящий отказ/успех bind живёт отдельно от показанного
                // состояния — `bind_failure_display` читает именно его,
                // переключатель его не трогает (см. `toggle_link`).
                match &l {
                    State::Failed(reason) => ui.bind_failed = Some(reason.clone()),
                    State::Up { .. } => ui.bind_failed = None,
                    _ => {}
                }
                let shown = local_display(&ui.cfg.host.mode, ui.enabled.load(Ordering::Relaxed), l);
                set_link(cx, shown);
                // Вердикт готовности здесь не считается: `launch`/`toggle_link`
                // зовут `ensure_publication_now` на каждый случай, когда могла
                // подняться связь, и `Ev::Publish` ниже — единое место, что
                // перепроверяет готовность, уже после того, как HTTP-сервис
                // подтверждён, а не наперегонки с его стартом (гонка дала бы
                // ложный жёлтый на старте, который потом некому было бы снять).
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
            set_status(ui, line.clone(), None);
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
            // Почва могла сдвинуться («Установить» в т.ч.) — вердикт
            // «дойдёт ли агент до базы» устарел, пересчитываем.
            run_readiness(cx.global_mut::<Ui>());
        }
        Ev::Checks(checks) => {
            let failed = checks.iter().filter(|c| !c.ok).count();
            let lines = check_lines(&checks);
            let ui = cx.global_mut::<Ui>();
            // Итог «Проверить» и есть вердикт готовности: карточка и проверка
            // не могут расходиться (спека).
            verdict_from_checks(ui, &checks);
            ui.failed_checks = failed;
            for line in lines {
                note_line(ui, line);
            }
            ui.busy = None;
            // Пункта меню «Открыть отчёт» больше нет (заказчик 2026-10-20:
            // два пункта делали одно и то же) — файл называет строка журнала
            // и подсказка подвала, но не сам итог рядом с кнопкой: туда путь
            // не влезал (заказчик 2026-10-21).
            let report = ui.kit.state_path("last-check.txt").display().to_string();
            let status = check_status(failed);
            note_line(ui, status.clone());
            note_line(ui, report_line(&report));
            set_status(ui, status, Some(report_line(&report).text));
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
        Ev::Publish(r) => match r {
            Ok(()) => {
                let ui = cx.global::<Ui>();
                let local = ui.cfg.host.mode == "local";
                let enabled = ui.enabled.load(Ordering::Relaxed);
                // Только здесь: локальный режим, переключатель включён и
                // карточка ещё показывает «Включаю…» от этого самого клика
                // (`toggle_link`) — настоящий сеанс в локальном режиме
                // больше никаких событий не пришлёт (слушатели не
                // перезапускаются переключателем). В остальных случаях
                // (туннель, «Сохранить») показом уже ведёт сеанс — трогать
                // нечего.
                let settle = local && enabled && matches!(ui.link, State::Connecting);
                if settle {
                    set_link(cx, State::Up { since: Instant::now() });
                }
                // Публикация подтверждена прямо сейчас — `ensure_publication`
                // уже дождалась готовности своего HTTP-сервиса, так что это
                // (а не момент bind'а слушателей в `Ev::Link`, который мог
                // прийти раньше её старта) и есть момент, когда почва для
                // «дойдёт ли агент до базы» устоялась. Зовётся при каждом
                // случае, когда могла подняться связь — щелчке и автозапуске
                // с уже включённым переключателем (оба зовут
                // `ensure_publication_now`), не только при «settle» выше.
                if local && enabled {
                    run_readiness(cx.global_mut::<Ui>());
                }
            }
            // Карточка не должна врать «MCP включены», когда агент не видит
            // базу — отказ публикации становится тем же `Failed`, что и у
            // отказа сеанса, с текстом причины (находка повторного ревью).
            Err(e) => {
                let l = publication_failure(cx.global_mut::<Ui>(), &e);
                set_link(cx, l);
            }
        },
        Ev::Ready(gen, first_fail) => {
            let again = apply_ready(cx.global_mut::<Ui>(), gen, first_fail);
            cx.refresh_windows();
            if again {
                run_readiness(cx.global_mut::<Ui>());
            }
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

/// Код выхода для провалившегося трея (`Some(1)`) или `None`, когда он
/// построился и выходить незачем. Отдельная чистая функция от типа трея и
/// ошибки — решение «ошибка здесь ведёт к выходу» проверяется тестом без
/// настоящего `Tray`/GTK.
fn exit_code_for<T, E>(tray: &Result<T, E>) -> Option<i32> {
    tray.is_err().then_some(1)
}

pub fn launch(kit: Kit, hidden: bool) -> anyhow::Result<()> {
    let cfg = Cfg::load(&kit)?;
    // Тот же единственный владелец рабочего места, что у `connect`/`serve` —
    // см. `crate::single_instance`. Вторая копия окна (двойной щелчок или
    // автозапуск при живом трее) на Windows тихо поднимает окно первой и
    // выходит; замок держим до конца `run()` ниже — переменная остаётся на
    // стеке этой функции, пока приложение не закроется.
    let _guard = crate::single_instance::acquire(&kit, &cfg.host.workstation)?;
    // ponytail: усечение сбросом, ротация — когда кому-то понадобится история
    let log_path = kit.state_path(LOG_FILE);
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
    // Замок считается до первого окна: «включать MCP при запуске» его не
    // обходит (спека, «Автозапуск не обходит замок»), поэтому начальное
    // значение флага обслуживания уже обязано его слушаться.
    let gate = gate_now(&kit, &cfg);
    let serve_on_start = autostart_enabled(gate.as_deref(), autoconnect_on);
    crate::bridge::set_off_reason(gate.clone());
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
        // глобал `Ui` ещё не выставлен, поэтому `quit()` этого модуля не
        // годится. Раньше здесь звали `cx.quit()` — он просит GPUI уйти
        // своим чередом, но до открытия первого окна цикл обработки ещё не
        // крутится, попросить его не успевает никто, и процесс вместо
        // обещанного завершения висел живым (проверено на экране: больше
        // минуты) и держал замок `single_instance` — ровно от чего
        // сообщение обещало избавить следующий запуск. `exit_code_for`
        // отдельно от этого, чтобы решение «ошибка — на выход» проверялось
        // тестом без живого трея и GTK.
        let tray_result = tray::init().and_then(|()| tray::build(tx.clone(), theme::Choice::load(&kit), autostart_on, autoconnect_on));
        if let Err(e) = &tray_result {
            fatal(&format!("Значок в трее не создался: {e:#}\nБез него программу нельзя закрыть, поэтому она завершается."));
        }
        if let Some(code) = exit_code_for(&tray_result) {
            // Замок единственного экземпляра (`single_instance::Guard`) — на
            // Windows объект ядра, на Linux файловый дескриптор с `flock`; оба
            // снимаются самой системой при завершении процесса, этим выходом
            // в т.ч., отдельно освобождать нечего.
            std::process::exit(code);
        }
        let tray = tray_result.expect("exit_code_for вернул None только для Ok");
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
            bind_failed: None,
            enabled: Arc::new(AtomicBool::new(serve_on_start)),
            gate: gate.clone(),
            ready: Readiness::Unknown,
            ready_at: None,
            ready_gen: 0,
            ready_run: 0,
            ready_pending: false,
            ready_since: None,
            ready_again: false,
            licence: crate::licence::State::Valid,
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
            status_tip: None,
            tray: Some(tray),
            tray_shown: None,
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
        // Про срок — до подъёма сеанса: первое, что человек читает в журнале.
        note_licence_change(cx);
        // В локальном режиме слушатели держат порты весь срок жизни
        // процесса (Zed не переподключается сам — см. заголовок модуля),
        // поэтому сеанс поднимается здесь безусловно; «подключаться при
        // запуске» в этом режиме уже отдано начальному значению `enabled`
        // чуть выше. В туннельном режиме — как раньше, по той же галочке.
        // Замок при запуске — строкой в журнал: человек видит серую точку и
        // запертый переключатель, и обязан прочитать, почему.
        if let Some(why) = &gate {
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Warn, text: format!("MCP не включены при запуске: {why}") });
        }
        if local_mode || serve_on_start {
            match start_session(cx) {
                Ok(()) => ensure_publication_now(cx.global_mut::<Ui>()),
                Err(e) => note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e }),
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
        let (kit, cfg) = (Kit::at(dir), Cfg::default());
        // Замок считается, как в `launch`, а не заводится пустым: в каталоге
        // теста нет ни файла параметров, ни разложенных инструментов, и
        // фикстура с `None` обещала бы «включать можно» там, где нельзя — на
        // этом обещании тесты «Сохранить» расходились бы с живой программой.
        let gate = gate_now(&kit, &cfg);
        Ui {
            kit,
            cfg,
            session: None,
            session_gen: 0,
            link: State::Off,
            bind_failed: None,
            enabled: Arc::new(AtomicBool::new(true)),
            gate,
            ready: Readiness::Unknown,
            ready_at: None,
            ready_gen: 0,
            ready_run: 0,
            ready_pending: false,
            ready_since: None,
            ready_again: false,
            licence: crate::licence::State::Valid,
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
            status_tip: None,
            tray: None,
            tray_shown: None,
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
        apply_ready, autoconnect, autostart_enabled, badge, bind_failure_display, can_cancel, carried_base_url, check_lines, check_status, confirm_gate_text,
        confirm_text, drop_stale_readiness,
        exit_code_for, first_phase, flip_enabled_only, is_stale, licence_line, line_of, link_is_current, link_line, local_display, locks_gate, note_file, note_line, on_done,
        note_licence_change_ui, on_line, on_progress, op_refusal, progress_text, project_primary, publication_detail, readiness_reason, refresh_gate, report_line, save_host,
        publication_failure, readiness_lost, run_readiness, save_needs_confirm, serving, sync_tray, READINESS_LOST,
        stale_line, stale_text, start_readiness, took, tick_redraw, tray_tooltip, verdict_from_checks, Dot, Kind, Line, OpView,
        Readiness, Running, Tab, TrayState,
        Ui, Which, LOG_CAP, LOG_FILE, PUBLICATION_TITLE, TRAY_PNG,
    };
    use crate::cfg::{self, Cfg, Host};
    use crate::session::{self, State};
    use common::check::Check;
    use common::op::OpState;
    use common::{Event, Phase, Progress};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn op(id: &str, dot: Dot) -> OpView {
        OpView { id: id.into(), title: id.into(), dot, detail: String::new() }
    }

    #[test]
    fn a_fresh_ui_has_not_run_the_readiness_check_yet() {
        let ui = Ui::for_test();
        assert!(matches!(ui.ready, Readiness::Unknown), "{:?}", ui.ready);
    }

    /// Переключатель (в любую сторону) делает старый вердикт неактуальным
    /// немедленно, до похода по сети — `toggle_link` зовёт именно это ядро
    /// синхронно, поход (`run_readiness`) не тестируется здесь отдельно: он
    /// настоящий TCP/HTTP, не для юнит-теста (спека «держать их чистыми»).
    #[test]
    fn drop_stale_readiness_resets_to_unknown() {
        let mut ui = Ui::for_test();
        ui.ready = Readiness::Warn(Check::fail("х", "y", "z"));
        drop_stale_readiness(&mut ui);
        assert!(matches!(ui.ready, Readiness::Unknown), "{:?}", ui.ready);
    }

    /// Итог «Проверить» и есть вердикт (спека): полный набор в этот момент
    /// уже посчитан, первый провал — причина в карточке. Без этой связки
    /// живой случай заказчика 2026-10-20 повторяется дословно: точка
    /// зелёная, а проверка тут же нашла два провала.
    #[test]
    fn a_finished_check_becomes_the_verdict() {
        let mut ui = Ui::for_test();
        ui.ready = Readiness::Ok;
        let checks = vec![
            Check::pass("платформа 1С", "есть"),
            Check::note("комплект на носителе", "FAT32"),
            Check::fail("проект", "/home/dev3/project не разложен", "1c-connect install"),
            Check::fail("Gitea", "не отвечает", "dev1c-setup check"),
        ];
        verdict_from_checks(&mut ui, &checks);
        match &ui.ready {
            Readiness::Warn(c) => assert_eq!(c.what, "проект", "первый провал, сведение пропущено"),
            other => panic!("{other:?}"),
        }
        // Проверка прошла — вердикт зелёный, тем же набором.
        verdict_from_checks(&mut ui, &[Check::pass("платформа 1С", "есть"), Check::note("комплект на носителе", "FAT32")]);
        assert!(matches!(ui.ready, Readiness::Ok), "{:?}", ui.ready);
        // Пустой набор — упавший поток проверки: ни «всё хорошо», ни прежний
        // вердикт (тот был `Ok`, и точка осталась бы зелёной).
        verdict_from_checks(&mut ui, &[]);
        assert!(matches!(ui.ready, Readiness::Unknown), "{:?}", ui.ready);
    }

    /// Находка ревью: `again` съедался и при чужом поколении, и тогда
    /// `handle` звал пересчёт, сбрасывая вердикт в `Unknown` (то есть снова в
    /// зелёное) сразу после того, как «Проверить» нашла провалы. Это ровно
    /// случай заказчика со снимка, только в другой ветке.
    #[test]
    fn a_stale_generation_answer_keeps_the_remembered_repeat() {
        let mut ui = Ui::for_test();
        let first = start_readiness(&mut ui, Instant::now()).expect("расчёт после операции пошёл");
        // «Сохранить» пока считают — повтор помечен, второго потока нет.
        assert!(start_readiness(&mut ui, Instant::now()).is_none());
        assert!(ui.ready_again);
        // «Проверить» нашла провал — вердикт жёлтый, поколение чужое.
        verdict_from_checks(&mut ui, &[Check::fail("проект", "не разложен", "1c-connect install")]);

        assert!(!apply_ready(&mut ui, first, None), "ответ прежнего поколения пересчёта не просит");
        assert!(matches!(ui.ready, Readiness::Warn(_)), "свежий итог «Проверить» уцелел: {:?}", ui.ready);
        assert!(ui.ready_again, "обещанный повтор не съеден");
        // Исполняет и снимает отметку тот расчёт, который её читает.
        assert!(start_readiness(&mut ui, Instant::now()).is_some());
        assert!(!ui.ready_again);
    }

    /// Поколение вердикта — та же защита, что у состояния связи
    /// (`link_is_current`): набор считается десяток секунд, и ответ прежнего
    /// расчёта не должен затереть свежий.
    #[test]
    fn a_stale_generation_answer_is_dropped() {
        let mut ui = Ui::for_test();
        ui.ready_gen = 2;
        ui.ready_pending = true;
        assert!(!apply_ready(&mut ui, 2, Some(Check::fail("Gitea", "не отвечает", "dev1c-setup check"))));
        assert!(matches!(ui.ready, Readiness::Warn(_)), "{:?}", ui.ready);

        // Ответ первого поколения приходит позже — вердикт он не трогает.
        // И защёлку тоже: её держит другой, идущий прямо сейчас расчёт
        // (`ready_run`), а снять её чужим ответом значило бы пустить вторую
        // пробу инструментов поверх живой (находка N1 повторного ревью).
        ui.ready_pending = true;
        ui.ready_since = Some(Instant::now());
        ui.ready_run = 2;
        assert!(!apply_ready(&mut ui, 1, None));
        assert!(matches!(ui.ready, Readiness::Warn(_)), "{:?}", ui.ready);
        assert!(ui.ready_pending, "чужой ответ снял защёлку с живого расчёта");
        assert!(ui.ready_since.is_some(), "и отметку времени — сторожу нечем было бы мерить");
        // Ответ владельца снимает и то, и другое.
        assert!(!apply_ready(&mut ui, 2, None));
        assert!(!ui.ready_pending && ui.ready_since.is_none());
    }

    /// Цепочка «Сохранить → публикация → операция» не должна поднимать три
    /// пробы подряд и держать точку «занято» минуту: пока поток считает,
    /// новое событие только помечает повтор.
    #[test]
    fn a_second_recompute_waits_for_the_first() {
        let mut ui = Ui::for_test();
        let gen = start_readiness(&mut ui, Instant::now()).expect("первый расчёт идёт сам");
        assert!(ui.ready_pending);

        assert!(start_readiness(&mut ui, Instant::now()).is_none(), "второго потока нет");
        assert!(ui.ready_again, "повтор помечен");
        assert_eq!(ui.ready_gen, gen, "поколение не трогаем — считает всё тот же поток");

        // Возврат потока: ровно один пересчёт, и ответ про старую почву не применён.
        assert!(apply_ready(&mut ui, gen, None), "по возврату — пересчитать");
        assert!(matches!(ui.ready, Readiness::Unknown), "{:?}", ui.ready);
        assert!(!ui.ready_pending && !ui.ready_again);
        assert_eq!(start_readiness(&mut ui, Instant::now()), Some(gen + 1));
    }

    /// Дефект (заказчик 2026-10-20): карточка не должна повторять подсказку,
    /// уже вписанную в сам текст факта (TOOLS-Blocked несёт её внутри
    /// `detail`, см. `ops_host::tools_state`), и никогда не говорит
    /// «факт N провален».
    #[test]
    fn readiness_reason_says_what_and_what_to_do_without_echoing_the_hint() {
        let blocked = Check::fail(
            "инструменты комплекта",
            "в комплекте нет vrunner/, git/ или httpd/ — скопируйте полный комплект заново",
            "скопируйте полный комплект заново",
        );
        let text = readiness_reason(&blocked);
        assert_eq!(text.matches("скопируйте полный комплект заново").count(), 1, "{text}");
        assert!(!text.to_lowercase().contains("факт"), "{text}");

        let web = Check::fail("веб-сервер", "порт 1818 не отвечает", "запустите Apache/IIS, на котором опубликована база");
        let text = readiness_reason(&web);
        assert!(text.contains("порт 1818 не отвечает") && text.contains("запустите Apache/IIS"), "{text}");
    }

    /// Дефект (заказчик 2026-10-21): в строку итога вписали полный путь
    /// отчёта, и она перестала помещаться в подвал — на снимке «Проблем: 2.
    /// Подробности — отчё…». Итог называет только числа, а путь отчёта
    /// уходит отдельной строкой журнала и подсказкой подвала.
    #[test]
    fn the_check_status_line_stays_short() {
        for failed in [1usize, 2, 99] {
            let l = check_status(failed);
            assert_eq!((l.kind, l.text.as_str()), (Kind::Fail, format!("Проблем: {failed}").as_str()));
            assert!(!l.text.contains("last-check"), "путь отчёта вернулся в строку подвала: {}", l.text);
            assert!(l.text.chars().count() <= 80, "строка итога не влезает в подвал: {}", l.text);
        }
        let ok = check_status(0);
        assert_eq!((ok.kind, ok.text.as_str()), (Kind::Ok, "Проверка пройдена"));
        assert!(ok.text.chars().count() <= 80, "{}", ok.text);
        // Путь не пропал — он уехал в строку журнала (она же подсказка подвала).
        let report = "/home/dev3/.local/share/dev1c/last-check.txt";
        assert_eq!(report_line(report), Line { kind: Kind::Say, text: format!("Отчёт — {report}") });
    }

    #[test]
    fn a_failed_tray_icon_leads_to_the_exit_path_not_a_silent_hang() {
        let ok: Result<(), &str> = Ok(());
        assert_eq!(exit_code_for(&ok), None, "трей построился — выходить незачем");
        let failed: Result<(), &str> = Err("GTK не инициализирован");
        assert_eq!(exit_code_for(&failed), Some(1), "без значка окно не закрыть — код выхода ненулевой");
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

    /// Кнопки окна ведут к тем же четырём операциям, что запрещённые команды
    /// (`Which` — ровно `install`/`uninstall`/`export`/`import`), и после
    /// срока не начинают работу: `run_op` спрашивает эти ворота первым делом.
    /// Пока срок идёт, даже в последний его день, ворота открыты.
    #[test]
    fn an_expired_licence_refuses_the_window_buttons_that_change_the_machine() {
        let no = op_refusal(crate::licence::State::Expired).expect("после срока операция не начинается");
        assert!(no.kind == Kind::Fail && no.text == crate::licence::REFUSAL, "{no:?}");
        assert!(op_refusal(crate::licence::State::Soon { days: 0 }).is_none());
        assert!(op_refusal(crate::licence::State::Valid).is_none());
    }

    /// Окно обязано объяснить, что случилось и до какого числа срок был;
    /// пока срок идёт — ни строки, журнал не место для пустых напоминаний.
    #[test]
    fn the_window_explains_an_expired_licence_and_stays_quiet_while_it_runs() {
        let dead = licence_line(crate::licence::State::Expired).expect("истёкший срок — строка в журнале");
        assert!(dead.kind == Kind::Fail && dead.text.contains(&crate::licence::until_text()), "{dead:?}");
        let soon = licence_line(crate::licence::State::Soon { days: 5 }).expect("предупреждение — строка в журнале");
        assert!(soon.kind == Kind::Warn && soon.text.contains('5'), "{soon:?}");
        assert!(licence_line(crate::licence::State::Valid).is_none());
    }

    #[test]
    fn note_file_writes_the_file_and_not_the_window() {
        let mut ui = Ui::for_test();
        note_file(&ui.kit, &Line { kind: Kind::Say, text: "рабочая область и угол флайаута: 2560x1397+0+0 → 2168,965".into() });
        let text = std::fs::read_to_string(ui.kit.state_path(LOG_FILE)).unwrap();
        assert!(text.contains("рабочая область и угол флайаута"), "{text:?}");
        // Главное: служебная строка не попала ни в список окна, ни в счётчик
        // непрочитанных — человеку её читать незачем.
        assert!(ui.log.is_empty(), "{:?}", ui.log);
        assert_eq!(ui.unread, 0);
        // `note_line` рядом — по-прежнему и в файл, и в окно.
        note_line(&mut ui, Line { kind: Kind::Ok, text: "готово".into() });
        assert_eq!(ui.log.len(), 1);
    }

    #[test]
    fn note_line_appends_marks_and_head_lines_to_the_log_file() {
        let mut ui = Ui::for_test();
        note_line(&mut ui, Line { kind: Kind::Head, text: "— устанавливаю — · 12:34".into() });
        note_line(&mut ui, Line { kind: Kind::Ok, text: "готово".into() });
        let text = std::fs::read_to_string(ui.kit.state_path(LOG_FILE)).unwrap();
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
        // Провал вердикта — свой цвет, не зелёный и не жёлтый «занято».
        let warn = badge(&base, TrayState::Warn);
        assert_eq!(at(&warn, c, c), vec![0xe8, 0x7a, 0x14]);
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

    /// Прежняя `tray_state` (своё правило значка: «поднята связь — зелёный»)
    /// удалена вместе с этим тестом: двух правд о значке быть не должно, а
    /// состояние теперь считает точка карточки — `tray_state_now`, и
    /// сторожит его `tray::tests::the_tray_follows_the_card_order`.
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
        assert_eq!(tray_tooltip(Some(&r), &State::Up { since: now }, None, now), "1c-connect: выгружаю конфигурацию из 1С, 12 мин");
        let fresh = Running { started: now, seen: now, ..r };
        assert_eq!(tray_tooltip(Some(&fresh), &State::Off, None, now), "1c-connect: выгружаю конфигурацию из 1С, меньше минуты");
        assert_eq!(tray_tooltip(None, &State::Up { since: now }, None, now), "1c-connect: подключён");
        assert_eq!(tray_tooltip(None, &State::Off, None, now), "1c-connect: не подключён");
        // Причина замка — в подсказке значка; идущая операция её побеждает
        // (тот же порядок разрешения, что в карточке).
        let why = Some("не задан пользователь базы");
        assert_eq!(tray_tooltip(None, &State::Off, why, now), "1c-connect: включать нечего — не задан пользователь базы");
        assert_eq!(tray_tooltip(Some(&r), &State::Off, why, now), "1c-connect: выгружаю конфигурацию из 1С, 12 мин");
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
        let before = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok();
        let mut host = ui.cfg.host.clone();
        host.server = "no-port".into();
        let err = save_host(&mut ui, host).unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok(), before);
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
        assert_eq!(link_line(&State::Connecting, &h, "tunnel").unwrap().text, "подключаюсь к s:1 как dev2");
        assert_eq!(link_line(&State::Off, &h, "tunnel").unwrap().text, "отключено");
        assert!(link_line(&State::Up { since: Instant::now() }, &h, "tunnel").is_none());
    }

    #[test]
    fn local_mode_connecting_line_hides_the_environment_fields() {
        // Адрес среды, пароль и имя рабочего места скрыты в «Параметрах» в
        // локальном режиме и ничего не значат — журнал не должен о них
        // говорить (находка целостного ревью).
        let h = Host { server: "s:1".into(), workstation: "dev2".into(), ..Default::default() };
        let text = link_line(&State::Connecting, &h, "local").unwrap().text;
        assert!(!text.contains("s:1") && !text.contains("dev2"), "{text}");
    }

    #[test]
    fn a_bind_failure_stays_visible_regardless_of_the_switch() {
        // Отказ («порт занят») переживает любое число щелчков: карточка не
        // должна соврать «MCP включены», когда ничего не слушает. Решает по
        // сохранённому `bind_failed`, а не по текущему показанному
        // состоянию — повторная находка ревью: прежний
        // `keep_failure_on_enable` ловил лишь один клик «включить», а
        // «выключить» перед ним стирал отказ в `Off`.
        assert_eq!(bind_failure_display(&Some("порт занят".into())), Some(State::Failed("порт занят".into())));
        assert_eq!(bind_failure_display(&None), None);
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
        // "webserver": ensure_publication_now (зовётся теперь и отсюда) не
        // должна трогать настоящий httpd/1cv8 этой машины — тест про
        // перезапуск сеанса, не про публикацию.
        ui.cfg.host.publication = "webserver".into();
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
    fn saving_a_new_base_in_local_mode_keeps_the_session_and_does_not_restart() {
        // Блокирующая находка целостного ревью: раньше «Сохранить» рвало
        // слушатели и принятые соединения в локальном режиме, и человеку
        // приходилось переподключать два сервера в Zed руками. Спека прямо
        // требует, чтобы это было незаметно: слушатели не трогаются, сеанс
        // не перезапускается, следующий вызов уходит в новую базу через
        // `Session::update_cfg`.
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.cfg.host.publication = "webserver".into();
        ui.session = Some(doomed_session(&ui));
        let gen_before = ui.session_gen;
        let log_before = ui.log.len();
        let mut host = ui.cfg.host.clone();
        host.base_path = "C:\\Bases\\Other".into();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.session_gen, gen_before, "сеанс не перезапущен");
        let new_lines = &ui.log[log_before..];
        assert_eq!(new_lines.len(), 1, "{new_lines:?}");
        assert_eq!(new_lines[0].text, "MCP: параметры обновлены, база C:\\Bases\\Other");
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
        // "webserver": ensure_publication_now не должна трогать настоящий
        // httpd/1cv8 этой машины.
        let mut ui = Ui::for_test();
        ui.cfg.host.publication = "webserver".into();
        let log_before = ui.log.len();
        let mut host = ui.cfg.host.clone();
        host.base_path = "C:\\Bases\\Other".into();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.log.len(), log_before);
    }

    /// «Подключаться при запуске» замок не обходит: коннектор поднимается,
    /// обслуживание остаётся выключенным, причина уходит строкой в журнал
    /// (сама строка — в `launch`, туда теста не дотянуться, как и до всего
    /// остального в нём).
    #[test]
    fn autostart_does_not_enable_while_the_gate_holds() {
        assert!(!autostart_enabled(Some("не задан пользователь базы"), true), "замок заперт — обслуживание не включается");
        assert!(autostart_enabled(None, true), "условия в порядке — галочка работает как раньше");
        assert!(!autostart_enabled(None, false), "галочки нет — включать и нечего");
        assert!(!autostart_enabled(Some("в комплекте нет mcp-1c"), false));
    }

    /// «Сохранить», убирающее холодное условие на живом обслуживании,
    /// спрашивает тем же диалогом, что и смена базы. Живого замка здесь нет
    /// (в каталоге теста не разложены ни инструменты, ни платформа), поэтому
    /// проверяется та же пара, что работает в окне: `cold_gate` на новых
    /// полях и `locks_gate` поверх неё.
    #[test]
    fn saving_away_a_cold_condition_asks_first() {
        use crate::gui::gate::cold_gate;
        let v8 = std::path::Path::new("1cv8.exe");
        let tools = OpState::Done("vrunner 3.0.2".into());
        let before = Cfg::default();
        let mut after = Cfg::default();
        after.host.db_user = String::new();
        let gate_before = cold_gate(&before, true, true, &tools, Some(v8), true, &crate::licence::State::Valid);
        let gate_after = cold_gate(&after, true, true, &tools, Some(v8), true, &crate::licence::State::Valid);
        assert_eq!(gate_before, None, "до правки включать было можно");
        let why = locks_gate(gate_before.as_deref(), gate_after.clone(), true).expect("вопрос обязателен");
        assert!(why.contains("пользователь базы"), "{why}");
        // Обслуживания нет — гасить нечего, вопрос не задаём.
        assert_eq!(locks_gate(gate_before.as_deref(), gate_after.clone(), false), None);
        // Заперто и до правки — тоже молчим: обслуживание уже погашено.
        assert_eq!(locks_gate(Some("в комплекте нет mcp-1c"), gate_after.clone(), true), None);
        // Правка ничего не убрала — обычное «Сохранить» без вопроса.
        assert_eq!(locks_gate(gate_before.as_deref(), None, true), None);
        // Диалог называет последствие, а не одно «Сохранить?».
        let (message, detail) = confirm_gate_text(&why);
        assert!(message.contains("MCP выключатся"), "{message}");
        assert!(detail.contains("пользователь базы"), "{detail}");

        // И то же через настоящий вход окна. Поле берём такое, которого нет в
        // списке «сменилась ли база» (`database_changed`), иначе вопрос
        // пришёл бы по старому поводу и ничего бы не доказал.
        let mut ui = Ui::for_test();
        ui.session = Some(doomed_session(&ui));
        ui.gate = None;
        let mut host = ui.cfg.host.clone();
        host.server = String::new();
        assert!(save_needs_confirm(&ui, &host), "туннель без адреса среды — обслуживание погаснет, вопрос обязателен");
        ui.gate = Some("в комплекте нет mcp-1c".into());
        assert!(!save_needs_confirm(&ui, &host), "замок заперт и так — гасить нечего");
        let _ = ui.session.take().map(|s| s.stop());
    }

    /// Проводка замка к мосту: пока он заперт, отказ агенту несёт причину.
    /// Сверяем не текст (в каталоге теста замок запирает что придётся, и
    /// соседние тесты пишут в тот же процессный глобал), а то, что причина
    /// вообще доезжает — без неё отказ советовал бы включить то, чего
    /// включить нельзя.
    #[test]
    fn the_gate_reason_reaches_the_bridge() {
        let mut ui = Ui::for_test();
        refresh_gate(&mut ui);
        assert!(ui.gate.is_some(), "в каталоге теста нет ни параметров, ни инструментов — замок заперт");
        let reason = crate::bridge::off_reason();
        assert!(reason.is_some(), "причина замка не доехала до моста");
        assert!(crate::bridge::disabled_text(reason.as_deref()).starts_with(&format!("{}: ", crate::bridge::DISABLED_PREFIX)));
    }

    /// То же для отказа публикации (находка N2): человек ничего не выключал,
    /// и «включите переключатель в окне» было бы советом не по делу —
    /// агент должен услышать, что лежит публикация.
    #[test]
    fn a_failed_publication_tells_the_agent_what_really_happened() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.gate = None;
        ui.enabled.store(true, Ordering::Relaxed);

        let shown = publication_failure(&mut ui, "расширение не ответило");
        ui.link = shown;
        sync_tray(&mut ui, Instant::now());

        let text = crate::bridge::disabled_text(crate::bridge::off_reason().as_deref());
        assert!(text.contains("расширение не ответило"), "{text}");
        assert_ne!(text, crate::bridge::DISABLED_TEXT, "агенту советуют включить то, чего никто не выключал");

        // А когда выключил человек, прежний совет и есть правда.
        ui.link = State::Off;
        ui.bind_failed = None;
        sync_tray(&mut ui, Instant::now());
        assert_eq!(crate::bridge::disabled_text(crate::bridge::off_reason().as_deref()), crate::bridge::DISABLED_TEXT);
    }

    /// Проводка гашения: «Сохранить», убравшее холодное условие, снимает флаг,
    /// переводит карточку в «выключено» и пишет строку. Без последнего человек
    /// видел бы зелёное «MCP включены» и переключатель, который нарисован
    /// включённым, но на щелчок не отвечает (находка ревью).
    #[test]
    fn saving_away_a_cold_condition_douses_the_card_too() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.cfg.host.publication = "webserver".into();
        ui.session = Some(doomed_session(&ui));
        ui.link = State::Up { since: Instant::now() };
        // До правки замок был открыт — тот самый случай, ради которого
        // «Сохранить» и спрашивает подтверждение (в каталоге теста он заперт
        // сам по себе, поэтому состояние задаём явно).
        ui.gate = None;
        let log_before = ui.log.len();
        let mut host = ui.cfg.host.clone();
        host.db_user = String::new();
        save_host(&mut ui, host).unwrap();
        assert!(ui.gate.is_some(), "после правки условие пропало");
        assert!(!ui.enabled.load(Ordering::Relaxed), "обслуживание погашено");
        assert_eq!(ui.link, State::Off, "карточка и переключатель обязаны показать «выключено»");
        // Строка про гашение ровно одна (про обновлённые параметры рядом —
        // своя и законная: это другое событие).
        let doused: Vec<&Line> = ui.log[log_before..].iter().filter(|l| l.text.starts_with("MCP выключены:")).collect();
        // Какое именно условие назовёт строка, решает порядок `cold_gate`, а в
        // каталоге теста не выполнено сразу несколько — здесь проверяется сам
        // факт гашения со строкой, а список условий стережёт `gate::tests`.
        assert_eq!(doused.len(), 1, "{:?}", &ui.log[log_before..]);
        let _ = ui.session.take().map(|s| s.stop());
    }

    /// Спека: при выключенном обслуживании вердикт не считается — показывать
    /// его негде, а набор стоит 20–40 с и поднимает пробу инструментов
    /// (на Linux ещё и виртуальный экран). Замок при этом пересчитывается
    /// всегда: он решает, доступен ли переключатель (находка S9).
    #[test]
    fn the_verdict_is_not_computed_while_serving_is_off() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(false, Ordering::Relaxed);
        // «Уже считается», чтобы доказательство не подняло настоящий поток
        // фактов: пересчёт тогда только пометит повтор — эта пометка и
        // показывает, дошло ли до расчёта дело.
        ui.ready_pending = true;
        ui.ready_since = Some(Instant::now());
        ui.ready_again = false;
        ui.tray_shown = None;

        run_readiness(&mut ui);
        assert!(!ui.ready_again, "вердикт считается при выключенном обслуживании");
        assert!(ui.tray_shown.is_some(), "замок обязан пересчитываться и доезжать до значка всегда");

        // Включили — считается само, отдельного события не нужно.
        ui.enabled.store(true, Ordering::Relaxed);
        run_readiness(&mut ui);
        assert!(ui.ready_again, "обслуживание включено, а пересчёт не пошёл");
    }

    /// Сторож зависшего расчёта (находка S10): бюджет проверяется до факта, а
    /// не прерывает его, и поток может не ответить никогда. Залипшая защёлка
    /// означала бы вердикт, который больше не посчитается ни разу, — точка
    /// навсегда осталась бы «Проверяю…».
    #[test]
    fn a_lost_readiness_thread_does_not_latch_forever() {
        // Якорь в будущем: на свежей машине `Instant::now() - 5 мин` паникует.
        let now = Instant::now() + Duration::from_secs(3600);
        assert!(!readiness_lost(Some(now), now));
        assert!(!readiness_lost(Some(now), now + READINESS_LOST), "срок ещё не вышел");
        assert!(readiness_lost(Some(now), now + READINESS_LOST + Duration::from_secs(1)));
        assert!(readiness_lost(None, now), "защёлку взвели мимо `start_readiness` — ждать нечего");

        // Проводка: пока поток жив, второго не заводим; потерянный — заводим.
        let mut ui = Ui::for_test();
        let gen = start_readiness(&mut ui, now).expect("первый расчёт идёт сам");
        assert!(start_readiness(&mut ui, now + Duration::from_secs(30)).is_none(), "второй поток поверх живого");
        assert_eq!(start_readiness(&mut ui, now + READINESS_LOST + Duration::from_secs(1)), Some(gen + 1));
        // Ответ снимает и защёлку, и отметку времени — иначе следующий
        // расчёт считал бы себя потерянным с рождения.
        apply_ready(&mut ui, gen + 1, None);
        assert!(!ui.ready_pending && ui.ready_since.is_none());
    }

    /// «Обслуживаем» значит то, что читает мост, — флаг. Туннелю нужен ещё и
    /// сеанс, но сеанс сам по себе обслуживанием не является: с погашенным
    /// флагом поднятый туннель показывал зелёное «Подключён», пока агенту
    /// отказывали на каждом вызове (находка S5 итогового ревью).
    #[test]
    fn serving_means_the_flag_the_bridge_reads() {
        let mut ui = Ui::for_test();
        assert_eq!(ui.cfg.host.mode, "tunnel");
        ui.link = State::Up { since: Instant::now() };
        ui.enabled.store(false, Ordering::Relaxed);
        assert!(!serving(&ui), "туннель поднят, а обслуживание погашено — это не «обслуживаем»");
        ui.enabled.store(true, Ordering::Relaxed);
        assert!(serving(&ui));
        ui.link = State::Off;
        assert!(!serving(&ui), "в туннеле без сеанса вызову некуда ехать");
        // В локальном режиме слушатели подняты всегда — говорит только флаг.
        ui.cfg.host.mode = "local".into();
        assert!(serving(&ui));
        ui.enabled.store(false, Ordering::Relaxed);
        assert!(!serving(&ui));
    }

    /// Отказ публикации гасит обслуживание, а не только красит карточку:
    /// иначе показанное («Не подключён») расходится с настоящим, и первый
    /// щелчок по переключателю выключает вместо того, чтобы включить
    /// (находка S6 итогового ревью).
    #[test]
    fn a_failed_publication_douses_serving() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(true, Ordering::Relaxed);
        let shown = publication_failure(&mut ui, "расширение не ответило");
        match &shown {
            State::Failed(why) => assert!(why.contains("расширение не ответило"), "{why}"),
            other => panic!("ожидался отказ, а не {other:?}"),
        }
        assert!(!serving(&ui), "карточка говорит «Не подключён», а вызовы обслуживаются");
    }

    /// Проводка, а не чистая функция (находка S1): вердикт становится
    /// известен в `apply_ready`, и оттуда он обязан дойти до значка и до
    /// журнала. Раньше `Ev::Ready` звал только перерисовку окна, и
    /// установившееся положение дел было «карточка жёлтая, значок зелёный».
    #[test]
    fn a_verdict_reaches_the_tray_and_the_log() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.link = State::Up { since: Instant::now() };
        // Замок закрыл бы карточку собой (в каталоге теста он заперт), а речь
        // здесь про вердикт.
        ui.gate = None;
        ui.tray_shown = None;
        let gen = ui.ready_gen;
        let fail = Check::fail("проект", "/home/dev3/project не разложен", "1c-connect install");

        let log_before = ui.log.len();
        ui.ready_pending = true;
        assert!(!apply_ready(&mut ui, gen, Some(fail.clone())));
        let shown = ui.tray_shown.clone().expect("вердикт не дошёл до значка");
        assert_eq!(shown.state, TrayState::Warn, "значок остался прежним, пока карточка жёлтая");
        // И строкой в журнал: подсказка по наведению — не единственный канал
        // (спека называет три, журнал молчал — находка S3).
        assert_eq!(ui.log.len(), log_before + 1, "{:?}", &ui.log[log_before..]);
        let line = ui.log.last().unwrap();
        assert_eq!(line.kind, Kind::Warn);
        assert!(line.text.contains("/home/dev3/project не разложен"), "{}", line.text);

        // Та же причина во второй раз журнал не засыпает: вердикт
        // пересчитывается по каждому событию.
        ui.ready_pending = true;
        apply_ready(&mut ui, gen, Some(fail));
        assert_eq!(ui.log.len(), log_before + 1, "{:?}", &ui.log[log_before..]);

        // А зелёный вердикт доезжает до значка так же.
        ui.ready_pending = true;
        apply_ready(&mut ui, gen, None);
        assert_eq!(ui.tray_shown.unwrap().state, TrayState::Up);
    }

    /// Проводка замка до трея (находка S2): `sync_tray` стояла только внутри
    /// ветки гашения, поэтому замок, защёлкнувшийся при выключённом
    /// обслуживании, оставлял пункт «Включить MCP» нажимаемым — меню обещало
    /// то, в чём `toggle_link` потом отказывал.
    #[test]
    fn a_latching_gate_reaches_the_tray_even_with_nothing_to_douse() {
        let mut ui = Ui::for_test();
        // Гасить нечего — ветка `douse_serving` не берётся вовсе.
        ui.enabled.store(false, Ordering::Relaxed);
        ui.gate = None;
        ui.tray_shown = None;
        refresh_gate(&mut ui);
        assert!(ui.gate.is_some(), "в каталоге теста замок обязан защёлкнуться");
        let shown = ui.tray_shown.clone().expect("замок не дошёл до трея");
        assert!(shown.locked, "пункт меню остался нажимаемым при запертом замке");
        // Пункт трея и переключатель окна читают один и тот же замок.
        assert_eq!(shown.locked, ui.gate.is_some());
    }

    /// Проводка срока (находка S4): мост спрашивает часы на каждый вызов и
    /// после полуночи отказывает всем, а окно смотрело в запомненное и
    /// показывало зелёное. Смена состояния лицензии — такое же событие,
    /// меняющее почву, и кончается тем же пересчётом.
    #[test]
    fn a_licence_change_recomputes_readiness() {
        let mut ui = Ui::for_test();
        // Запомненное состояние заведомо отличается от настоящего, какое бы
        // оно ни было сегодня.
        let real = crate::licence::state_now();
        ui.licence = if real == crate::licence::State::Expired { crate::licence::State::Valid } else { crate::licence::State::Expired };
        // Обслуживание включено — иначе пересчёт и не начнётся (S9), и
        // доказательство ниже ничего бы не доказало.
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(true, Ordering::Relaxed);
        // Поток фактов в тесте нам не нужен: «уже считается» (и не потеряно,
        // см. `readiness_lost`) заставляет пересчёт только пометить повтор —
        // эта пометка и есть доказательство, что `run_readiness` позвали.
        ui.ready_pending = true;
        ui.ready_since = Some(Instant::now());
        ui.ready_again = false;
        ui.tray_shown = None;

        note_licence_change_ui(&mut ui);

        assert_eq!(ui.licence, real, "новое состояние срока не запомнено");
        assert!(ui.ready_again, "смена срока не привела к пересчёту готовности");
        assert!(ui.tray_shown.is_some(), "пересчёт не дошёл до значка");
    }

    /// Замок гасит обслуживание везде, где защёлкивается, а не только на
    /// «Сохранить». Сценарий заказчика: человек правит toml руками, условие
    /// пропадает, окно на показе перечитывает файл и пересчитывает ворота —
    /// карточка говорит «MCP выключены», и агента обслуживать больше нельзя.
    /// Раньше замок правил одни слова в отказе, а флаг оставался поднятым.
    #[test]
    fn a_gate_that_latches_douses_serving_once() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.link = State::Up { since: Instant::now() };
        assert!(ui.enabled.load(Ordering::Relaxed), "фикстура заводит обслуживание включённым");
        // Замок был открыт, обслуживание шло — ровно то положение, из которого
        // правка toml руками роняет условие. «Сохранить» здесь никто не жал.
        ui.gate = None;
        let log_before = ui.log.len();
        refresh_gate(&mut ui);
        assert!(ui.gate.is_some(), "в каталоге теста условия не выполнены");
        assert!(!ui.enabled.load(Ordering::Relaxed), "обслуживание погашено пересчётом, а не «Сохранить»");
        assert_eq!(ui.link, State::Off, "карточка и переключатель показывают «выключено»");
        assert_eq!(ui.log.len(), log_before + 1, "о гашении сказано один раз");
        // Второй пересчёт при том же замке молчит: гасить уже нечего.
        let log_after = ui.log.len();
        refresh_gate(&mut ui);
        assert_eq!(ui.log.len(), log_after);
    }

    /// Обратное направление: включать обслуживание ворота не умеют — это дело
    /// человека. Пересчёт при выключенном обслуживании не трогает ни флага,
    /// ни журнала; переключатель отпирается сам, по `ui.gate`
    /// (`panel::switch_locked`).
    #[test]
    fn the_gate_never_switches_serving_back_on() {
        let mut ui = Ui::for_test();
        ui.enabled.store(false, Ordering::Relaxed);
        ui.gate = None;
        let log_before = ui.log.len();
        refresh_gate(&mut ui);
        assert!(ui.gate.is_some(), "замок защёлкнулся");
        assert!(!ui.enabled.load(Ordering::Relaxed), "ворота обслуживание не включают");
        assert_eq!(ui.log.len(), log_before, "гасить было нечего — говорить не о чем");
    }

    #[test]
    fn save_needs_confirm_only_when_all_three_hold() {
        // Три условия сразу: база меняется, MCP включены, сеанс жив —
        // вопрос нужен.
        let mut ui = Ui::for_test();
        ui.session = Some(doomed_session(&ui));
        assert!(ui.enabled.load(Ordering::Relaxed), "фикстура заводит флаг включённым");
        let mut new_base = ui.cfg.host.clone();
        new_base.base_path = "C:\\Bases\\Other".into();
        assert!(save_needs_confirm(&ui, &new_base));
        let _ = ui.session.take().unwrap().stop();
    }

    #[test]
    fn save_needs_confirm_is_false_on_each_near_miss() {
        let mut ui = Ui::for_test();
        ui.session = Some(doomed_session(&ui));
        let mut new_base = ui.cfg.host.clone();
        new_base.base_path = "C:\\Bases\\Other".into();

        // Промах 1: MCP выключены.
        ui.enabled.store(false, Ordering::Relaxed);
        assert!(!save_needs_confirm(&ui, &new_base));
        ui.enabled.store(true, Ordering::Relaxed);

        // Промах 2: база та же, поменялось другое поле — оно не входит в
        // список, что решает «изменилась ли база» (`database_changed`).
        let mut other_field = ui.cfg.host.clone();
        other_field.project = "D:\\other\\proj".into();
        assert!(!save_needs_confirm(&ui, &other_field));

        // Промах 3: сеанса нет.
        let session = ui.session.take().unwrap();
        assert!(!save_needs_confirm(&ui, &new_base));
        let _ = session.stop();
    }

    #[test]
    fn declining_the_confirmation_leaves_the_toml_untouched() {
        // Пока вопрос не задан и не отвечен «да», «Сохранить» не должен
        // звать `save_host` вовсе — отказ ничего не пишет, поля окна
        // остаются как человек их оставил (сюда это проверяется тем, что
        // toml не тронут без явного вызова `save_host`).
        let mut ui = Ui::for_test();
        ui.session = Some(doomed_session(&ui));
        let before = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok();
        let mut new_base = ui.cfg.host.clone();
        new_base.base_path = "C:\\Bases\\Other".into();
        assert!(save_needs_confirm(&ui, &new_base), "сценарий должен требовать вопрос");
        // Отказ = не позвать save_host; toml как был.
        assert_eq!(std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).ok(), before);
        let _ = ui.session.take().unwrap().stop();
    }

    #[test]
    fn confirm_text_names_folders_and_keeps_full_paths_in_the_detail() {
        let (message, detail) = confirm_text("C:\\Bases\\Buh", "C:\\Bases\\УНФ");
        assert_eq!(message, "Агент сейчас работает с базой «Buh», после сохранения перейдёт на «УНФ». Сохранить?");
        assert_eq!(detail, "C:\\Bases\\Buh → C:\\Bases\\УНФ");
    }
}
