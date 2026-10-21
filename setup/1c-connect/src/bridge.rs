//! What answers on the other side of a forwarded channel: for 777N, a fresh
//! mcp-1c process on stdio; for 779N, the local web server. Nothing listens
//! on this machine — the channel is the connection.

use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A plain relay to a local TCP port (the web server behind 779N). A port
/// that does not answer is reported, not swallowed: from the container it
/// looks exactly like a tunnel fault, and only this side can tell.
pub async fn pipe_tcp<S>(stream: S, port: u16) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| format!("веб-сервер на порту {port} не отвечает ({e}) — публикация базы не поднята"))?;
    let mut stream = stream;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
    Ok(())
}

/// `mcp-1c.exe` as the bridge starts it: the kit's binary, the developer's base.
pub fn mcp_command(cfg: &crate::cfg::Cfg, kit: &common::kit::Kit) -> tokio::process::Command {
    let exe = kit.path(if cfg!(windows) { "mcp-1c-windows-amd64.exe" } else { "mcp-1c-linux-amd64" });
    let mut c = tokio::process::Command::new(exe);
    c.args(["--base", &cfg.base_url(), "--user", &cfg.host.db_user, "--password", &cfg.host.db_password, "--quiet"]);
    // Только свой каталог выгрузки, не записанное умолчание: см.
    // `Cfg::dump_override`.
    if let Some(dump) = cfg.dump_override() {
        c.args(["--dump", dump]);
    }
    // cwd — состояние, не комплект: путь к программе выше абсолютный, от
    // текущего каталога ничего не ищется, а комплект в образе только для
    // чтения — если mcp-1c однажды решит что-то положить рядом с собой, он
    // положит это туда, где писать можно (на Windows это тот же комплект).
    c.current_dir(kit.state());
    c
}

/// Процесс `vrunner-mcp` из инструментов на машине: cwd — проект, библиотеки
/// oscript находит сам (`lib.system = ../lib`), git — из `tools` через PATH,
/// подключение к базе — переменными (не в командной строке).
pub fn vrunner_command(cfg: &crate::cfg::Cfg, tools: &std::path::Path) -> tokio::process::Command {
    use crate::ops_host::{git_exe, ib_switch, oscript_exe, vrunner_lib};
    let lib = vrunner_lib(tools);
    let mut c = tokio::process::Command::new(oscript_exe(tools));
    c.arg(lib.join("vanessa-runner").join("src").join("mcp.os"));
    c.current_dir(&cfg.host.project);
    // oscript first: vrunner-mcp runs long commands as background tasks by
    // re-executing `oscript` from PATH; then the kit's git.
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut path = oscript_exe(tools).parent().map(|p| p.as_os_str().to_owned()).unwrap_or_default();
    path.push(sep);
    path.push(git_exe(tools).parent().map(|p| p.as_os_str().to_owned()).unwrap_or_default());
    path.push(sep);
    path.push(std::env::var_os("PATH").unwrap_or_default());
    c.env("PATH", path);
    c.env("VRUNNER_IBCONNECTION", ib_switch(&cfg.host.base_path));
    c.env("VRUNNER_DBUSER", &cfg.host.db_user);
    c.env("VRUNNER_DBPWD", &cfg.host.db_password);
    c.env("NO_PROXY", cfg.host.server.rsplit_once(':').map(|(h, _)| h).unwrap_or(&cfg.host.server));
    c.env("GIT_TERMINAL_PROMPT", "0");
    // Экран Конфигуратору даёт коннектор, а не вызывающий: через эту команду
    // идут `cf_decompile`/`cf_load`, а vanessa-runner поднимает ими
    // Конфигуратор — оконную программу. Без экрана `1c-connect export` из
    // командной строки падал с «Unable to initialize GTK+ or connect to the
    // windowing system», код 255 (замер 2026-10-20 в контейнере). На Windows
    // `with_display` возвращает команду как была.
    crate::ops_host::with_display(c.into_std()).into()
}

use crate::jsonrpc::{self, Frame, Framer, Route};
use crate::session::Live;
use common::{Event, Sink};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Общее начало обоих отказов «MCP выключены» — с советом включить и с
/// причиной замка. Тестам гейта важен сам отказ, а не его хвост, и смотрят они
/// сюда: иначе каждый из них зависел бы от того, заперт ли сейчас замок.
pub const DISABLED_PREFIX: &str = "MCP выключены в 1c-connect";

/// Текст отказа на вызов инструмента, пока переключатель «MCP» в окне выключен.
pub const DISABLED_TEXT: &str = "MCP выключены в 1c-connect: включите переключатель в окне";

/// Почему обслуживание не идёт, пока оно не идёт: причина холодного замка
/// (`gui::gate::cold_gate`) или отказ, который его погасил (занят порт, не
/// поднялась публикация). Порты в локальном режиме подняты всегда, и вызов
/// инструмента получает отказ — но MCP не выключены человеком, и в тексте
/// отказа обязана ехать причина: иначе ни агент, ни человек не узнают, что
/// править (спека, «Пока замок заперт, агент получает отказ с причиной»).
/// Отказ публикации сюда доезжает по той же надобности: человек ничего не
/// выключал, и «включите переключатель» было бы советом не по делу
/// (находка N2 повторного ревью).
///
/// ponytail: глобал на процесс — окно и мост живут в одном процессе, а
/// `Rules` собирают четверо (окно, `listen`, тесты); понадобится второй
/// независимый мост — поле `Arc` рядом с `enabled`.
static OFF_REASON: Mutex<Option<String>> = Mutex::new(None);

/// Окно пересчитало замок (`gui::refresh_gate`). Отравленный замок не
/// страшен: читаем и пишем короткую строку, рвать из-за неё отказы агенту
/// незачем.
pub fn set_off_reason(reason: Option<String>) {
    *OFF_REASON.lock().unwrap_or_else(|e| e.into_inner()) = reason;
}

