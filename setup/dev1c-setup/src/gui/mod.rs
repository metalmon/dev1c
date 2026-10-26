//! Окно: тонкая оболочка над теми же операциями и проверками, что и
//! командная строка. Логики развёртывания здесь нет намеренно.
//!
//! Чистые решения окна (что говорит карточка состояния, какая вкладка
//! открывается, что несёт плашка) живут в `crate::ui` — там нет GPUI, и их
//! тесты идут в сборке без окна. Здесь — состояние, события и вид.

pub mod panel;
pub mod window;

use crate::cfg::Cfg;
use crate::ops::{self, Ctx};
use crate::ui::{self, Busy, Kind, Line, OpView, Tab, Which, LOG_CAP, LOG_FILE};
use crate::verify;
use common::check::Check;
use common::kit::Kit;
use common::op::CleanLevel;
use common::{AlwaysDefault, Event, Runner};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{AnyWindowHandle, App, Entity, Global};
use std::sync::Arc;
use std::time::Instant;

/// Состояние окна целиком. Глобал GPUI: один на приложение.
pub struct Ui {
    pub kit: Kit,
    pub cfg: Cfg,
    /// Состояние операций, перечитанное в своём потоке (`refresh_ops`).
    pub ops: Vec<OpView>,
    /// Сколько фактов провалила последняя проверка — бейдж на «Проверке».
    pub failed_checks: usize,
    pub checks: Vec<Check>,
    /// Числа последнего замера локального сервера модели (окно, max_tokens),
    /// если он был — вкладка «Доступ». Без токена: он не для показа.
    /// Окно слота из последнего замера. Чисел для Zed больше нет: их
    /// забирает с `/props` сам провайдер.
    pub facts: Option<u32>,
    /// Адреса этой машины для вкладки «Доступ»: первый — тот, что в
    /// локальной сети. Разработчику адрес диктует администратор, и лучше
    /// показать настоящие, чем «<адрес>».
    pub addresses: Vec<String>,
    pub log: Vec<Line>,
    pub unread: usize,
    pub tab: Tab,
    /// Начало текущей работы — для «Готово за …».
    pub started: Option<Instant>,
    pub busy: Option<Busy>,
    /// Число живых потоков `refresh_ops` — полоса хода читает это вместе с `busy`.
    pub pending: usize,
    pub status: Option<Line>,
    /// Почему параметры не прочитались, если не прочитались: плашка над
    /// вкладками и строка в журнале при открытии.
    pub cfg_error: Option<String>,
    pub window: Option<AnyWindowHandle>,
    pub panel: Option<Entity<panel::Panel>>,
    /// Пока открыт системный диалог выбора файла, окно не считает себя
    /// потерявшим фокус.
    pub dialog_open: bool,
    pub tx: UnboundedSender<Ev>,
}
impl Global for Ui {}

impl Ui {
    /// Что-то идёт: работа (`busy`) или фоновое чтение состояния (`pending`).
    /// Это читает полоса хода и подтверждение при закрытии окна.
    pub fn working(&self) -> bool {
        self.busy.is_some() || self.pending > 0
    }
}

/// События в окно из рабочих потоков.
pub enum Ev {
    Line(Line),
    /// Конец работы: текст ошибки или ничего.
    Done(Option<String>),
    Checks(Vec<Check>),
    /// Адреса этой машины, собранные в своём потоке.
    Addresses(Vec<String>),
    /// Состояние операций и окно слота из замера, перечитанные в своём потоке.
    Ops(Vec<OpView>, Option<u32>),
}

/// Строка в журнал окна и в файл журнала рядом с комплектом.
pub fn note_line(ui: &mut Ui, line: Line) {
    ui::write_log_file(&ui.kit, &line);
    if ui.log.len() >= LOG_CAP {
        ui.log.drain(..ui.log.len() + 1 - LOG_CAP);
    }
    // Непрочитанные считаем, пока журнал не открыт; заголовки разделов не
    // считаются — они не новость, а разметка.
    if ui.tab != Tab::Log && line.kind != Kind::Head {
        ui.unread += 1;
    }
    ui.log.push(line);
}

/// Контекст операций: тот же, что у командной строки, но события идут
/// событием в окно. Журнал команд комплекта пишется первым, как и в CLI.
fn ctx_for(ui: &Ui, dry_run: bool) -> Ctx {
    let tx = ui.tx.clone();
    let kit = ui.kit.clone();
    let sink = Arc::new(move |e: Event| {
        if kit.journal(&e) {
            return;
        }
        if let Some(line) = ui::line_of(&e) {
            let _ = tx.unbounded_send(Ev::Line(line));
        }
    });
    Ctx { kit: ui.kit.clone(), cfg: ui.cfg.clone(), dry_run, sink, prompt: Box::new(AlwaysDefault) }
}

