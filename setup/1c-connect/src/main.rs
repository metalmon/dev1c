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
#[cfg(not(feature = "gui"))]
use one_c_connect::gate;
#[cfg(feature = "gui")]
use one_c_connect::gui::Start;
use one_c_connect::ops_host::{self, Ctx, McpServer};
use one_c_connect::{allow, cfg, door, flow, journal, licence, peer, peer_out, project, session, single_instance, verify};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "1c-connect",
    version = env!("CARGO_PKG_VERSION"),
    about = "Доступ агента к 1С: мост к инструментам и связь со второй стороной",
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
    /// Поднять мост к инструментам; работает, пока не нажать Ctrl+C.
    Connect,
    /// Без окна: поднять сеанс и держать его до Ctrl+C/SIGTERM. Для стенда и
    /// другого автозапуска без рабочего стола.
    Serve,
    /// Мост на стандартный поток — для MCP-клиента в редакторе (Zed, pi), у
    /// которого на машине человека нет проброшенного порта рабочего места
    /// (спека «MCP в редакторе ставит и держит коннектор»).
    Mcp {
        /// Какой из двух наших серверов: `1c` (mcp-1c) или `vrunner` (vrunner-mcp).
        server: McpServer,
    },
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
    /// Выдать новый код подключения; прежний код этого соединения гаснет.
    Code {
        /// Имя соединения, как оно записано в настройках; без имени — текущее.
        card: Option<String>,
    },
    /// Подключиться по коду, выданному на том компьютере, и дождаться ответа.
    Dial {
        /// Код подключения — строка, которую выдали на той стороне.
        code: String,
        /// Имя соединения, которым звонить; без имени — текущее.
        card: Option<String>,
    },
    /// Переключить текущее соединение; без имени — список, текущее отмечено.
    Use {
        /// Имя соединения, как оно записано в настройках.
        name: Option<String>,
    },
}

/// Запуск окна: без аргументов (двойной щелчок) или `--tray` (автозапуск
/// при входе в систему, см. `gui::autostart`). Ни тому, ни другому консоль
/// не нужна — `console::attach` с одним аргументом выделил бы её, и при
/// каждом входе в Windows всплывало бы чёрное окно.
#[cfg(feature = "gui")]
fn gui_launch() -> Option<bool> {
    hidden_for_args(&std::env::args().collect::<Vec<_>>())
}

/// Тот же разбор отдельно от `std::env::args`, чтобы ответ «с окном или без» проверялся тестом.
#[cfg(feature = "gui")]
fn hidden_for_args(args: &[String]) -> Option<bool> {
    match args {
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
        let text = format!("{e:#}");
        eprintln!();
        eprintln!("Остановлено: {text}");
        // Узнанный отказ платформы — строкой ниже, тем же текстом, что окно
        // пишет в журнал: в терминале человек так же не должен отличать
        // ограничение по числу сеансов от поломки по виду сырого сообщения
        // (`onec_error`).
        if let Some(c) = one_c_connect::onec_error::explain(&text) {
            eprintln!("{}", one_c_connect::onec_error::hint_line(c));
        }
        std::process::exit(1);
    }
}

