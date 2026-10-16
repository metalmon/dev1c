//! Сервер модели: что ему передать, что он ответил, что из этого следует
//! для Zed. Здесь только чистые функции — их и проверяют тесты; обёртки над
//! командами и HTTP — ниже, в разделе «обёртки».

use serde::Deserialize;

pub const OUTPUT_TOKENS: u32 = crate::cfg::OUTPUT_TOKENS;
/// Запас на приблизительный счёт токенов у Zed.
pub const MARGIN: u32 = 2048;
/// Меньше этого на агента — не кодинг на 1С (модуль в 100 КБ ≈ 40 000 токенов).
pub const MIN_BUDGET: u32 = 16384;

pub struct ServeInput<'a> {
    pub model: &'a str,
    pub alias: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub fit_ctx: u32,
    pub fit_target: u32,
    pub cache_ram: u32,
    /// Токен путём к файлу (служба хоста) …
    pub token_file: Option<&'a str>,
    /// … или переменной LLAMA_API_KEY (контейнер): тогда в строке его нет.
    pub token_env: bool,
}

/// Командная строка по спеке volt-llama. Окно НЕ задаём: подбор уменьшит его
/// под карту, а заданное окно он уменьшить не может и выносит слои на
/// процессор — генерация падает с 32 до 4 токенов в секунду.
pub fn serve_args(i: &ServeInput) -> Vec<String> {
    let port = i.port.to_string();
    let fit_ctx = i.fit_ctx.to_string();
    let fit_target = i.fit_target.to_string();
    let cache_ram = i.cache_ram.to_string();
    let mut a: Vec<String> = [
        "-m", i.model, "--alias", i.alias, "--host", i.host, "--port", &port,
        "--parallel", "3", "--kv-unified", "--fit", "on", "--fit-ctx", &fit_ctx, "--fit-target", &fit_target,
        "--cache-ram", &cache_ram, "--ctx-checkpoints", "32",
        "-b", "2048", "-ub", "512", "--flash-attn", "on", "-ctk", "q8_0", "-ctv", "q8_0",
        "--jinja", "--reasoning-budget", "0", "--temp", "0.7", "--top-k", "40", "--top-p", "0.95", "--min-p", "0.3", "--metrics",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(f) = i.token_file {
        a.push("--api-key-file".into());
        a.push(f.into());
    }
    a
}

/// Кэш подсказок: половина свободного ОЗУ, от 1024 до 8192 МиБ. Ноль ломает
/// слоты (простаивающему слоту некуда переехать), -1 — без предела, и кэш
/// выдавливает всё в подкачку.
pub fn cache_ram_mb(free_ram_mb: u64) -> u32 {
    ((free_ram_mb / 2) as u32).clamp(1024, 8192)
}

/// Карта с рабочим столом занята уже в покое: оставляем ей вдвое больше.
pub fn fit_target_mb(used_mb_idle: u64) -> u32 {
    if used_mb_idle > 300 {
        2048
    } else {
        1024
    }
}

/// Бюджет одного агента в Zed из окна сервера. Окно на агентов делится
/// только если так задано (agents > 1).
pub fn budget(window: u32, agents: u16) -> Option<u32> {
    let per = window / agents.max(1) as u32;
    let b = per.checked_sub(OUTPUT_TOKENS + MARGIN)?;
    (b >= MIN_BUDGET).then_some(b)
}

pub struct Props {
    pub window: u32,
    pub slots: u16,
    pub model: String,
}

#[derive(Deserialize)]
struct PropsJson {
    default_generation_settings: Option<Gen>,
    total_slots: Option<u16>,
    model_path: Option<String>,
}
#[derive(Deserialize)]
struct Gen {
    n_ctx: Option<u32>,
}

pub fn parse_props(json: &str) -> Option<Props> {
    let p: PropsJson = serde_json::from_str(json).ok()?;
    let window = p.default_generation_settings?.n_ctx?;
    let path = p.model_path.unwrap_or_default();
    let model = path.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
    Some(Props { window, slots: p.total_slots.unwrap_or(1), model })
}

/// Скорость разбора из ответа /v1/chat/completions. Меньше 500 посчитанных
/// токенов — это кэш подсказок, а не модель.
pub fn prefill_from(json: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let t = v.get("timings")?;
    if t.get("prompt_n")?.as_u64()? < 500 {
        return None;
    }
    t.get("prompt_per_second")?.as_f64()
}

pub struct Gpu {
    pub name: String,
    pub total_mb: u64,
    pub used_mb: u64,
    pub driver: String,
}

/// `nvidia-smi --query-gpu=name,memory.total,memory.used,driver_version --format=csv,noheader`
pub fn parse_nvidia_smi(csv: &str) -> Option<Gpu> {
    let line = csv.lines().find(|l| l.contains(','))?;
    let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
    if f.len() < 4 {
        return None;
    }
    let mb = |s: &str| s.split_whitespace().next()?.parse::<u64>().ok();
    Some(Gpu { name: f[0].to_string(), total_mb: mb(f[1])?, used_mb: mb(f[2])?, driver: f[3].to_string() })
}

// ------------------------------------------------------------ обёртки

use crate::ops::Ctx;
use anyhow::{Context, Result};
use common::docker;
use common::Runner;
use std::path::PathBuf;

pub const SERVICE: &str = "dev1c-llama";

pub fn dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\dev1c\llama")
    } else {
        PathBuf::from("/opt/dev1c/llama")
    }
}
pub fn server_exe() -> PathBuf {
    dir().join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" })
}
pub fn token_file() -> PathBuf {
    dir().join("dev1c-llama.token")
}
pub fn log_file() -> PathBuf {
    dir().join("llama.log")
}
/// Ряд бинарников под эту ОС в комплекте.
pub fn kit_bin(ctx: &Ctx) -> Option<PathBuf> {
    let p = ctx.kit.path(if cfg!(windows) { "llama/bin/windows-x64-cuda13" } else { "llama/bin/linux-x64-cuda13" });
    p.join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" }).exists().then_some(p)
}
/// nssm есть, откуда взять: в комплекте (llama/nssm.exe) или уже наша копия
/// в закрытом каталоге. Чужой файл на машине не берём.
pub fn nssm_available(ctx: &Ctx) -> bool {
    ctx.kit.path("llama/nssm.exe").exists() || dir().join("nssm.exe").exists()
}

