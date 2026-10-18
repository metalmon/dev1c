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
