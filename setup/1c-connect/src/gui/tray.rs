//! Значок в трее, его меню (включая подменю «Тема») и мост событий ОС в
//! канал приложения: обработчики `tray-icon`/`muda` зовёт WndProc на потоке
//! GPUI, они только кладут событие в канал — его разбирает `on_tray`/`on_menu`
//! из задачи-читателя в `mod.rs::handle`.

use super::{autostart, badge, note_line, open_web, quit, run_check, theme, toggle_link, window, Ev, Kind, Line, Tab, TrayState, Ui, TRAY_PNG};
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

/// Трей: значок, пункт-переключатель (меняет надпись), три пункта темы
/// (отмечен ровно один) и два пункта-галочки — автозапуск и автоподключение.
pub struct Tray {
    icon: TrayIcon,
    toggle: MenuItem,
    themes: [CheckMenuItem; 3],
    autostart: CheckMenuItem,
    autoconnect: CheckMenuItem,
}

/// Выбор темы → id пункта меню и наоборот.
const THEMES: [(theme::Choice, &str, &str); 3] =
    [(theme::Choice::System, "theme:system", "Системная"), (theme::Choice::Light, "theme:light", "Светлая"), (theme::Choice::Dark, "theme:dark", "Тёмная")];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAction {
    Toggle,
    Check,
    Web,
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
    match id {
        "toggle" => Some(MenuAction::Toggle),
        "check" => Some(MenuAction::Check),
        "web" => Some(MenuAction::Web),
        "autostart" => Some(MenuAction::Autostart),
        "autoconnect" => Some(MenuAction::Autoconnect),
        "quit" => Some(MenuAction::Quit),
        _ => None,
    }
}

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
    let toggle = MenuItem::with_id("toggle", "Подключить", true, None);
    let check = MenuItem::with_id("check", "Проверить", true, None);
    let web = MenuItem::with_id("web", "Открыть веб-клиент", true, None);
    let items = THEMES.map(|(c, id, text)| CheckMenuItem::with_id(id, text, true, c == choice, None));
    let theme_refs: Vec<&dyn IsMenuItem> = items.iter().map(|i| i as &dyn IsMenuItem).collect();
    let themes_submenu = Submenu::with_items("Тема", true, &theme_refs)?;
    let autostart = CheckMenuItem::with_id("autostart", AUTOSTART_TEXT, true, autostart_on, None);
    let autoconnect = CheckMenuItem::with_id("autoconnect", "Подключаться при запуске", true, autoconnect_on, None);
    let quit = MenuItem::with_id("quit", "Выход", true, None);
    let menu = Menu::with_items(&[
        &toggle,
        &check,
        &web,
        &PredefinedMenuItem::separator(),
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
    Ok(Tray { icon: tray, toggle, themes, autostart, autoconnect })
}

impl Tray {
    /// Значок badge(state), подсказка «подключён/не подключён» (без фазы —
    /// ту двигает `set_tooltip` из тика), надпись переключателя
    /// «Отключить»/«Подключить».
    pub fn set_state(&self, state: TrayState, on: bool) {
        let _ = self.icon.set_icon(Some(icon(state)));
        let up = state == TrayState::Up;
        let _ = self.icon.set_tooltip(Some(if up { "1c-connect: подключён" } else { "1c-connect: не подключён" }));
        self.toggle.set_text(if on { "Отключить" } else { "Подключить" });
    }

    /// Подсказка трея произвольным текстом — её двигает тик приложения на
    /// фазу операции (`tray_tooltip`), не чаще `TOOLTIP_EVERY`.
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.icon.set_tooltip(Some(text));
    }

    /// Отмечает ровно один пункт подменю «Тема».
    pub fn set_theme(&self, choice: theme::Choice) {
        for (item, (c, ..)) in self.themes.iter().zip(THEMES) {
            item.set_checked(c == choice);
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
        MenuAction::Web => open_web(cx),
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
    use super::{icon_rgba, menu_action, MenuAction, TrayState};
    use crate::gui::theme;
    use crate::gui::TRAY_PNG;

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
