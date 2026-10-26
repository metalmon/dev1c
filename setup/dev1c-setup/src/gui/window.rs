//! Открытие окна. Обычное окно Windows: своя рамка (`TitleBar` рисует
//! заголовок и кнопки — родной рамки GPUI не даёт), изменяемый размер,
//! по центру экрана.

use super::panel::Panel;
use gpui_kit::component::TitleBar;
use gpui_kit::*;
use std::sync::Arc;

/// Размер при первом открытии — почти квадрат (решение заказчика
/// 2026-10-19): так поля ввода не растягиваются на всю ширину, а «Параметры»
/// помещаются без прокрутки. Наименьший размер меньше: окно можно ужать.
pub const SIZE: Size<Pixels> = size(px(940.0), px(960.0));
pub const MIN_SIZE: Size<Pixels> = size(px(760.0), px(560.0));

/// Поля вокруг окна при открытии: столько остаётся до краёв рабочей области.
/// Немного: окно и так почти квадратное, а лишний отступ отъедает высоту и
/// возвращает прокрутку на «Параметрах».
const MARGIN: f32 = 24.0;

/// `SPI_GETWORKAREA` с поправкой на DPI; иначе — границы основного дисплея
/// целиком. Копия помощника из `1c-connect`: панель задач и масштаб экрана
/// иначе съедают низ окна.
fn work_area(cx: &App) -> Bounds<Pixels> {
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

/// Запас под содержимым, чтобы последняя строка не липла к подвалу.
const SLACK: f32 = 8.0;

/// Нужно ли мерить заново: впервые или пара (видимая область, содержимое)
/// сдвинулась больше чем на пиксель.
pub fn needs_fit(last: Option<(Pixels, Pixels)>, now: (Pixels, Pixels)) -> bool {
    match last {
        None => true,
        Some((v, c)) => (now.0 - v).abs() > px(1.0) || (now.1 - c).abs() > px(1.0),
    }
}

/// Подогнать высоту окна под содержимое вкладки «Параметры». Возвращает
/// высоту, которую поставили, — панель её помнит и по ней понимает, что
/// высоту потом изменил человек, а не она сама. Счёт — в `crate::ui`.
pub fn fit_height(window: &mut Window, cx: &mut App, content: Pixels, viewport: Pixels) -> Option<Pixels> {
    let wa = work_area(cx);
    let bounds = window.bounds();
    let h = crate::ui::fit_height_at(
        f32::from(bounds.size.height),
        f32::from(content),
        f32::from(viewport),
        f32::from(wa.size.height),
        f32::from(bounds.origin.y - wa.origin.y),
        MARGIN,
        SLACK,
    )?;
    window.resize(size(bounds.size.width, px(h)));
    Some(px(h))
}

/// Где и какого размера открыть окно: по центру рабочей области и не больше
/// неё — счёт размера с тестом живёт в `crate::ui::window_size`.
fn placement(cx: &App) -> Bounds<Pixels> {
    let wa = work_area(cx);
    let (w, h) = crate::ui::window_size(
        (f32::from(SIZE.width), f32::from(SIZE.height)),
        (f32::from(wa.size.width), f32::from(wa.size.height)),
        MARGIN,
    );
    let s = size(px(w), px(h));
    let origin = point(wa.origin.x + (wa.size.width - s.width) / 2., wa.origin.y + (wa.size.height - s.height) / 2.);
    Bounds::new(origin, s)
}

/// Открыть окно. Ручки окна и панели кладём в глобал: по ним ходят события.
pub fn open(cx: &mut App) {
    let icon = image::load_from_memory(include_bytes!("../../icons/icon.png")).ok().map(|i| Arc::new(i.to_rgba8()));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(placement(cx))),
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
            {
                let ui = cx.global_mut::<super::Ui>();
                ui.window = Some(handle);
                ui.panel = Some(panel);
            }
            // Закрытие во время работы спрашивает: повторный запуск
            // доустановит недостающее, но полуготовый контейнер случайным
            // щелчком плодить не надо (решение заказчика 2026-10-19).
            let _ = handle.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, |_, cx| {
                    if crate::ui::should_close(cx.global::<super::Ui>().working()) {
                        return true;
                    }
                    ask_interrupt()
                });
            });
        }
        // Окна нет — журнал тоже негде показать, а консоли у релизной сборки
        // не бывает. Молчать нельзя: процесс остался бы висеть без единого
        // окна. Системное сообщение и выход.
        Err(e) => {
            let text = format!("Окно не открылось: {e:#}\nПрограмма работает и из командной строки: dev1c-setup status.");
            eprintln!("{text}");
            say_and_quit(&text, cx);
        }
    }
}

/// Спросить, прерывать ли работу. Системное окно, а не своё: вопрос задаётся
/// из обработчика закрытия, где своей разметке взяться негде, и ответ нужен
/// сразу. «Да» — закрываем, «Нет» — остаёмся.
#[cfg(windows)]
fn ask_interrupt() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let text = wide("Работа не закончена. Прервать?\nПовторный запуск доустановит недостающее.");
    let title = wide("dev1c-setup");
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONWARNING) == IDYES }
}

#[cfg(not(windows))]
fn ask_interrupt() -> bool {
    // Не под Windows окна у этой программы нет вовсе (сборка без `gui`).
    true
}

/// Сказать и выйти: окна нет, значит ни журнала, ни кнопки «закрыть» —
/// процесс обязан закончиться, а не висеть невидимым.
#[cfg_attr(not(windows), allow(unused_variables))]
fn say_and_quit(text: &str, cx: &mut App) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        let (text, title) = (wide(text), wide("dev1c-setup"));
        unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
        }
    }
    cx.quit();
}
