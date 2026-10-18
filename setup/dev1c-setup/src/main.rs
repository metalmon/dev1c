// Оконная сборка: двойной щелчок не должен открывать чёрное окно консоли.
// Для команд консоль возвращает common::console::attach; в отладочной сборке
// она остаётся всегда.
#![cfg_attr(all(not(debug_assertions), feature = "gui"), windows_subsystem = "windows")]

//! Установка среды разработки 1С. Без аргументов — окно, с аргументами —
//! команды. У любой есть `--dry-run`: печатает план и ничего не делает.

mod cfg;
#[cfg(feature = "gui")]
mod gui;
mod llama;
mod ops;
mod ops_llama;
mod ui;
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
    common::console::attach();
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
            Event::Progress(p) => println!("{}", p.phase.title()),
            Event::Command(_) | Event::Output { .. } => {}
        }
    });
    let prompt: Box<dyn Prompt> = if yes { Box::new(AlwaysDefault) } else { Box::new(StdinPrompt) };
    Ok(Ctx { kit, cfg, dry_run, sink, prompt })
}

fn run() -> Result<()> {
    // Без аргументов — окно: так программу запускают двойным щелчком.
    // Сборка без окна показывает состояние — самое безобидное из возможного.
    #[cfg(feature = "gui")]
    if std::env::args().len() == 1 {
        return gui::launch(Kit::discover("DEV1C_KIT")?);
    }
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
    let ops = ops::server(&ctx.cfg);
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
            // Сверка комплекта до первого шага: ставить из битого архива —
            // значит разбирать потом ошибку движка контейнеров.
            if only.is_none() && !ctx.dry_run {
                ops::kit_is_intact(&ctx)?;
            }
            common::run::apply_all(&ctx, &ops, only.as_deref(), false)?;
            if ctx.dry_run || only.is_some() {
                return Ok(());
            }
            println!();
            check(&ctx)?;
            let [rdp, gitea, _model, ssh] = ctx.cfg.ports();
            println!();
            println!("Среда запущена:");
            println!("  Рабочие места   mstsc /v:<адрес этой машины>:{rdp}  —  dev1/dev1, dev2/dev2, dev3/dev3");
            println!("  Репозитории     http://<адрес>:{gitea}");
            println!("  Сервер модели   {} ({})", ctx.cfg.model_url(), ctx.cfg.model_alias());
            println!("  Для 1c-connect  адрес этой машины и порт {ssh}");
        }
        Cmd::Change { values } => {
            apply_changes(&mut ctx, &values)?;
            let ops = ops::server(&ctx.cfg);
            let touched = common::run::apply_all(&ctx, &ops, None, true)?;
            // Служба пересоздана — числа окна могли измениться: замер заново,
            // иначе контейнер и Zed остаются с прежними.
            if touched > 0 && ctx.cfg.form() != cfg::Form::Remote && !ctx.dry_run {
                common::run::apply_all(&ctx, &ops, Some("llama.answers"), false)?;
                common::run::apply_all(&ctx, &ops, None, true)?;
            }
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
            "model.serve" => ctx.cfg.model.serve = val.into(),
            "model.url" => ctx.cfg.model.url = val.into(),
            "model.key" => ctx.cfg.model.key = val.into(),
            "model.name" => ctx.cfg.model.name = val.into(),
            "model.file" => ctx.cfg.model.file = val.into(),
            "model.port" => ctx.cfg.model.port = val.parse().context("model.port — число")?,
            "model.agents" => ctx.cfg.model.agents = val.parse().context("model.agents — число")?,
            "model.fit_ctx" => ctx.cfg.model.fit_ctx = val.parse().context("model.fit_ctx — число")?,
            other => anyhow::bail!(
                "неизвестный параметр {other}; есть: port, memory, gpu, engine, model.serve, model.file, model.url, model.key, model.name, model.port, model.agents, model.fit_ctx"
            ),
        }
    }
    ctx.cfg.validate()?;
    if !ctx.dry_run {
        common::kit::write_text(&ctx.kit.path(cfg::FILE), &cfg::Cfg::render(&ctx.cfg))?;
        ctx.ok(format!("{} обновлён", cfg::FILE));
    }
    Ok(())
}

fn check(ctx: &Ctx) -> Result<()> {
    println!("Проверка среды:");
    let checks = verify::run(ctx);
    common::check::print(&checks);
    let (p, f) = verify::summary(&checks);
    let path = verify::report(ctx, &checks)?;
    println!();
    println!("Пройдено {p}, провалено {f}. Отчёт: {}", path.display());
    anyhow::ensure!(f == 0, "проверка не пройдена");
    Ok(())
}
