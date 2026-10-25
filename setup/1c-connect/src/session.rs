//! Один сеанс подключения: поток со своим tokio, туннель с переподключением
//! и мост на проброшенные порты. Общая часть команды `connect` и окна: обе
//! зовут `start`, держат `Session` и в конце — `stop`.

use crate::cfg::{Cfg, Ports};
use crate::{bridge, tunnel};
use common::kit::Kit;
use common::{Event, Sink};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{oneshot, watch};

/// Параметры с номером поколения: `set` меняет оба одним захватом мьютекса,
/// так что `get` видит либо целиком старое, либо целиком новое — никогда
/// смесь полей одной конфигурации с полями другой. `get`/`set` только
/// клонируют и сразу отпускают мьютекс — лока через `await` здесь нет и
/// быть не должно (см. `bridge::pipe_jsonrpc`, главный читатель).
#[derive(Clone)]
pub struct Live(Arc<Mutex<(u64, Cfg)>>);

impl Live {
    pub fn new(cfg: Cfg) -> Live {
        Live(Arc::new(Mutex::new((0, cfg))))
    }

    /// Поколение и снимок параметров одним захватом — пара всегда согласована.
    pub fn get(&self) -> (u64, Cfg) {
        let g = self.0.lock().unwrap();
        (g.0, g.1.clone())
    }

    pub fn set(&self, cfg: Cfg) {
        let mut g = self.0.lock().unwrap();
        g.0 = g.0.wrapping_add(1);
        g.1 = cfg;
    }
}

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

    /// Слова по `cfg.host.mode`: в туннельном — как раньше, в локальном —
    /// про MCP (слушатели держат порты сами, переподключаться там не от
    /// чего — см. `start`), а не про сеанс связи со средой.
    pub fn label(&self, mode: &str) -> String {
        let local = mode == "local";
        match self {
            State::Off => if local { "MCP выключены".into() } else { "не подключён".into() },
            State::Connecting => if local { "включаю…".into() } else { "подключаюсь…".into() },
            State::Up { since } => {
                let m = since.elapsed().as_secs() / 60;
                let verb = if local { "MCP включены" } else { "подключён" };
                if m < 60 {
                    format!("{verb} {m} мин")
                } else {
                    format!("{verb} {} ч {:02} мин", m / 60, m % 60)
                }
            }
            State::Retrying(why) => format!("переподключаюсь: {why}"),
            State::Failed(why) => if local { why.clone() } else { format!("не подключён: {why}") },
        }
    }
}

/// Живой сеанс. `stop` опускает туннель и ждёт поток; просто уронить
/// значение — то же самое, но без ожидания.
pub struct Session {
    stop: watch::Sender<bool>,
    done: Option<oneshot::Receiver<anyhow::Result<()>>>,
    /// Та же ячейка, что держит и обслуживающий `Serve` (см. `Serve::dispatch`):
    /// «Сохранить» в локальном режиме (`gui::save_host`) меняет параметры
    /// через неё, не поднимая новый сеанс — слушатели и уже принятые
    /// соединения это не трогает, следующий же вызов агента просто уходит в
    /// новую базу (спека «Смена проекта/базы… незаметна для Zed»). Мост сам
    /// перечитывает её на каждый запрос и, если ребёнок уже запущен не на
    /// тех параметрах, перезапускает его — см. `bridge::pipe_jsonrpc`.
    cfg: Live,
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

    /// Подменить параметры, которые мост перечитывает на каждый запрос —
    /// без перезапуска сеанса. Уже идущий у ребёнка вызов доигрывает со
    /// старыми; следующий застанет мост свободным и пересоздаст ребёнка на
    /// новых (см. `bridge::pipe_jsonrpc`). В туннельном режиме не зовётся
    /// (там порты живут внутри SSH и сеанс всё равно перезапускают целиком),
    /// но и там это не повредит.
    pub fn update_cfg(&self, cfg: Cfg) {
        self.cfg.set(cfg);
    }
}

/// Как `start_with_enabled`, но с MCP всегда включённым — для CLI
/// (`main.rs`: `connect`/`serve`), у которого нет окна и некому держать
/// переключатель дольше одного сеанса.
pub fn start(cfg: Cfg, kit: Kit, log: Sink, on_state: impl Fn(State) + Send + Sync + 'static) -> Session {
    start_with_enabled(cfg, kit, log, Arc::new(AtomicBool::new(true)), on_state)
}

