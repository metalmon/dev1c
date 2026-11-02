//! Значок в трее, его меню (включая подменю «Тема») и мост событий ОС в
//! канал приложения: обработчики `tray-icon`/`muda` зовёт WndProc на потоке
//! GPUI, они только кладут событие в канал — его разбирает `on_tray`/`on_menu`
//! из задачи-читателя в `mod.rs::handle`.

use super::{
    autostart, badge, confirm_database_change, note_line, quit, run_check, save_host, save_needs_confirm, save_tray_key, start_session,
    stop_session, theme, toggle_agent, toggle_link, window, Ev, Kind, Line, Tab, TrayState, Ui, TRAY_PNG,
};
use crate::editors;
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::App;
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Текст пункта «Запускать при входе»: на Windows речь про вход в саму
/// Windows, иначе — просто про вход (нет чего-то другого, с чем спутать).
#[cfg(windows)]
const AUTOSTART_TEXT: &str = "Запускать при входе в Windows";
#[cfg(not(windows))]
const AUTOSTART_TEXT: &str = "Запускать при входе";

/// Трей: значок, пункт-переключатель (меняет надпись), подменю «Режим
/// работы», «Публикация базы», «Запись в базу» и «Локальные агенты» (в
/// первых трёх отмечен ровно один, в четвёртом — независимые галочки по
/// агенту), три пункта темы (отмечен ровно один) и два пункта-галочки —
/// автозапуск и автоподключение.
pub struct Tray {
    icon: TrayIcon,
    toggle: MenuItem,
    modes: [CheckMenuItem; 2],
    publications: [CheckMenuItem; 2],
    /// Потолок записи (`cfg.host.write`/`Cfg::access()`) — тем же приёмом,
    /// что `modes`/`publications` выше: радио из двух пунктов, строится и
    /// читается одинаково.
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

/// Режим работы (`cfg.host.mode`, "tunnel"/"local") → id пункта меню и его
/// подпись. Поля в окне нет — режим пишет только этот пункт (см.
/// `gui::save_host`).
const MODES: [(&str, &str, &str); 2] = [("tunnel", "mode:tunnel", "Через туннель"), ("local", "mode:local", "Локально")];

/// Кто публикует базу (`cfg.host.publication`, "auto"/"webserver") → id
/// пункта меню и его подпись. Значения в toml прежние, подписи — человеческие:
/// человек читает название, а не имя режима. Поля в окне нет; адрес чужой
/// публикации спрашивается отдельным полем «Параметров», видимым только во
/// втором положении (`panel::shown_fields`).
const PUBLICATIONS: [(&str, &str, &str); 2] =
    [("auto", "publication:auto", "Публикует коннектор"), ("webserver", "publication:webserver", "Публикует платформа 1С")];

/// Потолок записи (`cfg.host.write`, значения — те же слова, что понимает
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

/// Заголовок подменю. Назван отдельно, потому что без слова «базы» пункты
/// под ним говорят о механике, а не о деле: «публикация» сама по себе не
/// отвечает, что публикуется (замечание владельца 28.10).
///
/// Обе строки под ним отвечают на один вопрос — кто делает публикацию:
/// коннектор своим httpd, или платформа 1С, когда человек уже опубликовал
/// базу диалогом Конфигуратора. Прежние «Уже опубликована» и «на
/// веб-сервере» называли состояние и механику, а не того, кто это сделал
/// (решение владельца 28.10).
const PUBLICATION_SUBMENU: &str = "Публикация базы";

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
    /// Выбор пункта подменю «Режим работы» — несёт "tunnel" или "local".
    Mode(&'static str),
    /// Выбор пункта подменю «Публикация базы» — несёт "auto" или "webserver".
    Publication(&'static str),
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
    for (mode, mode_id, _) in MODES {
        if id == mode_id {
            return Some(MenuAction::Mode(mode));
        }
    }
    for (publication, publication_id, _) in PUBLICATIONS {
        if id == publication_id {
            return Some(MenuAction::Publication(publication));
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
    // Отметка при создании меню: умолчание `Host::default()`, пока не
    // прочитано из toml — `build` не получает конфиг, в отличие от `choice`
    // (тот читает `theme::Choice::load` до вызова). `launch` поправляет её на
    // настоящий сохранённый режим сразу после `build`, до показа окна
    // (`Tray::set_mode`) — так что галочка никогда не достаётся человеку
    // неверной.
    let mode_items = MODES.map(|(m, id, text)| CheckMenuItem::with_id(id, text, true, m == "tunnel", None));
    let mode_refs: Vec<&dyn IsMenuItem> = mode_items.iter().map(|i| i as &dyn IsMenuItem).collect();
    let mode_submenu = Submenu::with_items("Режим работы", true, &mode_refs)?;
    // Та же оговорка про начальную отметку, что и у режима работы: здесь
    // умолчание «публикует коннектор», а `launch` поправляет её на
    // сохранённую сразу после `build` (`Tray::set_publication`).
    let publication_items = PUBLICATIONS.map(|(p, id, text)| CheckMenuItem::with_id(id, text, true, p == "auto", None));
    let publication_refs: Vec<&dyn IsMenuItem> = publication_items.iter().map(|i| i as &dyn IsMenuItem).collect();
    let publication_submenu = Submenu::with_items(PUBLICATION_SUBMENU, true, &publication_refs)?;
    // Та же оговорка, что у режима и публикации выше: умолчание здесь
    // «только чтение», `launch` поправляет отметку на настоящий потолок
    // сразу после `build` (`Tray::set_access`).
    let access_items = ACCESS.map(|(a, id, text)| CheckMenuItem::with_id(id, text, true, a == "read_only", None));
    let access_refs: Vec<&dyn IsMenuItem> = access_items.iter().map(|i| i as &dyn IsMenuItem).collect();
    let access_submenu = Submenu::with_items(ACCESS_SUBMENU, true, &access_refs)?;
    // Начальные галочка и доступность — оба `false`: `launch` поправляет их
    // сразу после `build` (`Tray::set_agents`), тем же приёмом, что у режима
    // и публикации выше — здесь ни `Found`, ни режим ещё не читали.
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
        &mode_submenu,
        &publication_submenu,
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
    let modes = [mode_items[0].clone(), mode_items[1].clone()];
    let publications = [publication_items[0].clone(), publication_items[1].clone()];
    let access = [access_items[0].clone(), access_items[1].clone()];
    let agents = [agent_items[0].clone(), agent_items[1].clone(), agent_items[2].clone(), agent_items[3].clone()];
    Ok(Tray {
        icon: tray,
        toggle,
        modes,
        publications,
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

    /// Отмечает ровно один пункт подменю «Режим работы».
    pub fn set_mode(&self, mode: &str) {
        for (item, (m, ..)) in self.modes.iter().zip(MODES) {
            item.set_checked(m == mode);
        }
    }

    /// Отмечает ровно один пункт подменю «Публикация» — тот же приём, что у
    /// режима работы: нативный пункт отмечает себя сам по щелчку, и вернуть
    /// отметку на действующее значение обязаны мы.
    pub fn set_publication(&self, publication: &str) {
        for (item, (p, ..)) in self.publications.iter().zip(PUBLICATIONS) {
            item.set_checked(p == publication);
        }
    }

    /// Отмечает ровно один пункт подменю «Запись в базу» — тот же приём, что
    /// у режима и публикации выше.
    pub fn set_access(&self, access: &str) {
        for (item, (a, ..)) in self.access.iter().zip(ACCESS) {
            item.set_checked(a == access);
        }
    }

    /// Галочки и доступность подменю «Локальные агенты» — зовётся при
    /// запуске и при каждой смене режима (решение владельца: доступность
    /// обязана обновляться на ходу, а не только при старте — режим
    /// переключается тем же меню трея, без перезапуска окна). Галочка стоит
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
                // «Публикацией базы» и «Темой», и переключение режима не
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

/// Операция или проверка идёт — тем же критерием, что блокирует «Сохранить»
/// (`panel::can_save`) и переключатель (`Switch::disabled`). Смена режима
/// посреди неё переписала бы конфигурацию и перезапустила сеанс под её
/// ногами — у подменю «Режим работы» этой защиты не было (находка целостного
/// ревью).
fn mode_change_blocked(ui: &Ui) -> bool {
    ui.busy.is_some() || ui.progress.is_some()
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

/// Записать новый режим и поднять сеанс заново. Зовётся после согласия
/// человека (`confirm_database_change`), поэтому `ui.cfg.host.mode` правится
/// здесь, а не до вопроса: `save_host` берёт режим из текущих параметров и
/// иначе переписал бы его обратно.
fn apply_mode(cx: &mut App, mode: &'static str) {
    cx.global_mut::<Ui>().cfg.host.mode = mode.into();
    let host = cx.global::<Ui>().cfg.host.clone();
    match save_host(cx.global_mut::<Ui>(), host) {
        Ok(()) => {
            if let Some(tray) = &cx.global::<Ui>().tray {
                tray.set_mode(mode);
                // Доступность «Локальных агентов» зависит от режима — обязана
                // обновиться тут же, не только при запуске (задача «Управление
                // переехало в трей»).
                tray.set_agents(mode);
            }
            // Новый транспорт — сеанс поднимается заново; `stop_session`
            // безвредна, если сеанса и не было.
            stop_session(cx);
            let ui = cx.global::<Ui>();
            if raise_session_after_mode_change(&ui.cfg.host.mode, ui.gate.as_deref()) {
                if let Err(e) = start_session(cx.global_mut::<Ui>()) {
                    note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
                }
            }
            let mut label = mode;
            for (m, _, text) in MODES {
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
            let label = MODES.iter().find(|(m, ..)| *m == mode).map(|(_, _, t)| *t).unwrap_or(mode);
            note_line(cx.global_mut::<Ui>(), unsaved_switch_line("режим работы", label, why));
            if let Some(tray) = &cx.global::<Ui>().tray {
                tray.set_mode(mode);
                tray.set_agents(mode);
            }
            cx.global_mut::<Ui>().tab = Tab::Params;
            window::show_window(cx);
        }
    }
    cx.refresh_windows();
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
fn apply_publication(cx: &mut App, publication: &'static str) {
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
        if crate::cfg::is_derived_address(&ui.cfg.host) {
            ui.cfg.host.base_url = String::new();
        }
    }
    cx.global_mut::<Ui>().cfg.host.publication = publication.into();
    let host = cx.global::<Ui>().cfg.host.clone();
    match save_host(cx.global_mut::<Ui>(), host) {
        Ok(()) => {
            if let Some(tray) = &cx.global::<Ui>().tray {
                tray.set_publication(publication);
            }
            stop_session(cx);
            let ui = cx.global::<Ui>();
            if raise_session_after_mode_change(&ui.cfg.host.mode, ui.gate.as_deref()) {
                if let Err(e) = start_session(cx.global_mut::<Ui>()) {
                    note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
                }
            }
            let mut label = publication;
            for (p, _, text) in PUBLICATIONS {
                if p == publication {
                    label = text;
                }
            }
            note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Ok, text: format!("публикация: {}", label.to_lowercase()) });
        }
        Err(why) => {
            // Проверка всего файла не прошла — на свежем комплекте поля ещё
            // пусты. Для переключателя это не повод не сработать: он не
            // действие над базой, а намерение человека. Пишем один ключ
            // (`save_tray_key`), остальное в файле остаётся как было.
            //
            // Именно в файл, а не в память: окно перечитывает файл на каждом
            // показе, и намерение, оставленное в памяти, стирается первым же
            // открытием окна — поле «Адрес публикации» тогда не появляется
            // вовсе (находка владельца 28.10).
            if let Err(e) = save_tray_key(cx.global_mut::<Ui>(), "publication", publication) {
                note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
                return;
            }
            let label = PUBLICATIONS.iter().find(|(p, ..)| *p == publication).map(|(_, _, t)| *t).unwrap_or(publication);
            note_line(cx.global_mut::<Ui>(), unsaved_switch_line("публикация базы", label, why));
            if let Some(tray) = &cx.global::<Ui>().tray {
                tray.set_publication(publication);
            }
            // И открываем то, что человеку осталось сделать: «Параметры».
            // В режиме «публикует платформа 1С» там же появляется поле
            // «Адрес публикации», которое этот режим и требует, — иначе до
            // него было не добраться вовсе.
            cx.global_mut::<Ui>().tab = Tab::Params;
            window::show_window(cx);
        }
    }
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

fn apply_access(cx: &mut App, access: &'static str) {
    cx.global_mut::<Ui>().cfg.host.write = access.into();
    let host = cx.global::<Ui>().cfg.host.clone();
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
            cx.global_mut::<Ui>().tab = Tab::Params;
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

/// Левый щелчок (отжатие кнопки) по значку — показать/скрыть окно; меню —
/// только по правому, сюда не попадает.
pub fn on_tray(ev: TrayIconEvent, cx: &mut App) {
    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
        window::toggle_window(cx);
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
        MenuAction::Mode(mode) => {
            // Та же защита, что у «Сохранить» и у переключателя
            // (`panel::can_save`, `Switch::disabled`): смена режима
            // переписывает toml и перезапускает сеанс — под идущей
            // операцией (например, «Выгрузить») это переписало бы
            // конфигурацию и оборвало бы её посреди работы. Нативный пункт
            // меню уже успел отметить себя сам — возвращаем галочку на
            // настоящий режим, раз действие не состоялось.
            let ui = cx.global::<Ui>();
            let current = ui.cfg.host.mode.clone();
            if mode_change_blocked(ui) || mode == current {
                if let Some(tray) = &ui.tray {
                    tray.set_mode(&current);
                }
                return;
            }
            // Галочка возвращается на действующий режим сразу: состоится ли
            // смена, решает вопрос ниже, а пункт отметил себя сам ещё до нас.
            // Согласие вернёт её на новый режим (`apply_mode`).
            if let Some(tray) = &ui.tray {
                tray.set_mode(&current);
            }
            // Тот же вопрос, что у «Сохранить»: смена режима может погасить
            // обслуживание — замки у режимов разные (туннелю нужны адрес
            // среды, пароль и имя рабочего места). Параметры меняем только
            // после согласия, а не до вопроса: иначе «было ли обслуживание
            // живым» считалось бы уже по новому режиму, и вопрос не
            // появлялся бы вовсе (находка ревью задачи 23).
            let mut host = ui.cfg.host.clone();
            host.mode = mode.into();
            // Спрашивать не о чем (или спросить негде — значок живёт и без
            // окна) — пишем сразу, как делали раньше.
            if !save_needs_confirm(ui, &host) || ui.window.is_none() {
                apply_mode(cx, mode);
                return;
            }
            // Диалог платформенный и принадлежит окну: на спрятанном окне
            // его человеку не видно, а смена режима тихо ждала бы ответа.
            window::show_window(cx);
            let Some(w) = cx.global::<Ui>().window else { return };
            let _ = w.update(cx, |_, window, cx| {
                confirm_database_change(window, cx, &host, move |_window, cx| apply_mode(cx, mode));
            });
        }
        MenuAction::Publication(publication) => {
            // Форма та же, что у «Режима работы» выше, и по тем же причинам:
            // под идущей операцией не переключаем, а отметку, которую пункт
            // поставил себе сам, возвращаем на действующее значение.
            let ui = cx.global::<Ui>();
            let current = ui.cfg.host.publication.clone();
            if mode_change_blocked(ui) || publication == current {
                if let Some(tray) = &ui.tray {
                    tray.set_publication(&current);
                }
                return;
            }
            if let Some(tray) = &ui.tray {
                tray.set_publication(&current);
            }
            // Уборка своей публикации — не здесь, а в `apply_publication`:
            // вопрос ниже человек может отклонить, и тогда гасить что-либо
            // было бы прямым вредом (свой httpd убит, режим прежний).
            let mut host = cx.global::<Ui>().cfg.host.clone();
            host.publication = publication.into();
            let ui = cx.global::<Ui>();
            if !save_needs_confirm(ui, &host) || ui.window.is_none() {
                apply_publication(cx, publication);
                return;
            }
            window::show_window(cx);
            let Some(w) = cx.global::<Ui>().window else { return };
            let _ = w.update(cx, |_, window, cx| {
                confirm_database_change(window, cx, &host, move |_window, cx| apply_publication(cx, publication));
            });
        }
        MenuAction::Access(access) => {
            // Форма та же, что у «Публикации базы» выше, без вопроса
            // человеку: потолок не действие над базой и не меняет
            // транспорт (см. `apply_access`), спрашивать не о чем.
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
        // Независимая галочка, не радио-группа (в отличие от режима и
        // публикации выше): нативный пункт уже переключил сам себя, и
        // `is_checked()` здесь читает то, что он только что выставил, —
        // тот же приём, что у «Автозапуска»/«Автоподключения» ниже.
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
        access_not_applied_line, agent_available, agent_checked, agent_menu_id, icon_rgba, menu_action, mode_change_blocked,
        raise_session_after_mode_change, save_host, toggle_enabled, unsaved_switch_line, Kind, MenuAction, TrayState, Ui, ACCESS,
        ACCESS_SUBMENU, AGENTS_SUBMENU, MODES, PUBLICATIONS, PUBLICATION_SUBMENU,
    };
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
        assert_eq!(menu_action("mode:tunnel"), Some(MenuAction::Mode("tunnel")));
        assert_eq!(menu_action("mode:local"), Some(MenuAction::Mode("local")));
        assert_eq!(menu_action("nope"), None);
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

    #[test]
    fn the_publication_submenu_speaks_in_human_words() {
        assert_eq!(menu_action("publication:auto"), Some(MenuAction::Publication("auto")));
        assert_eq!(menu_action("publication:webserver"), Some(MenuAction::Publication("webserver")));
        assert_eq!(menu_action("publication:iis"), None);
        // Значения toml не меняются, меняются подписи. Подпись называет
        // дело, а не механику: что публикуется (база) и кто это делает.
        let labels: Vec<&str> = PUBLICATIONS.iter().map(|(_, _, text)| *text).collect();
        assert_eq!(labels, vec!["Публикует коннектор", "Публикует платформа 1С"]);
        assert_eq!(PUBLICATIONS.map(|(v, ..)| v), ["auto", "webserver"]);
        assert_eq!(PUBLICATION_SUBMENU, "Публикация базы");
    }

    /// Тот же приём, что у публикации выше: значения toml — слова, что
    /// понимает ребёнок (`read_only`/`read_write`), подписи — человеческие.
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

    /// Подменю строится тем же кодом, что «Режим работы»
    /// (`mode_submenu_has_two_checked_items`) — отмечен ровно умолчальный
    /// пункт, «Только чтение».
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
        ui.cfg.host.write = "read_write".into();
        let host = ui.cfg.host.clone();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.cfg.access(), "read_write");
        let text = std::fs::read_to_string(ui.kit.state_path(crate::cfg::FILE)).unwrap();
        assert!(text.contains("write = \"read_write\""), "{text}");
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
        live.host.write = "read_write".into();
        assert!(live.validate().is_ok(), "вводная: живая ячейка держит годный Cfg");
        let tightened = live.clone().with_access("read_only");
        assert_eq!(tightened.host.base_path, live.host.base_path, "остальные поля не тронуты");
        assert_eq!(tightened.host.db_user, live.host.db_user, "остальные поля не тронуты");
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
    fn mode_submenu_has_two_checked_items() {
        // Тот же порядок конструирования, что в `build`: пункты подменю —
        // данные, не требуют ни настоящего значка трея, ни (на Windows, где
        // и крутится этот тест) инициализированного GTK.
        let items = MODES.map(|(m, id, text)| CheckMenuItem::with_id(id, text, true, m == "local", None));
        let refs: Vec<&dyn IsMenuItem> = items.iter().map(|i| i as &dyn IsMenuItem).collect();
        let submenu = Submenu::with_items("Режим работы", true, &refs).unwrap();
        assert_eq!(submenu.text(), "Режим работы");
        assert_eq!(submenu.items().len(), 2);
        assert_eq!(items[0].text(), "Через туннель");
        assert_eq!(items[1].text(), "Локально");
        // Отмечен ровно текущий — здесь «local».
        assert!(!items[0].is_checked());
        assert!(items[1].is_checked());
    }

    #[test]
    fn choosing_a_mode_writes_the_toml_and_restarts_the_session() {
        // `MenuAction::Mode`'s handler in `on_menu` needs `cx: &mut App`
        // (GPUI), which has no headless test harness here — same gap as the
        // window's own save path. What's testable without it, and what the
        // name promises: the config write that `stop_session`/`start_session`
        // key off next (mode lives in the config, not in extra state).
        let mut ui = Ui::for_test();
        assert_eq!(ui.cfg.host.mode, "tunnel");
        ui.cfg.host.mode = "local".into();
        let host = ui.cfg.host.clone();
        save_host(&mut ui, host).unwrap();
        assert_eq!(ui.cfg.host.mode, "local");
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
        ui.cfg.host.mode = "local".into();
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
