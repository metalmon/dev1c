//! Проверки по фактам: ничего не меняем, каждая строка — что проверено, итог,
//! и при провале — что делать. Отчёт пишется в файл: разработчик присылает
//! файл, а не пересказ.

use crate::ops::Ctx;
use anyhow::Result;
use common::docker;
use common::Runner;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Check {
    pub what: String,
    pub ok: bool,
    /// Информация, а не приговор: «туннель не поднят» — это не провал сервера.
    pub info: bool,
    pub detail: String,
    pub hint: String,
}

impl Check {
    fn pass(what: &str, detail: impl Into<String>) -> Self {
        Self { what: what.into(), ok: true, info: false, detail: detail.into(), hint: String::new() }
    }
    fn fail(what: &str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { what: what.into(), ok: false, info: false, detail: detail.into(), hint: hint.into() }
    }
    fn note(what: &str, detail: impl Into<String>) -> Self {
        Self { what: what.into(), ok: true, info: true, detail: detail.into(), hint: String::new() }
    }
}

/// Сколько прошло и сколько провалено; информационные не считаются провалом.
pub fn summary(checks: &[Check]) -> (usize, usize) {
    let failed = checks.iter().filter(|c| !c.ok).count();
    (checks.len() - failed, failed)
}

/// Текст отчёта: нумерованный список от основания к вершине.
pub fn render(version: &str, cfg_text: &str, checks: &[Check]) -> String {
    let mut s = format!("dev1c-setup {version}\n\n{cfg_text}\n");
    for (i, c) in checks.iter().enumerate() {
        let mark = if c.info { "info" } else if c.ok { " ok " } else { "FAIL" };
        s.push_str(&format!("{:>2}. [{mark}] {}: {}\n", i + 1, c.what, c.detail));
        if !c.hint.is_empty() {
            s.push_str(&format!("          → {}\n", c.hint));
        }
    }
    let (p, f) = summary(checks);
    s.push_str(&format!("\nПройдено {p}, провалено {f}.\n"));
    s
}

/// Параметры для отчёта без ключа.
pub fn cfg_for_report(cfg: &crate::cfg::Cfg) -> String {
    let mut c = cfg.clone();
    if !c.model.key.is_empty() {
        c.model.key = "…".into();
    }
    toml::to_string(&c).unwrap_or_default()
}

/// Настройки Zed отражают выбранную модель. Смотрим только на заготовки,
/// которые относятся к выбранному провайдеру: при локальной модели
/// CUSTOM_* остаются в файле по замыслу (entrypoint их не трогает).
pub fn settings_ok(settings: &str, provider: &str, model_name: &str) -> bool {
    if !settings.contains(&format!("\"provider\": \"{provider}\"")) || settings.contains("DEFAULT_MODEL_PLACEHOLDER") {
        return false;
    }
    if provider == "custom" {
        return !settings.contains("CUSTOM_API_URL_PLACEHOLDER")
            && !settings.contains("CUSTOM_MODEL_PLACEHOLDER")
            && settings.contains(model_name);
    }
    true
}

fn tcp_open(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok()
}

fn ssh_banner(port: u16) -> Option<String> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf[..n]).trim().to_string())
}

/// Gitea поднимается не сразу после процесса: до полуминуты повторяем.
fn http_status(url: &str) -> std::result::Result<u16, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match client.get(url).send() {
            Ok(r) => return Ok(r.status().as_u16()),
            Err(e) if std::time::Instant::now() > deadline => return Err(e.to_string()),
            Err(_) => std::thread::sleep(Duration::from_secs(3)),
        }
    }
}

