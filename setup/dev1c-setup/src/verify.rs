//! Проверки по фактам: ничего не меняем, каждая строка — что проверено, итог,
//! и при провале — что делать. Отчёт пишется в файл: разработчик присылает
//! файл, а не пересказ.

use crate::cfg::Form;
use crate::llama;
use crate::ops::Ctx;
use anyhow::Result;
use common::docker;
use common::Runner;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

pub use common::check::{render, summary, Check};

/// Параметры для отчёта без ключа.
pub fn cfg_for_report(cfg: &crate::cfg::Cfg) -> String {
    let mut c = cfg.clone();
    if !c.model.key.is_empty() {
        c.model.key = "…".into();
    }
    toml::to_string(&c).unwrap_or_default()
}

/// Настройки Zed отражают выбранную модель: провайдер на месте, заготовки
/// адреса и имени подставлены, имя — то, что задано.
pub fn settings_ok(settings: &str, provider: &str, model_name: &str) -> bool {
    settings.contains(&format!("\"provider\": \"{provider}\""))
        && !settings.contains("CUSTOM_API_URL_PLACEHOLDER")
        && !settings.contains("CUSTOM_MODEL_PLACEHOLDER")
        && settings.contains(model_name)
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
    // Локальный сервер модели: карта, ответ сервера, и сходятся ли его окно,
    // бюджет агента в Zed и слоты.
    if ctx.cfg.form() != Form::Remote {
        v.push(match llama::gpu(ctx) {
            Some(g) => Check::pass("видеокарта", format!("{}, {} МиБ, занято {} МиБ, драйвер {}", g.name, g.total_mb, g.used_mb, g.driver)),
            None => Check::fail("видеокарта", "nvidia-smi не отвечает", "драйвер NVIDIA на этой машине"),
        });
        let token = llama::read_token(&ctx.kit, ctx.cfg.form()).unwrap_or_default();
        let url = crate::ops_llama::local_url(ctx);
        let props = llama::props(&url, &token);
        v.push(match &props {
            Some(p) => Check::pass("сервер модели", format!("{}: окно {} токенов, слотов {}, модель {}", ctx.cfg.model_url(), p.window, p.slots, p.model)),
            None => Check::fail("сервер модели", format!("{url} не отвечает"), "dev1c-setup install; журнал службы — llama.log в каталоге сервера"),
        });
        if let Some(p) = props {
            v.push(numbers_fact(p.window, zed_max_tokens(&settings), ctx.cfg.model.agents));
            // Сервер сообщает окно слота (при --kv-unified это весь общий пул
            // сразу): доля, на которую можно рассчитывать при всех занятых
            // слотах, — пул, делённый на слоты, и объявляется рабочим местам
            // именно она.
            let slot = p.window / p.slots.max(1) as u32;
            if let Some(declared) = ctx.kit.state_get("llama.window").and_then(|s| s.parse::<u32>().ok()) {
                v.push(declared_window_fact(declared, slot));
            }
            // Карта общая с vGPU: столько токенов на слот она держит сейчас.
            if let Some(kv) = llama::kv_per_token(ctx) {
                let available = llama::window_that_fits(llama::vram_for_kv_mb(ctx, kv, p.window), kv, p.slots);
                v.push(model_window_fact(slot, available, ctx.cfg.model.fit_ctx));
            }
        }
        // Подкачка — требование к хосту; на Windows её размером не управляем,
        // и факта нет вовсе (ложного «пройдено» тоже).
        if let Some(swap) = llama::swap_total_mb() {
            v.push(swap_fact(swap, llama::limit_mb(ctx)));
        }
    }
    // Для Zed модель всегда провайдер custom: удалённая по имени из toml,
    // локальная — по имени файла.
    let (want_provider, want_model) = ("custom", ctx.cfg.model_alias());
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

/// Окно сервера, бюджет агента в Zed и слоты обязаны сойтись:
/// max_tokens × agents + ответ ≤ окно.
pub fn numbers_fact(window: u32, zed_max_tokens: Option<u32>, agents: u16) -> Check {
    let out = crate::cfg::OUTPUT_TOKENS;
    let Some(mt) = zed_max_tokens else {
        return Check::fail("три числа", "в настройках Zed нет max_tokens", "dev1c-setup install — замер запишет числа в настройки");
    };
    let need = mt as u64 * agents.max(1) as u64 + out as u64;
    if need <= window as u64 {
        Check::pass("три числа", format!("окно {window}, max_tokens {mt} × {agents} + ответ {out} ≤ окна"))
    } else {
        Check::fail(
            "три числа",
            format!("окно {window}, а max_tokens {mt} × {agents} + ответ {out} = {need}"),
            "dev1c-setup install — пересчитает и пересоздаст контейнер",
        )
    }
}

/// Окно, в которое верит Zed, против того, что сервер может дать сейчас.
/// Это та поломка, которая иначе выглядит как «агент поглупел»: Zed верит
/// объявленному окну, сервер режет по своему, и виноватым оказывается
/// качество модели. Оба числа — на слот.
pub fn declared_window_fact(declared: u32, available: u32) -> Check {
    if available >= declared {
        Check::pass("окно, объявленное рабочим местам", format!("объявлено {declared} токенов на слот, сервер даёт {available}"))
    } else {
        Check::fail(
            "окно, объявленное рабочим местам",
            format!("рабочим местам объявлено {declared} токенов на слот, а сервер даёт {available}"),
            "dev1c-setup install — пересчитает числа и пересоздаст контейнер среды",
        )
    }
}

/// Подкачки меньше, чем может попросить сервер модели. Предупреждение, а не
/// отказ: работать будет, но на пределе — поэтому `note`, и `check` не
/// уходит с ненулевым кодом из-за настройки хоста.
pub fn swap_fact(swap_total_mb: u64, limit_mb: u32) -> Check {
    if swap_total_mb >= limit_mb as u64 {
        return Check::pass("файл подкачки", format!("{swap_total_mb} МиБ при пределе контейнера модели {limit_mb} МиБ"));
    }
    let gb = (limit_mb as u64 + 1023) / 1024;
    Check::note(
        "файл подкачки",
        format!(
            "подкачки {swap_total_mb} МиБ, а контейнер модели может попросить {limit_mb} МиБ; увеличьте файл подкачки до {gb} ГБ — иначе переполнение кэша убьёт сервер модели вместе со всеми сеансами"
        ),
    )
}

/// Карта общая с vGPU: окно, замеренное при настройке, на следующий день
/// может больше не помещаться. «Было столько, сейчас карта даёт столько» —
/// всё, что нужно сказать администратору. Оба числа — токены на слот
/// (`llama::window_that_fits` переводит в них свободные МиБ карты).
pub fn model_window_fact(measured: u32, available: u32, fit_ctx: u32) -> Check {
    // Сравниваем с measured — окном, которое сервер реально загрузил, а не
    // с fit_ctx (настроенной целью): карта могла ещё удовлетворять fit_ctx и
    // при этом уже не помещать то окно, которое сервер фактически держит.
    if available >= measured {
        Check::pass("карта и окно модели", format!("было {measured}, сейчас карта даёт {available} токенов"))
    } else {
        Check::fail(
            "карта и окно модели",
            format!("было {measured}, а сейчас карта даёт только {available} токенов (настроенная цель — {fit_ctx})"),
            "видеокарта занята другой нагрузкой (vGPU) — проверьте её загрузку и повторите позже",
        )
    }
}

/// `max_tokens` из settings.json рабочего места; плейсхолдер или его
/// отсутствие — None.
fn zed_max_tokens(settings: &str) -> Option<u32> {
    settings
        .lines()
        .find_map(|l| l.trim().strip_prefix("\"max_tokens\":"))
        .and_then(|v| v.trim().trim_end_matches(',').parse().ok())
}

/// Записать отчёт в комплект и вернуть путь.
pub fn report(ctx: &Ctx, checks: &[Check]) -> Result<PathBuf> {
    let p = ctx.kit.path("last-check.txt");
    let text = render("dev1c-setup", env!("CARGO_PKG_VERSION"), &cfg_for_report(&ctx.cfg), checks);
    let mut f = std::fs::File::create(&p)?;
    f.write_all(text.as_bytes())?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_numbers_must_agree() {
        let c = numbers_fact(131072, Some(120832), 1);
        assert!(c.ok, "{c:?}");
        let c = numbers_fact(131072, Some(130000), 1);
        assert!(!c.ok && c.detail.contains("130000") && c.detail.contains("131072"), "{c:?}");
        let c = numbers_fact(131072, None, 1);
        assert!(!c.ok && c.hint.contains("install"), "{c:?}");
        // На три агента бюджет × 3 + ответ не должен превышать окно.
        assert!(!numbers_fact(131072, Some(60000), 3).ok);
        assert_eq!(zed_max_tokens("  \"max_tokens\": 120832,\n  \"max_output_tokens\": 8192,"), Some(120832));
        assert_eq!(zed_max_tokens("\"max_tokens\": CUSTOM_MAX_TOKENS_PLACEHOLDER,"), None);
    }

    #[test]
    fn a_window_zed_believes_but_the_server_cannot_give_is_a_failed_fact() {
        let c = declared_window_fact(250_000, 180_000);
        assert!(!c.ok);
        assert!(c.detail.contains("250") && c.detail.contains("180"));
        assert!(declared_window_fact(180_000, 180_000).ok);
    }

    #[test]
    fn too_little_swap_is_a_warning_with_both_numbers() {
        let c = swap_fact(2048, 22528);
        assert!(c.ok && c.info, "это предупреждение, а не отказ");
        assert!(c.detail.contains("2") && c.detail.contains("22"));
        let good = swap_fact(32768, 22528);
        assert!(good.ok && !good.info);
    }

    #[test]
    fn the_pilot_numbers_pass_both_counts_at_once() {
        // Стенд: по 250k на слот, три слота — значит пул 750k, и именно его
        // сообщает /props (llama.cpp: slot_n_ctx, а при --kv-unified
        // n_ctx_seq == n_ctx — весь пул). Zed получает долю одного слота.
        let pool = 250_000 * llama::SLOTS as u32;
        for agents in 1..=3u16 {
            let z = llama::zed_numbers(pool / llama::SLOTS as u32, agents);
            let c = numbers_fact(pool, Some(z.max_tokens), agents);
            assert!(c.ok, "agents={agents}: {c:?}");
            assert!(declared_window_fact(z.window, pool / llama::SLOTS as u32).ok);
        }
        assert_eq!(llama::zed_numbers(250_000, 1).max_tokens, 300_000);
    }

    #[test]
    fn settings_pass_only_with_the_custom_provider_filled_in() {
        // No model configured: the placeholders stay, and that is a failed fact.
        let unset = r#""provider": "custom", "api_url": "CUSTOM_API_URL_PLACEHOLDER", "model": "CUSTOM_MODEL_PLACEHOLDER""#;
        assert!(!settings_ok(unset, "custom", "x"));
        let remote = r#""provider": "custom", "api_url": "https://m", "model": "Q""#;
        assert!(settings_ok(remote, "custom", "Q"));
        assert!(!settings_ok(r#""provider": "custom", "api_url": "CUSTOM_API_URL_PLACEHOLDER", "model": "Q""#, "custom", "Q"));
        assert!(!settings_ok(remote, "custom", "Other"));
    }

    #[test]
    fn a_card_that_shrank_is_a_failed_fact_with_both_numbers() {
        let c = model_window_fact(65536, 20000, 65536);
        assert!(!c.ok);
        assert!(c.detail.contains("65536") && c.detail.contains("20000"));
        assert!(model_window_fact(65536, 70000, 65536).ok);
        // fit_ctx — цель из настроек, не повод закрыть глаза на то, что окно,
        // которое сервер реально загрузил (measured), уже не помещается.
        assert!(!model_window_fact(65536, 50000, 40000).ok);
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
