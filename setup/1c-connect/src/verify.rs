//! Проверки на машине с 1С, от основания к вершине: платформа → комплект →
//! инструменты и проект → база → веб-сервер и его порт → публикация и расширение →
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
const DB_USER: &str = "пользователь базы";
const SYSTEM_WEB: &str = "веб-сервер в системе";
// Имя про порт, а не про веб-сервер вообще: рядом стоит «веб-сервер в
// системе» (есть ли чем публиковать), и два похожих имени читались бы как
// повтор. Подробность этого факта и так всегда про порт.
const WEB: &str = "порт веб-сервера";
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
    // «Пользователь базы» — сразу за базой и до веб-сервера с публикацией:
    // без него работы не будет, как бы хорошо ни отвечала публикация.
    v.extend([KIT, TOOLS, VRUNNER, PROJECT, BASE, DB_USER]);
    // «Веб-сервер в системе» — перед портом веб-сервера и до публикации:
    // без системного apache2 и модуля платформы ни порт не ответит, ни
    // публикация не поднимется, и причина должна быть настоящей, а не
    // «порт не отвечает». Факт только на Linux: на Windows веб-сервер лежит
    // в комплекте, и его отсутствие ловит факт `инструменты комплекта`.
    if cfg!(target_os = "linux") {
        v.push(SYSTEM_WEB);
    }
    v.extend([WEB, PUBLICATION, GITEA]);
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

/// Адрес среды пуст — факт называет поле, а не говорит «Gitea не отвечает»
/// про адрес, которого человек никогда не задавал (поправка заказчика
/// 2026-10-21: программа не выдумывает адресов). Факт только про пустое
/// поле: когда адрес есть, отчёт показывает его сам — `gitea_fact` называет
/// адрес в любом исходе.
pub fn gitea_address_fact() -> Check {
    Check::fail(GITEA, "адрес среды не задан (поле gitea пусто)", "задайте адрес среды в 1c-connect.toml или выполните 1c-connect install")
}