/// `cfg` приходит уже загруженным — вызывающий (`run`) сам решает, каким
/// путём: `Cfg::load` с отказом на чужом шифртексте или `Cfg::load_unchecked`
/// без него (см. комментарий в `run` про разбор команд на «смотрят» и
/// «пользуются»). Здесь выбора нет и не может быть: один общий `Ctx` на все
/// подкоманды, кроме `example`/`mcp`.
fn make_ctx(kit: Kit, mut cfg: cfg::Cfg, dry_run: bool, yes: bool) -> Ctx {
    // Круг исправления задачи 1 плана «тонкий срез ценности»: без этого
    // `ctx.cfg.base_url()`/`verify` называли бы выведенный порт, а httpd
    // слушал бы ВЫБРАННЫЙ (`publication::Paths::of`) — агент шёл бы по
    // старому адресу и получал молчание.
    ops_host::settle_chosen_http_port(&mut cfg, &kit);
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
    Ctx { kit, cfg, dry_run, sink, prompt }
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
        ops_host::ensure_state(&kit)?;
        return one_c_connect::gui::launch(kit, Start::Window { hidden });
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
    // `example` печатает образец настроек и каталога состояния не трогает —
    // значит и отказа за него не получает (пункт 2 ревью целого). Решение
    // круга 4 было «отказ там, где каталог нужен, а не гейтом на старте», и
    // на Windows так и вышло само (`state_is_kit` истинно, отказывать не за
    // что), а на Linux состояние выведено из среды — и гейт стоял здесь,
    // валя ровно ту команду, ради которой его и разбирали. Остальным
    // подкомандам каталог состояния нужен на самом деле (журнал, замок,
    // отчёт проверки, сервер MCP), и отказ словами им положен — поэтому
    // проверка не убрана, а сдвинута за этот возврат.
    if let Some(Cmd::Example) = cli.cmd {
        print!("{}", cfg::EXAMPLE);
        return Ok(());
    }
    ops_host::ensure_state(&kit)?;
    // Тот же `kit`, что считал выше (`--kit`/`ONEC_CONNECT_KIT`/рядом с
    // программой) — свой второй `Kit::discover` внутри `run_mcp` был бы
    // другим комплектом: другой файл признака разрешения, другой журнал,
    // замок не по тому каталогу, что у окна (находка ревью задачи 3,
    // Important 1). Свой журнал файлом (ни строки в stdout — та труба занята
    // протоколом MCP), свои проверки срока и холодного замка — `ctx`/
    // `--dry-run`/`--yes` подкоманде не нужны, редактор поднимает её без них.
    if let Some(Cmd::Mcp { server }) = cli.cmd {
        return ops_host::run_mcp(server, kit);
    }
    let cmd = cli.cmd.unwrap_or(Cmd::Status);
    // Команды, которые только смотрят и рассказывают (`status`, `check`,
    // `verify`, `checksums`), не имеют права отказать на чужом шифртексте —
    // человек теряет диагностику ровно тогда, когда она нужнее всего (ревью
    // задачи 2, «Решение владельца»): `load_unchecked`, непрочитанный пароль
    // остаётся фактом в `Cfg::undecryptable`, а не отказом. Команды, которые
    // пароль ИСПОЛЬЗУЮТ — ходят в базу, SSH, Gitea (`connect`, `serve`,
    // подкоманда `mcp` — та и так на `Cfg::load` отдельной веткой выше,
    // `export`, `import`, `install`, `uninstall`) — остаются на `Cfg::load` и
    // отказывают как раньше: молчать там, где без пароля ничего не выйдет,
    // было бы хуже открытого текста.
    // `code` и `dial` — тоже сюда, и не поблажкой: пароля они не используют вовсе (вся их работа — стук в управляющую дверь), а
    // отказать им на чужом шифртексте значило бы, что человек не может ни выдать код, ни позвонить ровно тогда, когда настройки
    // переехали с другой машины, — то есть в первый же день работы по ним.
    let read_only =
        matches!(cmd, Cmd::Status | Cmd::Check | Cmd::Verify | Cmd::Checksums | Cmd::Use { .. } | Cmd::Code { .. } | Cmd::Dial { .. });
    let cfg = if read_only { cfg::Cfg::load_unchecked(&kit)? } else { cfg::Cfg::load(&kit)? };
    let ctx = make_ctx(kit, cfg, cli.dry_run, cli.yes);
    let ops = ops_host::host();
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
        Cmd::Mcp { .. } => unreachable!(),
        Cmd::Status => {
            println!("Комплект: {}", ctx.kit.root.display());
            println!("Соединение: {}", ctx.cfg.current().name);
            // `status` смотрит и рассказывает (`Cfg::load_unchecked` выше),
            // а не отказывает на чужом шифртексте — но молчать про это тоже
            // нельзя: это и есть диагностика, за которой человек сюда пришёл.
            if !ctx.cfg.undecryptable.is_empty() {
                println!("  [ !! ] {}", cfg::undecryptable_message(&ctx.cfg.undecryptable));
            }
            // Находки разбора — по той же причине и тем же видом строки:
            // `status` читает `load_unchecked` и обязан рассказать то, на чём
            // `serve`/`connect` отказали бы (круг починки 1, A и D).
            for note in &ctx.cfg.read_notes {
                println!("  [ !! ] {note}");
            }
            // Ушедшие ключи — сведение, а не находка: программа работает, а строки в файле просто больше никто не читает.
            for note in &ctx.cfg.gone_keys() {
                println!("  [info] {note}");
            }
            println!();
            for (id, title, st) in common::run::status(&ctx, &ops) {
                println!("  {:<16} {:<26} {}", id, title, st.label());
            }
            println!();
            // Дверь — последней строкой и первым клиентом: всё выше прочитано с диска и печатается всегда, а здесь впервые говорит ЖИВОЙ экземпляр. Разница названа словами нарочно: иначе человек не поймёт, свежее он видит или лежалое — а `status` он набирает ровно за этим.
            match door::ask(door::Ask::Status) {
                // Слова — в `door::status_lines` и только там: там же их проверяет тест, который держит разницу «о связи пока не сообщает» и «связей нет». Карточка без состояния контракта своей строки не получает вовсе — так же, как в списке окна (`contract::contract_word` отдаёт `None`), и слова за неё дверь не выдумывает.
                Ok(door::Answer::Status(v)) => {
                    for line in door::status_lines(&v) {
                        println!("{line}");
                    }
                }
                // Слова отказа — те самые, что живут в `door::NO_BACK_END`; второй их формулировки здесь нет.
                // Без своей точки после `{why}`: слова отказов кончаются по-разному, и приписанная тут точка однажды уже встала второй подряд (находка ревью круга 2).
                Err(why) => println!("{why}\nПоказанное выше прочитано с диска."),
                // Ответ не на тот вопрос — разные сборки по эту и ту сторону двери. Для `status` это не повод падать: всё, что он прочитал с диска, уже напечатано, и строкой ниже сказано, почему живой здесь не ответил.
                Ok(_) => println!("{}", door::WRONG_ANSWER),
            }
            println!();
            println!("Установить: 1c-connect install. Подключиться: 1c-connect connect. Выгрузить из 1С: 1c-connect export.");
        }
        Cmd::Install => {
            // Выводимые значения считаются один раз — здесь, при настройке, и
            // уезжают в toml; дальше правда только в файле (спека, «программа
            // не выдумывает адресов»). Дописываются до операций: их состояние
            // считается уже по записанному — в сухом прогоне тоже, иначе план
            // показывал бы «адрес среды не задан» там, где установка его сама
            // и запишет (находка ревью задачи 35).
            let mut ctx = ctx;
            let wrote = ops_host::write_derived_fields(&ctx.kit, &mut ctx.cfg, ctx.dry_run)?;
            if !wrote.is_empty() {
                let what = if ctx.dry_run { "будет записано" } else { "записано" };
                ctx.ok(format!("в {} {what}: {}", cfg::FILE, wrote.join(", ")));
            }
            common::run::apply_all(&ctx, &ops, None, false)?;
            if !ctx.dry_run {
                println!();
                check(&ctx)?;
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
        // `serve` — тот же бек, которому сказали не открывать окно при старте (спека, раздел 4): не отдельная программа и не
        // урезанная. Значок, замок, признак обслуживания, узел, публикация и сеанс у него те же самые, потому что путь подъёма
        // один — `gui::launch`; чем именно он разнится с двойным щелчком, названо вариантом `Start::Serve` целиком (шесть отличий,
        // см. его докстроку), а не условием на месте. Второго пути (своего `acquire`, своего сеанса) здесь больше нет: это была
        // «две меры на один вопрос» на уровне процесса.
        //
        // Сборка без окна идёт прежним путём `fn serve` ниже: значка там нет и быть не может, и спека разрешает это единственным
        // случаем — «машина без рабочего стола».
        #[cfg(feature = "gui")]
        Cmd::Serve => return one_c_connect::gui::launch(ctx.kit.clone(), Start::Serve),
        #[cfg(not(feature = "gui"))]
        Cmd::Serve => serve(&ctx)?,
        Cmd::Export { message } => {
            let t = flow::export(&ctx, &message, &project::NEVER).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{t}");
        }
        Cmd::Import => {
            let t = flow::import(&ctx, &project::NEVER).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{t}");
        }
        Cmd::Check => check(&ctx)?,
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
        Cmd::Use { name } => use_connection(&ctx, name.as_deref())?,
        Cmd::Code { card } => issue_code(card)?,
        Cmd::Dial { code, card } => dial(&ctx, code, card)?,
    }
    Ok(())
}

/// Подкоманда `code`: КЛИЕНТ управляющей двери и ничего не поднимает (закон 2 спеки). Живого нет — те же слова, что печатает `status`
/// (`door::NO_BACK_END`), и ненулевой код: в отличие от `status`, этой команде отсутствие живого значит «сделать не удалось».
///
/// Выдаёт ВСЕГДА новый и гасит прежний — то же, что кнопка «Выдать новый код»: строки уже выданного кода нет нигде, кроме памяти живого,
/// и показать её набранная команда не может (см. `door::Ask::Code`). Оговорка к «слепому» коду печатается ПЕРЕД самим кодом, видом
/// находки (`[ !! ]`), как её печатают остальные подкоманды.
fn issue_code(card: Option<String>) -> Result<()> {
    let answer = door::ask(door::Ask::Code { card }).map_err(|e| anyhow::anyhow!("{e}"))?;
    let door::Answer::Code(issued) = answer else { anyhow::bail!("{}", door::WRONG_ANSWER) };
    let issued = issued.map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(note) = issued.note {
        println!("  [ !! ] {note}");
    }
    println!("Код подключения: {}", issued.code);
    println!("Передайте его тому, кто подключается; он одноразовый и живёт сутки.");
    Ok(())
}

/// Подкоманда `dial`: тот же клиент двери, и он ЖДЁТ исхода, а не возвращается сразу (решение ведущего) — человеку в консоли нужен
/// ответ, а не обещание. Ждать приходится столько, сколько ждёт человека у чужого экрана сам разговор (`door::Ask::wait`).
///
/// Слова исхода — у зачисления (`door::dialled_words`), второго словаря здесь нет. Не зачислили — это «не сделано»: причина названа, код
/// возврата не ноль, и повторять за человека мы не берёмся (код одноразовый — повтор по нему получил бы тот же отказ).
fn dial(ctx: &Ctx, code: String, card: Option<String>) -> Result<()> {
    let name = card.clone().unwrap_or_else(|| ctx.cfg.current().name.clone());
    println!("Подключаюсь по «{name}»; ответа ждём до {} минут — на той стороне вопрос задают человеку.", DIAL_MINUTES);
    let answer = door::ask(door::Ask::Dial { code, card }).map_err(|e| anyhow::anyhow!("{e}"))?;
    let door::Answer::Dialled(out) = answer else { anyhow::bail!("{}", door::WRONG_ANSWER) };
    let text = door::dialled_words(&name, &out.map_err(|e| anyhow::anyhow!("{e}"))?).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{text}");
    Ok(())
}

/// Во что обходится человеку ожидание звонка — минутами, для одной строки предупреждения. Выводится из той же единственной меры
/// (`door::DIAL_WAIT`), а не набирается числом и не складывается заново из сроков зачисления: второе выражение той же меры — это уже
/// вторая мера, сколько бы они ни совпадали сегодня.
///
/// Округление ВВЕРХ, и это не мелочь (находка ревью круга 1): 330 секунд — это пять с половиной минут, а обещанные «до 5 минут» зовут
/// человека к Ctrl+C раньше, чем придёт ответ. Строка про ожидание врать в меньшую сторону не вправе.
const DIAL_MINUTES: u64 = door::DIAL_WAIT.as_secs().div_ceil(60);

/// Подкоманда `use`: без имени — список соединений (`Cfg::connections_overview`,
/// текст человеку). С именем — переключение: `Cfg::switch_current` сперва
/// проверяет имя В ПАМЯТИ (отказ со списком имён, если такого нет), а запись
/// на диск идёт тем же приёмом, что `gui::save::save_tray_key` — читаем файл
/// заново, правим ОДИН корневой ключ `current` (`common::toml_render::set` с
/// пустым именем секции — решение задачи 1 брифа), остальной файл не трогаем.
/// Пароли этот путь не касается вовсе: `current` не несёт секрета, и второй
/// проверки (как у `write_checked`) здесь не нужно.
fn use_connection(ctx: &Ctx, name: Option<&str>) -> Result<()> {
    let Some(name) = name else {
        println!("{}", ctx.cfg.connections_overview());
        return Ok(());
    };
    // Проверка имени — на том же `Cfg`, что прочитан командой (список
    // соединений оттуда), но запись на диск идёт по тексту файла, не по
    // этому `Cfg` целиком — тем же доводом, что у `set_current`
    // (`cfg.rs`): между чтением и записью человек может успеть править файл
    // руками.
    let mut cfg = ctx.cfg.clone();
    cfg.switch_current(name).map_err(|e| anyhow::anyhow!("{e}"))?;
    let path = ctx.kit.state_path(cfg::FILE);
    anyhow::ensure!(
        path.exists(),
        "нет файла параметров {} — создайте его из образца: 1c-connect example > {}",
        path.display(),
        path.display()
    );
    let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("{}: файл не прочитан — {e}", path.display()))?;
    let text = common::toml_render::set(&text, "", "current", &format!("{name:?}"));
    common::kit::write_text(&path, &text).map_err(|e| anyhow::anyhow!("{e:#}"))?;
    println!("Теперь текущее соединение — «{name}».");
    Ok(())
}

