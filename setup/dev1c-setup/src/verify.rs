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

/// Принимает ли порт соединение. Проба соединением, а не чтение журнала:
/// закрытый порт — это ровно то, во что упрётся разработчик.
pub fn tcp_open(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok()
}

/// Приветствие sshd с порта туннеля: по нему видно, что порт держит именно
/// наш sshd, а не чужая программа, занявшая номер.
pub fn ssh_banner(port: u16) -> Option<String> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf[..n]).trim().to_string())
}

/// Настоящая причина, а не «error sending request for url». reqwest держит
/// отказ в соединении, сброс и недоступный прокси в источниках ошибки, и без
/// них строка отчёта не говорит ничего — ровно это и было в отчёте заказчика.
fn cause_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        let t = c.to_string();
        if !s.contains(&t) {
            s.push_str(": ");
            s.push_str(&t);
        }
        src = c.source();
    }
    s
}

/// Один запрос, без ожидания: проверка — это снимок, а дожидаться служб
/// обязан шаг установки («Службы внутри»). Пока ожидание сидело здесь,
/// install всё равно спрашивал Gitea раньше, чем та начинала слушать, и
/// проверка валилась на здоровом стенде.
///
/// `no_proxy` — не придирка: системный прокси на машине администратора
/// перехватывает и 127.0.0.1, и тогда «Gitea не отвечает» сказано о вполне
/// рабочем стенде.
pub fn http_get(url: &str) -> std::result::Result<u16, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .map_err(|e| cause_chain(&e))?;
    client.get(url).send().map(|r| r.status().as_u16()).map_err(|e| cause_chain(&e))
}

/// Что именно не так с репозиториями. Случая три, и ремонт у них разный:
/// порт не принимает соединение (контейнера нет, Gitea не запущена),
/// соединение есть, а ответа нет (это ответил движок контейнеров, Gitea
/// внутри ещё не слушает), и ответ не тот (отвечает сама Gitea, и смотреть
/// надо её журнал). Раньше все три печатались одним «повторите».
pub fn gitea_fact(
    url: &str,
    port: u16,
    port_open: bool,
    answer: std::result::Result<u16, String>,
    engine: &str,
    name: &str,
) -> Check {
    if !port_open {
        return Check::fail(
            "Gitea",
            format!("порт {port} не принимает соединение"),
            format!("{engine} logs {name}: внутри контейнера Gitea должна слушать 3000"),
        );
    }
    match answer {
        Ok(code) if (200..400).contains(&code) => Check::pass("Gitea", format!("{url} отвечает {code}")),
        Ok(code) => Check::fail(
            "Gitea",
            format!("{url} отвечает {code}"),
            format!("это ответила сама Gitea — её журнал: {engine} exec {name} cat /var/lib/gitea/log/start.log"),
        ),
        Err(e) => Check::fail(
            "Gitea",
            format!("порт {port} открыт, а ответа нет: {e}"),
            format!("так отвечает {engine}, пока Gitea внутри контейнера не слушает; дождаться её — дело install"),
        ),
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
    v.push(gitea_fact(&url, gitea, tcp_open(gitea), http_get(&url), &ctx.engine(), &name));
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
            // «Занято 7823 из 8192» без пояснения читается как беда, а это
            // наши же веса модели: карта занята тем, для чего её и берут.
            Some(g) => Check::pass(
                "видеокарта",
                format!(
                    "{}, {} МиБ, занято {} МиБ вместе с весами модели, драйвер {}",
                    g.name, g.total_mb, g.used_mb, g.driver
                ),
            ),
            None => Check::fail("видеокарта", "nvidia-smi не отвечает", "драйвер NVIDIA на этой машине"),
        });
        let url = crate::ops_llama::local_url(ctx);
        // Ключа у нашего сервера нет (решение 26.10.2026): спрашиваем без него.
        let props = llama::props(&url, "");
        v.push(match &props {
            Some(p) => Check::pass("сервер модели", format!("{}: окно {} токенов, слотов {}, модель {}", ctx.cfg.model_url(), p.window, p.slots, p.model)),
            None => no_props_fact(&url),
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
    // Поднятый туннель — новость; не поднятый — обычное дело, разработчик
    // просто не подключён. Три строки «это нормально, если…» занимали шестую
    // часть отчёта и не сообщали ничего.
    let up: Vec<String> = (1..=3u16)
        .filter(|n| {
            docker::ask(ctx, &["exec", &name, "nc", "-z", "localhost", &(7770 + n).to_string()])
                .map(|o| o.ok())
                .unwrap_or(false)
        })
        .map(|n| format!("dev{n}"))
        .collect();
    if !up.is_empty() {
        v.push(Check::note("туннели рабочих мест", format!("подняты: {}", up.join(", "))));
    }
    v
}

/// Сервер модели не ответил. Причина у этого одна: его нет или он не поднялся
/// — ключа у нашего сервера больше не бывает (решение 26.10.2026), а значит и
/// 401 от нечитаемого токена, который прежде выдавали за молчащий сервер.
pub fn no_props_fact(url: &str) -> Check {
    Check::fail(
        "сервер модели",
        format!("{url} не отвечает"),
        "dev1c-setup install; журнал службы — llama.log в каталоге сервера",
    )
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
            format!(
                "окно слота {slot_window}, агентов {a} — по {per_agent} токенов на каждого; нужно не меньше {need} (вход {}, ответ {})",
                llama::MIN_BUDGET, llama::ANSWER_FLOOR
            ),
        )
    } else {
        Check::fail(
            "окно агента",
            format!(
                "окно слота {slot_window}, агентов {a} — по {per_agent} токенов на каждого, а нужно не меньше {need} (вход {}, ответ {})",
                llama::MIN_BUDGET, llama::ANSWER_FLOOR
            ),
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
    fn a_silent_server_is_named_a_silent_server() {
        // Ключа у нашего сервера нет (решение 26.10.2026), значит и 401 от
        // нечитаемого токена больше не бывает: причина у молчания одна.
        let c = no_props_fact("http://172.17.0.1:3392");
        assert!(!c.ok && c.detail.contains("не отвечает") && c.hint.contains("install"), "{c:?}");
        assert!(!c.detail.contains("токен"), "токена больше нет: {c:?}");
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
        // И на трёх: 83 333 больше 81 920 — но уже еле-еле.
        assert!(numbers_fact(250_000, 3).ok);
        // А вот слот, которого не хватает, обязан провалиться и назвать оба
        // числа: что вышло и что нужно.
        let c = numbers_fact(49_152, 3);
        assert!(!c.ok, "{c:?}");
        assert!(c.detail.contains("16384") && c.detail.contains("81920"), "{c:?}");
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
