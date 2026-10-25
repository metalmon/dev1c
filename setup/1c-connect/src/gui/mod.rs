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
    /// Поколение расчёта, который прямо сейчас держит защёлку
    /// `facts_pending`. Не то же, что `facts_gen`: тот двигает и
    /// «Проверить», ничего не запуская, — а защёлку обязан снимать
    /// только свой поток (находка N1 повторного ревью).
    pub facts_run: u64,
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
    /// Разобранное состояние git; `None` — раскладки нет, замок занят или
    /// git ответил ошибкой: причина тогда в `why`.
    pub facts: Option<project::Facts>,
    pub ok: bool,
    pub why: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dot {
    Off,
    Busy,
    Up,
    Down,
    /// MCP включены, но молчаливый расчёт готовности (`verify::gather` в
    /// своём потоке) нашёл провалившийся факт — карточка жёлтая, а не
    /// зелёная или красная: обслуживание идёт, просто не всё готово.
    /// Провалиться может и то, что базы не касается (проект не разложен,
    /// среда не отвечает), поэтому заголовок говорит общее «Не всё готово»,
    /// а что именно — говорит причина рядом (`panel::reason`).
    Warn,
}

/// Вердикт «дойдёт ли агент до базы» — выведен из ОДНОГО набора фактов
/// (`verify::gather`/`verify::cheap`), а не хранится рядом с ним: два
/// источника правды об одном итоге — ровно та болезнь, против которой писана
/// вся перестройка (задача 5). Пересчитывается по событиям, что могли
/// сдвинуть почву (включение MCP, «Сохранить», конец операции), и по такту
/// (задача 6, `needs_full_pass`) — не только по ним одним, как было раньше.
/// `PartialEq` — ради одного вопроса: сменился ли вердикт (`verdict_line`).
/// Сравнивается он целиком, вместе с фактом-причиной: сменившаяся причина —
/// такая же новость, как сменившееся состояние.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Обслуживание выключено — набор не считается вовсе (спека: «выключено
    /// — ничего не считаем»), и показывать нечего.
    Off,
    /// Обслуживание включено, но набора фактов ещё не было ни разу. Зелёным
    /// это не бывает: карточка показывает «Проверяю…» с точкой «занято»
    /// (спека: «Зелёной точки до вердикта готовности не бывает»).
    Checking,
    /// Все факты набора — факт сам по себе и есть доказательство.
    Ready,
    /// Первый не прошедший факт — не поломка, а «занято сейчас» (открытый
    /// сеанс 1С, наша же долгая операция): точка карточки от неё не красится
    /// (`panel::card` трактует `Busy` как `Ready`), в отличие от `NotReady`.
    /// Приоритет над `NotReady` обратный тому, что в порядке набора: поломка
    /// важнее занятости, и её человек должен увидеть даже при занятой базе.
    Busy(Check),
    /// Первый провалившийся факт — его `what`/`detail`/`hint` и есть причина,
    /// которую видит человек в карточке.
    NotReady(Check),
}

