//! Окно и трей. Тонкая оболочка: страница зовёт те же операции и проверки,
//! что и командная строка, и рисует события; сеанс туннеля живёт здесь в
//! состоянии приложения и переживает закрытие окна — его опускает только
//! «Выход».

use crate::cfg::{self, Cfg, Host};
use crate::ops_host::{self, Ctx};
use crate::session::{self, Session, State as Link};
use crate::verify;
use common::check::Check;
use common::kit::Kit;
use common::op::CleanLevel;
use common::{AlwaysDefault, Event};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

const TRAY: &str = "main";
const TRAY_PNG: &[u8] = include_bytes!("../icons/tray.png");

pub struct Ui {
    kit: Kit,
    cfg: Mutex<Cfg>,
    session: Mutex<Option<Session>>,
    link: Mutex<Link>,
}

#[derive(Serialize, Clone)]
struct Line {
    kind: String,
    text: String,
}

#[derive(Serialize, Clone)]
struct LinkView {
    label: String,
    on: bool,
    up: bool,
}

#[derive(Serialize)]
struct Machine {
    kit: String,
    file_exists: bool,
    is_sample: bool,
    host: Host,
    ports: [u16; 3],
    web_url: String,
    link: LinkView,
}

#[derive(Serialize)]
struct OpView {
    id: String,
    title: String,
    kind: String,
    label: String,
}

fn link_view(l: &Link) -> LinkView {
    LinkView { label: l.label(), on: l.is_on(), up: l.is_up() }
}

/// Значок трея со статусом: кружок в правом нижнем углу, зелёный —
/// подключён; без связи значок обесцвечен и кружок серый.
pub fn badge(base: &Image<'_>, up: bool) -> Image<'static> {
    let (w, h) = (base.width(), base.height());
    let mut px = base.rgba().to_vec();
    if !up {
        for p in px.chunks_exact_mut(4) {
            let g = ((p[0] as u32 * 30 + p[1] as u32 * 59 + p[2] as u32 * 11) / 100) as u8;
            p[0] = g;
            p[1] = g;
            p[2] = g;
        }
    }
    let r = (w as f32 * 0.22).max(3.0);
    let (cx, cy) = (w as f32 - r - 1.0, h as f32 - r - 1.0);
    let colour: [u8; 4] = if up { [0x2f, 0x8f, 0x4e, 255] } else { [0x8d, 0x86, 0x7e, 255] };
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d <= r {
                let i = ((y * w + x) * 4) as usize;
                let rim = d > r - 1.5;
                px[i..i + 4].copy_from_slice(if rim { &[255, 255, 255, 255] } else { &colour });
            }
        }
    }
    Image::new_owned(px, w, h)
}

fn web_url(cfg: &Cfg) -> String {
    let port = cfg.web_port();
    let host = if port == 80 { "localhost".to_string() } else { format!("localhost:{port}") };
    format!("http://{host}{}", cfg.publication())
}

fn ctx_for(app: &AppHandle, ui: &Ui) -> Ctx {
    let handle = app.clone();
    let kit = ui.kit.clone();
    let sink = Arc::new(move |e: Event| {
        if kit.journal(&e) {
            return;
        }
        let line = match e {
            Event::Ok(t) => Line { kind: "ok".into(), text: t },
            Event::Warn(t) => Line { kind: "warn".into(), text: t },
            Event::Fail(t) => Line { kind: "fail".into(), text: t },
            Event::Say(t) => Line { kind: "say".into(), text: t },
            Event::Command(_) | Event::Output { .. } => return,
        };
        let _ = handle.emit("conn:line", line);
    });
    Ctx { kit: ui.kit.clone(), cfg: ui.cfg.lock().unwrap().clone(), dry_run: false, sink, prompt: Box::new(AlwaysDefault) }
}

/// Переход сеанса: в состояние, в окно, в трей.
fn set_link(app: &AppHandle, l: Link) {
    let ui = app.state::<Ui>();
    *ui.link.lock().unwrap() = l.clone();
    let _ = app.emit("conn:state", link_view(&l));
    refresh_tray(app, &l);
}

