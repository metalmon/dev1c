//! Локальный транспорт: сервер сам слушает оба порта на петле (127.0.0.1) —
//! используется, когда на одной машине всё: база, агент, мост и проект
//! (`cfg.current().mode == "local"`), и пробрасывать порты через SSH некуда и
//! незачем — агент стучится на петлю отсюда же. Дальше — тот же
//! `session::Serve`, что и у туннеля: один разбор «порт → мост» на оба
//! транспорта. Переподключаться здесь не от чего, поэтому ни повторов, ни
//! состояния `Retrying` в этом режиме не бывает.

use crate::cfg::Ports;
use crate::holder;
use crate::session::Serve;
use common::Event;
use std::net::Shutdown;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Текст отказа бинда: при `AddrInUse` называет держателя порта, если его
/// видно (`holder::of_port`), и признаёт незнание, когда не видно.
///
/// «Занят ДРУГИМ процессом» мы не говорим (находка владельца 26.10): про
/// невидимого держателя мы не знаем ни того, что он чужой, ни того, что он
/// ещё жив — им мог быть и наш же прежний процесс, уже успевший закрыться.
/// Правило ветки: говорить ровно то, что знаем.
fn busy_port_text(port: u16, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::AddrInUse {
        match holder::of_port(port) {
            Some(who) => format!("порт {port} занят процессом {who}"),
            None => format!("порт {port} занят, кем — не видно"),
        }
    } else {
        format!("порт {port}: {e}")
    }
}

/// После скольких подряд ошибок `accept()` сеанс сдаётся вместо того, чтобы
/// крутиться впустую (кончились дескрипторы или ошибка не проходит сама).
const MAX_ACCEPT_ERRORS: u32 = 20;
/// Пауза между попытками `accept()` после ошибки — не даёт циклу сжечь ядро,
/// пока ошибка не ушла сама (например, кто-то другой освободил дескрипторы).
const ACCEPT_ERROR_PAUSE: Duration = Duration::from_millis(200);
/// Занять порт на петле ОБОИМИ семействами адресов.
///
/// Зачем не один `127.0.0.1`: пока мы занимали только IPv4, `::1` оставался
/// свободен — и тот же логический порт спокойно забирал кто-то другой. На
/// стенде 26.10 это был проброс SSH с машины человека: `sshd` сдаётся только
/// если не смог НИ ОДНО семейство, занял `::1`, доложил «проброс поднят», и
/// готовый текст «рабочее место занято» (`tunnel::workstation_busy_text`) не
/// сработал ни разу. К какой базе попадёт агент, решал после этого порядок в
/// резолвере: `localhost` — это и `127.0.0.1`, и `::1`.
///
/// IPv4 обязателен. IPv6 — по возможности: на машине без него `bind` отвечает
/// отказом, и это не беда, занимать нечего. Но `AddrInUse` на `::1` — это
/// ровно тот случай, ради которого всё и делается: порт занят кем-то ещё, и
/// молчать нельзя.
async fn bind_loopback(port: u16) -> anyhow::Result<(TcpListener, Option<TcpListener>)> {
    use std::net::{Ipv4Addr, Ipv6Addr};
    let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await.map_err(|e| anyhow::anyhow!(busy_port_text(port, &e)))?;
    let v6 = match TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
        Ok(l) => Some(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return Err(anyhow::anyhow!(busy_port_text(port, &e))),
        Err(_) => None,
    };
    Ok((v4, v6))
}

/// `accept()` того слушателя, которого может и не быть: без IPv6 ветка
/// `select!` обязана просто никогда не срабатывать, а не падать.
async fn accept_maybe(l: &Option<TcpListener>) -> std::io::Result<(TcpStream, std::net::SocketAddr)> {
    match l {
        Some(l) => l.accept().await,
        None => std::future::pending().await,
    }
}