pub fn off_reason() -> Option<String> {
    OFF_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Текст отказа: без замка — прежний («включите переключатель»), с замком —
/// его причина, потому что включать нечего и совет был бы ложью.
pub fn disabled_text(reason: Option<&str>) -> String {
    match reason {
        Some(why) => format!("{DISABLED_PREFIX}: {why}"),
        None => DISABLED_TEXT.to_string(),
    }
}

/// Правила моста на одно соединение. Строка подключения, её пользователь и
/// каталог проекта здесь не лежат готовыми — они читаются из `live` на
/// каждый вызов (см. `handle_request`), а не однократно при подключении:
/// Zed держит это соединение весь сеанс и сам не переподключается, так что
/// однократного чтения было бы недостаточно, чтобы «Сохранить» подействовало.
pub struct Rules {
    pub live: Live,
    /// Как пересобрать команду ребёнка на свежих параметрах, когда `live`
    /// сменился под работающим соединением. `None` — рестарт недоступен
    /// (один ребёнок на всю жизнь соединения, как было раньше): так у
    /// вызывающих, которым смена параметров на ходу не нужна (`listen.rs`,
    /// тесты), — ничего не меняется.
    pub rebuild: Option<Arc<dyn Fn(&crate::cfg::Cfg) -> tokio::process::Command + Send + Sync>>,
    /// Каталог инструментов на машине — только для своих инструментов
    /// (`project::Project::from_cfg`).
    pub tools_dir: std::path::PathBuf,
    pub configurator_running: Arc<dyn Fn() -> usize + Send + Sync>,
    /// Переключатель «MCP включены» окна — общий на оба порта одного сеанса.
    /// Выключен — каждый вызов инструмента получает отказ, не доходя до
    /// ребёнка; `initialize`, `tools/list`, уведомления и отмены идут как
    /// обычно, иначе Zed остался бы без ответа и счёл сервер упавшим.
    pub enabled: Arc<AtomicBool>,
    /// Подмешивать ли свои инструменты (`project_status`/`pull`/`push`) в
    /// список и исполнять их здесь же: `true` у vrunner-mcp (778N), `false`
    /// у mcp-1c (777N) — там нет ни белого списка инструментов, ни наших.
    pub inject: bool,
}

impl Rules {
    /// Мост 778N: `vrunner-mcp`. Белый список из `[host] tools`, свои
    /// инструменты подмешиваются в `tools/list` и исполняются мостом.
    pub fn for_vrunner(
        live: Live,
        tools_dir: std::path::PathBuf,
        rebuild: Arc<dyn Fn(&crate::cfg::Cfg) -> tokio::process::Command + Send + Sync>,
        configurator_running: Arc<dyn Fn() -> usize + Send + Sync>,
        enabled: Arc<AtomicBool>,
    ) -> Rules {
        Rules { live, rebuild: Some(rebuild), tools_dir, configurator_running, enabled, inject: true }
    }

    /// Мост 777N: `mcp-1c`. Голый проброс — ни белого списка инструментов,
    /// ни наших, ни аргументов подключения. Без рестарта и без «живых»
    /// параметров по умолчанию — вызывающий, которому они нужны (`Serve::dispatch`),
    /// подставляет свои `live`/`rebuild` в уже созданное значение, поля для
    /// этого публичные.
    pub fn for_mcp(enabled: Arc<AtomicBool>) -> Rules {
        Rules { live: Live::new(crate::cfg::Cfg::default()), rebuild: None, tools_dir: std::path::PathBuf::new(), configurator_running: Arc::new(|| 0), enabled, inject: false }
    }
}

/// Подключение к базе и её пользователь как аргументы инструмента vrunner:
/// `vrunner-mcp` читает их отсюда, а не из переменных окружения. Пустой
/// пароль не передаётся — иначе платформа получит `/P` с пустым значением.
/// Зовётся на каждый подходящий вызов (см. `handle_request`), а не один раз
/// на соединение — иначе смена базы не дошла бы до уже открытого у Zed порта.
pub fn connection_args(cfg: &crate::cfg::Cfg) -> Value {
    let mut a = serde_json::json!({"ibconnection": crate::ops_host::ib_switch(&cfg.host.base_path), "db-user": cfg.host.db_user});
    if !cfg.host.db_password.is_empty() {
        a["db-pwd"] = Value::String(cfg.host.db_password.clone());
    }
    a
}

/// Сколько процессов платформы запущено на машине: Конфигураторы и толстые
/// клиенты (`1cv8.exe`) и тонкие (`1cv8c.exe`) — файловую базу держит любой.
pub fn configurator_count() -> usize {
    if !cfg!(windows) {
        return 0;
    }
    let mut c = std::process::Command::new("tasklist");
    c.args(["/NH", "/FO", "CSV"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.output().map(|o| count_platform(&String::from_utf8_lossy(&o.stdout))).unwrap_or(0)
}

/// Строки `tasklist /FO CSV`, начинающиеся с процесса платформы.
fn count_platform(csv: &str) -> usize {
    csv.lines().filter(|l| l.starts_with("\"1cv8.exe\"") || l.starts_with("\"1cv8c.exe\"")).count()
}

/// id → (id, список допущенных инструментов, действовавший на момент
/// запроса) — ответ на `tools/list` обязан фильтроваться тем же списком,
/// каким был отфильтрован сам запрос, даже если параметры успели смениться
/// до того, как ребёнок ответил.
type Pending = Arc<Mutex<HashMap<String, (Value, Vec<String>)>>>;
type Running = Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>;

fn key(id: &Value) -> String {
    id.to_string()
}

/// Общий мост на оба порта: `stream` ↔ процесс (`vrunner-mcp` на 778N,
/// `mcp-1c` на 777N), построчно. У 778N `tools/list` переписывается, чужие
/// и наши инструменты перехватываются; у 777N (`Rules::for_mcp`) — голый
/// проброс. Переключатель «MCP» выключен — любой `tools/call` получает
/// отказ, не доходя до ребёнка. В канал пишет один писатель — за `out_tx`.
///
/// Zed держит одно соединение на весь сеанс и сам никогда не переподключается
/// — поэтому строка подключения, её пользователь и каталог проекта читаются
/// не один раз при приёме соединения, а на каждый запрос (`rules.live`, см.
/// `handle_request`). Для аргументов вызова и своих инструментов этого
/// достаточно; но окружение и рабочий каталог самого ребёнка задаются при
/// его запуске и не переживают правку на лету. Если у `rules` есть
/// `rebuild` и параметры сменились под работающим соединением, функция сама
/// завершает текущего ребёнка и поднимает нового по `rebuild` — но только на
/// следующем запросе, застающем её свободной (`in_flight == 0`); вызов, уже
/// ушедший ребёнку, спокойно доигрывает со старыми параметрами. Без
/// `rebuild` (`Rules::for_mcp` без доустройки) — один ребёнок на всю жизнь
/// соединения, как и было.
pub async fn pipe_jsonrpc<S>(stream: S, cmd: tokio::process::Command, rules: Rules, log: Sink, on_exit: impl FnOnce(i32, String))
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sr, mut sw) = tokio::io::split(stream);
    // Bounded: a multi-megabyte report waits for the tunnel instead of piling
    // up in memory while it stalls.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    // Ответы ребёнка на tools/list, которых ещё ждут; ключ — id как JSON.
    // Общие на всё соединение, не на одного ребёнка: к моменту рестарта они
    // в любом случае пусты (см. `in_flight` ниже).
    let pending: Pending = Arc::default();
    let running: Running = Arc::default();
    let rules = Arc::new(rules);
    // Один Warn в журнал на «строка от агента не опознана, пока MCP
    // выключены» за всё соединение: иначе баннер или keepalive, повторяющийся
    // на каждой строке, залил бы журнал человеку одинаковыми записями.
    let mcp_off_warned = Arc::new(AtomicBool::new(false));
    // Сколько запросов уже ушло текущему ребёнку и не получило ответа — пока
    // не ноль, смена параметров его не трогает: середина чужого вызова
    // никому не достаётся, ни старому, ни новому.
    let in_flight = Arc::new(AtomicU64::new(0));
    // Рукопожатие этого соединения (`initialize` и, если пришёл, следующий
    // за ним `notifications/initialized`) — переигрывается свежему ребёнку
    // при рестарте (см. `replay_handshake`), иначе тот, будучи обычным
    // MCP-сервером, вправе отказать первому же `tools/call`.
    let handshake: HandshakeCell = Arc::default();
    // id переигранного `initialize`, чей ответ агенту не нужен (свой он уже
    // получил) — `from_child` ниже сам его не пропустит.
    let swallow: Swallow = Arc::default();
    let who = if rules.inject { "vrunner-mcp" } else { "mcp-1c" };

    let writer = tokio::spawn(async move {
        while let Some(b) = out_rx.recv().await {
            if sw.write_all(&b).await.is_err() {
                break;
            }
        }
        let _ = sw.shutdown().await;
    });

    /// Чем закончился разговор с одним конкретным ребёнком.
    enum EpochEnd {
        /// Канал к агенту закрылся.
        AgentEof,
        /// Не удалось записать ребёнку (его stdin закрылся) — тот же итог.
        WriteFailed,
        /// Ребёнок вышел сам.
        ChildExited(std::io::Result<std::process::ExitStatus>),
        /// Параметры сменились, и ни один вызов не висит без ответа: поднять
        /// нового ребёнка на свежих параметрах, не трогая само соединение.
        Restart,
    }

    let mut f = Framer::new();
    let mut buf = vec![0u8; 64 * 1024];
    // Строки, уже вынутые из потока агента, но ещё не доставленные ребёнку:
    // при рестарте остаются здесь и уходят уже новому, в своём порядке.
    let mut pending_frames: std::collections::VecDeque<Frame> = std::collections::VecDeque::new();
    let (mut gen, mut cfg) = rules.live.get();
    // Первый ребёнок — тем же `cmd`, что передал вызывающий; следующие (если
    // дело дойдёт до рестарта) — свежей сборкой по `rules.rebuild`.
    let mut next_cmd = Some(cmd);

    let (code, tail) = 'connection: loop {
        let (mut is_restart, mut cmd) = match next_cmd.take() {
            Some(c) => (false, c),
            None => match &rules.rebuild {
                Some(rebuild) => (true, rebuild(&cfg)),
                // Рестарт только и всегда гейтится наличием `rebuild` (см.
                // ниже) — сюда попасть без него нельзя; но падать насмерть
                // вместо непонятного паникующего unwrap лучше честной ошибкой.
                None => break 'connection (-1, "внутренняя ошибка: рестарт без пересборщика команды".to_string()),
            },
        };
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        crate::job::group(&mut cmd);
        let mut job = crate::job::Job::new();
        // Запуск — с повтором: команда не находится (бинарь, каталог) — не
        // повод рвать соединение или отвечать тишиной. Zed держит это
        // соединение весь сеанс, и разрыв означает для человека ручной
        // перезапуск обоих портов в Zed ровно тогда, когда он разбирается с
        // настройками. Каждый следующий запрос — новая попытка на свежих
        // параметрах; до первой удачи каждый вызов получает честный отказ
        // (`degraded_reply`).
        let mut last_logged_error: Option<String> = None;
        let mut child = loop {
            match cmd.spawn() {
                Ok(c) => break c,
                Err(e) => {
                    let Some(rebuild) = &rules.rebuild else {
                        break 'connection (-1, format!("не удалось запустить: {e}"));
                    };
                    let msg = e.to_string();
                    if last_logged_error.as_deref() != Some(msg.as_str()) {
                        log(Event::Warn(format!("{who}: не удалось запустить: {msg} — отвечаю отказом вызовам, пока не исправится")));
                        last_logged_error = Some(msg);
                    }
                    // Ждём следующий запрос (или конец соединения): отвечаем
                    // ему отказом и пробуем снова, а не спавним вслепую по кругу.
                    loop {
                        if !pending_frames.is_empty() {
                            let fr = pending_frames.pop_front().unwrap();
                            if let Frame::Line(l) = &fr {
                                capture_handshake(l, &handshake);
                                if let Some(reply) = degraded_reply(l, rules.inject, who, &e.to_string()) {
                                    let _ = out_tx.send(reply).await;
                                }
                            }
                            break;
                        }
                        tokio::select! {
                            n = sr.read(&mut buf) => match n {
                                Ok(0) | Err(_) => break 'connection (-1, "агент отключился, пока ребёнок не запускался".to_string()),
                                Ok(n) => pending_frames.extend(f.push(&buf[..n])),
                            }
                        }
                    }
                    // Следующий ребёнок никогда не видел рукопожатия, даже
                    // если первая попытка была самой первой на этом
                    // соединении — его переиграют ниже (см. `is_restart`).
                    is_restart = true;
                    cmd = rebuild(&cfg);
                    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
                    #[cfg(windows)]
                    cmd.creation_flags(0x0800_0000);
                    crate::job::group(&mut cmd);
                    job = crate::job::Job::new();
                }
            }
        };
        if let Some(j) = &job {
            j.assign(&child);
        }
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();

        // Ребёнок → агент: своя пара задач на каждого ребёнка (стдин/стдаут
        // предыдущего уже не существуют), но пишут в тот же `out_tx`.
        let from_child = {
            let (out, pending, log, in_flight, inject, swallow) = (out_tx.clone(), pending.clone(), log.clone(), in_flight.clone(), rules.inject, swallow.clone());
            tokio::spawn(async move {
                let mut f = Framer::new();
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = match stdout.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    for fr in f.push(&buf[..n]) {
                        warn_on_session_limit(&fr, &log);
                        if let Frame::Line(l) = &fr {
                            if let Some(id) = jsonrpc::is_response_with_id(l) {
                                // Ответ на переигранный initialize: клиент уже
                                // получил свой, второй с тем же id сбил бы
                                // Zed с толку, а не относящийся к его вызову.
                                if swallow.lock().unwrap().remove(&key(&id)) {
                                    continue;
                                }
                                dec_in_flight(&in_flight);
                            }
                        }
                        if out.send(rewrite_reply(fr, inject, &pending)).await.is_err() {
                            return;
                        }
                    }
                }
                if let Some(fr) = f.finish() {
                    warn_on_session_limit(&fr, &log);
                    if let Frame::Line(l) = &fr {
                        if let Some(id) = jsonrpc::is_response_with_id(l) {
                            if swallow.lock().unwrap().remove(&key(&id)) {
                                return;
                            }
                            dec_in_flight(&in_flight);
                        }
                    }
                    let _ = out.send(rewrite_reply(fr, inject, &pending)).await;
                }
            })
        };
        let err_tail = tokio::spawn(async move {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s).await;
            let lines: Vec<&str> = s.lines().collect();
            lines[lines.len().saturating_sub(5)..].join("\n")
        });

        // Рестарт: переиграть рукопожатие свежему ребёнку, прежде чем до
        // него дойдёт вызов, который рестарт вызвал — иначе обычный
        // MCP-сервер вправе отказать первому же tools/call как
        // необинициализированному. Неудача переигровки — не повод делать
        // вид, что ребёнок готов: вызов, который ждёт в `pending_frames`,
        // получает честный отказ, а не тишину или ответ от сырого процесса.
        if is_restart {
            let snapshot = handshake.lock().unwrap().clone();
            if (snapshot.initialize.is_some() || snapshot.initialized.is_some())
                && replay_handshake(&mut stdin, &snapshot, &swallow).await.is_err()
            {
                if let Some(Frame::Line(l)) = pending_frames.pop_front() {
                    if let Some(id) = request_id(&l) {
                        let _ = out_tx.send(jsonrpc::text_reply(&id, RESTART_REFUSAL_TEXT, true)).await;
                    }
                }
                log(Event::Warn(format!("{who}: рукопожатие не переигралось свежему ребёнку после смены базы — повторите вызов")));
            }
        }

        // Агент → ребёнок: читается прямо здесь (не в отдельной задаче),
        // чтобы `sr` можно было забрать назад и отдать следующему ребёнку
        // при рестарте — то же самое соединение, другой процесс позади него.
        let end = loop {
            if !pending_frames.is_empty() {
                // Перед каждой строкой, а не только между партиями из
                // одного `read`: иначе первый же запрос после смены
                // параметров ушёл бы ещё старому ребёнку, и рестарт
                // откладывался бы до запроса, который его застанет праздным.
                // Без `rebuild` рестарта вообще не бывает — один ребёнок на
                // всю жизнь соединения, как у `Rules::for_mcp` без доустройки.
                if rules.rebuild.is_some() && in_flight.load(Ordering::Relaxed) == 0 {
                    let (now_gen, now_cfg) = rules.live.get();
                    if now_gen != gen {
                        gen = now_gen;
                        cfg = now_cfg;
                        break EpochEnd::Restart;
                    }
                }
                let fr = pending_frames.pop_front().unwrap();
                if handle_request(fr, &rules, &out_tx, &pending, &running, &mut stdin, &log, &mcp_off_warned, &in_flight, &handshake).await.is_err() {
                    break EpochEnd::WriteFailed;
                }
                continue;
            }
            tokio::select! {
                n = sr.read(&mut buf) => {
                    match n {
                        Ok(0) | Err(_) => break EpochEnd::AgentEof,
                        Ok(n) => pending_frames.extend(f.push(&buf[..n])),
                    }
                }
                status = child.wait() => break EpochEnd::ChildExited(status),
            }
        };

        // Канал закрылся (или рестарт), а ребёнок не вышел сам: несколько
        // секунд на выход, затем принудительно — иначе `oscript.exe` с
        // Конфигуратором под ним живут до перезапуска, а следующий вызов
        // видит «закройте Конфигуратор».
        let grace_kill = |mut stdin: tokio::process::ChildStdin, mut child: tokio::process::Child| async move {
            let _ = stdin.shutdown().await;
            match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                Ok(s) => s,
                Err(_) => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            }
        };
        match end {
            EpochEnd::ChildExited(status) => {
                let _ = from_child.await;
                let tail = err_tail.await.unwrap_or_default();
                drop(job);
                break (status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1), tail);
            }
            EpochEnd::AgentEof | EpochEnd::WriteFailed => {
                let status = grace_kill(stdin, child).await;
                let _ = from_child.await;
                let tail = err_tail.await.unwrap_or_default();
                drop(job);
                break (status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1), tail);
            }
            EpochEnd::Restart => {
                let _ = grace_kill(stdin, child).await;
                let _ = from_child.await;
                let _ = err_tail.await;
                drop(job);
                log(Event::Say(format!("{who}: параметры базы сменились, следующий вызов идёт в {}", cfg.host.base_path)));
                continue;
            }
        }
    };

    // Ребёнок ушёл, не ответив на tools/list: наши инструменты агент получает
    // всё равно — но только у моста, который их вообще подмешивает.
    let unanswered: Vec<Value> = pending.lock().unwrap().drain().map(|(_, (id, _))| id).collect();
    if rules.inject {
        for id in unanswered {
            let _ = out_tx.send(jsonrpc::list_reply(&id, jsonrpc::our_tools())).await;
        }
    }
    // Свои задачи дорабатывают: их ответы ещё могут дойти до агента.
    let ours: Vec<_> = running.lock().unwrap().drain().map(|(_, h)| h).collect();
    for h in ours {
        let _ = h.await;
    }
    drop(out_tx);
    let _ = writer.await;
    on_exit(code, common::docker::redact_text(&tail));
}

