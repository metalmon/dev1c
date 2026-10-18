//! Открытие окна. Обычное окно Windows: своя рамка (`TitleBar` рисует
//! заголовок и кнопки — родной рамки GPUI не даёт), изменяемый размер,
//! по центру экрана.

use super::panel::Panel;
use gpui_kit::component::TitleBar;
use gpui_kit::*;
use std::sync::Arc;

/// Размер при первом открытии и наименьший допустимый — числа старого окна
/// на Tauri: они проверены на рабочем месте администратора.
pub const SIZE: Size<Pixels> = size(px(1040.0), px(800.0));
pub const MIN_SIZE: Size<Pixels> = size(px(920.0), px(660.0));

/// Открыть окно. Ручки окна и панели кладём в глобал: по ним ходят события.
pub fn open(cx: &mut App) {
    let icon = image::load_from_memory(include_bytes!("../../icons/icon.png")).ok().map(|i| Arc::new(i.to_rgba8()));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, SIZE, cx))),
        window_min_size: Some(MIN_SIZE),
        kind: WindowKind::Normal,
        is_resizable: true,
        is_minimizable: true,
        focus: true,
        show: true,
        window_decorations: Some(WindowDecorations::Client),
        icon,
        ..TitleBar::window_options()
    };
    match gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| Panel::new(window, cx))) {
        Ok((handle, panel)) => {
            let ui = cx.global_mut::<super::Ui>();
            ui.window = Some(handle);
            ui.panel = Some(panel);
        }
        // Окна нет — говорить больше некуда: журнал живёт в окне (задача 3
        // кладёт эту строку туда).
        Err(e) => eprintln!("окно не открылось: {e:#}"),
    }
}
