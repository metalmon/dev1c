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
use common::op::CleanLevel;
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

// Разделы окна: у каждого свои тесты рядом с кодом, и правка в разделе
// проверяется им одним — `cargo test gui::facts` вместо всего набора.
// Имена переэкспортированы: внешние пути (`gui::run_readiness` из `panel.rs`,
// `gui::save_host` из окна) остались теми же, что до разделения.
mod facts;
mod link;
mod log;
mod save;
pub use facts::*;
pub use link::*;
pub use log::*;
pub use save::*;

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
    /// независимый от переключателя: «выключить» его не чинит — порт занят
    /// при любом положении флага. Заводится (`Ev::Link` с `Failed`) и
    /// снимается (`Ev::Link` с `Up`) настоящим сеансом, а новая попытка
    /// (`start_session`) стирает его вперёд: сеанс, которому он принадлежал,
    /// кончился, и держатель порта мог уже уйти. Без
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
    /// Набор фактов последнего расчёта (`verify::gather`/`verify::cheap`) —
    /// ОДИН набор, из которого выведен и вердикт (`verdict_of`), и всё, что
    /// показывают поверхности (карточка, вкладка «Проект», подвал, значок).
    /// Пуст, пока ничего не посчитано, либо когда обслуживание выключено
    /// (`apply_facts`).
    pub facts: Vec<Check>,
    /// Вердикт — чистая функция от `facts` и того, обслуживаем ли мы вызовы
    /// (`verdict_of`). Хранится здесь, а не пересчитывается на каждый показ,
    /// только чтобы не гонять `verdict_of` по кругу — источник правды всё
    /// равно один: `facts`.
    pub verdict: Verdict,
    /// На какой час верен набор (`gui::clock`, «15:54»); `None` — набора нет
    /// (`facts` пуст). Пересчитывается по событиям и по такту, а не
    /// непрерывно (спека, «Чем за это платим»), значит бывает устаревшим —
    /// и человек должен видеть, на какой момент он верен. Пишется там же,
    /// где набор становится известен: `apply_facts`.
    pub facts_at: Option<String>,
    /// Поколение набора: растёт на каждый начатый расчёт и на каждый итог
    /// «Проверить». Ответ чужого поколения отбрасывается — так же, как
    /// хвостовое событие прежнего сеанса (`session_gen`): набор считается
    /// десяток секунд, и окно для гонки широкое.
    pub facts_gen: u64,
    /// Расчёт набора идёт прямо сейчас — второй поток не заводится ни по
    /// такту, ни по кнопке «Проверить» (задача 6: «один расчёт в полёте, не
    /// два») — обе защёлка общая.
    pub facts_pending: bool,
    /// Когда начался идущий расчёт. Нужен сторожу `readiness_lost`: поток
    /// может не ответить никогда (бюджет проверяется **до** факта, а не
    /// прерывает его, и `holder::of_port` поднимает `ss`/`netstat` вовсе без
    /// срока), а залипшая защёлка `facts_pending` означала бы набор, который
    /// больше не посчитается ни разу, и точку, которая никогда не станет
    /// зелёной (находка S10).
    pub facts_since: Option<Instant>,
    /// Пока поток считал, почва сдвинулась ещё раз: пересчитать один раз по
    /// его возвращении. Иначе цепочка «Сохранить → публикация → операция»
    /// поднимала бы три пробы подряд.
    pub facts_again: bool,
    /// Кнопка «Проверить» попросила громкий (с отчётом и строками в журнал)
    /// показ — если защёлка (`facts_pending`) была занята чужим расчётом
    /// (тактом или прежней кнопкой), сама кнопка второй поток не заводит
    /// (задача 6, «Добавлено управляющим»): она либо начинает его сама
    /// (защёлка была свободна), либо просит громкий показ у того расчёта,
    /// что уже идёт (`facts_again` вместе с этим флагом). Снимается тем
    /// проходом, чей результат наконец применится (`Ev::Ready`), не раньше.
    pub check_requested: bool,
    /// Дешёвый набор прошлого такта (`verify::cheap`) — с ним сравнивает
    /// следующий такт (`needs_full_pass`), не изменился ли исход дешёвого
    /// факта (скажем, открыли/закрыли базу между тактами).
    ///
    /// Пишется только вместе с расчётом, который его прочёл (`on_cheap`,
    /// Minor 11 ревью задач 5+6): запомнить изменение, по которому расчёт
    /// только помечен повтором, значит потерять его между тактом и
    /// применённым набором.
    pub prev_cheap: Vec<Check>,
    /// Число открытых сеансов 1С с последнего опроса; `None` — не спрашивали
    /// ни разу. Это то единственное, ради чего нужен секундный такт (спека,
    /// цель 6: «человек закрыл Предприятие — через секунду-две позеленело»):
    /// его смена и есть повод пойти за дорогими фактами.
    pub sessions: Option<usize>,
    /// Дешёвый такт считается прямо сейчас в своём потоке — второго не
    /// заводим. На потоке окна такт не считает ничего (Important 4 ревью
    /// задач 5+6): `verify::cheap` стоит 230–330 мс, а
    /// `bridge::configurator_count` на Windows стоил до половины секунды, и
    /// всё это шло на потоке окна ровно когда человек на него смотрит.
    pub cheap_pending: bool,
    /// Когда последний раз считался ВЕСЬ дешёвый набор (`verify::cheap`).
    /// `None` — ни разу: первый же такт обязан посчитать его целиком.
    pub cheap_at: Option<Instant>,
    /// Когда такт последний раз перерисовывал окно в покое. Шаг такта при
    /// открытом окне — секунда, а перерисовка от этого учащаться не должна:
    /// в покое её ждёт только счётчик минут в карточке связи (`tick_redraw`).
    pub redraw_at: Option<Instant>,
    /// Когда закончился последний полный (с дорогими фактами) расчёт.
    /// `None` — ни разу: такт обязан пойти за дорогими фактами на первом же
    /// такте, а не считать себя «только что посчитавшим».
    pub facts_full_at: Option<Instant>,
    /// Состояние заглушки лицензии, о котором уже сказано в журнале. Решений
    /// на нём не принимают — и кнопки (`run_op`), и мост спрашивают часы
    /// заново; поле нужно только чтобы заметить смену под открытым окном и
    /// сказать о ней один раз, а не каждый тик.
    pub licence: crate::licence::State,
    pub log: Vec<Line>,
    pub unread: usize,
    pub tab: Tab,
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
    /// Число живых потоков `refresh_project` — растёт в момент
    /// `thread::spawn`, падает (насыщенно) в `Ev::Project`. Входит
    /// в `working()`: полоса хода в разделителе над подвалом.
    pub pending: usize,
    pub status: Option<Line>,
    /// Что подвал говорит по наведению сверх самого итога — путь отчёта
    /// после проверки. Сам итог короткий (`check_status`): длинный путь
    /// рядом с кнопкой не помещается, а потерять его нельзя. Ставится и
    /// снимается только вместе со `status` (`set_status`), иначе подсказка
    /// пережила бы итог, к которому её написали.
    pub status_tip: Option<String>,
    /// Когда итог положили в подвал. Снимается он не первым же пришедшим
    /// набором фактов (так было до находки ревью 26.10: человек нажимал
    /// «Установить», получал отказ — и подвал гас раньше, чем он дочитал,
    /// потому что следом шёл пересчёт «почва могла сдвинуться»), а по
    /// собственному сроку `STATUS_LINGER`.
    pub status_at: Option<Instant>,
    /// Подняли ли МЫ публикацию в этом запуске. Выход гасит свой httpd
    /// только тогда: `httpd.pid` и `httpd.conf` лежат в состоянии
    /// ПОЛЬЗОВАТЕЛЯ ОС, один на всех, а замок единственного экземпляра
    /// заведён на пользователя ПЛЮС имя рабочего места — то есть два
    /// коннектора одного человека с разными именами живут одновременно и
    /// делят один pid-файл. Без этого флага выход одного гасил бы httpd
    /// соседа (находка ревью 26.10). Чего не подняли — того не трогаем.
    pub raised_publication: bool,
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
    /// фоновое обновление состояния (`pending` — `refresh_project`
    /// ещё не ответил). Полоса хода в разделителе над подвалом читает это.
    pub fn working(&self) -> bool {
        self.busy.is_some() || matches!(self.link, State::Connecting) || self.pending > 0
    }
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
    /// Занят мьютекс проекта (идёт наша выгрузка или загрузка) — это
    /// единственная причина молчания вкладки, которой НЕТ в наборе фактов:
    /// раскладку (`Layout`) набор называет сам, своими словами и со своим
    /// советом, и повторять его карточкой незачем (находка владельца 26.10:
    /// «про проект и ветку два раза зачем-то пишем»).
    pub busy: bool,
    /// Разобранное состояние git; `None` — раскладки нет, замок занят или
    /// git ответил ошибкой: причина тогда в `why`.
    pub facts: Option<project::Facts>,
    pub ok: bool,
    pub why: String,
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
    /// Каталог проекта, перечитанный в своём потоке (`refresh_project`).
    Project(ProjectView),
    /// Ответ `ensure_publication`, поднятой в своём потоке: проба (до 3 с),
    /// проверка расширения в режиме «webserver» и ожидание после spawn не
    /// должны держать главный поток окна (находка повторного ревью).
    Publish(Result<(), String>),
    /// Поколение и набор, посчитанные в своём потоке (`spawn_full_pass`) —
    /// один путь и для такта, и для кнопки «Проверить» (задача 6: один
    /// расчёт в полёте, не два). `loud` — громкий показ (отчёт на диске,
    /// строки в журнал, вкладка «Журнал»): его просила кнопка, а не такт.
    Ready(u64, Vec<Check>, bool),
    /// Итог дешёвого такта, посчитанный в своём потоке (`spawn_cheap`,
    /// Important 4): число открытых сеансов 1С и — только на медленном такте
    /// — весь дешёвый набор. `None` в любом из полей значит «не спрашивали»
    /// (или поток упал с паникой): нуль сеансов — это ответ, а не его
    /// отсутствие, и подменять им незнание нельзя.
    Cheap { sessions: Option<usize>, facts: Option<Vec<Check>> },
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

