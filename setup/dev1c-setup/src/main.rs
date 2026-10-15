//! Установка среды разработки 1С. Без аргументов — состояние (окно появится
//! следующим этапом), с аргументами — команды. У любой есть `--dry-run`:
//! печатает план и ничего не делает.

mod cfg;
mod ops;
mod verify;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use common::kit::Kit;
use common::op::CleanLevel;
use common::{AlwaysDefault, Event, Prompt, Runner, StdinPrompt};
use ops::Ctx;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "dev1c-setup",
    version = env!("CARGO_PKG_VERSION"),
    about = "Установка среды разработки 1С",
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
    /// Каталог комплекта (по умолчанию — рядом с программой, или DEV1C_KIT).
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
    /// Применить всё, чего не хватает, и проверить.
    Install {
        /// Только одна операция, по идентификатору (image, container…).
        #[arg(long)]
        only: Option<String>,
    },
    /// Состояние операций, без изменений на машине.
    Status,
    /// Изменить параметр (port=4000, memory=16g, gpu=false) и применить только разошедшееся.
    Change { values: Vec<String> },
    /// Проверки по фактам; код возврата 1 при провале. Отчёт — last-check.txt.
    Check,
    /// Снять контейнер (--stack) или и данные тоже (--data).
    Clean {
        #[arg(long)]
        stack: bool,
        #[arg(long)]
        data: bool,
    },
    /// Сверить комплект с контрольными суммами.
    Verify,
    /// Собрать список контрольных сумм (при сборке поставки).
    Checksums,
    /// Напечатать образец dev1c.toml.
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
    // В терминал идут приговоры и строки плана; команды и их вывод — в журнал.
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
        None => Kit::discover("DEV1C_KIT")?,
    };
    if let Some(Cmd::Example) = cli.cmd {
        print!("{}", cfg::EXAMPLE);
        return Ok(());
    }
    let mut ctx = make_ctx(kit, cli.dry_run, cli.yes)?;
    let ops = ops::server();
    match cli.cmd.unwrap_or(Cmd::Status) {
        Cmd::Example => unreachable!(),
        Cmd::Status => {
            println!("Комплект: {}", ctx.kit.root.display());
            println!();
            for (id, title, st) in common::run::status(&ctx, &ops) {
                println!("  {:<10} {:<20} {}", id, title, st.label());
            }
            println!();
            println!("Применить то, чего не хватает: dev1c-setup install");
        }
        Cmd::Install { only } => {
            common::run::apply_all(&ctx, &ops, only.as_deref(), false)?;
            if ctx.dry_run || only.is_some() {
                return Ok(());
            }
            println!();
            check(&ctx)?;
            let [rdp, gitea, model, ssh] = ctx.cfg.ports();
            println!();
            println!("Среда запущена:");
            println!("  Рабочие места   mstsc /v:<адрес этой машины>:{rdp}  —  dev1/dev1, dev2/dev2, dev3/dev3");
            println!("  Репозитории     http://<адрес>:{gitea}");
            println!("  Сервер модели   http://<адрес>:{model}");
            println!("  Для 1c-connect  адрес этой машины и порт {ssh}");
        }
        Cmd::Change { values } => {
            apply_changes(&mut ctx, &values)?;
            common::run::apply_all(&ctx, &ops, None, true)?;
        }
        Cmd::Check => check(&ctx)?,
        Cmd::Clean { stack, data } => {
            anyhow::ensure!(stack || data, "укажите уровень: --stack (снять контейнер) или --data (и удалить данные)");
            let level = if data { CleanLevel::Data } else { CleanLevel::Stack };
            for (id, acts) in common::run::clean_plan(&ctx, &ops, level) {
                println!("== {id}");
                for a in acts {
                    println!("   · {a}");
                }
            }
            if ctx.dry_run {
                println!("План напечатан, на машине ничего не менялось.");
                return Ok(());
            }
            if data {
                let name = ctx.prompt.ask(
                    "Удалить данные: репозитории, проекты, настройки. Введите имя контейнера для подтверждения",
                    "",
                );
                anyhow::ensure!(name == ctx.cfg.server.name, "имя не совпало, ничего не удалено");
            }
            common::run::clean_apply(&ctx, &ops, level)?;
        }
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

/// `port=4000 memory=16g gpu=false` → в toml комплекта. Правится файл, не
/// память: следующий запуск должен видеть новые значения как желаемые.
fn apply_changes(ctx: &mut Ctx, values: &[String]) -> Result<()> {
    anyhow::ensure!(!values.is_empty(), "что менять? Например: dev1c-setup change port=4000");
    for v in values {
        let (k, val) = v.split_once('=').with_context(|| format!("ожидалось ключ=значение, получено {v}"))?;
        match k {
            "port" => ctx.cfg.server.port = val.parse().context("port — число")?,
            "memory" => ctx.cfg.server.memory = val.into(),
            "gpu" => ctx.cfg.server.gpu = matches!(val, "true" | "да" | "1"),
            "engine" => ctx.cfg.server.engine = val.into(),
            "model.url" => ctx.cfg.model.url = val.into(),
            "model.key" => ctx.cfg.model.key = val.into(),
            "model.name" => ctx.cfg.model.name = val.into(),
            "model.file" => ctx.cfg.model.file = val.into(),
            other => anyhow::bail!(
                "неизвестный параметр {other}; есть: port, memory, gpu, engine, model.url, model.key, model.name, model.file"
            ),
        }
    }
    ctx.cfg.validate()?;
    if !ctx.dry_run {
        common::kit::write_text(&ctx.kit.path(cfg::FILE), &toml::to_string(&ctx.cfg)?)?;
        ctx.ok(format!("{} обновлён", cfg::FILE));
    }
    Ok(())
}

fn check(ctx: &Ctx) -> Result<()> {
    println!("Проверка среды:");
    let checks = verify::run(ctx);
    for (i, c) in checks.iter().enumerate() {
        let mark = if c.info { "info" } else if c.ok { " ok " } else { "FAIL" };
        println!("{:>3}. [{mark}] {}: {}", i + 1, c.what, c.detail);
        if !c.hint.is_empty() {
            println!("           → {}", c.hint);
        }
    }
    let (p, f) = verify::summary(&checks);
    let path = verify::report(ctx, &checks)?;
    println!();
    println!("Пройдено {p}, провалено {f}. Отчёт: {}", path.display());
    anyhow::ensure!(f == 0, "проверка не пройдена");
    Ok(())
}
