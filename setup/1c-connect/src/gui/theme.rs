//! Выбор темы окна: система/светлая/тёмная, хранится в комплекте и
//! переживает перезапуск программы. Сами шрифт и палитра — общие данные
//! комплекта, они живут в `common::gui`.

use common::kit::Kit;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::App;

pub const STATE_KEY: &str = "theme";

/// Выбор темы: система или явно светлая/тёмная.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Choice {
    #[default]
    System,
    Light,
    Dark,
}

impl Choice {
    pub fn parse(s: &str) -> Choice {
        match s {
            "light" => Choice::Light,
            "dark" => Choice::Dark,
            _ => Choice::System,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Choice::System => "system",
            Choice::Light => "light",
            Choice::Dark => "dark",
        }
    }

    pub fn load(kit: &Kit) -> Choice {
        kit.state_get(STATE_KEY).map(|s| Choice::parse(&s)).unwrap_or_default()
    }

    pub fn store(self, kit: &Kit) -> anyhow::Result<()> {
        kit.state_set(STATE_KEY, self.as_str())
    }
}

/// Тема контекстного меню трея: Win32 не знает «как система», ему нужен
/// прямой ответ.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuMode {
    Light,
    Dark,
}

/// Какой теме следовать меню при выборе `choice` и системной теме `system_dark`.
pub fn menu_mode(choice: Choice, system_dark: bool) -> MenuMode {
    match choice {
        Choice::Light => MenuMode::Light,
        Choice::Dark => MenuMode::Dark,
        Choice::System if system_dark => MenuMode::Dark,
        Choice::System => MenuMode::Light,
    }
}

/// Перекрашивает меню трея: его рисует Win32 мимо GPUI, и светлым на тёмном
/// рабочем столе оно выглядит чужим.
///
/// API недокументирован: `uxtheme.dll` отдаёт `SetPreferredAppMode` (ординал
/// 135) и `FlushMenuThemes` (136) только по номеру, без имён; так к ним ходят
/// Проводник, Notepad++ и wxWidgets. Появились в Windows 10 1903 — на более
/// старых системах ординалов нет, и тогда молча ничего не делаем.
#[cfg(windows)]
pub fn apply_menu_theme(mode: MenuMode) {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    const FORCE_DARK: i32 = 2;
    const FORCE_LIGHT: i32 = 3;

    let name: Vec<u16> = "uxtheme.dll\0".encode_utf16().collect();
    unsafe {
        let lib = LoadLibraryW(name.as_ptr());
        if lib.is_null() {
            return;
        }
        // Как `MAKEINTRESOURCE`: номер ординала лежит в младшем слове
        // «указателя» на имя.
        let (Some(set), Some(flush)) = (GetProcAddress(lib, 135 as *const u8), GetProcAddress(lib, 136 as *const u8)) else {
            return;
        };
        let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set);
        let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
        set(match mode {
            MenuMode::Dark => FORCE_DARK,
            MenuMode::Light => FORCE_LIGHT,
        });
        flush();
    }
}

#[cfg(not(windows))]
pub fn apply_menu_theme(_mode: MenuMode) {}

/// Применяет выбор: `System` подхватывает палитру Windows, `Light`/`Dark` — явно.
pub fn apply(choice: Choice, cx: &mut App) {
    match choice {
        Choice::System => Theme::sync_system_appearance(None, cx),
        Choice::Light => Theme::change(ThemeMode::Light, None, cx),
        Choice::Dark => Theme::change(ThemeMode::Dark, None, cx),
    }
    // Читаем уже применённый режим: для `System` он и есть ответ системы.
    apply_menu_theme(menu_mode(choice, Theme::global(cx).is_dark()));
}

/// Шрифты, палитра и выбранная тема. Звать из `launch` сразу после
/// `gpui_kit::init`.
pub fn install(kit: &Kit, cx: &mut App) {
    common::gui::fonts(cx);
    common::gui::palette(cx);
    apply(Choice::load(kit), cx);
}

#[cfg(test)]
mod tests {
    use super::{menu_mode, Choice, MenuMode};
    use common::kit::Kit;

    #[test]
    fn theme_choice_round_trips_through_kit_state_and_defaults_to_system() {
        let dir = std::env::temp_dir().join(format!("onec-connect-theme-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let kit = Kit::at(&dir);
        assert_eq!(Choice::load(&kit), Choice::System);
        Choice::Dark.store(&kit).unwrap();
        assert_eq!(Choice::load(&kit), Choice::Dark);
        assert_eq!(Choice::parse("garbage"), Choice::System);
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn menu_mode_follows_the_choice_and_the_system_only_when_asked() {
        assert_eq!(menu_mode(Choice::Light, true), MenuMode::Light);
        assert_eq!(menu_mode(Choice::Dark, false), MenuMode::Dark);
        assert_eq!(menu_mode(Choice::System, true), MenuMode::Dark);
        assert_eq!(menu_mode(Choice::System, false), MenuMode::Light);
    }
}
