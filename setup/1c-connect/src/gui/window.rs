//! Показ и прятание флайаута: размер и зазор, угол у трея на рабочей
//! области (без наезда на панель задач), и правило «клик по значку сразу
//! после скрытия — закрыть, а не открыть снова» (окно прячется по потере
//! фокуса раньше, чем клик по трею успевает его поднять заново).

use super::panel::Panel;
use super::{note_line, run_readiness, Kind, Line, Tab, Ui};
use crate::cfg::Cfg;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Наименьший логический размер флайаута: ширина у окна постоянная, высота —
/// начальная и нижняя граница подбора по содержимому (см. `fit_height`).
/// Высота нарочно мала: окно растёт и ужимается под «Параметры», а пол
/// страхует только от вырожденного обмера.
pub const MIN_SIZE: Size<Pixels> = size(px(380.0), px(420.0));
/// Зазор до края рабочей области.
pub const GAP: Pixels = px(12.0);
/// Запас под округления дробных пикселей и перенос подвала на вторую
/// строку: без него окно и содержимое сходятся ровно в упор и подбор
/// гоняется по кругу на высоту одной строки. Неподвижная точка подбора —
/// содержимое на `SLACK` меньше видимой области.
pub const SLACK: Pixels = px(4.0);
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
    if let Some(wa) = x11_work_area() {
        return wa;
    }
    cx.primary_display().map(|d| d.bounds()).unwrap_or_default()
}

/// Рабочая область из `_NET_WORKAREA` корневого окна X11 — её выставляет сам
/// оконный менеджер (у xfwm4 это область без панели XFCE) по тому же стандарту
/// (EWMH), что форма «спрятать за панелью» ищет на Windows через
/// `SPI_GETWORKAREA`. `None` — не X11 (Wayland без XWayland, тест без дисплея)
/// или оконный менеджер свойство не выставил; тогда `work_area` падает на
/// границы всего дисплея, как было раньше. Соединение отдельное от того,
/// через которое GPUI уже говорит с сервером — короткоживущее, только на этот
/// один запрос, и закрывается в конце функции вместе с `RustConnection`.
#[cfg(target_os = "linux")]
fn x11_work_area() -> Option<Bounds<Pixels>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen_num)?.root;
    let atom = conn.intern_atom(false, b"_NET_WORKAREA").ok()?.reply().ok()?.atom;
    let reply = conn.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 4).ok()?.reply().ok()?;
    let v: Vec<u32> = reply.value32()?.collect();
    (v.len() >= 4).then(|| Bounds::new(point(px(v[0] as f32), px(v[1] as f32)), size(px(v[2] as f32), px(v[3] as f32))))
}

#[cfg(not(target_os = "linux"))]
fn x11_work_area() -> Option<Bounds<Pixels>> {
    None
}

/// Размер окна для нового показа: ширина постоянная, высота — подобранная в
/// прошлый раз (`Ui.height`), иначе наименьшая. Второй показ открывается
/// сразу нужной высоты, без скачка.
pub fn wanted_size(cx: &App) -> Size<Pixels> {
    size(MIN_SIZE.width, cx.global::<Ui>().height.unwrap_or(MIN_SIZE.height))
}

/// Высота окна под содержимое — в обе стороны: ровно на разницу между
/// содержимым и видимой областью (переполнение растит, пустота ужимает)
/// плюс запас `SLACK`, не выше рабочей области с зазорами и не ниже
/// `MIN_SIZE.height`. `None` — разница меньше пикселя, трогать окно
/// незачем.
pub fn fit_height_for(current: Pixels, content: Pixels, viewport: Pixels, work_area_h: Pixels, gap: Pixels) -> Option<Pixels> {
    let want = (current + (content - viewport) + SLACK).min(work_area_h - gap - gap).max(MIN_SIZE.height);
    ((want - current).abs() > px(1.0)).then_some(want)
}

