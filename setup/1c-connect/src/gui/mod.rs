//! Окно и трей на GPUI (gpui-kit) + tray-icon. Глобал `Ui` живёт в
//! приложении GPUI, сеанс туннеля — в нём же, и переживает закрытие окна:
//! его опускает только «Выход». Трей и обработчики ОС кладут события в один
//! канал; задача-читатель канала — единственное, что меняет `Ui`.

use crate::cfg::{self, Cfg, Host};
use crate::ops_host::{self, Ctx};
use crate::session::{self, Session, State};
use crate::verify;
use common::check::{self, Check};
use common::kit::{write_text, Kit};
use common::op::{CleanLevel, OpState};
use common::{run, AlwaysDefault, Event, Sink};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::*;
use std::sync::Arc;
use std::time::Instant;

pub mod panel;
pub mod theme;
pub mod tray;
pub mod window;

pub const TRAY_PNG: &[u8] = include_bytes!("../../icons/tray.png");

/// Значок трея со статусом: кружок в правом нижнем углу, зелёный —
/// подключён; без связи значок обесцвечен и кружок серый.
pub fn badge(base: &image::RgbaImage, up: bool) -> image::RgbaImage {
    let (w, h) = base.dimensions();
    let mut out = base.clone();
    if !up {
        for p in out.pixels_mut() {
            let [r, g, b, _] = p.0;
            let gr = ((r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100) as u8;
            p.0[0] = gr;
            p.0[1] = gr;
            p.0[2] = gr;
        }
    }
    let r = (w as f32 * 0.22).max(3.0);
    let (cx, cy) = (w as f32 - r - 1.0, h as f32 - r - 1.0);
    let colour: [u8; 4] = if up { [0x2f, 0x8f, 0x4e, 255] } else { [0x8d, 0x86, 0x7e, 255] };
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d <= r {
                let rim = d > r - 1.5;
                out.put_pixel(x, y, image::Rgba(if rim { [255, 255, 255, 255] } else { colour }));
            }
        }
    }
    out
}

pub fn web_url(cfg: &Cfg) -> String {
    let port = cfg.web_port();
    let host = if port == 80 { "localhost".to_string() } else { format!("localhost:{port}") };
    format!("http://{host}{}", cfg.publication())
}