/// nssm, которым регистрируется и управляется служба: только копия в
/// закрытом каталоге (её кладёт шаг llama.bin). Путь к нему запекается в
/// запись службы, и с флешки он быть не должен.
fn nssm_exe() -> Result<String> {
    let n = dir().join("nssm.exe");
    anyhow::ensure!(n.exists(), "нет {}: его туда кладёт шаг llama.bin", n.display());
    Ok(n.to_string_lossy().to_string())
}

/// Каталог службы — во владении администраторов и закрыт: SYSTEM и
/// администраторы, больше никто (Linux: root, 755). Сброс рекурсивный и при
/// каждой установке: иначе подмена llama-server.exe — это выполнение кода от
/// службы, а каталог, созданный кем-то заранее, остался бы его.
pub fn ensure_dir(ctx: &Ctx) -> Result<()> {
    let d = dir();
    std::fs::create_dir_all(&d)?;
    // Перенаправление (junction, символическая ссылка) на месте каталога
    // службы — подмена: закрывать права мы стали бы у чужого каталога.
    for p in [d.parent().map(|p| p.to_path_buf()), Some(d.clone())].into_iter().flatten() {
        if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            anyhow::bail!("{} — ссылка, а не каталог: удалите её и повторите", p.display());
        }
    }
    if cfg!(windows) {
        let mut paths = vec![d.to_string_lossy().to_string()];
        if let Some(parent) = d.parent() {
            paths.insert(0, parent.to_string_lossy().to_string());
        }
        for path in &paths {
            let o = docker::exec(ctx, "takeown", &["/F", path, "/A", "/R", "/D", "Y"])?;
            anyhow::ensure!(o.ok(), "не забрать {path} во владение администраторов: {}", o.tail(2).join(" "));
            let o = docker::exec(
                ctx,
                "icacls",
                &[path, "/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/T", "/Q"],
            )?;
            anyhow::ensure!(o.ok(), "не закрыть права на {path}: {}", o.tail(2).join(" "));
        }
    } else {
        let path = d.to_string_lossy().to_string();
        let o = docker::exec(ctx, "chown", &["-R", "root:root", &path])?;
        anyhow::ensure!(o.ok(), "не сменить владельца {path}: {}", o.tail(2).join(" "));
        let o = docker::exec(ctx, "chmod", &["755", &path])?;
        anyhow::ensure!(o.ok(), "не закрыть права на {path}: {}", o.tail(2).join(" "));
    }
    Ok(())
}

