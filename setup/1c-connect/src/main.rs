// Оконная сборка: двойной щелчок не должен открывать чёрное окно консоли.
// Для команд консоль возвращает common::console::attach; в отладочной сборке
// она остаётся всегда.
#![cfg_attr(all(not(debug_assertions), feature = "gui"), windows_subsystem = "windows")]

//! 1c-connect: подключение машины с 1С к среде разработки. Лежит на флешке,
//! ничего не устанавливает. Без аргументов — окно и значок в трее, с
//! аргументами — команды; у любой есть `--dry-run`.

use anyhow::Result;
use clap::{Parser, Subcommand};
use common::kit::Kit;
use common::op::CleanLevel;
use common::{AlwaysDefault, Event, Prompt, StdinPrompt};
use one_c_connect::ops_host::{self, Ctx};
use one_c_connect::{cfg, flow, project, session, verify};
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "1c-connect",
    version = env!("CARGO_PKG_VERSION"),
    about = "Подключение машины с 1С к среде разработки",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    help_template = "{about}\n\nИспользование: {usage}\n\nКоманды:\n{subcommands}\n\nКлючи:\n{options}"
)]
struct Cli {
    /// Показать эту справку.
    #[arg(short = 'h', long = "help", action = clap::ArgAction::Help, global = true)]
    help: Option<bool>,
    /// Показать версию программы.
    #[arg(short = 'V', long = "version", action = clap::ArgAction::Version)]
    version: Option<bool>,
    /// Печатать план действий и ничего не делать.
    #[arg(long, global = true)]
    dry_run: bool,
    /// Каталог комплекта (по умолчанию — рядом с программой, или ONEC_CONNECT_KIT).
    #[arg(long, global = true)]
    kit: Option<String>,
    /// Не задавать вопросов: значения по умолчанию.
    #[arg(short = 'y', long, global = true)]
    yes: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Состояние, без изменений.
    Status,
    /// Установить расширение mcp-1c в базу.
    Install,
    /// Снять расширение из базы и забыть отпечаток сервера.
    Uninstall,
    /// Поднять туннель и мост; работает, пока не нажать Ctrl+C.
    Connect,
    /// Выгрузить конфигурацию в src (cf_decompile) и отправить ветку в Gitea.
    Export {
        /// Сообщение коммита.
        #[arg(long, default_value = "выгрузка")]
        message: String,
    },
    /// Забрать свою ветку из Gitea и загрузить в базу (cf_load, инкрементом).
    Import,
    /// Проверки по фактам; отчёт — last-check.txt.
    Check,
    /// Сверить комплект с контрольными суммами.
    Verify,
    /// Собрать список контрольных сумм (при сборке поставки).
    Checksums,
    /// Напечатать образец 1c-connect.toml.
    Example,
}

/// Запуск окна: без аргументов (двойной щелчок) или `--tray` (автозапуск
/// при входе в систему, см. `gui::autostart`). Ни тому, ни другому консоль
/// не нужна — `console::attach` с одним аргументом выделил бы её, и при
/// каждом входе в Windows всплывало бы чёрное окно.
#[cfg(feature = "gui")]
fn gui_launch() -> Option<bool> {
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_] => Some(false),
        [_, a] if a == "--tray" => Some(true),
        _ => None,
    }
}

fn main() {
    #[cfg(feature = "gui")]
    let console = gui_launch().is_none();
    #[cfg(not(feature = "gui"))]
    let console = true;
    if console {
        common::console::attach();
    }
    if let Err(e) = run() {
        eprintln!();
        eprintln!("Остановлено: {e:#}");
        std::process::exit(1);
    }
}

fn make_ctx(kit: Kit, dry_run: bool, yes: bool) -> Result<Ctx> {
    let cfg = cfg::Cfg::load(&kit)?;
    let k = kit.clone();
    let sink = Arc::new(move |e: Event| {
        if k.journal(&e) {
            return;
        }
        match e {
            Event::Ok(t) => println!("  [ ok ] {t}"),
            Event::Warn(t) => println!("  [ !! ] {t}"),
            Event::Fail(t) => println!("  [FAIL] {t}"),
            Event::Say(t) => println!("{t}"),
            Event::Progress(p) => println!("== {}", p.phase.title()),
            Event::Command(_) | Event::Output { .. } => {}
        }
    });
    let prompt: Box<dyn Prompt> = if yes { Box::new(AlwaysDefault) } else { Box::new(StdinPrompt) };
    Ok(Ctx { kit, cfg, dry_run, sink, prompt })
}

