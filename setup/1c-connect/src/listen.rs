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
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

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
    let mut open: Vec<std::net::TcpStream> = Vec::new();
    loop {
        tokio::select! {
            r = mcp_listener.accept() => {
                if let Ok((stream, _)) = r {
                    if let Ok(watcher) = accept_for(&serve, ports.mcp, stream) {
                        open.push(watcher);
                    }
                }
            }
            r = vrunner_listener.accept() => {
                if let Ok((stream, _)) = r {
                    if let Ok(watcher) = accept_for(&serve, ports.vrunner, stream) {
                        open.push(watcher);
                    }
                }
            }
            _ = stop.changed() => {
                for w in &open {
                    let _ = w.shutdown(Shutdown::Both);
                }
                return Ok(());
            }
        }
    }
}

/// Раздваивает дескриптор принятого соединения перед тем, как отдать поток
/// мосту: вторая копия остаётся здесь на случай `stop` и уже не зависит от
/// того, что мост с первой копией делает (держит в мосте, читает построчно,
/// уже уронил). `shutdown` на любой копии закрывает общий сокет целиком.
fn accept_for(serve: &Serve, port: u16, stream: TcpStream) -> std::io::Result<std::net::TcpStream> {
    let std_stream = stream.into_std()?;
    let watcher = std_stream.try_clone()?;
    let async_stream = TcpStream::from_std(std_stream)?;
    serve.dispatch(port, async_stream);
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::kit::Kit;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;

    /// Три теста этого модуля делят всего два запасных рабочих места
    /// (dev2/dev3) — без замка параллельный запуск мог бы столкнуть два
    /// теста на одной паре портов.
    static PORT_LOCK: Mutex<()> = Mutex::new(());

    /// Порт свободен прямо сейчас: бинд успел и тут же отпущен. Короткое
    /// окно гонки терпимо — так же делают существующие тесты session.rs.
    fn free(port: u16) -> bool {
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    }

    /// dev2/dev3: dev1 — реальное рабочее место, там может стоять живой
    /// коннектор; тесты никогда не трогают его порты (7771/7781).
    fn free_ports() -> crate::cfg::Cfg {
        let mut cfg = crate::cfg::Cfg::default();
        for ws in ["dev3", "dev2"] {
            cfg.host.workstation = ws.into();
            let p = cfg.ports();
            if free(p.mcp) && free(p.vrunner) {
                return cfg;
            }
        }
        panic!("ни dev2, ни dev3 не свободны для теста");
    }

    /// `Kit` без настоящего комплекта: бинарей mcp-1c/vrunner в нём нет —
    /// ровно то, что нужно тесту, которому важен только факт, что соединение
    /// дошло до моста, а не живая 1С.
    fn fake_kit() -> (Kit, std::path::PathBuf) {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
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
        let _guard = PORT_LOCK.lock().unwrap();
        let cfg = free_ports();
        let (mcp_port, vrunner_port) = (cfg.ports().mcp, cfg.ports().vrunner);
        let (kit, dir) = fake_kit();
        let (bridge_log, bridge_seen) = recording_sink();
        let serve = Arc::new(Serve::new(&cfg, &kit, bridge_log, Arc::new(AtomicBool::new(true))));
        let (stop_tx, stop_rx) = watch::channel(false);
        let (on_event, events) = recording_sink();
        let task = tokio::spawn(async move { run(serve, cfg.ports(), move |e| on_event(e), stop_rx).await });

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
        let _guard = PORT_LOCK.lock().unwrap();
        let cfg = free_ports();
        let (mcp_port, vrunner_port) = (cfg.ports().mcp, cfg.ports().vrunner);
        let (kit, dir) = fake_kit();
        let serve = Arc::new(Serve::new(&cfg, &kit, Arc::new(|_: Event| {}), Arc::new(AtomicBool::new(true))));
        let busy = std::net::TcpListener::bind(("127.0.0.1", vrunner_port)).unwrap();
        let (_stop_tx, stop_rx) = watch::channel(false);

        let r = run(serve, cfg.ports(), |_: Event| {}, stop_rx).await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains(&vrunner_port.to_string()), "{err}");

        drop(busy);
        let retry = std::net::TcpListener::bind(("127.0.0.1", mcp_port));
        assert!(retry.is_ok(), "первый слушатель должен был освободиться");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stop_closes_accepted_connections() {
        let _guard = PORT_LOCK.lock().unwrap();
        let cfg = free_ports();
        let mcp_port = cfg.ports().mcp;
        let (kit, dir) = fake_kit();
        let serve = Arc::new(Serve::new(&cfg, &kit, Arc::new(|_: Event| {}), Arc::new(AtomicBool::new(true))));
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = tokio::spawn(async move { run(serve, cfg.ports(), |_: Event| {}, stop_rx).await });

        let mut client = loop {
            match TcpStream::connect(("127.0.0.1", mcp_port)).await {
                Ok(s) => break s,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        };

        // Живо: чтение не возвращает ничего за отведённое время.
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
}
