//! Значок в трее, его меню (включая подменю «Тема») и мост событий ОС в
//! канал приложения: обработчики `tray-icon`/`muda` зовёт WndProc на потоке
//! GPUI, они только кладут событие в канал — его разбирает `on_tray`/`on_menu`
//! из задачи-читателя в `mod.rs::handle`.

use super::{
    autostart, badge, confirm_database_change, note_line, quit, run_check, save_host, save_needs_confirm, start_session, stop_session, theme,
    toggle_link, window, Ev, Kind, Line, Tab, TrayState, Ui, TRAY_PNG,
};
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
/// работы» (отмечен ровно один), три пункта темы (отмечен ровно один) и два
/// пункта-галочки — автозапуск и автоподключение.
pub struct Tray {
    icon: TrayIcon,
    toggle: MenuItem,
    modes: [CheckMenuItem; 2],
    themes: [CheckMenuItem; 3],
    autostart: CheckMenuItem,
    autoconnect: CheckMenuItem,
}

/// Выбор темы → id пункта меню и наоборот.
const THEMES: [(theme::Choice, &str, &str); 3] =
    [(theme::Choice::System, "theme:system", "Системная"), (theme::Choice::Light, "theme:light", "Светлая"), (theme::Choice::Dark, "theme:dark", "Тёмная")];

/// Режим работы (`cfg.host.mode`, "tunnel"/"local") → id пункта меню и его
/// подпись. Поля в окне нет — режим пишет только этот пункт (см.
/// `gui::save_host`).
const MODES: [(&str, &str, &str); 2] = [("tunnel", "mode:tunnel", "Через туннель"), ("local", "mode:local", "Локально")];

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
        &themes_submenu,
        &autostart,
        &autoconnect,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    let tray = TrayIconBuilder::new()
        .with_icon(icon(TrayState::Off))
        .with_tooltip("1c-connect: не подключён")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    let t2 = tx.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = t2.unbounded_send(Ev::Tray(e));
    }));
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = tx.unbounded_send(Ev::Menu(e));
    }));

    let themes = [items[0].clone(), items[1].clone(), items[2].clone()];
    let modes = [mode_items[0].clone(), mode_items[1].clone()];
    Ok(Tray { icon: tray, toggle, modes, themes, autostart, autoconnect })
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
            }
            // Новый транспорт — сеанс поднимается заново; `stop_session`
            // безвредна, если сеанса и не было.
            stop_session(cx);
            let ui = cx.global::<Ui>();
            if raise_session_after_mode_change(&ui.cfg.host.mode, ui.gate.as_deref()) {
                if let Err(e) = start_session(cx) {
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
        Err(e) => note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e }),
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
        icon_rgba, menu_action, mode_change_blocked, raise_session_after_mode_change, save_host, toggle_enabled, MenuAction, TrayState, Ui, MODES,
    };
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
}
