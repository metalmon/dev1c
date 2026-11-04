//! Значок в трее, его меню (включая подменю «Тема») и мост событий ОС в
//! канал приложения: обработчики `tray-icon`/`muda` зовёт WndProc на потоке
//! GPUI, они только кладут событие в канал — его разбирает `on_tray`/`on_menu`
//! из задачи-читателя в `mod.rs::handle`.

use super::{
    autostart, badge, confirm_database_change, confirm_database_change_with_text, confirm_switch_current_text, descriptor_changed,
    ensure_publication_now, link, note_line, quit, republish_after_save, run_check, run_readiness, save_host, save_needs_confirm,
    save_tray_key, save_tray_keys, start_session, stop_session, switch_current, tab_for_unsaved_switch, theme, toggle_agent, toggle_link,
    window, Ev, Kind, Line, Tab, TrayState, Ui, TRAY_PNG,
};
use crate::editors;
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::App;
use std::sync::atomic::Ordering;
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Текст пункта «Запускать при входе»: на Windows речь про вход в саму
/// Windows, иначе — просто про вход (нет чего-то другого, с чем спутать).
#[cfg(windows)]
const AUTOSTART_TEXT: &str = "Запускать при входе в Windows";
#[cfg(not(windows))]
const AUTOSTART_TEXT: &str = "Запускать при входе";

/// Трей: значок, пункт-переключатель (меняет надпись), подменю «Запись в
/// базу» и «Локальные агенты» (в первом отмечен ровно один, во втором —
/// независимые галочки по агенту), три пункта темы (отмечен ровно один) и
/// два пункта-галочки — автозапуск и автоподключение.
///
/// Режима работы и публикации базы здесь больше нет (решение владельца 1.11:
/// «публикация — поле соединения, и пункт меню убран вовсе»; тем же доводом
/// раньше ушёл режим) — переключатель в значке один на машину, а соединений
/// на машине несколько; обоими правит карточка окна (`panel::set_card_mode`,
/// `panel::set_card_publication`).
pub struct Tray {
    icon: TrayIcon,
    toggle: MenuItem,
    /// Потолок записи (`cfg.current().write`/`Cfg::access()`) — радио из
    /// двух пунктов: нативный пункт отмечает себя сам по щелчку, вернуть
    /// отметку на действующее значение обязаны мы.
    access: [CheckMenuItem; 2],
    agents: [CheckMenuItem; 4],
    /// Само меню и раздел агентов в нём — чтобы раздел можно было убрать и
    /// вернуть на месте, а не гасить серым (решение владельца 30.10: «может
    /// скрывать это меню в туннельном режиме?», и его же изначальное «раздела
    /// нет»). `Menu` и `Submenu` в `muda` клонируемы (внутри `Rc`), а
    /// `Menu::remove`/`Menu::insert` правят живое меню — прежний комментарий
    /// здесь утверждал, что без пересборки меню это невозможно, и был неверен.
    menu: Menu,
    agents_submenu: Submenu,
    agents_pos: usize,
    agents_shown: std::cell::Cell<bool>,
    themes: [CheckMenuItem; 3],
    autostart: CheckMenuItem,
    autoconnect: CheckMenuItem,
}

/// Выбор темы → id пункта меню и наоборот.
const THEMES: [(theme::Choice, &str, &str); 3] = [
    (theme::Choice::System, "theme:system", "Системная"),
    (theme::Choice::Light, "theme:light", "Светлая"),
    (theme::Choice::Dark, "theme:dark", "Тёмная"),
];

/// Слова для журнальной строки о смене режима (`apply_mode`) — без
/// идентификаторов пунктов меню: пункта «Режим работы» больше нет (решение
/// владельца 1.11), а словами по-прежнему нужно что-то называть режим в
/// журнале. Режим пишет карточка окна (`panel::set_card_mode` → тут же,
/// `request_current_mode_change`).
const MODE_WORDS: [(&str, &str); 2] = [("tunnel", "Через туннель"), ("local", "Локально")];

/// Потолок записи (`cfg.current().write`, значения — те же слова, что понимает
/// дочерний `mcp-1c`, `read_write`/`read_only`: перевод создал бы третью
/// таблицу соответствий без всякой пользы) → id пункта меню и его подпись.
/// Поля в окне нет — потолок пишет только этот пункт (см. `gui::save_host`,
/// перенос `write` той же строкой, что `platform`/`configuration`).
/// «Только чтение» — первым, как отмеченное по умолчанию (решение
/// владельца 1.11, `Cfg::access()`).
///
/// Накрывает инструменты канала `mcp-1c` — не весь агентский доступ:
/// канал `vrunner` этим потолком не ограничен (подробнее — докстрока
/// `apply_access`).
const ACCESS: [(&str, &str, &str); 2] =
    [("read_only", "access:read_only", "Только чтение"), ("read_write", "access:read_write", "Чтение и запись")];

/// Заголовок подменю «Запись в базу» — называет дело (может ли агент менять
/// данные), а не механику ключа `--access`.
const ACCESS_SUBMENU: &str = "Запись в базу";

/// Набор локальных агентов живёт одним реестром в `editors::AGENTS`
/// (решение владельца 29.10, «Четыре локальных агента, и настраиваются они
/// по-разному») — здесь id пункта меню строится из `def.id` как
/// `agent:<id>`, по той же причине, по которой раньше был отдельный массив:
/// названы агентами, а не редакторами, человек работает с агентом внутри
/// Zed/pi/Claude Code/Codex, не с редактором как таким.
fn agent_menu_id(id: &str) -> String {
    format!("agent:{id}")
}

/// Заголовок подменю «Локальные агенты».
const AGENTS_SUBMENU: &str = "Локальные агенты";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAction {
    /// Показать окно. Пункт есть только на Linux: там значок не сообщает о
    /// щелчках (бэкенд appindicator), и левая кнопка открывает то же меню,
    /// так что иначе окно было бы не достать.
    #[cfg(target_os = "linux")]
    Open,
    Toggle,
    Check,
    /// Выбор пункта подменю «Запись в базу» — несёт "read_write" или "read_only".
    Access(&'static str),
    /// Щелчок по галочке подменю «Локальные агенты» — несёт "zed" или "pi".
    Agent(&'static str),
    Theme(theme::Choice),
    Autostart,
    Autoconnect,
    Quit,
}

/// id пункта меню → действие; незнакомый id (сепараторы под событие не
/// попадают вовсе) — `None`.
pub fn menu_action(id: &str) -> Option<MenuAction> {
    for (choice, theme_id, _) in THEMES {
        if id == theme_id {
            return Some(MenuAction::Theme(choice));
        }
    }
    for (access, access_id, _) in ACCESS {
        if id == access_id {
            return Some(MenuAction::Access(access));
        }
    }
    for def in editors::AGENTS.iter() {
        if id == agent_menu_id(def.id) {
            return Some(MenuAction::Agent(def.id));
        }
    }
    match id {
        #[cfg(target_os = "linux")]
        "open" => Some(MenuAction::Open),
        "toggle" => Some(MenuAction::Toggle),
        "check" => Some(MenuAction::Check),
        "autostart" => Some(MenuAction::Autostart),
        "autoconnect" => Some(MenuAction::Autoconnect),
        "quit" => Some(MenuAction::Quit),
        _ => None,
    }
}

/// Как часто прокручивается главный цикл GTK на Linux (см. `pump`). Тик
/// приложения (секунда в работе, полминуты в покое) для меню слишком редок:
/// человек ждал бы отклика на щелчок.
#[cfg(target_os = "linux")]
pub const PUMP: std::time::Duration = std::time::Duration::from_millis(50);

/// На Linux значок рисует libappindicator, а это GTK-библиотека: без
/// `gtk::init()` первый же пункт меню роняет процесс («GTK has not been
/// initialized»). Зовётся до `build` и на том же потоке, где потом живёт
/// значок, — поэтому `pump` и окно остаются на одном потоке, и пересылать
/// между потоками нечего.
#[cfg(target_os = "linux")]
pub fn init() -> anyhow::Result<()> {
    gtk::init().map_err(|e| anyhow::anyhow!("GTK не инициализирован: {e}"))
}

/// На остальных системах трею GTK не нужен.
#[cfg(not(target_os = "linux"))]
pub fn init() -> anyhow::Result<()> {
    Ok(())
}

/// Разобрать накопившиеся события GTK, не блокируя поток: через них
/// приходят щелчки по меню значка и ответы панели по DBus. Своего цикла у
/// нас нет — его роль играет эта функция из задачи в `launch`.
#[cfg(target_os = "linux")]
pub fn pump() {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn pump() {}

/// Значок трея с бейджем состояния, байты RGBA — тестируемое ядро `icon`,
/// не трогает Win32.
fn icon_rgba(state: TrayState) -> Vec<u8> {
    let base = image::load_from_memory(TRAY_PNG).expect("tray png").to_rgba8();
    badge(&base, state).into_raw()
}

/// Значок трея с бейджем состояния.
pub fn icon(state: TrayState) -> Icon {
    let base = image::load_from_memory(TRAY_PNG).expect("tray png").to_rgba8();
    let (w, h) = base.dimensions();
    Icon::from_rgba(icon_rgba(state), w, h).expect("icon rgba")
}

/// Построить значок трея с меню и подписаться на события ОС — обработчики
/// шлют их в `tx`. Левый щелчок не открывает меню (оно только по правому).
/// `autostart_on`/`autoconnect_on` — начальные галочки, читает `launch`.
pub fn build(tx: UnboundedSender<Ev>, choice: theme::Choice, autostart_on: bool, autoconnect_on: bool) -> anyhow::Result<Tray> {
    #[cfg(target_os = "linux")]
    let open = MenuItem::with_id("open", "Открыть окно", true, None);
    let toggle = MenuItem::with_id("toggle", "Включить MCP", true, None);
    let check = MenuItem::with_id("check", "Проверить", true, None);
    // Отметка при создании меню: умолчание «только чтение», пока не
    // прочитано из toml — `build` не получает конфиг, в отличие от `choice`
    // (тот читает `theme::Choice::load` до вызова). `launch` поправляет её на
    // настоящий потолок сразу после `build`, до показа окна (`Tray::set_access`)
    // — так что галочка никогда не достаётся человеку неверной.
    let access_items = ACCESS.map(|(a, id, text)| CheckMenuItem::with_id(id, text, true, a == "read_only", None));
    let access_refs: Vec<&dyn IsMenuItem> = access_items.iter().map(|i| i as &dyn IsMenuItem).collect();
    let access_submenu = Submenu::with_items(ACCESS_SUBMENU, true, &access_refs)?;
    // Начальные галочка и доступность — оба `false`: `launch` поправляет их
    // сразу после `build` (`Tray::set_agents`), тем же приёмом, что у потолка
    // записи выше — здесь ни `Found`, ни режим ещё не читали.
    let agent_items = editors::AGENTS.map(|def| CheckMenuItem::with_id(agent_menu_id(def.id), def.label, false, false, None));
    let agent_refs: Vec<&dyn IsMenuItem> = agent_items.iter().map(|i| i as &dyn IsMenuItem).collect();
    let agents_submenu = Submenu::with_items(AGENTS_SUBMENU, true, &agent_refs)?;
    let items = THEMES.map(|(c, id, text)| CheckMenuItem::with_id(id, text, true, c == choice, None));
    let theme_refs: Vec<&dyn IsMenuItem> = items.iter().map(|i| i as &dyn IsMenuItem).collect();
    let themes_submenu = Submenu::with_items("Тема", true, &theme_refs)?;
    let autostart = CheckMenuItem::with_id("autostart", AUTOSTART_TEXT, true, autostart_on, None);
    let autoconnect = CheckMenuItem::with_id("autoconnect", "Подключаться при запуске", true, autoconnect_on, None);
    let quit = MenuItem::with_id("quit", "Выход", true, None);
    let menu = Menu::with_items(&[
        #[cfg(target_os = "linux")]
        &open,
        #[cfg(target_os = "linux")]
        &PredefinedMenuItem::separator(),
        &toggle,
        &check,
        &PredefinedMenuItem::separator(),
        &access_submenu,
        &agents_submenu,
        &themes_submenu,
        &autostart,
        &autoconnect,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    let tray = TrayIconBuilder::new()
        .with_icon(icon(TrayState::Off))
        .with_tooltip("1c-connect: не подключён")
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(false)
        .build()?;
    // Место раздела агентов считаем у готового меню, а не считаем пункты
    // глазами: на Linux впереди ещё два пункта, и число разъехалось бы с
    // кодом при первой же правке списка.
    let agents_pos = menu.items().iter().position(|k| k.id() == agents_submenu.id()).unwrap_or(menu.items().len());

    let t2 = tx.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = t2.unbounded_send(Ev::Tray(e));
    }));
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = tx.unbounded_send(Ev::Menu(e));
    }));

    let themes = [items[0].clone(), items[1].clone(), items[2].clone()];
    let access = [access_items[0].clone(), access_items[1].clone()];
    let agents = [agent_items[0].clone(), agent_items[1].clone(), agent_items[2].clone(), agent_items[3].clone()];
    Ok(Tray {
        icon: tray,
        toggle,
        access,
        agents,
        themes,
        autostart,
        autoconnect,
        menu,
        agents_submenu,
        agents_pos,
        agents_shown: std::cell::Cell::new(true),
    })
}

