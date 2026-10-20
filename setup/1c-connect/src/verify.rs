//! Проверки на машине с 1С, от основания к вершине: платформа → комплект →
//! инструменты и проект → база → веб-сервер → публикация и расширение →
//! среда (Gitea) → что мешает загрузке → туннель. Человек читает сверху
//! вниз и останавливается на первом провале.

use crate::ops_host::{extension_version, find_1cv8, mcp_exe, tools_dir, tools_state, Ctx, Gitea};
use common::check::Check;
use common::op::OpState;
use common::{Phase, Runner};

// Имена фактов — часть контракта (см. тесты). Общие — не зависят от режима;
// остальные расставляет `order`/`run` по режиму из `cfg.host.mode`.
const PARAMS: &str = "параметры";
const PLATFORM: &str = "платформа 1С";
const DISPLAY: &str = "экран для конфигуратора";
const KIT: &str = "mcp-1c в комплекте";
const TOOLS: &str = "инструменты комплекта";
const VRUNNER: &str = "vrunner-mcp отвечает";
const PROJECT: &str = "проект";
const BASE: &str = "база";
const WEB: &str = "веб-сервер";
const PUBLICATION: &str = "публикация и расширение";
const GITEA: &str = "Gitea";
const MEDIA: &str = "комплект на носителе";
const CONFIGURATOR: &str = "Конфигуратор";
const TUNNEL: &str = "туннель";
const WORKSTATION_PORTS: &str = "порты рабочего места";
const DEADLINE: &str = "срок действия";
const OPEN_SESSION: &str = "в базе открыт сеанс";

/// Порядок фактов для режима: общие, затем то, что мешает загрузке/приёму
/// (`Конфигуратор`/`в базе открыт сеанс`), носитель, и последний —
/// готовность канала к агенту (`туннель`/`порты рабочего места`).
pub fn order(mode: &str) -> Vec<&'static str> {
    // Срок — раньше всего остального: когда он вышел, ниже смотреть нечего.
    let mut v = vec![DEADLINE, PARAMS, PLATFORM];
    // Экран — сразу за платформой: без него Конфигуратор не стартует вовсе,
    // и провалится всё, что ниже (vrunner, загрузка, расширение). Факт
    // только на Linux: на Windows экран у Конфигуратора есть по устройству
    // системы, а на macOS платформы нет вовсе — и там и там факт был бы
    // красным зря (см. `display_fact`).
    if cfg!(target_os = "linux") {
        v.push(DISPLAY);
    }
    v.extend([KIT, TOOLS, VRUNNER, PROJECT, BASE, WEB, PUBLICATION, GITEA]);
    if mode == "local" {
        v.push(OPEN_SESSION);
        v.push(MEDIA);
        v.push(WORKSTATION_PORTS);
    } else {
        v.push(CONFIGURATOR);
        v.push(MEDIA);
        v.push(TUNNEL);
    }
    v
}

/// Срок зашитой заглушки лицензии (`crate::licence`). Первый факт набора:
/// когда срок вышел, всё, что ниже, уже не имеет значения — коннектор работу
/// не обслуживает, как бы хорошо ни стояли платформа, база и среда.
/// Предупреждение заранее — сведение, а не приговор: работа идёт.
pub fn licence_fact(state: &crate::licence::State) -> Check {
    let until = crate::licence::until_text();
    match state {
        crate::licence::State::Expired => Check::fail(DEADLINE, format!("истёк {until}"), crate::licence::REFUSAL),
        crate::licence::State::Soon { days } => Check::note(DEADLINE, format!("до {until} — осталось дней: {days}")),
        crate::licence::State::Valid => Check::pass(DEADLINE, format!("до {until}")),
    }
}

pub fn gitea_fact(g: &Gitea) -> Check {
    match g {
        Gitea::Down(e) => Check::fail(GITEA, format!("не отвечает: {e}"), "среда не поднята или адрес не тот — dev1c-setup check на сервере"),
        Gitea::NoOrg => Check::fail(GITEA, "отвечает, но организации dev1c в среде нет", "образ среды старый — dev1c-setup install с новым комплектом"),
        Gitea::Ok(v) => Check::pass(GITEA, format!("Gitea {v}, организация dev1c есть")),
    }
}

