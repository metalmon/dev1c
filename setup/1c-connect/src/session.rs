//! Один сеанс подключения: поток со своим tokio, туннель с переподключением
//! и мост на проброшенные порты. Общая часть команды `connect` и окна: обе
//! зовут `start`, держат `Session` и в конце — `stop`.

use crate::cfg::Cfg;
use crate::{bridge, tunnel};
use common::kit::Kit;
use common::{Event, Sink};
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::{oneshot, watch};

/// Что с туннелем. Переходы — по событиям `tunnel::run` (и только по ним:
/// предупреждения моста состояние не меняют) и по его завершению.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Off,
    Connecting,
    Up { since: Instant },
    Retrying(String),
    Failed(String),
}

impl State {
    /// Сеанс жив (поток работает), даже если сейчас переподключается.
    pub fn is_on(&self) -> bool {
        !matches!(self, State::Off | State::Failed(_))
    }

    pub fn is_up(&self) -> bool {
        matches!(self, State::Up { .. })
    }

    /// Следующее состояние после события туннеля.
    pub fn next(self, e: &Event) -> State {
        match e {
            Event::Ok(_) => State::Up { since: Instant::now() },
            Event::Warn(why) => State::Retrying(why.clone()),
            Event::Fail(why) => State::Failed(why.clone()),
            _ => self,
        }
    }

    /// Состояние после того, как `tunnel::run` вернулся: остановлен человеком
    /// или упал на том, что чинить ему же.
    pub fn ended(r: &anyhow::Result<()>) -> State {
        match r {
            Ok(()) => State::Off,
            Err(e) => State::Failed(format!("{e:#}")),
        }
    }

    pub fn label(&self) -> String {
        match self {
            State::Off => "не подключён".into(),
            State::Connecting => "подключаюсь…".into(),
            State::Up { since } => {
                let m = since.elapsed().as_secs() / 60;
                if m < 60 {
                    format!("подключён {m} мин")
                } else {
                    format!("подключён {} ч {:02} мин", m / 60, m % 60)
                }
            }
            State::Retrying(why) => format!("переподключаюсь: {why}"),
            State::Failed(why) => format!("не подключён: {why}"),
        }
    }
}

/// Живой сеанс. `stop` опускает туннель и ждёт поток; просто уронить
/// значение — то же самое, но без ожидания.
pub struct Session {
    stop: watch::Sender<bool>,
    done: Option<oneshot::Receiver<anyhow::Result<()>>>,
}

impl Session {
    /// Остановить и дождаться. Не звать изнутри tokio-рантайма.
    pub fn stop(mut self) -> anyhow::Result<()> {
        let _ = self.stop.send(true);
        match self.done.take() {
            Some(rx) => rx.blocking_recv().unwrap_or(Ok(())),
            None => Ok(()),
        }
    }

    /// Дождаться, пока сеанс закончится сам (ошибка, которую не чинят
    /// повтором). Отмена ожидания сеанс не трогает.
    pub async fn wait(&mut self) -> anyhow::Result<()> {
        match self.done.as_mut() {
            Some(rx) => {
                let r = rx.await;
                self.done = None;
                r.unwrap_or(Ok(()))
            }
            None => Ok(()),
        }
    }
}

/// Поднять сеанс в своём потоке. `log` получает все события (туннеля и
/// моста), `on_state` — каждый переход состояния.
pub fn start(cfg: Cfg, kit: Kit, log: Sink, on_state: impl Fn(State) + Send + Sync + 'static) -> Session {
    let (stop_tx, stop_rx) = watch::channel(false);
    let (done_tx, done_rx) = oneshot::channel();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                on_state(State::Failed(e.to_string()));
                let _ = done_tx.send(Err(e.into()));
                return;
            }
        };
        let r = rt.block_on(async {
            on_state(State::Connecting);
            let state = Mutex::new(State::Connecting);
            let serve = serve(&cfg, &kit, log.clone());
            let on_event = |e: Event| {
                log(e.clone());
                let mut s = state.lock().unwrap();
                *s = s.clone().next(&e);
                on_state(s.clone());
            };
            let r = tunnel::run(&cfg, kit.path("known_hosts"), on_event, serve, stop_rx).await;
            on_state(State::ended(&r));
            r
        });
        let _ = done_tx.send(r);
    });
    Session { stop: stop_tx, done: Some(done_rx) }
}