/// Состояние окна целиком. Глобал GPUI: один на приложение, живёт дольше
/// любого окна.
pub struct Ui {
    pub kit: Kit,
    pub cfg: Cfg,
    pub session: Option<Session>,
    /// Растёт на каждый новый сеанс и на каждую остановку: хвостовое
    /// событие от уже не текущего сеанса (умирающий поток ещё договаривает)
    /// несёт старое значение и отбрасывается в `handle`.
    pub session_gen: u64,
    pub link: State,
    pub log: Vec<Line>,
    pub unread: usize,
    pub tab: Tab,
    pub checks: Vec<Check>,
    pub failed_checks: usize,
    pub ops: Vec<OpView>,
    pub busy: Option<Busy>,
    pub status: Option<Line>,
    pub tray: Option<tray::Tray>,
    pub window: Option<WindowHandle<panel::Panel>>,
    pub hidden_at: Option<Instant>,
    pub dialog_open: bool,
    pub tx: UnboundedSender<Ev>,
}
impl Global for Ui {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Ok,
    Warn,
    Fail,
    Say,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Params,
    Ext,
    Check,
    Log,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Busy {
    Ext,
    Check,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dot {
    Off,
    Busy,
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub struct OpView {
    pub id: String,
    pub title: String,
    pub dot: Dot,
    pub detail: String,
}

pub enum Ev {
    Tray(tray_icon::TrayIconEvent),
    Menu(tray_icon::menu::MenuEvent),
    /// Переход сеанса, помеченный поколением, что был текущим в момент
    /// отправки — старое поколение значит сеанс уже не тот.
    Link(u64, State),
    Line(Line),
    Done(Option<String>),
    Checks(Vec<Check>),
}

pub const LOG_CAP: usize = 2000;

/// `Command`/`Output` — в журнал команд на диске (см. `ctx_for`), не в окно.
pub fn line_of(e: &Event) -> Option<Line> {
    let (kind, text) = match e {
        Event::Ok(t) => (Kind::Ok, t.clone()),
        Event::Warn(t) => (Kind::Warn, t.clone()),
        Event::Fail(t) => (Kind::Fail, t.clone()),
        Event::Say(t) => (Kind::Say, t.clone()),
        Event::Command(_) | Event::Output { .. } => return None,
    };
    Some(Line { kind, text })
}

/// Строка перехода сеанса — тексты как в сегодняшнем `gui.rs::set_link`.
/// Начало и конец видны и в журнале: туннель сам пишет только про поднятую
/// связь и обрывы, а отказ возвращает ошибкой.
pub fn link_line(l: &State, host: &Host) -> Option<Line> {
    match l {
        State::Connecting => Some(Line { kind: Kind::Say, text: format!("подключаюсь к {} как {}", host.server, host.workstation) }),
        State::Failed(why) => Some(Line { kind: Kind::Fail, text: why.clone() }),
        State::Off => Some(Line { kind: Kind::Say, text: "отключено".into() }),
        State::Up { .. } | State::Retrying(_) => None,
    }
}

/// `Ev::Link(gen, ..)` стоит применять только если `gen` — поколение сеанса,
/// текущее прямо сейчас; иначе это хвост уже остановленного или уже
/// замещённого сеанса.
fn link_is_current(ui: &Ui, gen: u64) -> bool {
    gen == ui.session_gen
}

/// Строка в журнал, с потолком в `LOG_CAP`. Непрочитанные растут, если
/// журнал сейчас не на экране — вкладка другая или окно спрятано.
pub fn note_line(ui: &mut Ui, line: Line) {
    ui.log.push(line);
    if ui.log.len() > LOG_CAP {
        let extra = ui.log.len() - LOG_CAP;
        ui.log.drain(0..extra);
    }
    if ui.tab != Tab::Log || ui.window.is_none() {
        ui.unread += 1;
    }
}

/// Карточка операции из состояния `apply_all`/`status`.
pub fn op_view(id: String, title: String, st: OpState) -> OpView {
    let (dot, detail) = match st {
        OpState::Done(d) => (Dot::Up, d),
        OpState::Missing => (Dot::Off, "не установлено".into()),
        OpState::Drifted { from, to } => (Dot::Busy, format!("было: {from} → станет: {to}")),
        OpState::Blocked(why) => (Dot::Down, why),
    };
    OpView { id, title, dot, detail }
}

/// Сохранить параметры сервера. Полей проекта в окне пока нет (задача 6):
/// берём их из текущих параметров, иначе пустые значения из формы затёрли
/// бы toml.
pub fn save_host(ui: &mut Ui, mut host: Host) -> Result<(), String> {
    host.project = ui.cfg.host.project.clone();
    host.tools = ui.cfg.host.tools.clone();
    let cfg = Cfg { host };
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    write_text(&ui.kit.path(cfg::FILE), &cfg.render()).map_err(|e| format!("{e:#}"))?;
    ui.cfg = cfg;
    Ok(())
}

/// Контекст одного прогона: события идут в журнал на диске (команды и их
/// сырой вывод) или в окно — остальное.
fn ctx_for(ui: &Ui) -> Ctx {
    let kit = ui.kit.clone();
    let tx = ui.tx.clone();
    let sink: Sink = Arc::new(move |e: Event| {
        if kit.journal(&e) {
            return;
        }
        if let Some(line) = line_of(&e) {
            let _ = tx.unbounded_send(Ev::Line(line));
        }
    });
    Ctx { kit: ui.kit.clone(), cfg: ui.cfg.clone(), dry_run: false, sink, prompt: Box::new(AlwaysDefault) }
}

/// Переход сеанса: в состояние, в журнал, в трей, перерисовать окно.
fn set_link(cx: &mut App, l: State) {
    let host = cx.global::<Ui>().cfg.host.clone();
    let line = link_line(&l, &host);
    let ui = cx.global_mut::<Ui>();
    ui.link = l.clone();
    if let Some(line) = line {
        note_line(ui, line);
    }
    if let Some(tray) = &ui.tray {
        tray.set_state(l.is_up(), l.is_on());
    }
    cx.refresh_windows();
}

/// Тексты ошибок — дословно из сегодняшнего `gui.rs::start_session`.
pub fn start_session(cx: &mut App) -> Result<(), String> {
    let ui = cx.global_mut::<Ui>();
    if !ui.kit.path(cfg::FILE).exists() {
        return Err("в комплекте нет 1c-connect.toml — заполните параметры и нажмите «Сохранить»".into());
    }
    if ui.cfg.is_sample() {
        return Err("server всё ещё из образца — впишите адрес своей среды, его печатает dev1c-setup install".into());
    }
    if ui.session.is_some() && ui.link.is_on() {
        return Ok(());
    }
    // Новый сеанс — новое поколение: хвост от прежнего (если он всё ещё
    // договаривает) будет отбрасываться в `handle` по несовпадению.
    ui.session_gen += 1;
    let gen = ui.session_gen;
    let ctx = ctx_for(ui);
    let (cfg, kit, tx) = (ui.cfg.clone(), ui.kit.clone(), ui.tx.clone());
    let session = session::start(cfg, kit, ctx.sink, move |l| {
        let _ = tx.unbounded_send(Ev::Link(gen, l));
    });
    cx.global_mut::<Ui>().session = Some(session);
    Ok(())
}

/// Остановить сеанс (блокирующе — в своём потоке) и сразу показать «отключено».
pub fn stop_session(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    // Хвостовой `Ev::Link` от останавливаемого сеанса (тот всё ещё может
    // договаривать в своём потоке) должен отбрасываться — он уже не про
    // текущее поколение.
    ui.session_gen += 1;
    if let Some(s) = ui.session.take() {
        std::thread::spawn(move || {
            let _ = s.stop();
        });
    }
    set_link(cx, State::Off);
}

pub fn toggle_link(cx: &mut App) {
    let on = cx.global::<Ui>().link.is_on();
    if on {
        stop_session(cx);
        return;
    }
    if let Err(e) = start_session(cx) {
        window::show_window(cx);
        note_line(cx.global_mut::<Ui>(), Line { kind: Kind::Fail, text: e });
        cx.refresh_windows();
    }
}

/// Установить (`uninstall = false`) или снять расширение; ход — событиями
/// из `ctx_for`, конец — `Ev::Done`.
pub fn run_op(cx: &mut App, uninstall: bool) {
    let ui = cx.global_mut::<Ui>();
    ui.busy = Some(Busy::Ext);
    let ctx = ctx_for(ui);
    let tx = ui.tx.clone();
    cx.refresh_windows();
    std::thread::spawn(move || {
        let ops = ops_host::host();
        let r = if uninstall {
            ops.iter().find(|o| o.id() == "host.extension").map(|ext| ext.revert_apply(&ctx, CleanLevel::Stack)).unwrap_or(Ok(()))
        } else {
            run::apply_all(&ctx, &ops, None, false).map(|_| ())
        };
        let _ = tx.unbounded_send(Ev::Done(r.err().map(|e| format!("{e:#}"))));
    });
}

/// Семь фактов от основания к вершине; отчёт пишется в `last-check.txt` тем
/// же текстом, что печатает командная строка.
pub fn run_check(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    ui.busy = Some(Busy::Check);
    let ctx = ctx_for(ui);
    let link = ui.link.clone();
    let tx = ui.tx.clone();
    cx.refresh_windows();
    std::thread::spawn(move || {
        let state = link.is_up().then(|| link.label());
        let checks = verify::run(&ctx, state.as_deref());
        let text = check::render("1c-connect", env!("CARGO_PKG_VERSION"), &verify::cfg_for_report(&ctx.cfg), &checks);
        let _ = write_text(&ctx.kit.path("last-check.txt"), &text);
        let _ = tx.unbounded_send(Ev::Checks(checks));
    });
}

/// Перечитать состояние операций на машине (без изменений) в `ui.ops`.
pub fn refresh_ops(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    let ctx = ctx_for(ui);
    ui.ops = run::status(&ctx, &ops_host::host()).into_iter().map(|(id, title, st)| op_view(id, title, st)).collect();
    cx.refresh_windows();
}

pub fn open_web(cx: &mut App) {
    let url = web_url(&cx.global::<Ui>().cfg);
    cx.open_url(&url);
}

/// Выход: ждём, пока сеанс действительно опустится (поток собственного
/// рантайма сеанса), и только потом закрываем приложение — иначе туннель
/// обрывается не чисто.
pub fn quit(cx: &mut App) {
    if let Some(s) = cx.global_mut::<Ui>().session.take() {
        let stopped = std::thread::spawn(move || {
            let _ = s.stop();
        });
        let _ = stopped.join();
    }
    cx.quit();
}

fn handle(ev: Ev, cx: &mut App) {
    match ev {
        Ev::Tray(e) => tray::on_tray(e, cx),
        Ev::Menu(e) => tray::on_menu(e, cx),
        Ev::Link(gen, l) => {
            if link_is_current(cx.global::<Ui>(), gen) {
                set_link(cx, l);
            }
        }
        Ev::Line(line) => {
            note_line(cx.global_mut::<Ui>(), line);
            cx.refresh_windows();
        }
        Ev::Done(err) => {
            let ui = cx.global_mut::<Ui>();
            ui.busy = None;
            ui.status = Some(match err {
                Some(e) => Line { kind: Kind::Fail, text: e },
                None => Line { kind: Kind::Ok, text: "Готово.".into() },
            });
            refresh_ops(cx);
        }
        Ev::Checks(checks) => {
            let failed = checks.iter().filter(|c| !c.ok).count();
            let ui = cx.global_mut::<Ui>();
            ui.checks = checks;
            ui.failed_checks = failed;
            ui.busy = None;
            ui.status = Some(if failed > 0 {
                Line { kind: Kind::Fail, text: format!("Провалено: {failed}. Отчёт — last-check.txt в комплекте") }
            } else {
                Line { kind: Kind::Ok, text: "Все факты в порядке. Отчёт — last-check.txt".into() }
            });
            cx.refresh_windows();
        }
    }
}

pub fn launch(kit: Kit) -> anyhow::Result<()> {
    let cfg = Cfg::load(&kit)?;
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        theme::install(&kit, cx);
        // Окно закрыто — процесс живёт, пока «Выход» в трее.
        cx.set_quit_mode(QuitMode::Explicit);
        // Без опроса: обработчики трея (их зовёт WndProc на этом же потоке)
        // кладут событие в канал, задача GPUI просыпается по нему.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Ev>();
        // Закрытие `run` — `FnOnce(&mut App)`, без `Result`: ошибка трея не
        // смертельна (значок просто не появится), поэтому не `?`, а лог.
        let tray = match tray::build(tx.clone(), theme::Choice::load(&kit)) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("трей не создался: {e:#}");
                None
            }
        };
        cx.set_global(Ui {
            kit,
            cfg,
            session: None,
            session_gen: 0,
            link: State::Off,
            log: Vec::new(),
            unread: 0,
            tab: Tab::default(),
            checks: Vec::new(),
            failed_checks: 0,
            ops: Vec::new(),
            busy: None,
            status: None,
            tray,
            window: None,
            hidden_at: None,
            dialog_open: false,
            tx,
        });
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(ev) = rx.next().await {
                let _ = cx.update(|cx| handle(ev, cx));
            }
        })
        .detach();
        refresh_ops(cx);
        window::show_window(cx);
    });
    Ok(())
}