/// Одна строка от агента: кому она. `Err` — ребёнок закрыл stdin.
async fn handle_request(
    fr: Frame,
    rules: &Arc<Rules>,
    out: &mpsc::Sender<Vec<u8>>,
    pending: &Pending,
    running: &Running,
    stdin: &mut tokio::process::ChildStdin,
    log: &Sink,
    mcp_off_warned: &AtomicBool,
    in_flight: &AtomicU64,
    handshake: &HandshakeCell,
) -> Result<(), ()> {
    // A line over the Framer's cap is never passed on, on either port: a
    // line the bridge did not buffer whole is not one it can gate or route.
    // The agent hears why (a parse error with id null) instead of silence.
    let line = match fr {
        Frame::Raw(b) => {
            if b.ends_with(b"\n") {
                let _ = out.send(jsonrpc::error_reply(&Value::Null, -32700, "мост 1c-connect: строка длиннее 8 МиБ отброшена")).await;
            }
            return Ok(());
        }
        Frame::Line(l) => l,
    };
    // Ни на одном порту, ни при каком переключателе: ответить такому вызову
    // нечем (нет id), а пропустить его — значит исполнить побочный эффект
    // (например, cf_load) без единого следа в ответах агенту. Решение
    // заказчика: отбой всегда, вместо того чтобы один раз обойти проверку.
    if jsonrpc::is_tools_call_without_id(&line) {
        log(Event::Warn("вызов инструмента без идентификатора отброшен".into()));
        return Ok(());
    }
    // Несёт ли исходная строка верхнеключевой `id`: если дойдёт до ребёнка,
    // тот обязан ответить на неё ровно один раз — на этом считает
    // `in_flight` (см. заголовок `pipe_jsonrpc`), чтобы смена параметров не
    // ударила по вызову, который уже в работе.
    let reply_expected = expects_reply(&line);
    // Рукопожатие этого соединения — на всякий случай, независимо от
    // переключателя и порта: `initialize`/`notifications/initialized` и так
    // идут как обычно (см. ветки ниже), запоминается только их факт.
    capture_handshake(&line, handshake);
    // Истёкший срок заглушки лицензии запирает вызовы тем же путём, что
    // снятый переключатель (спека, п. 9): не своей веткой на каждый отказ, а
    // тем же `off` — иначе пришлось бы продублировать все ветки ниже, и
    // какая-нибудь из них однажды разошлась бы с остальными. Меняется только
    // текст отказа: человек должен прочитать причину, а не «включите
    // переключатель», которого не хватит.
    let expired = matches!(crate::licence::state_now(), crate::licence::State::Expired);
    let off = expired || !rules.enabled.load(Ordering::Relaxed);
    // Текст считаем только когда он нужен: на включённом обслуживании каждый
    // кадр брал мьютекс замка и выделял строку, которую никто не читает.
    let refusal_text = || if expired { crate::licence::REFUSAL.to_string() } else { disabled_text(off_reason().as_deref()) };
    if !rules.inject {
        // 777N (mcp-1c) не ведёт разбор JSON-RPC вовсе — голый проброс, как
        // раньше у pipe_process. Включён — байт в байт, что угодно; выключен
        // — закрыто по умолчанию: вперёд уходит только то, что уверенно
        // опознано безопасным (см. `jsonrpc::mcp_gate_while_off`), иначе
        // непонятая строка стала бы лазейкой в обход переключателя.
        if !off {
            let mut raw = line;
            raw.push(b'\n');
            return forward_counted(stdin, &raw, in_flight, reply_expected).await;
        }
        return match jsonrpc::mcp_gate_while_off(&line) {
            jsonrpc::McpOffGate::Forward => {
                let mut raw = line;
                raw.push(b'\n');
                forward_counted(stdin, &raw, in_flight, reply_expected).await
            }
            jsonrpc::McpOffGate::Refuse(id) => {
                let _ = out.send(jsonrpc::text_reply(&id, &refusal_text(), true)).await;
                Ok(())
            }
            jsonrpc::McpOffGate::ParseError(id) => {
                let _ = out.send(jsonrpc::error_reply(&id, -32700, "мост 1c-connect: строка не распознана, пока MCP выключены")).await;
                Ok(())
            }
            jsonrpc::McpOffGate::Drop => {
                // Пустая строка (голый \n от баннера или keepalive) — вообще
                // не повод для журнала; на не пустых — одна запись за всё
                // соединение, иначе шквал одинаковых строк залил бы журнал.
                if !line.is_empty() && !mcp_off_warned.swap(true, Ordering::Relaxed) {
                    log(Event::Warn(
                        "mcp-1c: нераспознанная строка от агента отброшена, пока MCP выключены (дальнейшие такие строки в журнал не пишутся)".into(),
                    ));
                }
                Ok(())
            }
        };
    }
    // 778N (vrunner-mcp): полный разбор — белый список, наши инструменты.
    // Свежий снимок параметров на этот самый вызов — не раньше (иначе
    // «Сохранить» между вызовами осталось бы незаметным) и не позже (иначе
    // один вызов мог бы собрать строку подключения из одной конфигурации, а
    // каталог проекта — из другой). Переключатель — тем же механизмом, что
    // обычный белый список: пустой список на время разбора строки проводит
    // любой tools/call через Route::Disabled/Route::OurCall, и обе ветки
    // ниже тут же отвечают отказом по переключателю вместо обычного текста.
    let (_, cfg) = rules.live.get();
    let empty: Vec<String> = Vec::new();
    let tools = cfg.host.tools.clone();
    let allowed: &[String] = if off { &empty } else { &tools };
    let defaults = connection_args(&cfg);
    match jsonrpc::route_request_with(&line, allowed, &defaults) {
        Route::Drop => Ok(()),
        Route::Reject { id, code, why } => {
            let _ = out.send(jsonrpc::error_reply(&id, code, &format!("мост 1c-connect: {why}"))).await;
            Ok(())
        }
        Route::ListRequest { id, line } => {
            pending.lock().unwrap().insert(key(&id), (id, tools.clone()));
            forward_counted(stdin, &line, in_flight, reply_expected).await
        }
        Route::Disabled { id, tool } => {
            let text = if off { refusal_text() } else { format!("инструмент {tool} выключен в 1c-connect.toml ([host] tools)") };
            let _ = out.send(jsonrpc::text_reply(&id, &text, true)).await;
            Ok(())
        }
        Route::PlatformCall { id, tool, line } => {
            let n = (rules.configurator_running)();
            if n > 0 {
                let _ = out
                    .send(jsonrpc::text_reply(
                        &id,
                        &format!("{tool}: на машине запущен 1cv8.exe ({n}) — закройте Конфигуратор и клиентов 1С, затем повторите"),
                        true,
                    ))
                    .await;
                Ok(())
            } else {
                forward_counted(stdin, &line, in_flight, reply_expected).await
            }
        }
        Route::OurCall { id, tool, args } => {
            if off {
                let _ = out.send(jsonrpc::text_reply(&id, &refusal_text(), true)).await;
                return Ok(());
            }
            // Свежий `Project` из того же снимка `cfg`, что и выше — не из
            // ячейки, замороженной при подключении: каталог и ветка идут за
            // «Сохранить», как и строка подключения.
            let project = Arc::new(crate::project::Project::from_cfg(&cfg, &rules.tools_dir));
            let out = out.clone();
            let k = key(&id);
            let h = tokio::spawn(async move {
                let (text, is_error) = tokio::task::spawn_blocking(move || crate::project::call(&project, &tool, &args))
                    .await
                    .unwrap_or_else(|e| (format!("сорвалось: {e}"), true));
                let _ = out.send(jsonrpc::text_reply(&id, &text, is_error)).await;
            });
            // Kept for the life of the connection, finished or not: a cancel
            // for an id the bridge answered must never reach the child.
            running.lock().unwrap().insert(k, h);
            Ok(())
        }
        // ponytail: abort drops the waiter; the blocking git call itself runs
        // to its end and frees the project mutex then. Enough for a pilot.
        Route::Cancel { id, line } => {
            let ours = {
                let map = running.lock().unwrap();
                map.get(&key(&id)).map(|h| h.abort()).is_some()
            };
            if ours {
                Ok(())
            } else {
                forward_counted(stdin, &line, in_flight, reply_expected).await
            }
        }
        Route::Forward(line) => forward_counted(stdin, &line, in_flight, reply_expected).await,
    }
}

