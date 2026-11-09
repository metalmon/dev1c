//! Показ и прятание флайаута: размер и зазор, угол у трея на рабочей
//! области (без наезда на панель задач). Окном владеет человек: решение
//! владельца 1.11 — окно больше не прячется само по потере фокуса, закрывает
//! его только явное действие (крестик, Esc, трей).

use super::panel::Panel;
use super::{note_file, note_line, run_readiness, Kind, Line, Tab, Ui};
use crate::cfg::Cfg;
use crate::ops_host;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;

/// Наименьший логический размер флайаута: ширина у окна постоянная, высота —
/// нижняя граница (`height_on`). Пол страхует от вырожденного обмера экрана:
/// высота считается от рабочей области, и если та измерилась неправдоподобно
/// малой, окно всё равно остаётся окном, а не щелью в три строки.
pub const MIN_SIZE: Size<Pixels> = size(px(380.0), px(420.0));
/// Зазор до края рабочей области.
pub const GAP: Pixels = px(12.0);
/// Какую долю высоты рабочей области занимает окно.
///
/// Почему именно 0,9. Высота больше не подбирается под содержимое (решение
/// владельца 9.11: «пусть сразу таким высоким открывается, от высоты экрана
/// пусть считает, что не лезет — то в прокрутку»), значит одно число должно
/// подойти всем состояниям сразу. На живой машине владельца (рабочая
/// область 1104 логических точки, журнал окна) прежний подбор давал 832
/// точки на «Всё на одной машине» и 1029 на «Принимаю подключения» — тот
/// самый скачок на четверть экрана. 0,9 (993 точки) покрывает высокое из
/// состояний почти целиком: прокрутка появляется только у самого длинного, и
/// сразу видно, что она есть. Брать всю область нельзя — окно вплотную к
/// краям экрана читается как панель, а не как флайаут у трея; меньше 0,8
/// значит отправлять в прокрутку и короткую локальную карточку, то есть
/// вернуть жалобу с другой стороны.
pub const SCREEN_SHARE: f32 = 0.9;

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

/// Высота окна по высоте рабочей области: доля `SCREEN_SHARE`, но не за край
/// области (с зазорами по обе стороны) и не ниже `MIN_SIZE.height`.
///
/// Содержимого функция не знает — и в этом вся суть правки: высота окна
/// перестала зависеть от того, что внутри. До 9.11 её подбирал
/// `fit_height_for` по обмеру «Связи», и переключение «Это соединение» с «Всё
/// на одной машине» на «Принимаю подключения» растило окно рывком на четверть
/// высоты экрана (жалоба тянулась с 19.10). Теперь высота считается один раз,
/// при открытии окна (`show_window`), из той же рабочей области, по которой
/// считается угол флайаута, а всё, что не влезло, уезжает в прокрутку
/// содержимого вкладки.
///
/// Пол сильнее потолка нарочно: на вырожденно малой области (обмер экрана
/// соврал) лучше окно, торчащее за край, чем нечитаемая щель.
pub fn height_on(work_area_h: Pixels) -> Pixels {
    (work_area_h * SCREEN_SHARE).min(work_area_h - GAP - GAP).max(MIN_SIZE.height)
}

