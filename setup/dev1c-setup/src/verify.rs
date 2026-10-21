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
    // Заготовка адреса проверяется только у рабочего провайдера: файл несёт
    // оба, и у простаивающего она остаётся по построению.
    let url_placeholder = if provider == "custom" { "CUSTOM_API_URL_PLACEHOLDER" } else { "LLAMA_API_URL_PLACEHOLDER" };
    settings.contains(&format!("\"provider\": \"{provider}\""))
        && !settings.contains(url_placeholder)
        && !settings.contains("MODEL_NAME_PLACEHOLDER")
        && !settings.contains("MODEL_PROVIDER_PLACEHOLDER")
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
        let token_file = llama::token_path(&ctx.kit, ctx.cfg.form());
        let token = llama::read_token(&ctx.kit, ctx.cfg.form());
        // Токен не прочитан, а файл на месте — это права, а не сервер.
        let unreadable = token.is_none() && token_file.exists();
        let url = crate::ops_llama::local_url(ctx);
        let props = llama::props(&url, &token.unwrap_or_default());
        v.push(match &props {
            Some(p) => Check::pass("сервер модели", format!("{}: окно {} токенов, слотов {}, модель {}", ctx.cfg.model_url(), p.window, p.slots, p.model)),
            None => no_props_fact(&url, unreadable.then(|| token_file.display().to_string())),
        });
        if let Some(p) = props {
            v.push(numbers_fact(p.window, ctx.cfg.model.agents));
            // Сервер сообщает окно слота (при --kv-unified это весь общий пул
            // сразу): окно, которое слот честно держит вместе с ответом,
            // отдаёт контекст СЛОТА (slot_n_ctx), а не пул: без унификации
            // кэша это его собственная доля, и делить её не на что.
            let slot = p.window;
            // Объявленного окна в настройках больше нет: штатный провайдер
            // забирает его с /props. Сравнивать есть смысл только с тем, что
            // записал замер: карта могла сжаться после него.
            if let Some(declared) = ctx.kit.state_get("llama.window").and_then(|s| s.parse().ok()) {
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
        // Предел не узнан — сравнивать подкачку не с чем, и выдуманное число
        // здесь означало бы «здоров» против неверного предела.
        if let (Some(swap), Some(limit)) = (llama::swap_total_mb(), llama::limit_mb(ctx)) {
            v.push(swap_fact(swap, limit));
        }
    }
    // Провайдер зависит от формы: наш сервер — штатный `llama.cpp`, чужой
    // OpenAI-совместимый — `custom` (решение владельца 2026-10-21). Имя
    // модели: удалённая по имени из toml, локальная — по имени файла.
    let want_provider = match ctx.cfg.form() {
        crate::cfg::Form::Remote => "custom",
        _ => "llama.cpp",
    };
    let want_model = ctx.cfg.model_alias();
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

/// Сервер модели не ответил — и надо назвать настоящую причину. Пустой токен
/// при существующем файле токена значит, что его не дали прочитать: установка
/// под `sudo` пишет `dev1c-llama.token` как `root:root 0600`, а `check` тем же
/// администратором, но без `sudo`, видит пустой ключ, получает от сервера 401
/// — и обвинял бы в этом совершенно здоровый сервер.
pub fn no_props_fact(url: &str, unreadable_token: Option<String>) -> Check {
    match unreadable_token {
        Some(f) => Check::fail(
            "сервер модели",
            format!("токен {f} есть, но не читается: без него серверу нечем ответить (401), и о самом сервере это не говорит ничего"),
            "install и check выполняет один пользователь: ставили под sudo — и проверяйте `sudo dev1c-setup check`, либо отдайте файл токена тому, кто проверяет",
        ),
        None => Check::fail("сервер модели", format!("{url} не отвечает"), "dev1c-setup install; журнал службы — llama.log в каталоге сервера"),
    }
}

/// Хватает ли окна агенту. Раньше здесь сходились три числа — вход, ответ и
/// запас, — потому что вход мы объявляли Zed сами. Со штатным провайдером
/// `llama.cpp` (решение владельца 2026-10-21) объявлять нечего: окно приходит
/// с `/props`, и редактор делит его сам. Осталось одно сравнение, зато то,
/// которое правда зависит от карты.
///
/// Слоты здесь не множатся: у каждого свой контекст, `/props` отдаёт долю
/// слота, а не пул. Несколько агентов одного человека делят его слот — на них
/// и делим.
pub fn numbers_fact(slot_window: u32, agents: u16) -> Check {
    let a = agents.max(1);
    let per_agent = llama::per_agent(slot_window, a);
    let need = llama::MIN_BUDGET + llama::ANSWER_FLOOR;
    if per_agent >= need {
        Check::pass(
            "окно агента",
            format!("слот {slot_window} на {a} агентов = {per_agent} ≥ {need} (вход {} и ответ {})", llama::MIN_BUDGET, llama::ANSWER_FLOOR),
        )
    } else {
        Check::fail(
            "окно агента",
            format!("слот {slot_window} на {a} агентов даёт {per_agent}, а нужно {need} (вход {} и ответ {})", llama::MIN_BUDGET, llama::ANSWER_FLOOR),
            "dev1c-setup install — пересчитает и пересоздаст контейнер; либо agents в dev1c.toml меньше",
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
    fn an_unreadable_token_is_named_instead_of_blaming_the_server() {
        // Установка под sudo, check без него: токен есть, но не читается.
        // Виноваты права, а не сервер, и «install» тут ничего не исправит.
        let c = no_props_fact("http://172.17.0.1:3392", Some("/srv/dev1c-kit/dev1c-llama.token".into()));
        assert!(!c.ok);
        assert!(c.detail.contains("dev1c-llama.token") && c.detail.contains("не читается"), "{c:?}");
        assert!(c.hint.contains("sudo"), "{c:?}");
        assert!(!c.detail.contains("не отвечает"), "здоровый сервер не виноват: {c:?}");
        // Токена нет вовсе — это по-прежнему «сервер не отвечает».
        let c = no_props_fact("http://172.17.0.1:3392", None);
        assert!(!c.ok && c.detail.contains("не отвечает") && c.hint.contains("install"), "{c:?}");
    }

    #[test]
    fn a_window_zed_believes_but_the_server_cannot_give_is_a_failed_fact() {
        let c = declared_window_fact(250_000, 180_000);
        assert!(!c.ok);
        assert!(c.detail.contains("250") && c.detail.contains("180"));
        assert!(declared_window_fact(180_000, 180_000).ok);
    }

    #[test]
    fn the_agent_window_must_hold_the_input_and_the_answer() {
        // Стенд: слот 250k. На одного агента хватает с запасом.
        assert!(numbers_fact(250_000, 1).ok);
        // И на трёх: 83 333 больше 24 576.
        assert!(numbers_fact(250_000, 3).ok);
        // А вот слот, которого не хватает, обязан провалиться и назвать оба
        // числа: что вышло и что нужно.
        let c = numbers_fact(49_152, 3);
        assert!(!c.ok, "{c:?}");
        assert!(c.detail.contains("16384") && c.detail.contains("24576"), "{c:?}");
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
        // Стенд: сервер отдал 250k на слот (это и есть slot_n_ctx из /props,
        // без унификации кэша — доля слота, а не пул). Наша арифметика из
        // цепочки ушла: что сервер сказал, то и проверяем.
        let slot = 250_000;
        for agents in 1..=3u16 {
            let c = numbers_fact(slot, agents);
            assert!(c.ok, "agents={agents}: {c:?}");
            assert!(declared_window_fact(llama::per_agent(slot, agents), slot / agents as u32).ok);
        }
        // Замер записал окно слота, карта его и даёт — расхождения нет.
        assert!(declared_window_fact(slot, slot).ok);
    }
    #[test]
    fn settings_pass_only_with_the_live_provider_filled_in() {
        // Модель не настроена: заготовки на месте, и это провалившийся факт.
        let unset = r#""provider": "MODEL_PROVIDER_PLACEHOLDER", "api_url": "LLAMA_API_URL_PLACEHOLDER", "model": "MODEL_NAME_PLACEHOLDER""#;
        assert!(!settings_ok(unset, "llama.cpp", "x"));
        // Наш сервер: штатный провайдер, адрес подставлен.
        let local = r#""provider": "llama.cpp", "api_url": "http://host.docker.internal:1235", "model": "Q""#;
        assert!(settings_ok(local, "llama.cpp", "Q"));
        assert!(!settings_ok(local, "llama.cpp", "Other"));
        // Чужой сервер: вторая ветвь, и проверяется её собственная заготовка.
        let remote = r#""provider": "custom", "api_url": "https://m", "model": "Q""#;
        assert!(settings_ok(remote, "custom", "Q"));
        assert!(!settings_ok(r#""provider": "custom", "api_url": "CUSTOM_API_URL_PLACEHOLDER", "model": "Q""#, "custom", "Q"));
        // Заготовка простаивающей ветви рабочему провайдеру не мешает: файл
        // несёт оба, и у неподключённого она остаётся по построению.
        let both = r#""provider": "llama.cpp", "api_url": "http://host:1235", "x": "CUSTOM_API_URL_PLACEHOLDER", "model": "Q""#;
        assert!(settings_ok(both, "llama.cpp", "Q"));
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