/// Поднимает слушатели mcp и vrunner на петле (веб-порт в локальном режиме
/// не слушается) и раздаёт принятые соединения `serve`. Ошибка второго бинда
/// закрывает первый слушатель и возвращается `Err` с номером занятого порта.
/// Останавливается по `stop`: закрыть `TcpListener` уже принятых соединений
/// не трогает, поэтому каждое принятое соединение закрывается напрямую —
/// `shutdown` общего сокета доходит до моста, что бы тот сейчас ни делал, и
/// расходится по обеим его копиям дескриптора.
/// Куда вести принятое соединение: в СВОЙ мост или трубой в канал, к той стороне, где база.
///
/// Решается один раз, на весь сеанс, по сочетанию соединения (`cfg::Combo`) — не по каждому соединению и не по порту. «Уйти в канал,
/// когда база здесь» и «поднять своего ребёнка, когда базы нет» этим становятся невыразимы, а не запрещены условием.
///
/// Канал зовётся ЗАМЫКАНИЕМ, а не ручкой потока узла: слушатель собирается и без окна (`cargo build` без фичи `gui`), а поток узла
/// живёт за ней. Кто умеет увести соединение в канал, тот и даёт сюда замыкание.
#[derive(Clone)]
pub enum Toward {
    /// База здесь: поднимаем своего ребёнка тем же мостом, что и всегда.
    Child,
    /// База на другом компьютере: соединение уходит трубой в канал.
    Channel(ToChannel),
}

/// Слова отказа, когда база на другом компьютере, а канал держать некому: подкоманда без окна.
///
/// Отказ ОДИН на оба случая, которые прежде считались по отдельности («канал нужен, а ручки нет» и «ручки нет, и канала не
/// будет»): своего ребёнка над ЧУЖОЙ базой мы не поднимаем ни там, ни там, и человеку в обоих случаях говорится одно и то же.
pub const NO_WINDOW: &str =
    "база этого соединения на другом компьютере, а связь с ним держит окно программы — без окна вызовы вести некуда";

impl Toward {
    /// Куда вести принятое соединение — ОДНИМ решением, по сочетанию и по тому, есть ли чем держать канал.
    ///
    /// Конструктор стоит здесь, рядом с типом и с тем, кто его потребляет, а не у сеанса: прежде сеанс считал развилку своим
    /// трёхзначным `Transport` и тут же пересобирал её в этот двузначный `Toward`, схлопывая два своих варианта в один отказ.
    /// Два слова на одну развилку — та же беда, что «две меры на один вопрос», только в словаре: читающий обязан держать в
    /// голове оба перечисления и помнить, какой вариант одного во что превращается у другого.
    ///
    /// **Ось одна — где база** (спека сочетаний). Третьим ходом здесь был ssh в среду разработки, и с 11.11 его нет: среда
    /// уехала из программы целиком (план `2026-11-11-env-removal.md`), а выбирать транспорт по режиму стало невыразимо.
    pub fn of(combo: crate::cfg::Combo, to_channel: Option<ToChannel>) -> Result<Toward, &'static str> {
        match (combo.base_is_here(), to_channel) {
            (true, _) => Ok(Toward::Child),
            (false, Some(to_channel)) => Ok(Toward::Channel(to_channel)),
            (false, None) => Err(NO_WINDOW),
        }
    }
}

/// Увести соединение в канал к той стороне, где база. Отказ — словами в журнал сеанса.
pub type ToChannel =
    std::sync::Arc<dyn Fn(crate::ops_host::McpServer, TcpStream) -> Result<tokio::sync::oneshot::Receiver<()>, String> + Send + Sync>;