impl Tray {
    /// Значок badge(state) и надпись переключателя «Выключить MCP»/«Включить MCP».
    /// Подсказку `set_state` не трогает: `TrayState` одно на троих (занят,
    /// подключён, нет связи) и само по себе не знает фазу операции — её
    /// несёт только `tray_tooltip`, и каждый зовущий (`set_link`, `run_op`,
    /// `Ev::Done`, тик) сам зовёт `set_tooltip` следом тем же текстом, иначе
    /// занятость при поднятой связи осталась бы с «не подключён».
    /// `locked` — холодный замок заперт (`gui::gate::cold_gate`): пункт
    /// «Включить MCP» недоступен ровно тогда же, когда переключатель в окне,
    /// а причина едет в подсказку значка (`gui::tray_tooltip`).
    pub fn set_state(&self, state: TrayState, on: bool, locked: bool) {
        let _ = self.icon.set_icon(Some(icon(state)));
        self.toggle.set_text(if on { "Выключить MCP" } else { "Включить MCP" });
        self.toggle.set_enabled(toggle_enabled(locked, on));
    }

    /// Подсказка трея произвольным текстом (`tray_tooltip`) — единственный
    /// источник, который её меняет.
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.icon.set_tooltip(Some(text));
    }

    /// Отмечает ровно один пункт подменю «Тема».
    pub fn set_theme(&self, choice: theme::Choice) {
        for (item, (c, ..)) in self.themes.iter().zip(THEMES) {
            item.set_checked(c == choice);
        }
    }

    /// Отмечает ровно один пункт подменю «Запись в базу»: нативный пункт
    /// отмечает себя сам по щелчку, и вернуть отметку на действующее
    /// значение обязаны мы.
    pub fn set_access(&self, access: &str) {
        for (item, (a, ..)) in self.access.iter().zip(ACCESS) {
            item.set_checked(a == access);
        }
    }

    /// Галочки и доступность подменю «Локальные агенты» — зовётся при
    /// запуске и при каждой смене режима (решение владельца: доступность
    /// обязана обновляться на ходу, а не только при старте — режим можно
    /// сменить в карточке текущего соединения без перезапуска окна). Галочка стоит
    /// только на `Found::Ours` (`agent_checked`), доступность — агенты живут
    /// на этой машине (`agents_live_here`) и агент на машине вообще есть
    /// (`agent_available`): там, где агентов настраивает не наше меню (точка
    /// входа образа), и отсутствующего агента галочкой не включить. Состояние
    /// у всех четырёх читается с диска — у файловых агентов свой файл
    /// настроек, у CLI-агентов тот, в который пишет их CLI (`Setup::look`):
    /// цикл по реестру один, разница спрятана там. Опрос самих CLI отсюда
    /// убран — замер 30.10 дал 17–22 с замершего окна на один проход этого
    /// цикла (подробности у `editors::look_cli_agent`).
    ///
    /// Галочка отражает ОДНУ половину — записи MCP (решение владельца 31.10,
    /// повод 2): записи наши — галочка стоит, что бы сейчас ни лежало в доме
    /// навыков. Навыки не условие работы инструментов, и проблема с ними
    /// называется строкой журнала (`gui::skill_line`, `gui::refresh_line`), а
    /// не снятой галочкой. Та же половина, что у факта набора
    /// (`verify::cheap` → `local_editor_mcp_fact`): расходиться им нельзя.
    pub fn set_agents(&self, mode: &str) {
        // Раздел не гаснет серым, а исчезает — решение владельца 30.10,
        // увидевшего четыре серых пункта и спросившего «может скрывать это
        // меню в туннельном режиме?»; оно же было его изначальным «раздела
        // нет». Серый раздел ничего не сообщал: ни что агенты есть, ни чего с
        // ними делать, — а вопрос «почему серо» задавал каждому, кто его
        // открыл. Диск при этом не читаем вовсе: лишний поход за знанием,
        // которого всё равно не покажем.
        let live = editors::agents_live_here(mode);
        if live != self.agents_shown.get() {
            let changed = if live {
                // Вернуть на прежнее место, а не в конец: раздел стоит между
                // «Запись в базу» и «Темой», и переключение режима не
                // повод тасовать меню. Не вышло (меню короче, чем было) —
                // лучше в конец, чем нигде.
                self.menu.insert(&self.agents_submenu, self.agents_pos).or_else(|_| self.menu.append(&self.agents_submenu)).is_ok()
            } else {
                self.menu.remove(&self.agents_submenu).is_ok()
            };
            if changed {
                self.agents_shown.set(live);
            }
        }
        if !live {
            return;
        }
        let exe = std::env::current_exe().ok();
        for (item, def) in self.agents.iter().zip(editors::AGENTS.iter()) {
            let present = def.setup.present();
            let found = match &exe {
                Some(exe) => def.setup.look(exe),
                None => Ok(editors::Found::Missing),
            };
            item.set_checked(agent_checked(&found));
            item.set_enabled(agent_available(mode, present));
        }
    }
}

/// Галочка одного агента стоит только на `Found::Ours` — остальные
/// состояния (`Missing`/`Absent`/`Foreign`) из галочки неотличимы нарочно:
/// решение владельца — молчание про детали в меню, причина отказа идёт
/// отдельно, строкой в журнал, не текстом пункта.
///
/// Нечитаемое состояние (`Err` — файл агента есть, а разобрать его не вышло)
/// — тоже снятая галочка при доступном пункте: это не `Ours` и не `Absent`, и
/// меню тут выбирает молчать, а не красить. Отказ человек увидит словами при
/// щелчке — `setup_cli`/`teardown_cli` на нечитаемом состоянии CLI не зовут
/// вовсе и называют причину.
fn agent_checked(found: &Result<editors::Found, String>) -> bool {
    matches!(found, Ok(editors::Found::Ours))
}

/// Доступен ли пункт меню одного агента: агенты живут на этой машине
/// (`editors::agents_live_here` — один предикат на трей, на факт набора и на
/// тихую доставку навыков при запуске окна; счёт потребителей держать здесь
/// в согласии с его собственной докстрокой) и
/// агент на машине вообще есть — отсутствующего агента показываем
/// недоступным, а не прячем вовсе: набор агентов расширяется, и серый пункт —
/// единственный способ человеку узнать, что агент вообще поддерживается.
fn agent_available(mode: &str, present: bool) -> bool {
    editors::agents_live_here(mode) && present
}

/// Состояние значка по точке карточки: трей следует тому же порядку
/// разрешения (спека, «Трей живёт по тем же правилам»), а считать его дважды
/// незачем — порядок уже разобран в `panel::card`, значку остаётся перевести
/// точку в своё состояние. Отсюда и «при идущей операции побеждает операция»:
/// точка в этот момент уже `Dot::Busy`.
///
/// Провал вердикта — своё, четвёртое состояние значка (`TrayState::Warn`,
/// оранжевый бейдж): «не всё готово» нельзя спутать ни с «занято», ни с
/// «всё хорошо». Отказ (`Dot::Down`) показывается серым, как и выключенное
/// обслуживание: своего цвета у него нет и не было — причину человек читает
/// в подсказке значка.
pub fn tray_state_of(dot: super::Dot) -> TrayState {
    match dot {
        super::Dot::Up => TrayState::Up,
        super::Dot::Busy => TrayState::Busy,
        super::Dot::Warn => TrayState::Warn,
        super::Dot::Off | super::Dot::Down => TrayState::Off,
    }
}

/// Доступен ли пункт «Включить MCP»: трей живёт по тем же правилам, что окно
/// (спека, «Трей живёт по тем же правилам») — замок запирает только включение,
/// выключить обслуживание можно всегда. Обратная сторона `panel::switch_locked`
/// без хода операции: нативный пункт меню идущая операция не запирала и
/// раньше, её держит `mode_change_blocked` и сам `toggle_link`.
fn toggle_enabled(locked: bool, on: bool) -> bool {
    on || !locked
}

/// Операция, проверка или публикация идёт — тем же критерием, что блокирует
/// «Сохранить» (`panel::can_save`) и переключатель (`Switch::disabled`).
/// Смена режима посреди неё переписала бы конфигурацию и перезапустила
/// сеанс под её ногами — у смены режима этой защиты не было (находка
/// целостного ревью).
///
/// `ui.publishing.is_some()` — круг починки 5 задачи 16, ruling а: идущая
/// публикация — та же гонка, что идущая операция, только над httpd, а не
/// над toml, и дёргать текущее соединение (переключение, смена режима)
/// посреди неё нельзя — результат публикации, пришедший уже после
/// переключения, мог бы лечь на прежнюю базу. Один счёт на обе двери
/// (`request_current_mode_change`, `request_switch_current`), не два
/// похожих.
///
/// `pub(crate)` с круга починки 1 задачи 2 плана «Карточка публикации и
/// удаление» (Minor 5 ревью): `save::delete_blocked_reason` зовёт её же, а
/// не свою копию тройного `||` — второй счёт того же самого молча
/// разошёлся бы с этим, когда здесь появится четвёртое поле занятости.
pub(crate) fn mode_change_blocked(ui: &Ui) -> bool {
    ui.busy.is_some() || ui.progress.is_some() || ui.publishing.is_some()
}

/// Слово о том, что идёт прямо сейчас — для отказа «Сделать текущим»
/// словами (круг починки 5 задачи 16, ruling а: «отказ называет, что
/// именно идёт»). Зовётся только когда `mode_change_blocked` уже сказала
/// «да» — порядок проверки внутри тот же, что у неё: операция (есть
/// карточка хода) важнее проверки (занято без карточки хода), а публикация
/// проверяется последней — её одну можно начать и без операции/проверки.
pub(crate) fn busy_now_word(ui: &Ui) -> &'static str {
    if ui.progress.is_some() {
        "операция"
    } else if ui.busy.is_some() {
        "проверка"
    } else {
        "публикация базы"
    }
}

/// Отказ пункта «Запись в базу» на пустом списке соединений — чистое ядро
/// защиты от воскрешения карточки-фантома (находка ревью задачи 3):
/// `Cfg::current_mut()` на пустом списке заводит ПЕРВОЕ соединение по
/// умолчанию, а пункт меню значка, правящий ТЕКУЩИЕ параметры, не имеет
/// права создавать карточку, которой человек не заводил (`apply_access`
/// правит именно её). Слова — у `Cfg::no_connections_refusal()`, ОДНОГО
/// источника на оба отказа (Minor 1 круга починки 1 задачи 6: свой литерал
/// здесь развёлся бы с `Cfg::switch_current` при первой же правке
/// формулировки). `None` — список не пуст, пункт работает как обычно.
fn access_refused_on_empty_list(empty: bool) -> Option<String> {
    empty.then(crate::cfg::Cfg::no_connections_refusal)
}

/// Поднимать ли сеанс после смены режима. Замок к этому моменту уже
/// пересчитан (`save_host` → `run_readiness` → `refresh_gate`), и если он
/// защёлкнулся, обслуживание уже погашено.
///
/// В туннельном режиме сеанс и есть обслуживание: поднять туннель поверх
/// погашенного флага значило бы показать зелёное «Подключён», пока мост
/// отказывает каждому вызову — ровно находка S5 итогового ревью. В локальном
/// режиме слушатели держат порты всегда, независимо от замка и
/// переключателя: иначе агент получил бы отказ соединения вместо причины
/// (спека: «Пока замок заперт, агент получает отказ с причиной»).
fn raise_session_after_mode_change(mode: &str, gate: Option<&str>) -> bool {
    mode == "local" || gate.is_none()
}