/// Поднять сеанс в своём потоке. `log` получает все события (туннеля и
/// моста), `on_state` — каждый переход состояния. `enabled` — переключатель
/// «обслуживаются ли вызовы агента»: заводит и держит его окно (`gui::Ui`),
/// он живёт дольше любого одного сеанса (переживает «Сохранить» и
/// переключатель в локальном режиме), сюда только передаётся, а не
/// создаётся заново на каждый перезапуск. Транспорт — по `cfg.host.mode`:
/// `tunnel` держит SSH с переподключением, `local` слушает оба порта на
/// петле сам (база внутри образа — переподключаться там не от чего,
/// `Retrying` в этом режиме не бывает).
pub fn start_with_enabled(cfg: Cfg, kit: Kit, log: Sink, enabled: Arc<AtomicBool>, on_state: impl Fn(State) + Send + Sync + 'static) -> Session {
    let (stop_tx, stop_rx) = watch::channel(false);
    let (done_tx, done_rx) = oneshot::channel();
    // Общая ячейка параметров: заведена здесь (не внутри `Serve::new`), чтобы
    // и поток сеанса, и вызывающий (через `Session::update_cfg`) держали один
    // и тот же `Live` — иначе подмена снаружи не дошла бы до того `Serve`,
    // что реально обслуживает соединения.
    let shared_cfg = Live::new(cfg.clone());
    let session_cfg = shared_cfg.clone();
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
            let serve = Arc::new(Serve::shared(shared_cfg, &cfg, &kit, log.clone(), cfg.ports(), enabled));
            let on_event = |e: Event| {
                log(e.clone());
                let mut s = state.lock().unwrap();
                *s = s.clone().next(&e);
                on_state(s.clone());
            };
            let r = if cfg.host.mode == "local" {
                crate::listen::run(serve, cfg.ports(), on_event, stop_rx).await
            } else {
                let dispatch = serve.clone();
                let on_incoming = move |inc: tunnel::Incoming| {
                    dispatch.dispatch(inc.port, inc.channel.into_stream());
                };
                tunnel::run(&cfg, kit.state_path("known_hosts"), on_event, on_incoming, stop_rx).await
            };
            on_state(State::ended(&r));
            r
        });
        // Задачи моста (и дети под ними) живут на этом `rt` — уронить его
        // здесь и ждёт их завершения раньше, чем окно узнает «не подключён»;
        // иначе человек видит разрыв, а `oscript` под ним ещё жив и держит
        // единственный сеанс учебной лицензии в образе.
        drop(rt);
        let _ = done_tx.send(r);
    });
    Session { stop: stop_tx, done: Some(done_rx), cfg: session_cfg }
}

/// Что отвечает на соединение, принятое на порт: на mcp-1c — мост
/// `pipe_jsonrpc` с процессом mcp-1c на stdio, на vrunner — тот же мост с
/// процессом `vrunner-mcp`, на порту веб-клиента — локальный веб-сервер.
/// Один разбор «порт → мост» на оба транспорта: туннель отдаёт сюда канал
/// russh как поток (`into_stream`), локальный слушатель — `TcpStream`; оба —
/// через один и тот же `dispatch`.
pub struct Serve {
    /// Общая ячейка: «Сохранить» (`Session::update_cfg`) подменяет её на
    /// лету, и мост (`bridge::pipe_jsonrpc`) перечитывает её на каждый
    /// запрос — не только при приёме соединения (см. заголовок сессии).
    cfg: Live,
    kit: Kit,
    log: Sink,
    ports: Ports,
    web_port: u16,
    tools_dir: std::path::PathBuf,
    enabled: Arc<AtomicBool>,
}

impl Serve {
    /// `ports` приходит от вызывающего (а не считается заново из `cfg`
    /// внутри), чтобы тесты могли завести слушатели на эфемерных портах и
    /// `Serve` узнавал об этом напрямую, не трогая 777x/778x. Параметры здесь
    /// неподменяемы (своя ячейка на один раз) — для этого у вызывающего,
    /// которому нужна подмена на ходу, есть `shared` ниже.
    pub fn new(cfg: &Cfg, kit: &Kit, log: Sink, ports: Ports, enabled: Arc<AtomicBool>) -> Serve {
        Serve::shared(Live::new(cfg.clone()), cfg, kit, log, ports, enabled)
    }

    /// Как `new`, но с уже заведённой снаружи общей ячейкой `cfg` — чтобы
    /// `start_with_enabled` мог отдать тот же `Live` и в `Session`
    /// (`update_cfg` подменяет её, не трогая `Serve`). `cfg_now` — разовое
    /// чтение того, что не должно меняться на ходу одного сеанса (порт
    /// веб-клиента): смена режима/портов и так перезапускает сеанс целиком.
    fn shared(cfg: Live, cfg_now: &Cfg, kit: &Kit, log: Sink, ports: Ports, enabled: Arc<AtomicBool>) -> Serve {
        // Инструменты берутся из комплекта по месту: в профиль ничего не
        // переносится (леджер комплекта, пункт 15).
        let tools_dir = kit.root.clone();
        Serve { cfg, kit: kit.clone(), log, ports, web_port: cfg_now.web_port(), tools_dir, enabled }
    }

