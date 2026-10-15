//! What answers on the other side of a forwarded channel: for 777N, a fresh
//! mcp-1c process on stdio; for 779N, the local web server. Nothing listens
//! on this machine — the channel is the connection.

use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Start `cmd` with piped stdio and let `stream` speak to it: stream → stdin,
/// stdout → stream. When the child exits, the stream is shut down and
/// `on_exit` gets the exit code and the last lines of stderr.
pub async fn pipe_process<S>(stream: S, mut cmd: tokio::process::Command, on_exit: impl FnOnce(i32, String))
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            on_exit(-1, format!("не удалось запустить: {e}"));
            return;
        }
    };
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (mut sr, mut sw) = tokio::io::split(stream);
    let to_child = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut sr, &mut stdin).await;
        let _ = stdin.shutdown().await;
    });
    let from_child = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut stdout, &mut sw).await;
        let _ = sw.shutdown().await;
    });
    let err_tail = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        let lines: Vec<&str> = s.lines().collect();
        lines[lines.len().saturating_sub(5)..].join("\n")
    });
    let status = child.wait().await;
    let _ = from_child.await;
    to_child.abort();
    let tail = err_tail.await.unwrap_or_default();
    on_exit(status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1), tail);
}

/// A plain relay to a local TCP port (the web server behind 779N).
pub async fn pipe_tcp<S>(stream: S, port: u16)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok(mut tcp) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else { return };
    let mut stream = stream;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
}

/// `mcp-1c.exe` as the bridge starts it: the kit's binary, the developer's base.
pub fn mcp_command(cfg: &crate::cfg::Cfg, kit: &common::kit::Kit) -> tokio::process::Command {
    let exe = kit.path(if cfg!(windows) { "mcp-1c-windows-amd64.exe" } else { "mcp-1c-linux-amd64" });
    let mut c = tokio::process::Command::new(exe);
    c.args(["--base", &cfg.host.base_url, "--user", &cfg.host.db_user, "--password", &cfg.host.db_password, "--quiet"]);
    if !cfg.host.dump.is_empty() {
        c.args(["--dump", &cfg.host.dump]);
    }
    c.current_dir(&kit.root);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The test binary itself as the child: `__echo_child` below copies stdin
    /// to stdout when BRIDGE_ECHO is set, or dies with a message when
    /// BRIDGE_DIE is set — a portable `cat` and a portable failure.
    fn child(env: (&str, &str)) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "bridge::tests::__echo_child", "--nocapture", "--quiet"]);
        c.env(env.0, env.1);
        c
    }

    #[test]
    fn __echo_child() {
        if std::env::var("BRIDGE_ECHO").is_ok() {
            let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::stdout());
            std::process::exit(0);
        }
        if let Ok(msg) = std::env::var("BRIDGE_DIE") {
            eprintln!("{msg}");
            std::process::exit(3);
        }
    }

    #[tokio::test]
    async fn process_sees_the_stream_and_answers_through_it() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_process(theirs, child(("BRIDGE_ECHO", "1")), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (mut r, mut w) = tokio::io::split(ours);
        w.write_all(b"{\"jsonrpc\":\"2.0\"}\n").await.unwrap();
        w.shutdown().await.unwrap();
        let mut got = String::new();
        r.read_to_string(&mut got).await.unwrap();
        // The test harness prints its own lines before the child echoes.
        assert!(got.ends_with("{\"jsonrpc\":\"2.0\"}\n"), "{got:?}");
        let (code, _) = rx.await.unwrap();
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn a_child_that_dies_at_once_closes_the_stream_and_reports_stderr() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(pipe_process(theirs, child(("BRIDGE_DIE", "база недоступна")), move |code, err| {
            let _ = tx.send((code, err));
        }));
        let (mut r, _w) = tokio::io::split(ours);
        let mut got = Vec::new();
        r.read_to_end(&mut got).await.unwrap(); // EOF: the stream closed
        let (code, err) = rx.await.unwrap();
        assert_eq!(code, 3);
        assert!(err.contains("база недоступна"), "{err:?}");
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