/// Переставить окно левым верхним углом в точку (логические px) — только на
/// Linux. Своего «переставь окно» у GPUI нет, но на Windows его и не нужно:
/// угол из `window_bounds` менеджер окон соблюдает, а второго случая не
/// осталось — высота с 9.11 не меняется после показа, и возвращать окно в
/// угол после подгонки больше незачем. Windows-версия этой функции
/// (`SetWindowPos` по HWND) здесь была и уехала вместе с подгонкой: без
/// вызова она стала мёртвым кодом.
///
/// Флайаут на Linux — `WindowKind::Dialog`
/// (управляемое менеджером окно, см. `show_window`: заказчик 2026-10-20 не
/// мог печатать в полях на `PopUp`/override-redirect — такие окна менеджер
/// не фокусирует вовсе), так что это обычный `ConfigureWindow` на уже
/// отображённое окно — оконный менеджер перехватывает такие запросы у себя
/// (`SubstructureRedirect` на корне) и для своих же управляемых окон отдаёт их
/// как есть, без права вето. До этой правки здесь была пустая функция: после
/// первого же подбора высоты (того самого, которого с 9.11 больше нет) окно
/// меняло высоту, но не угол, и съезжало вниз на разницу высот — измеренная геометрия (`380x476+2170+965` при рабочей
/// области `0,0,2560,1397`) — это именно старый угол, посчитанный под
/// `MIN_SIZE.height`, с уже выросшей высотой содержимого (дефект 2026-10-20).
/// Соединение — как в `x11_work_area`, короткоживущее и только на этот запрос.
#[cfg(target_os = "linux")]
fn move_to(window: &Window, origin: Point<Pixels>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
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

/// Имя окну и просьба убрать его из панели задач и переключателя окон.
///
/// Две недоделки в одной картинке (заказчик 2026-10-21: окно висит в панели
/// задач и зовётся «безымянное окно»), и чинятся они вместе.
///
/// **Имени не было вовсе.** Своей рамки мы не рисуем (`titlebar: None`,
/// `window_decorations: Client`), и вместе с рамкой пропал заголовок:
/// `WM_NAME`/`_NET_WM_NAME` окну никто не задаёт, а менеджер подставляет своё
/// «безымянное окно». Задаём свойства сами — решение про рамку при этом не
/// трогается: заголовок окна это свойство, а не полоска сверху.
///
/// **Его там и быть не должно.** `WindowKind::Dialog` выбран ровно затем,
/// чтобы менеджер прятал окно из панели и Alt-Tab по типу
/// (`_NET_WM_WINDOW_TYPE_DIALOG`), но XFCE в образе показывает его всё
/// равно. На одном типе окна полагаться нельзя — просим явно
/// `_NET_WM_STATE_SKIP_TASKBAR` и `_NET_WM_STATE_SKIP_PAGER`.
///
/// Почему состояние меняется сообщением, а не записью свойства: окно к этому
/// времени уже отображено, а у отображённого окна `_NET_WM_STATE` принадлежит
/// менеджеру — по EWMH его просят `ClientMessage`'ом в корень. Имя — обычная
/// запись, и в обеих формах: `WM_NAME` (старые панели) и `_NET_WM_NAME`
/// (UTF-8, его читают современные).
///
/// Соединение короткоживущее и только на эти запросы — как в `move_to` и
/// `x11_work_area`. Молча: окно без имени в панели задач неприятно, но
/// работать не мешает, и срывать показ из-за него нельзя.
#[cfg(target_os = "linux")]
fn name_and_hide_from_taskbar(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode};
    // `change_property8` живёт в своём трейте-обёртке, а не в том же, что
    // `intern_atom`/`send_event`: без этой строки Linux-сборка не найдёт
    // метод (на Windows этот код не компилируется вовсе, и промах вылез бы
    // только в CI — проверено по исходникам x11rb 0.13.2).
    use x11rb::wrapper::ConnectionExt as _;
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Xcb(h) = handle.as_raw() else { return };
    let win = h.window.get();
    let Ok((conn, screen_num)) = x11rb::connect(None) else { return };
    let Some(root) = conn.setup().roots.get(screen_num).map(|s| s.root) else { return };
    let atom = |name: &[u8]| conn.intern_atom(false, name).ok().and_then(|c| c.reply().ok()).map(|r| r.atom);
    // Имя окна в панели задач. Внутри функции, а не константой модуля: на
    // Windows её некому прочитать, и сборка справедливо ругалась бы на
    // мёртвый код.
    const WM_TITLE: &[u8] = b"1c-connect";

    let _ = conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, WM_TITLE).map(|c| c.check());
    if let (Some(net_name), Some(utf8)) = (atom(b"_NET_WM_NAME"), atom(b"UTF8_STRING")) {
        let _ = conn.change_property8(PropMode::REPLACE, win, net_name, utf8, WM_TITLE).map(|c| c.check());
    }
    if let (Some(state), Some(no_taskbar), Some(no_pager)) =
        (atom(b"_NET_WM_STATE"), atom(b"_NET_WM_STATE_SKIP_TASKBAR"), atom(b"_NET_WM_STATE_SKIP_PAGER"))
    {
        // Порядок полей по EWMH: действие (1 — добавить), два свойства,
        // источник (1 — просит приложение), ноль в запас.
        let ev = ClientMessageEvent::new(32, win, state, [1, no_taskbar, no_pager, 1, 0]);
        let _ = conn.send_event(false, root, EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT, ev).map(|c| c.check());
    }
    let _ = conn.flush();
}

// Заглушки для не-Linux нет намеренно: зовут функцию из блока под
// `#[cfg(target_os = "linux")]`, и пустышка на Windows была бы мёртвым кодом.

