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
    let t = one_c_connect::tunnel::Tunnel::connect(&server, "dev1", &password, &kh, &[7771]).await.expect("connect + forward");
    assert!(kh.exists(), "host key remembered on first connect");
    let line = std::fs::read_to_string(&kh).unwrap();
    assert!(line.starts_with(&server), "{line}");
    // Second connect with the same file must succeed (key matches). Leave
    // properly first: a dropped session keeps its forward on the server for
    // a moment, and the reconnect would find 7771 taken.
    t.close().await;
    let t2 = one_c_connect::tunnel::Tunnel::connect(&server, "dev1", &password, &kh, &[7771]).await.expect("reconnect with pinned key");
    t2.close().await;
    // A tampered key must be refused.
    std::fs::write(&kh, format!("{server} ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n")).unwrap();
    let e = one_c_connect::tunnel::Tunnel::connect(&server, "dev1", &password, &kh, &[7771]).await.err().expect("changed key refused");
    assert!(e.to_string().contains("отпечаток"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

// Important 5 финального ревью: `tunnel::run` на каждом удачном коннекте зовёт
// `session::flip_remote_servers`, а та — `tokio::task::block_in_place` (докстрока
// `flip_remote_servers`: ПАНИКА, не отказ, вне многопоточного рантайма). По умолчанию
// `#[tokio::test]` — однопоточный рантайм, и именно на нём эта ветка сломала тест: панику ловили
// не как «SSH недоступен», а как настоящий крах `cargo test`.
#[tokio::test(flavor = "multi_thread")]
async fn run_keeps_the_forward_open_and_the_container_sees_it() {
    let Some((server, password)) = env() else { return };
    let dir = std::env::temp_dir().join(format!("1c-connect-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = one_c_connect::cfg::Cfg::default();
    cfg.host.server = server;
    // Workstation dev2 → 7772/7792: the other test holds 7771 and the two run
    // in parallel; sharing a port would make one of them a victim of timing.
    // The tunnel logs in as the workstation itself; in the image the password
    // equals the login, so the dev1 password from the environment is not it.
    cfg.host.workstation = "dev2".into();
    cfg.host.password = if std::env::var("DEV1C_DOCKER").is_ok() { "dev2".into() } else { password };
    let (tx, rx) = tokio::sync::watch::channel(false);
    let kh = dir.join("known_hosts");
    let editor_mcp = std::sync::Arc::new(std::sync::Mutex::new(None));
    let task = tokio::spawn(async move { one_c_connect::tunnel::run(&cfg, kh, |_| {}, |_| {}, rx, editor_mcp).await });
    if std::env::var("DEV1C_DOCKER").is_ok() {
        // The real proof: from inside the container the workstation port
        // answers. Connect + auth + forward take a few seconds on a slow
        // runner, so poll rather than sleep a fixed time.
        let mut open = false;
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if task.is_finished() {
                break; // run() gave up: its error is what to report
            }
            let st = std::process::Command::new("docker").args(["exec", "dev1c", "nc", "-z", "localhost", "7772"]).status().unwrap();
            if st.success() {
                open = true;
                break;
            }
        }
        if !open {
            let why = if task.is_finished() { format!("{:?}", task.await) } else { "run() still alive, port never opened".into() };
            panic!("the container must reach the forwarded port within 40 s: {why}");
        }
    } else {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), task).await.expect("run stops on request").unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Задача 7, Important 4 ревью: доказывает именно проводку в `tunnel::run`
/// (`tunnel.rs:221`/`236`), а не только `flip_with`/`flip_remote_servers`
/// сами по себе — убери любой из двух вызовов оттуда, и этот тест упадёт.
/// Второе SSH-соединение (без пробросов — `&[]`, чтобы не спорить с тем,
/// что держит сам `run()`) сеет файл редактора своей командой и читает его
/// независимо, пока `run()` живёт своей жизнью на отдельном соединении.
///
/// Important 5 финального ревью: `flip_remote_servers` зовёт `block_in_place`, который паникует
/// вне многопоточного рантайма — `flavor = "multi_thread"` здесь обязателен, не только у соседнего
/// теста, иначе SSH-отказ и паника рантайма неотличимы в выводе `cargo test`.
#[tokio::test(flavor = "multi_thread")]
async fn run_flips_editor_mcp_enabled_around_the_session() {
    let Some((server, password)) = env() else { return };
    let dir = std::env::temp_dir().join(format!("1c-connect-flip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let password = if std::env::var("DEV1C_DOCKER").is_ok() { "dev3".to_string() } else { password };

    let control = one_c_connect::tunnel::Tunnel::connect(&server, "dev3", &password, &dir.join("known_hosts_control"), &[])
        .await
        .expect("control connect (no forwards)");
    let before = r#"{"context_servers":{"1c":{"command":"nc","enabled":false},"vrunner":{"command":"nc","enabled":false}}}"#;
    control.exec(&format!("mkdir -p ~/.config/zed && cat > ~/.config/zed/settings.json\n{before}")).await.expect("seed the file");

    let mut cfg = one_c_connect::cfg::Cfg::default();
    cfg.host.server = server;
    cfg.host.workstation = "dev3".into();
    cfg.host.password = password;
    let (tx, rx) = tokio::sync::watch::channel(false);
    let editor_mcp = std::sync::Arc::new(std::sync::Mutex::new(None));
    let kh = dir.join("known_hosts");
    let task = tokio::spawn(async move { one_c_connect::tunnel::run(&cfg, kh, |_| {}, |_| {}, rx, editor_mcp).await });

    let mut on = String::new();
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if task.is_finished() {
            panic!("run() завершился раньше, чем успел включить записи: {:?}", task.await);
        }
        on = control.exec("cat ~/.config/zed/settings.json").await.expect("read after connect");
        if on.contains("\"enabled\": true") {
            break;
        }
    }
    assert!(on.contains("\"enabled\": true"), "run() обязан включить записи после удачного проброса: {on}");

    tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), task).await.expect("run stops on request").unwrap().unwrap();
    let off = control.exec("cat ~/.config/zed/settings.json").await.expect("read after stop");
    assert!(off.contains("\"enabled\": false"), "run() обязан выключить записи ДО разрыва: {off}");

    control.close().await;
    let _ = std::fs::remove_dir_all(&dir);
}