/// Поднимать ли сеанс после перецикла (`recycle_session`) — MEDIUM ревью
/// круга починки 4 задачи 16: `raise_session_after_mode_change` одна не
/// видит выключатель, и переключение текущего при ВЫКЛЮЧЕННЫХ MCP поднимало
/// сеанс, обходя единственное, что человек может нажать без окна. Живой
/// сеанс можно перезапустить; выключенный обязан остаться выключенным —
/// `was_enabled` читается ДО перецикла (сам перецикл `enabled` не трогает,
/// так что «до» и «во время» здесь одно и то же значение).
fn should_raise_session(was_enabled: bool, mode: &str, gate: Option<&str>) -> bool {
    was_enabled && raise_session_after_mode_change(mode, gate)
}

/// Записать новый режим и поднять сеанс заново. Зовётся после согласия
/// человека (`confirm_database_change`), поэтому `ui.cfg.current().mode` правится
/// здесь, а не до вопроса: `save_host` берёт режим из текущих параметров и
/// иначе переписал бы его обратно.
fn apply_mode(cx: &mut App, mode: &'static str) {
    cx.global_mut::<Ui>().cfg.current_mut().mode = mode.into();
    // Критическая находка круга починки 1 задачи 16, другой конец: открытая
    // карточка держит СВОЙ снимок режима (`Panel::original`), и после
    // записи ниже его надо перевязать — тем же `sync_card_topology`, что
    // зовёт и переключатель «Это соединение» в карточке (`request_current_mode_change`).
    // Имя берём один раз сейчас, не после записи: при отказе (`Err`) ниже
    // имя текущего соединения не меняется, но брать его стоит из одного
    // места, а не дважды по разным путям match.
    let name = cx.global::<Ui>().cfg.current().name.clone();
    let host = cx.global::<Ui>().cfg.current().clone();
    match save_host(cx.global_mut::<Ui>(), host) {
        Ok(()) => {
            // Тревный значок, готовность/замок, гашение и подъём сеанса —
            // общий хвост с переключением текущего соединения (Critical
            // ревью круга починки 2 задачи 16, `recycle_session`).
            recycle_session(cx);
            let mut label = mode;
            for (m, text) in MODE_WORDS {
                if m == mode {
                    label = text;
                }
            }
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Ok, text: format!("режим работы: {}", label.to_lowercase()) });
        }
        Err(why) => {
            // Та же развязка, что у публикации, и по той же причине: выбор
            // режима — намерение человека, а не действие над базой, и на
            // свежем комплекте с пустыми полями он обязан сработать. Память
            // тут не годится: окно перечитывает файл на каждом показе.
            if let Err(e) = save_tray_key(cx.global_mut::<Ui>(), "mode", mode) {
                note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
                return;
            }
            let label = MODE_WORDS.iter().find(|(m, _)| *m == mode).map(|(_, t)| *t).unwrap_or(mode);
            note_line(cx.global_mut::<Ui>(), unsaved_switch_line("режим работы", label, why));
            if let Some(tray) = &cx.global::<Ui>().tray {
                tray.set_agents(mode);
            }
            cx.global_mut::<Ui>().tab = tab_for_unsaved_switch();
            window::show_window(cx);
        }
    }
    super::panel::sync_card_topology(cx, &name);
    cx.refresh_windows();
}

/// Перецикл сеанса на уже переписанных параметрах — общий хвост смены
/// режима (`apply_mode`) и переключения текущего соединения
/// (`apply_switch_current`, Critical ревью круга починки 2 задачи 16).
/// Пересчёт готовности и замка (`run_readiness`) — ПЕРВЫМ: от него зависит
/// и `ui.gate`, который читает `raise_session_after_mode_change` ниже, и
/// сама карточка состояния. Для смены режима это второй такой пересчёт
/// подряд (первый уже сделал `save_host` внутри себя) — цена заведомо
/// меньше третьего пути записи, а для переключения текущего он единственный
/// (`switch_current` сама по себе ничего не пересчитывает, это и есть
/// находка ревью).
///
/// Имя `raise_session_after_mode_change` осталось от режима, а смысл шире —
/// «можно ли обслуживать на только что записанных параметрах», и он
/// буквально годится переключению текущего: замок один на машину, не на
/// повод смены.
///
/// Подъём сеанса в конце — через `should_raise_session`, а не напрямую
/// через `raise_session_after_mode_change` (MEDIUM ревью круга починки 4):
/// выключенное обслуживание обязано остаться выключенным, и перецикл не
/// обходит переключатель.
fn recycle_session(cx: &mut App) {
    run_readiness(cx.global_mut::<Ui>());
    let mode = cx.global::<Ui>().cfg.current().mode.clone();
    if let Some(tray) = &cx.global::<Ui>().tray {
        // Доступность «Локальных агентов» зависит от режима — обязана
        // обновиться тут же (задача «Управление переехало в трей»).
        tray.set_agents(&mode);
    }
    // Параметры сеанса (база, логин, пароль, транспорт) — целиком
    // собственность ТЕКУЩЕГО соединения, и после любой правки старый сеанс
    // недействителен; `stop_session` безвредна, если сеанса и не было.
    stop_session(cx);
    let ui = cx.global::<Ui>();
    let was_enabled = ui.enabled.load(Ordering::Relaxed);
    if should_raise_session(was_enabled, &ui.cfg.current().mode, ui.gate.as_deref()) {
        if let Err(e) = start_session(cx.global_mut::<Ui>()) {
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
        }
    }
}

/// Три исхода нажатия «Сделать текущим» — чистое ядро `request_switch_current`,
/// под тестом без гарнеса (Critical ревью круга починки 2 задачи 16: тест
/// обязателен, а GPUI-гарнеса в дереве нет).
///
/// **Короткого пути «параметры совпали — не перецикливаем» здесь НЕТ** — в
/// отличие от `request_current_mode_change`, которая пропускает перецикл,
/// когда новый режим равен старому. Ровно это отсутствие и было находкой
/// ревью: параметры сеанса берутся из текущего соединения ЦЕЛИКОМ (база,
/// логин, пароль, папка, транспорт), и сравнивать здесь не с чем —
/// переключение само по себе и есть смена всех их разом, даже когда режим
/// у обеих карточек один.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SwitchCurrentOutcome {
    /// Идёт операция (`mode_change_blocked`) — дёргать текущее соединение
    /// посреди обмена с базой нельзя, вопрос не успевает.
    Blocked,
    /// То же основание, что у `save_needs_confirm` при «Сохранить» — один
    /// диалог на оба повода, не два текста об одном и том же.
    NeedsConfirm,
    /// Ничего не мешает — применяем сразу.
    Proceed,
}

pub(crate) fn switch_current_outcome(blocked: bool, needs_confirm: bool) -> SwitchCurrentOutcome {
    if blocked {
        SwitchCurrentOutcome::Blocked
    } else if needs_confirm {
        SwitchCurrentOutcome::NeedsConfirm
    } else {
        SwitchCurrentOutcome::Proceed
    }
}

/// Переключить текущее соединение — тем же путём, что смена режима
/// (Critical ревью круга починки 2 задачи 16): `gui::switch_current`
/// (`save.rs:758`) сама по себе — ОДИН ключ в toml и `ui.cfg`, не больше
/// (и остаётся им — см. её докстроку и тест границы в `save/tests.rs`), а
/// сеанс в окне один и до этого круга оставался поднятым с параметрами
/// ПРЕЖНЕГО текущего соединения: другая база, другой логин, другой
/// транспорт — агент продолжал бы молча отвечать не из той базы, которую
/// показывает окно.
///
/// Единственный вызывающий — `LinkAct::MakeCurrent`
/// (`panel.rs`, пункт меню «Сделать текущим»): `switch_current` сама по
/// себе больше не всё, что происходит при этом действии.
///
/// `false` — смена не состоялась из-за идущей операции; тем же смыслом,
/// что `request_current_mode_change`.
pub(crate) fn request_switch_current(cx: &mut App, name: &str) -> bool {
    let ui = cx.global::<Ui>();
    let blocked = mode_change_blocked(ui);
    // Имени может не быть в списке (опечатка, файл изменился руками, пока
    // окно было открыто) — у `switch_current` уже есть свой осмысленный
    // отказ словами, и спрашивать «переключить?» о карточке, которой нет,
    // незачем: вопрос относится к базе, а не к ошибке адресации.
    let Some(new_host) = ui.cfg.by_name(name).cloned() else {
        if blocked {
            return false;
        }
        apply_switch_current(cx, name);
        return true;
    };
    // Круг починки 3 задачи 16: механизм решения «спрашивать ли» —
    // `save_needs_confirm`/`save_question`, тот же, что у «Сохранить» и у
    // смены режима, но СЛОВА свои: человек нажал «Сделать текущим», не
    // «Сохранить», и вопрос про сохранение здесь был бы неправдой —
    // неправильный вопрос учит жать «да» не читая, это хуже отсутствия
    // вопроса.
    let old_path = ui.cfg.current().base_path.clone();
    match switch_current_outcome(blocked, save_needs_confirm(ui, &new_host)) {
        SwitchCurrentOutcome::Blocked => false,
        SwitchCurrentOutcome::Proceed => {
            apply_switch_current(cx, name);
            true
        }
        SwitchCurrentOutcome::NeedsConfirm => {
            if cx.global::<Ui>().window.is_none() {
                apply_switch_current(cx, name);
                return true;
            }
            window::show_window(cx);
            let Some(w) = cx.global::<Ui>().window else { return true };
            let owned = name.to_string();
            let (message, detail) = confirm_switch_current_text(name, &old_path, &new_host.base_path);
            let _ = w.update(cx, |_, window, cx| {
                confirm_database_change_with_text(
                    window,
                    cx,
                    &new_host,
                    &message,
                    &detail,
                    "Да, сделать текущим",
                    "Отмена",
                    move |_window, cx| apply_switch_current(cx, &owned),
                );
            });
            true
        }
    }
}

/// Записать новое текущее соединение, переустановить публикацию и перецикл
/// сеанса (`recycle_session`) — хвост `request_switch_current`, вызванный
/// либо сразу, либо из согласия диалога. Отказ `switch_current` (имя
/// исчезло между вопросом и ответом) — строка в журнал, дальше не идём:
/// нечего переключать.
///
/// **HIGH ревью круга починки 4 задачи 16.** `recycle_session` гасит и
/// поднимает сеанс, но публикации не касается — а httpd после переключения
/// продолжал бы публиковать ПРЕЖНЮЮ базу: сеанс новый, публикация ещё
/// старая, агент может получить данные не той базы, которую показывает
/// окно. Тем же путём, что `save_host` (`descriptor_changed` →
/// `republish_after_save` → `ensure_publication_now`), и своего второго
/// способа публиковать не заводим. Порядок — публикация РАНЬШЕ перецикла
/// сеанса, тот же, что в `save_host`: ребёнок моста пойдёт по
/// опубликованному адресу.
///
/// **Круг починки 5 задачи 16 — два ревьюера, одна находка.** Гашение
/// прежней публикации обязано смотреть на СТАРУЮ карточку: `ctx`, собранный
/// ПОСЛЕ `switch_current`, несёт уже НОВУЮ (`ui.cfg` переписан), а
/// `stop_own_publication` проверяет `publication != "webserver"` у той
/// карточки, что лежит в `ctx.cfg.current()`. Старая auto (наш httpd),
/// новая webserver — старым кодом `stop_own_publication` решал бы «нечего
/// гасить» по НОВОЙ карточке, признак сбрасывался безусловно, и процесс
/// сиротел до перезагрузки машины. Снимок — `old_cfg`, целый `Cfg` до
/// записи (не только `Connection`: `Ctx` несёт `Cfg`, и его `.current()`
/// должен указывать на старую карточку по имени, которое мы покидаем).
///
/// Признак `ui.raised_publication` сбрасывается ТОЛЬКО по удаче
/// (`Ok(true)`) — на отказе (`Err`) или на «и так не наша публикация»
/// (`Ok(false)`, не должно случаться при согласованном признаке, но не
/// повод учить признак забывать) он остаётся как был, чтобы выключение и
/// выход могли повторить попытку, а не унести забытый процесс в бессрочное
/// сиротство.
///
/// Гонку результата публикации, пришедшего уже после переключения,
/// отдельно не лечим: предусловие (`mode_change_blocked` с
/// `ui.publishing.is_some()`, ruling а) не даёт переключиться, пока
/// публикация идёт — значит поздний результат может прийти только от
/// публикации, начатой уже ПОСЛЕ переключения, и это законный случай, не
/// гонка.
///
/// Что делать в момент ПРИМЕНЕНИЯ «Сделать текущим» — отдельно от решения
/// `request_switch_current` (которое смотрит на предусловие ДО вопроса, а
/// вопрос нужен не всегда). Диалог не отнимает у окна фокус (та же спека, на
/// которой стоит `panel::delete_on_confirm`), и между вопросом и «Да,
/// сделать текущим» человек успевает запустить проверку, операцию или
/// публикацию — предусловие обязано быть пересчитано ЗАНОВО здесь, в момент
/// согласия, а не взято из того, что было верно до вопроса (отложенная
/// мелочь этой задачи, тот же довод, что Important 1 ревью круга починки 1
/// нашло у удаления карточки).
///
/// Тип, а не `bool`, тем же приёмом, что `DeleteOnConfirm`: запись
/// (`write_switch_current`) требует имя карточки, и это имя лежит ИМЕННО в
/// варианте `Proceed`, а не рядом, в параметре функции-обёртки — тем же
/// грузом, каким `DeleteOnConfirm::Proceed` несёт `stop_session_first`. Без
/// этого груза «запись недостижима до `Proceed`» было бы утверждением
/// докстроки, которое ничего не доказывает: `apply_switch_current` могла бы
/// игнорировать исход и писать по своему же параметру `name` прямо из ветки
/// `Blocked` — компилятор не возразил бы (круг починки 1 задачи 7,
/// Minor 2).
#[derive(Debug, PartialEq, Eq)]
enum ApplySwitchCurrent {
    /// Отказ словами — ни запись, ни перецикл сеанса не идут, `current`
    /// остаётся прежним.
    Blocked(String),
    /// Можно писать — имя карточки, на которую переключаемся, тем же
    /// именем, что спросили на входе (`apply_switch_current_decision`
    /// самой не вольно его придумать — оно только повторяет то, что дали).
    Proceed(String),
}