/// Состояния операций по текущим параметрам — в своём потоке: спросить
/// движок контейнеров быстро не всегда получается.
pub fn refresh_ops(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    ui.pending += 1;
    let ctx = ctx_for(ui, true);
    let tx = ui.tx.clone();
    std::thread::spawn(move || {
        ui::guarded(
            |ev| {
                let _ = tx.unbounded_send(ev);
            },
            Ev::Line,
            || {
                let ops = common::run::status(&ctx, &ops::server(&ctx.cfg))
                    .into_iter()
                    .map(|(id, title, st)| ui::op_view(id, title, st))
                    .collect();
                let facts = crate::llama::facts_load(&ctx).map(|f| f.window);
                Ev::Ops(ops, facts)
            },
            // Паника не должна оставить полосу хода навсегда: пустой ответ
            // снимает `pending`, а строку о поломке уже отправил `guarded`.
            || Ev::Ops(Vec::new(), None),
        );
    });
}

/// Работа кнопки: установка, применение изменений или очистка.
pub fn run_op(cx: &mut App, which: Which) {
    let ui = cx.global_mut::<Ui>();
    if ui.busy.is_some() {
        return;
    }
    ui.busy = Some(Busy::Run);
    ui.started = Some(Instant::now());
    ui.tab = Tab::Log;
    ui.unread = 0;
    note_line(ui, ui::head_line(which.title()));
    let ctx = ctx_for(ui, false);
    let tx = ui.tx.clone();
    std::thread::spawn(move || {
        ui::guarded(
            |ev| {
                let _ = tx.unbounded_send(ev);
            },
            Ev::Line,
            || {
                let list = ops::server(&ctx.cfg);
                let r = match which {
                    Which::Change => common::run::apply_all(&ctx, &list, None, true).map(|_| ()),
                    Which::CleanStack => common::run::clean_apply(&ctx, &list, CleanLevel::Stack),
                    Which::CleanData => common::run::clean_apply(&ctx, &list, CleanLevel::Data),
                    // Установка начинается сверкой комплекта и кончается
                    // проверкой: обещаниям тут не место.
                    Which::Install | Which::Check => {
                        ops::kit_is_intact(&ctx).and_then(|_| common::run::apply_all(&ctx, &list, None, false)).and_then(|_| checked(&ctx))
                    }
                };
                Ev::Done(r.err().map(|e| format!("{e:#}")))
            },
            || Ev::Done(Some(ui::PANIC_TEXT.into())),
        );
    });
}

/// Проверки после установки: факты строками, отчёт файлом, провал — ошибкой.
fn checked(ctx: &Ctx) -> anyhow::Result<()> {
    let checks = verify::run(ctx);
    let (_, failed) = common::check::summary(&checks);
    let path = verify::report(ctx, &checks)?;
    for line in ui::check_lines(&checks) {
        ctx.emit(match line.kind {
            Kind::Ok => Event::Ok(line.text),
            Kind::Fail => Event::Fail(line.text),
            Kind::Warn => Event::Warn(line.text),
            _ => Event::Say(line.text),
        });
    }
    ctx.say(format!("Отчёт: {}", path.display()));
    anyhow::ensure!(failed == 0, "проверка не пройдена");
    Ok(())
}

/// Кнопка «Проверить»: только факты, ничего не меняет.
pub fn run_check(cx: &mut App) {
    let ui = cx.global_mut::<Ui>();
    if ui.busy.is_some() {
        return;
    }
    ui.busy = Some(Busy::Check);
    ui.started = Some(Instant::now());
    // Проверка идёт при открытой вкладке «Проверка»: факты появляются там,
    // где человек их ждёт, как и в прежнем окне.
    ui.tab = Tab::Check;
    note_line(ui, ui::head_line(Which::Check.title()));
    let ctx = ctx_for(ui, true);
    let tx = ui.tx.clone();
    std::thread::spawn(move || {
        let send = |ev| {
            let _ = tx.unbounded_send(ev);
        };
        // Один и тот же отправитель достаётся и обёртке, и телу ниже: `send`
        // держит `tx` по ссылке, поэтому само замыкание `Copy` — копии хватает
        // обоим, и заимствовать его незачем.
        ui::guarded(
            send,
            Ev::Line,
            || {
                let checks = verify::run(&ctx);
                // Путь отчёта — строкой в журнал: в прежнем окне об нём
                // говорила подсказка под списком.
                if let Ok(path) = verify::report(&ctx, &checks) {
                    send(Ev::Line(ui::Line::say(format!("Отчёт: {}", path.display()))));
                }
                Ev::Checks(checks)
            },
            // Паника — не «проверка прошла»: пустой список фактов читался бы
            // как «0 провалов». Конец работы с ошибкой, факты не трогаем.
            || Ev::Done(Some(ui::PANIC_TEXT.into())),
        );
    });
}