/// Вердикт из набора — чистая функция, ни счёта, ни сети: `serving` решает
/// всё («выключено — ничего не считаем»), иначе пустой набор значит «ещё не
/// посчитано» (`Checking`), а непустой разбирается по исходам фактов
/// (`Check::outcome`). Поломка (`Outcome::Bad`) ищется первой и перекрывает
/// занятость (`Outcome::Busy`) независимо от того, какой из двух факторов
/// в наборе встретился раньше: человек должен увидеть сломанное и тогда,
/// когда рядом ещё и занято. `note`/`pass` (оба — `Outcome::Good`) никогда
/// не становятся ни тем, ни другим.
pub fn verdict_of(serving: bool, facts: &[Check]) -> Verdict {
    if !serving {
        return Verdict::Off;
    }
    if facts.is_empty() {
        return Verdict::Checking;
    }
    if let Some(c) = facts.iter().find(|c| c.outcome() == check::Outcome::Bad) {
        return Verdict::NotReady(c.clone());
    }
    if let Some(c) = facts.iter().find(|c| c.outcome() == check::Outcome::Busy) {
        return Verdict::Busy(c.clone());
    }
    Verdict::Ready
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
///
/// Жёлтый вердикт тоже называет причину (Critical 2 ревью задач 5+6): значок
/// желтел на «занято» и оранжевел на «не всё готово», а подсказка при этом
/// говорила «подключён» — человек видел цвет без объяснения. Порядок и условие
/// те же, что у карточки (`panel::card`/`panel::reason`): вердикт показывается
/// только в локальном режиме при поднятом обслуживании, иначе подсказка и
/// карточка разошлись бы.
pub fn tray_tooltip(progress: Option<&Running>, link: &State, mode: &str, verdict: &Verdict, gate: Option<&str>, now: Instant) -> String {
    if let (None, Some(why)) = (progress, gate) {
        // Та же формулировка, что у подсказки переключателя и подвала:
        // три поверхности говорили о замке тремя разными фразами.
        return format!("1c-connect: {}", crate::gui::panel::gate_text(why).to_lowercase());
    }
    if progress.is_none() && mode == "local" && link.is_up() {
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

/// Главное действие вкладки «Проект»: пока расширение не стоит — «Установить»,
/// дальше человеку нужен обмен с git, и главной становится «Выгрузить».
/// Остальное — в меню «…».
///
/// Выбирается по ФАКТУ набора (`verify::EXTENSION`), а не по состоянию
/// операций (`run::status`, прежний `ui.ops`) — решение 2 спеки: ни одна
/// поверхность не знает больше набора.
///
/// `None` — факта расширения в наборе нет, то есть набора ещё нет вовсе
/// («Проверяю», первый расчёт после входа). Главной кнопке тогда сказать
/// нечего: «Установить» на пустом наборе — утверждение о том, чего мы не
/// знаем (Minor 4 ревью задач 7+8), а вкладка и так показывает «Проверяю…».
/// В наборе этот факт есть в обоих режимах (`verify::order`), так что его
/// отсутствие и значит ровно «ещё не посчитали».
///
/// Занятость расширению не приговор: база нам в эту минуту отказала, стоит
/// расширение или нет — неизвестно, и советовать «Установить» человеку,
/// который просто работает в базе, нельзя (STATUS 26). Поэтому «Установить»
/// только у настоящего провала (`Outcome::Bad`).
pub fn project_primary(facts: &[Check]) -> Option<Which> {
    let c = facts.iter().find(|c| c.what == verify::EXTENSION)?;
    Some(if c.outcome() == check::Outcome::Bad { Which::Install } else { Which::Export })
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
            // `!ok` — всегда провал: «не проверено» (`!ok && info`) больше
            // не существует ни у одного факта (ушло вместе с `Limits`/
            // `not_checked` в задаче 2), отдельной ветки под него не держим.
            (false, _) => Kind::Fail,
        };
        out.push(Line { kind, text: format!("{}: {}", c.what, c.detail) });
        // Совет — у ЛЮБОГО факта, которому есть что добавить, а не только у
        // провалившегося (Minor 2 ревью задач 7+8): у занятого факта
        // («закройте «Предприятие»») он терялся, и человек читал в журнале
        // проблему без выхода из неё. И не дважды: `hint_adds` — то же
        // правило, по которому склеивает `fact_detail`.
        if hint_adds(c) {
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
/// факт из `verify::gather`, одной строкой — что не так и что сделать, без
/// «факт N провален» (заказчик 2026-10-20 читает это как 1С-разработчик).
/// Подсказка не дублируется, когда она уже вписана в само `detail` (так у
/// TOOLS-факта «в комплекте нет … — скопируйте заново»), иначе человек видел
/// бы один совет дважды.
pub fn readiness_reason(check: &Check) -> String {
    format!("{}: {}", check.what, fact_detail(check))
}

/// Подробность факта без его имени: что не так и что сделать, одной строкой.
/// Подсказка не дублируется, когда она уже вписана в само `detail`.
///
/// Одно место на причину вердикта (`readiness_reason` — подвал, подсказка
/// значка, журнал) и на строку вкладки «Проект» (`panel::fact_rows`): два
/// похожих склеивания `detail` с `hint` расходились бы молча, и человек
/// читал бы один и тот же факт двумя разными способами.
pub fn fact_detail(check: &Check) -> String {
    if hint_adds(check) {
        format!("{} — {}", check.detail, check.hint)
    } else {
        check.detail.clone()
    }
}

/// Говорит ли подсказка факта что-то, чего нет в самом `detail`. Одно правило
/// на оба показа подсказки — строкой вкладки (`fact_detail`, через тире) и
/// отдельной строкой журнала (`check_lines`, со своим значком): рендеры
/// разные, а решение «есть ли что добавить» одно. Так у TOOLS-факта («в
/// комплекте нет … — скопируйте заново») совет не показывается дважды.
fn hint_adds(check: &Check) -> bool {
    !check.hint.is_empty() && !check.detail.contains(check.hint.as_str())
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

/// Каталог выгрузки для записи — тот же приём, что у адреса публикации.
/// `install` дописывает выведенный `<project>\src` в toml, и записанное
/// умолчание обязано и дальше идти за каталогом проекта: иначе смена проекта
/// оставила бы выгрузку в старом месте. Свой каталог (отличается от
/// выведенного) смену `project` переживает. Пустое поле остаётся пустым: там
/// умолчание и так считается при каждом чтении (`Cfg::dump_dir`).
pub fn carried_dump(saved: &Host, new_project: &str) -> String {
    if saved.dump == cfg::derived_dump(&saved.project) {
        cfg::derived_dump(new_project)
    } else {
        saved.dump.clone()
    }
}

/// Порт своей публикации для записи — тот же приём, что у каталога выгрузки:
/// `install` дописывает выведенный `8080 + N` в toml, и записанное умолчание
/// обязано и дальше идти за рабочим местом. Свой порт (отличается от
/// выведенного) смену рабочего места переживает; незаданный остаётся
/// незаданным.
///
/// Переезд не молчаливый: новый порт может оказаться занят — например, вторым
/// коннектором другого рабочего места на этой же машине, — поэтому о нём (как
/// и обо всём прочем, что едет за рабочим местом) `save_host` пишет строку в
/// журнал (`moved_lines`); поднять HTTP-сервис на занятом порту всё равно не
/// выйдет, и операция публикации скажет, кто его держит.
pub fn carried_http_port(saved: &Host, new_workstation: &str) -> Option<u16> {
    if saved.http_port == Some(cfg::derived_http_port(&saved.workstation)) {
        Some(cfg::derived_http_port(new_workstation))
    } else {
        saved.http_port
    }
}

/// Что переехало за переименованием — одной строкой на каждую причину.
/// Правило одно на все переезды: о каждом говорим, ни об одном не молчим
/// (находка ревью задачи 35 — прежде журнал называл только порт публикации,
/// хотя за рабочим местом едут ещё ветка проекта, `origin` и порты, а за
/// каталогом проекта — каталог выгрузки). `Warn`, а не `Ok`: новые порты
/// может держать другой коннектор, а прежний каталог выгрузки остаётся
/// лежать со старым содержимым.
pub fn moved_lines(old: &Host, new: &Host) -> Vec<String> {
    let mut v = Vec::new();
    if old.workstation != new.workstation {
        let p = Cfg { host: new.clone() }.ports();
        let port = match (old.http_port, new.http_port) {
            (Some(from), Some(to)) if from != to => format!(", порт публикации {from} → {to}"),
            _ => String::new(),
        };
        v.push(format!(
            "за рабочим местом {} → {} переехали: ветка проекта и origin, порты {}/{}/{}{port}",
            old.workstation, new.workstation, p.mcp, p.vrunner, p.web
        ));
    }
    if old.project != new.project && old.dump != new.dump {
        v.push(format!("за каталогом проекта переехал каталог выгрузки: {} (прежний остался со старым содержимым)", new.dump));
    }
    v
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
/// значения из формы затёрли бы toml. Каталог проекта — из формы, и за ним
/// едут умолчания: адрес публикации (`carried_base_url`), каталог выгрузки
/// (`carried_dump`) и — за рабочим местом — порт публикации
/// (`carried_http_port`). О каждом переезде — строка в журнал
/// (`moved_lines`).
pub fn save_host(ui: &mut Ui, mut host: Host) -> Result<(), String> {
    let old_host = ui.cfg.host.clone();
    host.gitea = ui.cfg.host.gitea.clone();
    host.dump = carried_dump(&ui.cfg.host, &host.project);
    host.platform = ui.cfg.host.platform.clone();
    host.tools = ui.cfg.host.tools.clone();
    host.mode = ui.cfg.host.mode.clone();
    host.publication = ui.cfg.host.publication.clone();
    host.http_port = carried_http_port(&ui.cfg.host, &host.workstation);
    host.base_url = carried_base_url(&ui.cfg.host, &host.base_path);
    // Переезды — не молчком, и все до одного: см. `moved_lines`.
    let moved = moved_lines(&old_host, &host);
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
    for text in moved {
        note_line(ui, Line { kind: Kind::Warn, text });
    }
    // Публикация не должна молчать про старую базу: иначе агент отвечает из
    // прежней, пока кто-нибудь не перезапустит HTTP-сервис руками (находка
    // целостного ревью) — до следующего вызова она обязана смотреть на
    // новую. `ensure_publication` сама решает «auto»/«webserver» и когда уже
    // «сделано» — здесь только зовём её, не повторяя эту логику.
    if republish_after_save(descriptor_changed, ui.session.is_some()) {
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
pub(crate) fn save_question(ui: &Ui, new_host: &Host) -> Option<(String, String)> {
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
        // Молчать нельзя: человек нажал кнопку и обязан узнать, почему ничего
        // не произошло. Молчаливый выход отсюда и был вторым тупиком той же
        // кнопки.
        set_status(cx.global_mut::<Ui>(), Line { kind: Kind::Warn, text: "Сначала ответьте на уже открытый вопрос".into() }, None);
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
/// Поднимать ли HTTP-сервис базы после записи параметров. Условий два, и
/// второе появилось по замечанию заказчика 2026-10-21: «сохранил — в журнале
/// сообщение об успешном поднятии http сервера, хотя я даже не пытался
/// включить MCP».
///
/// Переменилась база — старый дескриптор указывает не туда, и сервис обязан
/// посмотреть на новую, иначе агент отвечал бы из прежней базы. Но нужно это
/// только тому, кто сейчас обслуживает: при выключенном переключателе мост
/// отказывает ещё до публикации, и поднимать её — работа, которой никто не
/// просил, да ещё и объявленная в журнале как успех. Включат MCP —
/// `ensure_publication` сходит сама.
pub fn republish_after_save(database_changed: bool, serving: bool) -> bool {
    database_changed && serving
}

/// Подъём публикации в своём потоке. Голос к базе он берёт не здесь, а внутри
/// `ops_host::extension_reach` — в единственном месте, которое правда говорит
/// с базой (Important 2 ревью круга 2): мьютекс вокруг потока можно отменить,
/// убрав одну строку, а дорогу к базе мимо `extension_reach` не проложить.
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
    let tools = ops_host::tools_state(&kit.root);
    // Существование спрашиваем только у файловой базы (`gate::wants_directory`,
    // правило самого факта «база»): `exists()` по недоступной сетевой шаре
    // висит на таймауте SMB, а считается это на главном потоке окна.
    let base = &cfg.host.base_path;
    let base_exists = gate::wants_directory(base) && std::path::Path::new(base).exists();
    let platform = ops_host::platform_choice(cfg);
    gate::cold_gate(
        cfg,
        kit.state_path(cfg::FILE).exists(),
        base_exists,
        &tools,
        // Выбор платформы и причина отказа — одной лестницей (`platform_choice`):
        // окно запирает переключатель тем же текстом, что показывает проверка.
        platform.as_ref().map(|c| c.exe.as_path()).map_err(String::as_str),
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

/// Расчёт набора (`verify::gather` — тот же набор фактов и тот же срок
/// `verify::SET_DEADLINE`, что у «Проверить») в своём потоке: он ходит по
/// сети и поднимает пробу инструментов, и не должен держать главный поток
/// окна — тем же приёмом, что `ensure_publication_now` выше. Зовётся, когда
/// почва могла сдвинуться: карточка стала «MCP включены» (включили
/// переключатель, либо сеанс пришёл уже включённым — автозапуск), кончилась
/// любая операция (дешевле перепроверить лишний раз, чем отличать операции
/// друг от друга), «Сохранить», показ окна, и теперь ещё и такт (задача 6,
/// `tick_facts`/`needs_full_pass`) — не только по дискретным событиям, как
/// было раньше.
pub fn run_readiness(ui: &mut Ui) {
    // Замок — по тем же событиям, что и набор (почва для них одна), но без
    // потока: он считается по диску и отвечает сразу, а переключатель обязан
    // слушаться его уже на ближайшей отрисовке. На такт этот вызов НЕ
    // переезжает — см. `start_full_pass`.
    refresh_gate(ui);
    start_full_pass(ui);
}

/// Тот же расчёт, но без холодного замка — путь такта (Critical 3 ревью
/// задач 5+6; спека, «Решения, принятые мной», п. 5: «Холодный замок на
/// секундный такт не переезжает»).
///
/// У замка есть побочное действие: защёлкнувшись, он гасит обслуживание и
/// рвёт сеанс (`refresh_gate` → `douse_serving`). На такте он читает диск —
/// `tools_state`, скан платформ, `exists()` по каталогу базы, — и уснувшая
/// флешка с комплектом или отвалившаяся сетевая шара выключали коннектор под
/// человеком, который ничего не делал. Замок остаётся на своих прежних
/// событиях (включение, «Сохранить», конец операции, показ окна).
///
/// Выключено — считать нечего (спека: «при выключенном обслуживании не
/// считается НИЧЕГО»). Карточка его и не покажет, а набор стоит до минуты,
/// пробы инструментов и поднятого на Linux виртуального экрана — всё ради
/// ответа, который некому показать (находка S9). Включат — посчитается само:
/// включение и есть одно из событий пересчёта.
///
/// Громкость берётся у `check_requested`, а не задаётся тихой наглухо
/// (Important 4 ревью круга 2). Иначе потерянный громкий поток
/// (`readiness_lost`) заменялся ТИХИМ, его ответ гасил «Проверяю…», а отметку
/// кнопки не снимал — и следующий чужой повтор, по какому угодно поводу,
/// становился громким: отчёт `last-check.txt` и десяток строк в журнал без
/// того, чтобы человек нажимал кнопку. Громкость принадлежит нажатию, и
/// спрашивать её надо там, где расчёт заводится, — здесь.
///
/// `true` — расчёт правда начался (а не был только помечен повтором): такт
/// по этому решает, можно ли запомнить прочитанный дешёвый набор.
fn start_full_pass(ui: &mut Ui) -> bool {
    if !serving(ui) {
        return false;
    }
    let Some(gen) = start_facts(ui, Instant::now()) else { return false };
    spawn_full_pass(ui, gen, ui.check_requested);
    true
}

/// Поток полного расчёта: один набор, один срок (`verify::SET_DEADLINE`) —
/// тот же, что у «Проверить». Прежние быстрый и терпеливый заходы с разными
/// сроками разошлись друг с другом (заказчик 25.10: быстрый говорил «не
/// готово», «Проверить» тут же — 18 из 18); второго, отдельно придуманного
/// захода здесь больше нет.
///
/// `loud` — громкий показ: журнал, отчёт на диске, подвал — то, что раньше
/// делал только `run_check`. И такт, и кнопка «Проверить» заводят этот же
/// поток через ту же защёлку (`facts_pending`, задача 6, «один расчёт в
/// полёте, не два»): тихий путь не пишет отчёт и не трогает журнал
/// (`verify::gather` напрямую), громкий — тот же набор через `verify::run`
/// (она же печатает фазу `Phase::Check`) и дополнительно пишет
/// `last-check.txt`, как раньше. Паника внутри сбора фактов (подпроцессы,
/// reqwest, чтение путей) обязана договорить ответом, иначе `facts_pending`
/// остался бы взведённым навсегда, — на панике отдаём не пустой набор
/// («всё хорошо»), а видимую причину.
fn spawn_full_pass(ui: &Ui, gen: u64, loud: bool) {
    // Тихий путь — `quiet_ctx`, а не `ctx_for`: ни строки в журнале, ни фазы
    // в карточке хода. Громкий зовёт `verify::run`, которая печатает фазу и
    // сама же зовёт `gather` — отдельного набора шагов для кнопки не нужно.
    let ctx = if loud { ctx_for(ui) } else { quiet_ctx(ui) };
    let mode = ui.cfg.host.mode.clone();
    let link = ui.link.clone();
    let tx = ui.tx.clone();
    std::thread::spawn(move || {
        guarded(
            &tx,
            move || {
                let state = link.is_up().then(|| link.label(&mode));
                let facts = if loud {
                    verify::run(&ctx, state.as_deref(), verify::PortUse::WeListen)
                } else {
                    verify::gather(&ctx, state.as_deref(), verify::PortUse::WeListen, Instant::now() + verify::SET_DEADLINE)
                };
                if loud {
                    let text = check::render("1c-connect", env!("CARGO_PKG_VERSION"), &verify::cfg_for_report(&ctx.cfg), &facts);
                    let _ = write_text(&ctx.kit.state_path("last-check.txt"), &text);
                }
                Ev::Ready(gen, facts, loud)
            },
            move || Ev::Ready(gen, vec![Check::fail("готовность", PANIC_TEXT, "нажмите «Проверить»")], loud),
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
/// `None` у начала — защёлку взвели где-то мимо `start_facts`; считаем
/// потерянным, иначе она осталась бы навсегда.
fn readiness_lost(since: Option<Instant>, now: Instant) -> bool {
    match since {
        Some(t) => now.saturating_duration_since(t) > READINESS_LOST,
        None => true,
    }
}

/// Поколение и отметка повтора. `None` — считать не надо: поток уже считает,
/// пересчёт помечен и пойдёт один раз по его возвращении.
///
/// Старый набор на экране не трогаем (задача 5, «Пошёл новый расчёт: набор
/// ещё прежний, и «Проверяю» не возвращается»): прежняя версия сбрасывала
/// вердикт в `Unknown` синхронно здесь же, и такт на секундном ритме
/// превратил бы это в мигание «Проверяю…» на каждый проход. Набор теперь
/// переживает начало следующего расчёта и уступает место только настоящему
/// ответу (`apply_facts`).
///
/// Исключение — потерянный поток (`readiness_lost`): ответа от него нет и,
/// похоже, не будет, а `facts_pending` без этого остаётся взведённым
/// навсегда — набор не посчитается больше ни разу, и карточка застрянет на
/// «Проверяю…» (находка S10). Тогда расчёт начинается заново, а ответ
/// пропавшего потока, если он всё-таки придёт, отбросит поколение
/// (`apply_facts`). Человеку об этом не говорим: он видит, что следующий
/// пересчёт прошёл, и этого довольно.
fn start_facts(ui: &mut Ui, now: Instant) -> Option<u64> {
    if ui.facts_pending && !readiness_lost(ui.facts_since, now) {
        ui.facts_again = true;
        return None;
    }
    // Этот расчёт и есть обещанный повтор — отметку снимаем здесь, а не в
    // ответе: иначе она пережила бы собственное исполнение и дала лишний
    // проход.
    ui.facts_again = false;
    ui.facts_pending = true;
    ui.facts_since = Some(now);
    ui.facts_gen += 1;
    ui.facts_run = ui.facts_gen;
    Some(ui.facts_gen)
}

/// Правило кнопки «Проверить» (задача 6, «Добавлено управляющим»): пока
/// набор уже считается (тактом или прежним нажатием), второй поток за ним
/// не идёт — кнопка не встаёт в очередь отдельно от такта и не отказывает
/// молча, она взводит ту же отметку повтора (`start_facts` сама поставит
/// `facts_again`), а `check_requested` просит, чтобы результат, который
/// применится первым, получил громкий показ (отчёт, строки журнала, вкладка
/// «Журнал») — тот самый, который кнопка обещала своим нажатием.
fn request_check(ui: &mut Ui, now: Instant) -> Option<u64> {
    ui.check_requested = true;
    start_facts(ui, now)
}

/// Ответ потока расчёта; защёлка снимается, отметка полного расчёта ставится
/// и «Проверяю…» гаснет на ЛЮБОМ пришедшем наборе от владельца защёлки
/// (Critical 1 ревью задач 5+6). Возвращает `true`, когда набор надо
/// посчитать заново — ровно один раз; сам пришедший набор при этом всё равно
/// применяется, `again` значит «посчитай ещё раз после», а не «этот ответ не
/// нужен».
///
/// Чужое поколение значит, что набор моложе этого ответа. Тогда не трогаем
/// ни вердикт, ни отметку повтора — съесть её здесь означало бы отбросить
/// заведомо более свежий запрос. Отметку исполнит и снимет тот расчёт,
/// который её и прочтёт (`start_facts`).
///
/// Выключенное обслуживание — отдельный исход: набор не сохраняется вовсе
/// (`verdict_of(false, ..)` и так дал бы `Off`, но пустой `facts` важен и
/// сам по себе — поверхности не должны показывать устаревшие факты
/// выключенной базы).
fn apply_facts(ui: &mut Ui, gen: u64, facts: Vec<Check>) -> bool {
    // Защёлку снимает только тот расчёт, который её держит (`facts_run`), —
    // его ответ и значит «поток кончился». Раньше её снимал любой ответ, и
    // опоздавший ответ потерянного потока отпирал защёлку живого: следующее
    // событие поднимало вторую пробу инструментов поверх первой (находка
    // N1). Ответ не-владельца защёлку не трогает вовсе; если владелец не
    // ответит никогда, её подберёт сторож `readiness_lost`.
    if gen == ui.facts_run {
        ui.facts_pending = false;
        ui.facts_since = None;
        // Отметка полного расчёта — на ЛЮБОЙ пришедший набор от владельца
        // защёлки, ДО разбора повтора (Critical 1 ревью задач 5+6). Раньше
        // она ставилась только в ветке «повтора не просили», и получалась
        // живая блокировка: пока держится `ui.busy`/`ui.progress`, такт идёт
        // раз в секунду, `since_full` всё ещё больше порога, значит каждый
        // такт просит повтор — готовый набор выбрасывался, расчёт
        // перезапускался, `last-check.txt` переписывался по кругу, а
        // `gather` ходил к базе без паузы. Ровно анти-цель спеки.
        ui.facts_full_at = Some(Instant::now());
        // «Проверяю…» снимается тем же ответом: `ui.busy` висел до громкого
        // показа, а до него дело не доходило — карточка стояла на «Проверяю…»
        // вечно. Операцию (`Busy::Ext`/`Busy::Project`) ответ расчёта не
        // трогает: её конец объявляет `Ev::Done`.
        //
        // `!check_requested` (Important 4 ревью круга 2): пока нажатие ждёт
        // своего громкого показа, «Проверяю…» гаснуть не должно — иначе
        // карточка мигает, возвращается к вердикту и через проход снова
        // уходит в «Проверяю…». Живой блокировки это не возвращает: цикл
        // рвёт отметка `facts_full_at` выше, а она безусловна.
        if ui.busy == Some(Busy::Check) && !ui.check_requested {
            ui.busy = None;
        }
    }
    if gen != ui.facts_gen {
        return false;
    }
    // Пришедший набор ПРИМЕНЯЕТСЯ всегда: `again` значит «посчитай ещё раз
    // после», а не «этот ответ не нужен» (Critical 1). Выбрасывать свежие
    // факты, которые уже оплачены походом к базе, не за что — следующий
    // расчёт их только подтвердит.
    let again = std::mem::take(&mut ui.facts_again);
    let was = ui.verdict.clone();
    if serving(ui) {
        ui.facts = facts;
    } else {
        ui.facts = Vec::new();
    }
    ui.verdict = verdict_of(serving(ui), &ui.facts);
    // Час — это момент, когда набор стал ПОЛНЫМ (решение 6 спеки), поэтому
    // ставится он только вместе с настоящим набором. Уронённому набору
    // (выключили обслуживание, пока расчёт шёл) час ни к чему: показывать
    // «проверено в 15:54» там, где показывать нечего, — врать вдвойне
    // (Minor 3 ревью задач 7+8).
    ui.facts_at = (!ui.facts.is_empty()).then(clock);
    // Журнал — только на смене вердикта (решение 6 спеки): вердикт
    // пересчитывается по каждому поводу и по такту, и повторять одну и ту же
    // строку значит засыпать журнал. Зелёное тоже новость: прежде говорилось
    // только о жёлтом, и человек, починивший проблему, не читал об этом
    // ничего.
    if let Some(line) = verdict_line(&was, &ui.verdict) {
        note_line(ui, line);
    }
    // Значок считается по вердикту (`tray_state_now` → `panel::card`): без
    // этой строки карточка желтела, а значок оставался прежним — зелёным или
    // «занято» (находка S1 итогового ревью).
    sync_tray(ui, Instant::now());
    again
}

/// Причина жёлтого вердикта человеческими словами, `None` — вердикт не
/// жёлтый. Одна формулировка на журнал и на подсказку значка (`panel::reason`
/// берёт ту же `readiness_reason`, только с большой буквы — он пишет в подвал).
///
/// Жёлтых вердиктов два, и начало строки у них разное (Critical 2 ревью задач
/// 5+6): «не всё готово» значит починить, «занято» — подождать или закрыть
/// базу. Прежде `Busy` не давал причины вовсе, и ни журнал, ни значок о нём не
/// говорили.
fn warn_reason(verdict: &Verdict) -> Option<String> {
    match verdict {
        Verdict::NotReady(c) => Some(format!("не всё готово: {}", readiness_reason(c))),
        Verdict::Busy(c) => Some(format!("занято: {}", readiness_reason(c))),
        _ => None,
    }
}

/// Что сказать в журнал на пришедшем наборе (решение 6 спеки: «обычный расчёт
/// молчит — строка идёт только когда вердикт СМЕНИЛСЯ»). Расчёт идёт по
/// каждому поводу и по такту; говорить на каждом значило бы засыпать журнал
/// одной и той же строкой, а читать его человек перестанет.
///
/// Молчим в трёх случаях, и каждый по своей причине:
///
/// * **«Проверяю» — не новость.** Это отсутствие ответа, а не ответ, и
///   показывать его человеку нельзя (спека, цель 2).
/// * **«Выключено» человек сделал сам** — переключателем, и строка о нём уже
///   есть; второй раз пересказывать его же щелчок незачем.
/// * **то же самое** — вердикт не сменился.
///
/// Слова жёлтых вердиктов берёт `warn_reason` — те же, что подвал и подсказка
/// значка: один факт человек читает одинаково, где бы ни прочёл.
pub fn verdict_line(was: &Verdict, now: &Verdict) -> Option<Line> {
    if matches!(now, Verdict::Checking | Verdict::Off) || was == now {
        return None;
    }
    // Осталось три: `Ready` (у него `warn_reason` молчит — сказать про него
    // нечего, кроме того, что всё сошлось) и два жёлтых.
    Some(match warn_reason(now) {
        Some(text) => Line { kind: Kind::Warn, text },
        None => Line { kind: Kind::Ok, text: VERDICT_READY.into() },
    })
}

/// Строка журнала о зелёном вердикте. «Всё готово» — а не «готово»: рядом в
/// журнале стоит «Готово за 2 мин» от конца операции, и два разных события
/// одним словом читались бы как одно.
pub const VERDICT_READY: &str = "всё готово: агент дойдёт до базы";

/// Сколько в наборе плохих фактов — бейдж «Проблем: N» на вкладке «Журнал» и
/// итог в подвале. Считается ТОЛЬКО исход «плохо» (решение владельца 25.10):
/// занято — не провал ни в окне, ни в отчёте, человек, работающий в базе,
/// получает 18 из 18 и строку «занято».
///
/// Бейдж берёт живой набор (`Ui::facts`), а не снимок последней проверки:
/// снимок и был тем расхождением, ради которого всё затевалось — зелёная
/// карточка при «Проблем: 2» рядом.
pub fn problem_count(facts: &[Check]) -> usize {
    facts.iter().filter(|c| c.outcome() == check::Outcome::Bad).count()
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
        tooltip: tray_tooltip(ui.progress.as_ref(), &ui.link, &ui.cfg.host.mode, &ui.verdict, ui.gate.as_deref(), now),
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
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Warn, text: crate::gui::panel::gate_text(&why) });
            cx.refresh_windows();
            return;
        }
    }
    if cx.global::<Ui>().cfg.host.mode == "local" {
        let ui = cx.global_mut::<Ui>();
        let on = flip_enabled_only(ui);
        // Набор прошлого состояния переключателя старый на экране не держим
        // синхронно (задача 5: «Пошёл новый расчёт — старый не возвращаем»):
        // «Выключено» и так не читает `ui.verdict` вовсе (`card` сворачивает
        // по `link`/`enabled` раньше), а «Включаю…» — свой исход карточки,
        // не читающий вердикт, пока `link` не станет `Up`. К этому моменту
        // `run_readiness` (ниже/в `Ev::Publish`) уже в полёте и заменит его
        // настоящим ответом; задержка — тот же компромисс, что и у такта.
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
                let (facts, ok, why) = match ok.then(|| project::facts_locked(&p)) {
                    Some(Ok(f)) => (Some(f), true, why),
                    Some(Err(e)) => (None, false, e),
                    None => (None, ok, why),
                };
                Ev::Project(ProjectView { dir: p.dir.display().to_string(), branch: p.branch, facts, ok, why })
            },
            || Ev::Project(ProjectView { dir: String::new(), branch: String::new(), facts: None, ok: false, why: PANIC_TEXT.into() }),
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
pub fn run_check(cx: &mut App) {
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
pub const SLOW_HIDDEN: Duration = Duration::from_secs(120);

/// Пора ли идти за дорогими (сетевыми) фактами заново — задача 6. Дешёвый
/// такт (`verify::cheap`, без сети) можно звать часто; дорогой — нет, и эта
/// чистая функция решает, когда разница в дешёвом наборе или истёкший срок
/// того стоят.
///
/// Изменившийся дешёвый факт — повод немедленно, независимо от срока:
/// закрыли базу между тактами — агенту может открыться дорога, и держать
/// это знание устаревшим до следующего медленного такта незачем. Иначе —
/// порог по тому, видно ли окно (`SLOW_OPEN`/`SLOW_HIDDEN`): реже, когда
/// никто не смотрит. Пропущенный надолго такт (окно простояло скрытым
/// сутки) не «протухает» сам по себе — `since_full` просто оказывается
/// больше любого порога, и расчёт идёт сразу же, без отдельной ветки.
pub fn needs_full_pass(prev_cheap: &[Check], now_cheap: &[Check], since_full: Duration, window_open: bool) -> bool {
    if prev_cheap != now_cheap {
        return true;
    }
    since_full >= if window_open { SLOW_OPEN } else { SLOW_HIDDEN }
}

/// Дешёвая половина такта (задача 6) — на потоке окна она НЕ СЧИТАЕТ НИЧЕГО
/// (Important 4 ревью задач 5+6): здесь только решение по отметкам времени,
/// пора ли, и заведённый поток. Замеры ревью на машине владельца: сам
/// `verify::cheap` — 230–330 мс (два `netstat`/`tasklist` на порты, два
/// `git.exe`, скан четырёх каталогов платформ, `exists()` по сетевой шаре —
/// там десятки секунд таймаута SMB), плюс `bridge::configurator_count()` —
/// до половины секунды. Итого такт был 0,6–0,8 с и пять подпроцессов на
/// потоке окна, ровно когда человек смотрит на карточку хода.
///
/// Раз в секунду при открытом окне спрашивается ОДНО — число открытых
/// сеансов 1С; это то, ради чего секундный такт и нужен (спека, цель 6:
/// «человек закрыл Предприятие — через секунду-две позеленело»). Весь
/// дешёвый набор — на медленном такте (`SLOW_OPEN`/`SLOW_HIDDEN`):
/// остальные дешёвые факты за секунду не меняются.
///
/// Ничего не считает при выключенном обслуживании (`serving`, та же причина,
/// что у `start_full_pass`) — дешёвые факты тогда некому показывать. Решение
/// про дорогой расчёт принимает `on_cheap`, когда поток ответит.
fn tick_facts(ui: &mut Ui, now: Instant, window_open: bool) {
    if !serving(ui) || ui.cheap_pending {
        return;
    }
    let since_cheap = ui.cheap_at.map(|t| now.saturating_duration_since(t)).unwrap_or(Duration::MAX);
    let whole = since_cheap >= if window_open { SLOW_OPEN } else { SLOW_HIDDEN };
    ui.cheap_pending = true;
    spawn_cheap(ui, whole);
}

/// Поток дешёвого такта: число открытых сеансов 1С всегда, весь дешёвый набор
/// (`verify::cheap`) — только когда просят (`whole`). Паника обязана
/// договорить, иначе `cheap_pending` остался бы взведённым навсегда и такт
/// замолчал бы до конца сеанса; на панике отдаём незнание, а не выдуманный
/// нуль сеансов.
fn spawn_cheap(ui: &Ui, whole: bool) {
    let ctx = quiet_ctx(ui);
    let mode = ui.cfg.host.mode.clone();
    let link = ui.link.clone();
    let tx = ui.tx.clone();
    std::thread::spawn(move || {
        guarded(
            &tx,
            move || {
                let sessions = crate::bridge::configurator_count();
                let facts = whole.then(|| {
                    let state = link.is_up().then(|| link.label(&mode));
                    verify::cheap(&ctx, state.as_deref(), verify::PortUse::WeListen, sessions)
                });
                Ev::Cheap { sessions: Some(sessions), facts }
            },
            || Ev::Cheap { sessions: None, facts: None },
        );
    });
}

/// Ответ дешёвого такта: решить, пора ли за дорогими фактами. Поводов два —
/// сменилось число открытых сеансов 1С (закрыли «Предприятие» — агенту
/// открылась дорога, и держать это знание устаревшим до медленного такта
/// незачем) и общее правило `needs_full_pass` по всему дешёвому набору.
///
/// Эскалация идёт через ту же защёлку (`facts_pending`), что и кнопка
/// «Проверить»: идущий расчёт такт не обгоняет, он только попросит добавку по
/// его возвращении. Замок при этом не трогается (`start_full_pass`,
/// Critical 3).
///
/// ponytail: `prev_cheap` не обновляется из набора, посчитанного «по поводу»
/// (`run_readiness` из события) — `gather` считает дешёвую половину внутри
/// себя, а наружу её не отдаёт, так что следующий медленный такт посчитает её
/// ещё раз. Лишние 230–330 мс раз в полминуты в фоновом потоке дешевле, чем
/// тащить дешёвую половину из `gather` наружу отдельным возвратом. Станет
/// дорого — отдавать её вместе с набором и класть сюда.
fn on_cheap(ui: &mut Ui, sessions: Option<usize>, facts: Option<Vec<Check>>, now: Instant, window_open: bool) {
    ui.cheap_pending = false;
    if !serving(ui) {
        return;
    }
    // Первый ответ — ещё не смена: сравнивать не с чем.
    let sessions_moved = match sessions {
        Some(n) => ui.sessions.replace(n).is_some_and(|was| was != n),
        None => false,
    };
    if facts.is_some() {
        ui.cheap_at = Some(now);
    }
    let since_full = ui.facts_full_at.map(|t| now.saturating_duration_since(t)).unwrap_or(Duration::MAX);
    let fresh = facts.as_deref().unwrap_or(&ui.prev_cheap);
    let go = sessions_moved || needs_full_pass(&ui.prev_cheap, fresh, since_full, window_open);
    let started = go && start_full_pass(ui);
    // Запомнить прочитанное можно только вместе с расчётом, который его и
    // прочёл (Minor 11): расчёт, которому досталась лишь отметка повтора,
    // пойдёт по старой почве, и если забыть изменение здесь, повода пойти за
    // дорогими фактами больше никто не вспомнит.
    if let Some(f) = facts {
        if started || !go {
            ui.prev_cheap = f;
        }
    }
}

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
            let text = tray_tooltip(ui.progress.as_ref(), &ui.link, &ui.cfg.host.mode, &ui.verdict, ui.gate.as_deref(), now);
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
                let failed = problem_count(&ui.facts);
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
                let status = check_status(failed);
                note_line(ui, status.clone());
                note_line(ui, report_line(&report));
                set_status(ui, status, Some(report_line(&report).text));
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
                let ui = cx.global_mut::<Ui>();
                // Громкость переживает отброшенный ответ: кнопку, нажатую
                // пока считал такт, обслужит тот расчёт, который наконец
                // применится, а не обязательно этот самый.
                let loud_retry = ui.check_requested;
                if let Some(gen) = start_facts(ui, Instant::now()) {
                    spawn_full_pass(ui, gen, loud_retry);
                }
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
            facts: Vec::new(),
            verdict: Verdict::Checking,
            facts_at: None,
            facts_gen: 0,
            facts_run: 0,
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
            facts: Vec::new(),
            verdict: Verdict::Checking,
            facts_at: None,
            facts_gen: 0,
            facts_run: 0,
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
        apply_facts, autoconnect, autostart_enabled, republish_after_save, badge, bind_failure_display, can_cancel, carried_base_url, carried_dump, carried_http_port, check_lines, moved_lines, check_status, confirm_gate_text,
        confirm_text,
        douse_serving, exit_code_for, first_phase, flip_enabled_only, gate_now, is_stale, licence_line, line_of, link_is_current, link_line, local_display, locks_gate, needs_full_pass, note_file, note_line, on_done,
        start_full_pass, tick_facts, on_cheap, Busy,
        note_licence_change_ui, on_line, on_progress, op_refusal, panel, progress_text, project_primary, readiness_reason, refresh_gate, report_line, request_check, save_host,
        problem_count, publication_failure, readiness_lost, run_readiness, save_needs_confirm, serving, sync_tray, tray_state_now, warn_reason, READINESS_LOST,
        stale_line, stale_text, start_facts, took, tick_redraw, tray_tooltip, verdict_line, verdict_of, Dot, Kind, Line,
        Verdict, Running, Tab, TrayState, REDRAW_IDLE, SLOW_HIDDEN, SLOW_OPEN,
        Ui, Which, LOG_CAP, LOG_FILE, TRAY_PNG, VERDICT_READY,
    };
    use crate::cfg::{self, Cfg, Host};
    use crate::session::{self, State};
    use crate::verify;
    use common::check::Check;
    use common::op::OpState;
    use common::{Event, Phase, Progress};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn a_fresh_ui_has_not_run_the_readiness_check_yet() {
        let ui = Ui::for_test();
        assert!(matches!(ui.verdict, Verdict::Checking), "{:?}", ui.verdict);
    }

    /// Вердикт из набора — задача 5. `Outcome::Bad` важнее `Outcome::Busy`
    /// независимо от того, какой факт в наборе раньше: поломку человек
    /// должен увидеть и при занятой базе, а не только свободной.
    #[test]
    fn the_verdict_reads_the_set_in_one_order() {
        let good = Check::pass("база", "есть");
        let busy = Check::busy("в базе открыт сеанс", "1 открыт сеанс 1С", "закройте Предприятие");
        let bad = Check::fail("проект", "не разложен", "1c-connect install");
        assert!(matches!(verdict_of(false, &[good.clone()]), Verdict::Off), "выключено — ничего не считаем");
        assert!(matches!(verdict_of(true, &[]), Verdict::Checking));
        assert!(matches!(verdict_of(true, &[good.clone()]), Verdict::Ready));
        assert!(matches!(verdict_of(true, &[busy.clone(), good]), Verdict::Busy(_)));
        // Поломка важнее занятости: её человек должен увидеть и при открытой базе.
        assert!(matches!(verdict_of(true, &[busy, bad]), Verdict::NotReady(_)));
    }

    /// Пошёл новый расчёт — набор на экране остаётся прежним, «Проверяю…» не
    /// возвращается (задача 5): такт гоняет этот путь каждые секунды, и
    /// сброс на каждый старт превратил бы карточку в мигающую.
    #[test]
    fn a_recalculation_keeps_the_previous_verdict_on_screen() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        let gen = ui.facts_gen;
        apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
        assert!(matches!(ui.verdict, Verdict::Ready));
        start_facts(&mut ui, Instant::now());
        assert!(matches!(ui.verdict, Verdict::Ready), "{:?}", ui.verdict);
    }

    /// Ответ, пришедший уже после того, как обслуживание выключили, не
    /// зеленит и не красит карточку своим содержимым: `apply_facts` смотрит
    /// `serving` на момент применения, а не на момент старта расчёта.
    ///
    /// `mode = "local"` — обязательно (Important 7 ревью задач 5+6): у
    /// `Ui::for_test` режим туннельный и `link = State::Off`, поэтому
    /// `serving()` там ложно независимо от `ui.enabled` — тест оставался
    /// зелёным и без строки, которую он проверяет. Обратная половина внизу
    /// это и сторожит: при поднятом флаге тот же набор применяется.
    #[test]
    fn a_set_that_arrives_after_the_switch_went_off_is_dropped() {
        let fact = || vec![Check::fail("проект", "не разложен", "install")];
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(false, Ordering::Relaxed);
        assert!(!serving(&ui), "выключено — иначе тест проверяет не то");
        let gen = ui.facts_gen;
        apply_facts(&mut ui, gen, fact());
        assert!(matches!(ui.verdict, Verdict::Off), "{:?}", ui.verdict);
        assert!(ui.facts.is_empty());

        // А при поднятом флаге тот же набор доезжает — значит отбросил его
        // именно выключенный переключатель, а не что-то ещё.
        ui.enabled.store(true, Ordering::Relaxed);
        assert!(serving(&ui));
        apply_facts(&mut ui, gen, fact());
        assert!(matches!(ui.verdict, Verdict::NotReady(_)), "{:?}", ui.verdict);
        assert_eq!(ui.facts.len(), 1);
    }

    /// Critical 1 ревью задач 5+6 — живая блокировка: «Проверить» зависало
    /// навсегда. Пока держится `ui.busy`, такт идёт раз в секунду и каждый
    /// раз просит повтор; прежний код в этом случае выбрасывал готовый
    /// набор, не ставил отметку полного расчёта и не снимал `ui.busy` — то
    /// есть карточка стояла на «Проверяю…» вечно, а `gather` ходил к базе
    /// без паузы.
    #[test]
    fn a_set_that_arrives_while_a_repeat_is_pending_is_applied_and_ends_the_chain() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.busy = Some(Busy::Check);
        let gen = start_facts(&mut ui, Instant::now()).expect("расчёт пошёл");
        // Такт при идущем расчёте: второго потока нет, просьба о повторе есть.
        assert!(start_facts(&mut ui, Instant::now()).is_none());
        assert!(ui.facts_again);

        let again = apply_facts(&mut ui, gen, vec![Check::fail("проект", "не разложен", "install")]);
        assert!(again, "повтор обещан — он и будет, ровно один");
        assert!(matches!(ui.verdict, Verdict::NotReady(_)), "готовый набор выброшен: {:?}", ui.verdict);
        assert!(ui.facts_full_at.is_some(), "отметка полного расчёта не поставлена — такт просил бы повтор вечно");
        assert_eq!(ui.busy, None, "«Проверяю…» не снято — карточка стоит на нём навсегда");

        // Отметка свежая, и следующий такт на том же дешёвом наборе молчит —
        // череда расчётов кончилась сама, без отдельного тормоза.
        let since = Instant::now().saturating_duration_since(ui.facts_full_at.unwrap());
        assert!(!needs_full_pass(&ui.prev_cheap, &ui.prev_cheap, since, true));
        // Операцию ответ расчёта не трогает — её конец объявляет `Ev::Done`.
        ui.busy = Some(Busy::Project);
        let own = ui.facts_gen;
        ui.facts_run = own;
        apply_facts(&mut ui, own, Vec::new());
        assert_eq!(ui.busy, Some(Busy::Project));
    }

    /// Critical 3 ревью задач 5+6: холодный замок на такт не переезжает
    /// (спека, «Решения, принятые мной», п. 5). Защёлкнувшись, он гасит
    /// обслуживание и рвёт сеанс — на секундном такте уснувшая флешка с
    /// комплектом выключала коннектор под человеком. В каталоге теста замок
    /// заперт (ни инструментов, ни платформы), поэтому `refresh_gate` на этом
    /// пути сразу виден: `ui.gate` стал бы `Some`.
    #[test]
    fn the_tick_path_never_asks_the_cold_gate() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        assert!(gate_now(&ui.kit, &ui.cfg).is_some(), "в каталоге теста замок обязан быть заперт");
        ui.gate = None;

        // Защёлка СВОБОДНА, и расчёт правда заводится (Important 3 ревью
        // круга 2): прежняя версия входила с занятой защёлкой и выходила на
        // раннем возврате, так что `refresh_gate`, поставленный после
        // проверки защёлки, тест прошёл бы. Поток расчёта до `Ui` не
        // дотягивается — он шлёт `Ev`, — так что смотреть после возврата
        // можно без гонки.
        assert!(!ui.facts_pending, "защёлка должна быть свободна — иначе тест идёт по раннему возврату");
        assert!(start_full_pass(&mut ui), "расчёт по такту не начался — тест проверяет не то");
        assert!(ui.facts_pending, "защёлка не взведена");
        assert!(ui.gate.is_none(), "такт позвал замок — он может погасить обслуживание под человеком");
        assert!(ui.enabled.load(Ordering::Relaxed), "и он его погасил");

        // А путь событий (включение, «Сохранить», конец операции) замок
        // по-прежнему считает — он остался на своих прежних поводах.
        run_readiness(&mut ui);
        assert!(ui.gate.is_some(), "замок пропал и с событий тоже");
    }

    /// Итог «Проверить» и есть вердикт (спека): полный набор в этот момент
    /// уже посчитан, первый провал — причина в карточке. Без этой связки
    /// живой случай заказчика 2026-10-20 повторяется дословно: точка
    /// зелёная, а проверка тут же нашла два провала.
    #[test]
    fn a_finished_check_becomes_the_verdict() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        let gen = ui.facts_gen;
        let checks = vec![
            Check::pass("платформа 1С", "есть"),
            Check::note("комплект на носителе", "FAT32"),
            Check::fail("проект", "/home/dev3/project не разложен", "1c-connect install"),
            Check::fail("Gitea", "не отвечает", "dev1c-setup check"),
        ];
        apply_facts(&mut ui, gen, checks);
        match &ui.verdict {
            Verdict::NotReady(c) => assert_eq!(c.what, "проект", "первый провал, сведение пропущено"),
            other => panic!("{other:?}"),
        }
        // Проверка прошла — вердикт зелёный, тем же набором.
        apply_facts(&mut ui, gen, vec![Check::pass("платформа 1С", "есть"), Check::note("комплект на носителе", "FAT32")]);
        assert!(matches!(ui.verdict, Verdict::Ready), "{:?}", ui.verdict);
        // Пустой набор — «ещё не посчитано», а не прежний («всё хорошо»):
        // прежний вердикт мог быть и `Ready`, и точка осталась бы зелёной
        // при упавшей проверке.
        apply_facts(&mut ui, gen, Vec::new());
        assert!(matches!(ui.verdict, Verdict::Checking), "{:?}", ui.verdict);
    }

    /// Поколение набора — та же защита, что у состояния связи
    /// (`link_is_current`): набор считается десяток секунд, и ответ прежнего
    /// расчёта не должен затереть свежий.
    #[test]
    fn a_stale_generation_answer_is_dropped() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.facts_gen = 2;
        ui.facts_pending = true;
        assert!(!apply_facts(&mut ui, 2, vec![Check::fail("Gitea", "не отвечает", "dev1c-setup check")]));
        assert!(matches!(ui.verdict, Verdict::NotReady(_)), "{:?}", ui.verdict);

        // Ответ первого поколения приходит позже — вердикт он не трогает.
        // И защёлку тоже: её держит другой, идущий прямо сейчас расчёт
        // (`facts_run`), а снять её чужим ответом значило бы пустить вторую
        // пробу инструментов поверх живой (находка N1 повторного ревью).
        ui.facts_pending = true;
        ui.facts_since = Some(Instant::now());
        ui.facts_run = 2;
        assert!(!apply_facts(&mut ui, 1, Vec::new()));
        assert!(matches!(ui.verdict, Verdict::NotReady(_)), "{:?}", ui.verdict);
        assert!(ui.facts_pending, "чужой ответ снял защёлку с живого расчёта");
        assert!(ui.facts_since.is_some(), "и отметку времени — сторожу нечем было бы мерить");
        // Ответ владельца снимает и то, и другое.
        assert!(!apply_facts(&mut ui, 2, Vec::new()));
        assert!(!ui.facts_pending && ui.facts_since.is_none());
    }

    /// Цепочка «Сохранить → публикация → операция» не должна поднимать три
    /// пробы подряд и держать точку «занято» минуту: пока поток считает,
    /// новое событие только помечает повтор.
    #[test]
    fn a_second_recompute_waits_for_the_first() {
        let mut ui = Ui::for_test();
        let gen = start_facts(&mut ui, Instant::now()).expect("первый расчёт идёт сам");
        assert!(ui.facts_pending);

        assert!(start_facts(&mut ui, Instant::now()).is_none(), "второго потока нет");
        assert!(ui.facts_again, "повтор помечен");
        assert_eq!(ui.facts_gen, gen, "поколение не трогаем — считает всё тот же поток");

        // Возврат потока: ровно один пересчёт, и ответ про старую почву не применён.
        assert!(apply_facts(&mut ui, gen, Vec::new()), "по возврату — пересчитать");
        assert!(!ui.facts_pending && !ui.facts_again);
        assert_eq!(start_facts(&mut ui, Instant::now()), Some(gen + 1));
    }

    /// Добавлено управляющим после ревью задач 3+4 (задача 6): «Проверить»
    /// не плодит второй поход к базе, пока такт (или прежняя кнопка) уже
    /// считает — защёлка одна на обоих. Кнопка не отказывает молча: она
    /// помечает повтор (`facts_again`, та же отметка, что у такта) и
    /// просит себе громкий показ (`check_requested`), который переживёт
    /// чужой, отброшенный ответ.
    #[test]
    fn the_check_button_does_not_race_the_tick() {
        let mut ui = Ui::for_test();
        let gen = start_facts(&mut ui, Instant::now()).expect("такт начал расчёт первым");
        assert!(ui.facts_pending);

        assert!(request_check(&mut ui, Instant::now()).is_none(), "кнопка пошла в базу вторым потоком");
        assert!(ui.facts_again, "повтор помечен — тем же флагом, что у такта");
        assert!(ui.check_requested, "и громкий показ достанется ему");

        // Чужой (тактовый) ответ вернулся — он отброшен (`again`), но
        // отметка кнопки переживает чужой итог: громкий показ не потерян.
        assert!(apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]));
        assert!(ui.check_requested, "отметка кнопки не съедена чужим ответом");
    }

    /// Такт и поводы для пересчёта — задача 6. Изменившийся дешёвый факт
    /// тянет дорогие независимо от срока; неизменившийся — молчит до срока,
    /// который пореже при скрытом окне; а пропущенный надолго такт не
    /// «протухает»: `since_full` просто больше любого порога.
    #[test]
    fn a_changed_cheap_fact_pulls_the_expensive_ones() {
        let free = vec![Check::pass("в базе открыт сеанс", "нет")];
        let busy = vec![Check::busy("в базе открыт сеанс", "1 открыт сеанс 1С", "закройте")];
        assert!(needs_full_pass(&busy, &free, Duration::ZERO, true), "закрыли базу — спросить заново");
        assert!(!needs_full_pass(&free, &free, Duration::from_secs(5), true), "ничего не менялось — молчим");
        assert!(needs_full_pass(&free, &free, SLOW_OPEN, true), "медленный такт при открытом окне");
        assert!(!needs_full_pass(&free, &free, SLOW_OPEN, false), "при закрытом окне такт реже");
        assert!(needs_full_pass(&free, &free, SLOW_HIDDEN, false));
        // Окно было скрыто сутки — набор не «протухает»: считаем сразу.
        assert!(needs_full_pass(&free, &free, Duration::from_secs(86_400), false));
    }

    /// Important 4 ревью задач 5+6: на потоке окна такт НЕ СЧИТАЕТ НИЧЕГО —
    /// он только решает по отметкам времени, пора ли, и заводит поток. Прежний
    /// такт звал на потоке окна `verify::cheap` и `configurator_count`:
    /// 0,6–0,8 с и пять подпроцессов, ровно когда человек смотрит на карточку.
    #[test]
    fn the_window_thread_tick_only_raises_the_latch() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.sessions = Some(3);
        ui.prev_cheap = vec![Check::pass("в базе открыт сеанс", "нет")];

        // Защёлка СВОБОДНА, и такт правда доходит до конца (Important 3 ревью
        // круга 2): прежняя версия входила с `cheap_pending = true` и выходила
        // на собственном страже, так что все четыре `assert` были истинны
        // тривиально — верни `verify::cheap` на поток окна после стража, и
        // тест бы не заметил. Поток `spawn_cheap` до `Ui` не дотягивается, он
        // шлёт `Ev`, поэтому смотреть сразу после возврата можно без гонки.
        assert!(!ui.cheap_pending, "защёлка должна быть свободна — иначе тест идёт по раннему возврату");
        tick_facts(&mut ui, Instant::now(), true);
        assert!(ui.cheap_pending, "такт не завёл потока — тест проверяет не то");
        assert_eq!(ui.sessions, Some(3), "такт посчитал сеансы на потоке окна");
        assert_eq!(ui.prev_cheap.len(), 1, "такт посчитал дешёвый набор на потоке окна");
        assert!(ui.cheap_at.is_none(), "такт отметил дешёвый набор, которого не считал");
        assert!(!ui.facts_again && !ui.facts_pending, "такт сам полез за дорогими фактами");

        // Пока поток не ответил, второго такт не заводит.
        let before = ui.facts_gen;
        tick_facts(&mut ui, Instant::now(), true);
        assert_eq!(ui.facts_gen, before);

        // Выключено — не считается ничего, и поток не заводится (спека).
        ui.cheap_pending = false;
        ui.enabled.store(false, Ordering::Relaxed);
        tick_facts(&mut ui, Instant::now(), true);
        assert!(!ui.cheap_pending, "при выключенном обслуживании такт всё равно пошёл считать");
    }

    /// Important 8 ревью задач 5+6: проводка такта (Critical 1 и 3 живут
    /// именно в ней) не была покрыта вовсе. Ответ дешёвого потока обязан
    /// отпустить свою защёлку, не обогнать идущий расчёт, не позвать замок,
    /// не терять изменение (Minor 11) и ничего не считать при выключенном
    /// обслуживании.
    #[test]
    fn a_cheap_answer_asks_for_the_expensive_pass_without_overtaking_it() {
        let free = vec![Check::pass("в базе открыт сеанс", "нет")];
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.gate = None;
        // Полный расчёт уже идёт: эскалация обязана только попросить добавку.
        ui.facts_pending = true;
        ui.facts_since = Some(Instant::now());
        ui.cheap_pending = true;

        on_cheap(&mut ui, Some(0), Some(free.clone()), Instant::now(), true);
        assert!(!ui.cheap_pending, "защёлка дешёвого такта не отпущена — такт замолчал бы навсегда");
        assert!(ui.facts_again, "первый дешёвый набор не позвал дорогие факты");
        assert!(ui.cheap_at.is_some(), "отметка дешёвого набора не поставлена");
        assert!(ui.gate.is_none(), "такт позвал холодный замок (Critical 3)");
        assert_eq!(ui.sessions, Some(0));
        // Minor 11: расчёт получил только отметку повтора — прочитанное за ним
        // не запоминаем, иначе изменение пропадёт между тактом и ответом.
        assert!(ui.prev_cheap.is_empty(), "дешёвый набор запомнен расчётом, который его не читал");

        // Сменилось число открытых сеансов — повод переспросить базу сам по
        // себе, без всего дешёвого набора: ровно ради этого нужен секундный
        // такт («закрыл Предприятие — через секунду-две позеленело»).
        ui.facts_again = false;
        on_cheap(&mut ui, Some(1), None, Instant::now(), true);
        assert!(ui.facts_again, "открытое/закрытое «Предприятие» не позвало пересчёт");
        assert_eq!(ui.sessions, Some(1));

        // То же число, тот же дешёвый набор и свежая отметка полного расчёта —
        // повода нет, и прочитанное запоминается (сравнивать следующий такт
        // будет с ним).
        ui.facts_again = false;
        ui.facts_full_at = Some(Instant::now());
        ui.prev_cheap = free.clone();
        on_cheap(&mut ui, Some(1), Some(free.clone()), Instant::now(), true);
        assert!(!ui.facts_again, "такт зовёт дорогие факты на каждом тике");
        assert_eq!(ui.prev_cheap, free);

        // Выключили — ответ потока защёлку отпускает и больше ничего.
        ui.cheap_pending = true;
        ui.enabled.store(false, Ordering::Relaxed);
        on_cheap(&mut ui, Some(7), Some(Vec::new()), Instant::now(), true);
        assert!(!ui.cheap_pending);
        assert!(!ui.facts_again, "вердикт считается при выключенном обслуживании");
        assert_eq!(ui.sessions, Some(1), "при выключенном обслуживании такт что-то запомнил");
        assert_eq!(ui.prev_cheap, free);
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

    /// Решение 2 спеки: главная кнопка вкладки «Проект» выбирается по ФАКТУ
    /// расширения из набора, а не по состоянию операций (`run::status`), и
    /// имя факта берётся у того же `verify::EXTENSION`, что печатает отчёт —
    /// своего, второго имени у окна нет.
    #[test]
    fn the_project_button_installs_until_the_extension_is_up_then_exports() {
        let ext = |c: Check| project_primary(&[Check::pass("база", "есть"), c]);
        // Набора ещё нет — кнопке сказать нечего: «Установить» было бы
        // утверждением о том, чего мы не знаем (Minor 4).
        assert_eq!(project_primary(&[]), None);
        assert_eq!(project_primary(&[Check::pass("база", "есть")]), None, "факта расширения нет — утверждать нечего");
        assert_eq!(ext(Check::fail(verify::EXTENSION, "расширения в базе нет", "install")), Some(Which::Install));
        // Расширение стоит — главная кнопка про обмен с git.
        assert_eq!(ext(Check::pass(verify::EXTENSION, "0.4.8")), Some(Which::Export));
        // Занято — не провал: советовать «Установить» человеку, который
        // просто работает в базе, нельзя (STATUS 26).
        assert_eq!(ext(Check::busy(verify::EXTENSION, "сеансов больше не осталось", "закройте «Предприятие»")), Some(Which::Export));
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
        let ok = Verdict::Ready;
        assert_eq!(tray_tooltip(Some(&r), &State::Up { since: now }, "local", &ok, None, now), "1c-connect: выгружаю конфигурацию из 1С, 12 мин");
        let fresh = Running { started: now, seen: now, ..r };
        assert_eq!(tray_tooltip(Some(&fresh), &State::Off, "local", &ok, None, now), "1c-connect: выгружаю конфигурацию из 1С, меньше минуты");
        assert_eq!(tray_tooltip(None, &State::Up { since: now }, "local", &ok, None, now), "1c-connect: подключён");
        assert_eq!(tray_tooltip(None, &State::Off, "local", &ok, None, now), "1c-connect: не подключён");
        // Причина замка — в подсказке значка; идущая операция её побеждает
        // (тот же порядок разрешения, что в карточке).
        let why = Some("не задан пользователь базы");
        assert_eq!(tray_tooltip(None, &State::Off, "local", &ok, why, now), "1c-connect: нельзя включить: не задан пользователь базы");
        assert_eq!(tray_tooltip(Some(&r), &State::Off, "local", &ok, why, now), "1c-connect: выгружаю конфигурацию из 1С, 12 мин");
    }

    /// Critical 2 ревью задач 5+6 — «занято» горело ЗЕЛЁНЫМ. На стенде
    /// человек с открытым «Предприятием» видел зелёную точку и «MCP
    /// включены», хотя агенту в эту минуту откажут; владелец 25.10:
    /// «не должно быть противоречий, когда MCP типа подключены, горит
    /// зелёный, а в статусах что-то красным горит». Четыре поверхности —
    /// карточка, подвал, подсказка значка и сам значок — обязаны сказать
    /// «занято» словами факта, и ни одна из них не зелёная.
    #[test]
    fn a_busy_verdict_is_never_green_on_any_surface() {
        let now = Instant::now();
        // Пара, которую код правда рождает (Minor 9 ревью круга 2): отказ
        // базы по числу сеансов, словами и советом самой платформы. Прежде
        // тест лепил факт «в базе открыт сеанс» с советом «закройте
        // Предприятие» — такой пары `verify` уже не отдаёт, открытый сеанс
        // стал сведением.
        let cause = crate::onec_error::explain("Превышено количество сеансов").expect("ограничение узнаётся");
        let fact = Check::busy("расширение в базе", cause.what, cause.fix);
        let busy = Verdict::Busy(fact);
        let up = State::Up { since: now };

        // Карточка: точка не зелёная, заголовок — состояние, а не проза.
        let c = panel::card(None, &up, "local", &busy, None, now);
        assert_ne!(c.dot, Dot::Up, "«занято» показано зелёным: {:?}", c.title);
        assert_eq!((c.dot, c.title.as_str()), (Dot::Busy, "Занято"));
        assert_ne!(c.title, "MCP включены");

        // Подвал: причина — словами факта.
        let why = panel::reason(&up, "local", &busy, None, Some("15:54")).expect("причина занятости названа");
        assert!(why.starts_with("Занято:"), "{why}");
        assert!(why.contains("расширение в базе") && why.contains(cause.what), "{why}");
        assert!(why.contains(cause.fix), "{why}");

        // Подсказка значка — та же причина, теми же словами.
        let tip = tray_tooltip(None, &up, "local", &busy, None, now);
        assert!(tip.contains("занято") && tip.contains(cause.what), "{tip}");
        assert_ne!(tip, "1c-connect: подключён");

        // Значок: жёлтый «занято», не зелёный.
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.link = up;
        ui.gate = None;
        ui.verdict = busy;
        assert_eq!(tray_state_now(&ui, now), TrayState::Busy);

        // И журнал про занятость говорит — своими словами, не «не всё готово».
        let line = warn_reason(&ui.verdict).expect("журналу есть что сказать");
        assert!(line.starts_with("занято:"), "{line}");
    }

    /// Important 1 ревью задач 7+8: «выключено значит выключено». Набор
    /// сбрасывался в одном месте — `apply_facts`, — а при выключенном
    /// обслуживании расчёт не идёт вовсе, и прежний набор оставался живым:
    /// серая карточка «MCP выключены» рядом с красным бейджем «Проблем: 1» и
    /// вкладка «Проект» со старыми красными точками.
    ///
    /// Оба пути выключения, и оба кончаются `sync_tray`: ручное (щелчок —
    /// `flip_enabled_only` + `set_link`, за которым она и стоит) и погашенное
    /// замком (`douse_serving`).
    #[test]
    fn the_switch_going_off_drops_the_set() {
        let bad = || vec![Check::fail("проект", "не разложен", "1c-connect install")];
        // Пока обслуживаем — набор на месте: иначе этот тест был бы зелёным и
        // на безусловном сбросе, то есть на сломанном коде.
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(true, Ordering::Relaxed);
        assert!(serving(&ui));
        let gen = ui.facts_gen;
        apply_facts(&mut ui, gen, bad());
        assert_eq!(ui.facts.len(), 1, "набор не применился — тест проверяет не то");
        assert_eq!(problem_count(&ui.facts), 1);
        sync_tray(&mut ui, Instant::now());
        assert_eq!(problem_count(&ui.facts), 1, "набор уронили при живом обслуживании");
        assert!(matches!(ui.verdict, Verdict::NotReady(_)), "{:?}", ui.verdict);

        // Путь первый: щелчок по переключателю.
        assert!(!flip_enabled_only(&mut ui), "щелчок выключил");
        ui.link = local_display("local", false, ui.link.clone());
        sync_tray(&mut ui, Instant::now());
        assert!(!serving(&ui));
        assert!(ui.facts.is_empty(), "бейдж и вкладка показывают набор выключенного обслуживания");
        assert_eq!(problem_count(&ui.facts), 0);
        assert_eq!(ui.verdict, Verdict::Off, "{:?}", ui.verdict);
        assert!(ui.facts_at.is_none(), "«проверено в 15:54» при пустом наборе");

        // Путь второй: замок защёлкнулся и погасил обслуживание сам.
        ui.enabled.store(true, Ordering::Relaxed);
        let gen = ui.facts_gen;
        apply_facts(&mut ui, gen, bad());
        assert_eq!(ui.facts.len(), 1, "набор не вернулся — второй путь проверять нечем");
        douse_serving(&mut ui);
        assert!(!serving(&ui));
        assert!(ui.facts.is_empty(), "замок погасил обслуживание, а набор остался на экране");
        assert_eq!(ui.verdict, Verdict::Off, "{:?}", ui.verdict);
    }

    /// Minor 3 ревью задач 7+8: час «проверено в 15:54» — это момент, когда
    /// набор стал ПОЛНЫМ. Уронённому набору час ни к чему: показывать его
    /// там, где показывать нечего, — врать вдвойне.
    #[test]
    fn the_hour_belongs_to_a_full_set() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(true, Ordering::Relaxed);
        let gen = ui.facts_gen;
        apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
        assert!(ui.facts_at.is_some(), "набор полный, а часа нет");

        // Пустой набор при живом обслуживании — часа ему не положено
        // (`apply_facts`): час ставится только вместе с настоящим набором.
        assert!(serving(&ui), "иначе час уронило бы выключение, а не эта ветка");
        let gen = ui.facts_gen;
        apply_facts(&mut ui, gen, Vec::new());
        assert!(ui.facts_at.is_none(), "час остался от набора, которого нет");

        // И выключение (`sync_tray`) уносит час вместе с набором.
        let gen = ui.facts_gen;
        apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
        assert!(ui.facts_at.is_some());
        ui.enabled.store(false, Ordering::Relaxed);
        ui.link = local_display("local", false, ui.link.clone());
        sync_tray(&mut ui, Instant::now());
        assert!(ui.facts.is_empty());
        assert!(ui.facts_at.is_none(), "«проверено в 15:54» при выключенном обслуживании");
    }

    /// Minor 2 ревью задач 7+8: совет в журнале — у любого факта, которому
    /// есть что добавить, а не только у провалившегося. У занятого факта
    /// («закройте «Предприятие»») он терялся, и человек читал проблему без
    /// выхода из неё.
    #[test]
    fn the_log_keeps_the_hint_of_a_busy_fact_too() {
        let lines = check_lines(&[
            Check::busy("в базе открыт сеанс", "сеансов больше не осталось", "закройте «Предприятие»"),
            // Совет уже вписан в сам `detail` — вторым разом не повторяем
            // (то же правило, по которому склеивает `fact_detail`).
            Check::fail("инструменты комплекта", "в комплекте нет vrunner — скопируйте заново", "скопируйте заново"),
        ]);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts.len(), 3, "{texts:?}");
        assert_eq!(texts[0], "в базе открыт сеанс: сеансов больше не осталось");
        assert_eq!((lines[1].kind, texts[1]), (Kind::Warn, "закройте «Предприятие»"));
        assert_eq!(texts[2], "инструменты комплекта: в комплекте нет vrunner — скопируйте заново");
    }

    /// Задача 8, решение 6 спеки: обычный расчёт молчит. Строка в журнал идёт
    /// только на СМЕНЕ вердикта, и «Проверяю» новостью не бывает — это
    /// отсутствие ответа, а не ответ.
    #[test]
    fn the_journal_speaks_only_when_the_verdict_changes() {
        let bad = Verdict::NotReady(Check::fail("проект", "не разложен", "1c-connect install"));
        let busy = Verdict::Busy(Check::busy("расширение в базе", "сеансов больше не осталось", "закройте «Предприятие»"));
        // То же самое — молчим, сколько бы расчётов ни пришло.
        assert!(verdict_line(&Verdict::Ready, &Verdict::Ready).is_none());
        assert!(verdict_line(&bad, &bad).is_none());
        // «Проверяю» и «выключено» — не новость ни после чего.
        assert!(verdict_line(&bad, &Verdict::Checking).is_none());
        assert!(verdict_line(&Verdict::Ready, &Verdict::Off).is_none());
        // Смена — строка, и словами самого факта.
        let line = verdict_line(&Verdict::Ready, &bad).expect("смена вердикта — строка");
        assert_eq!(line.kind, Kind::Warn);
        assert!(line.text.contains("проект") && line.text.contains("1c-connect install"), "{line:?}");
        // Жёлтое жёлтому не равно: «занято» говорит своими словами.
        let line = verdict_line(&bad, &busy).expect("с поломки на занятость — новость");
        assert!(line.text.starts_with("занято:"), "{line:?}");
        // Зелёное — тоже новость: починивший проблему человек читает об этом.
        let line = verdict_line(&bad, &Verdict::Ready).expect("починилось — новость");
        assert_eq!((line.kind, line.text.as_str()), (Kind::Ok, VERDICT_READY));
        // И сменившаяся ПРИЧИНА при том же состоянии — тоже смена.
        let other = Verdict::NotReady(Check::fail("Gitea", "не отвечает", "dev1c-setup check"));
        assert!(verdict_line(&bad, &other).is_some());

        // Проводка: два одинаковых набора подряд — одна строка, не две.
        // `mode = "local"` обязателен, иначе `serving` ложно (`Ui::for_test`
        // туннельный), набор не применится и тест остался бы зелёным на
        // сломанном коде.
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(true, Ordering::Relaxed);
        assert!(serving(&ui), "иначе тест проверяет не то");
        let fact = || vec![Check::fail("проект", "не разложен", "1c-connect install")];
        let gen = ui.facts_gen;
        let before = ui.log.len();
        apply_facts(&mut ui, gen, fact());
        assert_eq!(ui.log.len(), before + 1, "о смене вердикта журнал не сказал");
        apply_facts(&mut ui, gen, fact());
        assert_eq!(ui.log.len(), before + 1, "тот же вердикт сказан дважды — журнал засыпан");
    }

    /// Задача 8: бейдж «Проблем: N» считает плохие факты ЖИВОГО набора.
    /// Занято — не провал ни в окне, ни в отчёте (решение владельца 25.10):
    /// человек, работающий в базе, получает 18 из 18 и строку «занято».
    #[test]
    fn the_badge_counts_only_bad_facts() {
        let facts = vec![
            Check::pass("база", "C:\\Bases\\Buh"),
            Check::note("комплект на носителе", "FAT32"),
            Check::busy("в базе открыт сеанс", "сеансов больше не осталось", "закройте «Предприятие»"),
            Check::fail("проект", "не разложен", "1c-connect install"),
        ];
        assert_eq!(problem_count(&facts), 1);
        // Пустой набор — не «ноль проблем доказано», но и показывать нечего:
        // бейджа нет, а состояние говорит карточка («Проверяю…»).
        assert_eq!(problem_count(&[]), 0);
        // Набор живой: применился новый — изменилось и число, без всякого
        // нажатия «Проверить».
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(true, Ordering::Relaxed);
        assert!(serving(&ui), "иначе набор не применится и тест проверяет не то");
        let gen = ui.facts_gen;
        apply_facts(&mut ui, gen, facts);
        assert_eq!(problem_count(&ui.facts), 1);
        apply_facts(&mut ui, gen, vec![Check::pass("проект", "в порядке")]);
        assert_eq!(problem_count(&ui.facts), 0, "бейдж держится за снимок, а не за живой набор");
    }

    /// Minor 10 ревью круга 2: пока набора нет, значок жёлтый (`Dot::Busy` из
    /// `panel::card`), а подсказка отдавала «подключён» — зелёное слово под
    /// жёлтым значком, та же ложь, что была в Critical 2, только в трее.
    /// Спека: «значок в трее жёлтый, подсказка „проверяю“».
    #[test]
    fn the_tray_says_it_is_checking_while_there_is_no_set_yet() {
        let now = Instant::now();
        let up = State::Up { since: now };
        assert_eq!(tray_tooltip(None, &up, "local", &Verdict::Checking, None, now), "1c-connect: проверяю");
        assert_eq!(panel::card(None, &up, "local", &Verdict::Checking, None, now).dot, Dot::Busy, "значок и подсказка обязаны говорить одно");
        // Посчитано и всё сошлось — прежняя подсказка про связь.
        assert_eq!(tray_tooltip(None, &up, "local", &Verdict::Ready, None, now), "1c-connect: подключён");
        // В туннельном режиме вердикт карточка не показывает — и подсказка тоже.
        assert_eq!(tray_tooltip(None, &up, "tunnel", &Verdict::Checking, None, now), "1c-connect: подключён");
    }

    /// Important 4 ревью круга 2: отчёт и строки в журнал без нажатия кнопки.
    /// Громкость принадлежит нажатию (`check_requested`), и `start_full_pass`
    /// обязан её спрашивать: иначе потерянный громкий поток заменялся тихим,
    /// тот гасил «Проверяю…», отметку кнопки не снимал, и следующий повтор по
    /// любому поводу становился громким — `last-check.txt` и десяток строк
    /// журнала человеку, который ничего не нажимал.
    ///
    /// Тот же флаг держит «Проверяю…»: пока нажатие ждёт громкого показа,
    /// карточка не мигает.
    #[test]
    fn loudness_belongs_to_the_press_and_the_card_does_not_blink() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.busy = Some(Busy::Check);
        ui.check_requested = true;
        let gen = start_facts(&mut ui, Instant::now()).expect("расчёт пошёл");

        // Тихий ответ пришёл, а кнопке ещё должен громкий показ — «Проверяю…»
        // не гаснет, и отметка кнопки цела.
        apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
        assert_eq!(ui.busy, Some(Busy::Check), "карточка мигнула: «Проверяю…» сняли до отчёта");
        assert!(ui.check_requested, "отметка кнопки съедена, и отчёт достанется чужому повтору");
        // Но отметка полного расчёта безусловна — цикл рвёт именно она.
        assert!(ui.facts_full_at.is_some(), "живая блокировка вернулась");

        // Расчёт, который заводится, пока нажатие не обслужено, — ГРОМКИЙ:
        // отчёт получит он, а не случайный следующий. Громкость видна по
        // `Ev::Ready`, который присылает поток; громкий путь шлёт по дороге
        // ещё и фазу со строками, поэтому ждём именно набор.
        fn loudness_of_the_pass(rx: &mut futures::channel::mpsc::UnboundedReceiver<super::Ev>) -> bool {
            loop {
                match futures::executor::block_on(futures::StreamExt::next(rx)).expect("расчёт не ответил") {
                    super::Ev::Ready(_, _, loud) => return loud,
                    _ => continue,
                }
            }
        }

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        ui.tx = tx;
        assert!(start_full_pass(&mut ui), "повтор не начался");
        assert!(loudness_of_the_pass(&mut rx), "кнопка нажата, а расчёт пошёл тихим — отчёта человек не увидит");
        assert!(ui.kit.state_path("last-check.txt").exists(), "громкий расчёт отчёта не написал");

        // А без нажатия — тихий, и отчёта нет: такт человеку журнал не сыплет.
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        ui.tx = tx;
        assert!(!ui.check_requested);
        assert!(start_full_pass(&mut ui), "расчёт по такту не начался");
        assert!(!loudness_of_the_pass(&mut rx), "тихий расчёт попросил громкий показ");
        assert!(!ui.kit.state_path("last-check.txt").exists(), "отчёт написан без нажатия кнопки");
    }

    #[test]
    fn tick_redraws_when_the_window_is_open_or_something_changed() {
        // Окно открыто и в покое — минуты в карточке связи всё равно не
        // должны стоять, та считает их сама от `since.elapsed()`.
        assert!(tick_redraw(true, false, false, REDRAW_IDLE));
        // Но не чаще прежнего шага такта (Important 4, п. 4): сам такт стал
        // секундным, а перерисовка от этого учащаться не должна.
        assert!(!tick_redraw(true, false, false, Duration::from_secs(1)));
        // Окно спрятано и тихо — перерисовывать нечего.
        assert!(!tick_redraw(false, false, false, Duration::MAX));
        // Спрятано, но пришли новые данные или идёт операция — всё равно да,
        // и срок тут ни при чём.
        assert!(tick_redraw(false, true, false, Duration::ZERO));
        assert!(tick_redraw(false, false, true, Duration::ZERO));
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

    /// `install` записывает выведенный каталог выгрузки в toml — и записанное
    /// умолчание обязано и дальше идти за каталогом проекта, иначе смена
    /// проекта оставила бы выгрузку в старом месте. Свой каталог не трогаем.
    #[test]
    fn carried_dump_follows_the_project_unless_it_was_chosen_by_hand() {
        let new = cfg::derived_dump("C:\\proj\\Torg");
        let mut saved = Host { project: "C:\\proj\\Buh".into(), dump: cfg::derived_dump("C:\\proj\\Buh"), ..Host::default() };
        assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), new);
        saved.dump = "D:\\dump".into();
        assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), "D:\\dump");
        // Пустое поле остаётся пустым: умолчание и так считается при чтении.
        saved.dump = String::new();
        assert_eq!(carried_dump(&saved, "C:\\proj\\Torg"), "");
    }

    /// Та же ловушка у порта публикации: записанный `8080 + N` идёт за
    /// рабочим местом, свой порт остаётся, незаданный — незаданным.
    #[test]
    fn carried_http_port_follows_the_workstation_unless_it_was_chosen_by_hand() {
        let mut saved = Host { workstation: "dev1".into(), http_port: Some(cfg::derived_http_port("dev1")), ..Host::default() };
        assert_eq!(carried_http_port(&saved, "dev2"), Some(8082));
        saved.http_port = Some(9000);
        assert_eq!(carried_http_port(&saved, "dev2"), Some(9000));
        saved.http_port = None;
        assert_eq!(carried_http_port(&saved, "dev2"), None);
    }

    /// Переезд порта не молчком: новый порт может быть занят, и человек
    /// должен прочитать про переезд, а не искать, почему публикация упала.
    #[test]
    fn saving_a_new_workstation_moves_a_derived_port_and_says_so() {
        let mut ui = Ui::for_test();
        ui.cfg.host.http_port = Some(cfg::derived_http_port(&ui.cfg.host.workstation));
        let mut host = ui.cfg.host.clone();
        host.workstation = "dev3".into();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.cfg.host.http_port, Some(8083));
        assert_eq!(ui.cfg.base_url(), "http://localhost:8083/hs/mcp-1c");
        assert!(ui.log.iter().any(|l| l.text.contains("порт публикации 8081 → 8083")), "{:?}", ui.log);
    }

    /// Та же ловушка на живом «Сохранить»: записанный выведенный каталог
    /// выгрузки переезжает вместе с каталогом проекта.
    #[test]
    fn saving_a_new_project_moves_a_derived_dump_with_it() {
        let mut ui = Ui::for_test();
        ui.cfg.host.project = "C:\\proj\\Buh".into();
        ui.cfg.host.dump = cfg::derived_dump("C:\\proj\\Buh");
        let mut host = ui.cfg.host.clone();
        host.project = "C:\\proj\\Torg".into();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.cfg.host.dump, cfg::derived_dump("C:\\proj\\Torg"));
        let text = std::fs::read_to_string(ui.kit.state_path(cfg::FILE)).unwrap();
        assert!(text.contains(&format!("dump = {:?}", cfg::derived_dump("C:\\proj\\Torg"))), "{text}");
        // И переезд назван, а не сделан молча.
        assert!(ui.log.iter().any(|l| l.text.contains("переехал каталог выгрузки")), "{:?}", ui.log);
    }

    /// Правило одно на все переезды: о каждом — строка. За рабочим местом
    /// едут ветка, origin, порты и порт публикации; за каталогом проекта —
    /// каталог выгрузки; ничего не менялось — строк нет.
    #[test]
    fn every_move_gets_a_line_not_just_the_port() {
        let old = Host {
            workstation: "dev1".into(),
            project: "C:\\proj\\Buh".into(),
            dump: cfg::derived_dump("C:\\proj\\Buh"),
            http_port: Some(cfg::derived_http_port("dev1")),
            ..Host::default()
        };
        assert!(moved_lines(&old, &old).is_empty());

        let mut new = old.clone();
        new.workstation = "dev3".into();
        new.http_port = carried_http_port(&old, "dev3");
        let line = moved_lines(&old, &new).join("\n");
        for part in ["dev1 → dev3", "ветка проекта и origin", "7773/7783/7793", "порт публикации 8081 → 8083"] {
            assert!(line.contains(part), "{part} не назван: {line}");
        }

        let mut moved_project = old.clone();
        moved_project.project = "C:\\proj\\Torg".into();
        moved_project.dump = carried_dump(&old, "C:\\proj\\Torg");
        let lines = moved_lines(&old, &moved_project);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains(&cfg::derived_dump("C:\\proj\\Torg")), "{lines:?}");

        // Свой каталог выгрузки не переезжает — и строки про него нет.
        let mut own = old.clone();
        own.dump = "D:\\dump".into();
        let mut own_new = own.clone();
        own_new.project = "C:\\proj\\Torg".into();
        own_new.dump = carried_dump(&own, "C:\\proj\\Torg");
        assert!(moved_lines(&own, &own_new).is_empty());
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

    /// Отменяются и обмен с git, и работа платформы. Выгрузка и загрузка
    /// добавлены 25.10: `local::call` научился замечать флаг и добивать куст
    /// процессов через Job Object, а до того самые долгие операции отмены не
    /// имели вовсе (STATUS, техдолг 16).
    #[test]
    fn git_and_platform_phases_can_be_cancelled() {
        for p in [Phase::Push, Phase::Pull, Phase::Decompile, Phase::Load] {
            assert!(can_cancel(p), "{p:?}");
        }
        // Установка расширения и проверка — минуты и секунды, не десятки
        // минут: кнопку им не даём, чтобы она не обещала больше, чем нужно.
        for p in [Phase::Install, Phase::Uninstall, Phase::Check] {
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
    /// Запись параметров не поднимает HTTP-сервис базы, пока никто не
    /// обслуживает. Заказчик 2026-10-21: «сохранил — в журнале сообщение об
    /// успешном поднятии http сервера, хотя я даже не пытался включить MCP».
    /// Смена базы при выключенных MCP — не повод делать работу, которой не
    /// просили, и тем более объявлять её успехом.
    #[test]
    fn saving_does_not_raise_the_publication_while_nobody_serves() {
        assert!(!republish_after_save(true, false), "MCP выключены — публикацию не трогаем");
        assert!(republish_after_save(true, true), "обслуживаем — сервис обязан смотреть на новую базу");
        assert!(!republish_after_save(false, true), "база та же — поднимать нечего");
    }

    #[test]
    fn saving_away_a_cold_condition_asks_first() {
        use crate::gui::gate::cold_gate;
        let v8 = std::path::Path::new("1cv8.exe");
        let tools = OpState::Done("vrunner 3.0.2".into());
        let before = Cfg::default();
        let mut after = Cfg::default();
        after.host.db_user = String::new();
        let gate_before = cold_gate(&before, true, true, &tools, Ok(v8), true, &crate::licence::State::Valid);
        let gate_after = cold_gate(&after, true, true, &tools, Ok(v8), true, &crate::licence::State::Valid);
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
    /// его негде, а набор стоит до минуты (`verify::SET_DEADLINE`) и
    /// поднимает пробу инструментов (на Linux ещё и виртуальный экран).
    /// Замок при этом пересчитывается всегда: он решает, доступен ли
    /// переключатель (находка S9).
    #[test]
    fn the_verdict_is_not_computed_while_serving_is_off() {
        let mut ui = Ui::for_test();
        ui.cfg.host.mode = "local".into();
        ui.enabled.store(false, Ordering::Relaxed);
        // «Уже считается», чтобы доказательство не подняло настоящий поток
        // фактов: пересчёт тогда только пометит повтор — эта пометка и
        // показывает, дошло ли до расчёта дело.
        ui.facts_pending = true;
        ui.facts_since = Some(Instant::now());
        ui.facts_again = false;
        ui.tray_shown = None;

        run_readiness(&mut ui);
        assert!(!ui.facts_again, "вердикт считается при выключенном обслуживании");
        assert!(ui.tray_shown.is_some(), "замок обязан пересчитываться и доезжать до значка всегда");

        // Включили — считается само, отдельного события не нужно.
        ui.enabled.store(true, Ordering::Relaxed);
        run_readiness(&mut ui);
        assert!(ui.facts_again, "обслуживание включено, а пересчёт не пошёл");
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
        assert!(readiness_lost(None, now), "защёлку взвели мимо `start_facts` — ждать нечего");

        // Проводка: пока поток жив, второго не заводим; потерянный — заводим.
        let mut ui = Ui::for_test();
        let gen = start_facts(&mut ui, now).expect("первый расчёт идёт сам");
        assert!(start_facts(&mut ui, now + Duration::from_secs(30)).is_none(), "второй поток поверх живого");
        assert_eq!(start_facts(&mut ui, now + READINESS_LOST + Duration::from_secs(1)), Some(gen + 1));
        // Ответ снимает и защёлку, и отметку времени — иначе следующий
        // расчёт считал бы себя потерянным с рождения.
        apply_facts(&mut ui, gen + 1, Vec::new());
        assert!(!ui.facts_pending && ui.facts_since.is_none());
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
    /// известен в `apply_facts`, и оттуда он обязан дойти до значка и до
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
        let gen = ui.facts_gen;
        let fail = Check::fail("проект", "/home/dev3/project не разложен", "1c-connect install");

        let log_before = ui.log.len();
        ui.facts_pending = true;
        assert!(!apply_facts(&mut ui, gen, vec![fail.clone()]));
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
        ui.facts_pending = true;
        apply_facts(&mut ui, gen, vec![fail]);
        assert_eq!(ui.log.len(), log_before + 1, "{:?}", &ui.log[log_before..]);

        // А зелёный вердикт доезжает до значка так же.
        ui.facts_pending = true;
        apply_facts(&mut ui, gen, vec![Check::pass("проект", "в порядке")]);
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
        ui.facts_pending = true;
        ui.facts_since = Some(Instant::now());
        ui.facts_again = false;
        ui.tray_shown = None;

        note_licence_change_ui(&mut ui);

        assert_eq!(ui.licence, real, "новое состояние срока не запомнено");
        assert!(ui.facts_again, "смена срока не привела к пересчёту готовности");
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