/// Показ окна — событие пересчёта готовности (спека): человек открывает
/// флайаут именно затем, чтобы увидеть, как дела, а вердикт бывает
/// устаревшим. Файл параметров перечитывается здесь же и до постройки панели
/// (та читает `ui.cfg` при создании): правку toml руками иначе никто не
/// заметил бы до перезапуска. Файл не читается или сломан — остаёмся на
/// прежних параметрах молча, о них человеку уже сказали при запуске.
fn reread_and_recheck(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    if let Ok(mut cfg) = Cfg::load_unchecked(&ui.kit) {
        // Круг исправления задачи 1 плана «тонкий срез ценности»: перечитывание
        // строит свою копию `Cfg` — не общую с `gui::launch`'ой начальной и не
        // с «Сохранить» (`gui::save::save_host`).
        ops_host::settle_chosen_http_port(&mut cfg, &ui.kit);
        // Не только в окно, но и в живой сеанс — иначе агент остался бы на
        // старой базе (`gui::adopt_cfg`).
        super::adopt_cfg(ui, cfg);
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
    let wa = work_area(cx);
    // Высота — от экрана и только от экрана (`height_on`), один раз на показ:
    // дальше окно её не меняет, что бы ни показывала «Связь».
    let s = size(MIN_SIZE.width, height_on(wa.size.height));
    let origin = flyout_origin(wa, s, GAP);
    // Диагностика наезда на панель задач (заказчик 2026-10-20): без этой
    // строки жалоба «флайаут торчит за панелью» решается только через
    // скриншот + вопрос, какая рабочая область была в тот момент. Пишется
    // только в файл журнала (`note_file`): в окне она у человека занимала
    // половину списка на каждом открытии (заказчик 2026-10-21).
    note_file(
        &cx.global::<Ui>().kit,
        &Line {
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
        // и не получилось (менеджера композитинга в сеансе xrdp нет ни на
        // одном дисплее, замеры 2026-10-21) — и остаётся непрозрачным: без
        // композитора прозрачность даёт только бледные пятна по углам. Углы
        // там скруглений нет вовсе: без композитора они не получаются.
        // На Windows прозрачность работает (снимок заказчика), не трогаем.
        window_background: if cfg!(windows) { WindowBackgroundAppearance::Transparent } else { WindowBackgroundAppearance::Opaque },
        icon,
        ..Default::default()
    };
    match gpui_kit::open_window(opts, cx, |window, cx| cx.new(|cx| Panel::new(window, cx))) {
        Ok((handle, panel)) => {
            // `Dialog` на Linux — управляемое окно, и угол при самом первом
            // показе решает не наш хинт `window_bounds`, а размещение самого
            // менеджера (`WM_SIZE_HINTS` страхует только размер, не позицию);
            // поэтому угол доводим сами, тем же `move_to` и по той же
            // формуле (`flyout_origin`). Второго случая у `move_to` больше
            // нет: высота окна с 9.11 не меняется после показа, и возвращать
            // окно в угол после подгонки стало незачем.
            #[cfg(target_os = "linux")]
            let _ = handle.update(cx, |_, window, _| {
                move_to(window, origin);
                // Имя и «не показывай меня в панели задач» — здесь же, по
                // тому же отображённому окну: раньше оно висело в панели
                // безымянным (заказчик 2026-10-21).
                name_and_hide_from_taskbar(window);
                // Больше окна здесь никто не трогает: размер задан при
                // открытии и остаётся до закрытия.
            });
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
    // Вопрос живёт внутри этого окна: окна нет — и ждать ответа не от кого.
    // Флаг держали, пока ответа нет, и если окно закрылось с открытым
    // вопросом, он оставался поднятым навсегда: «Сохранить» после этого молча
    // ничего не делала (тот же симптом, что заказчик ловил 2026-10-21, но
    // другая причина).
    ui.dialog_open = false;
    // Следующий показ — уже другое окно X11, и форма ему нужна своя, даже
    // если размер совпал с прежним.
}

pub fn hide_window(window: &mut Window, cx: &mut App) {
    forget_window(cx.global_mut::<Ui>());
    window.remove_window();
}

#[cfg(test)]
mod tests {
    use super::{flyout_origin, forget_window, height_on, GAP, MIN_SIZE};
    use crate::gui::Ui;
    use gpui_kit::{point, px, size, Bounds};

    /// Высота окна — чистая функция от рабочей области, и только от неё:
    /// содержимого эта функция не видит вовсе, поэтому переключение режима
    /// не может её изменить.
    #[test]
    fn the_window_height_comes_from_the_screen_not_from_the_content() {
        // Рабочая область владельца 9.11 — из журнала окна: «рабочая область
        // и угол флайаута: 2048x1104+0+0».
        let h = height_on(px(1104.));
        assert!(h > MIN_SIZE.height, "окно должно открываться высоким, а не наименьшим: {h:?}");
        assert!(h <= px(1104.) - GAP - GAP, "окно вылезло за рабочую область: {h:?}");
        // Прежние два состояния («Всё на одной машине» и «Принимаю
        // подключения») открывались на 832 и 1029 логических точках — скачок
        // на четверть экрана. Новая высота покрывает высокое из них почти
        // целиком, остаток уходит в прокрутку.
        assert!(h >= px(950.), "высота не покрывает туннельное состояние: {h:?}");
        // Низкий экран: доля считается от него, окно не выше области с
        // зазорами.
        let low = height_on(px(600.));
        assert!(low <= px(600.) - GAP - GAP, "на низком экране окно за рабочей областью: {low:?}");
        assert!(low > MIN_SIZE.height, "600 точек хватает на высоту больше наименьшей: {low:?}");
        // Вырожденный обмер (рабочая область меньше самого окна): пол
        // `MIN_SIZE.height` сильнее потолка — окно шириной в палец и высотой в
        // три строки бесполезно, а неверный обмер экрана бывает, и лучше
        // торчать за край, чем показать нечитаемую щель.
        assert_eq!(height_on(px(300.)), MIN_SIZE.height);
        assert_eq!(MIN_SIZE.height, px(420.));
    }

    #[test]
    fn the_flyout_sits_in_the_corner_and_never_leaves_the_work_area() {
        let wa = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(700.)));
        assert_eq!(flyout_origin(wa, MIN_SIZE, GAP), point(px(608.), px(268.)));
        let tiny = Bounds::new(point(px(100.), px(50.)), size(px(300.), px(300.)));
        assert_eq!(flyout_origin(tiny, MIN_SIZE, GAP), point(px(100.), px(50.)));
        // Дефект 1: рабочая область меньше дисплея (панель XFCE внизу срезает
        // 40 px из 1080) — высота считается от реальной рабочей области, и
        // окно целиком остаётся внутри неё, а не торчит нижним краем за экран.
        let shrunk = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1040.)));
        let h = height_on(shrunk.size.height);
        let origin = flyout_origin(shrunk, size(MIN_SIZE.width, h), GAP);
        assert!(origin.y + h <= shrunk.size.height - GAP, "нижний край окна за рабочей областью");
        // Та же доля, но по ошибке — от границ всего дисплея (1080 без вычета
        // панели, ровно старый баг): низ окна проезжает за настоящую рабочую
        // область (1040) — учёт панели в `work_area` и исправляет именно это.
        let whole_display = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)));
        let h_wrong = height_on(whole_display.size.height);
        let origin_wrong = flyout_origin(whole_display, size(MIN_SIZE.width, h_wrong), GAP);
        assert!(origin_wrong.y + h_wrong > shrunk.size.height, "без учёта панели низ окна должен был уйти под неё");

        // Живые рабочие области: ноутбук заказчика 2026-10-20 (2560x1397,
        // `_NET_WORKAREA` с панелью XFCE), машина владельца 9.11 (2048x1104,
        // журнал окна) и вырожденно низкий экран. Ни низ, ни правый край окна
        // не уходят за рабочую область с зазором — кроме вырожденного случая,
        // где пол `MIN_SIZE.height` сильнее потолка нарочно.
        for area in [size(px(2560.), px(1397.)), size(px(2048.), px(1104.)), size(px(1366.), px(768.))] {
            let wa = Bounds::new(point(px(0.), px(0.)), area);
            let h = height_on(area.height);
            let origin = flyout_origin(wa, size(MIN_SIZE.width, h), GAP);
            assert!(origin.y + h <= area.height - GAP, "низ флайаута за рабочей областью: область {area:?}, высота {h:?}");
            assert!(origin.x + MIN_SIZE.width <= area.width - GAP, "правый край флайаута за рабочей областью: область {area:?}");
        }
    }

    #[test]
    fn hiding_forgets_both_handles() {
        let mut ui = Ui::for_test();
        forget_window(&mut ui);
        assert!(ui.window.is_none(), "ручка окна");
        assert!(ui.panel.is_none(), "сущность панели");
    }

    /// Окно закрылось с открытым вопросом — флаг не должен его пережить.
    /// Иначе «Сохранить» потом молча ничего не делает: диалог считается
    /// занятым навсегда.
    #[test]
    fn hiding_clears_a_question_that_nobody_answered() {
        let mut ui = Ui::for_test();
        ui.dialog_open = true;
        forget_window(&mut ui);
        assert!(!ui.dialog_open, "вопрос жил в этом окне, окна больше нет");
    }
}
