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
use common::{AlwaysDefault, Event, Prompt, Runner, StdinPrompt};
use one_c_connect::ops_host::{self, Ctx};
use one_c_connect::{cfg, flow, licence, project, session, single_instance, verify};
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
    /// Без окна: поднять сеанс и держать его до Ctrl+C/SIGTERM. Для стенда и
    /// другого автозапуска без рабочего стола.
    Serve,
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
        let kit = ops_host::wire_state(Kit::discover("ONEC_CONNECT_KIT")?);
        ops_host::ensure_state(&kit);
        return one_c_connect::gui::launch(kit, hidden);
    }
    let cli = Cli::parse();
    // `wire_state` разводит чтение и запись: из комплекта читаем, своё пишем
    // в состояние пользователя — в образе комплект общий и только для
    // чтения. Явный `--kit` этого не касается: он значит «вот этот комплект
    // целиком», и состояние остаётся в нём — иначе прогон на отдельном
    // комплекте читал бы и писал настоящее состояние человека.
    let kit = match &cli.kit {
        Some(p) => Kit::at(p),
        None => ops_host::wire_state(Kit::discover("ONEC_CONNECT_KIT")?),
    };
    ops_host::ensure_state(&kit);

    if let Some(Cmd::Example) = cli.cmd {
        print!("{}", cfg::EXAMPLE);
        return Ok(());
    }
    let ctx = make_ctx(kit, cli.dry_run, cli.yes)?;
    let ops = ops_host::host();
    let cmd = cli.cmd.unwrap_or(Cmd::Status);
    // Срок заглушки лицензии вышел — разовые команды, меняющие машину, не
    // работают (спека, п. 9). `status`, `check` и `example` остаются: человеку
    // надо чем-то показать поддержке. `connect`/`serve` тоже поднимаются —
    // отказывает мост на каждый вызов агента, с тем же текстом.
    if matches!(cmd, Cmd::Install | Cmd::Uninstall | Cmd::Export { .. } | Cmd::Import)
        && matches!(licence::state_now(), licence::State::Expired)
    {
        anyhow::bail!("{}", licence::REFUSAL);
    }
    match cmd {
        Cmd::Example => unreachable!(),
        Cmd::Status => {
            println!("Комплект: {}", ctx.kit.root.display());
            println!("Рабочее место {} → среда {}", ctx.cfg.host.workstation, ctx.cfg.host.server);
            println!();
            for (id, title, st) in common::run::status(&ctx, &ops) {
                println!("  {:<16} {:<26} {}", id, title, st.label());
            }
            println!();
            println!("Установить: 1c-connect install. Подключиться: 1c-connect connect. Выгрузить из 1С: 1c-connect export.");
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
        Cmd::Serve => serve(&ctx)?,
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
    let path = ctx.kit.state_path("last-check.txt");
    let text = common::check::render("1c-connect", env!("CARGO_PKG_VERSION"), &verify::cfg_for_report(&ctx.cfg), &checks);
    common::kit::write_text(&path, &text)?;
    println!();
    println!("Пройдено {p}, провалено {f}. Отчёт: {}", path.display());
    anyhow::ensure!(f == 0, "проверка не пройдена");
    Ok(())
}

/// Туннель плюс мост, пока не нажмут Ctrl+C или сеанс не упадёт сам.
fn connect(ctx: &Ctx) -> Result<()> {
    let params = ctx.kit.state_path(cfg::FILE);
    anyhow::ensure!(
        params.exists(),
        "нет файла параметров {} — создайте его из образца: 1c-connect example > {}",
        params.display(),
        params.display()
    );
    // У рабочего места один владелец (спека): вторая копия не борется за
    // порты и мьютекс проекта с этой — см. `single_instance`.
    let _guard = single_instance::acquire(&ctx.kit, &ctx.cfg.host.workstation)?;
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

/// Текст для журнала, если публикация не поднялась — `None`, если всё в
/// порядке. Отдельная чистая функция (не внутри `serve`) — чтобы решение
/// «что сказать при отказе» проверялось без настоящего httpd/apache и без
/// реального сеанса.
fn publication_failure_text(result: &Result<()>) -> Option<String> {
    result.as_ref().err().map(|e| format!("публикация базы не поднята: {e:#}"))
}

/// То же самое, что `connect`, но без вопросов и без печати плана: для
/// запуска без рабочего стола (стенд, автозапуск). Останавливается по
/// Ctrl+C, а на Linux — ещё и по SIGTERM, чтобы `docker stop` и остановка
/// стенда закрывали мосты штатно и срабатывала уборка группы процессов.
fn serve(ctx: &Ctx) -> Result<()> {
    let params = ctx.kit.state_path(cfg::FILE);
    anyhow::ensure!(
        params.exists(),
        "нет файла параметров {} — создайте его из образца: 1c-connect example > {}",
        params.display(),
        params.display()
    );
    // Тот же единственный владелец рабочего места, что у `connect` — см.
    // `single_instance`.
    let _guard = single_instance::acquire(&ctx.kit, &ctx.cfg.host.workstation)?;
    // Без сеанса на стенде никто больше базу не публикует (см. спеку
    // «HTTP-сервис базы ведёт коннектор») — поднимаем её перед тем, как
    // начать слушать, тем же сервисом, что умрёт вместе с этим процессом
    // (ops_host::ensure_publication, Lifetime::WithCaller).
    //
    // Отказ не останавливает serve: транспорт MCP (порты mcp-1c/vrunner) не
    // зависит от публикации вовсе — она нужна только агенту, который зовёт
    // HTTP-сервис базы отдельно. Оборвать здесь весь сеанс значило бы гасить
    // работающий транспорт из-за соседней проблемы (например, порт занят
    // чужим apache2), которую человек ещё не разрешил — и заставлять
    // перезапускать всё заново, когда освободит порт. Громко сообщаем
    // (Fail — это и строка в журнале, и то, что видно на экране) и идём
    // дальше: `1c-connect check` уже знает отдельный факт о публикации
    // (`host.publication`), там отказ и смотреть.
    if let Some(text) = publication_failure_text(&ops_host::ensure_publication(ctx)) {
        ctx.fail(text);
    }
    let mut s = session::start(ctx.cfg.clone(), ctx.kit.clone(), ctx.sink.clone(), |_| {});
    let rt = tokio::runtime::Runtime::new()?;
    let r = rt.block_on(async {
        // Условный атрибут внутри `select!` макрос не принимает, поэтому
        // ожидание SIGTERM — отдельное будущее: на Linux оно ждёт сигнал, на
        // Windows не завершается никогда и ветка просто не срабатывает.
        let term = async {
            #[cfg(unix)]
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut t) => {
                    t.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
            #[cfg(not(unix))]
            std::future::pending::<()>().await
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => Ok(()),
            _ = term => Ok(()),
            r = s.wait() => r,
        }
    });
    let stopped = s.stop();
    r?;
    stopped?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_is_a_subcommand() {
        let cli = Cli::try_parse_from(["1c-connect", "serve"]).expect("serve разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Serve)));
    }

    #[test]
    fn serve_takes_no_flags_of_its_own() {
        assert!(Cli::try_parse_from(["1c-connect", "serve", "--port", "1"]).is_err());
    }

    #[test]
    fn a_failed_publication_is_reported_but_serve_keeps_going() {
        // Решение из отчёта: занятый порт публикации не должен обрывать
        // сеанс MCP — только громко сообщить. Здесь проверяем именно текст
        // и факт «есть сообщение», не настоящий serve (тот поднимает
        // реальные сокеты и ждёт Ctrl+C).
        let err: Result<()> = Err(anyhow::anyhow!("HTTP-сервис базы не поднят: порт 127.0.0.1:8083 уже занят"));
        let text = publication_failure_text(&err).expect("отказ должен дать сообщение");
        assert!(text.contains("публикация"), "{text}");
        assert!(text.contains("8083"), "{text}");
    }

    #[test]
    fn a_successful_publication_has_nothing_to_report() {
        let ok: Result<()> = Ok(());
        assert!(publication_failure_text(&ok).is_none());
    }
}
