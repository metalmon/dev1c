//! Проверки на машине с 1С, от основания к вершине: платформа → комплект →
//! инструменты и проект → база → веб-сервер и его порт → публикация и расширение →
//! среда (Gitea) → что мешает загрузке → туннель. Человек читает сверху
//! вниз и останавливается на первом провале.

use crate::ops_host::{mcp_exe, tools_state, Ctx, Gitea};
use common::check::Check;
use common::op::OpState;
use common::{Phase, Runner};

// Имена фактов — часть контракта (см. тесты). Общие — не зависят от режима;
// остальные расставляет `order`/`run` по режиму из `cfg.host.mode`.
const PARAMS: &str = "параметры";
const PLATFORM: &str = "платформа 1С";
const DISPLAY: &str = "экран для конфигуратора";
const ANALYZER: &str = "платформа для анализатора";
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
    let mut v = vec![DEADLINE, PARAMS, PLATFORM, ANALYZER];
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

/// Строки платформы в `bsl-analyzer.toml` проекта (модуль `analyzer`). Факт
/// нужен ровно потому, что поверх строки человека программа не пишет: без
/// него чужая строка жила бы молча, а молчание тут и есть отказ — ради него
/// файл и затевался.
///
/// Провал — ровно по одной строке, `target_platform_version`: она говорит
/// анализатору, на какой линии платформы код, и значение не от этой машины
/// гасит проверку имён. `min_platform_version` коннектор не пишет вовсе
/// (решение заказчика 2026-10-21: версия машины — не минимум заказчика), и
/// расхождением она быть не может — только сведением: её поставил человек.
///
/// Чистая. `ours` — линия платформы этой машины (`analyzer::machine_line`);
/// `None` значит, что платформы нет и сверять нечем — тогда факт так и
/// говорит, а не выдаёт непроверенное за проверенное (находка повторного
/// ревью задачи 36). `want` — что эта машина записала бы
/// (`analyzer::expected`), `have` — что стоит в корне файла
/// (`analyzer::found`). Показываем то, что в файле, а не то, что мы бы
/// написали: иначе зелёный факт называл бы строку, которой там нет.
pub fn analyzer_fact(ours: Option<&str>, want: &[(&'static str, String)], have: &[(&'static str, String)]) -> Check {
    use crate::analyzer::{FILE, MIN, TARGET};
    let got = |key: &str| have.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone());
    let shown = || have.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join(", ");
    let Some(ours) = ours else {
        return match have.is_empty() {
            true => Check::note(ANALYZER, "версия платформы неизвестна — строку не пишем"),
            false => Check::note(ANALYZER, format!("платформу 1С не нашли — строки в {FILE} не сверяли: {}", shown())),
        };
    };
    if let Some(target) = got(TARGET) {
        if target != ours {
            return Check::fail(
                ANALYZER,
                format!("{TARGET} = {target}, а платформа этой машины {ours}"),
                format!("чужая строка гасит проверку имён — поправьте {FILE} в проекте или удалите строку, 1c-connect install запишет свою"),
            );
        }
    }
    let missing: Vec<&str> = want.iter().map(|(k, _)| *k).filter(|k| got(k).is_none()).collect();
    if !missing.is_empty() {
        let where_ = if have.is_empty() { format!("{FILE} в проекте нет") } else { format!("в {FILE} нет строк") };
        return Check::note(ANALYZER, format!("{where_}: {} — их запишет 1c-connect install", missing.join(", ")));
    }
    if have.is_empty() {
        // Файла нет, и он не нужен: линия машины не новее каталога
        // анализатора, а минимум — не наше дело.
        return Check::note(ANALYZER, format!("платформа {ours} — строка анализатору не нужна"));
    }
    let mut lines = vec![shown()];
    // Минимум в файле — всегда рука человека: программа его не пишет.
    if got(MIN).is_some() {
        lines.push("минимум задан вручную".into());
    }
    Check::pass(ANALYZER, lines.join(", "))
}

/// Какая платформа и **почему** она (`platform::Why`), а не только путь:
/// выбор идёт по лестнице 1С, и человек должен видеть, что его решило —
/// поле в toml, пин базы, общий пин или «не было выбора». Другие
/// установленные версии называются тут же: иначе «самая новая» звучит как
/// «единственная».
///
/// Отказ — это и пустая машина, и названный пин, которого на ней нет; текст
/// приходит из `platform::resolve` и годится без переделки, потому что он
/// же становится причиной холодного замка.
pub fn platform_fact(choice: &Result<crate::platform::Choice, String>, others: &[String]) -> Check {
    match choice {
        Err(e) => Check::fail(PLATFORM, e.clone(), "установите нужную версию платформы 1С или исправьте platform в 1c-connect.toml"),
        Ok(c) => {
            let mut detail = format!("{} ({}) — {}", c.version, c.exe.display(), c.why);
            if !others.is_empty() {
                detail.push_str(&format!("; на машине есть ещё: {}", others.join(", ")));
            }
            Check::pass(PLATFORM, detail)
        }
    }
}

