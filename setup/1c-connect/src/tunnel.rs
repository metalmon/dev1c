//! One SSH session into the environment with remote forwards. Password as
//! `dev`, the host key pinned in the kit after the first connect. Forwarded
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
}

impl std::fmt::Display for TunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelError::Auth => write!(f, "сервер не принял пароль учётки dev — проверьте server и password в 1c-connect.toml"),
            TunnelError::HostKeyChanged { old, new } => write!(
                f,
                "отпечаток сервера изменился: был {old}, стал {new}. Если среду переустанавливали — удалите known_hosts из комплекта (или 1c-connect uninstall) и подключитесь заново"
            ),
            TunnelError::Forward(p) => write!(
                f,
                "сервер отказал в пробросе порта {p}: у кого-то ещё поднят туннель этого рабочего места, или workstation в 1c-connect.toml не ваш"
            ),
            TunnelError::Net(e) => write!(f, "нет связи с сервером: {e} — проверьте адрес и порт в 1c-connect.toml и что среда запущена"),
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
            anyhow::bail!("сервер предъявил сертификат, а не ключ")
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
            KeyDecision::Unreadable(e) => Err(TunnelError::Net(format!("не прочитать known_hosts в комплекте: {e}")).into()),
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
    pub async fn connect(server: &str, password: &str, known_hosts: &Path, forwards: &[u16]) -> Result<Tunnel> {
        let config = Arc::new(client::Config {
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            inactivity_timeout: Some(std::time::Duration::from_secs(600)),
            ..Default::default()
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let handler = ForwardHandler { server: server.to_string(), known_hosts: known_hosts.to_path_buf(), incoming: tx };
        let mut handle = tokio::time::timeout(std::time::Duration::from_secs(15), client::connect(config, server, handler))
            .await
            .map_err(|_| TunnelError::Net("тайм-аут подключения".into()))?
            .map_err(|e| match e.downcast_ref::<TunnelError>() {
                Some(TunnelError::HostKeyChanged { old, new }) => TunnelError::HostKeyChanged { old: old.clone(), new: new.clone() },
                _ => TunnelError::Net(e.to_string()),
            })?;
        let auth = handle.authenticate_password("dev", password).await.map_err(|e| TunnelError::Net(e.to_string()))?;
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
}