/// Адрес называется в каждом исходе: отчёт показывает, с чем программа
/// работает, — иначе провал «не отвечает» не говорит, куда она стучалась.
pub fn gitea_fact(g: &Gitea, url: &str) -> Check {
    match g {
        Gitea::Down(e) => Check::fail(GITEA, format!("{url} не отвечает: {e}"), "среда не поднята или адрес не тот — dev1c-setup check на сервере"),
        Gitea::NoOrg => Check::fail(GITEA, format!("{url} отвечает, но организации dev1c в среде нет"), "образ среды старый — dev1c-setup install с новым комплектом"),
        Gitea::Ok(v) => Check::pass(GITEA, format!("Gitea {v} на {url}, организация dev1c есть")),
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

/// Занимает ли порты рабочего места тот, для кого считаются факты. Правило
/// спеки: **порты важны там, где мы их занимаем**. Занимают их окно,
/// `connect` и `serve` — там занятый чужим порт остаётся провалом: два
/// коннектора на одних портах дерутся и за порты, и за мьютекс проекта, и за
/// базу, и человек обязан об этом узнать. `install`, `export`, `import` и
/// `check` портов не трогают вовсе — им занятый порт только сведение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortUse {
    /// Порты занимаем мы. Просит факты с этим признаком одно окно
    /// (`gui::run_check`): `connect` и `serve` порты тоже занимают, но набор
    /// фактов не считают вовсе — некому и некуда его показать.
    WeListen,
    /// Разовая команда из командной строки (`check`, и она же в конце
    /// `install`) — портов не занимает.
    WeDont,
}

/// Порты моста (777N/778N). `busy` — занятые **не нами** порты вместе с
/// описанием держателя, как его называет `holder::of_port`
/// («`1c-connect (pid 812)`»); свободные и занятые этим же процессом в список
/// не попадают.
///
/// Чей это процесс — свой или чужой — здесь не разбирается вовсе, и это
/// нарочно (ревью 2026-10-21): опознать своего надёжно не выходит — путь к
/// образу программы признаёт своим коннектор другого пользователя ОС с тем же
/// именем рабочего места (порты `7770+N` считаются от имени рабочего места, не
/// от пользователя), а чтение чужого образа зависит от прав, и вердикт начинал
/// бы зависеть от того, из-под кого запущена команда. Важно единственное:
/// занимаем ли порты мы сами.
pub fn workstation_ports_fact(mcp: u16, vrunner: u16, busy: &[(u16, String)], ports: PortUse) -> Check {
    if busy.is_empty() {
        return Check::pass(WORKSTATION_PORTS, format!("{mcp} и {vrunner}: свободны либо заняты этим же процессом"));
    }
    match ports {
        // Текст прежний слово в слово: тому, кто сам собирался занять эти
        // порты, занятый порт и правда мешает — он их не получит.
        PortUse::WeListen => Check::fail(
            WORKSTATION_PORTS,
            format!("порт(ы) {} заняты другим процессом", busy.iter().map(|(p, _)| p.to_string()).collect::<Vec<_>>().join(", ")),
            "похоже, на этой машине уже открыт второй 1c-connect — закройте его и проверьте снова",
        ),
        // Держателя называем по имени: оно уже добыто, а человеку полезно
        // видеть, кто держит порт, даже когда это ничему не мешает.
        PortUse::WeDont => Check::note(
            WORKSTATION_PORTS,
            format!("{} — порты разовой команде не нужны", busy.iter().map(|(p, who)| format!("{p} держит {who}")).collect::<Vec<_>>().join(", ")),
        ),
    }
}

/// pid из `holder::of_port` (строка вида `"имя (pid N)"`), не разбирая вывод
/// `ss`/`netstat` заново. Нужен ровно для одного — отсеять свой же процесс
/// (окно проверяет само себя): это сравнение с `std::process::id()`, без
/// чтения чужих процессов и без всякой зависимости от прав.
fn holder_pid(desc: &str) -> Option<u32> {
    desc.rsplit("pid ").next()?.trim_end_matches(')').parse().ok()
}

pub fn media_fact(kind: Option<String>, root: &str) -> Check {
    match kind {
        Some(k) => Check::note(MEDIA, format!("{root}: {k} — инструменты работают из %LOCALAPPDATA%, комплект можно вынуть после install")),
        None => Check::pass(MEDIA, format!("{root}: обычный диск")),
    }
}

/// Спрашивать ли диск о каталоге базы. У `сервер\база` и у сетевого пути —
/// нет: `exists()` по недоступной шаре висит на таймауте SMB десятками
/// секунд. Правило одно на проверку и на замок переключателя, и живёт оно
/// здесь, в нижнем слое: окно собирается по фиче `gui`, а проверка — всегда,
/// так что обратное направление (`verify` → `gui`) ломало бы сборку без окна.
pub fn wants_directory(path: &str) -> bool {
    !path.starts_with("\\\\") && !(path.contains('\\') && !path.contains(':'))
}

/// Существует ли каталог базы — с учётом правила выше. Готовность этого
/// стоила особенно дорого (ревью S8): на сетевой базе уходил весь бюджет в
/// 20 с, и все факты ниже «базы» приходили как «не проверено».
fn base_exists_on_disk(path: &str) -> bool {
    wants_directory(path) && std::path::Path::new(path).exists()
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

/// Пользователь базы задан. Чистая функция от настроек: ни одного запроса в
/// сеть, и проваливает готовность сама по себе — до публикации и расширения.
///
/// Почему не хватает проверки публикации (замер 2026-10-20, стенд): она ходит
/// с basic-auth из полей, а учебная база пускает и анонима — `GET …/version`
/// отвечает 200 без заголовка авторизации, карточка горит зелёным. Дальше
/// начинается настоящая работа, и Конфигуратор отказывает: «Пользователь ИБ не
/// идентифицирован». Зелёный держался на терпимости конкретной базы и обещал
/// работу, которой нет; с этим фактом он держится на собственных настройках.
pub fn db_user_fact(user: &str) -> Check {
    if db_user_is_set(user) {
        Check::pass(DB_USER, user.to_string())
    } else {
        Check::fail(DB_USER, "не задан", "заполните поле «Пользователь базы» и нажмите «Сохранить»")
    }
}

/// Само правило «пользователь базы задан» — отдельно от факта, потому что
/// поверхностей у него две и обе требует спека: факт проверки (выше) и ворота
/// переключателя (`gui::gate`). Дублирование законно, расхождение — нет:
/// пробелы здесь считаются пустым полем («Пользователь ИБ не
/// идентифицирован»), и так это должно читаться в обеих.
pub fn db_user_is_set(user: &str) -> bool {
    !user.trim().is_empty()
}

/// Чем публиковать базу под Linux: системный `apache2` и модуль платформы
/// (`wsap24.so`/`wsap24t.so`). Веб-сервер под Linux — системный, и это
/// решение заказчика (спека, «Веб-сервер под Linux — системный»): в комплекте
/// его нет, в образе он есть, а на машине разработчика может не быть.
///
/// Отказ при запуске публикации («apache2 не найден в $PATH…») остаётся —
/// этот факт нужен, чтобы человек узнал о нехватке **до** того, как нажмёт, и
/// чтобы точка не была зелёной там, где публиковать нечем. Чистая функция:
/// оба пути ищет вызывающий (`publication::apache2_on_path`/`module_in` —
/// те же, которыми пользуется сама публикация), сюда приходит результат.
pub fn web_server_fact(apache: Option<&std::path::Path>, module: Option<&std::path::Path>) -> Check {
    let Some(apache) = apache else {
        return Check::fail(
            SYSTEM_WEB,
            "apache2 не найден в $PATH",
            "установите пакет apache2 — под Linux базу публикует веб-сервер системы, своего в комплекте нет",
        );
    };
    let Some(module) = module else {
        return Check::fail(
            SYSTEM_WEB,
            format!("{} есть, но рядом с платформой нет модуля 1С (wsap24.so/wsap24t.so)", apache.display()),
            "переустановите платформу 1С с компонентом «Модули расширения веб-сервера»",
        );
    };
    Check::pass(SYSTEM_WEB, format!("{}, модуль {}", apache.display(), module.display()))
}

/// Пути для `web_server_fact` на этой машине. Отдельной функцией и под
/// `cfg`: `publication::apache2_on_path` существует только не на Windows, а
/// `cfg!(target_os = ...)` в теле `facts_for_readiness` компилировался бы и
/// там тоже.
#[cfg(target_os = "linux")]
fn system_web_server_fact() -> Check {
    let apache = crate::publication::apache2_on_path();
    // Модуль ищем ровно там же, где его берёт конфигурация публикации
    // (`Paths::of`): рядом с найденным 1cv8, второго способа не заводим.
    let module = find_1cv8().and_then(|exe| crate::publication::module_in(exe.parent()?).ok());
    web_server_fact(apache.as_deref(), module.as_deref())
}

/// Что делать, когда порт базы молчит. Совет обязан быть правдивым в обоих
/// режимах публикации и на обеих системах (ревью S7): прежний «запустите
/// Apache/IIS» врал дважды — IIS на Linux не существует вовсе, а в режиме
/// `auto` (его пишет и сам стенд) веб-сервер поднимает **коннектор**, и
/// запускать человеку руками нечего: ему надо включить обслуживание и
/// прочитать, почему публикация не поднялась.
///
/// `own` — публикуем сами (`publication = "auto"`), `windows` — система, где
/// IIS бывает. Оба параметрами, а не `cfg!` внутри: так правило проверяется
/// тестом целиком, а не только на той системе, где тест запущен.
fn web_hint(own: bool, windows: bool) -> &'static str {
    match (own, windows) {
        (true, _) => "публикацию поднимает сам коннектор — включите обслуживание MCP; если оно включено, причину смотрите в журнале",
        (false, true) => "запустите Apache или IIS, на котором опубликована база",
        (false, false) => "запустите apache2, на котором опубликована база",
    }
}

/// HTTP-порт базы отвечает (обычный TCP-коннект, без логина) — `up` уже
/// посчитан вызывающим (`run`/`readiness`), здесь только оформление факта.
/// `own` — публикуем ли мы сами: от этого зависит совет (см. `web_hint`).
pub fn web_fact(port: u16, up: bool, own: bool) -> Check {
    if up {
        Check::pass(WEB, format!("порт {port} отвечает"))
    } else {
        Check::fail(WEB, format!("порт {port} не отвечает"), web_hint(own, cfg!(windows)))
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

/// Срок коннекта к порту веб-сервера — один на оба набора: он и так короче
/// любого из них, и растягивать его незачем.
const WEB_CONNECT: std::time::Duration = std::time::Duration::from_secs(3);

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
    /// Занимает ли порты рабочего места тот, кто просит факты — от этого
    /// зависит факт портов (`workstation_ports_fact`). Второго похожего
    /// параметра у набора фактов не завели: контекст вызывающего едет здесь.
    pub ports: PortUse,
}

impl Limits {
    /// «Проверить» и `1c-connect check`: сроки прежние, бюджета нет. `ports`
    /// у этих двоих разный — окно порты занимает, командная строка нет,
    /// поэтому он параметром, а не зашит.
    pub fn full(ports: PortUse) -> Self {
        Self { tools: std::time::Duration::from_secs(60), net: std::time::Duration::from_secs(60), budget: None, ports }
    }

    /// Готовность: короткие сроки и общий бюджет в 20 с. Считает её только
    /// окно, а оно порты занимает — выбора здесь нет.
    pub fn quick() -> Self {
        Self {
            tools: std::time::Duration::from_secs(15),
            net: std::time::Duration::from_secs(5),
            budget: Some(std::time::Duration::from_secs(20)),
            ports: PortUse::WeListen,
        }
    }
}

/// «Не проверено за N с» — своё состояние факта, ни зелёное, ни приговор:
/// `!ok` (точка не зелёная, и причину человек читает) вместе с `info` (это не
/// поломка — советовать «переустановите» тут нечего, совет один: спросить
/// заново). Так выглядит и сорванный срок самого факта, и вышедший общий
/// бюджет: спека требует именно этого, а не выдуманного провала.
pub fn not_checked(name: &str, waited: std::time::Duration) -> Check {
    Check {
        ok: false,
        info: true,
        ..Check::fail(
            name,
            format!("не проверено за {} с", waited.as_secs()),
            "готовность считается коротко — нажмите «Проверить», она считает без ограничения по времени",
        )
    }
}

/// Факт под общим бюджетом: когда бюджет уже вышел, факт не считается вовсе —
/// вместо выдуманного провала человек читает «не проверено».
fn within(started: std::time::Instant, budget: Option<std::time::Duration>, name: &'static str, f: impl FnOnce() -> Check) -> Check {
    match budget {
        Some(b) if started.elapsed() >= b => not_checked(name, b),
        _ => f(),
    }
}

/// Неудача, на которую ушёл весь срок, — не отказ, а сорванный срок: отличить
/// их внутри самого факта нечем (`extension_version` отдаёт `None` на любую
/// беду, `local::call` — строку), а вот по времени ожидания видно. Проба
/// инструментов отвечает 8–11 с при сроке 15, расширение — до 60 с при сроке
/// 5: срок рвётся штатно, и приговор «переустановите расширение» на нём — та
/// самая ложь карточки, против которой задача и делалась. Удачный факт сроком
/// не портится, быстрый отказ остаётся отказом со своим советом.
fn unless_timed_out(fact: Check, waited: std::time::Duration, limit: std::time::Duration) -> Check {
    if !fact.ok && !fact.info && waited >= limit {
        return not_checked(&fact.what, limit);
    }
    fact
}

/// Первый не зелёный факт — он и есть причина жёлтого вердикта. Сведения
/// («в базе открыт сеанс», «Конфигуратор запущен», «комплект на носителе»)
/// пропускаются: они `ok` и говорят «сейчас занято человеком», а не
/// «сломано». «Не проверено» (`!ok` при `info`) пропускать нельзя — зелёной
/// точке непосчитанный факт основанием не бывает.
pub fn first_fail(checks: &[Check]) -> Option<Check> {
    checks.iter().find(|c| !c.ok).cloned()
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
    let fact = match crate::local::call(cmd, "cache_show", serde_json::json!({}), timeout, &|_| {}) {
        Ok(r) if !r.is_error => Check::pass(VRUNNER, format!("AutumnMCP, {} с до ответа", t0.elapsed().as_secs())),
        Ok(r) => Check::fail(VRUNNER, format!("отвечает ошибкой: {}", r.text), "каталог проекта и инструменты: 1c-connect install"),
        Err(e) => Check::fail(VRUNNER, e, "1c-connect install (host.tools); журнал — setup.log в комплекте"),
    };
    unless_timed_out(fact, t0.elapsed(), timeout)
}

fn project_fact(ctx: &Ctx, tools: &std::path::Path) -> Check {
    use crate::project::Layout;
    let p = crate::project::Project::from_cfg(&ctx.cfg, tools);
    match crate::project::layout_state(&p) {
        Layout::Ok => Check::pass(PROJECT, format!("{}, ветка {}", p.dir.display(), p.branch)),
        Layout::Missing => Check::fail(PROJECT, format!("{} не разложен", p.dir.display()), "1c-connect install (host.project)"),
        Layout::BaseDir(f) => Check::fail(PROJECT, format!("{} — каталог базы ({f})", p.dir.display()), "project в 1c-connect.toml — другой каталог"),
        Layout::Drifted(d) => Check::fail(PROJECT, d, "1c-connect install — поправит ветку, origin и файлы"),
        Layout::NoGitea => Check::fail(PROJECT, "адрес среды не задан — origin собирать не из чего", "задайте адрес среды в 1c-connect.toml или выполните 1c-connect install"),
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
/// это состояние сессии, а не то, что можно вычислить здесь. `ports` —
/// занимает ли порты тот, кто просит проверку: окно (`PortUse::WeListen`) и
/// командная строка (`PortUse::WeDont`) зовут одну и ту же `run`, а порты
/// занимает только окно.
pub fn run(ctx: &Ctx, tunnel_state: Option<&str>, ports: PortUse) -> Vec<Check> {
    // Фаза — только здесь: по ней окно показывает карточку хода и запирает
    // кнопки. Сбор фактов её не объявляет, иначе молчаливый расчёт
    // готовности выглядел бы начавшейся операцией (спека).
    ctx.progress(Phase::Check);
    facts_for_readiness(ctx, tunnel_state, Limits::full(ports))
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
    v.push(within(t0, budget, BASE, || base_fact(&h.base_path, base_exists_on_disk(&h.base_path))));
    // Без сети и без сроков: это чистая функция от настроек, и считать её под
    // бюджетом было бы враньём — «не проверено» тут не бывает.
    v.push(db_user_fact(&h.db_user));
    // Поиск по `$PATH` и каталогу платформы — быстрый, но под бюджетом, как и
    // остальное: порядок фактов важнее экономии на одном `exists()`.
    #[cfg(target_os = "linux")]
    v.push(within(t0, budget, SYSTEM_WEB, system_web_server_fact));
    let web = ctx.cfg.http_port();
    v.push(within(t0, budget, WEB, || {
        // Закрытый порт отказывает мгновенно, так что весь срок здесь — это
        // не «не отвечает», а «не дозвонились» (брандмауэр, мёртвый адрес):
        // советовать «запустите Apache» на этом нельзя.
        let asked = std::time::Instant::now();
        let up = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], web).into(), WEB_CONNECT).is_ok();
        // `auto` — публикацию поднимает сам коннектор, и совет при молчащем
        // порту другой (`web_hint`).
        unless_timed_out(web_fact(web, up, h.publication == "auto"), asked.elapsed(), WEB_CONNECT)
    }));
    let base_url = ctx.cfg.base_url();
    v.push(within(t0, budget, PUBLICATION, || {
        // Срок сетевого запроса отличается от отказа только временем:
        // `extension_version_in` отдаёт `None` и на «не отвечает», и на
        // «не успел» (см. `unless_timed_out`).
        let asked = std::time::Instant::now();
        let ver = crate::ops_host::extension_version_in(&base_url, &h.db_user, &h.db_password, limits.net);
        unless_timed_out(publication_fact(&base_url, ver), asked.elapsed(), limits.net)
    }));
    v.push(within(t0, budget, GITEA, || match ctx.cfg.gitea_url() {
        Some(url) => gitea_fact(&crate::ops_host::gitea_probe(url), url),
        None => gitea_address_fact(),
    }));
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
            // В список идут только порты, занятые НЕ нами: порт свободный,
            // занятый этим же процессом (окно проверяет само себя) или такой,
            // про который держателя не выяснить, обвинять человека не даёт.
            let busy: Vec<(u16, String)> = [p.mcp, p.vrunner]
                .into_iter()
                .filter_map(|port| {
                    let who = crate::holder::of_port(port)?;
                    (holder_pid(&who)? != me).then_some((port, who))
                })
                .collect();
            workstation_ports_fact(p.mcp, p.vrunner, &busy, limits.ports)
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
        // Адрес среды задан полем и ведёт на закрытый локально порт: провал
        // мгновенный, как в ops_host::gitea_probe_on_a_closed_port_is_down.
        cfg.host.server = "127.0.0.1:3".into();
        cfg.host.gitea = "http://127.0.0.1:1".into();
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
        assert!(at("порт веб-сервера") < at("публикация и расширение"));
        assert!(at("публикация и расширение") < at("туннель"));
    }

    #[test]
    fn order_ends_with_the_tunnel_or_the_ports_and_puts_the_screen_after_the_platform() {
        // На Linux у набора два своих факта, которых нет ни на Windows, ни на
        // macOS: «экран для конфигуратора» сразу за платформой и «веб-сервер в
        // системе» перед портом веб-сервера. `d` сдвигает то, что стоит после
        // экрана, `w` — то, что стоит после обоих.
        let d = usize::from(cfg!(target_os = "linux"));
        let w = 2 * d;
        let tunnel = order("tunnel");
        assert_eq!(tunnel.len(), 15 + w);
        assert_eq!(tunnel[14 + w], "туннель");
        assert_eq!(tunnel[5 + d], "vrunner-mcp отвечает");
        assert_eq!(tunnel[11 + w], "Gitea");
        if d == 1 {
            assert_eq!(tunnel[3], "экран для конфигуратора");
            assert_eq!(tunnel[10], "веб-сервер в системе");
            assert_eq!(tunnel[11], "порт веб-сервера");
        }
        let local = order("local");
        assert_eq!(local.len(), 15 + w);
        assert_eq!(local[14 + w], "порты рабочего места");
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
            let checks = run(&ctx, Some("работает с 12:40"), PortUse::WeDont);
            let names: Vec<&str> = checks.iter().map(|c| c.what.as_str()).collect();
            assert_eq!(names, order(mode), "mode {mode}");
        }
    }

    /// Штатное состояние рабочего места: трей держит оба порта, а человек
    /// запускает разовую команду (`install`, `check`, `export`). Портов она не
    /// занимает, поэтому занятый порт — сведение, а не провал; и держателя
    /// факт называет по имени, чтобы человек не гадал, кто его держит.
    #[test]
    fn a_busy_port_is_only_a_note_for_a_command_that_does_not_listen() {
        let busy = [(7773, "1c-connect (pid 812)".to_string()), (7783, "1c-connect (pid 812)".to_string())];
        let c = workstation_ports_fact(7773, 7783, &busy, PortUse::WeDont);
        assert!(c.ok && c.info, "сведение, а не провал: {c:?}");
        assert!(c.detail.contains("1c-connect (pid 812)"), "держателя надо назвать: {c:?}");
        assert!(c.detail.contains("порты разовой команде не нужны"), "{c:?}");
    }

    /// Тому, кто сам собирается занять порты (окно, `connect`, `serve`),
    /// занятый порт мешает по-настоящему — прежний провал прежним текстом.
    #[test]
    fn a_busy_port_still_fails_whoever_needs_to_listen() {
        let busy = [(7783, "nginx (pid 500)".to_string())];
        let c = workstation_ports_fact(7773, 7783, &busy, PortUse::WeListen);
        assert!(!c.ok && !c.info, "{c:?}");
        assert!(c.detail.contains("порт(ы) 7783 заняты другим процессом"), "{c:?}");
        assert!(c.hint.contains("второй 1c-connect"), "{c:?}");
    }

    /// Свободные порты зелены в обоих случаях — правило о занятых, а не о
    /// самой проверке.
    #[test]
    fn a_free_port_passes_either_way() {
        for ports in [PortUse::WeListen, PortUse::WeDont] {
            let c = workstation_ports_fact(7773, 7783, &[], ports);
            assert!(c.ok && !c.info, "{ports:?}: {c:?}");
        }
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
        let checks = run(&ctx, None, PortUse::WeDont);

        let web = checks.iter().find(|c| c.what == WEB).unwrap();
        assert!(web.detail.contains("18080") && !web.detail.contains("9999"), "{web:?}");

        let publication = checks.iter().find(|c| c.what == PUBLICATION).unwrap();
        assert!(publication.detail.contains("18080"), "{publication:?}");
        assert!(!publication.detail.contains("9999") && !publication.detail.contains("Buh"), "{publication:?}");
    }

    /// Пустое поле «Пользователь базы» — собственный провал, и он обязан
    /// случиться раньше публикации. Живой замер 2026-10-20: база учебного
    /// стенда пускает анонима, `GET …/version` отвечает 200 и без заголовка
    /// авторизации — публикация зеленеет, а Конфигуратор потом отказывает
    /// («Пользователь ИБ не идентифицирован»). Зелёный, держащийся на
    /// терпимости конкретной базы, обещает работу, которой нет.
    #[test]
    fn an_empty_db_user_fails_before_any_request() {
        let empty = db_user_fact("");
        assert!(!empty.ok && !empty.info, "{empty:?}");
        assert!(empty.hint.contains("Пользователь базы"), "{empty:?}");
        assert!(db_user_fact("Администратор").ok, "{:?}", db_user_fact("Администратор"));
        // Пробелы — то же пустое поле: «Пользователь ИБ не идентифицирован».
        assert!(!db_user_fact("   ").ok);

        // Факт стоит раньше публикации — и в порядке, и в самом наборе.
        let names = order("local");
        let at = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(at(DB_USER) < at(PUBLICATION) && at(DB_USER) < at(WEB), "{names:?}");

        let mut ctx = test_ctx("local");
        ctx.cfg.host.db_user = String::new();
        let checks = run(&ctx, None, PortUse::WeDont);
        // Причина — пустой пользователь, а не «публикация не отвечает»:
        // смотрим с его места вниз, выше него в наборе свои провалы
        // (параметры, платформа) и к этому правилу они отношения не имеют.
        let tail: Vec<Check> = checks.iter().skip_while(|c| c.what != DB_USER).cloned().collect();
        assert_eq!(first_fail(&tail).unwrap().what, DB_USER, "{tail:?}");
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

    /// Пустое поле — провал про поле, а не «Gitea не отвечает» по
    /// выдуманному адресу: человек не должен искать неисправность в среде,
    /// которую он не настраивал.
    #[test]
    fn an_unset_environment_address_is_named_as_such() {
        let c = gitea_address_fact();
        assert!(!c.ok && !c.info, "{c:?}");
        assert!(c.detail.contains("адрес среды не задан") && c.detail.contains("gitea"), "{c:?}");
        assert!(!c.detail.contains("не отвечает"), "{c:?}");
        assert!(c.hint.contains("1c-connect install"), "{c:?}");
        // И факт берётся именно по пустому полю, а не по счёту от server.
        let mut ctx = test_ctx("local");
        ctx.cfg.host.gitea = String::new();
        let v = run(&ctx, None, PortUse::WeDont);
        let g = v.iter().find(|c| c.what == GITEA).expect("факт Gitea");
        assert!(g.detail.contains("адрес среды не задан"), "{g:?}");
    }

    /// И каждый исход называет адрес: отчёт показывает, с чем программа
    /// работает, а провал — куда она стучалась.
    #[test]
    fn gitea_fact_distinguishes_no_server_from_no_org_and_names_the_address() {
        let url = "http://localhost:3000";
        let down = gitea_fact(&crate::ops_host::Gitea::Down("timeout".into()), url);
        assert!(!down.ok && down.hint.contains("dev1c-setup"), "{down:?}");
        let noorg = gitea_fact(&crate::ops_host::Gitea::NoOrg, url);
        assert!(!noorg.ok && noorg.detail.contains("организации dev1c"), "{}", noorg.detail);
        let ok = gitea_fact(&crate::ops_host::Gitea::Ok("1.22".into()), url);
        assert!(ok.ok && ok.detail.contains("1.22"));
        for c in [&down, &noorg, &ok] {
            assert!(c.detail.contains(url), "{c:?}");
        }
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

    /// Совет при молчащем порту обязан быть правдой во всех четырёх случаях
    /// (ревью S7). Прежний «запустите Apache/IIS» врал дважды: IIS на Linux
    /// не существует, а в режиме `auto` веб-сервер поднимает сам коннектор —
    /// запускать человеку нечего.
    #[test]
    fn a_dead_port_is_advised_by_mode_and_system() {
        // Публикуем сами — что бы ни стояло в системе, руками не запускают.
        for windows in [true, false] {
            let own = web_hint(true, windows);
            assert!(own.contains("сам коннектор") && own.contains("включите"), "{own}");
            assert!(!own.contains("IIS") && !own.contains("Apache") && !own.contains("apache2"), "{own}");
        }
        // Чужой веб-сервер: IIS называем только там, где он бывает.
        let win = web_hint(false, true);
        assert!(win.contains("IIS") && win.contains("Apache"), "{win}");
        let linux = web_hint(false, false);
        assert!(linux.contains("apache2") && !linux.contains("IIS"), "{linux}");

        // И сам факт берёт совет оттуда же — зелёный остаётся без советов.
        assert_eq!(web_fact(1818, false, true).hint, web_hint(true, cfg!(windows)));
        assert!(web_fact(1818, true, true).hint.is_empty(), "{:?}", web_fact(1818, true, true));
    }

    /// Существование каталога спрашивают только у файловой базы — то же
    /// правило, что в воротах (`gate::wants_directory`), и тот же его
    /// источник. Ревью S8: `exists()` по недоступной шаре висит на таймауте
    /// SMB и съедал весь бюджет готовности в 20 с, после чего всё, что ниже
    /// «базы», приходило как «не проверено».
    #[test]
    fn a_network_base_is_never_looked_for_on_disk() {
        for path in ["\\\\nas\\bases\\buh", "srv-1c\\buh"] {
            assert!(!crate::gui::gate::wants_directory(path), "{path}");
            // Диска не касаемся вовсе — и ответ фактом не используется:
            // существование такой базы не проверяется по самому правилу.
            assert!(!base_exists_on_disk(path), "{path}");
            assert!(base_fact(path, false).ok, "{path}: факт существования не спрашивает");
        }
        // Файловую базу по-прежнему спрашиваем: иначе факт перестал бы ловить
        // отсутствующий каталог, ради которого он и заведён.
        let dir = std::env::temp_dir();
        assert!(base_exists_on_disk(&dir.display().to_string()), "{}", dir.display());
        assert!(!base_exists_on_disk(&dir.join("нет-такого-каталога-34").display().to_string()));
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
        let up = web_fact(1818, true, false);
        assert!(up.ok && up.detail.contains("1818"), "{up:?}");
        // Чужой веб-сервер — совет запустить его; имя сервера зависит от
        // системы и проверяется отдельно (`a_dead_port_is_advised_by_mode_and_system`).
        let down = web_fact(1818, false, false);
        assert!(!down.ok && down.hint.contains("опубликована база"), "{down:?}");
        let ok = publication_fact("http://x", Some("1.2".into()));
        assert!(ok.ok && ok.detail.contains("1.2"), "{ok:?}");
        let fail = publication_fact("http://x", None);
        assert!(!fail.ok && fail.hint.contains("install"), "{fail:?}");
    }

    /// Главное правило спеки про сроки: факт, чей срок сорвался, —
    /// «не проверено», а не приговор. Прежний тест на это место сверял имена
    /// фактов `readiness` и `run`, но после правки оба зовут один и тот же
    /// сбор — совпадение обеспечено конструкцией, а не проверкой (состав и
    /// порядок всё равно сверяет `every_fact_has_a_name_in_order`), и именно
    /// поэтому мимо прошёл живой случай: срок у пробы инструментов и у
    /// расширения рвётся штатно, а человек читал «1c-connect install» и
    /// «публикация в Конфигураторе» — лечение того, что цело.
    #[test]
    fn a_broken_deadline_is_not_checked_rather_than_a_failure() {
        let net = std::time::Duration::from_secs(5);
        let refusal = Check::fail(PUBLICATION, "http://x/version не отвечает", "1c-connect install, затем публикация в Конфигураторе");

        // Отказ пришёл сразу — настоящий провал со своим советом.
        let fast = unless_timed_out(refusal.clone(), std::time::Duration::from_millis(80), net);
        assert!(!fast.ok && !fast.info, "{fast:?}");
        assert!(fast.hint.contains("install"), "{fast:?}");

        // На отказ ушёл весь срок — приговора нет, есть «не проверено».
        let slow = unless_timed_out(refusal, net, net);
        assert_eq!(slow.what, PUBLICATION);
        assert!(!slow.ok && slow.info, "ни зелёный, ни провал: {slow:?}");
        assert!(slow.detail.contains("не проверено за 5 с"), "{slow:?}");
        assert!(!slow.hint.contains("install"), "чужого лечения в непроверенном факте быть не может: {slow:?}");
        // Точка на таком факте не зелёная — он и есть причина.
        assert_eq!(first_fail(&[slow]).unwrap().what, PUBLICATION);

        // Удачный факт сроком не портится, сведение — тоже.
        let took_long = std::time::Duration::from_secs(99);
        assert!(unless_timed_out(Check::pass(VRUNNER, "AutumnMCP"), took_long, net).ok);
        assert!(unless_timed_out(open_session_fact(), took_long, net).info);
    }

    /// Вердикт считает первый не зелёный факт, а сведения пропускает: «в базе
    /// открыт сеанс», «Конфигуратор запущен» и «комплект на носителе» говорят
    /// «сейчас занято человеком», а не «сломано», и цвет точки не красят.
    #[test]
    fn first_fail_skips_notes() {
        let checks = vec![
            Check::pass(PLATFORM, "есть"),
            Check::note(OPEN_SESSION, "занято человеком"),
            Check::fail(PROJECT, "не разложен", "1c-connect install"),
            Check::fail(GITEA, "не отвечает", "dev1c-setup check"),
        ];
        assert_eq!(first_fail(&checks).unwrap().what, PROJECT);
        assert!(first_fail(&[Check::pass(PLATFORM, "есть"), configurator_fact(2), media_fact(Some("FAT32".into()), "E:\\")]).is_none());
    }

    /// Факт, на который не хватило общего бюджета, — то же «не проверено», а
    /// не выдуманный провал; бюджета нет (командная строка) — срок не
    /// наступает никогда.
    #[test]
    fn a_fact_that_did_not_fit_the_budget_is_not_a_made_up_failure() {
        let long_ago = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(30)).expect("часы без 30 с");
        let c = within(long_ago, Some(std::time::Duration::from_secs(20)), GITEA, || unreachable!("бюджет вышел — факт не считают"));
        assert_eq!(c.what, GITEA);
        assert!(!c.ok && c.info, "{c:?}");
        assert!(c.detail.contains("не проверено за 20 с"), "{c:?}");
        assert_eq!(first_fail(&[c]).unwrap().what, GITEA, "непосчитанный факт зелёной точке основанием не бывает");
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

    /// Под Linux веб-сервер — системный, и это решение заказчика (спека,
    /// «Веб-сервер под Linux — системный»): в комплекте его нет, на машине
    /// разработчика он может не стоять. Отказ при публикации уже есть, но
    /// человек обязан узнать про это **до** того, как нажмёт: точка на машине
    /// без сервера зелёной быть не может.
    #[test]
    fn a_missing_apache_names_the_package() {
        let module = std::path::PathBuf::from("/opt/1cv8t/x86_64/8.5.1.1150/wsap24t.so");
        let c = web_server_fact(None, Some(&module));
        assert!(!c.ok && !c.info, "{c:?}");
        assert!(c.detail.contains("apache2"), "{c:?}");
        assert!(c.hint.contains("apache2"), "пакет надо назвать: {c:?}");
    }

    /// Apache на месте, а модуля платформы нет — беда другая и лечится другим:
    /// пакет тут ни при чём, не хватает компонента самой платформы 1С.
    #[test]
    fn a_missing_1c_module_names_the_platform_component() {
        let apache = std::path::PathBuf::from("/usr/sbin/apache2");
        let c = web_server_fact(Some(&apache), None);
        assert!(!c.ok && !c.info, "{c:?}");
        assert!(c.detail.contains("wsap24"), "{c:?}");
        assert!(c.hint.contains("веб-сервера"), "компонент платформы: {c:?}");
        assert!(!c.hint.contains("apache2"), "apache2 на месте — лечить его незачем: {c:?}");
    }

    /// Оба на месте — зелёный с путями: человек видит, какой именно apache2 и
    /// какой модуль будут подняты, а не голое «всё хорошо».
    #[test]
    fn both_present_pass_and_name_the_path() {
        let apache = std::path::PathBuf::from("/usr/sbin/apache2");
        let module = std::path::PathBuf::from("/opt/1cv8t/x86_64/8.5.1.1150/wsap24t.so");
        let c = web_server_fact(Some(&apache), Some(&module));
        assert!(c.ok && !c.info, "{c:?}");
        assert!(c.detail.contains("/usr/sbin/apache2"), "{c:?}");
        assert!(c.detail.contains("wsap24t.so"), "{c:?}");
    }

    #[test]
    fn the_tunnel_fact_is_information_when_nobody_connected() {
        let c = tunnel_fact(None);
        assert!(c.info && c.ok, "{c:?}");
        let c = tunnel_fact(Some("работает с 12:40"));
        assert!(!c.info && c.ok);
    }
}