/// Канонический текст уже с `\n` на конце.
async fn forward(stdin: &mut tokio::process::ChildStdin, line: &[u8]) -> Result<(), ()> {
    stdin.write_all(line).await.map_err(|_| ())
}

/// Как `forward`, но считает «отправлено ребёнку — ждёт ответа» (см.
/// `in_flight` в `pipe_jsonrpc`): пока счётчик не ноль, менять ребёнка под
/// разговором нельзя, только когда он сам опустеет на следующем запросе.
async fn forward_counted(stdin: &mut tokio::process::ChildStdin, line: &[u8], in_flight: &AtomicU64, reply_expected: bool) -> Result<(), ()> {
    if reply_expected {
        in_flight.fetch_add(1, Ordering::Relaxed);
    }
    forward(stdin, line).await
}

/// Несёт ли строка от агента верхнеключевой `id` — грубая проверка (не
/// через `key_ci`, как `route_request_with`): для подсчёта висящих вызовов
/// точность разбора не нужна, а дубль ключа здесь не повод отказывать.
fn expects_reply(line: &[u8]) -> bool {
    serde_json::from_slice::<Value>(line).ok().and_then(|v| v.as_object().map(|o| o.contains_key("id"))).unwrap_or(false)
}

/// Снять со счётчика висящих вызовов без риска уйти в минус: ответ, пришедший
/// без пары (ребёнок и сам может прислать лишнее), не должен навечно
/// заблокировать рестарт.
fn dec_in_flight(in_flight: &AtomicU64) {
    let _ = in_flight.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| Some(v.saturating_sub(1)));
}

/// Метод строки от агента, если это JSON-объект с понятным `method`: грубое
/// распознавание `initialize`/`notifications/initialized` для рукопожатия
/// (см. `Handshake`), без претензии на строгость `route_request_with`.
fn method_of(line: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(line).ok()?.get("method")?.as_str().map(str::to_string)
}

/// `id` строки, если он есть — чтобы ответить прямым отказом вызову, которому
/// не удалось передать рукопожатие (см. `replay_handshake`).
fn request_id(line: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Value>(line).ok()?.get("id").cloned()
}

/// Запомнить рукопожатие, если эта строка — `initialize` или
/// `notifications/initialized`; остальное не трогает. Общая часть для
/// обычной обработки (`handle_request`) и режима без ребёнка (см.
/// `degraded_reply`) — там, где это работает заранее, прежде чем ребёнок
/// вообще поднимется.
fn capture_handshake(line: &[u8], handshake: &HandshakeCell) {
    let Some(m) = method_of(line) else { return };
    if m.eq_ignore_ascii_case("initialize") {
        let mut raw = line.to_vec();
        raw.push(b'\n');
        handshake.lock().unwrap().initialize = Some(raw);
    } else if m.eq_ignore_ascii_case("notifications/initialized") {
        let mut raw = line.to_vec();
        raw.push(b'\n');
        handshake.lock().unwrap().initialized = Some(raw);
    }
}

/// Пока ребёнка нет (запуск не удался) — читаемый ответ вместо тишины или
/// разрыва соединения: иначе Zed решит, что сервер упал, и человеку придётся
/// руками поднимать оба порта заново. `initialize`/`tools/list` отвечают
/// так, будто сервер жив (свои инструменты — свои, их и спрашивать не у
/// кого, в списке только они, у 777N — пусто), `tools/call` и всё прочее
/// именованное с `id` — явным отказом с причиной; уведомление (нет `id`) —
/// отвечать нечем, и это не повод для записи в журнал.
fn degraded_reply(line: &[u8], inject: bool, who: &str, why: &str) -> Option<Vec<u8>> {
    let v: Value = serde_json::from_slice(line).ok()?;
    let method = v.get("method")?.as_str()?.to_string();
    let id = v.get("id").cloned();
    if method.eq_ignore_ascii_case("initialize") {
        let id = id?;
        let reply = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "1c-connect", "version": "0"}},
        });
        let mut out = serde_json::to_vec(&reply).unwrap();
        out.push(b'\n');
        Some(out)
    } else if method.eq_ignore_ascii_case("tools/list") {
        let id = id?;
        Some(jsonrpc::list_reply(&id, if inject { jsonrpc::our_tools() } else { Vec::new() }))
    } else {
        let id = id?;
        Some(jsonrpc::text_reply(&id, &format!("мост 1c-connect: {who} не запускается ({why}) — исправьте и повторите"), true))
    }
}

/// Рукопожатие одного соединения: `initialize` и, если он пришёл, идущий
/// следом `notifications/initialized` — ровно то, что агент послал бы
/// заново сам, если бы Zed переподключался. Нужно пережить рестарт ребёнка
/// (см. `pipe_jsonrpc`): свежий процесс — обычный MCP-сервер и вправе
/// отказать первому же `tools/call`, если до него не было `initialize`.
#[derive(Default, Clone)]
struct Handshake {
    initialize: Option<Vec<u8>>,
    initialized: Option<Vec<u8>>,
}

type HandshakeCell = Arc<Mutex<Handshake>>;
/// id переигранных `initialize`, чей ответ агенту отправлять не нужно: свой
/// он уже получил от первого ребёнка, второй с тем же id сбил бы Zed с толку.
type Swallow = Arc<Mutex<std::collections::HashSet<String>>>;

const RESTART_REFUSAL_TEXT: &str = "мост 1c-connect: сервер перезапущен на новой базе — повторите вызов";

/// Переиграть рукопожатие свежему ребёнку. Отдельный вызов `forward`, а не
/// `forward_counted`: это не вызов агента, и в `in_flight` ему не место —
/// ответ на `initialize` ловится через `swallow`, а не через обычный учёт.
async fn replay_handshake(stdin: &mut tokio::process::ChildStdin, handshake: &Handshake, swallow: &Swallow) -> Result<(), ()> {
    if let Some(init) = &handshake.initialize {
        if let Some(id) = request_id(init) {
            swallow.lock().unwrap().insert(key(&id));
        }
        forward(stdin, init).await?;
    }
    if let Some(ready) = &handshake.initialized {
        forward(stdin, ready).await?;
    }
    Ok(())
}