/// Подсказка трея: во время операции — фаза строчными и сколько она идёт
/// («выгружаю конфигурацию, 12 мин»; меньше минуты — «меньше минуты»); без
/// операции — просто про связь.
/// Причина замка попадает в подсказку значка наравне с фазой операции; идущая
/// операция побеждает — тот же порядок разрешения, что в карточке.
///
/// Жёлтый вердикт тоже называет причину (Critical 2 ревью задач 5+6): значок
/// желтел на «занято» и оранжевел на «не всё готово», а подсказка при этом
/// говорила «подключён» — человек видел цвет без объяснения. Порядок и условие
/// те же, что у карточки (`panel::card`/`panel::reason`): вердикт показывается
/// при поднятом обслуживании, в ОБОИХ режимах (Critical 1 финального ревью —
/// исключения по режиму нет), иначе подсказка и карточка разошлись бы.
pub fn tray_tooltip(progress: Option<&Running>, link: &State, verdict: &Verdict, gate: Option<&str>, now: Instant) -> String {
    if let (None, Some(why)) = (progress, gate) {
        // Та же формулировка, что у подсказки переключателя и подвала:
        // три поверхности говорили о замке тремя разными фразами.
        return format!("1c-connect: {}", crate::gui::panel::gate_text(why).to_lowercase());
    }
    if progress.is_none() && link.is_up() {
        if let Some(why) = warn_reason(verdict) {
            return format!("1c-connect: {why}");
        }
        // Набора ещё нет — так и говорим (Minor 10 ревью круга 2; спека: «значок
        // в трее жёлтый, подсказка „проверяю“»). Значок в это время жёлтый
        // (`Dot::Busy` из `panel::card`), а подсказка отдавала «подключён» —
        // та же ложь, что была в Critical 2, только в трее: зелёное слово под
        // жёлтым значком.
        if matches!(verdict, Verdict::Checking) {
            return "1c-connect: проверяю".to_string();
        }
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

/// Отменить можно обмен с git (`Push`/`Pull`) и работу платформы
/// (`Decompile`/`Load`).
///
/// Прежде здесь стояли только `Push`/`Pull`, а про платформу было написано
/// «не прерывается» — и это было правдой лишь в том смысле, что никто не
/// пробовал: `local::call` слова `cancel` не знал вовсе, у него был один
/// срок в 3600 с, по которому вызов перестают ждать, а Конфигуратор
/// продолжает работать, держа базу и диск. Самые долгие операции (выгрузка
/// УНФ — десятки минут и 90 тысяч файлов) отмены не имели, и человек,
/// нажавший «Выгрузить» не на той базе, мог только снять процесс руками.
/// Теперь `local::call` замечает флаг и, выходя, добивает весь куст через
/// Job Object (`job.rs`), то есть вместе с `1cv8.exe`. STATUS, техдолг 16.
///
/// `Install`/`Uninstall` сюда не входят: это `mcp-1c --install`, другой путь
/// и минуты, а не десятки минут. `Check` не отменяют — он короткий.
pub fn can_cancel(phase: Phase) -> bool {
    matches!(phase, Phase::Push | Phase::Pull | Phase::Decompile | Phase::Load)
}

/// Кнопка «Отменить» карточки хода — поток сам решает, когда заметить флаг
/// и договорить `Ev::Done { cancelled: true, .. }`.
pub fn cancel_op(cx: &mut App) {
    cx.global::<Ui>().cancel.store(true, Ordering::Relaxed);
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
    tray::tray_state_of(panel::card(ui.progress.as_ref(), &ui.link, &ui.cfg.host.mode, &ui.verdict, ui.gate.as_deref(), now).dot)
}

/// Синхронизировать трей с `Ui`: состояние (по точке карточки) и подсказка
/// на текущий момент. Общий конец всех мест, что меняют почву под значком —
/// `set_link`, `run_op`, `Ev::Done`, `Ev::Ready` (набор и вердикт),
/// `refresh_gate` (замок); каждый забытый вызов — это значок, застрявший на
/// прежнем состоянии, и ровно так жили находки S1 и S2 итогового ревью.
///
/// Показанное запоминается в `ui.tray_shown`: иначе «значок узнал» — побочное
/// действие, которого тесту не видно (настоящего `Tray` в тестах нет), и
/// проводку приходится сторожить сверкой чистых функций между собой. Три
/// таких зелёных теста при сломанном поведении эта работа уже пережила.
fn sync_tray(ui: &mut Ui, now: Instant) {
    // «Выключено значит выключено» (спека, не-цель): выключили обслуживание —
    // набора больше нет. Ронять его надо именно здесь, а не у каждого
    // выключения по-своему: `apply_facts` — единственное место, где набор
    // сбрасывался, а при выключенном обслуживании расчёт не идёт вовсе
    // (`start_full_pass` выходит рано), и прежний набор оставался живым.
    // Человек видел серую карточку «MCP выключены» рядом с красным бейджем
    // «Проблем: 2» и вкладку «Проект» со старыми красными точками — ровно тот
    // класс противоречий, против которого вся перестройка (Important 1 ревью
    // задач 7+8).
    //
    // Воронка выбрана проверенная: через `sync_tray` проходят ВСЕ переходы,
    // что меняют почву под значком, — и ручное выключение (`set_link`), и
    // погашенное замком (`douse_serving`). Своей ветки на каждый путь это не
    // требует, и забыть один из них нельзя.
    if !serving(ui) {
        ui.facts = Vec::new();
        ui.verdict = Verdict::Off;
        // Час без набора ни к чему: он — момент, когда набор стал полным.
        ui.facts_at = None;
    }
    let view = TrayView {
        state: tray_state_now(ui, now),
        on: ui.link.is_on(),
        locked: ui.gate.is_some(),
        tooltip: tray_tooltip(ui.progress.as_ref(), &ui.link, &ui.verdict, ui.gate.as_deref(), now),
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
    // «Установить» — та же настройка, что `1c-connect install`: выводимые
    // значения считаются здесь один раз и уезжают в toml, а дальше читаются
    // только оттуда (спека, «программа не выдумывает адресов»).
    if which == Which::Install {
        match ops_host::write_derived_fields(&ui.kit, &mut ui.cfg, false) {
            Ok(wrote) if !wrote.is_empty() => note_line(ui, Line { kind: Kind::Ok, text: format!("в {} записано: {}", cfg::FILE, wrote.join(", ")) }),
            Ok(_) => {}
            Err(e) => note_line(ui, Line { kind: Kind::Fail, text: format!("параметры не дописаны: {e:#}") }),
        }
    }
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
    // Корень комплекта — он же каталог инструментов: git берётся оттуда, где
    // лежит комплект, а не из профиля (леджер комплекта, пункт 15).
    let (cfg, tx, kit_root) = (ui.cfg.clone(), ui.tx.clone(), ui.kit.root.clone());
    ui.pending += 1;
    std::thread::spawn(move || {
        guarded(
            &tx,
            || {
                let p = Project::from_cfg(&cfg, &kit_root);
                let (ok, why) = match project::layout_state(&p) {
                    Layout::Ok => (true, String::new()),
                    Layout::Missing => (false, "каталог проекта ещё не разложен".into()),
                    Layout::BaseDir(f) => (false, format!("это каталог базы ({f})")),
                    Layout::NoGitea => (false, cfg::NO_GITEA.into()),
                    Layout::Drifted(d) => (false, d),
                };
                // Те же факты и под тем же замком, что отдаёт агенту
                // `project_status`: во время выгрузки вкладка говорит
                // «занято», а не читает полузаписанный индекс (правка 6B на
                // main). Отказ замка или git — в `why`, и тогда вкладка
                // показывает причину вместо состояния.
                let (facts, ok, why, busy) = match ok.then(|| project::facts_locked(&p)) {
                    Some(Ok(f)) => (Some(f), true, why, false),
                    Some(Err(e)) => (None, false, e, true),
                    None => (None, ok, why, false),
                };
                Ev::Project(ProjectView { dir: p.dir.display().to_string(), branch: p.branch, facts, ok, why, busy })
            },
            || Ev::Project(ProjectView { dir: String::new(), branch: String::new(), facts: None, ok: false, why: PANIC_TEXT.into(), busy: false }),
        );
    });
}

/// Полный набор от основания к вершине, громко: отчёт пишется в
/// `last-check.txt` тем же текстом, что печатает командная строка, и
/// журналу/вкладке «Журнал» достаётся итог (см. `Ev::Ready` в `handle`).
///
/// Защёлка (`facts_pending`) общая с тактом (задача 6, «Добавлено
/// управляющим»): если набор прямо сейчас уже считается — тактом или прежним
/// нажатием, — кнопка второй поход к базе не начинает (`request_check`
/// вернёт `None`). Она не отказывает молча и не встаёт в очередь отдельно от
/// такта: она взводит тот же повтор, что и такт (`facts_again`), и просит
/// громкий показ результату, который применится первым (`check_requested`).
/// «Проверяю…»/занятость кнопка показывает сразу (`ui.busy`), не дожидаясь,
/// чей это будет расчёт.
///
/// Выключено — считать нечего, и сторож тот же, что у такта (`start_full_pass`,
/// Important 3 финального ревью). Без него нажатие заводило полный набор при
/// выключенном обслуживании, `apply_facts` его потом ронял, и громкий показ
/// печатал ноль строк фактов и «Проверка пройдена» — на выключенном
/// коннекторе, при отчёте с провалами на диске. Молча не отказываем: кнопка
/// говорит, почему проверять нечего.
pub fn run_check(cx: &mut App) {
    if !serving(cx.global::<Ui>()) {
        let ui = cx.global_mut::<Ui>();
        ui.tab = Tab::Log;
        ui.unread = 0;
        note_line(ui, Line { kind: Kind::Warn, text: CHECK_OFF.into() });
        cx.refresh_windows();
        return;
    }
    let ui = cx.global_mut::<Ui>();
    ui.busy = Some(Busy::Check);
    note_line(ui, head_line("Проверка"));
    ui.tab = Tab::Log;
    ui.unread = 0;
    cx.refresh_windows();
    let ui = cx.global_mut::<Ui>();
    if let Some(gen) = request_check(ui, Instant::now()) {
        spawn_full_pass(ui, gen, true);
    }
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
    // Свой httpd — тоже наш, и выход обязан забрать его с собой: он живёт в
    // каталоге комплекта и переживал коннектор, не давая удалить папку
    // (находка владельца 26.10). Чужой веб-сервер (`publication =
    // "webserver"`) не трогаем — за него отвечает человек.
    //
    // Итог — строкой в файл журнала, а не в окно: окна к этому моменту уже
    // нет, читать её человек будет потом, разбираясь, куда делся сервер.
    let ui = cx.global::<Ui>();
    // Гасим ТОЛЬКО то, что подняли сами в этом запуске (`raised_publication`):
    // `httpd.pid` один на пользователя ОС, а коннекторов у него может быть
    // несколько — по одному на имя рабочего места. Чужой httpd (соседнего
    // рабочего места или режима `webserver`) не наш, и остановка его была бы
    // хуже утечки, которую мы чиним.
    let line = match ui.raised_publication.then(|| ops_host::stop_own_publication(&quiet_ctx(ui))) {
        Some(Ok(true)) => Some(Line { kind: Kind::Ok, text: "HTTP-сервис базы остановлен при выходе".into() }),
        Some(Ok(false)) | None => None,
        Some(Err(e)) => Some(Line { kind: Kind::Warn, text: format!("HTTP-сервис базы не остановлен при выходе: {e:#}") }),
    };
    if let Some(line) = &line {
        note_file(&ui.kit, line);
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

/// Перерисовать на тике: есть что показать (новые данные или операция на
/// ходу) — всегда; открытое окно в покое — не чаще `REDRAW_IDLE`.
///
/// Шаг такта при открытом окне стал секундой (Important 4), а перерисовка от
/// этого учащаться не должна: в покое её ждёт один счётчик минут в карточке
/// связи (он считает их сам от `since.elapsed()` на каждой отрисовке), и
/// минуте довольно прежнего тридцатисекундного шага. Всё остальное приезжает
/// своим событием, которое и зовёт `refresh_windows()` само.
pub const REDRAW_IDLE: Duration = TICK_IDLE;

fn tick_redraw(window_open: bool, dirty: bool, running: bool, since_redraw: Duration) -> bool {
    dirty || running || (window_open && since_redraw >= REDRAW_IDLE)
}

/// Медленный такт: как часто идут весь дешёвый набор (`verify::cheap`) и —
/// если он не изменился — дорогие факты. Открытое окно спрашивает чаще,
/// скрытое реже, но идёт всё равно: иначе набор «протухнет» тихо, пока никто
/// не смотрит. Секундный такт при открытом окне спрашивает ОДНО (число
/// открытых сеансов 1С) — остальные дешёвые факты за секунду не меняются.
pub const SLOW_OPEN: Duration = Duration::from_secs(30);

/// Сколько итог операции держит верхний слот подвала, прежде чем уступить
/// его состоянию. Полминуты — потому что столько же живёт набор фактов
/// (`SLOW_OPEN`): к следующему пересчёту итог уже прочитан, а если человек
/// отошёл — в журнале он остался. Меньше нельзя: пересчёт идёт сразу за
/// операцией («почва могла сдвинуться»), и отказ гас бы через секунду после
/// показа.
pub const STATUS_LINGER: Duration = Duration::from_secs(30);

pub const SLOW_HIDDEN: Duration = Duration::from_secs(120);

/// Единый тик приложения (`launch` зовёт его раз в `TICK_BUSY`/`TICK_IDLE`):
/// перерисовывает окно (см. `tick_redraw`), один раз на операцию отмечает
/// застой строкой в журнал (`stale_line`), не чаще `TOOLTIP_EVERY` двигает
/// подсказку трея на фазу текущей операции, и — задача 6 — считает дешёвые
/// факты и решает, не пора ли за дорогими (`tick_facts`).
fn tick(cx: &mut App) {
    let now = Instant::now();
    let ui = cx.global_mut::<Ui>();
    let since_redraw = ui.redraw_at.map(|t| now.saturating_duration_since(t)).unwrap_or(Duration::MAX);
    let redraw = tick_redraw(ui.window.is_some(), ui.dirty, ui.progress.is_some(), since_redraw);
    ui.dirty = false;
    if redraw {
        ui.redraw_at = Some(now);
        cx.refresh_windows();
    }
    let ui = cx.global_mut::<Ui>();
    if ui.progress.is_some() {
        if let Some(line) = ui.progress.as_mut().and_then(|r| stale_line(r, now)) {
            note_line(ui, line);
        }
        let due = ui.tooltip_at.map(|at| now.saturating_duration_since(at) >= TOOLTIP_EVERY).unwrap_or(true);
        if due {
            let text = tray_tooltip(ui.progress.as_ref(), &ui.link, &ui.verdict, ui.gate.as_deref(), now);
            ui.tooltip_at = Some(now);
            if let Some(tray) = &ui.tray {
                tray.set_tooltip(&text);
            }
        }
    }
    note_licence_change(cx);
    let ui = cx.global_mut::<Ui>();
    let window_open = ui.window.is_some();
    tick_facts(ui, now, window_open);
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
            // Отказ платформы — словами: что это было и что делать. Строкой
            // НИЖЕ сырого текста, как подсказка провалившегося факта
            // (`check_lines`): сам текст 1С человек видит по-прежнему, а
            // понимает с первой строки. Главное, зачем заведено, — отличить
            // «упёрлись в ограничение по числу сеансов» от поломки
            // (`onec_error`, решение заказчика 25.10.2026).
            let cause = matches!(line.kind, Kind::Fail).then(|| crate::onec_error::explain(&line.text)).flatten();
            if let Some(c) = cause {
                note_line(ui, Line { kind: Kind::Warn, text: crate::onec_error::hint_line(c) });
            }
            // В подвал — короткая причина, а не сырой текст: в двух строках
            // подвала отказ платформы всё равно обрезается многоточием, и
            // человек читает его начало вместо смысла.
            let status = match cause {
                Some(c) => Line { kind: Kind::Fail, text: c.what.to_string() },
                None => line.clone(),
            };
            set_status(ui, status, None);
            // Подсказка без операции: `on_done` уже опустил `progress`/`busy`.
            // `tooltip_at` тоже назад в `None` — следующая операция не должна
            // наследовать свежий штамп и ждать до TOOLTIP_EVERY своей подсказки.
            let now = Instant::now();
            ui.tooltip_at = None;
            sync_tray(ui, now);
            let hidden = ui.window.is_none();
            cx.refresh_windows();
            if hidden {
                // Во всплывающем окне ОС места ещё меньше, чем в подвале:
                // туда тем более короткая причина, а не текст платформы.
                let body = match cause {
                    Some(c) => c.what.to_string(),
                    None => line.text.clone(),
                };
                cx.show_system_notification(SystemNotification { tag: "op".into(), title: "1c-connect".into(), body: body.into(), actions: vec![] });
            }
            refresh_project(cx);
            // Почва могла сдвинуться («Установить» в т.ч.) — вердикт
            // «дойдёт ли агент до базы» устарел, пересчитываем.
            run_readiness(cx.global_mut::<Ui>());
        }
        Ev::Project(v) => {
            let ui = cx.global_mut::<Ui>();
            ui.project = Some(v);
            ui.pending = ui.pending.saturating_sub(1);
            cx.refresh_windows();
        }
        Ev::Publish(r) => match r {
            Ok(()) => {
                cx.global_mut::<Ui>().raised_publication = true;
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
        Ev::Ready(gen, facts, loud) => {
            let again = apply_facts(cx.global_mut::<Ui>(), gen, facts);
            // Громкий показ — по тому, был ли громким сам расчёт, а не по
            // `again` (Critical 1): набор применён в любом случае, и отчёт,
            // который кнопка обещала, уже лежит на диске. Прежде повтор
            // отменял и показ, и кнопка «Проверить» не договаривала.
            if loud {
                // Громкий показ — то, что раньше делал только `Ev::Checks`:
                // отчёт на диске уже лежит (его написал `spawn_full_pass`),
                // здесь — журнал, подвал и перечитывание проекта.
                let ui = cx.global_mut::<Ui>();
                // Итог спрашиваем у `loud_status`: набор мог быть уронен
                // выключением, пока расчёт шёл, и тогда «Проверка пройдена»
                // была бы прямой ложью (Important 3 финального ревью).
                let status = loud_status(&ui.facts);
                if ui.facts.is_empty() {
                    // Показывать нечего, и отчёт называть незачем. Кнопку всё
                    // равно отпускаем — иначе «Проверяю…» навсегда.
                    note_line(ui, status.clone());
                    set_status(ui, status, None);
                } else {
                    let lines = check_lines(&ui.facts);
                    for line in lines {
                        note_line(ui, line);
                    }
                    // Пункта меню «Открыть отчёт» больше нет (заказчик
                    // 2026-10-20: два пункта делали одно и то же) — файл
                    // называет строка журнала и подсказка подвала, но не сам
                    // итог рядом с кнопкой: туда путь не влезал (заказчик
                    // 2026-10-21).
                    let report = ui.kit.state_path("last-check.txt").display().to_string();
                    note_line(ui, status.clone());
                    note_line(ui, report_line(&report));
                    set_status(ui, status, Some(report_line(&report).text));
                }
                ui.busy = None;
                ui.check_requested = false;
                // Трей (S1): проверка — другое место, где `busy` возвращается
                // в `None`, и значок/подсказку надо подвинуть, как у
                // `Ev::Done` — иначе жёлтый («занят») может застрять до
                // следующего перехода.
                sync_tray(ui, Instant::now());
                // Каталог проекта — тоже как у `Ev::Done`: он не факт набора,
                // а подробность рядом с ним (`ProjectView`), и своего повода
                // перечитаться у него нет. Состояние операций больше не
                // перечитывается вовсе: вкладка «Проект» показывает тот же
                // набор, что и проверка, и расходиться им теперь нечем.
                refresh_project(cx);
            }
            cx.refresh_windows();
            if again {
                // Через `start_full_pass`, а не своей парой
                // `start_facts`+`spawn_full_pass` (Important 3 финального
                // ревью): у неё сторож `serving`, и без него обещанный повтор
                // считал ПОСЛЕ выключения — целый набор с походом в базу, на
                // диск, до минуты, да ещё и с отчётом, если громкий. Своя
                // пара отличалась от неё только тем, что брала громкость
                // отдельной строкой, а она берёт ту же `check_requested`
                // (громкость переживает отброшенный ответ: кнопку, нажатую
                // пока считал такт, обслужит тот расчёт, который наконец
                // применится, а не обязательно этот самый).
                start_full_pass(cx.global_mut::<Ui>());
            }
        }
        Ev::Cheap { sessions, facts } => {
            let ui = cx.global_mut::<Ui>();
            let window_open = ui.window.is_some();
            on_cheap(ui, sessions, facts, Instant::now(), window_open);
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
    // Окно открывается и на незаполненном файле: в нём человек поля и
    // заполняет. Чего не хватает — скажет набор фактов своими словами, а не
    // отказ при запуске (решение владельца 26.10).
    let cfg = Cfg::load_unchecked(&kit)?;
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
            facts: Vec::new(),
            verdict: Verdict::Checking,
            facts_at: None,
            facts_gen: 0,
            facts_pending: false,
            facts_since: None,
            facts_again: false,
            check_requested: false,
            prev_cheap: Vec::new(),
            sessions: None,
            cheap_pending: false,
            cheap_at: None,
            redraw_at: None,
            facts_full_at: None,
            licence: crate::licence::State::Valid,
            log: Vec::new(),
            unread: 0,
            tab: Tab::default(),
            project: None,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            dirty: false,
            busy: None,
            pending: 0,
            status: None,
            status_at: None,
            raised_publication: false,
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
        // Открытое окно — тоже секундный ритм (Important 4, п. 4): спека
        // обещает «секунда при открытом окне», а шаг в 30 с при `SLOW_OPEN`
        // = 30 с давал полный расчёт раз в минуту. Перерисовка от этого не
        // учащается — её держит `tick_redraw`/`REDRAW_IDLE`.
        cx.spawn(async move |cx| {
            let mut idle = Duration::ZERO;
            loop {
                cx.background_executor().timer(TICK_BUSY).await;
                let busy = cx.update(|cx| {
                    let ui = cx.global::<Ui>();
                    ui.progress.is_some() || ui.busy.is_some() || ui.window.is_some()
                });
                idle = if busy { Duration::ZERO } else { idle + TICK_BUSY };
                if busy || idle >= TICK_IDLE {
                    idle = Duration::ZERO;
                    cx.update(tick);
                }
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(ev) = rx.next().await {
                cx.update(|cx| handle(ev, cx));
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
        // Про срок — до подъёма сеанса: первое, что человек читает в журнале.
        note_licence_change(cx);
        // Сеанс поднимается только при включённом обслуживании — и в
        // локальном режиме тоже. Прежде локальный поднимался безусловно
        // («слушатели держат порты весь срок процесса»), и выключенный
        // коннектор занимал порты рабочего места, не обслуживая никого:
        // забрать их на другой машине было нельзя, пока он не закрыт
        // (находка владельца 26.10). Выключено значит выключено.
        // Замок при запуске — строкой в журнал: человек видит серую точку и
        // запертый переключатель, и обязан прочитать, почему.
        if let Some(why) = &gate {
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Warn, text: format!("MCP не включены при запуске: {why}") });
        }
        if serve_on_start {
            match start_session(cx.global_mut::<Ui>()) {
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
        // Заполненный набор, а не умолчания: с 26.10 умолчания пусты («не
        // задано» программа не выдумывает), а окну в тестах нужны рабочие
        // параметры — иначе `save_host` отказывает на незаполненном поле.
        let (kit, cfg) = (Kit::at(dir), crate::cfg::filled_for_test());
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
            facts: Vec::new(),
            verdict: Verdict::Checking,
            facts_at: None,
            facts_gen: 0,
            facts_pending: false,
            facts_since: None,
            facts_again: false,
            check_requested: false,
            prev_cheap: Vec::new(),
            sessions: None,
            cheap_pending: false,
            cheap_at: None,
            redraw_at: None,
            facts_full_at: None,
            licence: crate::licence::State::Valid,
            log: Vec::new(),
            unread: 0,
            tab: Tab::default(),
            project: None,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            dirty: false,
            busy: None,
            pending: 0,
            status: None,
            status_at: None,
            raised_publication: false,
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

/// Сеанс, которому заведомо некуда дозвониться (порт только что закрыт) —
/// тот же приём, что в `session.rs`: тест не ждёт настоящей связи, ему
/// важно лишь то, что `ui.session` не пусто.
#[cfg(test)]
fn doomed_session(ui: &Ui) -> crate::session::Session {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut cfg = ui.cfg.clone();
    cfg.host.server = format!("127.0.0.1:{port}");
    session::start_with_enabled(cfg, ui.kit.clone(), Arc::new(|_| {}), ui.enabled.clone(), |_| {})
}

/// Сеанс, который УЖЕ кончился сам: туннель в никуда (первая попытка
/// терминальная), и ждём, пока поток уйдёт. Ровно то, что остаётся в
/// `Ui` после отказа bind: `session` не пусто, а работать нечему.
/// Режим — туннельный независимо от `ui.cfg`: локальный сеанс занял бы
/// настоящие порты и остался бы жив, а тесту нужен мёртвый.
#[cfg(test)]
fn ended_session(ui: &Ui) -> crate::session::Session {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut cfg = ui.cfg.clone();
    cfg.host.mode = "tunnel".into();
    cfg.host.server = format!("127.0.0.1:{port}");
    let mut s = session::start_with_enabled(cfg, ui.kit.clone(), Arc::new(|_| {}), ui.enabled.clone(), |_| {});
    for _ in 0..200 {
        if s.finished() {
            return s;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("сеанс, которому некуда звонить, обязан кончиться сам");
}

/// Живой сеанс: на том конце слушает немой сокет — рукопожатие не
/// кончается ни успехом, ни отказом, пока слушатель жив. Слушатель
/// возвращается наружу: уронить его значит отпустить и сеанс.
#[cfg(test)]
fn live_session(ui: &Ui) -> (crate::session::Session, std::net::TcpListener) {
    let quiet = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = ui.cfg.clone();
    cfg.host.mode = "tunnel".into();
    cfg.host.server = format!("127.0.0.1:{}", quiet.local_addr().unwrap().port());
    (session::start_with_enabled(cfg, ui.kit.clone(), Arc::new(|_| {}), ui.enabled.clone(), |_| {}), quiet)
}

#[cfg(test)]
fn with_params_file(ui: &Ui) {
    write_text(&ui.kit.state_path(cfg::FILE), &ui.cfg.render()).unwrap();
}

#[cfg(test)]
mod tests;