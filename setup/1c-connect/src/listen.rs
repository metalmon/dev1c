//! Локальный транспорт: сервер сам слушает оба порта на петле (127.0.0.1) —
//! используется, когда база 1С живёт на этой же машине (`cfg.host.mode ==
//! "local"`), вместо проброса портов через SSH. Дальше — тот же
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
/// видно (`holder::of_port`); иначе — как раньше, голое «занято».
fn busy_port_text(port: u16, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::AddrInUse {
        match holder::of_port(port) {
            Some(who) => format!("порт {port} занят процессом {who}"),
            None => format!("порт {port} уже занят другим процессом"),
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

/// Поднимает слушатели mcp и vrunner на петле (веб-порт в локальном режиме
/// не слушается) и раздаёт принятые соединения `serve`. Ошибка второго бинда
/// закрывает первый слушатель и возвращается `Err` с номером занятого порта.
/// Останавливается по `stop`: закрыть `TcpListener` уже принятых соединений
/// не трогает, поэтому каждое принятое соединение закрывается напрямую —
/// `shutdown` общего сокета доходит до моста, что бы тот сейчас ни делал, и
/// расходится по обеим его копиям дескриптора.
pub async fn run(serve: Arc<Serve>, ports: Ports, on_event: impl Fn(Event), mut stop: watch::Receiver<bool>) -> anyhow::Result<()> {
    let mcp_listener = TcpListener::bind(("127.0.0.1", ports.mcp)).await.map_err(|e| anyhow::anyhow!(busy_port_text(ports.mcp, &e)))?;
    let vrunner_listener = match TcpListener::bind(("127.0.0.1", ports.vrunner)).await {
        Ok(l) => l,
        Err(e) => {
            drop(mcp_listener);
            return Err(anyhow::anyhow!(busy_port_text(ports.vrunner, &e)));
        }
    };
    on_event(Event::Ok(format!("MCP включены: порты {} и {}", ports.mcp, ports.vrunner)));
    // Пара (задача моста, дубликат дескриптора) на каждое живое соединение.
    // Задача снимается из списка сама, как только закончилась — иначе список
    // и число открытых дескрипторов растут без предела за долгий рабочий
    // день реконнектов.
    let mut open: Vec<(JoinHandle<()>, std::net::TcpStream)> = Vec::new();
    let mut mcp_errors = 0u32;
    let mut vrunner_errors = 0u32;
    loop {
        tokio::select! {
            r = mcp_listener.accept() => {
                handle_accept(r, &serve, ports.mcp, &on_event, &mut open, &mut mcp_errors).await?;
            }
            r = vrunner_listener.accept() => {
                handle_accept(r, &serve, ports.vrunner, &on_event, &mut open, &mut vrunner_errors).await?;
            }
            _ = stop.changed() => {
                if *stop.borrow() {
                    for (_, w) in &open {
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
async fn handle_accept(
    r: std::io::Result<(TcpStream, std::net::SocketAddr)>,
    serve: &Arc<Serve>,
    port: u16,
    on_event: &impl Fn(Event),
    open: &mut Vec<(JoinHandle<()>, std::net::TcpStream)>,
    errors: &mut u32,
) -> anyhow::Result<()> {
    match r {
        Ok((stream, _)) => {
            *errors = 0;
            open.retain(|(h, _)| !h.is_finished());
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
    Ok(serve.dispatch(port, async_stream).map(|h| (h, watcher)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge;
    use common::kit::Kit;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        let cfg = crate::cfg::Cfg::default();
        let (mcp_port, vrunner_port) = ephemeral_ports();
        let (kit, dir) = fake_kit();
        let (bridge_log, bridge_seen) = recording_sink();
        let serve_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let serve = Arc::new(Serve::new(&cfg, &kit, bridge_log, serve_ports, Arc::new(AtomicBool::new(true))));
        let (stop_tx, stop_rx) = watch::channel(false);
        let (on_event, events) = recording_sink();
        let run_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let task = tokio::spawn(async move { run(serve, run_ports, move |e| on_event(e), stop_rx).await });

        // Ждём, пока оба слушателя поднимутся, вместо фиксированной паузы.
        for _ in 0..50 {
            if events.lock().unwrap().iter().any(|e| matches!(e, Event::Ok(_))) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let said: Vec<Event> = events.lock().unwrap().clone();
        assert!(
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

    #[tokio::test]
    async fn a_busy_port_fails_the_whole_session_and_frees_the_first_listener() {
        let cfg = crate::cfg::Cfg::default();
        let (mcp_port, vrunner_port) = ephemeral_ports();
        let (kit, dir) = fake_kit();
        let serve_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let serve = Arc::new(Serve::new(&cfg, &kit, Arc::new(|_: Event| {}), serve_ports, Arc::new(AtomicBool::new(true))));
        let busy = std::net::TcpListener::bind(("127.0.0.1", vrunner_port)).unwrap();
        let (_stop_tx, stop_rx) = watch::channel(false);

        let run_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let r = run(serve, run_ports, |_: Event| {}, stop_rx).await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains(&vrunner_port.to_string()), "{err}");

        drop(busy);
        let retry = std::net::TcpListener::bind(("127.0.0.1", mcp_port));
        assert!(retry.is_ok(), "первый слушатель должен был освободиться");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stop_closes_accepted_connections() {
        let cfg = crate::cfg::Cfg::default();
        let (mcp_port, vrunner_port) = ephemeral_ports();
        let (kit, dir) = fake_kit();
        let serve_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let serve = Arc::new(Serve::new(&cfg, &kit, Arc::new(|_: Event| {}), serve_ports, Arc::new(AtomicBool::new(true))));
        let (stop_tx, stop_rx) = watch::channel(false);
        let run_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let task = tokio::spawn(async move { run(serve, run_ports, |_: Event| {}, stop_rx).await });

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
        let cfg = crate::cfg::Cfg::default();
        let (mcp_port, vrunner_port) = ephemeral_ports();
        let (kit, dir) = fake_kit();
        let serve_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let serve = Arc::new(Serve::new(&cfg, &kit, Arc::new(|_: Event| {}), serve_ports, Arc::new(AtomicBool::new(true))));
        let (stop_tx, stop_rx) = watch::channel(false);
        let run_ports = Ports { mcp: mcp_port, vrunner: vrunner_port, web: 0 };
        let task = tokio::spawn(async move { run(serve, run_ports, |_: Event| {}, stop_rx).await });

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
        let rules = bridge::Rules::for_mcp(Arc::new(AtomicBool::new(true)));
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
            tokio::spawn(bridge::pipe_jsonrpc(async_stream, cmd, rules, log, move |code, err| {
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
        let before = tokio::time::timeout(std::time::Duration::from_millis(300), client.read(&mut buf)).await;
        if let Ok(Some((code, err))) = tokio::time::timeout(std::time::Duration::from_millis(1), exit_rx.recv()).await {
            panic!("фиктивный потомок не запустился (код {code}): {err}");
        }
        assert!(before.is_err(), "мост реально заблокирован на чтении — соединение не должно было закрыться само");

        watcher.shutdown(Shutdown::Both).unwrap();
        let after = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf)).await;
        assert_eq!(after.unwrap().unwrap(), 0, "shutdown должен был закрыть соединение несмотря на заблокированный мост");
    }
}