pub async fn run(
    serve: Arc<Serve>,
    ports: Ports,
    toward: Toward,
    on_event: impl Fn(Event),
    mut stop: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let (mcp_listener, mcp6) = bind_loopback(ports.mcp).await?;
    let (vrunner_listener, vrunner6) = match bind_loopback(ports.vrunner).await {
        Ok(l) => l,
        Err(e) => {
            drop(mcp_listener);
            drop(mcp6);
            return Err(e);
        }
    };
    // `Ok` здесь — не про цвет строки, а про состояние: `session::State::next`
    // переводит связь в «поднято» именно по нему. Нейтральным в журнале его
    // делает `session`, одной развилкой на оба транспорта: цвет решает тот,
    // кто пишет в журнал, а не тот, кто поднимает канал. Попытка сменить цвет
    // ЗДЕСЬ стоила того, что связь переставала подниматься вовсе, а карточка
    // навсегда застревала на «Включаю…» (30.10).
    on_event(Event::Ok(format!("Доступ к 1С включён: порты {} и {}", ports.mcp, ports.vrunner)));
    // Пара (задача моста, дубликат дескриптора) на каждое живое соединение.
    // Задача снимается из списка сама, как только закончилась — иначе список
    // и число открытых дескрипторов растут без предела за долгий рабочий
    // день реконнектов.
    let mut open: Vec<(JoinHandle<()>, std::net::TcpStream)> = Vec::new();
    // Вторые дескрипторы тех соединений, что уехали трубой в канал: своей задачи у них здесь нет, а закрывать их на остановке надо.
    // Убираются они тем же приёмом, что и задачи своих мостов, и по тому же доводу: без уборки список и число открытых дескрипторов
    // растут без предела за долгий рабочий день реконнектов (вторая проверка безопасности 11.11).
    let mut piped: Vec<(tokio::sync::oneshot::Receiver<()>, std::net::TcpStream)> = Vec::new();
    let mut mcp_errors = 0u32;
    let mut vrunner_errors = 0u32;
    loop {
        tokio::select! {
            r = mcp_listener.accept() => {
                handle_accept(r, &serve, &toward, ports, ports.mcp, &on_event, &mut open, &mut piped, &mut mcp_errors).await?;
            }
            r = vrunner_listener.accept() => {
                handle_accept(r, &serve, &toward, ports, ports.vrunner, &on_event, &mut open, &mut piped, &mut vrunner_errors).await?;
            }
            r = accept_maybe(&mcp6) => {
                handle_accept(r, &serve, &toward, ports, ports.mcp, &on_event, &mut open, &mut piped, &mut mcp_errors).await?;
            }
            r = accept_maybe(&vrunner6) => {
                handle_accept(r, &serve, &toward, ports, ports.vrunner, &on_event, &mut open, &mut piped, &mut vrunner_errors).await?;
            }
            _ = stop.changed() => {
                if *stop.borrow() {
                    for (_, w) in &open {
                        let _ = w.shutdown(Shutdown::Both);
                    }
                    for (_, w) in &piped {
                        let _ = w.shutdown(Shutdown::Both);
                    }
                    return Ok(());
                }
            }
        }
    }
}