fn refresh_tray(app: &AppHandle, l: &Link) {
    if let Some(tray) = app.tray_by_id(TRAY) {
        if let Ok(base) = Image::from_bytes(TRAY_PNG) {
            let _ = tray.set_icon(Some(badge(&base, l.is_up())));
        }
        let ui = app.state::<Ui>();
        let h = ui.cfg.lock().unwrap().host.clone();
        let _ = tray.set_tooltip(Some(format!("{} → {}: {}", h.workstation, h.server, l.label())));
    }
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn start_session(app: &AppHandle, ui: &Ui) -> Result<(), String> {
    let cfg = ui.cfg.lock().unwrap().clone();
    if !ui.kit.path(cfg::FILE).exists() {
        return Err("в комплекте нет 1c-connect.toml — заполните параметры и нажмите «Сохранить»".into());
    }
    if cfg.is_sample() {
        return Err("server всё ещё из образца — впишите адрес своей среды, его печатает dev1c-setup install".into());
    }
    let mut slot = ui.session.lock().unwrap();
    if slot.is_some() && ui.link.lock().unwrap().is_on() {
        return Ok(());
    }
    let ctx = ctx_for(app, ui);
    let handle = app.clone();
    *slot = Some(session::start(cfg, ui.kit.clone(), ctx.sink.clone(), move |l| set_link(&handle, l)));
    Ok(())
}

fn stop_session(ui: &Ui) -> Result<(), String> {
    let s = ui.session.lock().unwrap().take();
    match s {
        Some(s) => s.stop().map_err(|e| format!("{e:#}")),
        None => Ok(()),
    }
}

#[tauri::command]
fn machine(ui: State<Ui>) -> Machine {
    let cfg = ui.cfg.lock().unwrap().clone();
    let p = cfg.ports();
    Machine {
        kit: ui.kit.root.display().to_string(),
        file_exists: ui.kit.path(cfg::FILE).exists(),
        is_sample: cfg.is_sample(),
        host: cfg.host.clone(),
        ports: [p.mcp, p.vrunner, p.web],
        web_url: web_url(&cfg),
        link: link_view(&ui.link.lock().unwrap()),
    }
}

/// Состояние сеанса; окно спрашивает его раз в полминуты ради «подключён N мин».
#[tauri::command]
fn link(app: AppHandle, ui: State<Ui>) -> LinkView {
    let l = ui.link.lock().unwrap().clone();
    refresh_tray(&app, &l);
    link_view(&l)
}

#[tauri::command]
fn save_cfg(ui: State<Ui>, host: Host) -> Result<(), String> {
    let cfg = Cfg { host };
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    common::kit::write_text(&ui.kit.path(cfg::FILE), &cfg.render()).map_err(|e| format!("{e:#}"))?;
    *ui.cfg.lock().unwrap() = cfg;
    Ok(())
}

#[tauri::command]
fn connect(app: AppHandle, ui: State<Ui>) -> Result<(), String> {
    start_session(&app, &ui)
}

#[tauri::command]
fn disconnect(ui: State<Ui>) -> Result<(), String> {
    stop_session(&ui)
}

#[tauri::command]
fn states(app: AppHandle, ui: State<Ui>) -> Vec<OpView> {
    let ctx = ctx_for(&app, &ui);
    common::run::status(&ctx, &ops_host::host())
        .into_iter()
        .map(|(id, title, st)| {
            use common::op::OpState::*;
            let kind = match st {
                Done(_) => "done",
                Missing => "missing",
                Drifted { .. } => "drift",
                Blocked(_) => "blocked",
            };
            OpView { id, title, kind: kind.into(), label: st.label() }
        })
        .collect()
}

/// Установить (`install`) или снять (`uninstall`) расширение; ход — событиями,
/// конец — `conn:done` с текстом ошибки или без.
#[tauri::command]
fn run(app: AppHandle, ui: State<Ui>, which: String) {
    let ctx = ctx_for(&app, &ui);
    std::thread::spawn(move || {
        let ops = ops_host::host();
        let r = if which == "uninstall" {
            ops.iter()
                .find(|o| o.id() == "host.extension")
                .map(|ext| ext.revert_apply(&ctx, CleanLevel::Stack))
                .unwrap_or(Ok(()))
        } else {
            common::run::apply_all(&ctx, &ops, None, false).map(|_| ())
        };
        let _ = app.emit("conn:done", r.err().map(|e| format!("{e:#}")));
    });
}

#[tauri::command]
async fn check(app: AppHandle, ui: State<'_, Ui>) -> Result<Vec<Check>, String> {
    let ctx = ctx_for(&app, &ui);
    let link = ui.link.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = link.is_up().then(|| link.label());
        let checks = verify::run(&ctx, state.as_deref());
        let text = common::check::render("1c-connect", env!("CARGO_PKG_VERSION"), &verify::cfg_for_report(&ctx.cfg), &checks);
        let _ = common::kit::write_text(&ctx.kit.path("last-check.txt"), &text);
        checks
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_web(ui: State<Ui>) -> Result<(), String> {
    let url = web_url(&ui.cfg.lock().unwrap());
    #[cfg(windows)]
    let r = std::process::Command::new("cmd").args(["/c", "start", "", &url]).spawn();
    #[cfg(not(windows))]
    let r = std::process::Command::new("xdg-open").arg(&url).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn hide(window: tauri::WebviewWindow) {
    let _ = window.hide();
}

#[tauri::command]
fn quit(app: AppHandle, ui: State<Ui>) {
    let _ = stop_session(&ui);
    app.exit(0);
}

fn tray(app: &tauri::App) -> tauri::Result<()> {
    let items = [
        MenuItem::with_id(app, "connect", "Подключить", true, None::<&str>)?,
        MenuItem::with_id(app, "disconnect", "Отключить", true, None::<&str>)?,
        MenuItem::with_id(app, "check", "Проверить", true, None::<&str>)?,
        MenuItem::with_id(app, "web", "Открыть веб-клиент", true, None::<&str>)?,
    ];
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&items[0], &items[1], &items[2], &items[3], &sep, &quit])?;
    let base = Image::from_bytes(TRAY_PNG)?;
    TrayIconBuilder::with_id(TRAY)
        .icon(badge(&base, false))
        .tooltip("1c-connect: не подключён")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| {
            let ui = app.state::<Ui>();
            match ev.id().0.as_str() {
                "connect" => {
                    if let Err(e) = start_session(app, &ui) {
                        show_window(app);
                        let _ = app.emit("conn:line", Line { kind: "fail".into(), text: e });
                    }
                }
                "disconnect" => {
                    if let Err(e) = stop_session(&ui) {
                        let _ = app.emit("conn:line", Line { kind: "fail".into(), text: e });
                    }
                }
                "check" => {
                    show_window(app);
                    let _ = app.emit("conn:run-check", ());
                }
                "web" => {
                    let _ = open_web(ui);
                }
                "quit" => {
                    let _ = stop_session(&ui);
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub fn launch(kit: Kit) -> anyhow::Result<()> {
    let cfg = Cfg::load(&kit)?;
    tauri::Builder::default()
        .setup(|app| {
            if let Some(w) = app.get_webview_window("main") {
                if let Ok(img) = Image::from_bytes(include_bytes!("../icons/icon.png")) {
                    let _ = w.set_icon(img);
                }
            }
            tray(app)?;
            Ok(())
        })
        .manage(Ui { kit, cfg: Mutex::new(cfg), session: Mutex::new(None), link: Mutex::new(Link::Off) })
        // Крестик прячет окно: туннель живёт, пока не сказано «Выход».
        .on_window_event(|w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![machine, link, save_cfg, connect, disconnect, states, run, check, open_web, hide, quit])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("окно не открылось: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_sits_in_the_corner_and_says_up_or_down() {
        let base = Image::from_bytes(TRAY_PNG).unwrap();
        let up = badge(&base, true);
        assert_eq!((up.width(), up.height()), (base.width(), base.height()));
        let w = up.width() as usize;
        let at = |img: &Image<'_>, x: usize, y: usize| img.rgba()[(y * w + x) * 4..(y * w + x) * 4 + 3].to_vec();
        // Centre of the dot: the colour, not the icon.
        let c = w - 1 - (w as f32 * 0.22) as usize;
        assert_eq!(at(&up, c, c), vec![0x2f, 0x8f, 0x4e]);
        let down = badge(&base, false);
        assert_eq!(at(&down, c, c), vec![0x8d, 0x86, 0x7e]);
        // Top-left is untouched by the dot and, when down, grey (r == g == b).
        let p = at(&down, 2, 2);
        assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    }

    #[test]
    fn web_client_url_follows_the_publication() {
        let mut c = Cfg::default();
        assert_eq!(web_url(&c), "http://localhost/Buh/");
        c.host.base_url = "http://localhost:8081/InfoBase/hs/mcp-1c".into();
        assert_eq!(web_url(&c), "http://localhost:8081/InfoBase/");
    }
}