/// Одна строка от ребёнка: ответ на tools/list переписывается, но только у
/// моста, который подмешивает свои инструменты; остальное как есть. Список
/// допущенных инструментов для фильтрации берётся не из текущих параметров
/// (к моменту ответа они могли уже смениться), а из того, что действовало
/// на момент самого запроса — он и лежит в `pending` рядом с `id`.
fn rewrite_reply(fr: Frame, inject: bool, pending: &Pending) -> Vec<u8> {
    match fr {
        Frame::Raw(b) => b,
        Frame::Line(mut l) => {
            let entry = jsonrpc::is_response_with_id(&l).and_then(|id| pending.lock().unwrap().remove(&key(&id)));
            if inject {
                if let Some((_, allowed)) = &entry {
                    if let Some((_, out)) = jsonrpc::rewrite_list_reply(&l, allowed) {
                        return out;
                    }
                }
            }
            l.push(b'\n');
            l
        }
    }
}

/// Текст лимита сеансов учебной (и не только) платформы — он же идёт агенту
/// как есть, но человек должен узнать об этом из журнала: забытый сеанс
/// «Предприятия» держит единственную лицензию файловой базы.
const SESSION_LIMIT_TEXT: &str = "превышено предельное количество подключений";

/// Если строка от ребёнка — ответ инструмента с текстом лимита сеансов,
/// пишет строку в журнал; иначе ничего не делает. Ответ агенту эта функция
/// не трогает и не меняет. Разбор как JSON — не бесплатный (ответы с
/// деревом метаданных бывают мегабайтами), поэтому сначала строка
/// проверяется на ASCII-метку `isError` в сырых байтах, и только если она
/// нашлась — строка парсится и там уже ищется сама фраза. Искать в сырых
/// байтах саму кириллическую фразу нельзя: ребёнок вправе экранировать
/// не-ASCII как `\uXXXX`, и тогда байты фразы не встретятся вовсе, а разбор
/// JSON их всё равно вернул бы (serde сам снимает экранирование) — именно
/// это и было найдено ревью. `isError` — ASCII и в обычных (неошибочных)
/// ответах, в т.ч. мегабайтных деревьях метаданных, не встречается.
fn warn_on_session_limit(fr: &Frame, log: &Sink) {
    let Frame::Line(l) = fr else { return };
    if !bytes_contain(l, b"isError") {
        return;
    }
    let Ok(v) = serde_json::from_slice::<Value>(l) else { return };
    let Some(text) = v.pointer("/result/content/0/text").and_then(Value::as_str) else { return };
    if !text.contains(SESSION_LIMIT_TEXT) {
        return;
    }
    // Текст платформы может нести строку подключения целиком (File=…;Usr=…;
    // Pwd=…) — в журнал она идёт без пароля, тем же способом, что и хвост
    // stderr дочернего процесса.
    let text = common::docker::redact_text(text);
    log(Event::Warn(format!("агент не вошёл в базу: {text}; если 1С открыта у вас — закройте её")));
}

