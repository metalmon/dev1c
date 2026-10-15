//! Against a real sshd: the container's. Skipped when DEV1C_SSH is unset so
//! `cargo test` stays green on a laptop without the environment.
use std::path::PathBuf;

fn env() -> Option<(String, String)> {
    Some((std::env::var("DEV1C_SSH").ok()?, std::env::var("DEV1C_SSH_PASSWORD").ok()?))
}

#[tokio::test]
async fn forwards_7771_and_remembers_the_host_key() {
    let Some((server, password)) = env() else {
        eprintln!("skipped: DEV1C_SSH unset");
        return;
    };
    let dir = std::env::temp_dir().join(format!("1c-connect-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let kh: PathBuf = dir.join("known_hosts");
    let t = one_c_connect::tunnel::Tunnel::connect(&server, &password, &kh, &[7771]).await.expect("connect + forward");
    assert!(kh.exists(), "host key remembered on first connect");
    let line = std::fs::read_to_string(&kh).unwrap();
    assert!(line.starts_with(&server), "{line}");
    // Second connect with the same file must succeed (key matches).
    drop(t);
    let t2 = one_c_connect::tunnel::Tunnel::connect(&server, &password, &kh, &[7771]).await.expect("reconnect with pinned key");
    drop(t2);
    // A tampered key must be refused.
    std::fs::write(&kh, format!("{server} ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n")).unwrap();
    let e = one_c_connect::tunnel::Tunnel::connect(&server, &password, &kh, &[7771]).await.err().expect("changed key refused");
    assert!(e.to_string().contains("отпечаток"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn run_keeps_the_forward_open_and_the_container_sees_it() {
    let Some((server, password)) = env() else { return };
    let dir = std::env::temp_dir().join(format!("1c-connect-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = one_c_connect::cfg::Cfg::default();
    cfg.host.server = server;
    cfg.host.password = password;
    let (tx, rx) = tokio::sync::watch::channel(false);
    let kh = dir.join("known_hosts");
    let task = tokio::spawn(async move { one_c_connect::tunnel::run(&cfg, kh, |_| {}, |_| {}, rx).await });
    if std::env::var("DEV1C_DOCKER").is_ok() {
        // The real proof: from inside the container the workstation port
        // answers. Connect + auth + forward take a few seconds on a slow
        // runner, so poll rather than sleep a fixed time.
        let mut open = false;
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let st = std::process::Command::new("docker").args(["exec", "dev1c", "nc", "-z", "localhost", "7771"]).status().unwrap();
            if st.success() {
                open = true;
                break;
            }
        }
        assert!(open, "the container must reach the forwarded port within 40 s");
    } else {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), task).await.expect("run stops on request").unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