fn check(ctx: &Ctx) -> Result<()> {
    println!("Проверка:");
    // `check` смотрит и рассказывает (`Cfg::load_unchecked` в `run`), а не отказывает на чужом шифртексте. До ревью задачи 7 (Important 2) находка печаталась здесь отдельным `println!` МИМО набора фактов — отчёт (`last-check.txt`), который человек прикладывает к обращению, о ней молчал. Теперь это обычный факт набора (`verify::undecryptable_fact`, через `verify::run` ниже) — печатать её второй раз отдельной строкой не нужно. У `install`, который зовёт `check` тем же путём, список `undecryptable` всегда пуст: он сам на `Cfg::load` и отказал бы раньше.
    // Командная строка (`check`, и `install` сразу за установкой) портов
    // рабочего места не занимает: занятый порт для неё — сведение, а не
    // провал (спека, «порты важны там, где мы их занимаем»).
    let checks = verify::run(ctx, verify::PortUse::WeDont);
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

/// Снимает признак разрешения (`allow::write(.., false)`) любым путём, каким `connect`/`serve` выходят — в т.ч. через `?`: `Drop`, а не строка на каждый возврат, иначе ранний отказ (например, `tokio::runtime::Runtime::new()`) оставлял бы на диске «on» от процесса, которого уже нет.
struct OffOnExit<'a>(&'a Kit);