/// Файл с секретом: создаётся сразу с закрытыми правами, не после записи.
fn write_secret(path: &std::path::Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path).with_context(|| format!("не создать {}", path.display()))?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

/// 128 бит системной случайности без лишней зависимости: RandomState сеется
/// из ОС при каждом создании.
pub fn random_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|_| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u8(0);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Токен, если он уже выписан. Живёт только в своём файле.
pub fn read_token() -> Option<String> {
    let t = std::fs::read_to_string(token_file()).ok()?;
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Токен службы: файл в закрытом каталоге (права наследуются от него —
/// SYSTEM и администраторы, Linux: 0600), в команду попадает путь.
/// Каталог закрывается заново и до чтения существующего файла: так
/// старый токен тоже оказывается под замком.
pub fn ensure_token(ctx: &Ctx) -> Result<String> {
    ensure_dir(ctx)?;
    if let Some(t) = read_token() {
        return Ok(t);
    }
    let token = random_hex();
    write_secret(&token_file(), &token)?;
    Ok(token)
}

pub fn gpu(ctx: &Ctx) -> Option<Gpu> {
    let o = docker::probe(ctx, "nvidia-smi", &["--query-gpu=name,memory.total,memory.used,driver_version", "--format=csv,noheader"]).ok()?;
    if !o.ok() {
        return None;
    }
    parse_nvidia_smi(&o.text)
}

pub fn free_ram_mb(ctx: &Ctx) -> u64 {
    if cfg!(windows) {
        docker::probe(ctx, "powershell", &["-NoProfile", "-Command", "(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory"])
            .ok()
            .and_then(|o| o.text.trim().lines().next()?.trim().parse::<u64>().ok())
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    } else {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|t| t.lines().find(|l| l.starts_with("MemAvailable:"))?.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    }
}

/// Где слушать: на Windows петля (Docker Desktop доставляет host.docker.internal
/// до неё — замерено), на Linux адрес моста — виден контейнерам и машине, не сети.
pub fn bind_host(ctx: &Ctx) -> String {
    if cfg!(windows) {
        return "127.0.0.1".into();
    }
    docker::probe(ctx, &ctx.engine(), &["network", "inspect", "bridge", "-f", "{{(index .IPAM.Config 0).Gateway}}"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.text.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "172.17.0.1".into())
}

pub fn port_busy(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

pub struct Installed {
    pub present: bool,
    pub running: bool,
}

pub fn installed(ctx: &Ctx) -> Installed {
    if cfg!(windows) {
        match docker::probe(ctx, "sc", &["query", SERVICE]) {
            Ok(o) if o.ok() => Installed { present: true, running: o.text.contains("RUNNING") },
            _ => Installed { present: false, running: false },
        }
    } else {
        let present = docker::probe(ctx, "systemctl", &["cat", SERVICE]).map(|o| o.ok()).unwrap_or(false);
        let running = present && docker::probe(ctx, "systemctl", &["is-active", SERVICE]).map(|o| o.text.trim() == "active").unwrap_or(false);
        Installed { present, running }
    }
}

pub fn service_install(ctx: &Ctx, args: &[String]) -> Result<()> {
    let exe = server_exe().to_string_lossy().to_string();
    if cfg!(windows) {
        let nssm = nssm_exe()?;
        let mut cmd: Vec<&str> = vec!["install", SERVICE, &exe];
        cmd.extend(args.iter().map(|a| a.as_str()));
        let o = docker::exec(ctx, &nssm, &cmd)?;
        anyhow::ensure!(o.ok(), "служба не зарегистрирована: {}", o.tail(3).join(" "));
        let d = dir().to_string_lossy().to_string();
        let log = log_file().to_string_lossy().to_string();
        // Каждая настройка отдельной командой: nssm их так и принимает.
        for pair in [
            vec!["set", SERVICE, "AppDirectory", &d],
            vec!["set", SERVICE, "AppStdout", &log],
            vec!["set", SERVICE, "AppStderr", &log],
            vec!["set", SERVICE, "AppRotateFiles", "1"],
            vec!["set", SERVICE, "AppRotateBytes", "10485760"],
            // Упавший сервер поднимается сам: иначе агенты молчат до прихода администратора.
            vec!["set", SERVICE, "AppExit", "Default", "Restart"],
            vec!["set", SERVICE, "Start", "SERVICE_AUTO_START"],
            vec!["set", SERVICE, "DisplayName", "dev1c — сервер модели"],
        ] {
            let o = docker::exec(ctx, &nssm, &pair)?;
            anyhow::ensure!(o.ok(), "не настроить службу: {}", o.tail(2).join(" "));
        }
    } else {
        let line = args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ");
        let unit = format!(
            "[Unit]\nDescription=dev1c — сервер модели\nAfter=network-online.target docker.service\n\n[Service]\nType=simple\nExecStart={exe} {line}\nRestart=on-failure\nRestartSec=5\nWorkingDirectory={}\n\n[Install]\nWantedBy=multi-user.target\n",
            dir().display()
        );
        common::kit::write_text(std::path::Path::new(&format!("/etc/systemd/system/{SERVICE}.service")), &unit)?;
        let o = docker::exec(ctx, "systemctl", &["daemon-reload"])?;
        anyhow::ensure!(o.ok(), "systemd не перечитал юниты: {}", o.tail(2).join(" "));
        let o = docker::exec(ctx, "systemctl", &["enable", SERVICE])?;
        anyhow::ensure!(o.ok(), "служба не включена: {}", o.tail(2).join(" "));
    }
    Ok(())
}

fn service_cmd(ctx: &Ctx, verb: &str) -> Result<docker::Out> {
    if cfg!(windows) {
        let nssm = nssm_exe()?;
        docker::exec(ctx, &nssm, &[verb, SERVICE])
    } else {
        docker::exec(ctx, "systemctl", &[verb, SERVICE])
    }
}
pub fn service_start(ctx: &Ctx) -> Result<()> {
    let o = service_cmd(ctx, "start")?;
    anyhow::ensure!(o.ok(), "служба не запустилась: {}", o.tail(3).join(" "));
    Ok(())
}
pub fn service_remove(ctx: &Ctx) -> Result<()> {
    let _ = service_cmd(ctx, "stop");
    if cfg!(windows) {
        if let Ok(n) = nssm_exe() {
            let _ = docker::exec(ctx, &n, &["remove", SERVICE, "confirm"]);
        }
    } else {
        let _ = docker::exec(ctx, "systemctl", &["disable", SERVICE]);
        let _ = std::fs::remove_file(format!("/etc/systemd/system/{SERVICE}.service"));
        let _ = docker::exec(ctx, "systemctl", &["daemon-reload"]);
    }
    Ok(())
}

fn client(secs: u64) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(secs)).build().expect("reqwest client")
}

/// Модель читается с диска: на гигабайтах это минуты.
pub fn wait_health(url: &str, token: &str, secs: u64) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let c = client(10);
    loop {
        let last = match c.get(format!("{url}/health")).bearer_auth(token).send() {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => format!("ответ {}", r.status().as_u16()),
            Err(e) => e.to_string(),
        };
        anyhow::ensure!(std::time::Instant::now() < deadline, "сервер модели не готов за {secs} с: {last}");
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

pub fn props(url: &str, token: &str) -> Option<Props> {
    let r = client(10).get(format!("{url}/props")).bearer_auth(token).send().ok()?;
    parse_props(&r.text().ok()?)
}

/// Один длинный запрос с меткой в начале (иначе сервер отдаст скорость кэша,
/// а не модели) и запретом кэша подсказок.
pub fn measure_prefill(url: &str, token: &str) -> Option<f64> {
    let filler = format!("{} {}", random_hex(), "Регламент перевозки опасных грузов определяет порядок погрузки, крепления и сопровождения. ".repeat(110));
    let body = serde_json::json!({ "messages": [{"role": "user", "content": filler}], "max_tokens": 1, "stream": false, "cache_prompt": false });
    let r = client(600)
        .post(format!("{url}/v1/chat/completions"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .ok()?;
    prefill_from(&r.text().ok()?)
}

/// Числа замера — в состоянии комплекта; токен туда не пишется, он
/// читается из своего файла (комплект лежит на флешке с обычными правами).
pub fn facts_save(ctx: &Ctx, f: &crate::cfg::Facts, prefill: Option<f64>) -> Result<()> {
    ctx.kit.state_set("llama.window", &f.window.to_string())?;
    ctx.kit.state_set("llama.max_tokens", &f.max_tokens.to_string())?;
    ctx.kit.state_set("llama.prefill", &prefill.map(|p| format!("{p:.0}")).unwrap_or_default())
}

pub fn facts_load(ctx: &Ctx) -> Option<crate::cfg::Facts> {
    Some(crate::cfg::Facts {
        token: read_token()?,
        window: ctx.kit.state_get("llama.window")?.parse().ok()?,
        max_tokens: ctx.kit.state_get("llama.max_tokens")?.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(token_file: Option<&'a str>) -> ServeInput<'a> {
        ServeInput { model: "/m/x.gguf", alias: "x", host: "127.0.0.1", port: 1235, fit_ctx: 65536, fit_target: 1024, cache_ram: 4096, token_file, token_env: false }
    }

    #[test]
    fn the_window_is_left_to_the_fit_and_the_token_goes_by_path() {
        let a = serve_args(&input(Some("/k/dev1c-llama.token")));
        assert!(!a.iter().any(|x| x == "-c" || x == "--ctx-size"), "окно не задаём: {a:?}");
        let at = |k: &str| a.iter().position(|x| x == k).map(|i| a[i + 1].clone());
        assert_eq!(at("--fit-ctx").as_deref(), Some("65536"));
        assert_eq!(at("--fit-target").as_deref(), Some("1024"));
        assert_eq!(at("--parallel").as_deref(), Some("3"));
        assert!(a.iter().any(|x| x == "--kv-unified"));
        assert_eq!(at("--cache-ram").as_deref(), Some("4096"));
        assert_eq!(at("--api-key-file").as_deref(), Some("/k/dev1c-llama.token"));
        assert_eq!(at("--host").as_deref(), Some("127.0.0.1"));
        assert_eq!(at("--alias").as_deref(), Some("x"));
        assert!(!a.iter().any(|x| x == "--api-key"), "значение токена в строке недопустимо");
    }

    #[test]
    fn the_container_form_takes_the_token_from_the_environment() {
        let mut i = input(None);
        i.token_env = true;
        let a = serve_args(&i);
        assert!(!a.iter().any(|x| x == "--api-key-file"));
    }

    #[test]
    fn cache_ram_is_half_of_free_memory_within_bounds() {
        assert_eq!(cache_ram_mb(32000), 8192);
        assert_eq!(cache_ram_mb(6000), 3000);
        assert_eq!(cache_ram_mb(500), 1024);
    }

    #[test]
    fn a_desktop_on_the_card_doubles_the_fit_target() {
        assert_eq!(fit_target_mb(0), 1024);
        assert_eq!(fit_target_mb(200), 1024);
        assert_eq!(fit_target_mb(1250), 2048);
    }

    #[test]
    fn the_budget_subtracts_the_answer_and_the_margin_and_refuses_small_windows() {
        assert_eq!(budget(131072, 1), Some(131072 - 8192 - 2048));
        assert_eq!(budget(131072, 3), Some(131072 / 3 - 8192 - 2048));
        assert_eq!(budget(16384, 1), None, "меньше 16384 на агента — не бюджет");
    }

    #[test]
    fn props_and_timings_are_read_from_the_server_json() {
        let p = parse_props(r#"{"default_generation_settings":{"n_ctx":131072},"total_slots":3,"model_path":"/m/x.gguf"}"#).unwrap();
        assert_eq!((p.window, p.slots, p.model.as_str()), (131072, 3, "x.gguf"));
        assert!(parse_props("{}").is_none());
        assert_eq!(prefill_from(r#"{"timings":{"prompt_n":1800,"prompt_per_second":2600.5}}"#), Some(2600.5));
        assert_eq!(prefill_from(r#"{"timings":{"prompt_n":4,"prompt_per_second":9000.0}}"#), None, "горстка токенов — это кэш, не замер");
    }

    #[test]
    fn facts_round_trip_through_the_kit_state_and_the_token_is_random_hex() {
        let dir = std::env::temp_dir().join(format!("dev1c-llama-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir_for_test = dir.clone();
        let kit = common::kit::Kit::at(&dir);
        let ctx = crate::ops::Ctx { kit, cfg: crate::cfg::Cfg::default(), dry_run: false, sink: std::sync::Arc::new(|_| {}), prompt: Box::new(common::AlwaysDefault) };
        assert!(ctx.kit.state_get("llama.window").is_none());
        let f = crate::cfg::Facts { token: "abc".into(), window: 131072, max_tokens: 120832 };
        facts_save(&ctx, &f, Some(2600.0)).unwrap();
        // Токен в состоянии комплекта не хранится: он только в своём файле.
        assert!(ctx.kit.state_get("llama.token").is_none());
        assert_eq!(ctx.kit.state_get("llama.window").as_deref(), Some("131072"));
        assert_eq!(ctx.kit.state_get("llama.max_tokens").as_deref(), Some("120832"));
        if let Some(got) = facts_load(&ctx) {
            assert_eq!((got.window, got.max_tokens), (131072, 120832));
        }
        let secret = dir_for_test.join("s.token");
        write_secret(&secret, "x").unwrap();
        assert!(write_secret(&secret, "y").is_err(), "существующий файл секрета не перевыписывается");
        let t1 = random_hex();
        let t2 = random_hex();
        assert_eq!(t1.len(), 32);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()) && t1 != t2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nvidia_smi_csv_is_parsed_and_absence_is_none() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 3060 Ti, 8192 MiB, 1250 MiB, 610.88\n").unwrap();
        assert_eq!((g.name.as_str(), g.total_mb, g.used_mb, g.driver.as_str()), ("NVIDIA GeForce RTX 3060 Ti", 8192, 1250, "610.88"));
        assert!(parse_nvidia_smi("NVIDIA-SMI has failed").is_none());
    }
}
