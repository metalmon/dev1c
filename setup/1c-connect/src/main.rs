//! 1c-connect: подключение машины с 1С к среде разработки. Лежит на флешке,
//! ничего не устанавливает. Без аргументов — состояние (окно появится
//! следующим этапом), с аргументами — команды; у любой есть `--dry-run`.

use anyhow::Result;
use clap::{Parser, Subcommand};
use common::kit::Kit;
use common::op::CleanLevel;
use common::{AlwaysDefault, Event, Prompt, StdinPrompt};
use one_c_connect::ops_host::{self, Ctx};
use one_c_connect::{bridge, cfg, tunnel, verify};
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
    /// Проверки по фактам; отчёт — last-check.txt.
    Check,
    /// Сверить комплект с контрольными суммами.
    Verify,
    /// Собрать список контрольных сумм (при сборке поставки).
    Checksums,
    /// Напечатать образец 1c-connect.toml.
    Example,
}

fn main() {
    if let Err(e) = run() {
        eprintln!();
        eprintln!("Остановлено: {e:#}");
        std::process::exit(1);
    }
}

fn make_ctx(kit: Kit, dry_run: bool, yes: bool) -> Result<Ctx> {
    let cfg = cfg::Cfg::load(&kit)?;
    let log_path = kit.cmd_log();
    let sink = Arc::new(move |e: Event| {
        let log = |line: String| {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
                use std::io::Write;
                let _ = writeln!(f, "{line}");
            }
        };
        match e {
            Event::Ok(t) => println!("  [ ok ] {t}"),
            Event::Warn(t) => println!("  [ !! ] {t}"),
            Event::Fail(t) => println!("  [FAIL] {t}"),
            Event::Say(t) => println!("{t}"),
            Event::Command(c) => log(format!("$ {c}")),
            Event::Output { command, text } => log(format!("# {command}\n{text}")),
        }
    });
    let prompt: Box<dyn Prompt> = if yes { Box::new(AlwaysDefault) } else { Box::new(StdinPrompt) };
    Ok(Ctx { kit, cfg, dry_run, sink, prompt })
}

fn run() -> Result<()> {
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
            println!("Установить расширение: 1c-connect install. Подключиться: 1c-connect connect.");
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

/// Туннель плюс мост, пока не нажмут Ctrl+C. На порт mcp-1c — процесс на
/// stdio, на порт веб-клиента — локальный веб-сервер.
fn connect(ctx: &Ctx) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let ctrl = tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = stop_tx.send(true);
        });
        let cfg = ctx.cfg.clone();
        let kit = ctx.kit.clone();
        let ports = cfg.ports();
        let web_port = cfg.web_port();
        let sink = ctx.sink.clone();
        let serve = move |inc: tunnel::Incoming| {
            let stream = inc.channel.into_stream();
            if inc.port == ports.mcp {
                let cmd = bridge::mcp_command(&cfg, &kit);
                let sink = sink.clone();
                tokio::spawn(bridge::pipe_process(stream, cmd, move |code, err| {
                    if code != 0 {
                        sink(Event::Warn(format!("mcp-1c завершился с кодом {code}: {err}")));
                    }
                }));
            } else if inc.port == ports.web {
                tokio::spawn(bridge::pipe_tcp(stream, web_port));
            }
        };
        let sink2 = ctx.sink.clone();
        println!(
            "Подключаюсь к {} как {}; порты {} (mcp-1c) и {} (веб-клиент). Ctrl+C — отключиться.",
            ctx.cfg.host.server, ctx.cfg.host.workstation, ports.mcp, ports.web
        );
        let r = tunnel::run(&ctx.cfg, ctx.kit.path("known_hosts"), move |e| sink2(e), serve, stop_rx).await;
        ctrl.abort();
        r
    })?;
    println!("Отключено.");
    Ok(())
}