/// Linux: vanessa-runner поднимает Конфигуратор — оконную программу, и без
/// экрана он не стартует («Unable to initialize GTK+ or connect to the
/// windowing system», код 255 — замер 2026-10-20 в контейнере). Экран даёт
/// коннектор сам: либо его уже дал сеанс (`DISPLAY`), либо `xvfb-run`
/// (так же, как стенд делает это для своего `vrunner-mcp`).
///
/// Три исхода человек должен различать, поэтому `display` — значение из
/// окружения как оно есть, а `alive` — есть ли за ним X-сервер
/// (`ops_host::display_alive`; при `display = None` не смотрим): экран
/// сеанса, свой Xvfb вместо заданного-но-мёртвого, и провал, когда дать
/// экран нечем. Заданный мёртвый `DISPLAY` молча зелёным больше не проходит.
pub fn display_fact(display: Option<&str>, alive: bool, xvfb: Option<&std::path::Path>) -> Check {
    if let (Some(d), true) = (display, alive) {
        return Check::pass(DISPLAY, format!("DISPLAY={d} — Конфигуратор идёт в этот экран сеанса"));
    }
    // Что именно не так — в текст: без значения человек не поймёт, о каком
    // экране речь, а «задан, но мёртв» и «не задан» лечатся по-разному.
    let why = match display {
        Some(d) => format!("DISPLAY={d} задан, но X-сервера за ним нет"),
        None => "DISPLAY не задан".to_string(),
    };
    match xvfb {
        Some(p) => Check::pass(DISPLAY, format!("{why}; экран даёт {} (виртуальный, никто в него не смотрит)", p.display())),
        None => Check::fail(DISPLAY, why, crate::ops_host::SCREEN_HINT),
    }
}

pub fn configurator_fact(n: usize) -> Check {
    if n == 0 {
        Check::pass(CONFIGURATOR, "не запущен")
    } else {
        Check::fail(CONFIGURATOR, format!("запущено процессов 1cv8: {n}"), "закройте Конфигуратор и клиентов 1С перед загрузкой: cf_load и import откажут, пока он открыт")
    }
}

/// Локальный режим: агент стучится в ту же базу, что человек в Конфигураторе.
/// Отказ агенту при открытом сеансе человека — ожидаемо, не провал.
pub fn open_session_fact() -> Check {
    Check::note(
        OPEN_SESSION,
        "если в базе открыт Конфигуратор или клиент 1С, агенту при обращении откажут — это ожидаемо, закройте сеанс, когда агенту нужен доступ",
    )
}

/// Порты моста (777N/778N): свободны либо их держит этот же процесс — оба
/// случая в порядке. Чужой процесс на порту — почти наверняка второй
/// запущенный коннектор.
pub fn workstation_ports_fact(mcp: u16, vrunner: u16, foreign: &[u16]) -> Check {
    if foreign.is_empty() {
        Check::pass(WORKSTATION_PORTS, format!("{mcp} и {vrunner}: свободны либо заняты этим же процессом"))
    } else {
        let list = foreign.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
        Check::fail(
            WORKSTATION_PORTS,
            format!("порт(ы) {list} заняты другим процессом"),
            "похоже, на этой машине уже открыт второй 1c-connect — закройте его и проверьте снова",
        )
    }
}

/// pid из `holder::of_port` (строка вида `"имя (pid N)"`) — сравнить с
/// `std::process::id()`, не разбирая вывод `ss`/`netstat` заново.
fn holder_pid(desc: &str) -> Option<u32> {
    desc.rsplit("pid ").next()?.trim_end_matches(')').parse().ok()
}

pub fn media_fact(kind: Option<String>, root: &str) -> Check {
    match kind {
        Some(k) => Check::note(MEDIA, format!("{root}: {k} — инструменты работают из %LOCALAPPDATA%, комплект можно вынуть после install")),
        None => Check::pass(MEDIA, format!("{root}: обычный диск")),
    }
}

/// База: каталог есть (файловая), либо сервер\база (не проверяется); сетевой
/// путь — предупреждение, не провал.
pub fn base_fact(path: &str, exists: bool) -> Check {
    if path.starts_with("\\\\") {
        return Check::note(BASE, format!("{path} — сетевая база, одна на двоих: cf_load второго затирает работу первого"));
    }
    let server_base = path.contains('\\') && !path.contains(':');
    if server_base || exists {
        Check::pass(BASE, path.to_string())
    } else {
        Check::fail(BASE, format!("каталога {path} нет"), "base_path в 1c-connect.toml")
    }
}