fn run() -> Result<()> {
    // Без аргументов — окно: так программу запускают двойным щелчком.
    // Сборка без окна показывает состояние — самое безобидное из возможного.
    // `--tray` не попадает в `Cli`: это скрытый старт автозапуском при входе
    // в систему (см. `gui::autostart`), проверяется до `Cli::parse()`, как
    // и `len() == 1`.
    #[cfg(feature = "gui")]
    if let Some(hidden) = gui_launch() {
        return one_c_connect::gui::launch(Kit::discover("ONEC_CONNECT_KIT")?, hidden);
    }
    let cli = Cli::parse();
    let kit = match &cli.kit {
        Some(p) => Kit::at(p),
        None => Kit::discover("ONEC_CONNECT_KIT")?,
    };
    if let Some(Cmd::Example) = cli.cmd {
        print!("{}", cfg::EXAMPLE);
        return Ok(());
    }
    let ctx = make_ctx(kit, cli.dry_run, cli.yes)?;
    let ops = ops_host::host();
    match cli.cmd.unwrap_or(Cmd::Status) {
        Cmd::Example => unreachable!(),
        Cmd::Status => {
            println!("Комплект: {}", ctx.kit.root.display());
            println!("Рабочее место {} → среда {}", ctx.cfg.host.workstation, ctx.cfg.host.server);
            println!();
            for (id, title, st) in common::run::status(&ctx, &ops) {
                println!("  {:<16} {:<26} {}", id, title, st.label());
            }
            println!();
            println!("Установить: 1c-connect install. Подключиться: 1c-connect connect. Выгрузить в git: 1c-connect export.");
        }
        Cmd::Install => {
            common::run::apply_all(&ctx, &ops, None, false)?;
            if !ctx.dry_run {
                println!();
                check(&ctx, None)?;
            }
        }
        Cmd::Uninstall => {
            let ext = ops.iter().find(|o| o.id() == "host.extension").expect("extension op");
            for a in ext.revert(&ctx, CleanLevel::Stack).unwrap_or_default() {
                println!("   · {}", a.0);
            }
            if ctx.dry_run {
                println!("План напечатан, на машине ничего не менялось.");
                return Ok(());
            }
            if !ctx.prompt.confirm("Снять расширение из базы?", false) && !cli.yes {
                println!("Отменено.");
                return Ok(());
            }
            ext.revert_apply(&ctx, CleanLevel::Stack)?;
        }
        Cmd::Connect => connect(&ctx)?,
        Cmd::Export { message } => {
            let t = flow::export(&ctx, &message, &project::NEVER).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{t}");
        }
        Cmd::Import => {
            let t = flow::import(&ctx, &project::NEVER).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{t}");
        }
        Cmd::Check => check(&ctx, None)?,
        Cmd::Verify => {
            let r = common::integrity::verify(&ctx.kit, |i, n, p| println!("  {i}/{n} {p}"))?;
            for e in r.entries.iter().filter(|e| e.state != common::integrity::FileState::Ok) {
                println!("  [FAIL] {}: {:?}", e.path, e.state);
            }
            anyhow::ensure!(r.ok(), "комплект повреждён — скопируйте его заново");
            println!("Комплект цел.");
        }
        Cmd::Checksums => {
            let n = common::integrity::write_list(&ctx.kit, |p| println!("  {p}"))?;
            println!("Записано {n} сумм в checksums.txt");
        }
    }
    Ok(())
}

fn check(ctx: &Ctx, tunnel_state: Option<&str>) -> Result<()> {
    println!("Проверка:");
    let checks = verify::run(ctx, tunnel_state);
    common::check::print(&checks);
    let (p, f) = common::check::summary(&checks);
    let path = ctx.kit.path("last-check.txt");
    let text = common::check::render("1c-connect", env!("CARGO_PKG_VERSION"), &verify::cfg_for_report(&ctx.cfg), &checks);
    common::kit::write_text(&path, &text)?;
    println!();
    println!("Пройдено {p}, провалено {f}. Отчёт: {}", path.display());
    anyhow::ensure!(f == 0, "проверка не пройдена");
    Ok(())
}

/// Туннель плюс мост, пока не нажмут Ctrl+C или сеанс не упадёт сам.
fn connect(ctx: &Ctx) -> Result<()> {
    anyhow::ensure!(
        ctx.kit.path(cfg::FILE).exists(),
        "в комплекте нет {} — создайте его из образца: 1c-connect example > {}",
        cfg::FILE,
        cfg::FILE
    );
    let ports = ctx.cfg.ports();
    println!(
        "Подключаюсь к {} как {}; порты {} (mcp-1c) и {} (веб-клиент). Ctrl+C — отключиться.",
        ctx.cfg.host.server, ctx.cfg.host.workstation, ports.mcp, ports.web
    );
    let mut s = session::start(ctx.cfg.clone(), ctx.kit.clone(), ctx.sink.clone(), |_| {});
    let rt = tokio::runtime::Runtime::new()?;
    let r = rt.block_on(async {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => Ok(()),
            r = s.wait() => r,
        }
    });
    let stopped = s.stop();
    r?;
    stopped?;
    println!("Отключено.");
    Ok(())
}
