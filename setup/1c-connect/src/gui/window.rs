//! Показ и прятание флайаута: размер и зазор, угол у трея на рабочей
//! области (без наезда на панель задач), и правило «клик по значку сразу
//! после скрытия — закрыть, а не открыть снова» (окно прячется по потере
//! фокуса раньше, чем клик по трею успевает его поднять заново).

use super::panel::Panel;
use super::Ui;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Логический размер флайаута.
pub const SIZE: Size<Pixels> = size(px(380.0), px(640.0));
/// Зазор до края рабочей области.
pub const GAP: Pixels = px(12.0);
/// Окно только что спряталось по потере фокуса — клик по трею в этом окне
/// закрывает, а не открывает заново.
pub const JUST_HIDDEN: Duration = Duration::from_millis(400);

/// Правый нижний угол рабочей области, но не левее/выше её собственного
/// начала — на маленькой или смещённой рабочей области окно не вылезает за
/// край.
pub fn flyout_origin(work_area: Bounds<Pixels>, size: Size<Pixels>, gap: Pixels) -> Point<Pixels> {
    let x = (work_area.origin.x + work_area.size.width - size.width - gap).max(work_area.origin.x);
    let y = (work_area.origin.y + work_area.size.height - size.height - gap).max(work_area.origin.y);
    point(x, y)
}

/// Рабочая область монитора (без панели задач). На Windows — через
/// `SPI_GETWORKAREA` с поправкой на DPI; иначе — границы основного дисплея
/// целиком.
pub fn work_area(cx: &App) -> Bounds<Pixels> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA};
        let mut r: RECT = std::mem::zeroed();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut _ as *mut _, 0) != 0 {
            let s = GetDpiForSystem() as f32 / 96.0;
            return Bounds::new(
                point(px(r.left as f32 / s), px(r.top as f32 / s)),
                size(px((r.right - r.left) as f32 / s), px((r.bottom - r.top) as f32 / s)),
            );
        }
    }
    cx.primary_display().map(|d| d.bounds()).unwrap_or_default()
}

/// `false` сразу после того, как окно спряталось по потере фокуса — в этот
/// момент клик по значку трея, который и вызвал потерю фокуса, не должен
/// поднимать окно обратно.
pub fn should_show(hidden_at: Option<Instant>, now: Instant) -> bool {
    match hidden_at {
        Some(t) => now.saturating_duration_since(t) >= JUST_HIDDEN,
        None => true,
    }
}

/// Показать флайаут: если уже открыт — поднять на передний план, иначе
/// открыть новое окно у угла трея.
pub fn show_window(cx: &mut App) {
    if let Some(w) = cx.global::<Ui>().window {
        let _ = w.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let icon = image::load_from_memory(include_bytes!("../../icons/icon.png")).ok().map(|i| Arc::new(i.to_rgba8()));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(flyout_origin(work_area(cx), SIZE, GAP), SIZE))),
        titlebar: None,
        kind: WindowKind::PopUp,
        is_resizable: false,
        is_minimizable: false,
        focus: true,
        show: true,
        window_decorations: Some(WindowDecorations::Client),
        icon,
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

/// Щелчок по значку трея: показать, если это не отражение собственного
/// только что случившегося скрытия (см. `should_show`).
pub fn toggle_window(cx: &mut App) {
    if should_show(cx.global::<Ui>().hidden_at, Instant::now()) {
        show_window(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{flyout_origin, should_show, GAP, SIZE};
    use gpui_kit::{point, px, size, Bounds};
    use std::time::{Duration, Instant};

    #[test]
    fn the_flyout_sits_in_the_corner_and_never_leaves_the_work_area() {
        let wa = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(700.)));
        assert_eq!(flyout_origin(wa, SIZE, GAP), point(px(608.), px(48.)));
        let tiny = Bounds::new(point(px(100.), px(50.)), size(px(300.), px(300.)));
        assert_eq!(flyout_origin(tiny, SIZE, GAP), point(px(100.), px(50.)));
    }

    #[test]
    fn a_click_right_after_hiding_means_close_not_reopen() {
        let now = Instant::now();
        assert!(should_show(None, now));
        assert!(!should_show(Some(now - Duration::from_millis(100)), now));
        assert!(should_show(Some(now - Duration::from_millis(900)), now));
    }
}