impl Drop for OffOnExit<'_> {
    fn drop(&mut self) {
        let _ = allow::write(self.0, false);
    }
}

/// Узел этой машины и проводка от него к сеансу — для долгоживущей подкоманды (`connect`, `serve`).
///
/// Связь со второй стороной держит узел (`peer::start`), а не окно, и поднимает его всякий, кто держит сеанс. До задачи 4 плана
/// «Сопряжение переезжает в бек» узел жил только в окне, и сторона без базы отказывала словами «связь с ним держит окно программы»
/// — отказ был правдой ровно потому, что здесь узла никто не поднимал.
///
/// Ручку узла возвращаем наружу, и держать её обязан вызывающий до самого конца: когда база ЗДЕСЬ, проводка уходит в `Toward::Child`
/// и замыкание с узлом внутри умирает — а узел нужен как раз в этом сочетании, чтобы принимать вызовы ТОЙ стороны.
///
/// События узла идут человеку в консоль (`ctx.sink`, тот же вид строк, что у остальных подкоманд) и на диск (`journal::say`, тот же
/// файл и тот же вид, что у окна). Заявка на зачисление (`Out::Enroll`) и состояние потока карточки (`Out::Flow`) здесь молчат:
/// заявке нужен ответ человека, а показать её и спросить этой подкомандe нечем — слово о них за командной строкой сопряжения.
fn peer_wiring(ctx: &Ctx) -> (peer::Peer, session::Wiring) {
    let serving = session::Serving::default();
    let (out_tx, mut out_rx) = futures::channel::mpsc::unbounded::<peer_out::Out>();
    let node = peer::start(&ctx.kit, out_tx, serving.clone());
    let (kit, sink) = (ctx.kit.clone(), ctx.sink.clone());
    std::thread::spawn(move || {
        futures::executor::block_on(async move {
            use futures::StreamExt;
            while let Some(out) = out_rx.next().await {
                if let peer_out::Out::Say(e) = out {
                    journal::say(&kit, &e);
                    sink(e);
                }
            }
        })
    });
    // Замыкание, а не ручка узла: слушатель собирается и без окна, и знать про поток узла ему незачем (см. `listen::Toward`).
    let to_channel: session::ToChannel = {
        let (node, card) = (node.clone(), ctx.cfg.current().id().clone());
        Arc::new(move |kind, stream| node.calls(&card, kind, stream))
    };
    (node, session::Wiring { serving, to_channel, ports: None })
}

