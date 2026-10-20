//! One SSH session into the environment with remote forwards. Login and
//! password of the workstation, the host key pinned in the kit after the first
//! connect. Forwarded
//! channels are handed to whoever registered for the port (bridge.rs); here
//! they are only accepted.

use anyhow::{Context, Result};
use russh::client::{self, ChannelOpenHandle, Handle, Handler, Msg};
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::Channel;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

/// What a forwarded connection looks like to the bridge: the port the server
/// accepted it on, and the channel to speak through.
pub struct Incoming {
    pub port: u16,
    pub channel: Channel<Msg>,
}

#[derive(Debug)]
pub enum TunnelError {
    Auth,
    HostKeyChanged { old: String, new: String },
    Forward(u16),
    Net(String),
    /// Something on this machine: the kit's known_hosts unreadable, a
    /// certificate where a key was expected. Retrying changes nothing.
    Local(String),
}

/// Whatever `russh` or our handler returned, as one of ours. A ready
/// `TunnelError` passes through; anything else is the link.
pub fn classify(e: anyhow::Error) -> TunnelError {
    match e.downcast::<TunnelError>() {
        Ok(t) => t,
        Err(e) => TunnelError::Net(e.to_string()),
    }
}

impl std::fmt::Display for TunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelError::Local(e) => write!(f, "на этой машине: {e}"),
            TunnelError::Auth => write!(f, "сервер не принял логин и пароль рабочего места — проверьте workstation и password в 1c-connect.toml"),
            TunnelError::HostKeyChanged { old, new } => write!(
                f,
                "отпечаток сервера изменился: был {old}, стал {new}. Если среду переустанавливали — удалите known_hosts из комплекта (или 1c-connect uninstall) и подключитесь заново"
            ),
            TunnelError::Forward(p) => write!(
                f,
                "сервер отказал в пробросе порта {p}: у кого-то ещё поднят туннель этого рабочего места, или workstation в 1c-connect.toml не ваш"
            ),
            TunnelError::Net(e) => write!(f, "нет связи с сервером: {e} — проверьте server в 1c-connect.toml и что среда запущена"),
        }
    }
}
impl std::error::Error for TunnelError {}

pub struct ForwardHandler {
    server: String,
    known_hosts: PathBuf,
    incoming: mpsc::UnboundedSender<Incoming>,
}