/// Один исход `accept()`: на успехе — подготовить соединение и добавить его
/// в список живых (предварительно вымести завершившиеся — это и есть
/// уборка), на ошибке — предупреждение в журнал, пауза и, если ошибка не
/// отпускает `MAX_ACCEPT_ERRORS` раз подряд, завершение сеанса вместо
/// бесконечного холостого цикла.
#[allow(clippy::too_many_arguments)]
async fn handle_accept(
    r: std::io::Result<(TcpStream, std::net::SocketAddr)>,
    serve: &Arc<Serve>,
    toward: &Toward,
    ports: Ports,
    port: u16,
    on_event: &impl Fn(Event),
    open: &mut Vec<(JoinHandle<()>, std::net::TcpStream)>,
    piped: &mut Vec<(tokio::sync::oneshot::Receiver<()>, std::net::TcpStream)>,
    errors: &mut u32,
) -> anyhow::Result<()> {
    match r {
        Ok((stream, _)) => {
            *errors = 0;
            open.retain(|(h, _)| !h.is_finished());
            // Труба кончилась — её конец `done` уронен, и `try_recv` отвечает `Closed`: держать второй дескриптор больше незачем.
            piped.retain_mut(|(ended, _)| !matches!(ended.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Closed)));
            // База на другом компьютере — соединение уходит трубой в канал, и своего ребёнка здесь не поднимают вовсе.
            if let Toward::Channel(to_channel) = toward {
                if let Some(crate::session::Served::Mcp(kind)) = crate::session::Served::of_port(port, ports) {
                    // Второй дескриптор остаётся ЗДЕСЬ — тем же приёмом и по тому же доводу, что у своих мостов (`accept_for`):
                    // труба уезжает в поток узла и нашей остановки не слышит, а человек, выключивший доступ к 1С, вправе ждать, что
                    // выключилось и то, что уже идёт (находка проверки безопасности 11.11).
                    match stream.into_std().and_then(|s| s.try_clone().map(|w| (s, w))) {
                        Ok((s, watcher)) => match TcpStream::from_std(s) {
                            Ok(stream) => match to_channel(kind, stream) {
                                Ok(ended) => piped.push((ended, watcher)),
                                Err(why) => on_event(Event::Warn(why)),
                            },
                            Err(e) => on_event(Event::Warn(e.to_string())),
                        },
                        Err(e) => on_event(Event::Warn(e.to_string())),
                    }
                    return Ok(());
                }
            }
            match accept_for(serve, port, stream) {
                Ok(Some(entry)) => open.push(entry),
                // ponytail: порт всегда совпадает с тем, что знает `Serve` —
                // оба берутся из одного `cfg.ports()` при сборке сеанса;
                // `None` здесь означало бы рассогласование конфигурации, а
                // не рабочий случай, поэтому не журналируется отдельно.
                Ok(None) => {}
                Err(e) => on_event(Event::Warn(format!("порт {port}: не удалось подготовить соединение для моста: {e}"))),
            }
            Ok(())
        }
        Err(e) => {
            *errors += 1;
            on_event(Event::Warn(format!("порт {port}: не удалось принять соединение: {e}")));
            if *errors >= MAX_ACCEPT_ERRORS {
                return Err(anyhow::anyhow!("порт {port}: приём соединений не восстанавливается подряд уже {MAX_ACCEPT_ERRORS} раз: {e}"));
            }
            tokio::time::sleep(ACCEPT_ERROR_PAUSE).await;
            Ok(())
        }
    }
}