/// Что отвечает на проброшенное соединение: на порту mcp-1c — процесс на
/// stdio, на порту vrunner — мост `pipe_jsonrpc` с процессом `vrunner-mcp`,
/// на порту веб-клиента — локальный веб-сервер.
pub fn serve(cfg: &Cfg, kit: &Kit, log: Sink) -> impl Fn(tunnel::Incoming) + Send + Sync + 'static {
    let cfg = cfg.clone();
    let kit = kit.clone();
    let ports = cfg.ports();
    let web_port = cfg.web_port();
    let tools_dir = crate::ops_host::tools_dir();
    let project = std::sync::Arc::new(crate::project::Project::from_cfg(&cfg, &tools_dir));
    move |inc: tunnel::Incoming| {
        let stream = inc.channel.into_stream();
        if inc.port == ports.vrunner {
            let cmd = bridge::vrunner_command(&cfg, &tools_dir);
            let rules = bridge::Rules {
                allowed: cfg.host.tools.clone(),
                project: project.clone(),
                configurator_running: std::sync::Arc::new(bridge::configurator_count),
                defaults: bridge::connection_args(&cfg),
            };
            let log = log.clone();
            tokio::spawn(bridge::pipe_jsonrpc(stream, cmd, rules, move |code, err| {
                if code != 0 {
                    log(Event::Warn(format!("vrunner-mcp завершился с кодом {code}: {err}")));
                }
            }));
        } else if inc.port == ports.mcp {
            let cmd = bridge::mcp_command(&cfg, &kit);
            let log = log.clone();
            tokio::spawn(bridge::pipe_process(stream, cmd, move |code, err| {
                if code != 0 {
                    log(Event::Warn(format!("mcp-1c завершился с кодом {code}: {err}")));
                }
            }));
        } else if inc.port == ports.web {
            let log = log.clone();
            tokio::spawn(async move {
                if let Err(e) = bridge::pipe_tcp(stream, web_port).await {
                    log(Event::Warn(e));
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tunnel_events_drive_the_state() {
        let s = State::Connecting.next(&Event::Ok("туннель поднят".into()));
        assert!(s.is_up() && s.is_on());
        let s = s.next(&Event::Warn("оборвалась".into()));
        assert_eq!(s, State::Retrying("оборвалась".into()));
        assert!(s.is_on() && !s.is_up());
        // Lines that are not verdicts change nothing.
        assert_eq!(s.clone().next(&Event::Say("x".into())), s);
        assert_eq!(State::ended(&Ok(())), State::Off);
        let f = State::ended(&Err(anyhow::anyhow!("пароль")));
        assert!(matches!(&f, State::Failed(w) if w.contains("пароль")) && !f.is_on());
    }

    #[test]
    fn labels_say_how_long_and_why() {
        assert_eq!(State::Up { since: Instant::now() }.label(), "подключён 0 мин");
        assert!(State::Retrying("нет связи".into()).label().contains("нет связи"));
        assert_eq!(State::Off.label(), "не подключён");
    }

    #[test]
    fn a_session_that_cannot_dial_ends_by_itself_and_stop_is_still_fine() {
        // Nothing listens there: the first attempt is terminal (no session
        // ever existed), so wait() returns the error and stop() after it
        // does not hang.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let mut cfg = Cfg::default();
        cfg.host.server = format!("127.0.0.1:{port}");
        let dir = std::env::temp_dir().join(format!("onec-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let mut s = start(cfg, Kit::at(&dir), std::sync::Arc::new(|_| {}), move |st| seen2.lock().unwrap().push(st));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(s.wait());
        assert!(r.is_err(), "{r:?}");
        assert!(s.stop().is_ok());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.first(), Some(&State::Connecting));
        assert!(matches!(seen.last(), Some(State::Failed(_))), "{seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