#[cfg(test)]
impl Ui {
    fn for_test() -> Ui {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("onec-connect-gui-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        Ui {
            kit: Kit::at(dir),
            cfg: Cfg::default(),
            session: None,
            session_gen: 0,
            link: State::Off,
            log: Vec::new(),
            unread: 0,
            tab: Tab::default(),
            checks: Vec::new(),
            failed_checks: 0,
            ops: Vec::new(),
            busy: None,
            status: None,
            tray: None,
            window: None,
            hidden_at: None,
            dialog_open: false,
            tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{badge, line_of, link_is_current, link_line, note_line, save_host, Kind, Line, Tab, Ui, LOG_CAP, TRAY_PNG};
    use crate::cfg::{self, Cfg, Host};
    use crate::session::State;
    use common::Event;
    use std::time::Instant;

    #[test]
    fn the_badge_sits_in_the_corner_and_says_up_or_down() {
        let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
        let up = badge(&base, true);
        assert_eq!(up.dimensions(), base.dimensions());
        let w = up.width() as usize;
        let at = |img: &image::RgbaImage, x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[..3].to_vec();
        let c = w - 1 - (w as f32 * 0.22) as usize;
        assert_eq!(at(&up, c, c), vec![0x2f, 0x8f, 0x4e]);
        let down = badge(&base, false);
        assert_eq!(at(&down, c, c), vec![0x8d, 0x86, 0x7e]);
        let p = at(&down, 2, 2);
        assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    }

    #[test]
    fn web_client_url_follows_the_publication() {
        let mut c = Cfg::default();
        assert_eq!(super::web_url(&c), "http://localhost/Buh/");
        c.host.base_url = "http://localhost:8081/InfoBase/hs/mcp-1c".into();
        assert_eq!(super::web_url(&c), "http://localhost:8081/InfoBase/");
    }

    #[test]
    fn command_and_output_never_reach_the_window() {
        assert_eq!(line_of(&Event::Command("x".into())), None);
        assert_eq!(line_of(&Event::Output { command: "x".into(), text: "y".into() }), None);
        assert_eq!(line_of(&Event::Warn("w".into())), Some(Line { kind: Kind::Warn, text: "w".into() }));
    }

    #[test]
    fn the_log_keeps_the_last_two_thousand_lines_and_counts_unread() {
        let mut ui = Ui::for_test();
        for i in 0..LOG_CAP + 5 {
            note_line(&mut ui, Line { kind: Kind::Say, text: i.to_string() });
        }
        assert_eq!(ui.log.len(), LOG_CAP);
        assert_eq!(ui.log[0].text, "5");
        assert_eq!(ui.unread, LOG_CAP + 5);
        ui.tab = Tab::Log;
        ui.unread = 0; // окна нет — строки всё равно непрочитанные
        note_line(&mut ui, Line { kind: Kind::Say, text: "x".into() });
        assert_eq!(ui.unread, 1);
    }

    #[test]
    fn a_bad_server_leaves_the_toml_alone_and_says_why() {
        let mut ui = Ui::for_test();
        let before = std::fs::read_to_string(ui.kit.path(cfg::FILE)).ok();
        let mut host = ui.cfg.host.clone();
        host.server = "no-port".into();
        let err = save_host(&mut ui, host).unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(std::fs::read_to_string(ui.kit.path(cfg::FILE)).ok(), before);
    }

    #[test]
    fn link_lines_say_where_and_as_whom() {
        let h = Host { server: "s:1".into(), workstation: "dev2".into(), ..Default::default() };
        assert_eq!(link_line(&State::Connecting, &h).unwrap().text, "подключаюсь к s:1 как dev2");
        assert_eq!(link_line(&State::Off, &h).unwrap().text, "отключено");
        assert!(link_line(&State::Up { since: Instant::now() }, &h).is_none());
    }

    #[test]
    fn a_stale_session_generation_is_not_current() {
        let mut ui = Ui::for_test();
        ui.session_gen = 2;
        assert!(!link_is_current(&ui, 1), "поколение 1 устарело, текущее — 2");
        assert!(link_is_current(&ui, 2));
    }
}