/// Туннель плюс мост, пока не нажмут Ctrl+C или сеанс не упадёт сам.
fn connect(ctx: &Ctx) -> Result<()> {
    cfg::require_file(&ctx.kit)?;
    // Один коннектор на УЧЁТНУЮ ЗАПИСЬ в пределах машины (задача 14: было —
    // на сеанс): вторая копия не борется за порты и замок проекта с этой —
    // см. `single_instance`. Просить первую копию показать окно НЕ просим:
    // человек запросил обслуживание в терминале, и чужое всплывшее окно с
    // нулевым кодом было бы ответом не на его вопрос (`acquire`).
    //
    // Замок — это открытая управляющая дверь, и держится он до конца
    // функции. Первый снимок кладём сразу: `connect` ОБСЛУЖИВАЕТ, и
    // умолчание двери («выключен») было бы о нём неправдой. Про связь он
    // этой дверью пока не рассказывает — `cards: None` и есть «о связи не
    // сообщаю», и `status` говорит это человеку словами.
    let door = single_instance::acquire(&ctx.kit, false)?;
    door.show(door::StatusView { serving: true, cards: None });
    // Признак «обслуживает этот комплект» — рядом, своим смыслом (не
    // взаимозаменяем с замком сеанса выше): его слушает сторож моста по
    // коннектору (`orphan::guard_owner`), и ключ у него не плывёт при
    // переименовании рабочего места на ходу (круг исправления, см.
    // заголовок `orphan.rs`).
    let _serving = single_instance::hold_serving(&ctx.kit);
    // Межпроцессная часть замка проекта (`project::hold`): без этого вызова
    // файл не заводится, и второй долгоживущий процесс (другой редактор,
    // будущая подкоманда `mcp`) замка вовсе не увидит.
    project::set_lock_target(ctx.kit.state());
    // `connect` — такой же владелец сеанса, как окно (спека, «у сеанса один
    // владелец»), и признак разрешения на диске обязан сказать «on» здесь
    // же, иначе подкоманда `mcp` отказывала бы в каждом вызове при исправно
    // работающей связи.
    let _ = allow::write(&ctx.kit, true);
    let _off = OffOnExit(&ctx.kit);
    let ports = ctx.cfg.ports();
    println!("Соединение «{}»; порты {} (mcp-1c) и {} (веб-клиент). Ctrl+C — отключиться.", ctx.cfg.current().name, ports.mcp, ports.web);
    // Узел держится до конца функции, а не до конца проводки — см. докстроку `peer_wiring`.
    let (_node, wiring) = peer_wiring(ctx);
    let mut s =
        session::start_with_enabled(ctx.cfg.clone(), ctx.kit.clone(), ctx.sink.clone(), Arc::new(AtomicBool::new(true)), wiring, |_| {});
    let rt = tokio::runtime::Runtime::new()?;
    let r = rt.block_on(async {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => Ok(()),
            r = s.wait() => r,
        }
    });
    say_goodbye(&_node, &ctx.kit);
    let stopped = s.stop();
    r?;
    stopped?;
    println!("Отключено.");
    Ok(())
}

