//! Шрифты Zed (IBM Plex Sans / Lilex) и палитра macOS Classic для окна.
//! Выбор темы (система/светлая/тёмная) хранится в комплекте и переживает
//! перезапуск программы.

use common::kit::Kit;
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::App;
use std::borrow::Cow;

pub const LIGHT: &str = "macOS Classic Light";
pub const DARK: &str = "macOS Classic Dark";
pub const THEME_JSON: &str = include_str!("../../theme/macos-classic.json");
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

/// Применяет выбор: `System` подхватывает палитру Windows, `Light`/`Dark` — явно.
pub fn apply(choice: Choice, cx: &mut App) {
    match choice {
        Choice::System => Theme::sync_system_appearance(None, cx),
        Choice::Light => Theme::change(ThemeMode::Light, None, cx),
        Choice::Dark => Theme::change(ThemeMode::Dark, None, cx),
    }
}

/// Шрифты и палитра окна. Звать из `launch` сразу после `gpui_kit::init`.
pub fn install(kit: &Kit, cx: &mut App) {
    let _ = cx.text_system().add_fonts(vec![
        Cow::Borrowed(include_bytes!("../../fonts/IBMPlexSans-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../fonts/IBMPlexSans-SemiBold.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../fonts/Lilex-Regular.ttf").as_slice()),
    ]);
    let reg = ThemeRegistry::global_mut(cx);
    reg.load_themes_from_str(THEME_JSON).expect("theme json");
    let (light, dark) = (reg.themes()[LIGHT].clone(), reg.themes()[DARK].clone());
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    apply(Choice::load(kit), cx);
}

#[cfg(test)]
mod tests {
    use super::{Choice, DARK, LIGHT, THEME_JSON};
    use common::kit::Kit;
    use gpui_kit::component::ThemeSet;

    #[test]
    fn theme_file_has_one_light_and_one_dark_theme_with_the_fluent_radii() {
        let set: ThemeSet = serde_json::from_str(THEME_JSON).unwrap();
        let names: Vec<_> = set.themes.iter().map(|t| (t.name.to_string(), t.mode.is_dark())).collect();
        assert_eq!(names, vec![(DARK.into(), true), (LIGHT.into(), false)]);
        for t in &set.themes {
            assert_eq!((t.radius, t.radius_lg, t.shadow), (Some(4), Some(8), Some(false)));
            assert_eq!(t.font_family.as_deref(), Some("IBM Plex Sans"));
            assert!(t.colors.primary.is_some() && t.colors.ring.is_some());
        }
    }

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
}