fn apply_switch_current_decision(ui: &Ui, name: &str) -> ApplySwitchCurrent {
    if mode_change_blocked(ui) {
        // Те же слова, что у отказа ДО вопроса (`panel::run_link_act`,
        // `LinkAct::MakeCurrent`) — находка начала этой же задачи: один
        // повод молчать об одном и том же двумя разными фразами незачем.
        ApplySwitchCurrent::Blocked(format!("Сейчас идёт {} — подождите её завершения и повторите «Сделать текущим».", busy_now_word(ui)))
    } else {
        ApplySwitchCurrent::Proceed(name.to_string())
    }
}

/// Запись и решение «что делать с публикацией» вынесены в `write_switch_current`
/// (берёт и возвращает только `Ui`, без `App`) — круг починки 5 задачи 16:
/// находку «`ctx` смотрит на новую карточку» без этого нельзя было бы
/// покрыть тестом вовсе (GPUI-гарнеса в дереве нет), а так под тестом —
/// именно тот `Ctx`, который эта функция передаст `stop_own_publication`.
fn apply_switch_current(cx: &mut App, name: &str) {
    // Отказ (`Blocked`) кончается здесь же, в своей ветке — записи
    // (`write_switch_current`) и перецикла сеанса в ней синтаксически нет:
    // их видно только внутри `Proceed`, и имя, которым она пишет, приходит
    // ИЗ НЕГО, а не из параметра этой функции — последний только донёс его
    // до решения, второй раз его в записи не читают.
    let name = match apply_switch_current_decision(cx.global::<Ui>(), name) {
        ApplySwitchCurrent::Blocked(why) => {
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: why });
            cx.refresh_windows();
            return;
        }
        ApplySwitchCurrent::Proceed(name) => name,
    };
    let written = match write_switch_current(cx.global_mut::<Ui>(), &name) {
        Ok(w) => w,
        Err(e) => {
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
            cx.refresh_windows();
            return;
        }
    };
    if written.what.stop_old {
        match crate::ops_host::stop_own_publication(&written.old_ctx) {
            Ok(true) => cx.global_mut::<Ui>().raised_publication = false,
            Ok(false) => {} // признак и ctx разошлись бы — не трогаем его, разберётся следующая попытка
            Err(e) => {
                note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Warn, text: format!("HTTP-сервис прежней базы не остановлен: {e:#}") });
            }
        }
    }
    if written.what.raise_new {
        ensure_publication_now(cx.global_mut::<Ui>());
    }
    recycle_session(cx);
    cx.refresh_windows();
}

/// Результат `write_switch_current`: `Ctx`, с которым обязан быть позван
/// `stop_own_publication` (несёт СТАРЫЙ `Cfg` — снимок до записи, см. ниже),
/// и решение, что делать с публикацией (`publication_after_switch`).
struct SwitchCurrentWrite {
    old_ctx: crate::ops_host::Ctx,
    what: PublicationAfterSwitch,
}

/// Переписать `current` (`gui::switch_current`) и решить, что делать с
/// публикацией — часть `apply_switch_current`, которой GPUI не нужен вовсе:
/// берёт и правит только `Ui`. Выделена специально ради теста (круг починки
/// 5 задачи 16, находка двух ревьюеров): без неё «`old_ctx` несёт СТАРУЮ
/// карточку, не новую» было бы утверждением в докстроке, которое никто не
/// проверяет.
///
/// `old_cfg` — снимок ВСЕГО `Cfg`, не только прежнего `Connection`: `Ctx`
/// несёт `Cfg` целиком, и `stop_own_publication` читает `ctx.cfg.current()`
/// — так что снимок обязан указывать на старую карточку по имени, которое
/// мы сейчас покидаем, а не только нести её поля отдельно.
fn write_switch_current(ui: &mut Ui, name: &str) -> Result<SwitchCurrentWrite, String> {
    let was_enabled = ui.enabled.load(Ordering::Relaxed);
    let old_cfg = ui.cfg.clone();
    let old_host = old_cfg.current().clone();
    let raised_publication = ui.raised_publication;
    switch_current(ui, name)?;
    let base_changed = descriptor_changed(ui.cfg.current(), &old_host);
    let what = publication_after_switch(base_changed, was_enabled, raised_publication);
    let old_ctx = super::quiet_ctx_with(ui, old_cfg);
    Ok(SwitchCurrentWrite { old_ctx, what })
}

/// Что сделать с публикацией после переключения текущего — чистое ядро
/// публикационной половины `apply_switch_current` (HIGH ревью круга
/// починки 4 задачи 16), под тестом без гарнеса: сама обвязка
/// (`ops_host::stop_own_publication`, `ensure_publication_now`) трогает
/// диск и сеть, а решение «нужно ли» от них не зависит.
///
/// `stop_old` — гасить прежний свой httpd БЕЗУСЛОВНО, как только база
/// сменилась (`base_changed`), независимо от `was_enabled`: он публикует
/// адрес, которого больше нет у текущего соединения, и оставлять его
/// висеть значило бы, что состояние публикации рапортует «готово» про
/// прежний адрес. `raise_new` — тем же правилом, что `republish_after_save`
/// у `save_host`, но со `was_enabled` на месте «обслуживания» (ruling круга
/// 4: переустанавливаем публикацию только когда MCP включены).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PublicationAfterSwitch {
    pub(crate) stop_old: bool,
    pub(crate) raise_new: bool,
}

fn publication_after_switch(base_changed: bool, was_enabled: bool, raised_publication: bool) -> PublicationAfterSwitch {
    PublicationAfterSwitch { stop_old: base_changed && raised_publication, raise_new: republish_after_save(base_changed, was_enabled) }
}

/// Сменить режим ТЕКУЩЕГО соединения — общая последовательность, которую
/// проходит смена режима из ЛЮБОГО места (Important 3 ревью круга починки 1
/// задачи 16): отказ под идущей операцией (`mode_change_blocked`), вопрос
/// при живом обслуживании тем же диалогом, что у «Сохранить»
/// (`save_needs_confirm`/`confirm_database_change`), запись и перецикл
/// сеанса (`apply_mode`). До этого круга её знал только трей, а
/// переключатель «Это соединение» в карточке писал `mode` точечно, в обход —
/// карточка меняла транспорт ТЕКУЩЕГО соединения, не трогая ни сеанс, ни
/// замок, ни значок.
///
/// Пункт меню «Режим работы», который раньше её звал, решением владельца
/// 1.11 убран вовсе («публикация — поле соединения, и пункт меню убран
/// вовсе»; тем же доводом раньше ушёл режим — переключатель в значке один на
/// машину, а соединений несколько) — единственный зовущий теперь карточка
/// окна (`panel::set_card_mode`), и тревного значка здесь больше нет вовсе.
///
/// `false` — смена не состоялась из-за идущей операции; карточка сообщает об
/// этом строкой в журнал.
pub(crate) fn request_current_mode_change(cx: &mut App, mode: &'static str) -> bool {
    let ui = cx.global::<Ui>();
    if mode_change_blocked(ui) {
        return false;
    }
    let current = ui.cfg.current().mode.clone();
    if mode == current {
        return true;
    }
    let mut host = ui.cfg.current().clone();
    host.mode = mode.into();
    // Спрашивать не о чем (или спросить негде — окна сейчас нет) — пишем
    // сразу.
    if !save_needs_confirm(ui, &host) || ui.window.is_none() {
        apply_mode(cx, mode);
        return true;
    }
    window::show_window(cx);
    let Some(w) = cx.global::<Ui>().window else { return true };
    let _ = w.update(cx, |_, window, cx| {
        confirm_database_change(window, cx, &host, move |_window, cx| apply_mode(cx, mode));
    });
    true
}

/// Сменить публикацию ТЕКУЩЕГО соединения — та же общая последовательность и в
/// том же порядке, что у `request_current_mode_change` (задача 5 плана
/// «Карточка публикации и удаление»): отказ под идущей операцией
/// (`mode_change_blocked` — ОДИН счёт на все двери, а не свой у каждой), вопрос
/// при живом обслуживании тем же диалогом, что у «Сохранить»
/// (`save_needs_confirm`/`confirm_database_change`), запись и перецикл сеанса
/// (`apply_publication`).
///
/// **Почему у текущей карточки не точечная запись.** Публикация — такая же
/// ось, как режим: уходя в «публикует платформа 1С», мы гасим СВОЙ httpd и
/// убираем свой дескриптор, а это действие над живой базой, не намерение в
/// файле. Точечная запись (`save::set_publication`) оставила бы свой httpd
/// висеть на адресе, которого у соединения больше нет.
///
/// Своей второй копии уборки публикации здесь нет: её делает
/// `apply_publication` — и делает ПОСЛЕ согласия человека (ответил «нет» на
/// вопрос о смене базы — ничего не тронуто).
///
/// `false` — смена не состоялась из-за идущей операции; вызывающий решает, как
/// об этом сказать (карточка — строкой в журнал словами `busy_now_word`).
pub(crate) fn request_current_publication_change(cx: &mut App, publication: &'static str) -> bool {
    let ui = cx.global::<Ui>();
    if mode_change_blocked(ui) {
        return false;
    }
    let current = ui.cfg.current().publication.clone();
    if publication == current {
        return true;
    }
    let mut host = ui.cfg.current().clone();
    host.publication = publication.into();
    // Спрашивать не о чем (или спросить негде — окна сейчас нет) — пишем
    // сразу, тем же приёмом, что у режима выше.
    if !save_needs_confirm(ui, &host) || ui.window.is_none() {
        apply_publication(cx, publication);
        return true;
    }
    window::show_window(cx);
    let Some(w) = cx.global::<Ui>().window else { return true };
    let _ = w.update(cx, |_, window, cx| {
        confirm_database_change(window, cx, &host, move |_window, cx| apply_publication(cx, publication));
    });
    true
}

/// Что ложится в файл, когда `save_host` отказал на смене публикации
/// (Important 1 круга починки 1 задачи 5): публикация — всегда, а при снятом
/// нами адресе ещё и пустой `base_url`, ОДНОЙ записью (`save::save_tray_keys`).
///
/// **Почему пара, а не один ключ.** Выведенный от каталога базы адрес снимается
/// в памяти (см. `apply_publication`), и если на диск ушла бы только
/// публикация, файл остался бы с «webserver» и нашей же догадкой в `base_url`:
/// замок про неё молчит (поле непустое — `Cfg::publication_address_missing`),
/// `Cfg::base_url()` отдаёт догадку, и проба уходит на ЧУЖОЙ веб-сервер на 80.
/// Окно при следующем показе перечитывает файл и возвращает догадку в память —
/// то есть уборка в памяти без этой пары не держалась бы и минуты.
///
/// Решение вынесено в чистую функцию, чтобы держать его под тестом: саму
/// `apply_publication` без живого `App` не позвать (под тестом —
/// `a_refused_save_writes_the_publication_together_with_the_dropped_address`).
fn unsaved_publication_keys(publication: &'static str, dropped_address: bool) -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![("publication", publication)];
    if dropped_address {
        keys.push(("base_url", ""));
    }
    keys
}

