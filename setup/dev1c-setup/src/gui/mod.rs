//! Окно: тонкая оболочка над теми же операциями и проверками, что и
//! командная строка. Логики развёртывания здесь нет намеренно.
//!
//! Чистые решения окна (что говорит карточка состояния, какая вкладка
//! открывается, что несёт плашка) живут в `crate::ui` — там нет GPUI, и их
//! тесты идут в сборке без окна. Здесь — состояние, события и вид.

pub mod panel;
pub mod window;

use crate::cfg::Cfg;
use common::kit::Kit;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{AnyWindowHandle, Entity, Global};

/// Состояние окна целиком. Глобал GPUI: один на приложение.
pub struct Ui {
    pub kit: Kit,
    pub cfg: Cfg,
    pub window: Option<AnyWindowHandle>,
    pub panel: Option<Entity<panel::Panel>>,
}
impl Global for Ui {}

/// Окно без аргументов: двойной щелчок по файлу из комплекта.
pub fn launch(kit: Kit) -> anyhow::Result<()> {
    let cfg = Cfg::load(&kit)?;
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        common::gui::fonts(cx);
        common::gui::palette(cx);
        // Одна тема, светлая: выбора тем у этой программы нет.
        Theme::change(ThemeMode::Light, None, cx);
        cx.set_global(Ui { kit, cfg, window: None, panel: None });
        window::open(cx);
    });
    Ok(())
}