/// Прощание зачисленным сторонам — ПЕРВЫМ делом на выходе, до остановки сеанса, тем же порядком и тем же сроком, что у окна
/// (`gui::quit`).
///
/// Канал, закрытый молча, читается той стороной как обрыв, а обрыв она повторяет кругами и показывает «подключается снова» — вечно,
/// потому что звонить уже некуда. Слова превращают это в исход: круги прекращаются, и человек там читает «Та сторона закрыла
/// программу». До этой задачи командная строка узла не поднимала вовсе, и прощаться ей было нечем; теперь поднимает — значит и
/// прощаться обязана, иначе остановка стенда оставляла бы вторую сторону в кругах.
fn say_goodbye(node: &peer::Peer, kit: &Kit) {
    if !node.bye_all(one_c_connect::STOP_WAIT) {
        journal::write(kit, journal::Kind::Warn, "предупредить подключённых о выходе не удалось — у них связь просто оборвётся");
    }
}

/// Текст для журнала, если публикация не поднялась — `None`, если всё в
/// порядке. Отдельная чистая функция (не внутри `serve`) — чтобы решение
/// «что сказать при отказе» проверялось без настоящего httpd/apache и без
/// реального сеанса.
#[cfg(not(feature = "gui"))]
fn publication_failure_text(result: &Result<()>) -> Option<String> {
    result.as_ref().err().map(|e| format!("публикация базы не поднята: {e:#}"))
}