/// Проверки по порядку: движок → контейнер → порты → службы → настройки → туннели.
pub fn run(ctx: &Ctx) -> Vec<Check> {
    let mut v = Vec::new();
    let name = ctx.cfg.server.name.clone();
    let [rdp, gitea, _model, ssh] = ctx.cfg.ports();

    match docker::version(ctx) {
        Some(ver) => v.push(Check::pass("движок контейнеров", format!("{} {ver}", ctx.engine()))),
        None => {
            v.push(Check::fail(
                "движок контейнеров",
                format!("{} не отвечает", ctx.engine()),
                "запустите Docker Desktop (или службу docker) и повторите",
            ));
            return v;
        }
    }
    match docker::container_state(ctx, &name).as_str() {
        "running" => v.push(Check::pass("контейнер", "работает")),
        "" => {
            v.push(Check::fail("контейнер", "не создан", "dev1c-setup install"));
            return v;
        }
        other => {
            v.push(Check::fail(
                "контейнер",
                format!("в состоянии {other}"),
                format!("{} logs {name}, затем dev1c-setup install", ctx.engine()),
            ));
            return v;
        }
    }
    v.push(if tcp_open(rdp) {
        Check::pass("рабочие места (RDP)", format!("порт {rdp} принимает соединение"))
    } else {
        Check::fail(
            "рабочие места (RDP)",
            format!("порт {rdp} не отвечает"),
            format!("{} logs {name}: xrdp должен слушать 3389", ctx.engine()),
        )
    });
    let url = format!("http://127.0.0.1:{gitea}/");
    v.push(match http_status(&url) {
        Ok(code) if (200..400).contains(&code) => Check::pass("Gitea", format!("{url} отвечает {code}")),
        Ok(code) => Check::fail("Gitea", format!("{url} отвечает {code}"), "Gitea поднимается до минуты; повторите"),
        Err(e) => Check::fail("Gitea", format!("{url}: {e}"), "Gitea поднимается до минуты; повторите, затем логи контейнера"),
    });
    v.push(match ssh_banner(ssh) {
        Some(b) if b.starts_with("SSH-2.0") => Check::pass("SSH для туннеля", format!("порт {ssh}: {b}")),
        Some(b) => Check::fail("SSH для туннеля", format!("порт {ssh} отвечает не SSH: {b}"), "порт занят другой программой?"),
        None => Check::fail(
            "SSH для туннеля",
            format!("порт {ssh} не отвечает"),
            format!("{} logs {name}: sshd должен слушать 2022", ctx.engine()),
        ),
    });
    for s in ["gitea", "xrdp", "sshd"] {
        let ok = docker::ask(ctx, &["exec", &name, "pgrep", "-x", s]).map(|o| o.ok()).unwrap_or(false);
        v.push(if ok {
            Check::pass(&format!("служба {s}"), "работает")
        } else {
            Check::fail(&format!("служба {s}"), "не запущена", format!("{} logs {name}", ctx.engine()))
        });
    }
    let nc = docker::ask(ctx, &["exec", &name, "sh", "-c", "command -v nc"]).map(|o| o.ok()).unwrap_or(false);
    v.push(if nc {
        Check::pass("nc в образе", "есть — Zed доберётся до инструментов 1С через туннель")
    } else {
        Check::fail("nc в образе", "нет", "образ собран без netcat-openbsd: это другой образ")
    });
    let settings = docker::ask(ctx, &["exec", &name, "cat", "/home/dev1/.config/zed/settings.json"])
        .map(|o| o.text)
        .unwrap_or_default();
    let (want_provider, want_model) = if ctx.cfg.model.url.is_empty() {
        ("llama.cpp", String::new())
    } else {
        ("custom", ctx.cfg.model.name.clone())
    };
    v.push(
        if settings_ok(&settings, want_provider, &want_model) {
            Check::pass(
                "модель в настройках Zed",
                format!(
                    "провайдер {want_provider}{}",
                    if want_model.is_empty() { String::new() } else { format!(", модель {want_model}") }
                ),
            )
        } else if settings.is_empty() {
            Check::fail("модель в настройках Zed", "настроек dev1 ещё нет", "entrypoint раскладывает их при старте; повторите через минуту")
        } else {
            Check::fail(
                "модель в настройках Zed",
                "провайдер или модель не подставлены",
                "dev1c-setup change — пересоздаст контейнер с текущим dev1c.toml",
            )
        },
    );
    v.push(if settings.contains("\"7771\"") {
        Check::pass("порты рабочих мест в Zed", "dev1 → 7771/7781")
    } else {
        Check::fail("порты рабочих мест в Zed", "порт 7771 не подставлен", "образ старее этой программы: обновите образ")
    });
    for n in 1..=3u16 {
        let port = 7770 + n;
        let up = docker::ask(ctx, &["exec", &name, "nc", "-z", "localhost", &port.to_string()])
            .map(|o| o.ok())
            .unwrap_or(false);
        v.push(if up {
            Check::note(&format!("туннель dev{n}"), format!("поднят: порт {port} отвечает"))
        } else {
            Check::note(
                &format!("туннель dev{n}"),
                format!("не поднят — это нормально, если разработчик dev{n} сейчас не подключён"),
            )
        });
    }
    v
}

/// Записать отчёт в комплект и вернуть путь.
pub fn report(ctx: &Ctx, checks: &[Check]) -> Result<PathBuf> {
    let p = ctx.kit.path("last-check.txt");
    let text = render(env!("CARGO_PKG_VERSION"), &cfg_for_report(&ctx.cfg), checks);
    let mut f = std::fs::File::create(&p)?;
    f.write_all(text.as_bytes())?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_lines_never_count_as_failures() {
        let checks = vec![
            Check::pass("движок", "docker 28"),
            Check::note("туннель dev1", "не поднят"),
            Check::fail("RDP", "порт 3390 не отвечает", "dev1c-setup status"),
        ];
        assert_eq!(summary(&checks), (2, 1));
        let text = render("0.1.0", "", &checks);
        assert!(text.contains(" 2. [info] туннель dev1: не поднят"), "{text}");
        assert!(text.contains("→ dev1c-setup status"));
        assert!(text.ends_with("Пройдено 2, провалено 1.\n"));
    }

    #[test]
    fn local_model_settings_pass_with_the_remote_placeholders_still_in_place() {
        // The entrypoint substitutes CUSTOM_* only for a remote model; with a
        // local one those placeholders stay by design and must not fail the check.
        let local = r#""provider": "llama.cpp", "model": "Qwen.gguf", "api_url": "CUSTOM_API_URL_PLACEHOLDER", "model": "CUSTOM_MODEL_PLACEHOLDER""#;
        assert!(settings_ok(local, "llama.cpp", ""));
        assert!(!settings_ok(r#""provider": "llama.cpp", "model": "DEFAULT_MODEL_PLACEHOLDER""#, "llama.cpp", ""));
        let remote = r#""provider": "custom", "api_url": "https://m", "model": "Q""#;
        assert!(settings_ok(remote, "custom", "Q"));
        assert!(!settings_ok(r#""provider": "custom", "api_url": "CUSTOM_API_URL_PLACEHOLDER", "model": "Q""#, "custom", "Q"));
        assert!(!settings_ok(remote, "custom", "Other"));
    }

    #[test]
    fn the_key_never_reaches_the_report() {
        let mut c = crate::cfg::Cfg::default();
        c.model.url = "https://m".into();
        c.model.key = "secret-key".into();
        c.model.name = "Q".into();
        let t = cfg_for_report(&c);
        assert!(!t.contains("secret-key"));
        assert!(t.contains("https://m"));
    }
}