/// Разложены ли инструменты комплекта (`vrunner/`, `git/`, `httpd/`) на
/// машине. Та же функция считает этот факт в `run()` и в `readiness()` —
/// второй, отдельно придуманный набор фактов не заводится.
pub fn tools_fact(kit_root: &std::path::Path, tools: &std::path::Path) -> Check {
    match tools_state(kit_root, tools) {
        OpState::Done(d) => Check::pass(TOOLS, d),
        OpState::Missing => Check::fail(TOOLS, format!("в {} ничего нет", tools.display()), "1c-connect install"),
        OpState::Drifted { from, to } => Check::fail(TOOLS, format!("на машине {from}, в комплекте {to}"), "1c-connect install — обновит копию"),
        OpState::Blocked(d) => Check::fail(TOOLS, d, "скопируйте полный комплект заново"),
    }
}

/// HTTP-порт базы отвечает (обычный TCP-коннект, без логина) — `up` уже
/// посчитан вызывающим (`run`/`readiness`), здесь только оформление факта.
pub fn web_fact(port: u16, up: bool) -> Check {
    if up {
        Check::pass(WEB, format!("порт {port} отвечает"))
    } else {
        Check::fail(WEB, format!("порт {port} не отвечает"), "запустите Apache/IIS, на котором опубликована база")
    }
}

/// Расширение отвечает по настроенному адресу настроенными учётными данными —
/// `ver` уже посчитан (`extension_version`), здесь только оформление факта.
pub fn publication_fact(base_url: &str, ver: Option<String>) -> Check {
    match ver {
        Some(ver) => Check::pass(PUBLICATION, format!("{base_url} → расширение {ver}")),
        None => Check::fail(PUBLICATION, format!("{base_url}/version не отвечает"), "1c-connect install, затем публикация в Конфигураторе (см. install)"),
    }
}

/// Факты, что решают, дойдёт ли вызов агента до базы: разложены ли
/// инструменты комплекта, отвечает ли HTTP-сервис базы по настроенному адресу
/// настроенными учётными данными, стоит ли расширение. Подмножество `run()`,
/// в том же порядке и теми же функциями (`tools_fact`/`web_fact`/
/// `publication_fact`) — второй набор фактов, который мог бы разойтись с
/// полной проверкой, здесь не заводится. Окно зовёт это при включении MCP и
/// при автозапуске с уже включённым переключателем (`gui::run_readiness`),
/// не на таймере и не блокируя сами порты: те подняты раньше, вердикт только
/// подписывает витрину.
pub fn readiness(ctx: &Ctx) -> Vec<Check> {
    let tools = tools_dir();
    let web = ctx.cfg.http_port();
    let web_up = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], web).into(), std::time::Duration::from_secs(3)).is_ok();
    let base_url = ctx.cfg.base_url();
    let ver = extension_version(&base_url, &ctx.cfg.host.db_user, &ctx.cfg.host.db_password);
    vec![tools_fact(&ctx.kit.root, &tools), web_fact(web, web_up), publication_fact(&base_url, ver)]
}

/// Локальный `initialize` + безвредный инструмент: процесс поднимается из
/// копии на машине с cwd = project; туннель не нужен.
fn vrunner_fact(ctx: &Ctx, tools: &std::path::Path) -> Check {
    let t0 = std::time::Instant::now();
    let cmd = crate::bridge::vrunner_command(&ctx.cfg, tools).into_std();
    match crate::local::call(cmd, "cache_show", serde_json::json!({}), std::time::Duration::from_secs(60), &|_| {}) {
        Ok(r) if !r.is_error => Check::pass(VRUNNER, format!("AutumnMCP, {} с до ответа", t0.elapsed().as_secs())),
        Ok(r) => Check::fail(VRUNNER, format!("отвечает ошибкой: {}", r.text), "каталог проекта и инструменты: 1c-connect install"),
        Err(e) => Check::fail(VRUNNER, e, "1c-connect install (host.tools); журнал — setup.log в комплекте"),
    }
}

