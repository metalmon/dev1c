//! Показ и прятание флайаута: размер и зазор, угол у трея на рабочей
//! области (без наезда на панель задач), и правило «клик по значку сразу
//! после скрытия — закрыть, а не открыть снова» (окно прячется по потере
//! фокуса раньше, чем клик по трею успевает его поднять заново).

use super::panel::Panel;
use super::{note_line, Kind, Line, Ui};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Наименьший логический размер флайаута: ширина у окна постоянная, высота —
/// начальная и нижняя граница подбора по содержимому (см. `fit_height`).
pub const MIN_SIZE: Size<Pixels> = size(px(380.0), px(640.0));
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

/// Размер окна для нового показа: ширина постоянная, высота — подобранная в
/// прошлый раз (`Ui.height`), иначе наименьшая. Второй показ открывается
/// сразу нужной высоты, без скачка.
pub fn wanted_size(cx: &App) -> Size<Pixels> {
    size(MIN_SIZE.width, cx.global::<Ui>().height.unwrap_or(MIN_SIZE.height))
}

/// Насколько вырастить окно, чтобы содержимое влезло целиком: ровно на
/// переполнение, но не выше рабочей области с зазорами по краям. `None` —
/// содержимое уже помещается, трогать окно незачем.
pub fn grown_height(current: Pixels, content: Pixels, viewport: Pixels, work_area_h: Pixels, gap: Pixels) -> Option<Pixels> {
    if content <= viewport {
        return None;
    }
    Some((current + (content - viewport)).min(work_area_h - gap - gap))
}

/// Переставить окно левым верхним углом в точку (логические px). GPUI умеет
/// менять только размер окна, причём растит его вниз от прежнего угла, —
/// положение правим сами, на Windows через `SetWindowPos` по HWND окна.
#[cfg(windows)]
fn move_to(window: &Window, origin: Point<Pixels>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let s = window.scale_factor();
    let (x, y) = ((origin.x.as_f32() * s) as i32, (origin.y.as_f32() * s) as i32);
    unsafe {
        SetWindowPos(h.hwnd.get() as *mut _, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

#[cfg(not(windows))]
fn move_to(_window: &Window, _origin: Point<Pixels>) {}

/// Подогнать высоту окна под содержимое: вырасти ровно на переполнение (не
/// выше рабочей области), запомнить высоту в `Ui` — следующий показ откроется
/// сразу такой — и вернуть окно в угол у трея, потому что растёт оно вниз.
pub fn fit_height(window: &mut Window, cx: &mut App, content: Pixels, viewport: Pixels) {
    let wa = work_area(cx);
    let Some(h) = grown_height(window.bounds().size.height, content, viewport, wa.size.height, GAP) else { return };
    let s = size(MIN_SIZE.width, h);
    cx.global_mut::<Ui>().height = Some(h);
    window.resize(s);
    move_to(window, flyout_origin(wa, s, GAP));
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
    let s = wanted_size(cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(flyout_origin(work_area(cx), s, GAP), s))),
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
    match gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| Panel::new(window, cx))) {
        Ok((handle, panel)) => {
            let ui = cx.global_mut::<Ui>();
            ui.window = Some(handle);
            ui.panel = Some(panel);
        }
        Err(e) => note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: format!("окно не открылось: {e:#}") }),
    }
}

/// Окна больше нет: обе ручки забываются вместе — иначе `show_window`
/// поднимал бы закрытое окно, а журнал считал бы строки прочитанными.
pub fn forget_window(ui: &mut Ui) {
    ui.window = None;
    ui.panel = None;
    ui.hidden_at = Some(Instant::now());
}

pub fn hide_window(window: &mut Window, cx: &mut App) {
    forget_window(cx.global_mut::<Ui>());
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
    use super::{flyout_origin, forget_window, grown_height, should_show, GAP, MIN_SIZE};
    use crate::gui::Ui;
    use gpui_kit::{point, px, size, Bounds};
    use std::time::{Duration, Instant};

    #[test]
    fn grown_height_grows_by_the_overflow_and_stops_at_the_work_area() {
        assert_eq!(grown_height(px(640.), px(700.), px(560.), px(1300.), px(12.)), Some(px(780.)));
        assert_eq!(grown_height(px(640.), px(500.), px(560.), px(1300.), px(12.)), None);
        assert_eq!(grown_height(px(640.), px(2000.), px(560.), px(1000.), px(12.)), Some(px(976.)));
    }

    #[test]
    fn the_flyout_sits_in_the_corner_and_never_leaves_the_work_area() {
        let wa = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(700.)));
        assert_eq!(flyout_origin(wa, MIN_SIZE, GAP), point(px(608.), px(48.)));
        let tiny = Bounds::new(point(px(100.), px(50.)), size(px(300.), px(300.)));
        assert_eq!(flyout_origin(tiny, MIN_SIZE, GAP), point(px(100.), px(50.)));
    }

    #[test]
    fn a_click_right_after_hiding_means_close_not_reopen() {
        let now = Instant::now();
        assert!(should_show(None, now));
        assert!(!should_show(Some(now - Duration::from_millis(100)), now));
        assert!(should_show(Some(now - Duration::from_millis(900)), now));
    }

    #[test]
    fn hiding_forgets_both_handles_and_starts_the_grace_window() {
        let mut ui = Ui::for_test();
        ui.hidden_at = None;
        forget_window(&mut ui);
        assert!(ui.window.is_none(), "ручка окна");
        assert!(ui.panel.is_none(), "сущность панели");
        assert!(ui.hidden_at.is_some(), "щелчок по трею сразу после — закрыть, а не открыть");
    }
}