/// То же самое, что `connect`, но без вопросов и без печати плана: для
/// запуска без рабочего стола (стенд, автозапуск). Останавливается по
/// Ctrl+C, а на Linux — ещё и по SIGTERM, чтобы `docker stop` и остановка
/// стенда закрывали мосты штатно и срабатывала уборка группы процессов.
///
/// Путь сборки БЕЗ окна, и только её (задача 4 плана «Сопряжение переезжает
/// в бек»): значка там нет и быть не может — его код в сборку не входит, —
/// и спека разрешает это единственным случаем «машина без рабочего стола».
/// В сборке с окном `Cmd::Serve` идёт в `gui::launch(kit, Start::Serve)`, то
/// есть тем же путём, что двойной щелчок, со значком и без окна; второго
/// подъёма связи там нет вовсе, эта функция в неё не компилируется.
///
/// **Два решения у путей одинаковые — не перепутайте их с различиями.**
/// Обслуживание включает сам факт набранной команды: галочку «подключаться
/// при запуске» не спрашивает ни этот путь, ни `launch` в ветви
/// `Start::Serve` — она предпочтение ОКНА о том, что окно делает при старте
/// (круг правок 2). Холодный замок готовности спрашивают ОБА и оба
/// отказывают его словами, одними и теми же (`gate::refuse_to_serve`, зов
/// ниже в теле — круг правок 3). Снять здесь ту проверку как «дубль пути с
/// окном» нельзя: сюда путь с окном не компилируется вовсе.
///
/// Остаётся разница от сборки, а не от решения: значка нет (его код не
/// входит), журнал не усекается и ловушки паник нет (это делает `launch`), а
/// события идут на экран видом командной строки (`ctx.sink` — `[ ok ]`), а
/// не видом журнала.
#[cfg(not(feature = "gui"))]
fn serve(ctx: &Ctx) -> Result<()> {
    cfg::require_file(&ctx.kit)?;
    // Тот же единственный владелец на учётную запись, что у `connect` — см.
    // `single_instance`; и так же не просим первую копию показать окно:
    // `serve` поднимают планировщик и служба, которым нулевой код значил бы
    // «обслуживание идёт», а оно шло бы не здесь.
    //
    // Замок — это и есть управляющая дверь (задача 14), и берётся она здесь,
    // раньше всего прочего: набранная команда обязана застать её открытой с
    // первого мига, а не с того, как поднялось обслуживание (публикация одна
    // умеет ждать порт секундами). Отдельного `open` ниже больше нет —
    // второй подъём двери не дался бы и самому себе.
    let door = single_instance::acquire(&ctx.kit, false)?;
    // Тот же признак обслуживания, что у `connect` — см. комментарий там.
    let _serving = single_instance::hold_serving(&ctx.kit);
    // Та же проводка замка проекта, что у `connect` — см. комментарий там.
    project::set_lock_target(ctx.kit.state());
    // Холодный замок готовности — тот же вердикт и в том же месте по порядку,
    // что на пути с окном (`gui::launch`): после замков, до того как поднято
    // хоть что-нибудь. Один вердикт на всех потребителей — ровно затем
    // `gate` и не за фичей `gui` (см. его заголовок). Заперт — значит агенту
    // всё равно ничего не достанется (нет платформы, базы, mcp-1c или вышел
    // срок), и стартовать, заняв порты и замок, значило бы обещать работу,
    // которой не будет, не сказав ни слова. Слова и код возврата — общие
    // (`gate::refuse_to_serve`).
    if let Some(why) = gate::gate_now(&ctx.kit, &ctx.cfg) {
        return Err(gate::refuse_to_serve(&ctx.kit, &why));
    }
    // На стенде связь держит именно `serve` (сам он и есть «окно» этого
    // рабочего места) — тот же признак разрешения, что пишет `connect`, см.
    // комментарий там.
    let _ = allow::write(&ctx.kit, true);
    let _off = OffOnExit(&ctx.kit);
    // Первый снимок — сразу, не дожидаясь первой же перемены в сеансе: до неё дверь отвечала бы умолчанием, то есть «не обслуживает» при поднятом обслуживании.
    let card = ctx.cfg.current().name.clone();
    door.show(status_view(&card, &session::State::Off));
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
    // Узел держится до конца функции, а не до конца проводки — см. докстроку `peer_wiring`.
    let (_node, wiring) = peer_wiring(ctx);
    // Дверь учится ДЕЛАТЬ, а не только рассказывать: `code` и `dial` с этой машины идут к ЭТОМУ узлу (задача 6 плана). Решает за неё
    // бек целиком (`door::serves`) — тот же самый, что ставит себе окно, потому что клиенты у двери одни.
    door.answers(door::serves(ctx.kit.clone(), _node.clone(), one_c_connect::codes::Codes::default()));
    // Состояние сеанса здесь прежде никому не отдавали (`|_| {}`), и отдавать было некому: экрана у этой сборки нет. Теперь его спрашивают дверью, и снимок обновляется на каждую перемену — так `status` с другой машины видит связь, а не последнее, что успел напечатать терминал.
    let on_state = {
        let (door, card) = (door.clone(), card.clone());
        move |st: session::State| door.show(status_view(&card, &st))
    };
    let mut s =
        session::start_with_enabled(ctx.cfg.clone(), ctx.kit.clone(), ctx.sink.clone(), Arc::new(AtomicBool::new(true)), wiring, on_state);
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
    say_goodbye(&_node, &ctx.kit);
    let stopped = s.stop();
    r?;
    stopped?;
    Ok(())
}