/// Адрес среды пуст — факт называет поле, а не говорит «Gitea не отвечает»
/// про адрес, которого человек никогда не задавал (поправка заказчика
/// 2026-10-21: программа не выдумывает адресов). Факт только про пустое
/// поле: когда адрес есть, отчёт показывает его сам — `gitea_fact` называет
/// адрес в любом исходе.
pub fn gitea_address_fact() -> Check {
    Check::fail(GITEA, crate::cfg::NO_GITEA, crate::cfg::NO_GITEA_FIX)
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
pub fn open_session_fact(sessions: usize) -> Check {
    if sessions == 0 {
        return Check::note(OPEN_SESSION, "открытых сеансов 1С на этой машине нет — база свободна для агента");
    }
    // Про «один сеанс» не утверждаем: это правда учебной платформы, а на
    // машине разработчика база боевая и сеансов там сколько угодно
    // (заказчик 25.10). Говорим то, что верно всюду: открытый сеанс МОЖЕТ
    // закрыть агенту дорогу, и две причины почему.
    Check::note(
        OPEN_SESSION,
        format!(
            "{} — агенту могут отказать: Конфигуратор держит базу монопольно, а учебная платформа ограничена одним сеансом",
            common::word::count(sessions as u64, "открыт сеанс 1С", "открыто сеанса 1С", "открыто сеансов 1С")
        ),
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
/// машине. Один и тот же вызов — что из `run()`, что из молчаливого расчёта
/// готовности в окне: набор фактов (`gather`) один, второй не заводится.
pub fn tools_fact(kit_root: &std::path::Path) -> Check {
    match tools_state(kit_root) {
        OpState::Done(d) => Check::pass(TOOLS, d),
        // Копии на машине больше нет, значит нет и двух прежних исходов:
        // `Missing` («в профиле ничего нет») и `Drifted` («на машине одна
        // версия, в комплекте другая»). Остаётся одно — чего-то нет в самом
        // комплекте, и лечится это только целым комплектом.
        OpState::Missing | OpState::Drifted { .. } => Check::fail(TOOLS, "комплект неполный", "скопируйте полный комплект заново"),
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
/// Отказ при запуске публикации («apache2 не найден…») остаётся —
/// этот факт нужен, чтобы человек узнал о нехватке **до** того, как нажмёт, и
/// чтобы точка не была зелёной там, где публиковать нечем. Чистая функция:
/// оба пути ищет вызывающий (`publication::apache2_on_path`/`module_in` —
/// те же, которыми пользуется сама публикация), сюда приходит результат.
pub fn web_server_fact(apache: Option<&std::path::Path>, module: Option<&std::path::Path>) -> Check {
    let Some(apache) = apache else {
        return Check::fail(
            SYSTEM_WEB,
            "apache2 не найден ни в $PATH, ни в системных каталогах служб",
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
/// `cfg!(target_os = ...)` в теле `gather` компилировался бы и там тоже.
#[cfg(target_os = "linux")]
fn system_web_server_fact(cfg: &crate::cfg::Cfg) -> Check {
    let apache = crate::publication::apache2_on_path();
    // Модуль ищем ровно там же, где его берёт конфигурация публикации
    // (`Paths::of`): рядом с найденным 1cv8, второго способа не заводим.
    let module = crate::ops_host::find_1cv8(cfg).and_then(|exe| crate::publication::module_in(exe.parent()?).ok());
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

/// Потолок срока коннекта к порту веб-сервера. Закрытый порт отказывает
/// мгновенно, и для него потолок ничего не стоит; отфильтрованный
/// (брандмауэр с DROP) висит молча до конца срока — отдать ему весь остаток
/// общего бюджета значило бы оставить публикации и Gitea после него ровно
/// нулевой остаток (находка ревью задачи 2, круг 2).
const WEB_CONNECT: std::time::Duration = std::time::Duration::from_secs(3);

/// HTTP-порт базы отвечает (обычный TCP-коннект, без логина) — `up` уже
/// посчитан вызывающим (`gather`), здесь только оформление факта. `own` —
/// публикуем ли мы сами: от этого зависит совет (см. `web_hint`).
pub fn web_fact(port: u16, up: bool, own: bool) -> Check {
    if up {
        Check::pass(WEB, format!("порт {port} отвечает"))
    } else {
        Check::fail(WEB, format!("порт {port} не отвечает"), web_hint(own, cfg!(windows)))
    }
}

/// Расширение отвечает по настроенному адресу настроенными учётными данными —
/// `ver` уже посчитан (`extension_version`), здесь только оформление факта.
pub fn publication_fact(base_url: &str, ver: Option<String>, sessions: usize) -> Check {
    match ver {
        Some(ver) => Check::pass(PUBLICATION, format!("{base_url} → расширение {ver}")),
        // Совет по тому, что знаем. Открыт сеанс 1С — почти наверняка дело в
        // нём: учебная платформа держит один, и HTTP-сервису базы в неё не
        // попасть. Советовать тут установку значит посылать человека чинить
        // то, что не сломано (заказчик 25.10: «пользователь занят открытой
        // базой, а мы в журнале предлагаем что-то установить»).
        None if sessions > 0 => Check::fail(
            PUBLICATION,
            format!("{base_url}/version не отвечает, а на машине открыт сеанс 1С"),
            "закройте Предприятие или Конфигуратор и повторите — открытый сеанс может держать базу",
        ),
        None => Check::fail(PUBLICATION, format!("{base_url}/version не отвечает"), "1c-connect install, затем публикация в Конфигураторе (см. install)"),
    }
}

/// Первый не зелёный факт — он и есть причина жёлтого вердикта. Сведения
/// («в базе открыт сеанс», «Конфигуратор запущен», «комплект на носителе»)
/// пропускаются: они `ok` и говорят «сейчас занято человеком», а не
/// «сломано».
pub fn first_fail(checks: &[Check]) -> Option<Check> {
    checks.iter().find(|c| !c.ok).cloned()
}

/// Что известно про раннер без единого нового процесса — другого способа
/// узнать про него нет вовсе (спека: холодный старт, 28 с, запрещён как
/// плата за факт; замер 21.10 в контейнере). Прежде факт «vrunner-mcp
/// отвечает» всегда поднимал НОВЫЙ процесс и ждал его холодного старта; при
/// бюджете готовности в 20 с он не успевал никогда, и окно при обычном
/// включении писало «не всё готово», а во время своей же выгрузки второй
/// процесс отвечал ошибкой — «vrunner-mcp не отвечает» про инструменты,
/// которые в этот момент как раз работали. Леджер комплекта, пункты 17 и 19.
#[derive(Debug, PartialEq, Eq)]
pub enum RunnerState {
    /// Мьютекс проекта занят: идёт наша же долгая операция. Это не поломка, а
    /// «машина занята нами», и говорить об этом надо словами.
    Busy(String),
    /// Раннер подавал голос столько назад — значит он поднят и говорит.
    Answered(std::time::Duration),
    /// Ни разу за этот запуск — и пробу поднимать запрещено: холодный старт
    /// не плата за факт.
    Idle,
}

/// Давность словами. Секунды до минуты: «отвечал 0 мин назад» на свежем
/// ответе выглядело бы подозрительно круглым.
fn ago(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} с")
    } else if s < 3600 {
        format!("{} мин", s / 60)
    } else {
        format!("{} ч {:02} мин", s / 3600, (s % 3600) / 60)
    }
}

/// Знание о раннере без единого нового процесса. `None` — не знаем ничего,
/// и это не повод его поднимать: спека прямо запрещает холодный старт ради
/// факта (28 с), а мост сам отмечает, что раннер отвечал агенту, — это
/// знание бесплатно (`RunnerState::Idle`, `runner_fact`).
pub fn runner_evidence() -> Option<RunnerState> {
    evidence_from(busy_now(), crate::bridge::runner_answered_ago())
}

/// Идёт ли под мьютексом проекта наша долгая операция, и какая.
fn busy_now() -> Option<String> {
    match crate::project::LOCK.try_lock() {
        Err(std::sync::TryLockError::WouldBlock) => {
            let what = crate::project::doing();
            Some(if what.is_empty() { "другая операция".into() } else { what.to_string() })
        }
        _ => None,
    }
}

/// Развилка сама по себе, без глобалов: порядок важен — пока идёт выгрузка,
/// «отвечал минуту назад» было бы правдой, но не тем, что человеку нужно
/// знать.
fn evidence_from(busy: Option<String>, answered: Option<std::time::Duration>) -> Option<RunnerState> {
    match (busy, answered) {
        (Some(what), _) => Some(RunnerState::Busy(what)),
        (None, Some(d)) => Some(RunnerState::Answered(d)),
        (None, None) => None,
    }
}

/// Факт по знанию о раннере. Чистая функция: вся её логика проверяется
/// тестами без процессов, базы и сети.
pub fn runner_fact(ev: &RunnerState) -> Check {
    match ev {
        RunnerState::Busy(what) => Check::note(VRUNNER, format!("занят: идёт {what}")),
        RunnerState::Answered(d) => Check::pass(VRUNNER, format!("отвечает агенту, последний ответ {} назад", ago(*d))),
        // Не зелёный и не приговор: раннера сейчас нет, но его отсутствие до
        // первого вызова агента — норма, а не поломка. Холодный старт (28 с)
        // ради галочки запрещён спекой: мост даром отдаёт то же знание, как
        // только раннер хоть раз ответит агенту.
        RunnerState::Idle => Check::note(VRUNNER, "не поднимался: стартует по первому вызову агента (около 30 с)"),
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
        Layout::NoGitea => Check::fail(PROJECT, crate::cfg::NO_GITEA, crate::cfg::NO_GITEA_FIX),
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

/// Срок всего набора — один, на окно и на командную строку одинаково (спека:
/// «набор один, срок один»). Раньше у готовности был отдельный короткий срок
/// (бюджет 20 с, сеть 5 с) и терпеливый второй заход без срока вовсе, а
/// «Проверить» — третий, свой; три пробы с разными сроками противоречили друг
/// другу на одной и той же машине (заказчик 25.10: быстрый расчёт говорил
/// «не готово», «Проверить» тут же — 18 из 18). 60 с хватает на сетевые факты
/// с запасом; раннера набор не поднимает вовсе ни на каком проходе (спека,
/// `runner_evidence`).
pub const SET_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// Факты для режима `cfg.host.mode` (`"tunnel"` или, по умолчанию при любом
/// другом значении, тот же `"tunnel"` — валидирован раньше, здесь не
/// перепроверяется). `tunnel_state` имеет смысл только в туннельном режиме:
/// это состояние сессии, а не то, что можно вычислить здесь. `ports` —
/// занимает ли порты тот, кто просит проверку (`workstation_ports_fact`):
/// окно (`PortUse::WeListen`) и командная строка (`PortUse::WeDont`).
///
/// Печатает фазу и запирает кнопки — обёртка над `gather` с общим сроком
/// (`SET_DEADLINE`). Молчаливый расчёт (окно, фоном) зовёт `gather`
/// напрямую — ему фаза не нужна и не положена (см. `gui::run_readiness`).
pub fn run(ctx: &Ctx, tunnel_state: Option<&str>, ports: PortUse) -> Vec<Check> {
    // Фаза — только здесь: по ней окно показывает карточку хода и запирает
    // кнопки. Сбор фактов её не объявляет, иначе молчаливый расчёт
    // готовности выглядел бы начавшейся операцией (спека).
    ctx.progress(Phase::Check);
    gather(ctx, tunnel_state, ports, std::time::Instant::now() + SET_DEADLINE)
}

/// Сам сбор фактов — один набор для «Проверить» и для вердикта готовности,
/// один срок на весь набор (`deadline`). Ничего не печатает и не объявляет
/// фазы. Дорогим фактам (публикация, Gitea, порт веб-сервера) передаётся не
/// свой кусок бюджета, а то, что осталось от общего срока к их очереди —
/// `deadline.saturating_duration_since(Instant::now())`: кто досталось рано,
/// получает больше, и это честно, потому что порядок — тот же порядок, в
/// котором человек читает карточку (см. `order`). Исключение — порт
/// веб-сервера: его остаток берётся с потолком (`WEB_CONNECT`), иначе
/// отфильтрованный порт (брандмауэр с DROP, висит молча до конца срока)
/// съедал бы весь остаток и оставлял публикации с Gitea после себя ноль —
/// это и есть тот самый выдуманный отказ, против которого вся правка.
/// Срыв срока здесь не отдельный исход: дорогой факт получает 0 (или около
/// того) на остаток и отвечает как обычно — отказом, который и есть правда о
/// сорванном сроке, а не выдуманное «не проверено».
pub fn gather(ctx: &Ctx, tunnel_state: Option<&str>, ports: PortUse, deadline: std::time::Instant) -> Vec<Check> {
    let local = ctx.cfg.host.mode == "local";
    let mut v = Vec::new();
    let h = &ctx.cfg.host;
    v.push(licence_fact(&crate::licence::state_now()));
    v.push(params_fact(ctx.kit.state_path(crate::cfg::FILE).exists(), &h.server));
    // Платформу ищем один раз на два факта: сканирование каталогов не нужно
    // повторять, а строки для анализатора без неё не считаются вовсе.
    let (choice, others) = crate::ops_host::platform_report(&ctx.cfg);
    let platform = choice.as_ref().ok().map(|c| c.exe.clone());
    v.push(platform_fact(&choice, &others));
    v.push({
        let ours = platform.as_deref().and_then(crate::flow::platform_version_from_exe).and_then(|v| crate::analyzer::machine_line(&v));
        let want = crate::ops_host::analyzer_lines_of(platform.as_deref());
        analyzer_fact(ours.as_deref(), &want, &crate::analyzer::found(std::path::Path::new(&h.project)))
    });
    if cfg!(target_os = "linux") {
        v.push({
            let d = crate::ops_host::display_env();
            let xvfb = crate::ops_host::find_xvfb_run();
            display_fact(d.as_deref(), d.as_deref().is_some_and(crate::ops_host::display_alive), xvfb.as_deref())
        });
    }
    v.push({
        let exe = mcp_exe(&ctx.kit);
        if exe.exists() {
            Check::pass(KIT, exe.file_name().unwrap_or_default().to_string_lossy().to_string())
        } else {
            Check::fail(KIT, "файла нет", "комплект неполный — скопируйте заново")
        }
    });
    let tools = ctx.kit.root.clone();
    let tf = tools_fact(&ctx.kit.root);
    // Инструментов нет — факт об этом и говорит, пробовать нечего.
    let tools_ok = tf.ok;
    v.push(tf);
    v.push(if !tools_ok {
        Check::fail(VRUNNER, "инструментов на машине нет", "1c-connect install")
    } else {
        // Только знание, которое уже есть (занятость мьютекса проекта или
        // отметка моста, что раннер отвечал агенту): спека прямо запрещает
        // холодный старт (28 с) ради факта — мост отдаёт то же знание даром,
        // как только раннер хоть раз ответит агенту.
        runner_fact(&runner_evidence().unwrap_or(RunnerState::Idle))
    });
    v.push(project_fact(ctx, &tools));
    v.push(base_fact(&h.base_path, base_exists_on_disk(&h.base_path)));
    // Чистая функция от настроек, без сети и без срока.
    v.push(db_user_fact(&h.db_user));
    #[cfg(target_os = "linux")]
    v.push(system_web_server_fact(&ctx.cfg));
    let web = ctx.cfg.http_port();
    v.push({
        // Дорогой факт, но не весь остаток — свой кусок, не больше
        // `WEB_CONNECT` (см. её докстроку: почему остаток сюда отдавать
        // целиком нельзя).
        let left = deadline.saturating_duration_since(std::time::Instant::now()).min(WEB_CONNECT);
        let up = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], web).into(), left).is_ok();
        // `auto` — публикацию поднимает сам коннектор, и совет при молчащем
        // порту другой (`web_hint`).
        web_fact(web, up, h.publication == "auto")
    });
    let base_url = ctx.cfg.base_url();
    // Сколько сеансов 1С открыто прямо сейчас — считаем один раз на два
    // факта: по нему же публикация выбирает, что советовать.
    let sessions = crate::bridge::configurator_count();
    v.push({
        // Дорогой факт: срок — остаток общего (`extension_version_in` сама
        // отдаёт `None`, если остатка уже нет).
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let ver = crate::ops_host::extension_version_in(&base_url, &h.db_user, &h.db_password, left);
        publication_fact(&base_url, ver, sessions)
    });
    v.push(match ctx.cfg.gitea_url() {
        // Дорогой факт: срок — остаток общего.
        Some(url) => gitea_fact(&crate::ops_host::gitea_probe(url, deadline.saturating_duration_since(std::time::Instant::now())), url),
        None => gitea_address_fact(),
    });
    if local {
        v.push(open_session_fact(sessions));
    } else {
        v.push(configurator_fact(sessions));
    }
    let root = ctx.kit.root.display().to_string();
    // Носитель называем так, как его понимает человек: на Windows это буква
    // диска («C:\»), на Linux буквы нет — там показываем путь комплекта
    // целиком, иначе в отчёт попадает огрызок вида «/op».
    let media = if cfg!(windows) { root.get(..3).unwrap_or(&root) } else { root.as_str() };
    v.push(media_fact(crate::ops_host::media_kind(&ctx.kit.root), media));
    if local {
        v.push({
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
            workstation_ports_fact(p.mcp, p.vrunner, &busy, ports)
        });
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
    // В локальном режиме туннельных полей в toml рабочего места обычно нет
    // вовсе, и при чтении на их место встаёт умолчание образца. Печатать его
    // нельзя: так в отчёте стенда и появился `server = "192.168.1.10:3393"` —
    // адрес, которого никто не задавал и которым никто не пользуется (находка
    // соседней сессии 2026-10-21). Окно эти поля в локальном режиме тоже не
    // показывает; отчёт держится того же правила.
    if c.host.mode == "local" {
        c.host.server.clear();
        c.host.password.clear();
    }
    toml::to_string(&c).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::check::Outcome;

    /// Каждая чистая фактовая функция модуля (без `Ctx`, диска и сети) один
    /// раз, с заведомо плохими или граничными аргументами. Общая выборка для
    /// теста на «не проверено» (читает текст каждого факта) и для
    /// `sample_bad_facts` (оставляет только провалы).
    fn all_facts_sample() -> Vec<Check> {
        vec![
            licence_fact(&crate::licence::State::Expired),
            params_fact(false, "192.168.1.10:3393"),
            platform_fact(&Err("нет платформы 1С".into()), &[]),
            analyzer_fact(None, &[], &[]),
            display_fact(None, false, None),
            configurator_fact(1),
            open_session_fact(1),
            media_fact(Some("FAT32".into()), "E:\\"),
            base_fact("C:\\нет-такого-каталога-для-теста-one-truth", false),
            db_user_fact(""),
            web_server_fact(None, None),
            web_fact(1818, false, false),
            publication_fact("http://x", None, 0),
            gitea_address_fact(),
            gitea_fact(&crate::ops_host::Gitea::Down("timeout".into()), "http://x"),
            tunnel_fact(None),
            runner_fact(&RunnerState::Idle),
            workstation_ports_fact(7773, 7783, &[(7783, "nginx (pid 500)".to_string())], PortUse::WeListen),
        ]
    }

    /// Только настоящие провалы — для проверки, что у каждого есть совет.
    fn sample_bad_facts() -> Vec<Check> {
        all_facts_sample().into_iter().filter(|c| c.outcome() == Outcome::Bad).collect()
    }

    #[test]
    fn the_set_never_says_it_did_not_check() {
        // Срок уже вышел («остаток» для дорогих фактов — 0), и ни один из
        // них не откатывается к «не проверено»: такого исхода больше нет.
        let ctx = test_ctx("tunnel");
        let checks = gather(&ctx, None, PortUse::WeDont, std::time::Instant::now());
        assert!(!checks.is_empty());
        assert!(!format!("{checks:?}").contains("не проверено"), "{checks:?}");
        // И ни одна чистая фактовая функция модуля не говорит так сама по
        // себе — тот же запрет и на уровне отдельного факта.
        assert!(!format!("{:?}", all_facts_sample()).contains("не проверено"));
    }

    #[test]
    fn every_bad_fact_carries_a_hint() {
        let bad = sample_bad_facts();
        // Пустая выборка прошла бы циклом ниже вхолостую и ничего не
        // доказала бы — выборка обязана быть непустой сама по себе.
        assert!(!bad.is_empty(), "в выборке не нашлось ни одного провала");
        for c in bad {
            assert!(!c.hint.is_empty(), "провал без совета, что делать: {c:?}");
        }
    }

    #[test]
    fn the_report_hides_tunnel_fields_in_local_mode() {
        // Умолчание образца (`192.168.1.10:3393`) не должно попадать в отчёт
        // локального рабочего места: этого адреса никто не задавал.
        let mut cfg = crate::cfg::Cfg::default();
        cfg.host.mode = "local".into();
        let local = cfg_for_report(&cfg);
        assert!(!local.contains("192.168.1.10"), "{local}");
        cfg.host.mode = "tunnel".into();
        let tunnel = cfg_for_report(&cfg);
        assert!(tunnel.contains("192.168.1.10"), "в туннеле адрес среды нужен: {tunnel}");
    }

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
        assert_eq!(tunnel.len(), 16 + w);
        assert_eq!(tunnel[15 + w], "туннель");
        // Платформа и строка платформы для анализатора — рядом: вторая без
        // первой не считается вовсе.
        assert_eq!(tunnel[2], "платформа 1С");
        assert_eq!(tunnel[3], "платформа для анализатора");
        assert_eq!(tunnel[6 + d], "vrunner-mcp отвечает");
        assert_eq!(tunnel[12 + w], "Gitea");
        if d == 1 {
            assert_eq!(tunnel[4], "экран для конфигуратора");
            assert_eq!(tunnel[11], "веб-сервер в системе");
            assert_eq!(tunnel[12], "порт веб-сервера");
        }
        let local = order("local");
        assert_eq!(local.len(), 16 + w);
        assert_eq!(local[15 + w], "порты рабочего места");
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

    /// Дорогие факты — те три, кому `gather` отдаёт остаток общего срока, а
    /// не весь бюджет сразу (докстрока `gather`). Чистая функция, а не три
    /// отдельные проверки подстрок — честнее (находка ревью задачи 2, круг 2).
    fn is_expensive(name: &str) -> bool {
        matches!(name, WEB | PUBLICATION | GITEA)
    }

    /// Находка ревью, круг 2: ни один тест не доказывал, что короткий общий
    /// срок не роняет факты из списка (дорогой факт с нулевым остатком
    /// обязан ответить настоящим провалом и остаться в наборе — «не
    /// проверено» таким пропавшим фактом не прикрыть, его больше нет), и что
    /// дешёвые факты в порядке всегда идут раньше дорогих — иначе дорогому
    /// достаётся чужое время раньше, чем дешёвый успел посчитаться сам.
    #[test]
    fn a_short_deadline_drops_no_fact_and_counts_cheap_facts_first() {
        let ctx = test_ctx("tunnel");
        let checks = gather(&ctx, None, PortUse::WeDont, std::time::Instant::now());
        let names: Vec<&str> = checks.iter().map(|c| c.what.as_str()).collect();
        assert_eq!(names, order("tunnel"), "короткий срок уронил факт(ы): {names:?}");

        // Перед первым дорогим фактом в порядке — хотя бы один дешёвый, и
        // сами дорогие идут в том же порядке, в котором друг за другом
        // спрашивают остаток (`WEB`, затем `PUBLICATION`, затем `GITEA`).
        let cheap_before_first_expensive = names.iter().take_while(|n| !is_expensive(n)).count();
        assert!(cheap_before_first_expensive > 0, "перед первым дорогим должны стоять дешёвые: {names:?}");
        let expensive: Vec<&str> = names.iter().copied().filter(|n| is_expensive(n)).collect();
        assert_eq!(expensive, [WEB, PUBLICATION, GITEA], "{names:?}");
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
        // Факт теперь говорит, что видит, а не памятку: открытых сеансов нет
        // — так и сказано; есть — назван их счёт и что из этого следует.
        let none = open_session_fact(0);
        assert!(none.ok && none.info && none.detail.contains("нет"), "{none:?}");
        let c = open_session_fact(1);
        assert!(c.ok && c.info, "{c:?}");
        assert!(c.detail.contains("открыт сеанс 1С") && c.detail.contains("могут отказать"), "{c:?}");
        assert!(!c.detail.starts_with("учебная"), "про учебную платформу как про данность говорить нельзя: {c:?}");
        assert!(open_session_fact(2).detail.contains("2 открыто сеанса"), "{:?}", open_session_fact(2));
    }

    /// Совет идёт по тому, что мы знаем. Открыт сеанс 1С — дело почти
    /// наверняка в нём, и посылать человека устанавливать расширение, когда
    /// он просто работает в базе, нельзя (заказчик 25.10).
    #[test]
    fn an_open_session_changes_the_advice_about_the_publication() {
        let busy = publication_fact("http://x", None, 1);
        assert!(!busy.ok, "{busy:?}");
        assert!(busy.detail.contains("открыт сеанс 1С"), "{busy:?}");
        assert!(busy.hint.contains("закройте"), "{busy:?}");
        assert!(!busy.hint.contains("install"), "установку тут советовать нечего: {busy:?}");

        // Сеансов нет — прежний совет про установку остаётся верным.
        let free = publication_fact("http://x", None, 0);
        assert!(free.hint.contains("install"), "{free:?}");
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

    /// Чужая строка `target` опаснее её отсутствия: она гасит проверку имён
    /// молча. Поэтому расхождение — провал со словами, а не тихая правка
    /// файла.
    #[test]
    fn a_stale_analyzer_line_is_named_as_such() {
        use crate::analyzer::TARGET;
        let want = crate::analyzer::expected("8.5.1.1150", "8.3.27");
        let c = analyzer_fact(Some("8.5.1"), &want, &[(TARGET, "8.3.21".to_string())]);
        assert!(!c.ok, "{c:?}");
        assert!(c.detail.contains("8.3.21") && c.detail.contains("8.5.1"), "{c:?}");
        assert!(c.hint.contains(crate::analyzer::FILE), "{c:?}");
        // Совпало — зелено и названо значением, а не словом «ок».
        let ok = analyzer_fact(Some("8.5.1"), &want, &want);
        assert!(ok.ok && !ok.info && ok.detail.contains("8.5.1"), "{ok:?}");
        // Файла нет — сведение, а не провал: его запишет install.
        let none = analyzer_fact(Some("8.5.1"), &want, &[]);
        assert!(none.ok && none.info && none.detail.contains(crate::analyzer::FILE), "{none:?}");
        // Строка не нужна вовсе (линия машины не новее каталога) — тоже
        // сведение, и оно называет платформу, а не молчит.
        let not_needed = analyzer_fact(Some("8.3.27"), &[], &[]);
        assert!(not_needed.ok && not_needed.info && not_needed.detail.contains("8.3.27"), "{not_needed:?}");
    }

    /// Исходы, которых прежний факт не различал (находки ревью задачи 36):
    /// строка `target`, которой мы бы не написали, — тихо выключенная
    /// проверка имён, и потому провал; `min`, поставленный человеком, — не
    /// расхождение вовсе (коннектор его не пишет); а без найденной платформы
    /// факт не утверждает, что сверил.
    #[test]
    fn the_fact_judges_what_is_in_the_file_not_what_we_would_write() {
        use crate::analyzer::{MIN, TARGET};
        // Платформа и каталог одной линии — `target` мы бы не писали; он в
        // файле есть и от чужой линии: гасит проверку имён молча.
        let want = crate::analyzer::expected("8.3.27.1508", "8.3.27");
        assert!(want.is_empty(), "в этом случае строка не наша");
        let c = analyzer_fact(Some("8.3.27"), &want, &[(TARGET, "8.5.1".into())]);
        assert!(!c.ok && c.detail.contains(TARGET) && c.detail.contains("8.5.1"), "{c:?}");
        // Минимум человека — ручка, а не расхождение: факт зелёный и
        // говорит, что значение задано вручную.
        let newer = crate::analyzer::expected("8.5.1.1150", "8.3.27");
        let by_hand = analyzer_fact(Some("8.5.1"), &newer, &[(MIN, "8.2.19".into()), (TARGET, "8.5.1".into())]);
        assert!(by_hand.ok && !by_hand.info, "{by_hand:?}");
        assert!(by_hand.detail.contains("8.2.19") && by_hand.detail.contains("вручную"), "{by_hand:?}");
        // Строка, которую человек удалил, не называется зелёным фактом как
        // стоящая в файле: сведение называет именно её.
        let gone = analyzer_fact(Some("8.5.1"), &newer, &[(MIN, "8.5.1".into())]);
        assert!(gone.ok && gone.info, "{gone:?}");
        assert!(gone.detail.contains(TARGET) && !gone.detail.contains(MIN), "{gone:?}");
        // Платформы нет — сверять нечем, и факт этого не скрывает.
        let blind = analyzer_fact(None, &[], &[(TARGET, "8.5.1".into())]);
        assert!(blind.ok && blind.info, "{blind:?}");
        assert!(blind.detail.contains("не сверяли") && blind.detail.contains("8.5.1"), "{blind:?}");
        assert!(analyzer_fact(None, &[], &[]).detail.contains("неизвестна"));
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
            assert!(!wants_directory(path), "{path}");
            // Та же проверка со стороны ворот — только в сборке с окном:
            // без фичи `gui` модуля нет вовсе (сборка `--no-default-features`).
            #[cfg(feature = "gui")]
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
        let ok = publication_fact("http://x", Some("1.2".into()), 0);
        assert!(ok.ok && ok.detail.contains("1.2"), "{ok:?}");
        let fail = publication_fact("http://x", None, 0);
        assert!(!fail.ok && fail.hint.contains("install"), "{fail:?}");
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

    /// Самое важное из трёх: ни «занят», ни «не поднимался» не приговор.
    /// Ровно здесь жил отказ, который заказчик видел живьём — красный факт
    /// при исправной работе (леджер, пункты 17 и 19).
    #[test]
    fn a_busy_runner_is_a_note_not_a_verdict() {
        let c = runner_fact(&RunnerState::Busy("project_push".into()));
        assert!(c.ok && c.info, "занятость — это сведение, а не отказ: {c:?}");
        assert!(c.detail.contains("занят") && c.detail.contains("project_push"), "{c:?}");
    }

    #[test]
    fn a_runner_that_talks_is_green_and_says_how_long_ago() {
        let c = runner_fact(&RunnerState::Answered(std::time::Duration::from_secs(90)));
        assert!(c.ok && !c.info, "{c:?}");
        assert!(c.detail.contains("1 мин"), "{c:?}");
    }

    #[test]
    fn a_runner_nobody_called_yet_is_a_note() {
        let c = runner_fact(&RunnerState::Idle);
        assert!(c.ok && c.info, "до первого вызова агента раннера нет — это норма: {c:?}");
        assert!(c.detail.contains("не поднимался"), "{c:?}");
    }

    #[test]
    fn ago_reads_as_seconds_minutes_and_hours() {
        use std::time::Duration;
        assert_eq!(ago(Duration::from_secs(5)), "5 с");
        assert_eq!(ago(Duration::from_secs(600)), "10 мин");
        assert_eq!(ago(Duration::from_secs(3 * 3600 + 300)), "3 ч 05 мин");
    }

    /// Пока идёт долгая операция, знание о раннере — «занят», и никакой
    /// пробы: мьютекс проекта держит именно она.
    #[test]
    fn evidence_reports_busy_while_the_project_lock_is_held() {
        let _serial = crate::project::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let held = crate::project::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match runner_evidence() {
            Some(RunnerState::Busy(_)) => {}
            other => panic!("мьютекс проекта занят, а знание о раннере {other:?}"),
        }
        drop(held);
    }

    /// Спека прямо запрещает холодный старт раннера ради факта (28 с и
    /// больше) — ни на одном проходе, не только у готовности. Без знания
    /// (мьютекс свободен, мост ни разу не слышал раннера) факт обязан стать
    /// «не поднимался» и вернуться мгновенно; второй процесс больше негде
    /// поднять — в модуле не осталось ни одного вызова, который бы это
    /// делал (`vrunner_fact`/`local::call` отсюда убраны вместе с `Limits`).
    ///
    /// «Мост молчал» подставляем явно через `evidence_from`, а не через
    /// настоящий `runner_evidence()`: тесты бриджа в этом же процессе гоняют
    /// реальных детей и на каждый кадр зовут `note_runner_reply()`, и
    /// глобальная отметка (`bridge::RUNNER_REPLY`) к этому моменту может уже
    /// быть выставлена — полагаться на её чистоту здесь нечестно. Мьютекс
    /// проекта настоящий (`busy_now`): его в этом тесте никто не держит, а
    /// `TEST_SERIAL`, как у соседнего теста, не даёт двум проверкам реального
    /// `project::LOCK` идти параллельно.
    #[test]
    fn the_runner_is_never_cold_started_for_a_fact() {
        let _serial = crate::project::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let ev = evidence_from(busy_now(), None);
        assert_eq!(ev, None, "мьютекс свободен, а мост «молчит» — ровно то условие, которое раньше поднимало пробу");
        let t0 = std::time::Instant::now();
        // То же выражение, что и в `gather`: знание или `Idle`, без пробы.
        let c = runner_fact(&ev.unwrap_or(RunnerState::Idle));
        assert!(t0.elapsed() < std::time::Duration::from_secs(2), "факт о раннере ждал: {:?}", t0.elapsed());
        assert!(c.ok && c.info && c.detail.contains("не поднимался"), "{c:?}");
    }

    /// Развилка целиком, без глобалов: занятость старше живого ответа, а без
    /// обоих знание пусто — решает вызывающий, не этот слой.
    #[test]
    fn evidence_prefers_busy_and_is_empty_without_both() {
        use std::time::Duration;
        let min = Duration::from_secs(60);
        assert_eq!(evidence_from(Some("cf_decompile".into()), Some(min)), Some(RunnerState::Busy("cf_decompile".into())));
        assert_eq!(evidence_from(None, Some(min)), Some(RunnerState::Answered(min)));
        assert_eq!(evidence_from(None, None), None);
    }
}