/// Раздваивает дескриптор принятого соединения перед тем, как отдать поток
/// мосту: вторая копия остаётся здесь на случай `stop` и уже не зависит от
/// того, что мост с первой копией делает (держит в мосте, читает построчно,
/// уже уронил). `shutdown` на любой копии закрывает общий сокет целиком.
fn accept_for(serve: &Serve, port: u16, stream: TcpStream) -> std::io::Result<Option<(JoinHandle<()>, std::net::TcpStream)>> {
    let std_stream = stream.into_std()?;
    let watcher = std_stream.try_clone()?;
    let async_stream = TcpStream::from_std(std_stream)?;
    // Порт решает ЗДЕСЬ и только здесь: дальше идёт слово (`Served`).
    Ok(crate::session::Served::of_port(port, serve.ports())
        .map(|what| (serve.serve(what, crate::session::Incoming::Local(async_stream)), watcher)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge;
    use common::kit::Kit;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Куда вести соединение, выбирает ОДНА ось — где база, и у всех пяти сочетаний ответ назван здесь целиком.
    ///
    /// Этот тест и есть то, чего не было 11.11: сторона с базой в парах A и B уходила в ssh-ветку и сеанса не поднимала вовсе
    /// (замер в плане `2026-11-11-env-removal.md`, раздел 1). Таблица названа по сочетаниям, а не по режиму: режим на этот вопрос
    /// больше не отвечает.
    ///
    /// Переехал сюда 12.11 вместе с решением: прежде он сверял промежуточный `session::Transport`, то есть ПЕРВУЮ половину
    /// развилки, а то, во что сеанс пересобирал её дальше, не сверял никто. Теперь сверяется сам ответ — и слова отказа тоже:
    /// человек без окна читает именно их.
    #[test]
    fn where_a_connection_is_led_follows_the_one_axis_that_says_where_the_base_is() {
        use crate::cfg::Combo::*;
        let channel: ToChannel = std::sync::Arc::new(|_, _| Err("в тесте канал не зовут".to_string()));
        for combo in [Alone, AcceptBaseHere, DialBaseHere] {
            assert!(matches!(Toward::of(combo, Some(channel.clone())), Ok(Toward::Child)), "{combo:?}: база здесь — свой мост");
            assert!(matches!(Toward::of(combo, None), Ok(Toward::Child)), "{combo:?}: и канал для этого не нужен");
        }
        for combo in [AcceptBaseThere, DialBaseThere] {
            let led = Toward::of(combo, Some(channel.clone()));
            assert!(matches!(led, Ok(Toward::Channel(_))), "{combo:?}: база на другом компьютере — труба в канал");
            // Не `unwrap_err`: он требует `Debug` у удачного исхода, а `Toward::Channel` несёт замыкание, которое его не имеет.
            match Toward::of(combo, None) {
                Err(why) => assert_eq!(why, NO_WINDOW, "{combo:?}: отказ говорится этими словами"),
                Ok(_) => panic!("{combo:?}: без окна вызовы вести некуда — должен быть отказ"),
            }
        }
    }

    /// Порт 0 — пусть ОС выберет сама и тут же вернёт выбранный номер;
    /// короткий зонд-и-отпустить не трогает 777x/778x вовсе и не привязан к
    /// рабочим местам — коллизия с живым коннектором на машине исключена.
    fn ephemeral_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
    }

    /// `Ports` не `Clone` — `Serve` и `run()` каждый берут свой экземпляр,
    /// построенный из одних и тех же чисел.
    fn ephemeral_ports() -> (u16, u16) {
        (ephemeral_port(), ephemeral_port())
    }

    /// `Kit` без настоящего комплекта: бинарей mcp-1c/vrunner в нём нет —
    /// ровно то, что нужно тесту, которому важен только факт, что соединение
    /// дошло до моста, а не живая 1С.
    fn fake_kit() -> (Kit, std::path::PathBuf) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("onec-listen-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        (Kit::at(&dir), dir)
    }

    fn recording_sink() -> (common::Sink, Arc<Mutex<Vec<Event>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        (Arc::new(move |e: Event| seen2.lock().unwrap().push(e)), seen)
    }

    #[tokio::test]
    async fn both_listeners_come_up_and_a_connection_reaches_the_bridge() {
        let cfg = crate::cfg::filled_for_test();
        let (mcp_port, vrunner_port) = ephemeral_ports();
        let (kit, dir) = fake_kit();
        let (bridge_log, bridge_seen) = recording_sink();
        let serve_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let serve = Arc::new(Serve::new(&cfg, &kit, bridge_log, serve_ports, Arc::new(AtomicBool::new(true))));
        let (stop_tx, stop_rx) = watch::channel(false);
        let (on_event, events) = recording_sink();
        let run_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let task = tokio::spawn(async move { run(serve, run_ports, Toward::Child, move |e| on_event(e), stop_rx).await });

        // Ждём, пока оба слушателя поднимутся, вместо фиксированной паузы.
        for _ in 0..50 {
            if events.lock().unwrap().iter().any(|e| matches!(e, Event::Ok(_))) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let said: Vec<Event> = events.lock().unwrap().clone();
        assert!(
            // Именно `Ok`: по нему `session::State::next` узнаёт, что связь
            // поднялась. Цвет строки в журнале решает `session`, не этот вызов.
            said.iter().any(|e| matches!(e, Event::Ok(s) if s.contains(&mcp_port.to_string()) && s.contains(&vrunner_port.to_string()) && !s.contains("туннель"))),
            "{said:?}"
        );

        // Соединение на порт mcp доходит до dispatch: процесса mcp-1c в
        // фиктивном комплекте нет, запуск падает — мост пишет об этом
        // ровно одну строку в свой журнал.
        let _conn = TcpStream::connect(("127.0.0.1", mcp_port)).await.unwrap();
        for _ in 0..50 {
            if !bridge_seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let warns: Vec<Event> = bridge_seen.lock().unwrap().clone();
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert!(matches!(&warns[0], Event::Warn(s) if s.contains("mcp-1c")), "{warns:?}");

        stop_tx.send(true).unwrap();
        assert!(task.await.unwrap().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Порт, занятый кем-то на `::1`, — занятый порт, даже если IPv4
    /// свободен. Пока мы брали одно семейство, проброс SSH с другой машины
    /// забирал второе, `sshd` докладывал успех, и человек не узнавал ничего:
    /// оба коннектора горели зелёным, а к какой базе попадёт агент, решал
    /// порядок в резолвере (находка владельца 26.10).
    ///
    /// На машине без IPv6 занимать нечего — там тест просто нечего проверять,
    /// и это НЕ отказ: `bind_loopback` в таком случае работает на одном IPv4.
    #[tokio::test]
    async fn a_port_held_on_ipv6_only_is_still_busy() {
        // Номер — из тестового диапазона, а не эфемерный: отпущенный ниже
        // эфемерный порт успевал достаться соседнему тесту, и обратный
        // захват («освободившийся порт берём сами») падал на пустом месте.
        let port = crate::ports::test_port();
        let Ok(held) = std::net::TcpListener::bind(("::1", port)) else { return };
        let e = bind_loopback(port).await.unwrap_err().to_string();
        assert!(e.contains(&port.to_string()) && e.contains("занят"), "{e}");
        drop(held);
        let (v4, v6) = bind_loopback(port).await.expect("освободившийся порт берём сами");
        assert_eq!(v4.local_addr().unwrap().port(), port);
        assert!(v6.is_some(), "второе семейство тоже наше");
    }

    /// Отключённый IPv6 — не отказ: занимать нечего, работаем на IPv4. Падать
    /// мы обязаны только на `AddrInUse`, то есть когда порт правда занят.
    #[tokio::test]
    async fn no_ipv6_on_the_machine_is_not_a_failure() {
        let port = ephemeral_port();
        let (v4, _v6) = bind_loopback(port).await.expect("IPv4 свободен — этого достаточно");
        assert_eq!(v4.local_addr().unwrap().port(), port);
    }

    #[tokio::test]
    async fn a_busy_port_fails_the_whole_session_and_frees_the_first_listener() {
        let cfg = crate::cfg::filled_for_test();
        let (mcp_port, vrunner_port) = ephemeral_ports();
        let (kit, dir) = fake_kit();
        let serve_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let serve = Arc::new(Serve::new(&cfg, &kit, Arc::new(|_: Event| {}), serve_ports, Arc::new(AtomicBool::new(true))));
        let busy = std::net::TcpListener::bind(("127.0.0.1", vrunner_port)).unwrap();
        let (_stop_tx, stop_rx) = watch::channel(false);

        let run_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let r = run(serve, run_ports, Toward::Child, |_: Event| {}, stop_rx).await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains(&vrunner_port.to_string()), "{err}");

        drop(busy);
        let retry = std::net::TcpListener::bind(("127.0.0.1", mcp_port));
        assert!(retry.is_ok(), "первый слушатель должен был освободиться");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Текст занятого порта говорит ровно то, что известно. Про невидимого
    /// держателя неизвестно ничего — ни что он чужой, ни что он ещё жив
    /// (находка владельца 26.10: «уже занят другим процессом» про давно
    /// свободный порт, который держал наш же прежний процесс).
    #[test]
    fn a_busy_port_claims_nothing_about_a_holder_it_cannot_see() {
        let in_use = std::io::Error::new(std::io::ErrorKind::AddrInUse, "address in use");

        // Держателя видно — это наш собственный слушатель, и он назван.
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let text = busy_port_text(port, &in_use);
        assert!(text.starts_with(&format!("порт {port} занят процессом ")) && text.contains("pid "), "{text}");
        drop(held);

        // Держателя не видно — ни слова о том, чей он и жив ли.
        let free = ephemeral_port();
        let text = busy_port_text(free, &in_use);
        assert_eq!(text, format!("порт {free} занят, кем — не видно"));
        assert!(!text.contains("другим процессом"), "{text}");

        // Не «занято» — причина как есть, без догадок о держателе.
        let text = busy_port_text(free, &std::io::Error::other("нет прав"));
        assert_eq!(text, format!("порт {free}: нет прав"));
    }

    #[tokio::test]
    async fn stop_closes_accepted_connections() {
        let cfg = crate::cfg::filled_for_test();
        let (mcp_port, vrunner_port) = ephemeral_ports();
        let (kit, dir) = fake_kit();
        let serve_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let serve = Arc::new(Serve::new(&cfg, &kit, Arc::new(|_: Event| {}), serve_ports, Arc::new(AtomicBool::new(true))));
        let (stop_tx, stop_rx) = watch::channel(false);
        let run_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let task = tokio::spawn(async move { run(serve, run_ports, Toward::Child, |_: Event| {}, stop_rx).await });

        let mut client = loop {
            match TcpStream::connect(("127.0.0.1", mcp_port)).await {
                Ok(s) => break s,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        };

        // Живо: чтение не возвращает ничего за отведённое время (здесь мост
        // падает за миллисекунды — соединение живо только нашим дубликатом
        // дескриптора; случай с реально заблокированным мостом — тест ниже).
        let mut buf = [0u8; 1];
        let before = tokio::time::timeout(std::time::Duration::from_millis(150), client.read(&mut buf)).await;
        assert!(before.is_err(), "соединение не должно было закрыться само");

        stop_tx.send(true).unwrap();
        assert!(task.await.unwrap().is_ok());

        // Закрыто: чтение теперь даёт 0 байт (EOF), а не висит и не падает.
        let after = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buf)).await;
        assert_eq!(after.unwrap().unwrap(), 0, "после stop соединение должно быть закрыто");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 6B: комплект сломан (как у `fake_kit`) — ребёнка не поднять ни разу,
    /// но соединение не рвётся. Agent получает честный отказ на каждый
    /// запрос, а не тишину и не разрыв — иначе Zed счёл бы сервер упавшим и
    /// человеку пришлось бы руками поднимать оба порта заново.
    #[tokio::test]
    async fn a_broken_kit_answers_every_request_with_a_refusal_and_keeps_the_connection_open() {
        let cfg = crate::cfg::filled_for_test();
        let (mcp_port, vrunner_port) = ephemeral_ports();
        let (kit, dir) = fake_kit();
        let serve_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let serve = Arc::new(Serve::new(&cfg, &kit, Arc::new(|_: Event| {}), serve_ports, Arc::new(AtomicBool::new(true))));
        let (stop_tx, stop_rx) = watch::channel(false);
        let run_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let task = tokio::spawn(async move { run(serve, run_ports, Toward::Child, |_: Event| {}, stop_rx).await });

        let mut client = loop {
            match TcpStream::connect(("127.0.0.1", vrunner_port)).await {
                Ok(s) => break s,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        };

        client.write_all(br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.write_all(br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cf_load","arguments":{}}}"#).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        for _ in 0..100 {
            if got.iter().filter(|&&b| b == b'\n').count() >= 2 {
                break;
            }
            match tokio::time::timeout(std::time::Duration::from_millis(50), client.read(&mut buf)).await {
                Ok(Ok(0)) => panic!("соединение закрылось само, хотя ребёнка никто не трогал: {}", String::from_utf8_lossy(&got)),
                Ok(Ok(n)) => got.extend_from_slice(&buf[..n]),
                Ok(Err(e)) => panic!("{e}"),
                Err(_) => {}
            }
        }
        let text = String::from_utf8_lossy(&got).into_owned();
        let replies: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let init = replies.iter().find(|r| r["id"] == 1).unwrap_or_else(|| panic!("нет ответа на initialize: {text}"));
        assert!(init["result"]["protocolVersion"].is_string(), "{text}");
        let call = replies.iter().find(|r| r["id"] == 2).unwrap_or_else(|| panic!("нет ответа на tools/call: {text}"));
        assert_eq!(call["result"]["isError"], true, "{text}");
        assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("не запускается"), "{text}");

        // Живо: дальше читать — не EOF и не обрыв, несмотря на то что
        // ребёнок так и не поднялся.
        let still_open = tokio::time::timeout(std::time::Duration::from_millis(200), client.read(&mut buf)).await;
        assert!(still_open.is_err(), "соединение должно было остаться открытым: {text}");

        stop_tx.send(true).unwrap();
        assert!(task.await.unwrap().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Самовызывающийся фиктивный ребёнок (тот же приём, что у
    /// `bridge::tests::__echo_child`): читает построчно и не отвечает,
    /// EOF игнорирует — ровно то, что нужно следующему тесту, чтобы мост
    /// был реально заблокирован на чтении, а не просто не успел упасть.
    #[test]
    fn __lingering_child() {
        if std::env::var("LISTEN_LINGER").is_ok() {
            let mut s = String::new();
            while std::io::stdin().read_line(&mut s).map(|n| n > 0).unwrap_or(false) {}
            std::thread::sleep(std::time::Duration::from_secs(60));
            std::process::exit(0);
        }
    }

    #[tokio::test]
    async fn stop_closes_a_connection_whose_bridge_is_genuinely_blocked() {
        // В отличие от теста выше, ребёнок здесь по-настоящему запускается и
        // висит: `to_child` моста реально заблокирован на чтении у клиента,
        // как в жизни при молчащем агенте. Соединение закрывает именно наш
        // дубликат дескриптора — мосту тут помогать нечем.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (log, _seen) = recording_sink();
        let rules = bridge::Rules::for_mcp(Arc::new(AtomicBool::new(true)), crate::wire::Ceiling::Full);
        let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "listen::tests::__lingering_child", "--nocapture", "--quiet"]);
        cmd.env("LISTEN_LINGER", "1");

        // Исход потомка запоминаем: без этого провал запуска выглядел бы как
        // «соединение закрылось само» и причину пришлось бы искать руками.
        let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel();
        let accept_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let std_stream = stream.into_std().unwrap();
            let watcher = std_stream.try_clone().unwrap();
            let async_stream = TcpStream::from_std(std_stream).unwrap();
            tokio::spawn(bridge::pipe_jsonrpc(async_stream, Ok(cmd), rules, log, move |code, _by_us, err| {
                let _ = exit_tx.send((code, err));
            }));
            watcher
        });

        let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let watcher = accept_task.await.unwrap();

        // Фиктивный потомок — это тестовый бинарь, и запускатель тестов печатает
        // в стандартный вывод свой баннер («running 1 test»). Мост честно
        // пересылает его клиенту, поэтому сначала вычитываем всё, что пришло
        // сразу, и только потом проверяем, что дальше тишина.
        let mut banner = [0u8; 256];
        while tokio::time::timeout(std::time::Duration::from_millis(200), client.read(&mut banner)).await.is_ok() {}

        let mut buf = [0u8; 1];
        // Ждём тишины, а не «за 300 мс не пришло ни байта»: под нагрузкой
        // (чужая сборка рядом, CI) потомок — а это целый тестовый бинарь —
        // успевает напечатать баннер уже после вычитывания выше, и прежняя
        // проверка падала на безобидных его байтах. Важно не молчание само
        // по себе, а то, что соединение не закрылось: закрытие — это `Ok(0)`,
        // всё остальное — тот же баннер, его просто пропускаем.
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(300), client.read(&mut buf)).await {
                Err(_) => break,
                Ok(Ok(0)) => panic!("мост реально заблокирован на чтении — соединение не должно было закрыться само"),
                Ok(Ok(_)) => continue,
                Ok(Err(e)) => panic!("чтение до shutdown не удалось: {e}"),
            }
        }
        if let Ok(Some((code, err))) = tokio::time::timeout(std::time::Duration::from_millis(1), exit_rx.recv()).await {
            panic!("фиктивный потомок не запустился (код {code}): {err}");
        }

        watcher.shutdown(Shutdown::Both).unwrap();
        let after = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf)).await;
        assert_eq!(after.unwrap().unwrap(), 0, "shutdown должен был закрыть соединение несмотря на заблокированный мост");
    }
}