/// Что рассказывает о себе дверь `serve`: обслуживание включено (иначе эта команда не стартовала бы — холодный замок отказал бы её же словами выше), а слово про связь — из `contract::contract_word` и ниоткуда больше.
///
/// **Почему `Enroll::default()` здесь честен.** Заявок и потоков отдельных карточек эта подкоманда не видит вовсе: `peer_wiring` молчит про `Out::Enroll` и `Out::Flow` нарочно — показать заявку и спросить человека ей нечем, и слово о них за командами сопряжения (задачи 6–9 плана). Единственное, что `serve` про связь знает, — состояние транспорта, и оно сюда и приходит. Карточка без состояния контракта строки не получает: `contract_word` отдаёт `None`, и дверь слова за неё не выдумывает.
#[cfg(not(feature = "gui"))]
fn status_view(card: &str, st: &session::State) -> door::StatusView {
    use one_c_connect::contract::{contract_of, contract_word, Enroll};
    let word = contract_of(Some(st), &Enroll::default()).and_then(contract_word);
    // `Some`, а не `None`: эта подкоманда о связи СООБЩАЕТ — пустой список здесь значит «связей нет», и это правда, а не незнание (разница названа типом, см. `door::StatusView::cards`).
    let cards = word.map(|w| vec![door::CardState { name: card.to_string(), word: w.to_string() }]).unwrap_or_default();
    door::StatusView { serving: true, cards: Some(cards) }
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

    /// `use` разбирается и с именем, и без — без имени список, с именем
    /// переключение (`use_connection`, логика сама — в `cfg::switch_current`
    /// и `cfg::connections_overview`, проверены там).
    #[test]
    fn use_takes_an_optional_connection_name() {
        let cli = Cli::try_parse_from(["1c-connect", "use"]).expect("use без имени разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Use { name: None })));
        let cli = Cli::try_parse_from(["1c-connect", "use", "Ромашка"]).expect("use с именем разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Use { name: Some(n) }) if n == "Ромашка"));
    }

    /// `code` и `dial` разбираются и с именем карточки, и без: имя — необязательное, без него берётся текущее соединение. Код у `dial`,
    /// наоборот, обязателен — звонить без него некуда, и отказ здесь честнее пустой строки, уехавшей в узел.
    #[test]
    fn code_and_dial_take_an_optional_card_name() {
        let cli = Cli::try_parse_from(["1c-connect", "code"]).expect("code без имени разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Code { card: None })));
        let cli = Cli::try_parse_from(["1c-connect", "code", "Ромашка"]).expect("code с именем разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Code { card: Some(n) }) if n == "Ромашка"));
        let cli = Cli::try_parse_from(["1c-connect", "dial", "кодик"]).expect("dial с одним кодом разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Dial { code, card: None }) if code == "кодик"));
        let cli = Cli::try_parse_from(["1c-connect", "dial", "кодик", "Ромашка"]).expect("dial с кодом и именем разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Dial { code, card: Some(n) }) if code == "кодик" && n == "Ромашка"));
        assert!(Cli::try_parse_from(["1c-connect", "dial"]).is_err(), "без кода звонить некуда — отказ, а не пустая строка в узел");
    }

    /// Ровно два сервера — те самые имена, что увидит `settings.json` Zed и
    /// `~/.pi/agent/mcp.json` (спека «MCP в редакторе», «Не цели»). Третьему
    /// имени взяться неоткуда: `McpServer` — закрытый `ValueEnum`.
    #[test]
    fn mcp_takes_exactly_our_two_server_names() {
        let cli = Cli::try_parse_from(["1c-connect", "mcp", "1c"]).expect("1c разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Mcp { server: McpServer::OneC })));
        let cli = Cli::try_parse_from(["1c-connect", "mcp", "vrunner"]).expect("vrunner разбирается");
        assert!(matches!(cli.cmd, Some(Cmd::Mcp { server: McpServer::Vrunner })));
        assert!(Cli::try_parse_from(["1c-connect", "mcp", "bsl-reference"]).is_err(), "чужой сервер не проходит мимо clap");
        assert!(Cli::try_parse_from(["1c-connect", "mcp"]).is_err(), "без имени сервера — отказ");
    }

    /// Два скрытых способа запуска окна — двойной щелчок и автозапуск при входе в систему — разбираются до `Cli::parse()` и
    /// отличаются только значением `hidden`. Подкоманды сюда не попадают: `serve` разбирает clap, и его `Start` собирается в `run`.
    #[cfg(feature = "gui")]
    #[test]
    fn a_double_click_and_the_autostart_differ_only_in_whether_the_window_shows() {
        assert_eq!(hidden_for_args(&["1c-connect".into()]), Some(false), "двойной щелчок — с окном");
        assert_eq!(hidden_for_args(&["1c-connect".into(), "--tray".into()]), Some(true), "автозапуск при входе — без окна");
        assert_eq!(hidden_for_args(&["1c-connect".into(), "serve".into()]), None, "подкоманду разбирает clap, а не этот разбор");
    }

    #[cfg(not(feature = "gui"))]
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

    #[cfg(not(feature = "gui"))]
    #[test]
    fn a_successful_publication_has_nothing_to_report() {
        let ok: Result<()> = Ok(());
        assert!(publication_failure_text(&ok).is_none());
    }

    /// Снимок за дверью `serve` говорит словами ОБЩЕГО словаря (`contract_word`), а не своими: ровно за этим словарь и переезжал в бек. Проверяются оба исхода — состояние, у которого слово есть, и состояние, у которого его нет.
    #[cfg(not(feature = "gui"))]
    #[test]
    fn the_door_of_serve_names_the_link_with_the_shared_dictionary() {
        let up = status_view("Ромашка", &session::State::Up { since: std::time::Instant::now() });
        assert!(up.serving, "набранный serve обслуживает — иначе он бы не стартовал");
        let cards = up.cards.expect("эта подкоманда о связи СООБЩАЕТ, а не молчит");
        assert_eq!(cards.len(), 1, "у карточки есть что сказать о связи");
        assert_eq!(cards[0].name, "Ромашка");
        assert_eq!(cards[0].word, one_c_connect::contract::contract_word(one_c_connect::contract::Contract::Up).unwrap());
        let off = status_view("Ромашка", &session::State::Off);
        assert!(off.serving, "обслуживание включено и до первого подключения");
        assert_eq!(off.cards, Some(vec![]), "связей нет — и это сказано, а не умолчано: `Some` пустой, не `None`");
    }
}