fn project_fact(ctx: &Ctx, tools: &std::path::Path) -> Check {
    use crate::project::Layout;
    let p = crate::project::Project::from_cfg(&ctx.cfg, tools);
    match crate::project::layout_state(&p) {
        Layout::Ok => Check::pass(PROJECT, format!("{}, ветка {}", p.dir.display(), p.branch)),
        Layout::Missing => Check::fail(PROJECT, format!("{} не разложен", p.dir.display()), "1c-connect install (host.project)"),
        Layout::BaseDir(f) => Check::fail(PROJECT, format!("{} — каталог базы ({f})", p.dir.display()), "project в 1c-connect.toml — другой каталог"),
        Layout::Drifted(d) => Check::fail(PROJECT, d, "1c-connect install — поправит ветку, origin и файлы"),
    }
}

/// Файл параметров есть. Адрес среды не проверяется: в проде он может
/// совпасть с образцом.
pub fn params_fact(file_exists: bool, server: &str) -> Check {
    if file_exists {
        Check::pass(PARAMS, format!("среда {server}"))
    } else {
        Check::fail(PARAMS, "1c-connect.toml нет в комплекте", "1c-connect example > 1c-connect.toml, затем впишите свои значения")
    }
}

pub fn tunnel_fact(state: Option<&str>) -> Check {
    match state {
        Some(s) => Check::pass(TUNNEL, s.to_string()),
        None => Check::note(TUNNEL, "не поднят (поднимается командой connect)"),
    }
}

/// Факты для режима `cfg.host.mode` (`"tunnel"` или, по умолчанию при любом
/// другом значении, тот же `"tunnel"` — валидирован раньше, здесь не
/// перепроверяется). `tunnel_state` имеет смысл только в туннельном режиме:
/// это состояние сессии, а не то, что можно вычислить здесь.
pub fn run(ctx: &Ctx, tunnel_state: Option<&str>) -> Vec<Check> {
    ctx.progress(Phase::Check);
    let local = ctx.cfg.host.mode == "local";
    let mut v = Vec::new();
    let h = &ctx.cfg.host;
    v.push(licence_fact(&crate::licence::state_now()));
    v.push(params_fact(ctx.kit.state_path(crate::cfg::FILE).exists(), &h.server));
    v.push(match find_1cv8() {
        Some(p) => Check::pass(PLATFORM, p.display().to_string()),
        None => Check::fail(PLATFORM, "1cv8.exe не найден", "установите платформу 1С на эту машину"),
    });
    if cfg!(target_os = "linux") {
        let d = crate::ops_host::display_env();
        let xvfb = crate::ops_host::find_xvfb_run();
        v.push(display_fact(d.as_deref(), d.as_deref().is_some_and(crate::ops_host::display_alive), xvfb.as_deref()));
    }
    let exe = mcp_exe(&ctx.kit);
    v.push(if exe.exists() {
        Check::pass(KIT, exe.file_name().unwrap_or_default().to_string_lossy().to_string())
    } else {
        Check::fail(KIT, "файла нет", "комплект неполный — скопируйте заново")
    });
    let tools = tools_dir();
    let tf = tools_fact(&ctx.kit.root, &tools);
    let tools_ok = tf.ok;
    v.push(tf);
    v.push(if tools_ok { vrunner_fact(ctx, &tools) } else { Check::fail(VRUNNER, "инструментов на машине нет", "1c-connect install") });
    v.push(project_fact(ctx, &tools));
    v.push(base_fact(&h.base_path, std::path::Path::new(&h.base_path).exists()));
    let web = ctx.cfg.http_port();
    let web_up = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], web).into(), std::time::Duration::from_secs(3)).is_ok();
    v.push(web_fact(web, web_up));
    let base_url = ctx.cfg.base_url();
    v.push(publication_fact(&base_url, extension_version(&base_url, &h.db_user, &h.db_password)));
    v.push(gitea_fact(&crate::ops_host::gitea_probe(&ctx.cfg.gitea_url())));
    if local {
        v.push(open_session_fact());
    } else {
        v.push(configurator_fact(crate::bridge::configurator_count()));
    }
    let root = ctx.kit.root.display().to_string();
    // Носитель называем так, как его понимает человек: на Windows это буква
    // диска («C:\»), на Linux буквы нет — там показываем путь комплекта
    // целиком, иначе в отчёт попадает огрызок вида «/op».
    let media = if cfg!(windows) { root.get(..3).unwrap_or(&root) } else { root.as_str() };
    v.push(media_fact(crate::ops_host::media_kind(&ctx.kit.root), media));
    if local {
        let p = ctx.cfg.ports();
        let pid = std::process::id();
        let foreign: Vec<u16> =
            [p.mcp, p.vrunner].into_iter().filter(|&port| crate::holder::of_port(port).and_then(|d| holder_pid(&d)).is_some_and(|owner| owner != pid)).collect();
        v.push(workstation_ports_fact(p.mcp, p.vrunner, &foreign));
    } else {
        v.push(tunnel_fact(tunnel_state));
    }
    v
}

