//! Окно: тонкая оболочка над теми же операциями и проверками, что и
//! командная строка. Логики развёртывания здесь нет намеренно.

use crate::cfg::{self, Cfg};
use crate::ops::{self, Ctx};
use crate::verify;
use common::check::Check;
use common::kit::Kit;
use common::op::CleanLevel;
use common::{AlwaysDefault, Event, Runner};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

pub struct Ui {
    kit: Kit,
    cfg: Mutex<Cfg>,
}

#[derive(Serialize, Clone)]
struct Line {
    kind: String,
    text: String,
}

#[derive(Serialize)]
struct OpView {
    id: String,
    title: String,
    /// done | missing | drift | blocked
    kind: String,
    detail: String,
}

#[derive(Serialize)]
struct Machine {
    kit: String,
    file_exists: bool,
    cfg: Cfg,
    /// RDP, Gitea, модель, SSH для 1c-connect — из параметров.
    ports: [u16; 4],
    /// Адрес сервера модели, как его видит контейнер среды.
    model_url: String,
    /// Числа последнего замера локального сервера, если он был.
    facts: Option<crate::cfg::Facts>,
    ops: Vec<OpView>,
}

fn ctx_for(app: &AppHandle, ui: &Ui, dry_run: bool) -> Ctx {
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
        let _ = handle.emit("dev1c:line", line);
    });
    Ctx { kit: ui.kit.clone(), cfg: ui.cfg.lock().unwrap().clone(), dry_run, sink, prompt: Box::new(AlwaysDefault) }
}

fn states(ctx: &Ctx) -> Vec<OpView> {
    common::run::status(ctx, &ops::server(&ctx.cfg))
        .into_iter()
        .map(|(id, title, st)| {
            use common::op::OpState::*;
            // Значок говорит состояние; текст — только подробность.
            let (kind, detail) = match st {
                Done(d) => ("done", d),
                Missing => ("missing", "не сделано".into()),
                Drifted { from, to } => ("drift", format!("было: {from} → станет: {to}")),
                Blocked(why) => ("blocked", why),
            };
            OpView { id, title, kind: kind.into(), detail }
        })
        .collect()
}

#[tauri::command]
fn machine(app: AppHandle, ui: State<Ui>) -> Machine {
    let ctx = ctx_for(&app, &ui, true);
    Machine {
        kit: ui.kit.root.display().to_string(),
        file_exists: ui.kit.path(cfg::FILE).exists(),
        cfg: ctx.cfg.clone(),
        ports: ctx.cfg.ports(),
        model_url: ctx.cfg.model_url(),
        facts: crate::llama::facts_load(&ctx),
        ops: states(&ctx),
    }
}

/// Параметры → `dev1c.toml` с комментариями образца. Правится файл, не
/// память: следующий запуск должен видеть новые значения как желаемые.
#[tauri::command]
fn save_cfg(ui: State<Ui>, cfg: Cfg) -> Result<(), String> {
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    common::kit::write_text(&ui.kit.path(cfg::FILE), &Cfg::render(&cfg)).map_err(|e| format!("{e:#}"))?;
    *ui.cfg.lock().unwrap() = cfg;
    Ok(())
}

/// `install` — всё недостающее и проверка после; `change` — только
/// разошедшееся; `clean-stack` / `clean-data` — снять. Ход — событиями,
/// конец — `dev1c:done` с текстом ошибки или без.
#[tauri::command]
fn run(app: AppHandle, ui: State<Ui>, which: String) {
    let ctx = ctx_for(&app, &ui, false);
    std::thread::spawn(move || {
        let ops = ops::server(&ctx.cfg);
        let r = match which.as_str() {
            "change" => common::run::apply_all(&ctx, &ops, None, true).map(|_| ()),
            "clean-stack" => common::run::clean_apply(&ctx, &ops, CleanLevel::Stack),
            "clean-data" => common::run::clean_apply(&ctx, &ops, CleanLevel::Data),
            _ => common::run::apply_all(&ctx, &ops, None, false).and_then(|_| {
                let checks = verify::run(&ctx);
                let (_, f) = verify::summary(&checks);
                let path = verify::report(&ctx, &checks)?;
                for c in &checks {
                    if c.ok {
                        ctx.ok(format!("{}: {}", c.what, c.detail));
                    } else {
                        ctx.fail(format!("{}: {} — {}", c.what, c.detail, c.hint));
                    }
                }
                ctx.say(format!("Отчёт: {}", path.display()));
                anyhow::ensure!(f == 0, "проверка не пройдена");
                Ok(())
            }),
        };
        let _ = app.emit("dev1c:done", r.err().map(|e| format!("{e:#}")));
    });
}

#[tauri::command]
async fn check(app: AppHandle, ui: State<'_, Ui>) -> Result<Vec<Check>, String> {
    let ctx = ctx_for(&app, &ui, true);
    tauri::async_runtime::spawn_blocking(move || {
        let checks = verify::run(&ctx);
        let _ = verify::report(&ctx, &checks);
        checks
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

pub fn launch(kit: Kit) -> anyhow::Result<()> {
    let cfg = Cfg::load(&kit)?;
    tauri::Builder::default()
        .setup(|app| {
            // Значок окна ставим сами: иначе в панели задач окно идёт без него.
            if let Some(w) = app.get_webview_window("main") {
                if let Ok(img) = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png")) {
                    let _ = w.set_icon(img);
                }
            }
            Ok(())
        })
        .manage(Ui { kit, cfg: Mutex::new(cfg) })
        .invoke_handler(tauri::generate_handler![machine, save_cfg, run, check, quit])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("окно не открылось: {e}"))
}