/// Подстрока в сырых байтах — без разбора кодировки: фраза либо есть в
/// строке как есть, либо нет.
fn bytes_contain(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Короткое имя нужно только тестам: рабочий код пишет полный путь.
    use crate::cfg::Cfg;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

    /// The test binary itself as the child: `__echo_child` below runs one of
    /// the scripted shapes picked by an env var — a portable fake mcp-1c/
    /// vrunner-mcp without a real build.
    fn child(env: (&str, &str)) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "bridge::tests::__echo_child", "--nocapture", "--quiet"]);
        c.env(env.0, env.1);
        c
    }

    #[test]
    fn __echo_child() {
        // Dies at once with a message on stderr — a portable "child never
        // got going" failure, for on_exit/redact_text coverage.
        if let Ok(msg) = std::env::var("BRIDGE_DIE") {
            eprintln!("{msg}");
            std::process::exit(3);
        }
        // A scripted MCP server: lists three tools, echoes the called tool's
        // name, prints a plain line on a cancel notification. NOISY: first
        // 200 KB of stderr, the way OneScript logs a long command.
        if std::env::var("BRIDGE_SCRIPT_NOISY").is_ok() {
            use std::io::Write;
            let line = "предупреждение OneScript, строка журнала номер N, ничего страшного\n".repeat(3000);
            std::io::stderr().write_all(line.as_bytes()).unwrap();
            std::io::stderr().flush().unwrap();
        }
        if std::env::var("BRIDGE_SCRIPT").is_ok() || std::env::var("BRIDGE_SCRIPT_NOISY").is_ok() || std::env::var("BRIDGE_SCRIPT_RECORD").is_ok() {
            use std::io::{BufRead, Write};
            let stdin = std::io::stdin();
            let mut out = std::io::stdout();
            // Доказательство того, НА КАКИХ параметрах этого ребёнка подняли:
            // `make_cmd` в тесте кладёт их в переменную окружения, ребёнок
            // печатает её значение один раз при старте, не дожидаясь строк —
            // без этого тест не отличил бы «тот же процесс ответил снова» от
            // «подняли нового на свежих параметрах».
            if let Ok(m) = std::env::var("BRIDGE_MARKER") {
                writeln!(out, "MARKER {m}").unwrap();
                out.flush().unwrap();
            }
            for line in stdin.lock().lines() {
                let line = line.unwrap();
                // A test wants proof of exactly what the child did or did not
                // read: a marker line reaches the agent as plain (non-JSON-RPC)
                // text, exactly like a banner from a real child would. Printed
                // for every line read, parseable or not — batches and junk
                // included — so a test can prove the bridge never forwarded them.
                if std::env::var("BRIDGE_SCRIPT_RECORD").is_ok() {
                    writeln!(out, "GOT {line}").unwrap();
                    out.flush().unwrap();
                }
                let v: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let id = v["id"].clone();
                if v["method"].as_str() == Some("tools/call") && std::env::var("BRIDGE_SCRIPT_RECORD").is_ok() {
                    writeln!(out, "CALL {}", v["params"]["name"].as_str().unwrap_or("")).unwrap();
                    out.flush().unwrap();
                }
                let reply = match v["method"].as_str() {
                    Some("initialize") => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"script","version":"0"}}}),
                    Some("tools/list") => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"cf_load"},{"name":"infobase_delete"},{"name":"task_status"}]}}),
                    Some("tools/call") => {
                        let mut text = format!("child ran {}", v["params"]["name"].as_str().unwrap_or(""));
                        if std::env::var("BRIDGE_SCRIPT_ARGS").is_ok() {
                            text = format!("{text} args={}", v["params"]["arguments"]);
                        }
                        // Real MCP tool errors carry isError:true — the limit
                        // reply must shape it the same way, since the bridge's
                        // prefilter now anchors on that ASCII key.
                        let is_error = std::env::var("BRIDGE_SCRIPT_LIMIT").is_ok();
                        if is_error {
                            text = "ошибка сервера: превышено предельное количество подключений".into();
                        }
                        serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}],"isError":is_error}})
                    }
                    Some("notifications/cancelled") => {
                        writeln!(out, "child saw cancel {}", v["params"]["requestId"]).unwrap();
                        out.flush().unwrap();
                        continue;
                    }
                    _ => continue,
                };
                writeln!(out, "{reply}").unwrap();
                out.flush().unwrap();
            }
            std::process::exit(0);
        }
        // Ignores EOF on stdin: a server that does not exit when the client goes.
        if std::env::var("BRIDGE_LINGER").is_ok() {
            let mut s = String::new();
            while std::io::stdin().read_line(&mut s).map(|n| n > 0).unwrap_or(false) {}
            std::thread::sleep(std::time::Duration::from_secs(60));
            std::process::exit(0);
        }
        // Reads one line and leaves without answering.
        if std::env::var("BRIDGE_DIE_AFTER_READ").is_ok() {
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            std::process::exit(0);
        }
    }

    /// `Cfg` для тестов: белый список и каталог проекта (временный, не
    /// git-репозиторий — хватает для путей, где `project_status` обязан
    /// просто ответить ошибкой быстро, без настоящего git).
    fn test_cfg(allowed: &[&str]) -> Cfg {
        let mut cfg = Cfg::default();
        cfg.host.tools = allowed.iter().map(|s| s.to_string()).collect();
        cfg.host.project = std::env::temp_dir().to_string_lossy().into_owned();
        cfg
    }

    fn rules(allowed: &[&str], running: usize) -> Rules {
        rules_with(allowed, running, Arc::new(AtomicBool::new(true)))
    }

    /// Как `rules`, но с заданным переключателем — чтобы тест мог выключать
    /// и включать MCP прямо посреди разговора. `rebuild` — заглушка: эти
    /// тесты не меняют `live` после создания, так что рестарт никогда не
    /// срабатывает и пересборщик не зовётся.
    fn rules_with(allowed: &[&str], running: usize, enabled: Arc<AtomicBool>) -> Rules {
        Rules::for_vrunner(
            Live::new(test_cfg(allowed)),
            std::path::PathBuf::new(),
            std::sync::Arc::new(|_: &Cfg| tokio::process::Command::new("rebuild-unused")),
            std::sync::Arc::new(move || running),
            enabled,
        )
    }

    /// Журнал, который ничего не делает — тестам, которым нужны только ответы агенту.
    fn no_log() -> Sink {
        Arc::new(|_: Event| {})
    }

    /// Строка-ответ с этим `id` уже встретилась среди накопленных строк.
    fn has_reply(text: &str, id: i64) -> bool {
        text.lines().any(|l| serde_json::from_str::<Value>(l).ok().map_or(false, |v| v["id"] == id))
    }

    /// Читать из `r` в `got`, пока `pred` по накопленному тексту не станет
    /// true или не истечёт `budget`. Опрос вместо фиксированной паузы: под
    /// нагрузкой (полный прогон тестов) спавн и обмен с ребёнком иногда
    /// занимают дольше любой разумной константы — тест должен ждать
    /// условие, а не угадывать время.
    async fn read_until(r: &mut (impl AsyncRead + Unpin), got: &mut Vec<u8>, pred: impl Fn(&str) -> bool, budget: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + budget;
        let mut buf = [0u8; 4096];
        loop {
            if pred(&String::from_utf8_lossy(got)) {
                return true;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match tokio::time::timeout(remaining.min(std::time::Duration::from_millis(50)), r.read(&mut buf)).await {
                Ok(Ok(0)) => return false,
                Ok(Ok(n)) => got.extend_from_slice(&buf[..n]),
                Ok(Err(_)) => return false,
                Err(_) => {}
            }
        }
    }

    /// Drive the bridge: send the lines, wait, close; return the JSON replies,
    /// the raw output and the child's exit code.
    async fn talk(child_env: (&str, &str), rules: Rules, send: &[&str]) -> (Vec<serde_json::Value>, String, i32) {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, child(child_env), rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        for l in send {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        let (code, _) = rx.await.unwrap();
        (got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect(), got, code)
    }

    #[test]
    fn connection_env_is_passed_verbatim() {
        let mut cfg = crate::cfg::Cfg::default();
        cfg.host.base_path = "C:\\Bases\\Бух".into();
        cfg.host.db_password = "па\"роль".into();
        cfg.host.project = "C:\\proj\\Бух".into();
        cfg.host.server = "10.0.0.5:3393".into();
        let c = vrunner_command(&cfg, std::path::Path::new("T")).into_std();
        let env: std::collections::HashMap<_, _> =
            c.get_envs().filter_map(|(k, v)| v.map(|v| (k.to_string_lossy().to_string(), v.to_string_lossy().to_string()))).collect();
        assert_eq!(env["VRUNNER_IBCONNECTION"], "/FC:\\Bases\\Бух");
        assert_eq!(env["VRUNNER_DBUSER"], "Администратор");
        assert_eq!(env["VRUNNER_DBPWD"], "па\"роль");
        assert_eq!(env["NO_PROXY"], "10.0.0.5");
        assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
        let sep = std::path::MAIN_SEPARATOR;
        let bin = format!("T{sep}vrunner{sep}bin");
        let git = if cfg!(windows) { format!("T{sep}git{sep}cmd") } else { format!("T{sep}git{sep}bin") };
        // Background tasks of vrunner-mcp re-exec `oscript` from PATH; git is the bridge's own.
        assert!(env["PATH"].starts_with(&format!("{bin}{}{git}", if cfg!(windows) { ';' } else { ':' })), "{}", env["PATH"]);
        assert_eq!(c.get_current_dir().unwrap(), std::path::Path::new("C:\\proj\\Бух"));
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        // Обе ветки проверяются по тому, что получилось, а не тем же решением,
        // которое на результат влияло (иначе тест зелен при любом ответе).
        // На машине без живого `DISPLAY`, но с `xvfb-run`, команда обёрнута
        // (`ops_host::with_display`): `xvfb-run`, `-a`, прежняя программа, и
        // только потом `mcp.os`. В CI (`ubuntu-24.04` без `DISPLAY`) берётся
        // именно эта ветка — проверка обязана работать там, а не молчать.
        let prog = c.get_program().to_string_lossy().to_string();
        if prog.ends_with("xvfb-run") {
            assert_eq!(args[0], "-a", "{args:?}");
            assert!(args[1].contains("oscript"), "{args:?}");
            assert!(args[2].ends_with("mcp.os"), "{args:?}");
            assert_eq!(args.len(), 3, "{args:?}");
        } else {
            assert!(prog.contains("oscript"), "{prog}");
            assert!(args[0].ends_with("mcp.os"), "{args:?}");
            assert_eq!(args.len(), 1, "{args:?}");
        }
    }

    #[test]
    fn connection_args_carry_the_base_and_its_user() {
        let mut cfg = crate::cfg::Cfg::default();
        cfg.host.base_path = "C:\\Bases\\Бух".into();
        cfg.host.db_user = "mcp".into();
        cfg.host.db_password = "p w".into();
        let a = connection_args(&cfg);
        assert_eq!(a["ibconnection"], "/FC:\\Bases\\Бух");
        assert_eq!(a["db-user"], "mcp");
        assert_eq!(a["db-pwd"], "p w");
        cfg.host.db_password = String::new();
        assert!(connection_args(&cfg).get("db-pwd").is_none(), "an empty password is not sent as a value");
    }

    #[tokio::test]
    async fn list_is_filtered_and_extended_through_the_bridge() {
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#]).await;
        assert_eq!(replies.len(), 1, "{raw}");
        let names: Vec<&str> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["cf_load", "project_status", "project_pull", "project_push"]);
    }

    #[tokio::test]
    async fn disabled_tool_never_reaches_the_child() {
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"infobase_delete","arguments":{}}}"#]).await;
        assert_eq!(replies.len(), 1, "{raw}");
        assert_eq!(replies[0]["result"]["isError"], true);
        assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("1c-connect.toml"));
        assert!(!raw.contains("child ran"));
    }

    #[tokio::test]
    async fn platform_call_is_refused_while_the_configurator_runs() {
        let req = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#;
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 2), &[req]).await;
        assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("закройте Конфигуратор"), "{raw}");
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&["cf_load"], 0), &[req]).await;
        assert_eq!(replies[0]["result"]["content"][0]["text"], "child ran cf_load", "{raw}");
    }

    #[tokio::test]
    async fn a_tool_call_is_refused_while_mcp_are_off() {
        // Ребёнок запускается сразу на соединении, даже при выключенном
        // переключателе (решение заказчика: простаивающий ребёнок не держит
        // сеанс 1С и умирает вместе с сеансом) — и отвечает на initialize/
        // tools/list. Единственное, что до него не должно дойти, — сам
        // вызов инструмента; "CALL cf_load" — метка, которую ребёнок пишет,
        // только когда и если увидел tools/call (см. BRIDGE_SCRIPT_RECORD).
        let enabled = Arc::new(AtomicBool::new(false));
        let (replies, raw, _) = talk(
            ("BRIDGE_SCRIPT_RECORD", "1"),
            rules_with(&["cf_load"], 0, enabled),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#,
            ],
        )
        .await;
        assert!(!raw.contains("CALL cf_load"), "ребёнок не должен был увидеть вызов: {raw}");
        let call = replies.iter().find(|r| r["id"] == 3).unwrap();
        assert_eq!(call["result"]["isError"], true);
        // Начало отказа, а не весь текст: хвост зависит от того, заперт ли
        // сейчас холодный замок окна (`bridge::gate_reason`), а тест — про гейт.
        assert!(call["result"]["content"][0]["text"].as_str().unwrap_or_default().starts_with(DISABLED_PREFIX), "{raw}");
        assert!(replies.iter().any(|r| r["id"] == 1), "initialize должен был ответить: {raw}");
        assert!(replies.iter().any(|r| r["id"] == 2), "tools/list должен был ответить: {raw}");
    }

    /// Пока замок заперт, MCP не выключены человеком — их нельзя включить, и
    /// совет «включите переключатель» был бы ложью: в отказе едет причина.
    #[test]
    fn the_refusal_carries_the_off_reason() {
        assert_eq!(disabled_text(None), DISABLED_TEXT);
        assert_eq!(disabled_text(Some("не задан пользователь базы")), "MCP выключены в 1c-connect: не задан пользователь базы");
    }

    #[tokio::test]
    async fn the_mcp_port_gates_only_tools_call_while_off() {
        // inject == false (777N, mcp-1c) + переключатель выключен: гейт на
        // tools/call — единственное, что этот порт делает. Неизвестный
        // метод и уведомление проходят насквозь, как и раньше у
        // pipe_process, и ответ на tools/call — только отказ, который
        // никогда не превращается в Forward/PlatformCall (их тут вообще
        // нет: этот путь не вызывает route_request_with).
        let rules = Rules::for_mcp(Arc::new(AtomicBool::new(false)));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, child(("BRIDGE_SCRIPT_RECORD", "1")), rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        for l in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            r#"{"jsonrpc":"2.0","method":"кто-то/вендорский"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#,
        ] {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        assert!(!got.contains("CALL cf_load"), "гейт должен был поймать tools/call: {got}");
        assert!(!got.contains("-32601"), "неизвестный метод не должен получать ответ моста: {got}");
        let replies: Vec<Value> = got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        assert!(replies.iter().any(|r| r["id"] == 1), "initialize должен был пройти насквозь: {got}");
        let call = replies.iter().find(|r| r["id"] == 2).unwrap();
        assert!(call["result"]["content"][0]["text"].as_str().unwrap_or_default().starts_with(DISABLED_PREFIX), "{got}");
    }

    #[tokio::test]
    async fn a_tools_call_without_an_id_never_reaches_the_child() {
        // Нет id — отвечать нечем, а пропустить вызов — значит исполнить его
        // побочный эффект (cf_load) без единого следа в ответах агенту.
        // Отбой всегда, независимо от переключателя (решение заказчика).
        let call = r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#;
        for on in [false, true] {
            let (replies, raw, _) = talk(("BRIDGE_SCRIPT_RECORD", "1"), rules_with(&["cf_load"], 0, Arc::new(AtomicBool::new(on))), &[call]).await;
            assert!(replies.is_empty(), "вызову без id отвечать нечем (переключатель {on}): {raw}");
            assert!(!raw.contains("cf_load"), "ребёнок не должен был увидеть вызов (переключатель {on}): {raw}");
        }
    }

    #[tokio::test]
    async fn the_mcp_port_forwards_junk_only_when_the_switch_is_on() {
        // Второе ревью: гейт по tools_call_id пропускал насквозь всё, что
        // сам не опознал (батч, мусор) — пока выключено, это лазейка. Теперь
        // наоборот: вперёд уходит только уверенно безопасное, остальное —
        // закрыто по умолчанию. Включено — труба прозрачна, как и была.
        async fn drive(on: bool) -> String {
            let rules = Rules::for_mcp(Arc::new(AtomicBool::new(on)));
            let (ours, theirs) = tokio::io::duplex(1 << 16);
            let (tx, _rx) = tokio::sync::oneshot::channel();
            tokio::spawn(pipe_jsonrpc(theirs, child(("BRIDGE_SCRIPT_RECORD", "1")), rules, no_log(), move |code, err| {
                let _ = tx.send((code, err));
            }));
            let (r, mut w) = tokio::io::split(ours);
            for l in [r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#, "junk"] {
                w.write_all(l.as_bytes()).await.unwrap();
                w.write_all(b"\n").await.unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            w.shutdown().await.unwrap();
            let mut got = String::new();
            tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
            got
        }

        let off = drive(false).await;
        assert!(!off.contains("GOT"), "батч и мусор не должны были дойти до ребёнка, пока MCP выключены: {off}");

        let on = drive(true).await;
        assert!(on.contains(r#"GOT [{"jsonrpc":"2.0","id":1,"method":"ping"}]"#), "{on}");
        assert!(on.contains("GOT junk"), "{on}");
    }

    #[tokio::test]
    async fn the_mcp_off_path_journals_unrecognised_lines_once_per_connection() {
        // A banner or a keepalive repeating on every line must not fill the
        // journal with identical entries, and a bare newline (an empty line
        // frame) must not produce one at all.
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let log: Sink = Arc::new(move |e: Event| {
            if let Event::Warn(w) = e {
                seen2.lock().unwrap().push(w);
            }
        });
        let rules = Rules::for_mcp(Arc::new(AtomicBool::new(false)));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, child(("BRIDGE_SCRIPT_RECORD", "1")), rules, log, move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        for l in ["junk1", "", "junk2", "junk3"] {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        assert!(!got.contains("GOT"), "ничего не должно было дойти до ребёнка: {got}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "одна запись на всё соединение, не на каждую строку: {seen:?}");
        assert!(seen[0].contains("не пишутся"), "{}", seen[0]);
    }

    #[tokio::test]
    async fn flipping_the_switch_mid_connection_starts_serving() {
        let enabled = Arc::new(AtomicBool::new(false));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(
            theirs,
            child(("BRIDGE_SCRIPT", "1")),
            rules_with(&["cf_load"], 0, enabled.clone()),
            no_log(),
            move |code, err| {
                let _ = tx.send((code, err));
            },
        ));
        let (r, mut w) = tokio::io::split(ours);
        let call = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#;
        w.write_all(call).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        enabled.store(true, Ordering::Relaxed);
        w.write_all(call).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        let replies: Vec<Value> = got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        assert_eq!(replies.len(), 2, "{got}");
        assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap_or_default().starts_with(DISABLED_PREFIX), "{got}");
        assert_eq!(replies[1]["result"]["content"][0]["text"], "child ran cf_load", "{got}");
    }

    #[tokio::test]
    async fn the_mcp_port_does_not_get_our_tools_injected() {
        let mut cmd = child(("BRIDGE_SCRIPT", "1"));
        cmd.env("BRIDGE_SCRIPT_ARGS", "1");
        let rules = Rules::for_mcp(Arc::new(AtomicBool::new(true)));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, cmd, rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        for l in [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cf_load","arguments":{"x":1}}}"#,
        ] {
            w.write_all(l.as_bytes()).await.unwrap();
            w.write_all(b"\n").await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        w.shutdown().await.unwrap();
        let mut got = String::new();
        tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
        let replies: Vec<Value> = got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let list = replies.iter().find(|r| r["id"] == 1).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(!names.iter().any(|n| n.starts_with("project_")), "{names:?}");
        let call = replies.iter().find(|r| r["id"] == 2).unwrap();
        let text = call["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"x\":1"), "{text}");
        assert!(!text.contains("ibconnection") && !text.contains("db-user"), "{text}");
    }

    #[tokio::test]
    async fn a_session_limit_answer_adds_one_journal_line() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let log: Sink = Arc::new(move |e: Event| {
            if let Event::Warn(w) = e {
                seen2.lock().unwrap().push(w);
            }
        });
        let mut cmd = child(("BRIDGE_SCRIPT", "1"));
        cmd.env("BRIDGE_SCRIPT_LIMIT", "1");
        let (replies, raw, _) = {
            let (ours, theirs) = tokio::io::duplex(1 << 16);
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(pipe_jsonrpc(theirs, cmd, rules(&["task_status"], 0), log, move |code, err| {
                let _ = tx.send((code, err));
            }));
            let (r, mut w) = tokio::io::split(ours);
            w.write_all(br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"task_status","arguments":{}}}"#).await.unwrap();
            w.write_all(b"\n").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            w.shutdown().await.unwrap();
            let mut got = String::new();
            tokio::io::BufReader::new(r).read_to_string(&mut got).await.unwrap();
            let _ = rx.await;
            (got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect::<Vec<Value>>(), got, 0)
        };
        assert_eq!(replies.len(), 1, "{raw}");
        let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(SESSION_LIMIT_TEXT), "{text}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].starts_with("агент не вошёл в базу") && seen[0].contains(SESSION_LIMIT_TEXT), "{}", seen[0]);
    }

    #[tokio::test]
    async fn our_tool_runs_in_its_own_task_and_the_child_keeps_answering() {
        // project_status against a directory that is not a repo fails fast
        // with isError — enough to prove the path; git itself is tested in project.rs.
        let (replies, raw, _) = talk(
            ("BRIDGE_SCRIPT", "1"),
            rules(&["task_status"], 0),
            &[
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"project_status","arguments":{}}}"#,
                r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"task_status","arguments":{}}}"#,
            ],
        )
        .await;
        assert_eq!(replies.len(), 2, "{raw}");
        assert!(replies.iter().any(|r| r["id"] == 5 && r["result"]["content"][0]["text"] == "child ran task_status"));
        assert!(replies.iter().any(|r| r["id"] == 4 && r["result"]["content"][0]["type"] == "text"));
    }

    #[tokio::test]
    async fn cancel_of_our_task_is_swallowed_and_of_a_foreign_one_forwarded() {
        let (replies, raw, _) = talk(
            ("BRIDGE_SCRIPT", "1"),
            rules(&[], 0),
            &[
                r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"project_status","arguments":{}}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":6}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":99}}"#,
            ],
        )
        .await;
        assert!(raw.contains("child saw cancel 99"), "{raw}");
        assert!(!raw.contains("child saw cancel 6"), "{raw}");
        // Ours either finished first or was cancelled — never two answers for 6.
        assert!(replies.iter().filter(|r| r["id"] == 6).count() <= 1);
    }

    #[tokio::test]
    async fn a_child_that_ignores_eof_is_killed_when_the_channel_closes() {
        let t0 = std::time::Instant::now();
        let (_replies, _raw, code) = talk(("BRIDGE_LINGER", "1"), rules(&[], 0), &[r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#]).await;
        assert_ne!(code, 0, "a killed child does not exit 0");
        assert!(t0.elapsed() < std::time::Duration::from_secs(20), "killed within the grace period, not after 60 s");
    }

    #[test]
    fn thin_client_counts_as_a_running_platform() {
        let csv = "\"1cv8c.exe\",\"1234\",\"Console\",\"1\",\"100 K\"\n\"1cv8.exe\",\"77\",\"Console\",\"1\",\"1 K\"\n\"notepad.exe\",\"1\",\"Console\",\"1\",\"1 K\"\n";
        assert_eq!(count_platform(csv), 2);
        assert_eq!(count_platform("INFO: No tasks are running which match the specified criteria.\n"), 0);
    }

    #[tokio::test]
    async fn oversized_and_junk_agent_lines_get_an_error_reply() {
        let big = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"pad\":\"{}\"}}", "x".repeat(crate::jsonrpc::LINE_LIMIT + 1));
        let (replies, raw, _) = talk(("BRIDGE_SCRIPT", "1"), rules(&[], 0), &[&big, "junk"]).await;
        assert!(!raw.contains("child ran"), "{}", &raw[..raw.len().min(200)]);
        let errors: Vec<_> = replies.iter().filter(|r| r["error"]["code"] == -32700).collect();
        assert_eq!(errors.len(), 2, "{replies:?}");
        assert!(errors.iter().all(|r| r["id"].is_null()));
    }

    #[tokio::test]
    async fn child_death_still_answers_pending_list_with_our_tools() {
        let (replies, raw, code) = talk(("BRIDGE_DIE_AFTER_READ", "1"), rules(&["cf_load"], 0), &[r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#]).await;
        assert_eq!(code, 0);
        assert_eq!(replies.len(), 1, "{raw}");
        assert_eq!(replies[0]["id"], 7);
        let names: Vec<&str> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["project_status", "project_pull", "project_push"]);
    }

    #[tokio::test]
    async fn a_child_that_dies_at_once_reports_a_redacted_stderr_tail() {
        // Coverage lost when pipe_process (and its own version of this test)
        // was deleted: pipe_jsonrpc carries the same on_exit/redact_text
        // wiring and deserves the same proof — a secret in the child's
        // stderr must never reach the journal in the clear.
        let (ours, theirs) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(
            theirs,
            child(("BRIDGE_DIE", "ошибка подключения: --db-password hunter2")),
            rules(&[], 0),
            no_log(),
            move |code, err| {
                let _ = tx.send((code, err));
            },
        ));
        let (mut r, _w) = tokio::io::split(ours);
        let mut got = Vec::new();
        r.read_to_end(&mut got).await.unwrap(); // EOF: the stream closed
        let (code, err) = rx.await.unwrap();
        assert_eq!(code, 3);
        // `redact_text` затирает значение после флага, разделённого пробелом,
        // то есть получается `--db-password …`, а не `--db-password=…`.
        assert!(!err.contains("hunter2") && err.contains("--db-password") && err.contains('…'), "{err}");
    }

    /// 6B, главный сценарий: Zed держит одно соединение на весь сеанс и сам
    /// не переподключается. После «Сохранить» с новой базой второй вызов на
    /// ТОМ ЖЕ соединении обязан попасть к ребёнку, поднятому уже на новых
    /// параметрах, — не к старому, у которого они намертво зашиты при
    /// запуске (строка подключения, пользователь, рабочий каталог).
    #[tokio::test]
    async fn config_change_restarts_the_child_with_fresh_values() {
        let mut cfg_a = test_cfg(&["cf_load"]);
        cfg_a.host.db_user = "UserA".into();
        let live = Live::new(cfg_a.clone());
        // `make_cmd` — ровно то, что делает настоящий `vrunner_command`: берёт
        // параметры из `Cfg` на момент запуска ребёнка и зашивает их туда,
        // откуда на лету их уже не поменять (здесь — переменная окружения,
        // которую фальшивый ребёнок печатает один раз при старте).
        let make_cmd = |cfg: &Cfg| {
            let mut c = child(("BRIDGE_SCRIPT_RECORD", "1"));
            c.env("BRIDGE_MARKER", cfg.host.db_user.clone());
            c
        };
        let cmd0 = make_cmd(&cfg_a);
        let rules = Rules::for_vrunner(live.clone(), std::path::PathBuf::new(), Arc::new(make_cmd), Arc::new(|| 0), Arc::new(AtomicBool::new(true)));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, cmd0, rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (r, mut w) = tokio::io::split(ours);
        // Ждём сам маркер, а не «столько-то миллисекунд»: ребёнок здесь —
        // целый процесс, и под нагрузкой (чужая сборка рядом, CI) он не
        // укладывается в фиксированную паузу. Срок большой и один на обоих,
        // падать по нему тест будет только на настоящей поломке.
        let mut lines = tokio::io::BufReader::new(r).lines();
        let call = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#;
        w.write_all(call).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        assert_eq!(wait_line(&mut lines, |l| l.starts_with("MARKER ")).await, "MARKER UserA");
        // Дожидаемся именно **ответа** на первый вызов: мост меняет ребёнка
        // только на праздном соединении (`in_flight == 0`), и послать второй
        // вызов раньше ответа значило бы отдать его старому ребёнку — не
        // потому что рестарт сломан, а потому что мы поторопились.
        wait_line(&mut lines, |l| l.starts_with('{') && l.contains("\"id\":1")).await;

        let mut cfg_b = live.get().1;
        cfg_b.host.db_user = "UserB".into();
        live.set(cfg_b);
        w.write_all(call).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        assert_eq!(
            wait_line(&mut lines, |l| l.starts_with("MARKER ")).await,
            "MARKER UserB",
            "второй вызов должен был дойти до ребёнка, поднятого уже на новых параметрах"
        );
        w.shutdown().await.unwrap();
    }

    /// Ближайшая строка ответа моста, подходящая под условие; всё прочее
    /// пропускаем. Ждёт событие, а не «столько-то миллисекунд»: ребёнок
    /// здесь — целый процесс, и под нагрузкой (чужая сборка рядом, CI) он не
    /// укладывается в фиксированную паузу — прежние `sleep(300 мс)`/
    /// `sleep(500 мс)` и давали мигающий тест. Срок большой: падать по нему
    /// тест будет только на настоящей поломке.
    async fn wait_line(lines: &mut tokio::io::Lines<tokio::io::BufReader<impl tokio::io::AsyncRead + Unpin>>, ok: impl Fn(&str) -> bool) -> String {
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(20), lines.next_line())
                .await
                .expect("мост молчит дольше 20 с")
                .expect("чтение ответа моста")
                .expect("поток кончился раньше ожидаемой строки");
            if ok(&line) {
                return line;
            }
        }
    }

    /// Не половина старой и половина новой конфигурации в одном вызове:
    /// каждый ответ несёт строку подключения, пользователя и пароль из
    /// одного и того же снимка `Cfg` — то, что подставляет мост в аргументы
    /// вызова (`connection_args`), читается один раз на весь вызов.
    #[tokio::test]
    async fn config_change_never_mixes_old_and_new_into_one_calls_arguments() {
        let mut cfg_a = test_cfg(&["cf_load"]);
        cfg_a.host.base_path = "C:\\Bases\\A".into();
        cfg_a.host.db_user = "UserA".into();
        cfg_a.host.db_password = "pwA".into();
        let mut cfg_b = test_cfg(&["cf_load"]);
        cfg_b.host.base_path = "C:\\Bases\\B".into();
        cfg_b.host.db_user = "UserB".into();
        cfg_b.host.db_password = "pwB".into();
        let live = Live::new(cfg_a.clone());
        let make_cmd = |_: &Cfg| {
            let mut c = child(("BRIDGE_SCRIPT", "1"));
            c.env("BRIDGE_SCRIPT_ARGS", "1");
            c
        };
        let cmd0 = make_cmd(&cfg_a);
        let rules = Rules::for_vrunner(live.clone(), std::path::PathBuf::new(), Arc::new(make_cmd), Arc::new(|| 0), Arc::new(AtomicBool::new(true)));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, cmd0, rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (mut r, mut w) = tokio::io::split(ours);
        let call = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#;
        w.write_all(call).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        live.set(cfg_b.clone());
        w.write_all(call).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        w.shutdown().await.unwrap();

        let mut got = String::new();
        r.read_to_string(&mut got).await.unwrap();
        let replies: Vec<Value> = got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        assert_eq!(replies.len(), 2, "{got}");
        let args_of = |reply: &Value| -> Value {
            let text = reply["result"]["content"][0]["text"].as_str().unwrap();
            serde_json::from_str(text.split("args=").nth(1).unwrap()).unwrap()
        };
        let args1 = args_of(&replies[0]);
        let args2 = args_of(&replies[1]);
        let expected_a = connection_args(&cfg_a);
        let expected_b = connection_args(&cfg_b);
        assert_eq!(args1["ibconnection"], expected_a["ibconnection"]);
        assert_eq!(args1["db-user"], expected_a["db-user"]);
        assert_eq!(args1["db-pwd"], expected_a["db-pwd"]);
        assert_eq!(args2["ibconnection"], expected_b["ibconnection"]);
        assert_eq!(args2["db-user"], expected_b["db-user"]);
        assert_eq!(args2["db-pwd"], expected_b["db-pwd"]);
        // Ни строка подключения, ни пользователь одного снимка не просочились в ответ другого.
        assert_ne!(args1["ibconnection"], args2["ibconnection"], "{got}");
        assert_ne!(args1["db-user"], args2["db-user"], "{got}");
    }

    /// 6B, довершение: свежий ребёнок после рестарта — обычный MCP-сервер и
    /// вправе отказать первому tools/call, если до него не было initialize.
    /// Мост обязан переиграть рукопожатие сам, и агент должен получить ровно
    /// один ответ на каждый свой запрос — не два на initialize.
    #[tokio::test]
    async fn restart_replays_the_handshake_and_the_client_sees_one_reply_per_request() {
        let cfg_a = test_cfg(&["cf_load"]);
        let live = Live::new(cfg_a.clone());
        let make_cmd = |_: &Cfg| child(("BRIDGE_SCRIPT_RECORD", "1"));
        let cmd0 = make_cmd(&cfg_a);
        let rules = Rules::for_vrunner(live.clone(), std::path::PathBuf::new(), Arc::new(make_cmd), Arc::new(|| 0), Arc::new(AtomicBool::new(true)));
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_jsonrpc(theirs, cmd0, rules, no_log(), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (mut r, mut w) = tokio::io::split(ours);
        let mut got: Vec<u8> = Vec::new();
        w.write_all(br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        w.write_all(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        // Ждём именно ответ на initialize, а не фиксированную паузу — под
        // нагрузкой полного прогона тестов спавн фиктивного ребёнка может
        // занять заметно дольше, чем в одиночном запуске.
        assert!(
            read_until(&mut r, &mut got, |s| has_reply(s, 1), std::time::Duration::from_secs(10)).await,
            "нет ответа на initialize: {}",
            String::from_utf8_lossy(&got)
        );

        // Смена базы — второй ребёнок на том же соединении, который самого
        // рукопожатия никогда не видел.
        let mut cfg_b = live.get().1;
        cfg_b.host.base_path = "C:\\Bases\\B".into();
        live.set(cfg_b);
        w.write_all(br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#).await.unwrap();
        w.write_all(b"\n").await.unwrap();
        // То же: рестарт (убить старого, поднять нового, переиграть
        // рукопожатие) занимает переменное время — ждём ответ, не час.
        assert!(
            read_until(&mut r, &mut got, |s| has_reply(s, 2), std::time::Duration::from_secs(10)).await,
            "нет ответа на tools/call: {}",
            String::from_utf8_lossy(&got)
        );
        w.shutdown().await.unwrap();
        let mut tail = Vec::new();
        let _ = r.read_to_end(&mut tail).await;
        got.extend_from_slice(&tail);

        let got = String::from_utf8_lossy(&got).into_owned();
        let replies: Vec<Value> = got.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        assert_eq!(replies.iter().filter(|r| r["id"] == 1).count(), 1, "ровно один ответ на initialize, без дубля от нового ребёнка: {got}");
        assert_eq!(replies.iter().filter(|r| r["id"] == 2).count(), 1, "ровно один ответ на tools/call: {got}");
        assert_eq!(replies.iter().find(|r| r["id"] == 2).unwrap()["result"]["content"][0]["text"], "child ran cf_load", "{got}");
        // "GOT ..." печатается на каждую строку, которую ребёнок прочитал
        // (см. __echo_child): два вхождения — рукопожатие дошло и до
        // первого, и до второго (переигранного) ребёнка.
        assert_eq!(got.matches(r#""method":"initialize""#).count(), 2, "рукопожатие должно было дойти и до нового ребёнка: {got}");
        assert_eq!(got.matches("notifications/initialized").count(), 2, "{got}");
    }

    #[tokio::test]
    async fn a_dead_web_server_is_reported_not_swallowed() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l); // nothing listens there any more
        let (_ours, theirs) = tokio::io::duplex(64);
        let r = pipe_tcp(theirs, port).await;
        assert!(r.is_err(), "connect to a closed port must be an error");
        assert!(r.unwrap_err().contains(&port.to_string()));
    }

    #[tokio::test]
    async fn tcp_side_is_a_plain_relay() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut b = [0u8; 5];
            s.read_exact(&mut b).await.unwrap();
            s.write_all(b"pong!").await.unwrap();
        });
        let (ours, theirs) = tokio::io::duplex(1024);
        tokio::spawn(pipe_tcp(theirs, port));
        let (mut r, mut w) = tokio::io::split(ours);
        w.write_all(b"ping!").await.unwrap();
        let mut b = [0u8; 5];
        r.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong!");
    }
}