/// Параметры для отчёта без паролей.
pub fn cfg_for_report(cfg: &crate::cfg::Cfg) -> String {
    let mut c = cfg.clone();
    c.host.password = "…".into();
    if !c.host.db_password.is_empty() {
        c.host.db_password = "…".into();
    }
    toml::to_string(&c).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(mode: &str) -> Ctx {
        let mut cfg = crate::cfg::Cfg::default();
        cfg.host.mode = mode.to_string();
        // 127.0.0.1:3 → gitea_url порт 1: закрыт локально, провал мгновенный,
        // как в ops_host::gitea_probe_on_a_closed_port_is_down.
        cfg.host.server = "127.0.0.1:3".into();
        let sink: common::Sink = std::sync::Arc::new(|_: common::Event| {});
        Ctx {
            kit: common::kit::Kit::at(std::env::temp_dir().join(format!("verify-test-missing-kit-{}", std::process::id()))),
            cfg,
            dry_run: true,
            sink,
            prompt: Box::new(common::AlwaysDefault),
        }
    }

    #[test]
    fn web_server_fact_comes_before_tunnel_fact() {
        // Order is the contract: a dead web port must not look like a tunnel fault.
        let names = order("tunnel");
        let at = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(at("веб-сервер") < at("публикация и расширение"));
        assert!(at("публикация и расширение") < at("туннель"));
    }

    #[test]
    fn order_ends_with_the_tunnel_or_the_ports_and_puts_the_screen_after_the_platform() {
        // На Linux сразу за платформой стоит «экран для конфигуратора» (на
        // Windows и macOS его нет вовсе), и всё дальше сдвинуто на один факт.
        let d = usize::from(cfg!(target_os = "linux"));
        let tunnel = order("tunnel");
        assert_eq!(tunnel.len(), 14 + d);
        assert_eq!(tunnel[13 + d], "туннель");
        assert_eq!(tunnel[5 + d], "vrunner-mcp отвечает");
        assert_eq!(tunnel[10 + d], "Gitea");
        if d == 1 {
            assert_eq!(tunnel[3], "экран для конфигуратора");
        }
        let local = order("local");
        assert_eq!(local.len(), 14 + d);
        assert_eq!(local[13 + d], "порты рабочего места");
        // Неизвестный режим — то же, что туннельный: валидация где-то ещё.
        assert_eq!(order("something-else"), tunnel);
    }

    #[test]
    fn facts_follow_the_mode() {
        let tunnel = order("tunnel");
        assert!(tunnel.contains(&"туннель") && tunnel.contains(&"Конфигуратор"));
        assert!(!tunnel.contains(&"порты рабочего места") && !tunnel.contains(&"в базе открыт сеанс"));
        let local = order("local");
        assert!(local.contains(&"порты рабочего места") && local.contains(&"в базе открыт сеанс"));
        assert!(!local.contains(&"туннель") && !local.contains(&"Конфигуратор"));
    }

    #[test]
    fn every_fact_has_a_name_in_order() {
        for mode in ["tunnel", "local"] {
            let ctx = test_ctx(mode);
            let checks = run(&ctx, Some("работает с 12:40"));
            let names: Vec<&str> = checks.iter().map(|c| c.what.as_str()).collect();
            assert_eq!(names, order(mode), "mode {mode}");
        }
    }

    #[test]
    fn workstation_ports_fact_passes_when_free_or_ours_fails_on_a_stranger() {
        let c = workstation_ports_fact(7771, 7781, &[]);
        assert!(c.ok && !c.info, "{c:?}");
        let c = workstation_ports_fact(7771, 7781, &[7781]);
        assert!(!c.ok && c.detail.contains("7781") && c.hint.contains("второй"), "{c:?}");
    }

    #[test]
    fn holder_pid_reads_the_trailing_pid_or_nothing() {
        assert_eq!(holder_pid("1c-connect (pid 812)"), Some(812));
        assert_eq!(holder_pid("junk"), None);
    }

    /// В режиме `auto` адрес, который реально слушает мост, — `Cfg::base_url()`
    /// (считает от `http_port`), а не хранимое поле `[host] base_url`: они
    /// нарочно различаются портом и путём, чтобы проверка не прошла тихо по
    /// случайному совпадению. `веб-сервер` и «публикация и расширение» должны
    /// назвать именно вычисленный порт/адрес, а не хранимый.
    #[test]
    fn web_and_publication_facts_use_the_computed_address_in_auto_mode() {
        let mut cfg = crate::cfg::Cfg::default();
        cfg.host.server = "127.0.0.1:3".into();
        cfg.host.publication = "auto".into();
        cfg.host.http_port = Some(18080);
        cfg.host.base_url = "http://localhost:9999/Buh/hs/mcp-1c".into();
        let sink: common::Sink = std::sync::Arc::new(|_: common::Event| {});
        let ctx = Ctx {
            kit: common::kit::Kit::at(std::env::temp_dir().join(format!("verify-test-pub-addr-{}", std::process::id()))),
            cfg,
            dry_run: true,
            sink,
            prompt: Box::new(common::AlwaysDefault),
        };
        let checks = run(&ctx, None);

        let web = checks.iter().find(|c| c.what == WEB).unwrap();
        assert!(web.detail.contains("18080") && !web.detail.contains("9999"), "{web:?}");

        let publication = checks.iter().find(|c| c.what == PUBLICATION).unwrap();
        assert!(publication.detail.contains("18080"), "{publication:?}");
        assert!(!publication.detail.contains("9999") && !publication.detail.contains("Buh"), "{publication:?}");
    }

    #[test]
    fn open_session_fact_is_always_information_not_a_failure() {
        let c = open_session_fact();
        assert!(c.ok && c.info && c.detail.contains("Конфигуратор"), "{c:?}");
    }

    /// Истёкший срок — провал: человек обязан увидеть красное и дату, до
    /// которой срок был. Предупреждение заранее работу не ломает — это
    /// сведение со сроком и числом оставшихся дней.
    #[test]
    fn an_expired_licence_fails_and_a_warning_is_only_information() {
        let until = crate::licence::until_text();

        let dead = licence_fact(&crate::licence::State::Expired);
        assert!(!dead.ok && !dead.info, "{dead:?}");
        assert!(dead.detail.contains(&until), "{dead:?}");

        let soon = licence_fact(&crate::licence::State::Soon { days: 3 });
        assert!(soon.ok && soon.info, "{soon:?}");
        assert!(soon.detail.contains('3') && soon.detail.contains(&until), "{soon:?}");

        let ok = licence_fact(&crate::licence::State::Valid);
        assert!(ok.ok && !ok.info, "{ok:?}");
        assert!(ok.detail.contains(&until), "{ok:?}");
    }

    #[test]
    fn gitea_fact_distinguishes_no_server_from_no_org() {
        let down = gitea_fact(&crate::ops_host::Gitea::Down("timeout".into()));
        assert!(!down.ok && down.hint.contains("dev1c-setup"), "{down:?}");
        let noorg = gitea_fact(&crate::ops_host::Gitea::NoOrg);
        assert!(!noorg.ok && noorg.detail.contains("организации dev1c"), "{}", noorg.detail);
        let ok = gitea_fact(&crate::ops_host::Gitea::Ok("1.22".into()));
        assert!(ok.ok && ok.detail.contains("1.22"));
    }

    #[test]
    fn configurator_and_media_facts_say_what_to_do() {
        let c = configurator_fact(2);
        assert!(!c.ok && c.detail.contains('2') && c.hint.contains("загруз"), "{c:?}");
        assert!(configurator_fact(0).ok);
        let m = media_fact(Some("FAT32".into()), "E:\\");
        assert!(m.ok && m.info && m.detail.contains("FAT32"), "{m:?}");
        assert!(!media_fact(None, "C:\\").info);
    }

    #[test]
    fn a_network_base_is_flagged_as_shared() {
        let c = base_fact("\\\\srv\\bases\\Buh", true);
        assert!(c.ok && c.info && c.detail.contains("одна на двоих"), "{}", c.detail);
        assert!(!base_fact("C:\\Bases\\Buh", false).ok);
        assert!(base_fact("srv\\Buh", false).ok, "a server base is not a directory to look for");
    }

    #[test]
    fn a_missing_parameters_file_fails_first_and_says_what_to_do() {
        // Первый факт набора — «срок действия» (см. `order`), параметры за ним.
        assert_eq!(order("tunnel")[1], "параметры");
        let c = params_fact(false, "192.168.1.10:3393");
        assert!(!c.ok && c.hint.contains("example"), "{c:?}");
        // Адрес образца больше не повод для красного: в проде он может совпасть.
        assert!(params_fact(true, "192.168.1.10:3393").ok);
        assert!(params_fact(true, "10.0.0.5:3393").ok);
    }

    #[test]
    fn web_fact_and_publication_fact_say_what_to_do() {
        let up = web_fact(1818, true);
        assert!(up.ok && up.detail.contains("1818"), "{up:?}");
        let down = web_fact(1818, false);
        assert!(!down.ok && down.hint.contains("Apache"), "{down:?}");
        let ok = publication_fact("http://x", Some("1.2".into()));
        assert!(ok.ok && ok.detail.contains("1.2"), "{ok:?}");
        let fail = publication_fact("http://x", None);
        assert!(!fail.ok && fail.hint.contains("install"), "{fail:?}");
    }

    #[test]
    fn readiness_is_tools_web_and_publication_in_order() {
        // Каталог комплекта в test_ctx пуст (его и нет на диске) — ровно то
        // же, что строит `run()`'s TOOLS-факт, здесь не второй расчёт.
        let ctx = test_ctx("local");
        let checks = readiness(&ctx);
        let names: Vec<&str> = checks.iter().map(|c| c.what.as_str()).collect();
        assert_eq!(names, vec![TOOLS, WEB, PUBLICATION]);
        assert!(!checks[0].ok, "{:?}", checks[0]);
    }

    /// Провал — только когда дать экран нечем. Экран сеанса или `xvfb-run` —
    /// оба случая в порядке, но человек должен видеть, каким способом экран
    /// дан: это ровно то, что отличает прошедший `export` от падения «Unable
    /// to initialize GTK+» (замер 2026-10-20). И три исхода должны
    /// различаться: заданный мёртвый `DISPLAY` зелёным не проходит.
    #[test]
    fn display_fact_fails_only_without_display_and_without_xvfb() {
        let x = std::path::PathBuf::from("/usr/bin/xvfb-run");

        // экран сеанса — свой, обёртки не будет
        let own = display_fact(Some(":0"), true, Some(&x));
        assert!(own.ok && !own.info && own.detail.contains(":0") && own.detail.contains("сеанса"), "{own:?}");

        // DISPLAY нет — экран даёт xvfb-run, и это видно
        let wrapped = display_fact(None, false, Some(&x));
        assert!(wrapped.ok && wrapped.detail.contains("не задан") && wrapped.detail.contains("xvfb-run"), "{wrapped:?}");

        // DISPLAY задан, но мёртв — тоже xvfb-run, и причина названа значением
        let dead = display_fact(Some(":99"), false, Some(&x));
        assert!(dead.ok && dead.detail.contains(":99") && dead.detail.contains("нет") && dead.detail.contains("xvfb-run"), "{dead:?}");

        // дать нечем — единственный провал; оба случая, и пустой, и мёртвый
        for d in [None, Some(":99")] {
            let nothing = display_fact(d, false, None);
            assert!(!nothing.ok && nothing.hint.contains("xvfb") && nothing.hint.contains("xauth"), "{nothing:?}");
        }
    }

    #[test]
    fn the_tunnel_fact_is_information_when_nobody_connected() {
        let c = tunnel_fact(None);
        assert!(c.info && c.ok, "{c:?}");
        let c = tunnel_fact(Some("работает с 12:40"));
        assert!(!c.info && c.ok);
    }
}
