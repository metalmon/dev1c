//! Показ и прятание флайаута. Заглушка задачи 3: окно по центру экрана, без
//! позиционирования у трея и без пряталки по потере фокуса — это задача 4.

use super::panel::Panel;
use super::Ui;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Instant;

pub fn show_window(cx: &mut App) {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(380.0), px(560.0)), cx))),
        kind: WindowKind::PopUp,
        is_resizable: false,
        ..Default::default()
    };
    if let Ok((handle, _)) = gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| Panel::new(window, cx))) {
        cx.global_mut::<Ui>().window = handle.downcast::<Panel>();
    }
}

pub fn hide_window(window: &mut Window, cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    ui.window = None;
    ui.hidden_at = Some(Instant::now());
    window.remove_window();
}

/// Щелчок по значку при открытом окне — поднять его на передний план, а не
/// открывать заново.
pub fn toggle_window(cx: &mut App) {
    if let Some(w) = cx.global::<Ui>().window {
        let _ = w.update(cx, |_, window, _| window.activate_window());
        return;
    }
    show_window(cx);
}
