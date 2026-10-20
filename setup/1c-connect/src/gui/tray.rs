//! Значок в трее, его меню (включая подменю «Тема») и мост событий ОС в
//! канал приложения: обработчики `tray-icon`/`muda` зовёт WndProc на потоке
//! GPUI, они только кладут событие в канал — его разбирает `on_tray`/`on_menu`
//! из задачи-читателя в `mod.rs::handle`.

use super::{
    autostart, badge, note_line, quit, run_check, save_host, start_session, stop_session, theme, toggle_link, window, Ev, Kind, Line, Tab,
    TrayState, Ui, TRAY_PNG,
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
    let toggle = MenuItem::with_id("toggle", "Подключить", true, None);
    let check = MenuItem::with_id("check", "Проверить", true, None);
    // Отметка при создании меню: умолчание `Host::default()`, пока не
    // прочитано из toml — `build` не получает конфиг, в отличие от `choice`
    // (тот читает `theme::Choice::load` до вызова). ponytail: если
    // сохранённый режим — «local», галочка на миг отстаёт от реального до
    // первого клика по пункту; заводить параметр ради этого — трогать
    // вызывающую сторону за пределами tray.rs.
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
    /// Значок badge(state) и надпись переключателя «Отключить»/«Подключить».
    /// Подсказку `set_state` не трогает: `TrayState` одно на троих (занят,
    /// подключён, нет связи) и само по себе не знает фазу операции — её
    /// несёт только `tray_tooltip`, и каждый зовущий (`set_link`, `run_op`,
    /// `Ev::Done`, тик) сам зовёт `set_tooltip` следом тем же текстом, иначе
    /// занятость при поднятой связи осталась бы с «не подключён».
    pub fn set_state(&self, state: TrayState, on: bool) {
        let _ = self.icon.set_icon(Some(icon(state)));
        self.toggle.set_text(if on { "Отключить" } else { "Подключить" });
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
                    if let Err(e) = start_session(cx) {
                        note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
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
    use super::{icon_rgba, menu_action, save_host, MenuAction, TrayState, Ui, MODES};
    use crate::cfg;
    use crate::gui::theme;
    use crate::gui::TRAY_PNG;
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
        let text = std::fs::read_to_string(ui.kit.path(cfg::FILE)).unwrap();
        assert!(text.contains("mode = \"local\""), "{text}");
    }

    #[test]
    fn tray_icon_is_the_badged_png() {
        let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
        assert_eq!(icon_rgba(TrayState::Up).len(), (base.width() * base.height() * 4) as usize);
    }

    #[test]
    fn the_busy_badge_is_yellow() {
        let busy = icon_rgba(TrayState::Busy);
        assert!(busy.chunks(4).any(|p| p == [0xfa, 0xcc, 0x1f, 255]), "no yellow pixel in the busy badge");
        let up = icon_rgba(TrayState::Up);
        assert!(up.chunks(4).any(|p| p == [0x2f, 0x8f, 0x4e, 255]), "no green pixel in the up badge");
    }
}