/// Строка журнала, когда режим переключён, но в файл не записан. Применить
/// его мы вправе: это намерение человека, а не действие над базой —
/// публикацию здесь никто не поднимает, и связь при этом выключена. А вот
/// записать нечего, пока параметры не заполнены: `save_host` пишет весь
/// файл и проверяет его целиком (одна запись, один сторож — второго пути в
/// toml не заводим).
///
/// Поэтому `Warn`, а не `Fail`: сделано не всё, но сделано. И сказано, что
/// осталось, — иначе человек узнает о потере режима только после
/// перезапуска.
fn unsaved_switch_line(what: &str, label: &str, why: String) -> Line {
    Line {
        kind: Kind::Warn, text: format!("{what}: {label}. Параметры заполнены не до конца — допишите их и нажмите «Сохранить»: {why}")
    }
}

/// Записать, кто публикует базу, и поднять сеанс заново — та же форма, что у
/// `apply_mode`, и по той же причине: `save_host` берёт режим публикации из
/// текущих параметров, поэтому правится он здесь, после согласия человека.
///
/// Свой httpd и свой дескриптор к этому моменту уже убраны
/// (`ops_host::leave_own_publication` в обработчике): уборка идёт ДО записи,
/// иначе она увидела бы в параметрах «публикует веб-сервер» и вышла бы, не
/// тронув ничего.
///
/// **Открытая карточка перевязывается в конце — тем же
/// `sync_card_topology`, что у `apply_mode`** (Critical ревью всей ветки):
/// публикация — такая же ось состава полей карточки, как режим
/// (`panel::shown_fields`), и без перевязки карточка не отращивала «Адрес
/// публикации», хотя файл и `ui.cfg` уже были верны.
fn apply_publication(cx: &mut App, publication: &'static str) {
    // Имя — ДО записи, как в `apply_mode`, и из одного места: при отказе
    // текущее соединение не меняется, но брать его дважды по разным ветвям
    // match не за чем.
    let name = cx.global::<Ui>().cfg.current().name.clone();
    // Сняли ли мы выведенный нами адрес — нужно и ветви отказа ниже
    // (Important 1 круга починки 1: там это ВТОРОЙ ключ записи, иначе догадка
    // остаётся на диске), поэтому решение берётся здесь, до правки памяти, и
    // живёт одним признаком, а не считается заново по уже изменённому.
    let mut dropped_address = false;
    // Уходя в чужую публикацию, забираем своё: гасим свой httpd и убираем
    // свой дескриптор. **До** записи режима — после
    // неё уборка увидела бы «публикует веб-сервер» и не тронула бы ничего
    // (`leave_own_publication`), — и только здесь, после согласия человека:
    // ответил «нет» на вопрос о смене — ничего не тронуто.
    if publication == "webserver" {
        let ctx = super::ctx_for(cx.global::<Ui>());
        if let Err(e) = crate::ops_host::leave_own_publication(&ctx) {
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: format!("{e:#}") });
            return;
        }
        let ui = cx.global_mut::<Ui>();
        ui.raised_publication = false;
        // Выведенный от каталога базы адрес в toml написали мы, а не человек
        // (`carried_base_url` в обычном режиме). Принять его за ответ нельзя:
        // в этом режиме он ничего не вписывал, а проба по нашей догадке ушла
        // бы на чужой веб-сервер. Снимаем — и замок окна честно скажет
        // «адрес не задан».
        if crate::cfg::is_derived_address(ui.cfg.current()) {
            ui.cfg.current_mut().base_url = String::new();
            dropped_address = true;
        }
    }
    cx.global_mut::<Ui>().cfg.current_mut().publication = publication.into();
    let host = cx.global::<Ui>().cfg.current().clone();
    match save_host(cx.global_mut::<Ui>(), host) {
        Ok(()) => {
            stop_session(cx);
            let ui = cx.global::<Ui>();
            if raise_session_after_mode_change(&ui.cfg.current().mode, ui.gate.as_deref()) {
                if let Err(e) = start_session(cx.global_mut::<Ui>()) {
                    note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
                }
            }
            note_line(
                cx.global_mut::<Ui>(),
                Line { kind: Kind::Ok, text: format!("публикация: {}", link::publication_label(publication).to_lowercase()) },
            );
        }
        Err(why) => {
            // Проверка всего файла не прошла — на свежем комплекте поля ещё
            // пусты. Для переключателя это не повод не сработать: он не
            // действие над базой, а намерение человека. Пишем свои ключи
            // (`save_tray_keys`), остальное в файле остаётся как было.
            //
            // Именно в файл, а не в память: окно перечитывает файл на каждом
            // показе, и намерение, оставленное в памяти, стирается первым же
            // открытием окна — поле «Адрес публикации» тогда не появляется
            // вовсе (находка владельца 28.10). Снятый адрес — ТОТ ЖЕ случай и
            // тот же довод (Important 1 круга починки 1), поэтому он идёт
            // второй половиной той же пары, а не остаётся только в памяти.
            let keys = unsaved_publication_keys(publication, dropped_address);
            if let Err(e) = save_tray_keys(cx.global_mut::<Ui>(), &keys) {
                note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
                return;
            }
            note_line(cx.global_mut::<Ui>(), unsaved_switch_line("публикация базы", link::publication_label(publication), why));
            // Слова про снятый адрес — отдельной строкой, и только когда он
            // правда снят: человек обязан узнать, что в файле изменилось не
            // одно поле, и почему (адрес писали мы, а не он).
            if dropped_address {
                note_line(
                    cx.global_mut::<Ui>(),
                    Line {
                        kind: Kind::Warn,
                        text: "адрес публикации снят: прежний посчитала сама программа, а не вписали вы — впишите адрес, по которому \
                               база уже опубликована"
                            .into(),
                    },
                );
            }
            // И открываем то, что человеку осталось сделать: «Связь».
            // В режиме «публикует платформа 1С» там же появляется поле
            // «Адрес публикации», которое этот режим и требует, — иначе до
            // него было не добраться вовсе.
            cx.global_mut::<Ui>().tab = tab_for_unsaved_switch();
            window::show_window(cx);
        }
    }
    // Обе ветви — и удачная запись, и `save_tray_key` — оставляют в `ui.cfg`
    // новую публикацию, значит и снимок открытой карточки обязан её увидеть
    // (см. докстроку). Тот же порядок, что у `apply_mode`: перевязка
    // последней, перед перерисовкой.
    super::panel::sync_card_topology(cx, &name);
    cx.refresh_windows();
}

/// Записать потолок записи и сообщить об этом уже работающему ребёнку — без
/// перезапуска сеанса (в отличие от `apply_mode`/`apply_publication` выше):
/// потолок не меняет транспорт, только флаг у `mcp_command` на канале
/// `mcp-1c`, а механизм донести его без разрыва соединения с агентом уже
/// есть — `Session::update_cfg` кладёт новый `Cfg` в общую ячейку
/// (`rules.live`), и действующий ребёнок пересобирается на ней же на
/// следующий запрос (`Serve::dispatch`, `EpochEnd::Restart`). Вопроса
/// человеку здесь нет: это не смена базы (`save_needs_confirm` на одном
/// только `write` и так молчит), а независимая настройка потолка.
///
/// **Область действия — только канал `mcp-1c`.** Канал `vrunner` этим
/// потолком не ограничен: у него свой список инструментов
/// (`Rules::for_vrunner`), среди них есть пишущие в базу (например,
/// загрузка конфигурации), и подпись «Запись в базу» в меню не обещает
/// большего — решение, закрывать ли и этот канал потолком, за владельцем
/// (находка проверки безопасности на `c22b058`: своего списка пишущих имён
/// `vrunner` здесь нет и заводить его в этой задаче не просили).
///
/// **Действует на вызовы ПОСЛЕ смены, а не на уже идущий.** Пересборка
/// происходит, когда у моста нет вызова без ответа (`bridge.rs`,
/// `in_flight == 0` в `pipe_jsonrpc`) — начатый вызов доигрывает со старым
/// ребёнком, как и начинался; только следующий застаёт пересобранного.
/// Иначе быть не может: ответ ребёнка, уже начавшего работу, нельзя
/// отозвать на середине.
/// Текст отказа, когда потолок НЕ применён — запись в файл не удалась.
/// Поправка ведущего к кругу исправления 4 (вторая, окончательная для этой
/// ветки): первая версия оставляла потолок применённым только в живом
/// сеансе, но показ окна тут же (`window::show_window`, ниже) перечитывает
/// файл и откатывает его — фраза «применено к сеансу» оказывалась ложью
/// уже в момент произнесения, а трей показывал бы защиту, которой нет.
/// Второй источник истины (переменная-перекрытие рядом с файлом) снял бы
/// эту ложь, но завёл бы ровно то расхождение двух источников одного
/// значения, с которым вся эта ночь правок и боролась (вывод/выбор порта,
/// живая ячейка/файл, pid/личность процесса) — решение ведущего: не
/// плодить его снова. Один источник (файл), честный отказ, видимый
/// индикатор (трей читает факт после перечитывания — см. `apply_access`).
/// Отдельная функция — чтобы текст проверялся тестом без `cx`.
fn access_not_applied_line(e: &str) -> String {
    format!("запись в базу (mcp-1c): потолок НЕ применён — запись в файл не удалась: {e}")
}

/// Записать потолок записи ТЕКУЩЕГО соединения — единственный зовущий,
/// `on_menu::MenuAction::Access` (пункт «Запись в базу»).
///
/// **Страж на пустом списке соединений — находка ревью задачи 3, Important 1
/// и Minor 2 круга починки 1 задачи 6.** `Cfg::current_mut()` ниже на
/// пустом списке (свежий комплект или комплект после удаления последней
/// карточки) завела бы ПЕРВОЕ соединение по умолчанию — карточку-фантом,
/// которой человек не заводил. Страж стоит здесь, у защищаемого действия, а
/// не в `on_menu`: сегодня у функции один зовущий, и дырки нет, но второй
/// зовущий без стража открыл бы её снова (тот же способ, которым в
/// соседних задачах заводились «вторые двери»). Отказ идёт журналом И
/// показом окна (`tab_for_unsaved_switch`/`window::show_window`), тем же
/// путём, что и ветвь отказа записи ниже: значок живёт и без окна, и без
/// показа человек слов отказа не увидел бы вовсе.
fn apply_access(cx: &mut App, access: &'static str) {
    let empty = cx.global::<Ui>().cfg.connections.is_empty();
    if let Some(why) = access_refused_on_empty_list(empty) {
        note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: why });
        cx.global_mut::<Ui>().tab = tab_for_unsaved_switch();
        window::show_window(cx);
        let current = cx.global::<Ui>().cfg.access().to_string();
        if let Some(tray) = &cx.global::<Ui>().tray {
            tray.set_access(&current);
        }
        cx.refresh_windows();
        return;
    }
    cx.global_mut::<Ui>().cfg.current_mut().write = access.into();
    let host = cx.global::<Ui>().cfg.current().clone();
    match save_host(cx.global_mut::<Ui>(), host) {
        Ok(()) => {
            if let Some(tray) = &cx.global::<Ui>().tray {
                tray.set_access(access);
            }
            // `update_cfg` не повредит и в туннельном режиме (её же
            // докстрока) — отдельной ветки по режиму, в отличие от
            // `save_host`, здесь не нужно. Весь `ui.cfg` годен (`save_host`
            // только что это проверил целиком) — толкаем его как есть.
            if let Some(s) = &cx.global::<Ui>().session {
                s.update_cfg(cx.global::<Ui>().cfg.clone());
            }
            let mut label = access;
            for (a, _, text) in ACCESS {
                if a == access {
                    label = text;
                }
            }
            note_line(
                cx.global_mut::<Ui>(),
                Line {
                    kind: Kind::Ok,
                    text: format!(
                        "запись в базу (mcp-1c): {} — действует на вызовы mcp-1c после смены; канал vrunner этим не ограничен",
                        label.to_lowercase()
                    ),
                },
            );
        }
        Err(why) => {
            // Честный отказ, не живое ужесточение (финальное решение круга
            // исправления 4, поправка ведущего — см. докстроку
            // `access_not_applied_line`). Сеанс здесь не трогаем вовсе:
            // показ окна ниже перечитает файл сам, и если ключ лёг
            // (`save_tray_key` — Ok), он дойдёт до сеанса тем же путём, что
            // и любая другая правка toml руками (`adopt_cfg`); не лёг — не
            // дойдёт, и журнал обязан сказать это прямо, не создавая
            // видимость защиты.
            match save_tray_key(cx.global_mut::<Ui>(), "write", access) {
                Ok(()) => {
                    // Запись одного ключа — намерение человека, а не
                    // действие над базой (та же развязка, что у режима и
                    // публикации): на свежем комплекте с пустыми полями
                    // она обязана сработать даже при негодных остальных.
                    let label = ACCESS.iter().find(|(a, ..)| *a == access).map(|(_, _, t)| *t).unwrap_or(access);
                    note_line(cx.global_mut::<Ui>(), unsaved_switch_line("запись в базу", label, why));
                }
                Err(e) => {
                    note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: access_not_applied_line(&e) });
                }
            }
            cx.global_mut::<Ui>().tab = tab_for_unsaved_switch();
            window::show_window(cx);
            // Трей отражает ФАКТИЧЕСКИЙ потолок — то, что реально лежит в
            // `ui.cfg` после перечитывания файла показом окна выше
            // (`adopt_cfg`), не то, что человек нажал: иначе галочка
            // обещала бы защиту, которой может не быть.
            let factual = cx.global::<Ui>().cfg.access().to_string();
            if let Some(tray) = &cx.global::<Ui>().tray {
                tray.set_access(&factual);
            }
        }
    }
    cx.refresh_windows();
}