    /// Разобрать одно принятое соединение: порт решает, какой процесс за ним
    /// стоит. Неизвестный порт — `None`, ничего не запускается и `stream`
    /// просто закрывается, когда функция вернётся. Строка подключения,
    /// пользователь базы и каталог проекта мост берёт не отсюда (однократно
    /// на соединение), а на каждый запрос внутри себя (`rules.live`) — Zed
    /// держит это соединение весь сеанс и сам не переподключается, так что
    /// однократного чтения здесь было бы недостаточно (см. `bridge::pipe_jsonrpc`).
    pub fn dispatch<S>(&self, port: u16, stream: S) -> Option<tokio::task::JoinHandle<()>>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        if port == self.ports.vrunner {
            let (_, cfg0) = self.cfg.get();
            let tools_dir = self.tools_dir.clone();
            let cmd0 = bridge::vrunner_command(&cfg0, &tools_dir);
            let rebuild: Arc<dyn Fn(&Cfg) -> tokio::process::Command + Send + Sync> = {
                let tools_dir = tools_dir.clone();
                Arc::new(move |cfg: &Cfg| bridge::vrunner_command(cfg, &tools_dir))
            };
            let rules = bridge::Rules::for_vrunner(self.cfg.clone(), tools_dir, rebuild, Arc::new(bridge::configurator_count), self.enabled.clone());
            let log_journal = self.log.clone();
            let log_exit = self.log.clone();
            Some(tokio::spawn(bridge::pipe_jsonrpc(stream, cmd0, rules, log_journal, move |code, err| {
                if code != 0 {
                    log_exit(Event::Warn(format!("vrunner-mcp завершился с кодом {code}: {err}")));
                }
            })))
        } else if port == self.ports.mcp {
            let (_, cfg0) = self.cfg.get();
            let kit = self.kit.clone();
            let cmd0 = bridge::mcp_command(&cfg0, &kit);
            let mut rules = bridge::Rules::for_mcp(self.enabled.clone());
            rules.live = self.cfg.clone();
            rules.rebuild = Some(Arc::new(move |cfg: &Cfg| bridge::mcp_command(cfg, &kit)));
            let log_journal = self.log.clone();
            let log_exit = self.log.clone();
            Some(tokio::spawn(bridge::pipe_jsonrpc(stream, cmd0, rules, log_journal, move |code, err| {
                if code != 0 {
                    log_exit(Event::Warn(format!("mcp-1c завершился с кодом {code}: {err}")));
                }
            })))
        } else if port == self.ports.web {
            let log = self.log.clone();
            let web_port = self.web_port;
            Some(tokio::spawn(async move {
                if let Err(e) = bridge::pipe_tcp(stream, web_port).await {
                    log(Event::Warn(e));
                }
            }))
        } else {
            None
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
        assert_eq!(State::Up { since: Instant::now() }.label("tunnel"), "подключён 0 мин");
        assert!(State::Retrying("нет связи".into()).label("tunnel").contains("нет связи"));
        assert_eq!(State::Off.label("tunnel"), "не подключён");
    }

    #[test]
    fn local_mode_labels_speak_about_mcp() {
        assert_eq!(State::Off.label("local"), "MCP выключены");
        assert_eq!(State::Connecting.label("local"), "включаю…");
        assert_eq!(State::Up { since: Instant::now() }.label("local"), "MCP включены 0 мин");
        // Отказ — сама причина, без слова «подключён»: в локальном режиме
        // связи со средой нет, есть только слушатели на петле.
        assert_eq!(State::Failed("порт занят".into()).label("local"), "порт занят");
    }

    /// Главная опасность 6B: вызов, получивший строку подключения одной базы
    /// и каталог проекта другой. `Live::get` обязана отдавать поколение и
    /// `Cfg` одним захватом мьютекса — никогда вперемешку, сколько бы
    /// `set` ни звал параллельный поток.
    #[test]
    fn live_cfg_never_hands_out_a_mix_of_two_configurations() {
        let mut a = Cfg::default();
        a.host.base_path = "C:\\Bases\\A".into();
        a.host.db_user = "UserA".into();
        let mut b = Cfg::default();
        b.host.base_path = "C:\\Bases\\B".into();
        b.host.db_user = "UserB".into();
        let live = Live::new(a.clone());
        let writer_live = live.clone();
        let (wa, wb) = (a.clone(), b.clone());
        let writer = std::thread::spawn(move || {
            for _ in 0..2000 {
                writer_live.set(wb.clone());
                writer_live.set(wa.clone());
            }
        });
        for _ in 0..20_000 {
            let (_, cfg) = live.get();
            let whole_a = cfg.host.base_path == a.host.base_path && cfg.host.db_user == a.host.db_user;
            let whole_b = cfg.host.base_path == b.host.base_path && cfg.host.db_user == b.host.db_user;
            assert!(whole_a || whole_b, "смешались половины двух конфигураций: {:?}", cfg.host);
        }
        writer.join().unwrap();
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
