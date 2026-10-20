//! Проверки на машине с 1С, от основания к вершине: платформа → комплект →
//! инструменты и проект → база → веб-сервер → публикация и расширение →
//! среда (Gitea) → что мешает загрузке → туннель. Человек читает сверху
//! вниз и останавливается на первом провале.

use crate::ops_host::{find_1cv8, mcp_exe, tools_dir, tools_state, Ctx, Gitea};
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

/// Запущенный Конфигуратор — не поломка, а «сейчас занято человеком»: по
/// спеке («Исключение одно и названо списком») такие факты — сведения и
/// цвет точки не красят, иначе точка желтела бы у каждого, кто открыл 1С.
/// Совет, что делать перед загрузкой, остаётся при факте: его читают.
pub fn configurator_fact(n: usize) -> Check {
    if n == 0 {
        Check::pass(CONFIGURATOR, "не запущен")
    } else {
        Check {
            ok: true,
            info: true,
            ..Check::fail(
                CONFIGURATOR,
                format!("запущено процессов 1cv8: {n}"),
                "закройте Конфигуратор и клиентов 1С перед загрузкой: cf_load и import откажут, пока он открыт",
            )
        }
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

/// Кто держит порт рабочего места. Своим считается не только этот самый
/// процесс, но и коннектор того же рабочего места: трей держит порты всё
/// время работы — это штатное состояние, и разовой команде (`install`,
/// `check`, `export`) он не помеха, она этих портов не занимает.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// Мы сами — порт занят этим же процессом.
    Ours,
    /// Другой процесс, но тот же исполняемый файл коннектора: трей (или
    /// `serve`) этого рабочего места.
    OurConnector,
    /// Кто-то ещё — вот это и есть провал.
    Stranger,
}

/// Порты моста (777N/778N): свободны, заняты нами самими либо своим же
/// коннектором этого рабочего места — все три случая в порядке. Провал —
/// только чужой процесс на порту. `holders` называет лишь занятые порты;
/// свободные в список не попадают.
pub fn workstation_ports_fact(mcp: u16, vrunner: u16, holders: &[(u16, Holder)]) -> Check {
    let strangers: Vec<String> = holders.iter().filter(|(_, h)| *h == Holder::Stranger).map(|(p, _)| p.to_string()).collect();
    if !strangers.is_empty() {
        return Check::fail(
            WORKSTATION_PORTS,
            format!("порт(ы) {} заняты другим процессом", strangers.join(", ")),
            "похоже, на этой машине уже открыт второй 1c-connect — закройте его и проверьте снова",
        );
    }
    // Способ называем вслух: «занято» без объяснения на зелёном факте
    // человека только тревожит.
    if holders.iter().any(|(_, h)| *h == Holder::OurConnector) {
        Check::pass(WORKSTATION_PORTS, format!("{mcp} и {vrunner}: заняты коннектором этого рабочего места — разовой команде это не мешает"))
    } else {
        Check::pass(WORKSTATION_PORTS, format!("{mcp} и {vrunner}: свободны либо заняты этим же процессом"))
    }
}

/// pid из `holder::of_port` (строка вида `"имя (pid N)"`), не разбирая вывод
/// `ss`/`netstat` заново.
fn holder_pid(desc: &str) -> Option<u32> {
    desc.rsplit("pid ").next()?.trim_end_matches(')').parse().ok()
}

/// Чей это pid: наш собственный, нашего же коннектора или чужой. Признак
/// «наш коннектор» — тот же, которым замок единственного экземпляра отличает
/// свою копию: совпадение образа программы (`single_instance::same_image`).
/// Рабочее место сверять отдельно не нужно — порты в `holders` взяты из
/// конфигурации этого рабочего места, и коннектор с нашим образом на них
/// именно его и обслуживает.
fn holder_of(pid: u32, me: u32, ours: Option<&std::path::Path>) -> Holder {
    if pid == me {
        return Holder::Ours;
    }
    match ours {
        Some(o) if crate::single_instance::same_image(crate::holder::image_of(pid).as_deref(), o) => Holder::OurConnector,
        _ => Holder::Stranger,
    }
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

/// Сроки сбора фактов. «Проверить» и командная строка считают без
/// ограничения по времени — человек ждёт ответа сознательно; готовность
/// считается под рукой у него и молча, поэтому у неё свои короткие сроки и
/// общий бюджет (спека, «у готовности свои сроки»).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Срок пробы инструментов (`vrunner-mcp` поднимает свой процесс, живой
    /// замер — 8–11 с).
    pub tools: std::time::Duration,
    /// Срок одного сетевого запроса (версия расширения). Коннект к порту
    /// веб-сервера и Gitea и так коротки (3 с и 5 с), их не растягиваем.
    pub net: std::time::Duration,
    /// Общий бюджет на весь набор; `None` — без бюджета.
    pub budget: Option<std::time::Duration>,
}

impl Limits {
    /// «Проверить» и `1c-connect check`: сроки прежние, бюджета нет.
    pub fn full() -> Self {
        Self { tools: std::time::Duration::from_secs(60), net: std::time::Duration::from_secs(60), budget: None }
    }

    /// Готовность: короткие сроки и общий бюджет в 20 с.
    pub fn quick() -> Self {
        Self { tools: std::time::Duration::from_secs(15), net: std::time::Duration::from_secs(5), budget: Some(std::time::Duration::from_secs(20)) }
    }
}

/// Факт под общим бюджетом: когда бюджет уже вышел, факт не считается вовсе —
/// вместо выдуманного провала человек читает «не проверено». Точка от этого
/// не зелёная (факт не `ok`), но и вины человека в таком факте нет.
fn within(started: std::time::Instant, budget: Option<std::time::Duration>, name: &'static str, f: impl FnOnce() -> Check) -> Check {
    match budget {
        Some(b) if started.elapsed() >= b => Check::fail(
            name,
            format!("не проверено за {} с", b.as_secs()),
            "готовность считается коротко — нажмите «Проверить», она считает без ограничения по времени",
        ),
        _ => f(),
    }
}

/// Первый провалившийся факт — он и есть причина жёлтого вердикта. Сведения
/// («в базе открыт сеанс», «Конфигуратор запущен», «комплект на носителе»)
/// пропускаются: они говорят «сейчас занято человеком», а не «сломано».
pub fn first_fail(checks: &[Check]) -> Option<Check> {
    checks.iter().find(|c| !c.ok && !c.info).cloned()
}

/// Тот же набор фактов, что печатает `run`, но молча: ни `Phase::Check`, ни
/// одной строки в журнале окна (окно приняло бы фазу за начавшуюся операцию
/// и заперло бы переключатель с «Сохранить»), и по коротким срокам
/// `Limits::quick`. Карточка и «Проверить» обязаны считать одно и то же —
/// второй, отдельно придуманный набор здесь не заводится.
pub fn readiness(ctx: &Ctx, tunnel_state: Option<&str>) -> Vec<Check> {
    facts_for_readiness(ctx, tunnel_state, Limits::quick())
}

/// Локальный `initialize` + безвредный инструмент: процесс поднимается из
/// копии на машине с cwd = project; туннель не нужен. Инструмент нарочно
/// без сеанса «Предприятия» (`cache_show`) — иначе пересчёт готовности
/// отбирал бы единственный сеанс учебной платформы.
fn vrunner_fact(ctx: &Ctx, tools: &std::path::Path, timeout: std::time::Duration) -> Check {
    let t0 = std::time::Instant::now();
    let cmd = crate::bridge::vrunner_command(&ctx.cfg, tools).into_std();
    match crate::local::call(cmd, "cache_show", serde_json::json!({}), timeout, &|_| {}) {
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
    // Фаза — только здесь: по ней окно показывает карточку хода и запирает
    // кнопки. Сбор фактов её не объявляет, иначе молчаливый расчёт
    // готовности выглядел бы начавшейся операцией (спека).
    ctx.progress(Phase::Check);
    facts_for_readiness(ctx, tunnel_state, Limits::full())
}

/// Сам сбор фактов — общий для «Проверить» и для вердикта готовности.
/// Ничего не печатает и не объявляет фазы; всё, что может задержаться,
/// считается под общим бюджетом (`within`) и со своим сроком (`Limits`).
pub fn facts_for_readiness(ctx: &Ctx, tunnel_state: Option<&str>, limits: Limits) -> Vec<Check> {
    let t0 = std::time::Instant::now();
    let budget = limits.budget;
    let local = ctx.cfg.host.mode == "local";
    let mut v = Vec::new();
    let h = &ctx.cfg.host;
    v.push(within(t0, budget, DEADLINE, || licence_fact(&crate::licence::state_now())));
    v.push(within(t0, budget, PARAMS, || params_fact(ctx.kit.state_path(crate::cfg::FILE).exists(), &h.server)));
    v.push(within(t0, budget, PLATFORM, || match find_1cv8() {
        Some(p) => Check::pass(PLATFORM, p.display().to_string()),
        None => Check::fail(PLATFORM, "1cv8.exe не найден", "установите платформу 1С на эту машину"),
    }));
    if cfg!(target_os = "linux") {
        v.push(within(t0, budget, DISPLAY, || {
            let d = crate::ops_host::display_env();
            let xvfb = crate::ops_host::find_xvfb_run();
            display_fact(d.as_deref(), d.as_deref().is_some_and(crate::ops_host::display_alive), xvfb.as_deref())
        }));
    }
    v.push(within(t0, budget, KIT, || {
        let exe = mcp_exe(&ctx.kit);
        if exe.exists() {
            Check::pass(KIT, exe.file_name().unwrap_or_default().to_string_lossy().to_string())
        } else {
            Check::fail(KIT, "файла нет", "комплект неполный — скопируйте заново")
        }
    }));
    let tools = tools_dir();
    let tf = within(t0, budget, TOOLS, || tools_fact(&ctx.kit.root, &tools));
    // Инструментов нет — пробу не поднимаем вовсе; но и «не проверено за N с»
    // инструментов не значит «их нет», поэтому решение целиком под `within`.
    let tools_ok = tf.ok;
    v.push(tf);
    v.push(within(t0, budget, VRUNNER, || {
        if tools_ok {
            vrunner_fact(ctx, &tools, limits.tools)
        } else {
            Check::fail(VRUNNER, "инструментов на машине нет", "1c-connect install")
        }
    }));
    v.push(within(t0, budget, PROJECT, || project_fact(ctx, &tools)));
    v.push(within(t0, budget, BASE, || base_fact(&h.base_path, std::path::Path::new(&h.base_path).exists())));
    let web = ctx.cfg.http_port();
    v.push(within(t0, budget, WEB, || {
        let up = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], web).into(), std::time::Duration::from_secs(3)).is_ok();
        web_fact(web, up)
    }));
    let base_url = ctx.cfg.base_url();
    v.push(within(t0, budget, PUBLICATION, || {
        publication_fact(&base_url, crate::ops_host::extension_version_in(&base_url, &h.db_user, &h.db_password, limits.net))
    }));
    v.push(within(t0, budget, GITEA, || gitea_fact(&crate::ops_host::gitea_probe(&ctx.cfg.gitea_url()))));
    if local {
        v.push(open_session_fact());
    } else {
        v.push(within(t0, budget, CONFIGURATOR, || configurator_fact(crate::bridge::configurator_count())));
    }
    let root = ctx.kit.root.display().to_string();
    // Носитель называем так, как его понимает человек: на Windows это буква
    // диска («C:\»), на Linux буквы нет — там показываем путь комплекта
    // целиком, иначе в отчёт попадает огрызок вида «/op».
    let media = if cfg!(windows) { root.get(..3).unwrap_or(&root) } else { root.as_str() };
    v.push(media_fact(crate::ops_host::media_kind(&ctx.kit.root), media));
    if local {
        // Держатель порта считается подпроцессом (`netstat`/`ss`) — под
        // бюджетом, как и прочее небыстрое.
        v.push(within(t0, budget, WORKSTATION_PORTS, || {
            let p = ctx.cfg.ports();
            let me = std::process::id();
            let ours = std::env::current_exe().ok();
            // Порт, про который держателя не выяснить (свободен, или pid не
            // прочитать), в список не попадает вовсе — обвинять человека не в
            // чем.
            let holders: Vec<(u16, Holder)> = [p.mcp, p.vrunner]
                .into_iter()
                .filter_map(|port| Some((port, holder_of(holder_pid(&crate::holder::of_port(port)?)?, me, ours.as_deref()))))
                .collect();
            workstation_ports_fact(p.mcp, p.vrunner, &holders)
        }));
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

    /// Штатное состояние рабочего места: трей держит оба порта. Процесс
    /// другой, но коннектор тот же — разовой команде он не помеха, и факт
    /// обязан назвать способ, чтобы человек не гадал, почему порты заняты.
    #[test]
    fn our_own_tray_holding_the_ports_is_not_a_failure() {
        let c = workstation_ports_fact(7773, 7783, &[(7773, Holder::OurConnector), (7783, Holder::OurConnector)]);
        assert!(c.ok && !c.info, "{c:?}");
        assert!(c.detail.contains("коннектором этого рабочего места"), "{c:?}");
    }

    /// Чужой процесс на порту — прежний провал с прежним текстом; свой же
    /// порт рядом в список не попадает.
    #[test]
    fn a_stranger_on_the_same_port_still_fails() {
        let c = workstation_ports_fact(7773, 7783, &[(7773, Holder::Ours), (7783, Holder::Stranger)]);
        assert!(!c.ok, "{c:?}");
        assert!(c.detail.contains("7783") && !c.detail.contains("7773"), "{c:?}");
        assert!(c.hint.contains("второй"), "{c:?}");
    }

    #[test]
    fn a_free_port_passes() {
        let c = workstation_ports_fact(7773, 7783, &[]);
        assert!(c.ok && !c.info, "{c:?}");
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

    /// «Конфигуратор запущен» — не поломка, а «сейчас занято человеком»:
    /// по спеке такие факты переводятся в сведения и остаются сведениями,
    /// иначе точка желтела бы у каждого, кто открыл 1С. Совет, что делать
    /// перед загрузкой, при этом никуда не девается.
    #[test]
    fn configurator_and_media_facts_say_what_to_do() {
        let c = configurator_fact(2);
        assert!(c.ok && c.info, "{c:?}");
        assert!(c.detail.contains('2') && c.hint.contains("загруз"), "{c:?}");
        assert!(configurator_fact(0).ok && !configurator_fact(0).info);
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

    /// Карточка и «Проверить» не могут расходиться (спека, раздел «Зелёный
    /// значит „работает“»): набор фактов у готовности — тот же, что печатает
    /// проверка, и тем же порядком. Различаются они только сроками, не
    /// составом: три факта из тринадцати и были причиной живого случая
    /// заказчика 2026-10-20 (точка зелёная, проверка тут же нашла два провала).
    #[test]
    fn readiness_has_the_same_facts_as_check() {
        for mode in ["tunnel", "local"] {
            let ctx = test_ctx(mode);
            let quick: Vec<String> = readiness(&ctx, None).into_iter().map(|c| c.what).collect();
            let full: Vec<String> = run(&ctx, None).into_iter().map(|c| c.what).collect();
            assert_eq!(quick, full, "mode {mode}");
            assert_eq!(quick, order(mode), "mode {mode}");
        }
    }

    /// Вердикт считает первый *провал*, а сведения пропускает: «в базе открыт
    /// сеанс», «Конфигуратор запущен» и «комплект на носителе» говорят «сейчас
    /// занято человеком», а не «сломано», и цвет точки не красят.
    #[test]
    fn first_fail_skips_notes() {
        let checks = vec![
            Check::pass(PLATFORM, "есть"),
            Check::note(OPEN_SESSION, "занято человеком"),
            Check::fail(PROJECT, "не разложен", "1c-connect install"),
            Check::fail(GITEA, "не отвечает", "dev1c-setup check"),
        ];
        assert_eq!(first_fail(&checks).unwrap().what, PROJECT);
        assert!(first_fail(&[Check::pass(PLATFORM, "есть")]).is_none());
        // Сведение остаётся сведением, даже если помечено неудачей.
        let odd = Check { ok: false, info: true, ..Check::fail(CONFIGURATOR, "запущен", "закройте") };
        assert!(first_fail(&[odd]).is_none(), "сведение вердикт не красит");
    }

    /// Факт, на который не хватило общего бюджета, — «не проверено», а не
    /// выдуманный провал: точка от этого не зелёная, но и вины человека в
    /// таком факте нет.
    #[test]
    fn a_fact_that_did_not_fit_the_budget_is_not_a_made_up_failure() {
        let long_ago = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(30)).expect("часы без 30 с");
        let c = within(long_ago, Some(std::time::Duration::from_secs(20)), GITEA, || unreachable!("бюджет вышел — факт не считают"));
        assert_eq!(c.what, GITEA);
        assert!(!c.ok && !c.info, "{c:?}");
        assert!(c.detail.contains("не проверено за 20 с"), "{c:?}");
        // Бюджет ещё есть — факт считается как обычно.
        let ok = within(std::time::Instant::now(), Some(std::time::Duration::from_secs(20)), GITEA, || Check::pass(GITEA, "1.25"));
        assert!(ok.ok, "{ok:?}");
        // Без бюджета (`1c-connect check`) срок не наступает никогда.
        assert!(within(long_ago, None, GITEA, || Check::pass(GITEA, "1.25")).ok);
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