/// Нужно ли подбирать высоту заново: впервые (`last` — `None`) или пара
/// (видимая область, содержимое) сдвинулась больше чем на 1 px по любой из
/// координат с прошлого подбора. Чистая — `Panel::measured` зовёт её перед
/// каждым обмером вместо одноразового флага: подвал, переносящийся на вторую
/// строку, меняет видимую область и без повторного подбора окно прокручивалось
/// бы на высоту этой строки.
pub fn needs_fit(last: Option<(Pixels, Pixels)>, now: (Pixels, Pixels)) -> bool {
    match last {
        None => true,
        Some((v, c)) => (now.0 - v).abs() > px(1.0) || (now.1 - c).abs() > px(1.0),
    }
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

/// То же самое на X11: своего «переставь окно» у GPUI тут тоже нет, а
/// `SetWindowPos` выше — Windows-only. Флайаут на Linux — `WindowKind::Dialog`
/// (управляемое менеджером окно, см. `show_window`: заказчик 2026-10-20 не
/// мог печатать в полях на `PopUp`/override-redirect — такие окна менеджер
/// не фокусирует вовсе), так что это обычный `ConfigureWindow` на уже
/// отображённое окно — оконный менеджер перехватывает такие запросы у себя
/// (`SubstructureRedirect` на корне) и для своих же управляемых окон отдаёт их
/// как есть, без права вето. До этой правки здесь была пустая функция: после
/// первого же `fit_height` окно меняло высоту, но не угол, и съезжало вниз на
/// разницу высот — измеренная геометрия (`380x476+2170+965` при рабочей
/// области `0,0,2560,1397`) — это именно старый угол, посчитанный под
/// `MIN_SIZE.height`, с уже выросшей высотой содержимого (дефект 2026-10-20).
/// Соединение — как в `x11_work_area`, короткоживущее и только на этот запрос.
#[cfg(target_os = "linux")]
fn move_to(window: &Window, origin: Point<Pixels>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Xcb(h) = handle.as_raw() else { return };
    let Ok((conn, _)) = x11rb::connect(None) else { return };
    let s = window.scale_factor();
    let (x, y) = ((origin.x.as_f32() * s) as i32, (origin.y.as_f32() * s) as i32);
    let aux = ConfigureWindowAux::new().x(x).y(y);
    // Одним выражением: расписка о запросе одалживает соединение, и в `if let`
    // она пережила бы его на один шаг разрушения — сборка на Linux это ловит.
    let _ = conn.configure_window(h.window.get(), &aux).map(|cookie| cookie.check());
}

#[cfg(not(any(windows, target_os = "linux")))]
fn move_to(_window: &Window, _origin: Point<Pixels>) {}

/// Подогнать высоту окна под содержимое: вырасти на переполнение или ужаться
/// на пустоту (не выше рабочей области), запомнить высоту в `Ui` — следующий
/// показ откроется сразу такой — и вернуть окно в угол у трея, потому что
/// меняется оно вниз от прежнего угла.
pub fn fit_height(window: &mut Window, cx: &mut App, content: Pixels, viewport: Pixels) {
    let wa = work_area(cx);
    let Some(h) = fit_height_for(window.bounds().size.height, content, viewport, wa.size.height, GAP) else { return };
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

/// Показ окна — событие пересчёта готовности (спека): человек открывает
/// флайаут именно затем, чтобы увидеть, как дела, а вердикт бывает
/// устаревшим. Файл параметров перечитывается здесь же и до постройки панели
/// (та читает `ui.cfg` при создании): правку toml руками иначе никто не
/// заметил бы до перезапуска. Файл не читается или сломан — остаёмся на
/// прежних параметрах молча, о них человеку уже сказали при запуске.
fn reread_and_recheck(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    if let Ok(cfg) = Cfg::load(&ui.kit) {
        ui.cfg = cfg;
    }
    run_readiness(ui);
}

/// Показать флайаут: если уже открыт — поднять на передний план, иначе
/// открыть новое окно у угла трея.
pub fn show_window(cx: &mut App) {
    reread_and_recheck(cx);
    if let Some(w) = cx.global::<Ui>().window {
        let _ = w.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let icon = image::load_from_memory(include_bytes!("../../icons/icon.png")).ok().map(|i| Arc::new(i.to_rgba8()));
    let s = wanted_size(cx);
    let wa = work_area(cx);
    let origin = flyout_origin(wa, s, GAP);
    // Диагностика наезда на панель задач (заказчик 2026-10-20): без этой
    // строки жалоба «флайаут торчит за панелью» решается только через
    // скриншот + вопрос, какая рабочая область была в тот момент — с ней
    // решается одним скриншотом журнала. Строка служебная (`Kind::Say`),
    // человеку читать незачем.
    note_line(
        cx.global_mut::<Ui>(),
        Line {
            kind: Kind::Say,
            text: format!(
                "рабочая область и угол флайаута: {}x{}+{}+{} → {},{}",
                wa.size.width.as_f32() as i32,
                wa.size.height.as_f32() as i32,
                wa.origin.x.as_f32() as i32,
                wa.origin.y.as_f32() as i32,
                origin.x.as_f32() as i32,
                origin.y.as_f32() as i32,
            ),
        },
    );
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, s))),
        titlebar: None,
        // На Windows `PopUp` и так получает фокус — ветка не трогается. На
        // Linux/X11 `PopUp` оборачивается в override-redirect с типом
        // «уведомление» (см. `move_to`): оконный менеджер такие окна вообще
        // не фокусирует, и в поля ничего не вводилось (заказчик 2026-10-20).
        // `Dialog` — управляемое окно (менеджер его фокусирует), с типом
        // `_NET_WM_WINDOW_TYPE_DIALOG`, который менеджеры обычно прячут из
        // панели задач и Alt-Tab — ровно то, что нужно треевому флайауту;
        // своей рамки у нас и так нет (`window_decorations: Client` ниже).
        kind: if cfg!(windows) { WindowKind::PopUp } else { WindowKind::Dialog },
        is_resizable: false,
        is_minimizable: false,
        focus: true,
        show: true,
        window_decorations: Some(WindowDecorations::Client),
        // Прозрачность заводили только ради скруглённых углов: иначе панель
        // рисовала их поверх квадратной opaque-поверхности. На Linux это так
        // и не получилось (композитинг XFCE/xfwm4 оставлял углы квадратными
        // с бледной рамкой, снимки заказчика 2026-10-20), и от скруглений
        // там отказались вовсе (`panel::window_radius`) — а без них и
        // прозрачность не нужна: она давала только стеклянные пятна по
        // углам. На Windows углы прежние, поэтому прозрачность остаётся.
        window_background: if cfg!(windows) {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        },
        icon,
        ..Default::default()
    };
    match gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| Panel::new(window, cx))) {
        Ok((handle, panel)) => {
            // `Dialog` на Linux — управляемое окно, и угол при самом первом
            // показе решает не наш хинт `window_bounds`, а размещение самого
            // менеджера (`WM_SIZE_HINTS` страхует только размер, не позицию);
            // `fit_height` этим же `move_to` уже доводит угол верно после
            // подгонки высоты — тем же способом поправляем и первый кадр, не
            // трогая саму формулу (`flyout_origin`/`fit_height_for`).
            #[cfg(target_os = "linux")]
            let _ = handle.update(cx, |_, window, _| move_to(window, origin));
            let ui = cx.global_mut::<Ui>();
            ui.window = Some(handle);
            ui.panel = Some(panel);
            // Окно открылось сразу на «Журнале» — строки, набежавшие пока оно
            // было спрятано, теперь на виду, бейджу их считать нечего.
            if ui.tab == Tab::Log {
                ui.unread = 0;
            }
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
    use super::{fit_height_for, flyout_origin, forget_window, needs_fit, should_show, GAP, MIN_SIZE, SLACK};
    use crate::gui::Ui;
    use gpui_kit::{point, px, size, Bounds};
    use std::time::{Duration, Instant};

    #[test]
    fn the_height_follows_the_content_both_ways_and_stays_on_the_work_area() {
        // Содержимое не влезло — вырасти на переполнение плюс запас `SLACK`.
        assert_eq!(fit_height_for(px(640.), px(700.), px(560.), px(1300.), px(12.)), Some(px(784.)));
        // Содержимого стало меньше видимой области — ужаться на разницу
        // (вкладка «Параметры» — мерная: пустота под ней и была жалобой).
        assert_eq!(fit_height_for(px(640.), px(460.), px(560.), px(1300.), px(12.)), Some(px(544.)));
        // Неподвижная точка — содержимое на `SLACK` меньше видимой области:
        // окно уже на 4 px выше содержимого, подбор больше не дёргается.
        assert_eq!(fit_height_for(px(640.), px(556.), px(560.), px(1300.), px(12.)), None);
        // Разница меньше пикселя — окно не трогаем.
        assert_eq!(fit_height_for(px(640.), px(556.5), px(560.), px(1300.), px(12.)), None);
        // Сверху — рабочая область с зазорами, снизу — наименьшая высота.
        assert_eq!(fit_height_for(px(640.), px(2000.), px(560.), px(1000.), px(12.)), Some(px(976.)));
        assert_eq!(fit_height_for(px(640.), px(100.), px(560.), px(1300.), px(12.)), Some(MIN_SIZE.height));
        assert_eq!(MIN_SIZE.height, px(420.));
        assert_eq!(SLACK, px(4.));
    }

    #[test]
    fn a_new_measure_needs_a_fit_only_when_it_moved_more_than_a_pixel() {
        // Первый обмер — всегда подбираем.
        assert!(needs_fit(None, (px(560.), px(556.))));
        // Та же пара — подбирать незачем.
        let last = Some((px(560.), px(556.)));
        assert!(!needs_fit(last, (px(560.), px(556.))));
        // Сдвиг меньше пикселя — тоже нет.
        assert!(!needs_fit(last, (px(560.5), px(556.))));
        // Сдвиг на 2 px по видимой области — подбираем заново.
        assert!(needs_fit(last, (px(562.), px(556.))));
        // Сдвиг на 2 px по содержимому — тоже.
        assert!(needs_fit(last, (px(560.), px(558.))));
    }

    #[test]
    fn the_flyout_sits_in_the_corner_and_never_leaves_the_work_area() {
        let wa = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(700.)));
        assert_eq!(flyout_origin(wa, MIN_SIZE, GAP), point(px(608.), px(268.)));
        let tiny = Bounds::new(point(px(100.), px(50.)), size(px(300.), px(300.)));
        assert_eq!(flyout_origin(tiny, MIN_SIZE, GAP), point(px(100.), px(50.)));
        // Дефект 1: рабочая область меньше дисплея (панель XFCE внизу срезает
        // 40 px из 1080) и содержимое выше, чем осталось свободного места —
        // высота подбирается под реальную рабочую область, и окно целиком
        // остаётся внутри неё, а не торчит нижним краем за экран.
        let shrunk = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1040.)));
        let h = fit_height_for(MIN_SIZE.height, px(1200.), px(560.), shrunk.size.height, GAP).unwrap();
        let origin = flyout_origin(shrunk, size(MIN_SIZE.width, h), GAP);
        assert!(origin.y + h <= shrunk.size.height, "нижний край окна за рабочей областью");
        assert_eq!(origin.y, px(12.));
        // Та же высота содержимого, но по ошибке — на границах всего дисплея
        // (1080 без вычета панели, ровно старый баг): низ окна проезжает за
        // настоящую рабочую область (1040) — учёт панели в `work_area` и
        // исправляет именно это.
        let whole_display = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)));
        let h_wrong = fit_height_for(MIN_SIZE.height, px(1200.), px(560.), whole_display.size.height, GAP).unwrap();
        let origin_wrong = flyout_origin(whole_display, size(MIN_SIZE.width, h_wrong), GAP);
        assert!(origin_wrong.y + h_wrong > shrunk.size.height, "без учёта панели низ окна должен был уйти под неё");

        // То же самое на живых числах заказчика 2026-10-20: ноутбук
        // 2560x1440 с панелью XFCE внизу, `_NET_WORKAREA` = 2560x1397. Для
        // любой высоты содержимого — от меньше наименьшей до намного больше
        // рабочей области — ни низ, ни правый край окна не уходят за рабочую
        // область с зазором. `fit_height_for` вернёт `None`, когда искомая
        // высота совпала с текущей (420, `MIN_SIZE.height`) — тогда она и
        // есть нужная, `unwrap_or` восстанавливает её для проверки.
        let laptop_wa = Bounds::new(point(px(0.), px(0.)), size(px(2560.), px(1397.)));
        for content in [px(100.), px(460.), px(1200.), px(1397.), px(5000.)] {
            let h = fit_height_for(MIN_SIZE.height, content, px(560.), laptop_wa.size.height, GAP).unwrap_or(MIN_SIZE.height);
            let origin = flyout_origin(laptop_wa, size(MIN_SIZE.width, h), GAP);
            assert!(
                origin.y + h <= laptop_wa.size.height - GAP,
                "низ флайаута за рабочей областью: содержимое {content:?}, угол {origin:?}, высота {h:?}"
            );
            assert!(
                origin.x + MIN_SIZE.width <= laptop_wa.size.width - GAP,
                "правый край флайаута за рабочей областью: содержимое {content:?}, угол {origin:?}"
            );
        }
        // Измеренный на живом окне случай дефекта: содержимое чуть меньше
        // видимой области (460 при 560) держит окно на `MIN_SIZE.height` —
        // угол 965 по Y это и даёт (`1397 - 420 - 12`), ровно то число,
        // которое показал `xwininfo` до того, как `fit_height` сдвинул угол
        // следом за выросшей высотой.
        let h_min = fit_height_for(MIN_SIZE.height, px(460.), px(560.), laptop_wa.size.height, GAP);
        assert_eq!(h_min, None, "высота уже на полу — подбирать нечего");
        assert_eq!(flyout_origin(laptop_wa, MIN_SIZE, GAP).y, px(965.));
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
