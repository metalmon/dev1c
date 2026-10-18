//! Шрифт Zed (IBM Plex Sans) и палитра 1С:Такси — данные, общие для двух
//! окон комплекта. Поведение (выбор темы, тема меню трея) остаётся там, где
//! оно нужно: это модуль данных, не слой над GPUI.

use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::App;
use std::borrow::Cow;

pub const LIGHT: &str = "1C Light";
pub const DARK: &str = "1C Dark";
pub const THEME_JSON: &str = include_str!("../theme/1c.json");

/// Шрифт окна в текстовую систему. Звать из `launch` сразу после
/// `gpui_kit::init`, до первого окна.
pub fn fonts(cx: &mut App) {
    let _ = cx.text_system().add_fonts(vec![
        Cow::Borrowed(include_bytes!("../fonts/IBMPlexSans-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf").as_slice()),
    ]);
}

/// Палитра: обе темы из вшитого JSON становятся светлой и тёмной темой
/// приложения. Какую из них показывать, решает тот, кто зовёт (`Theme::change`).
///
/// Паника здесь намеренна: JSON вшит в двоичный файл на сборке, и его разбор
/// с обоими именами тем проверяет тест ниже — сломаться это может только при
/// правке самого файла, и тогда лучше сразу и громко.
pub fn palette(cx: &mut App) {
    let reg = ThemeRegistry::global_mut(cx);
    reg.load_themes_from_str(THEME_JSON).expect("theme json");
    let (light, dark) = (reg.themes()[LIGHT].clone(), reg.themes()[DARK].clone());
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
}

#[cfg(test)]
mod tests {
    use super::{DARK, LIGHT, THEME_JSON};
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
            assert_eq!(t.colors.primary.as_deref(), Some("#FACC1F"));
        }
    }
}