fn handle(ev: Ev, cx: &mut App) {
    match ev {
        Ev::Addresses(list) => {
            cx.global_mut::<Ui>().addresses = list;
            cx.refresh_windows();
        }
        Ev::Line(line) => {
            note_line(cx.global_mut::<Ui>(), line);
            cx.refresh_windows();
        }
        Ev::Done(err) => {
            let ui = cx.global_mut::<Ui>();
            ui.busy = None;
            let elapsed = ui.started.take();
            let line = match err {
                Some(e) => Line::fail(e),
                None => match elapsed {
                    Some(since) => Line::ok(format!("Готово за {}", ui::took(since.elapsed()))),
                    None => Line::ok("Готово."),
                },
            };
            note_line(ui, line.clone());
            ui.status = Some(line);
            cx.refresh_windows();
            refresh_ops(cx);
        }
        Ev::Checks(checks) => {
            let ui = cx.global_mut::<Ui>();
            ui.busy = None;
            let elapsed = ui.started.take();
            ui.failed_checks = checks.iter().filter(|c| !c.ok).count();
            ui.checks = checks.clone();
            for line in ui::check_lines(&checks) {
                note_line(ui, line);
            }
            let text = match elapsed {
                Some(since) => {
                    format!("Проверка: {} из {} — за {}", checks.len() - ui.failed_checks, checks.len(), ui::took(since.elapsed()))
                }
                None => format!("Проверка: {} из {}", checks.len() - ui.failed_checks, checks.len()),
            };
            let line = if ui.failed_checks == 0 { Line::ok(text) } else { Line::fail(text) };
            ui.status = Some(line);
            cx.refresh_windows();
        }
        Ev::Ops(ops, facts) => {
            let ui = cx.global_mut::<Ui>();
            ui.pending = ui.pending.saturating_sub(1);
            if !ops.is_empty() {
                ui.ops = ops;
            }
            if facts.is_some() {
                ui.facts = facts;
            }
            cx.refresh_windows();
        }
    }
}

/// Окно без аргументов: двойной щелчок по файлу из комплекта.
pub fn launch(kit: Kit) -> anyhow::Result<()> {
    // ponytail: усечение сбросом, ротация — когда кому-то понадобится история
    let log_path = kit.path(LOG_FILE);
    if std::fs::metadata(&log_path).map(|m| m.len() > 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&log_path);
    }
    let (cfg, cfg_error) = ui::load_or_default(&kit);
    let file_exists = kit.path(crate::cfg::FILE).exists();
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        common::gui::fonts(cx);
        common::gui::palette(cx);
        // Одна тема, светлая: выбора тем у этой программы нет.
        Theme::change(ThemeMode::Light, None, cx);
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Ev>();
        cx.set_global(Ui {
            kit,
            cfg,
            ops: Vec::new(),
            failed_checks: 0,
            checks: Vec::new(),
            facts: None,
            addresses: Vec::new(),
            log: Vec::new(),
            unread: 0,
            tab: Tab::default(),
            started: None,
            busy: None,
            pending: 0,
            status: None,
            cfg_error: cfg_error.clone(),
            window: None,
            panel: None,
            dialog_open: false,
            tx,
        });
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(ev) = rx.next().await {
                cx.update(|cx| handle(ev, cx));
            }
        })
        .detach();
        // Про сам файл параметров окно говорит сразу: и когда он не
        // прочитался, и когда его нет вовсе.
        if let Some(text) = ui::cfg_notice(file_exists, cfg_error.as_deref()) {
            let kind = if cfg_error.is_some() { Kind::Fail } else { Kind::Warn };
            note_line(cx.global_mut::<Ui>(), Line { kind, text });
        }
        window::open(cx);
        refresh_ops(cx);
        // Адреса собираем в стороне: разрешение имени машины ходит в DNS, а
        // окно должно открыться сразу.
        let tx = cx.global::<Ui>().tx.clone();
        std::thread::spawn(move || {
            let _ = tx.unbounded_send(Ev::Addresses(ui::local_addresses()));
        });
    });
    Ok(())
}