impl Handler for ForwardHandler {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool> {
        let PublicKeyOrCertificate::PublicKey { key: pk, .. } = key else {
            return Err(TunnelError::Local("сервер предъявил сертификат, а не ключ — это не наша среда".into()).into());
        };
        let line = format!("{} {}", self.server, pk.to_openssh().context("ключ хоста")?);
        let fp = pk.fingerprint(HashAlg::Sha256).to_string();
        let known = std::fs::read_to_string(&self.known_hosts);
        let existing = known.as_ref().ok().cloned().unwrap_or_default();
        match decide_key(known, &self.server, &line) {
            KeyDecision::Accept => Ok(true),
            KeyDecision::Remember => {
                common::kit::write_text(&self.known_hosts, &(existing + &line + "\n"))?;
                Ok(true)
            }
            KeyDecision::Changed { old } => Err(TunnelError::HostKeyChanged { old, new: fp }.into()),
            KeyDecision::Unreadable(e) => Err(TunnelError::Local(format!("не прочитать known_hosts в комплекте: {e}")).into()),
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<()> {
        reply.accept().await;
        let _ = self.incoming.send(Incoming { port: connected_port as u16, channel });
        Ok(())
    }
}

/// What to do with the key the server presented, given what the kit remembers.
#[derive(Debug, PartialEq)]
pub enum KeyDecision {
    /// Matches the remembered one.
    Accept,
    /// Nothing remembered for this server yet: accept and remember (accept-new).
    Remember,
    /// Remembered something else: refuse, the person decides.
    Changed { old: String },
    /// The file exists but cannot be read: refuse — not knowing is not the
    /// same as a first connect, and an unreadable file must not open the door.
    Unreadable(String),
}

pub fn decide_key(known: std::io::Result<String>, server: &str, line: &str) -> KeyDecision {
    match known {
        Ok(text) => match text.lines().find(|l| l.starts_with(&format!("{server} "))) {
            Some(k) if k.trim() == line.trim() => KeyDecision::Accept,
            Some(k) => KeyDecision::Changed { old: k.split_whitespace().nth(2).unwrap_or("?").chars().take(16).collect::<String>() + "…" },
            None => KeyDecision::Remember,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => KeyDecision::Remember,
        Err(e) => KeyDecision::Unreadable(e.to_string()),
    }
}

pub fn retry_delay(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs((2u64 << attempt.min(5)).min(60))
}

/// Сколько отказов именно проброса подряд терпим, прежде чем решить, что
/// рабочее место занято кем-то другим, а не просто ждёт переподключения
/// старой сессии.
const MAX_FORWARD_REFUSALS: u32 = 3;

/// Текст терминальной ошибки после `MAX_FORWARD_REFUSALS` отказов подряд:
/// протокол не говорит, кто держит порт, поэтому называем рабочее место и
/// все правдоподобные причины, а не сервер.
fn workstation_busy_text(workstation: &str) -> String {
    format!(
        "рабочее место {workstation} занято: другой коннектор (в среде или на другой машине) либо старый проброс спящей машины, который сам отпустится за три минуты"
    )
}

/// Ещё один отказ проброса подряд: считает его отдельно от общего счётчика
/// переподключений (`attempt`), чтобы обрыв связи и отказ проброса не делили
/// один лимит. Возвращает новое значение счётчика и, после третьего отказа
/// подряд, терминальный текст — иначе `None` (повторить как раньше).
fn forward_refusal_step(workstation: &str, refusals_so_far: u32) -> (u32, Option<String>) {
    let n = refusals_so_far + 1;
    if n >= MAX_FORWARD_REFUSALS { (n, Some(workstation_busy_text(workstation))) } else { (n, None) }
}

/// What a person has to fix is never retried: a refused password, a changed
/// key, a fault on this machine. The rest depends on history. Before the
/// first session, a refused port is somebody else's tunnel and no link is a
/// wrong address — stop and say so. After a drop, sshd still holds our own
/// forward for a while and the network may blink — retry with backoff.
pub fn is_terminal(e: &TunnelError, had_session: bool) -> bool {
    match e {
        TunnelError::Auth | TunnelError::HostKeyChanged { .. } | TunnelError::Local(_) => true,
        TunnelError::Forward(_) | TunnelError::Net(_) => !had_session,
    }
}

/// Держит туннель, пока его не остановят: переподключается после обрыва,
/// останавливается на ошибке, которую надо чинить человеку. Каждое
/// проброшенное соединение отдаёт `serve`.
pub async fn run(
    cfg: &crate::cfg::Cfg,
    known_hosts: PathBuf,
    on_event: impl Fn(common::Event),
    serve: impl Fn(Incoming) + Send + Sync + 'static,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let serve = Arc::new(serve);
    let ports = cfg.ports();
    let mut attempt = 0u32;
    let mut had_session = false;
    let mut forward_refusals = 0u32;
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        match Tunnel::connect(&cfg.host.server, &cfg.host.workstation, &cfg.host.password, &known_hosts, &[ports.mcp, ports.vrunner, ports.web]).await {
            Ok(mut t) => {
                attempt = 0;
                forward_refusals = 0;
                had_session = true;
                on_event(common::Event::Ok(format!(
                    "туннель поднят: {} → порты {} (mcp-1c), {} (vrunner) и {} (веб-клиент)",
                    cfg.host.server, ports.mcp, ports.vrunner, ports.web
                )));
                loop {
                    tokio::select! {
                        inc = t.incoming.recv() => match inc {
                            Some(inc) => serve(inc),
                            None => break, // the session is gone
                        },
                        _ = stop.changed() => {
                            let _ = t.handle.disconnect(russh::Disconnect::ByApplication, "", "").await;
                            return Ok(());
                        }
                    }
                }
                on_event(common::Event::Warn("связь с сервером оборвалась — переподключаюсь".into()));
            }
            Err(e) => {
                let te = classify(e);
                if is_terminal(&te, had_session) {
                    return Err(te.into());
                }
                // Отказ именно проброса (возможен тут только когда уже была
                // сессия — иначе он терминален выше) считается отдельно от
                // обрыва связи: три раза подряд — и дальше не повторяем,
                // потому что это не старая сессия сервера, а чей-то ещё
                // туннель этого рабочего места.
                if let TunnelError::Forward(_) = te {
                    let (n, give_up) = forward_refusal_step(&cfg.host.workstation, forward_refusals);
                    forward_refusals = n;
                    if let Some(text) = give_up {
                        return Err(anyhow::anyhow!(text));
                    }
                } else {
                    forward_refusals = 0;
                }
                // After a drop the server may still hold our own forward:
                // that is not a neighbour's tunnel, just the old session dying.
                let text = match &te {
                    TunnelError::Forward(p) if had_session => format!("порт {p} ещё держит прошлая сессия"),
                    _ => te.to_string(),
                };
                on_event(common::Event::Warn(format!("{text}; повтор через {} с", retry_delay(attempt).as_secs())));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(retry_delay(attempt)) => {}
            _ = stop.changed() => return Ok(()),
        }
        attempt += 1;
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(retry_delay(0).as_secs(), 2);
        assert_eq!(retry_delay(1).as_secs(), 4);
        assert_eq!(retry_delay(4).as_secs(), 32);
        assert_eq!(retry_delay(10).as_secs(), 60);
    }

    #[test]
    fn what_is_retried_depends_on_whether_a_session_ever_existed() {
        let fwd = TunnelError::Forward(7771);
        let net = TunnelError::Net("reset".into());
        // Before the first session: a refused port is somebody else's tunnel,
        // no link is a wrong address — both are the person's to fix.
        assert!(is_terminal(&fwd, false));
        assert!(is_terminal(&net, false));
        // After a drop: sshd still holds our own forward for a while and the
        // network may blink — both are retried.
        assert!(!is_terminal(&fwd, true));
        assert!(!is_terminal(&net, true));
        // Never retried: a refused password, a changed key, a local fault.
        for e in [TunnelError::Auth, TunnelError::HostKeyChanged { old: "a".into(), new: "b".into() }, TunnelError::Local("known_hosts".into())] {
            assert!(is_terminal(&e, false) && is_terminal(&e, true), "{e}");
        }
    }

    #[test]
    fn auth_error_names_the_workstation_not_a_shared_account() {
        let s = TunnelError::Auth.to_string();
        assert!(s.contains("рабочего места") && !s.contains("учётки dev"), "{s}");
    }

    #[test]
    fn errors_name_the_field_to_fix() {
        assert!(TunnelError::Auth.to_string().contains("password"));
        assert!(TunnelError::Forward(7771).to_string().contains("7771"));
        assert!(TunnelError::Net("x".into()).to_string().contains("server"));
        assert!(TunnelError::Local("known_hosts: нет прав".into()).to_string().contains("known_hosts"));
    }

    #[test]
    fn forwarding_refused_stops_after_three_tries() {
        // Фиктивный проброс, который всегда отказывает: три попытки — и
        // дальше терминальный Fail, текст называет рабочее место.
        let forward_always_refuses = || false;
        let mut refusals = 0u32;
        let mut tries = 0u32;
        let text = loop {
            tries += 1;
            assert!(!forward_always_refuses());
            let (n, give_up) = forward_refusal_step("dev2", refusals);
            refusals = n;
            if let Some(text) = give_up {
                break text;
            }
        };
        assert_eq!(tries, MAX_FORWARD_REFUSALS);
        assert!(text.contains("dev2") && text.contains("занято") && text.contains("три минуты"), "{text}");
    }

    #[test]
    fn a_ready_tunnel_error_passes_through_classification_unwrapped() {
        let e = classify(anyhow::Error::new(TunnelError::Local("x".into())));
        assert!(matches!(e, TunnelError::Local(_)), "{e}");
        let e = classify(anyhow::anyhow!("connection refused"));
        assert!(matches!(e, TunnelError::Net(ref m) if m == "connection refused"), "{e}");
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn unreadable_known_hosts_never_opens_the_door() {
        let line = "s:1 ssh-ed25519 AAAA";
        assert_eq!(decide_key(Err(std::io::Error::from(std::io::ErrorKind::NotFound)), "s:1", line), KeyDecision::Remember);
        assert!(matches!(decide_key(Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)), "s:1", line), KeyDecision::Unreadable(_)));
        assert_eq!(decide_key(Ok("s:1 ssh-ed25519 AAAA\n".into()), "s:1", line), KeyDecision::Accept);
        assert_eq!(decide_key(Ok("other:1 ssh-ed25519 AAAA\n".into()), "s:1", line), KeyDecision::Remember);
        assert!(matches!(decide_key(Ok("s:1 ssh-ed25519 BBBB\n".into()), "s:1", line), KeyDecision::Changed { .. }));
    }
}

pub struct Tunnel {
    pub handle: Handle<ForwardHandler>,
    pub incoming: mpsc::UnboundedReceiver<Incoming>,
}

impl Tunnel {
    /// Connect, authenticate, request every forward. Any failure is one of
    /// `TunnelError`, with the text the person needs.
    pub async fn connect(server: &str, login: &str, password: &str, known_hosts: &Path, forwards: &[u16]) -> Result<Tunnel> {
        // Keepalives every 30 s keep the session alive through hours of silence
        // (a long dump on the other side); no inactivity timeout on purpose —
        // a tunnel that quietly garbage-collects itself is the failure mode
        // this program exists to remove.
        let config = Arc::new(client::Config {
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            inactivity_timeout: None,
            ..Default::default()
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let handler = ForwardHandler { server: server.to_string(), known_hosts: known_hosts.to_path_buf(), incoming: tx };
        let mut handle = tokio::time::timeout(std::time::Duration::from_secs(15), client::connect(config, server, handler))
            .await
            .map_err(|_| TunnelError::Net("тайм-аут подключения".into()))?
            .map_err(classify)?;
        let auth = handle.authenticate_password(login, password).await.map_err(|e| TunnelError::Net(e.to_string()))?;
        if !auth.success() {
            return Err(TunnelError::Auth.into());
        }
        for &p in forwards {
            // A refused forward (port taken by another developer's tunnel, or
            // outside PermitListen) comes back as RequestDenied; anything else
            // is the link.
            match handle.tcpip_forward("localhost", p as u32).await {
                Ok(_) => {}
                Err(russh::Error::RequestDenied) => return Err(TunnelError::Forward(p).into()),
                Err(e) => return Err(TunnelError::Net(e.to_string()).into()),
            }
        }
        Ok(Tunnel { handle, incoming: rx })
    }

    /// Tell the server we are leaving, so it releases the forwards at once.
    /// Dropping the handle closes the socket eventually, and a reconnect in
    /// the meantime finds its port still taken.
    pub async fn close(self) {
        let _ = self.handle.disconnect(russh::Disconnect::ByApplication, "", "").await;
    }
}