/// Левый щелчок (отжатие кнопки) по значку — показать окно (открыть или
/// поднять уже открытое на передний план); закрывает окно только явное
/// действие человека в самом окне, не трей. Меню — только по правому, сюда
/// не попадает.
pub fn on_tray(ev: TrayIconEvent, cx: &mut App) {
    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
        window::show_window(cx);
    }
}

pub fn on_menu(ev: MenuEvent, cx: &mut App) {
    let Some(action) = menu_action(ev.id.0.as_str()) else { return };
    match action {
        #[cfg(target_os = "linux")]
        MenuAction::Open => window::show_window(cx),
        MenuAction::Toggle => toggle_link(cx),
        MenuAction::Check => {
            window::show_window(cx);
            cx.global_mut::<Ui>().tab = Tab::Log;
            // Пока проверка (или установка) идёт, пункт меню только
            // показывает вкладку: второй прогон поверх первого путал бы
            // `busy` и писал бы отчёт дважды.
            if cx.global::<Ui>().busy.is_none() {
                run_check(cx);
            } else {
                cx.refresh_windows();
            }
        }
        MenuAction::Access(access) => {
            // Без вопроса человеку: потолок не действие над базой и не
            // меняет транспорт (см. `apply_access`), спрашивать не о чем.
            // Отказ на пустом списке соединений (находка ревью задачи 3) —
            // не здесь: страж стоит в самой `apply_access` (Minor 2 круга
            // починки 1 задачи 6), у защищаемого действия, а не у
            // вызывающего — второй вызывающий иначе открыл бы ту же дверь
            // без присмотра.
            let ui = cx.global::<Ui>();
            let current = ui.cfg.access().to_string();
            if mode_change_blocked(ui) || access == current {
                if let Some(tray) = &ui.tray {
                    tray.set_access(&current);
                }
                return;
            }
            apply_access(cx, access);
        }
        // Независимая галочка, не радио-группа: нативный пункт уже
        // переключил сам себя, и `is_checked()` здесь читает то, что он
        // только что выставил, — тот же приём, что у
        // «Автозапуска»/«Автоподключения» ниже.
        //
        // `false` от `toggle_agent` теперь значит «щелчок не приняли вовсе»
        // (по этому агенту уже идёт работа), и только тогда галочку возвращаем
        // здесь же. Принятый щелчок уходит в свой поток, и его итог известен
        // позже (`Ev::Agent`) — галочку в согласие с файлом агента приводит
        // не этот код, а перестройка подменю по тому событию
        // (`Tray::set_agents`); возвращать её здесь было бы гаданием об итоге,
        // которого ещё нет.
        MenuAction::Agent(which) => {
            let ui = cx.global::<Ui>();
            let Some(i) = editors::AGENTS.iter().position(|d| d.id == which) else { return };
            let Some(on) = ui.tray.as_ref().map(|t| t.agents[i].is_checked()) else { return };
            if !toggle_agent(cx, i, on) {
                if let Some(t) = &cx.global::<Ui>().tray {
                    t.agents[i].set_checked(!on);
                }
            }
        }
        MenuAction::Theme(c) => {
            let kit = cx.global::<Ui>().kit.clone();
            if let Err(e) = c.store(&kit) {
                note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: format!("{e:#}") });
            }
            theme::apply(c, cx);
            if let Some(tray) = &cx.global::<Ui>().tray {
                tray.set_theme(c);
            }
        }
        MenuAction::Autostart => {
            let ui = cx.global::<Ui>();
            let Some(on) = ui.tray.as_ref().map(|t| t.autostart.is_checked()) else { return };
            if let Err(e) = autostart::set(on) {
                if let Some(t) = &cx.global::<Ui>().tray {
                    t.autostart.set_checked(!on);
                }
                note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: format!("автозапуск: {e:#}") });
            }
        }
        MenuAction::Autoconnect => {
            let ui = cx.global::<Ui>();
            let Some(on) = ui.tray.as_ref().map(|t| t.autoconnect.is_checked()) else { return };
            let kit = ui.kit.clone();
            if let Err(e) = kit.state_set("autoconnect", if on { "1" } else { "0" }) {
                if let Some(t) = &cx.global::<Ui>().tray {
                    t.autoconnect.set_checked(!on);
                }
                note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: format!("автоподключение: {e:#}") });
            }
        }
        MenuAction::Quit => quit(cx),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        access_not_applied_line, access_refused_on_empty_list, agent_available, agent_checked, agent_menu_id,
        apply_switch_current_decision, busy_now_word, icon_rgba, menu_action, mode_change_blocked, publication_after_switch,
        raise_session_after_mode_change, save_host, should_raise_session, switch_current_outcome, toggle_enabled, unsaved_publication_keys,
        unsaved_switch_line, write_switch_current, ApplySwitchCurrent, Kind, MenuAction, PublicationAfterSwitch, SwitchCurrentOutcome,
        TrayState, Ui, ACCESS, ACCESS_SUBMENU, AGENTS_SUBMENU,
    };
    use crate::cfg::Connection;
    use crate::editors;
    use crate::gui::theme;
    use crate::gui::{Busy, Running, Which, TRAY_PNG};
    use common::Phase;
    use std::time::Instant;
    use tray_icon::menu::{CheckMenuItem, IsMenuItem, Submenu};

    #[test]
    fn menu_ids_map_to_actions_and_unknown_ids_to_nothing() {
        assert_eq!(menu_action("toggle"), Some(MenuAction::Toggle));
        assert_eq!(menu_action("quit"), Some(MenuAction::Quit));
        assert_eq!(menu_action("theme:dark"), Some(MenuAction::Theme(theme::Choice::Dark)));
        assert_eq!(menu_action("theme:system"), Some(MenuAction::Theme(theme::Choice::System)));
        assert_eq!(menu_action("autostart"), Some(MenuAction::Autostart));
        assert_eq!(menu_action("autoconnect"), Some(MenuAction::Autoconnect));
        assert_eq!(menu_action("nope"), None);
    }

    /// Решение владельца 1.11: публикация — поле соединения, режим — тоже
    /// (тем же доводом раньше ушёл он), и пункт меню убран вовсе — проверяем,
    /// что `menu_action` больше не узнаёт id этих пунктов.
    #[test]
    fn the_tray_knows_nothing_about_mode_and_publication_items_any_more() {
        assert_eq!(menu_action("mode:tunnel"), None);
        assert_eq!(menu_action("mode:local"), None);
        assert_eq!(menu_action("publication:auto"), None);
        assert_eq!(menu_action("publication:webserver"), None);
    }

    #[test]
    fn a_switch_that_could_not_be_saved_still_happened() {
        // Режим публикации — намерение человека, а не действие над базой:
        // публикацию здесь никто не поднимает. Поэтому на незаполненных
        // параметрах он применяется, а человек читает, что осталось сделать
        // (находка владельца 28.10: «ничего не происходит» на комплекте, где
        // поля ещё не заполнены).
        let l = unsaved_switch_line("публикация базы", "Публикует платформа 1С", "[host]: workstation — dev1, dev2 или dev3".into());
        assert_eq!(l.kind, Kind::Warn);
        assert_eq!(
            l.text,
            "публикация базы: Публикует платформа 1С. Параметры заполнены не до конца — допишите их и нажмите «Сохранить»: [host]: workstation — dev1, dev2 или dev3"
        );
        // Та же строка служит и режиму работы: дыра у них общая.
        let m = unsaved_switch_line("режим работы", "Локально", "[host]: base_path — путь к базе".into());
        assert!(m.text.starts_with("режим работы: Локально. Параметры заполнены не до конца"), "{}", m.text);
    }

    /// Значения toml — слова, что понимает ребёнок (`read_only`/`read_write`),
    /// подписи — человеческие: человек читает название, а не имя режима.
    #[test]
    fn the_access_submenu_speaks_in_human_words() {
        assert_eq!(menu_action("access:read_only"), Some(MenuAction::Access("read_only")));
        assert_eq!(menu_action("access:read_write"), Some(MenuAction::Access("read_write")));
        assert_eq!(menu_action("access:read"), None);
        let labels: Vec<&str> = ACCESS.iter().map(|(_, _, text)| *text).collect();
        assert_eq!(labels, vec!["Только чтение", "Чтение и запись"]);
        assert_eq!(ACCESS.map(|(v, ..)| v), ["read_only", "read_write"]);
        assert_eq!(ACCESS_SUBMENU, "Запись в базу");
    }

    /// Отмечен ровно умолчальный пункт, «Только чтение».
    #[test]
    fn access_submenu_has_two_checked_items() {
        let items = ACCESS.map(|(a, id, text)| CheckMenuItem::with_id(id, text, true, a == "read_only", None));
        let refs: Vec<&dyn IsMenuItem> = items.iter().map(|i| i as &dyn IsMenuItem).collect();
        let submenu = Submenu::with_items(ACCESS_SUBMENU, true, &refs).unwrap();
        assert_eq!(submenu.text(), "Запись в базу");
        assert_eq!(submenu.items().len(), 2);
        assert_eq!(items[0].text(), "Только чтение");
        assert_eq!(items[1].text(), "Чтение и запись");
        assert!(items[0].is_checked());
        assert!(!items[1].is_checked());
    }

    /// Тот же предел, что у `choosing_a_mode_writes_the_toml_and_restarts_the_session`:
    /// обработчик меню нуждается в `cx: &mut App` (GPUI, без headless-
    /// гарнеса здесь) — тестируем то, что можно без него: запись потолка
    /// через `save_host`, от которой зависит пересборка `mcp_command`
    /// (шаг 6 брифа — там же проверено, что пересборка несёт новый
    /// `--access`).
    #[test]
    fn choosing_an_access_ceiling_writes_the_toml() {
        let mut ui = Ui::for_test();
        assert_eq!(ui.cfg.access(), "read_only");
        ui.cfg.current_mut().write = "read_write".into();
        let host = ui.cfg.current().clone();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.cfg.access(), "read_write");
        let text = std::fs::read_to_string(ui.kit.state_path(crate::cfg::FILE)).unwrap();
        assert!(text.contains("write = \"read_write\""), "{text}");
    }

    /// Находка ревью задачи 3: пункт меню «Запись в базу» на пустом списке
    /// соединений обязан отказать словами `Cfg::no_connections_refusal()` —
    /// тем же источником, что читает и `Cfg::switch_current` (Minor 1 круга
    /// починки 1 задачи 6: сравниваем с живым источником, а не с
    /// замороженной копией его слов, иначе правка формулировки там не
    /// уронила бы этот тест здесь), — а не дойти до `Cfg::current_mut()` и
    /// завести карточку, которой человек не заводил. Страж живёт в
    /// `apply_access` (Minor 2), недостижимой без живого `App` (GPUI, без
    /// headless-гарнеса здесь) — под тестом чистое ядро решения, которое она
    /// зовёт.
    #[test]
    fn access_is_refused_on_an_empty_list_with_the_same_words_as_switch_current() {
        assert_eq!(access_refused_on_empty_list(true), Some(crate::cfg::Cfg::no_connections_refusal()));
        assert_eq!(access_refused_on_empty_list(false), None, "список не пуст — пункт работает как обычно");
    }

    /// Окно отказывает на пути с `..` и НИЧЕГО не пишет. Своей проверки у окна для
    /// этого нет и не нужно: «Сохранить» зовёт `cfg.validate()` (`save_host`), и
    /// правило живёт в одном месте (`cfg::has_parent_component`). Тест стоит здесь
    /// именно потому, что это свойство структурное: вынеси кто-нибудь `validate()` из
    /// `save_host` — и окно начнёт молча писать путь, который обходит защиту «папка
    /// принадлежит одному соединению». Проверено обратным ходом: со снятой проверкой
    /// в `cfg.rs` этот тест падает.
    #[test]
    fn the_window_refuses_a_folder_with_a_parent_component_and_writes_nothing() {
        let mut ui = Ui::for_test();
        let before = std::fs::read_to_string(ui.kit.state_path(crate::cfg::FILE)).unwrap_or_default();
        let mut host = ui.cfg.current().clone();
        host.project = r"C:\work\Buh\..\Buh2".into();
        let e = save_host(&mut ui, host).unwrap_err();
        assert!(e.contains(".."), "{e}");
        assert_eq!(
            std::fs::read_to_string(ui.kit.state_path(crate::cfg::FILE)).unwrap_or_default(),
            before,
            "отказ не должен ничего записывать"
        );
    }

    /// Круг исправления задачи 3 (проверка безопасности на `c22b058`):
    /// ужесточение потолка не должно зависеть от того, годны ли остальные
    /// поля `ui.cfg` — иначе переключатель показал бы «Только чтение», пока
    /// работающий ребёнок остаётся на `read_write`, и это молчаливый
    /// неверный результат, худший из возможных. `Cfg::with_access` меняет
    /// ровно одно поле на уже годном `Cfg` (том, что лежит в живой ячейке
    /// сеанса), не трогая остальные.
    #[test]
    fn a_tightened_ceiling_reaches_the_rebuilt_command_even_with_bad_fields_elsewhere() {
        let mut live = crate::cfg::filled_for_test();
        live.current_mut().write = "read_write".into();
        assert!(live.validate().is_ok(), "вводная: живая ячейка держит годный Cfg");
        let tightened = live.clone().with_access("read_only");
        assert_eq!(tightened.current().base_path, live.current().base_path, "остальные поля не тронуты");
        assert_eq!(tightened.current().db_user, live.current().db_user, "остальные поля не тронуты");
        let kit = common::kit::Kit::at(std::env::temp_dir());
        let cmd = crate::bridge::mcp_command(&tightened, &kit);
        let args: Vec<String> = cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.windows(2).any(|w| w == ["--access", "read_only"]), "{args:?}");
    }

    /// Поправка ведущего к кругу исправления 4: строка при отказе диска
    /// обязана сказать, что потолок НЕ применён, а не что он применён к
    /// сеансу — прежняя формулировка описывала состояние, которого не
    /// бывает (показ окна тут же откатывает живую ячейку).
    #[test]
    fn the_write_failure_line_says_the_ceiling_was_not_applied() {
        let line = access_not_applied_line("диск только для чтения");
        assert!(line.contains("НЕ применён"), "{line}");
        assert!(!line.contains("применено к работающему сеансу"), "{line}");
        assert!(line.contains("диск только для чтения"), "{line}");
    }

    #[test]
    fn choosing_a_mode_writes_the_toml_and_restarts_the_session() {
        // `request_current_mode_change`/`apply_mode` need `cx: &mut App`
        // (GPUI), which has no headless test harness here — same gap as the
        // window's own save path. What's testable without it, and what the
        // name promises: the config write that `stop_session`/`start_session`
        // key off next (mode lives in the config, not in extra state).
        let mut ui = Ui::for_test();
        assert_eq!(ui.cfg.current().mode, "tunnel");
        ui.cfg.current_mut().mode = "local".into();
        let host = ui.cfg.current().clone();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.cfg.current().mode, "local");
        let text = std::fs::read_to_string(ui.kit.state_path(crate::cfg::FILE)).unwrap();
        assert!(text.contains("mode = \"local\""), "{text}");
    }

    #[test]
    fn tray_icon_is_the_badged_png() {
        let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
        assert_eq!(icon_rgba(TrayState::Up).len(), (base.width() * base.height() * 4) as usize);
    }

    #[test]
    fn mode_switch_is_blocked_while_an_operation_or_a_check_runs() {
        let mut ui = Ui::for_test();
        assert!(!mode_change_blocked(&ui), "в покое ничего не мешает");
        ui.busy = Some(Busy::Project);
        assert!(mode_change_blocked(&ui), "идёт экспорт/импорт");
        ui.busy = None;
        assert!(!mode_change_blocked(&ui));
        // Повторная находка ревью: тест никогда не заводил `ui.progress` —
        // без этого случая он прошёл бы и без ветки `progress.is_some()` в
        // `mode_change_blocked` (т.е. не поймал бы её удаление).
        let now = Instant::now();
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false });
        assert!(mode_change_blocked(&ui), "идёт операция с карточкой хода");
        ui.progress = None;
        assert!(!mode_change_blocked(&ui));
        // Круг починки 5 задачи 16, ruling а: идущая публикация — тот же
        // счёт, одна дверь на смену режима и на переключение текущего
        // (`request_current_mode_change`, `request_switch_current`), а не
        // два похожих. Без этой ветки тест прошёл бы и до правки — находка
        // ровно в её отсутствии.
        ui.publishing = Some(now);
        assert!(mode_change_blocked(&ui), "идёт публикация — дёргать текущее соединение нельзя");
        ui.publishing = None;
        assert!(!mode_change_blocked(&ui));
    }

    /// Important 1 круга починки 1 задачи 5 — мина в ветви ОТКАЗА `save_host`.
    /// В файл там ложится намерение человека, и при уходе в «публикует
    /// платформа 1С» со снятым адресом это ПАРА ключей: до правки писалась одна
    /// публикация, а выведенный нами адрес снимался только в памяти — на диске
    /// оставалась «webserver» и наша же догадка, замок про неё молчал (поле
    /// непустое), `Cfg::base_url()` отдавал догадку, и проба уходила на ЧУЖОЙ
    /// веб-сервер на 80; окно при следующем показе возвращало догадку и в
    /// память.
    ///
    /// Решение «что писать» вынесено в чистую функцию ровно затем, чтобы
    /// держать его под тестом: саму `apply_publication` без живого `App` не
    /// позвать. Вторую половину — что пара ложится ОДНОЙ записью и что в файле
    /// не остаётся догадки — держит
    /// `save::tests::the_tray_writer_puts_a_pair_of_keys_in_one_pass`.
    #[test]
    fn a_refused_save_writes_the_publication_together_with_the_dropped_address() {
        assert_eq!(unsaved_publication_keys("webserver", true), vec![("publication", "webserver"), ("base_url", "")]);
        // Адрес человека не снимали — и писать его нечего, в файле он свой.
        assert_eq!(unsaved_publication_keys("webserver", false), vec![("publication", "webserver")]);
        // Обратно в «публикует коннектор» адрес считает сама программа.
        assert_eq!(unsaved_publication_keys("auto", false), vec![("publication", "auto")]);
    }

    /// Задача 5 плана «Карточка публикации и удаление»: смена публикации у
    /// ТЕКУЩЕЙ карточки идёт той же дверью, что смена режима
    /// (`request_current_publication_change` → `mode_change_blocked`), и
    /// ВТОРОГО счёта занятости у неё нет — иначе два похожих счёта однажды
    /// разошлись бы, и переключатель карточки дёрнул бы текущее соединение
    /// посреди публикации (результат, пришедший уже после переключения, лёг бы
    /// на прежнюю базу).
    ///
    /// Чего этот тест НЕ проверяет, прямо (Minor 1 круга починки 1, и оттуда
    /// же имя теста — прежнее обещало покрытие самой последовательности):
    /// `request_current_publication_change` здесь не зовётся и позвана быть не
    /// может — без живого `App` её не вызвать, гарнеса `gpui::test` в этом
    /// ящике нет вовсе (`gpui` приходит транзитивно, с `gpui-kit`), и по той же
    /// причине своего теста нет и у `request_current_mode_change`. Под тестом
    /// стоит СЧЁТ у двери публикации и слово отказа — не то, что дверь его
    /// спрашивает.
    #[test]
    fn the_publication_door_counts_busy_with_the_same_single_count_and_word() {
        let mut ui = Ui::for_test();
        assert!(!mode_change_blocked(&ui), "в покое ничего не мешает");
        ui.publishing = Some(Instant::now());
        assert!(mode_change_blocked(&ui), "идёт публикация — смена публикации текущей карточки не состоится");
        // И отказ назовёт, что именно идёт, а не «операцию» вообще: то же
        // слово берёт карточка (`panel::set_card_publication`), что и отказ
        // «Сделать текущим».
        assert_eq!(busy_now_word(&ui), "публикация базы");
        ui.publishing = None;
        ui.busy = Some(Busy::Project);
        assert!(mode_change_blocked(&ui), "идёт проверка — тот же счёт, не второй");
    }

    /// Пункт трея и переключатель окна запираются одним замком и расходиться
    /// не могут: у трея нет своего мнения о готовности.
    #[test]
    fn the_tray_item_follows_the_same_gate() {
        use crate::gui::panel::switch_locked;
        for on in [false, true] {
            for gate in [None, Some("не задан пользователь базы")] {
                assert_eq!(
                    toggle_enabled(gate.is_some(), on),
                    !switch_locked(gate, on, false),
                    "трей и окно разошлись: on={on}, gate={gate:?}"
                );
            }
        }
        // Что именно это значит: заперто — не включить; уже включено —
        // выключить можно всегда.
        assert!(!toggle_enabled(true, false));
        assert!(toggle_enabled(true, true));
        assert!(toggle_enabled(false, false));
    }

    /// Значок идёт тем же порядком разрешения, что и точка в карточке.
    /// Проверяется боевой путь — `tray_state_now`, то самое, что зовёт
    /// `sync_tray`: прежний тест звал `tray_state_of` напрямую и оставался
    /// зелёным, пока значок считался старым правилом (находка ревью).
    #[test]
    fn the_tray_follows_the_card_order() {
        use crate::gui::tray_state_now;
        use crate::gui::Verdict;
        use crate::session::State;
        use common::check::Check;

        let now = Instant::now();
        let mut ui = Ui::for_test();
        ui.cfg.current_mut().mode = "local".into();
        ui.link = State::Up { since: now };
        // `Ui::for_test` считает замок по каталогу теста, и там он заперт
        // (ни инструментов, ни платформы) — а речь здесь про цвет значка при
        // открытом замке; запертый проверяется ниже отдельно.
        ui.gate = None;
        let fail = Verdict::NotReady(Check::fail("проект", "не разложен", "1c-connect install"));

        ui.verdict = Verdict::Ready;
        assert_eq!(tray_state_now(&ui, now), TrayState::Up, "всё сошлось — зелёный");
        // Вердикт провален — своё состояние, и зелёным оно не бывает.
        ui.verdict = fail.clone();
        assert_eq!(tray_state_now(&ui, now), TrayState::Warn);
        // Вердикт ещё не посчитан — «занято», не зелёный.
        ui.verdict = Verdict::Checking;
        assert_eq!(tray_state_now(&ui, now), TrayState::Busy);
        // Идёт операция — побеждает она, даже поверх провалившегося вердикта.
        ui.verdict = fail.clone();
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false });
        assert_eq!(tray_state_now(&ui, now), TrayState::Busy);
        ui.progress = None;
        // Идёт проверка — карточки хода у неё нет, но значку это занятость.
        ui.busy = Some(Busy::Check);
        assert_eq!(tray_state_now(&ui, now), TrayState::Busy);
        ui.busy = None;
        // Замок и выключенное обслуживание — серый.
        ui.verdict = Verdict::Ready;
        ui.gate = Some("не задан пользователь базы".into());
        assert_eq!(tray_state_now(&ui, now), TrayState::Off);
        ui.gate = None;
        ui.link = State::Off;
        assert_eq!(tray_state_now(&ui, now), TrayState::Off);
    }

    /// Смена режима не поднимает сеанс поверх погашенного обслуживания:
    /// туннель, поднятый при запертом замке, показал бы зелёное «Подключён»,
    /// пока мост отказывает каждому вызову (находка S5). Локальные слушатели
    /// при этом поднимаются всегда — они держат порты и несут агенту причину
    /// отказа, а не «соединение отклонено».
    #[test]
    fn a_locked_gate_keeps_the_tunnel_down_after_a_mode_change() {
        let why = Some("не задан пользователь базы");
        assert!(!raise_session_after_mode_change("tunnel", why), "туннель поверх запертого замка");
        assert!(raise_session_after_mode_change("tunnel", None));
        assert!(raise_session_after_mode_change("local", why), "порты в локальном режиме подняты всегда");
        assert!(raise_session_after_mode_change("local", None));
    }

    /// Critical ревью круга починки 2 задачи 16: три исхода нажатия
    /// «Сделать текущим», чистое ядро `request_switch_current` — заблокировано
    /// операцией, нужен вопрос (то же основание, что у «Сохранить»), или
    /// применяем сразу. Коротышки «параметры совпали — пропустить» здесь НЕТ
    /// нарочно: у этой функции только два параметра, и сравнивать режимы
    /// негде — находка ревью была ровно в том, что переключение (в отличие
    /// от смены режима) не имеет права на такой короткий путь вовсе.
    #[test]
    fn switch_current_outcome_blocks_first_then_asks_and_otherwise_proceeds() {
        assert_eq!(switch_current_outcome(true, true), SwitchCurrentOutcome::Blocked, "идёт операция — вопрос не успевает");
        assert_eq!(switch_current_outcome(true, false), SwitchCurrentOutcome::Blocked);
        assert_eq!(switch_current_outcome(false, true), SwitchCurrentOutcome::NeedsConfirm);
        assert_eq!(switch_current_outcome(false, false), SwitchCurrentOutcome::Proceed);
    }

    /// Отложенная мелочь этой задачи: диалог «Сделать текущим» не отнимает
    /// фокус окна, и между вопросом (где предусловие уже проверено один раз,
    /// `switch_current_outcome` выше) и согласием человек успевает запустить
    /// проверку — применение обязано переспросить предусловие ЗАНОВО, а не
    /// довериться более раннему ответу. `current` здесь не трогаем вовсе:
    /// `apply_switch_current_decision` читает `&Ui`, не `&mut Ui`, и у
    /// заблокированного исхода нет пути к записи — это видно из одной
    /// сигнатуры, без гарнеса GPUI. `Proceed` несёт то же имя, которое
    /// получила на входе (круг починки 1 задачи 7, Minor 2) — проверяю и
    /// это, а не только сам факт разрешения.
    #[test]
    fn switching_current_rechecks_the_precondition_at_apply_time() {
        let mut ui = Ui::for_test();
        let current_before = ui.cfg.current.clone();
        assert!(!mode_change_blocked(&ui), "в покое предусловие проходит — тест должен ловить именно занятость");

        ui.busy = Some(Busy::Project);
        match apply_switch_current_decision(&ui, "Ромашка") {
            ApplySwitchCurrent::Blocked(why) => assert!(why.contains("проверка") && why.contains("Сделать текущим"), "{why}"),
            ApplySwitchCurrent::Proceed(_) => panic!("занятость обязана отказать, а не разрешить запись"),
        }
        assert_eq!(ui.cfg.current, current_before, "отказ не трогает текущее соединение");

        ui.busy = None;
        assert_eq!(
            apply_switch_current_decision(&ui, "Ромашка"),
            ApplySwitchCurrent::Proceed("Ромашка".to_string()),
            "в покое запись разрешена, и несёт то же имя"
        );
    }

    /// MEDIUM ревью круга починки 4 задачи 16: перецикл (`recycle_session`,
    /// общий для смены режима и переключения текущего) не поднимает сеанс,
    /// если обслуживание было выключено ДО переключения — выключенный
    /// переключатель не обходится ни одним из этих двух действий.
    #[test]
    fn should_raise_session_keeps_a_disabled_switch_off() {
        assert!(!should_raise_session(false, "tunnel", None), "MCP выключены — перецикл не включает их обратно");
        assert!(!should_raise_session(false, "local", None), "выключено значит выключено даже в локальном режиме");
        assert!(should_raise_session(true, "tunnel", None), "живой сеанс можно перезапустить");
        assert!(!should_raise_session(true, "tunnel", Some("замок")), "замок всё равно сильнее включённого выключателя");
    }

    /// HIGH ревью круга починки 4 задачи 16: после переключения текущего
    /// публикация обязана переехать на новую базу, а чужой (нашей же, но
    /// прежней) больше не висеть — независимо от того, включено ли сейчас
    /// обслуживание (гасить прежний httpd нужно всегда, раз база сменилась;
    /// поднимать новую публикацию — только когда включено, тем же правилом,
    /// что у `save_host`/`republish_after_save`).
    #[test]
    fn publication_after_switch_stops_the_old_one_and_only_raises_the_new_when_enabled() {
        // База не изменилась — публикация прежней базы ещё в силе, трогать нечего.
        assert_eq!(publication_after_switch(false, true, true), PublicationAfterSwitch { stop_old: false, raise_new: false });
        // База изменилась, свой httpd был поднят, MCP включены — гасим старый и поднимаем новый.
        assert_eq!(publication_after_switch(true, true, true), PublicationAfterSwitch { stop_old: true, raise_new: true });
        // MCP выключены — новую публикацию не поднимаем, но прежний свой httpd всё равно гасим: он держит адрес, которого у текущего соединения больше нет.
        assert_eq!(publication_after_switch(true, false, true), PublicationAfterSwitch { stop_old: true, raise_new: false });
        // Свой httpd не поднимали (публикует веб-сервер) — гасить нечего, но новую, если включено, всё равно переустанавливаем.
        assert_eq!(publication_after_switch(true, true, false), PublicationAfterSwitch { stop_old: false, raise_new: true });
    }

    /// Круг починки 5 задачи 16 — находка двух ревьюеров одновременно, и
    /// ровно тот случай, который оба назвали: переключение со СТАРОЙ
    /// карточки `auto` (наш httpd поднят) на НОВУЮ `webserver`. Без снимка
    /// `old_cfg`, взятого ДО `switch_current`, `old_ctx` нёс бы уже НОВУЮ
    /// карточку — `stop_own_publication` решил бы по ней «нечего гасить»
    /// (`Ok(false)`), и прежний процесс остался бы сиротой.
    ///
    /// Тест гоняет `write_switch_current` — часть `apply_switch_current`,
    /// которой `App` не нужен вовсе, выделенную специально ради этого
    /// теста, — и дальше ПРОВЕРЯЕТ не решение, а сам `Ctx`: зовёт
    /// `stop_own_publication(&written.old_ctx)` и требует, чтобы она НЕ
    /// сказала «нечего гасить». Если кто-нибудь однажды соберёт `old_ctx`
    /// из `ui.cfg` ПОСЛЕ записи (как было до этого круга) — тест упадёт
    /// именно на этой строке.
    #[test]
    fn write_switch_current_builds_the_old_ctx_not_the_new_one() {
        let mut ui = Ui::for_test();
        ui.cfg.current_mut().publication = "auto".into();
        ui.raised_publication = true;
        ui.cfg.connections.push(Connection {
            name: "Ромашка".into(),
            base_path: "C:\\Bases\\Ромашка".into(),
            project: "C:\\proj\\Ромашка".into(),
            publication: "webserver".into(),
            ..ui.cfg.current().clone()
        });
        crate::gui::with_params_file(&ui);

        let written = write_switch_current(&mut ui, "Ромашка").unwrap();

        assert!(written.what.stop_old, "старая карточка auto держала свой httpd — гасить обязаны");
        assert_ne!(
            crate::ops_host::stop_own_publication(&written.old_ctx).ok(),
            Some(false),
            "old_ctx обязан нести СТАРУЮ карточку (auto) — Ok(false) означал бы, что он смотрит на новую (webserver)"
        );
    }

    /// Круг починки 5 задачи 16, ruling в: признак `raised_publication`
    /// сбрасывается ТОЛЬКО когда гашение решило, что оно и правда наше
    /// (`Ok(true)`) — `write_switch_current` сама этого не делает (решение
    /// принимает вызывающий, `apply_switch_current`, уже с реальным
    /// результатом `stop_own_publication`), и этот тест фиксирует именно
    /// это разделение: после переключения признак остаётся как был, пока
    /// никто не подтвердил удачу.
    #[test]
    fn write_switch_current_itself_never_touches_the_flag() {
        let mut ui = Ui::for_test();
        ui.cfg.current_mut().publication = "auto".into();
        ui.raised_publication = true;
        ui.cfg.connections.push(Connection {
            name: "Ромашка".into(),
            base_path: "C:\\Bases\\Ромашка".into(),
            project: "C:\\proj\\Ромашка".into(),
            publication: "webserver".into(),
            ..ui.cfg.current().clone()
        });
        crate::gui::with_params_file(&ui);

        write_switch_current(&mut ui, "Ромашка").unwrap();

        assert!(ui.raised_publication, "решение, сбрасывать ли признак, принимает вызывающий по результату гашения, не эта функция");
    }

    /// Круг починки 5 задачи 16, ruling а: отказ «Сделать текущим» называет,
    /// что именно идёт, — операцию важнее проверки (у операции есть
    /// карточка хода), публикацию последней (её одну можно начать и без
    /// операции/проверки).
    #[test]
    fn busy_now_word_names_the_operation_the_check_or_the_publication() {
        let mut ui = Ui::for_test();
        let now = Instant::now();
        ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false });
        assert_eq!(busy_now_word(&ui), "операция");
        ui.progress = None;
        ui.busy = Some(Busy::Project);
        assert_eq!(busy_now_word(&ui), "проверка");
        ui.busy = None;
        ui.publishing = Some(now);
        assert_eq!(busy_now_word(&ui), "публикация базы");
    }

    #[test]
    fn the_busy_badge_is_yellow() {
        let busy = icon_rgba(TrayState::Busy);
        assert!(busy.chunks(4).any(|p| p == [0xfa, 0xcc, 0x1f, 255]), "no yellow pixel in the busy badge");
        let up = icon_rgba(TrayState::Up);
        assert!(up.chunks(4).any(|p| p == [0x2f, 0x8f, 0x4e, 255]), "no green pixel in the up badge");
    }

    /// Задача «Четыре локальных агента»: id пунктов подменю «Локальные
    /// агенты» разбираются в действие для всех четырёх, незнакомый id —
    /// по-прежнему `None`. Порядок в реестре — файловые агенты, затем
    /// CLI-агенты, как и задумано.
    #[test]
    fn agent_menu_ids_map_to_actions() {
        assert_eq!(menu_action(&agent_menu_id("zed")), Some(MenuAction::Agent("zed")));
        assert_eq!(menu_action(&agent_menu_id("pi")), Some(MenuAction::Agent("pi")));
        assert_eq!(menu_action(&agent_menu_id("claude")), Some(MenuAction::Agent("claude")));
        assert_eq!(menu_action(&agent_menu_id("codex")), Some(MenuAction::Agent("codex")));
        assert_eq!(menu_action("agent:bsl-reference"), None);
        assert_eq!(AGENTS_SUBMENU, "Локальные агенты");
        assert_eq!(editors::AGENTS.iter().map(|d| d.id).collect::<Vec<_>>(), ["zed", "pi", "claude", "codex"]);
    }

    /// Галочка стоит только на `Found::Ours` — остальные состояния из неё не
    /// различимы нарочно, причина отказа идёт отдельно, в журнал. Нечитаемое
    /// состояние (`Err`) — тоже снятая галочка: `Ours` на том, чего мы не
    /// прочитали, был бы обещанием за чужой файл.
    #[test]
    fn the_checkbox_is_checked_only_on_ours() {
        use crate::editors::Found;
        assert!(agent_checked(&Ok(Found::Ours)));
        assert!(!agent_checked(&Ok(Found::Missing)));
        assert!(!agent_checked(&Ok(Found::Absent)));
        assert!(!agent_checked(&Ok(Found::Foreign(vec!["1c".into()]))));
        assert!(!agent_checked(&Err("не прочитать .claude.json".to_string())));
    }

    /// Пункт доступен только когда агенты живут на этой машине и только
    /// если агент на машине вообще есть — любое из двух условий само по
    /// себе не хватает.
    #[test]
    fn an_agent_item_needs_agents_here_and_the_agent_present() {
        assert!(agent_available("local", true));
        assert!(!agent_available("local", false), "агента нет — нечего включать галочкой");
        assert!(!agent_available("tunnel", true), "в туннеле агентов настраивает образ, не мы");
        assert!(!agent_available("tunnel", false));
    }
}
