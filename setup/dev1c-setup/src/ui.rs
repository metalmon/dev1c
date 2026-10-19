//! Решения окна без GPUI: что говорит карточка состояния, какая вкладка
//! открывается первой, что несёт плашка, что показывает «Доступ», есть ли
//! несохранённые правки, можно ли закрывать окно.
//!
//! Модуль собирается всегда, не только с флагом `gui`: GPUI на этой машине
//! не собирают (правило «сборка в CI»), а тесты логики должны идти локально.

use crate::cfg::{self, Cfg};
use common::check::Check;
use common::kit::Kit;
use common::op::OpState;
use common::Event;
use std::time::Duration;

/// Сколько строк журнала держим в окне.
pub const LOG_CAP: usize = 2000;
/// Журнал окна рядом с комплектом.
pub const LOG_FILE: &str = "dev1c-setup.log";
/// Текст в журнал, когда рабочий поток упал с паникой.
pub const PANIC_TEXT: &str = "внутренняя ошибка: операция прервана";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Ok,
    Warn,
    Fail,
    Say,
    /// Заголовок раздела журнала — название работы с часами.
    Head,
}

impl Kind {
    /// Значок перед строкой журнала; у `Say` и `Head` строка идёт без значка.
    pub fn mark(self) -> &'static str {
        match self {
            Kind::Ok => "✓",
            Kind::Warn => "!",
            Kind::Fail => "✗",
            Kind::Say | Kind::Head => "",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}

impl Line {
    pub fn ok(text: impl Into<String>) -> Line {
        Line { kind: Kind::Ok, text: text.into() }
    }
    pub fn say(text: impl Into<String>) -> Line {
        Line { kind: Kind::Say, text: text.into() }
    }
    pub fn fail(text: impl Into<String>) -> Line {
        Line { kind: Kind::Fail, text: text.into() }
    }
}

/// Пять вкладок прежнего окна. Порядок — порядок работы: сначала параметры,
/// потом установка, потом адреса, проверка и журнал (решение заказчика
/// 2026-10-19).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    /// Первой открывается она: полоса читается как порядок работы, и на
    /// первом запуске администратор идёт сюда. Состояние среды он видит и
    /// отсюда — карточка в шапке висит на всех вкладках.
    #[default]
    Params,
    Ops,
    Access,
    Check,
    Log,
}

impl Tab {
    /// Порядок вкладок в полосе; он же — порядок индексов `TabBar`.
    pub const ALL: [Tab; 5] = [Tab::Params, Tab::Ops, Tab::Access, Tab::Check, Tab::Log];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Ops => "Операции",
            Tab::Params => "Параметры",
            Tab::Access => "Доступ",
            Tab::Check => "Проверка",
            Tab::Log => "Журнал",
        }
    }
}

/// Что идёт прямо сейчас — от этого зависят кнопки и полоса хода.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Busy {
    Run,
    Check,
}

/// Работа, которую запускает кнопка.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Install,
    Change,
    CleanStack,
    CleanData,
    Check,
}

impl Which {
    /// Заголовок раздела в журнале перед работой.
    pub fn title(self) -> &'static str {
        match self {
            Which::Install => "— устанавливаю —",
            Which::Change => "— применяю изменения —",
            Which::CleanStack => "— снимаю контейнер —",
            Which::CleanData => "— удаляю данные —",
            Which::Check => "— проверяю —",
        }
    }
}

/// Кружок состояния: цвет говорит состояние, текст — подробность.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dot {
    /// Не сделано.
    Off,
    /// Сделано, но разошлось с параметрами.
    Busy,
    /// Сделано.
    Up,
    /// Заблокировано: сделать нельзя, пока не поправят.
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpView {
    pub id: String,
    pub title: String,
    pub dot: Dot,
    pub detail: String,
}

/// Карточка операции из её состояния.
pub fn op_view(id: String, title: String, st: OpState) -> OpView {
    let (dot, detail) = match st {
        OpState::Done(d) => (Dot::Up, d),
        OpState::Missing => (Dot::Off, "не сделано".into()),
        OpState::Drifted { from, to } => (Dot::Busy, format!("было: {from} → станет: {to}")),
        OpState::Blocked(why) => (Dot::Down, why),
    };
    OpView { id, title, dot, detail }
}

/// Параметры для окна: сломанный файл не повод не открыться — окно и есть
/// способ его поправить. Возвращает умолчания и причину, если файл не читается.
pub fn load_or_default(kit: &Kit) -> (Cfg, Option<String>) {
    match Cfg::load(kit) {
        Ok(cfg) => (cfg, None),
        Err(e) => (Cfg::default(), Some(format!("{e:#}"))),
    }
}

/// Плашка про сам файл параметров: его нет или он не прочитан. Первая
/// плашка окна — она важнее заблокированной операции, потому что операции в
/// этом случае посчитаны не по файлу.
pub fn cfg_notice(file_exists: bool, cfg_error: Option<&str>) -> Option<String> {
    match (file_exists, cfg_error) {
        (_, Some(e)) => Some(format!(
            "Параметры не прочитаны: {e}. Показаны значения по умолчанию — проверьте их на вкладке «Параметры»; «Сохранить» перезапишет файл."
        )),
        (false, None) => Some(
            "Параметров ещё нет: проверьте значения на вкладке «Параметры», сохраните и установите.".into(),
        ),
        (true, None) => None,
    }
}

/// Карточка состояния в шапке: кружок, заголовок, подстрочник и главная
/// кнопка со своей надписью. Пусто в `primary` — делать нечего, кнопки нет.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub dot: Dot,
    pub state: String,
    pub sub: String,
    pub primary: Option<(Which, &'static str)>,
}

/// Состояние среды по состояниям операций — те же пять случаев, что считало
/// старое окно. Среда «работает», когда сделана операция `container`:
/// единственное имя операции, которое окно знает (состав списка задаёт
/// `dev1c.toml`, не окно).
pub fn status_line(ops: &[OpView], name: &str) -> Card {
    let container = ops.iter().find(|o| o.id == "container");
    let running = container.is_some_and(|o| o.dot == Dot::Up);
    let todo = ops.iter().filter(|o| o.dot == Dot::Off || o.dot == Dot::Busy).count();
    let drift = ops.iter().any(|o| o.dot == Dot::Busy);
    let done = ops.iter().filter(|o| o.dot == Dot::Up).count();
    let blocked = ops.iter().find(|o| o.dot == Dot::Down);
    // Доустановить или установить — одна и та же работа, разные надписи:
    // человек должен видеть, начинает он или продолжает.
    let finish = |label| Some((Which::Install, label));

    let (dot, state, sub, primary) = if !running && done == 0 {
        (Dot::Off, "Среда не установлена", format!("сделано 0 из {}", ops.len()), finish("Установить"))
    } else if !running {
        (Dot::Busy, "Среда установлена не полностью", format!("сделано {done} из {}", ops.len()), finish("Доустановить"))
    } else if drift {
        let detail = container.map(|o| o.detail.clone()).unwrap_or_default();
        (Dot::Busy, "Среда работает, параметры разошлись", format!("контейнер {name} · {detail}"), Some((Which::Change, "Применить изменения")))
    } else if let Some(b) = blocked {
        let primary = if todo > 0 { finish("Доустановить") } else { None };
        (Dot::Busy, "Среда работает, не всё настроено", format!("{}: {}", b.title, b.detail), primary)
    } else {
        let detail = container.map(|o| o.detail.clone()).unwrap_or_default();
        let primary = if todo > 0 { finish("Доустановить") } else { None };
        (Dot::Up, "Среда работает", format!("контейнер {name} · {detail}"), primary)
    };
    Card { dot, state: state.into(), sub, primary }
}

/// Поля другой формы не уезжают в файл: скрытое поле человек очистить не
/// может, а `validate` не терпит и `file`, и `url` разом. Правило взято из
/// старого окна (`app.js`).
pub fn for_form(mut cfg: Cfg) -> Cfg {
    if cfg.model.serve == "remote" {
        cfg.model.file.clear();
    } else {
        cfg.model.url.clear();
        cfg.model.key.clear();
    }
    cfg
}

/// Записать параметры в `dev1c.toml` с комментариями образца. Проверка не
/// прошла или файл не пишется — текст ошибки, файл не тронут: набранное
/// остаётся в полях, человек правит и жмёт ещё раз.
pub fn save_to(kit: &Kit, cfg: &Cfg) -> Result<(), String> {
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    common::kit::write_text(&kit.path(cfg::FILE), &Cfg::render(cfg)).map_err(|e| format!("{}: {e:#}", cfg::FILE))
}

/// Есть ли несохранённые правки: установка идёт по файлу, не по полям.
pub fn fields_differ(saved: &Cfg, edited: &Cfg) -> bool {
    saved != edited
}

/// Почему главная кнопка не работает. Две причины, и обе про параметры:
/// несохранённые правки (установка пошла бы по старому файлу) и непрочитанный
/// файл (установка пошла бы по умолчаниям — не с тем именем и портами).
pub fn primary_blocked(dirty: bool, cfg_unread: bool) -> Option<&'static str> {
    if cfg_unread {
        Some("Параметры не прочитаны: проверьте их и сохраните")
    } else if dirty {
        Some("Сначала сохраните параметры")
    } else {
        None
    }
}

/// Размер окна при открытии: желаемый, но не больше рабочей области экрана
/// за вычетом полей. На экране 1920×1080 при масштабе 125% рабочая область
/// — около 1536×865 точек, и окно 1040×800 иначе свешивается за нижний край
/// вместе с подвалом и кнопкой «Сохранить».
pub fn window_size(want: (f32, f32), work: (f32, f32), margin: f32) -> (f32, f32) {
    let fit = |want: f32, work: f32| want.min((work - margin).max(240.0));
    (fit(want.0, work.0), fit(want.1, work.1))
}

/// Можно ли закрывать окно сразу. Пока идёт работа — нет, спрашиваем.
pub fn should_close(working: bool) -> bool {
    !working
}

/// Выбранный файл модели не `.gguf` — сказать словами, а не молча взять.
pub fn gguf_warning(path: &str) -> Option<String> {
    let ok = std::path::Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("gguf"));
    if ok {
        None
    } else {
        Some(format!("выбран не .gguf: {path}"))
    }
}

/// Строка вкладки «Доступ»: что это, чем пользоваться, пояснение.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessRow {
    pub what: &'static str,
    pub value: String,
    pub detail: String,
}

/// Если адрес этой машины не удалось узнать, в строках «Доступа» остаётся
/// заглушка — лучше честной заглушки только настоящий адрес.
pub const ADDRESS_UNKNOWN: &str = "<адрес>";

/// `<логин>@<адрес>` для команд scp в преградах: логин спрашивать не у кого
/// (сервер без интернета, ответить некому), а адрес — настоящий, если машина
/// его уже знает, и заглушка, если нет.
pub fn host_hint() -> String {
    let host = local_addresses().first().cloned().unwrap_or_else(|| ADDRESS_UNKNOWN.to_string());
    format!("<логин>@{host}")
}

/// Порядок адресов для «Доступа»: сперва адреса локальной сети (по ним
/// ходят рабочие места), потом всё остальное — например, адрес Tailscale.
/// Машина с туннелем иначе показала бы администратору адрес, по которому в
/// его сети к среде никто не подключится.
pub fn order_addresses(found: &[String]) -> Vec<String> {
    // Ярус 0 — сети, в которых стоят рабочие места. Ярус 1 — 172.16/12:
    // частная сеть, но в неё же Docker и Hyper-V кладут свои мосты, поэтому
    // ниже настоящей. Ярус 2 — всё остальное, включая Tailscale (100.64/10).
    let tier = |a: &String| {
        if a.starts_with("192.168.") || a.starts_with("10.") {
            0
        } else if a.strip_prefix("172.").and_then(|r| r.split('.').next()).and_then(|o| o.parse::<u8>().ok()).is_some_and(|o| (16..=31).contains(&o)) {
            1
        } else {
            2
        }
    };
    let mut out: Vec<String> = Vec::new();
    for want in 0..=2 {
        for a in found.iter().filter(|a| tier(a) == want) {
            // 169.254/16 — адрес, который машина дала себе сама, когда DHCP
            // не ответил: по нему не подключится никто.
            if !a.starts_with("169.254.") && !out.contains(a) {
                out.push(a.clone());
            }
        }
    }
    out
}

/// Предел памяти для поля ввода: число гигабайт, единица стоит в подписи
/// поля. Значение, которое в целых гигабайтах не выражается (`512m`),
/// показываем как есть — выдумывать за человека нечего.
pub fn memory_field(value: &str) -> String {
    let v = value.trim();
    let num = |s: &str| s.parse::<u64>().ok();
    if let Some(g) = v.strip_suffix(['g', 'G']).and_then(num) {
        return g.to_string();
    }
    if let Some(m) = v.strip_suffix(['m', 'M']).and_then(num).filter(|m| m % 1024 == 0) {
        return (m / 1024).to_string();
    }
    v.to_string()
}

/// Обратно в `dev1c.toml`: голое число — это гигабайты, к нему и дописываем
/// суффикс, который понимает движок контейнеров. Всё остальное идёт как есть.
pub fn memory_value(field: &str) -> String {
    let v = field.trim();
    match v.parse::<u64>() {
        Ok(g) => format!("{g}g"),
        Err(_) => v.to_string(),
    }
}

/// Итог сверки комплекта: сколько файлов совпало или чего не хватает.
/// Перечень обрывается: половина несовпавшего комплекта — это сотни имён, и
/// строка в журнал должна остаться читаемой.
pub fn integrity_report(ok: usize, bad: &[String]) -> Result<String, String> {
    const SHOWN: usize = 20;
    if bad.is_empty() {
        return Ok(format!("файлы комплекта: {ok} из {ok}, суммы совпали"));
    }
    let mut names = bad.iter().take(SHOWN).cloned().collect::<Vec<_>>().join("; ");
    if bad.len() > SHOWN {
        names.push_str(&format!("; и ещё {}", bad.len() - SHOWN));
    }
    Err(format!(
        "комплект скопирован не полностью или испорчен — возьмите его заново. Совпало {ok}, не совпало {}: {names}",
        bad.len()
    ))
}

/// Можно ли ещё подбирать высоту окна. Человек потянул окно за край —
/// значит высоту он выбрал сам, и спорить с ним нельзя: иначе окно
/// возвращается к прежней высоте на следующем же кадре.
pub fn should_fit(fitted: Option<f32>, current: f32) -> bool {
    match fitted {
        None => true,
        Some(h) => (current - h).abs() <= 1.0,
    }
}

/// Высота окна под содержимое вкладки: переполнение растит окно, пустота
/// ужимает. `None` — разница меньше пикселя, трогать окно незачем.
pub fn fit_height(current: f32, content: f32, viewport: f32, work_h: f32, margin: f32, slack: f32) -> Option<f32> {
    fit_height_at(current, content, viewport, work_h, 0.0, margin, slack)
}

/// То же, но с оглядкой на то, где у окна верх: вниз оно расти может только
/// до нижней границы рабочей области, иначе подвал уедет под панель задач.
pub fn fit_height_at(current: f32, content: f32, viewport: f32, work_h: f32, top: f32, margin: f32, slack: f32) -> Option<f32> {
    let room = (work_h - top - margin).max(MIN_WINDOW_H);
    let want = (current + (content - viewport) + slack).min(room).max(MIN_WINDOW_H);
    ((want - current).abs() > 1.0).then_some(want)
}

/// Ниже этого окно не ужимаем: ниже уже не окно, а щель.
pub const MIN_WINDOW_H: f32 = 560.0;

/// Адреса этой машины: тот, с которого система пошла бы наружу (UDP-сокету
/// хватает `connect`, пакетов не отправляем), плюс всё, что система знает о
/// собственном имени. Пусто — «Доступ» покажет заглушку.
pub fn local_addresses() -> Vec<String> {
    use std::net::ToSocketAddrs;
    let mut found = Vec::new();
    if let Some(ip) = std::net::UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|s| s.connect("8.8.8.8:80").ok().map(|_| s))
        .and_then(|s| s.local_addr().ok())
        .map(|a| a.ip())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
    {
        found.push(ip.to_string());
    }
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    if !host.is_empty() {
        if let Ok(addrs) = (host.as_str(), 0u16).to_socket_addrs() {
            for a in addrs.filter(|a| a.is_ipv4() && !a.ip().is_loopback()) {
                found.push(a.ip().to_string());
            }
        }
    }
    order_addresses(&found)
}

/// Адреса среды, выведенные из параметров: четыре порта подряд от
/// `server.port` и адрес сервера модели. `host` — адрес этой машины
/// (`local_address`) или заглушка; `facts` — числа последнего замера
/// локального сервера (окно, max_tokens), если он был.
pub fn access_rows(cfg: &Cfg, facts: Option<(u32, u32)>, host: &str) -> Vec<AccessRow> {
    let [rdp, gitea, _, ssh] = cfg.ports();
    let model_url = cfg.model_url();
    let row = |what: &'static str, value: String, detail: String| AccessRow { what, value, detail };
    vec![
        row("Рабочие места", format!("mstsc /v:{host}:{rdp}"), "dev1 / dev1, dev2 / dev2, dev3 / dev3".into()),
        row("Репозитории Gitea", format!("http://{host}:{gitea}"), "те же логины".into()),
        row(
            "Сервер модели",
            if model_url.is_empty() { "—".into() } else { model_url },
            match facts {
                Some((window, max_tokens)) => format!("окно слота {window} токенов, max_tokens {max_tokens}"),
                None => "замера ещё не было".into(),
            },
        ),
        row("Для 1c-connect", format!("{host}:{ssh}"), "адрес среды в 1c-connect.toml разработчика".into()),
    ]
}

/// Плашка над вкладками: первая заблокированная операция своим текстом и
/// указание, где это правят. Недоделанное плашкой не считается — это
/// обычная работа кнопки.
pub fn notice(ops: &[OpView]) -> Option<String> {
    ops.iter().find(|o| o.dot == Dot::Down).map(|o| format!("{} — вкладка «Параметры».", o.detail))
}

/// Рабочий поток обязан договорить: паника в теле всё равно шлёт конец
/// работы (иначе кнопки навсегда остались бы в «идёт работа») и строку об
/// этом в журнал. Отправитель — замыкание: так путь паники проверяется
/// тестом без канала GPUI.
pub fn guarded<E>(send: impl Fn(E), as_line: impl FnOnce(Line) -> E, body: impl FnOnce() -> E, on_panic: impl FnOnce() -> E) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(ev) => send(ev),
        Err(_) => {
            send(as_line(Line::fail(PANIC_TEXT)));
            send(on_panic());
        }
    }
}

/// Событие движка строкой журнала. `Command`/`Output` в окно не идут — им
/// место в журнале команд комплекта.
pub fn line_of(e: &Event) -> Option<Line> {
    let (kind, text) = match e {
        Event::Ok(t) => (Kind::Ok, t.clone()),
        Event::Warn(t) => (Kind::Warn, t.clone()),
        Event::Fail(t) => (Kind::Fail, t.clone()),
        Event::Say(t) => (Kind::Say, t.clone()),
        Event::Progress(p) => (Kind::Say, p.phase.title().into()),
        Event::Command(_) | Event::Output { .. } => return None,
    };
    Some(Line { kind, text })
}

/// Факты проверки строками в журнал; подсказка провалившегося — второй строкой.
pub fn check_lines(checks: &[Check]) -> Vec<Line> {
    let mut out = Vec::with_capacity(checks.len());
    for c in checks {
        // Информационный факт — не приговор: обычная строка, а не провал.
        let kind = match (c.ok, c.info) {
            (true, false) => Kind::Ok,
            (true, true) => Kind::Say,
            _ => Kind::Fail,
        };
        out.push(Line { kind, text: format!("{}: {}", c.what, c.detail) });
        if kind == Kind::Fail && !c.hint.is_empty() {
            out.push(Line { kind: Kind::Warn, text: c.hint.clone() });
        }
    }
    out
}

/// Сколько шла работа, для «Готово за …»: меньше минуты — секунды, иначе
/// минуты и секунды. Секунды целые, без округления вверх.
pub fn took(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs} с")
    } else {
        format!("{} мин {} с", secs / 60, secs % 60)
    }
}

/// Местное время `HH:MM` для заголовка раздела журнала.
#[cfg(windows)]
pub fn clock() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = SYSTEMTIME { wYear: 0, wMonth: 0, wDayOfWeek: 0, wDay: 0, wHour: 0, wMinute: 0, wSecond: 0, wMilliseconds: 0 };
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

#[cfg(not(windows))]
pub fn clock() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60)
}

/// Заголовок раздела журнала: название работы и час её начала.
pub fn head_line(title: &str) -> Line {
    Line { kind: Kind::Head, text: format!("{title} · {}", clock()) }
}

/// Строка журнала на диск, тем же текстом, что рисует окно, но без цвета:
/// значок и пробел перед строкой, у заголовка раздела — пустая строка перед
/// ним. Ошибки записи молча игнорируются: журнал на диске не обязателен.
pub fn write_log_file(kit: &Kit, line: &Line) {
    use std::io::Write;
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(kit.path(LOG_FILE)) else { return };
    let text = if line.kind == Kind::Head { format!("\n{}\n", line.text) } else { format!("{:<2}{}\n", line.kind.mark(), line.text) };
    let _ = f.write_all(text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit_with(text: Option<&str>) -> Kit {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("dev1c-setup-ui-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(t) = text {
            std::fs::write(dir.join(cfg::FILE), t).unwrap();
        }
        Kit::at(&dir)
    }

    fn op(id: &str, st: OpState) -> OpView {
        op_view(id.into(), id.into(), st)
    }

    #[test]
    fn a_broken_config_file_yields_defaults_and_the_reason() {
        let kit = kit_with(Some("[server]\nname =\n"));
        let (cfg, err) = load_or_default(&kit);
        assert_eq!(cfg.server.name, Cfg::default().server.name);
        assert!(err.unwrap().contains(cfg::FILE), "в тексте ошибки должно быть имя файла");
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn a_good_config_file_yields_no_reason() {
        let kit = kit_with(Some("[server]\nname = \"other\"\n"));
        let (cfg, err) = load_or_default(&kit);
        assert_eq!(cfg.server.name, "other");
        assert!(err.is_none());
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn a_missing_config_file_is_not_an_error() {
        let kit = kit_with(None);
        let (_, err) = load_or_default(&kit);
        assert!(err.is_none());
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn the_notice_carries_the_text_of_the_first_blocked_operation() {
        let ops = vec![
            op("env", OpState::Done("docker 27".into())),
            op("model", OpState::Blocked("модель не выбрана: [model] file".into())),
        ];
        assert!(notice(&ops).unwrap().starts_with("модель не выбрана: [model] file"));
        assert!(notice(&[op("env", OpState::Missing)]).is_none());
    }

    #[test]
    fn the_tab_strip_reads_in_the_order_of_work() {
        // Полоса вкладок и индексы, по которым приходит щелчок, — один
        // список: разъедутся они молча, поэтому порядок закреплён тестом.
        assert_eq!(Tab::ALL.map(|t| t.title()), ["Параметры", "Операции", "Доступ", "Проверка", "Журнал"]);
    }

    #[test]
    fn the_primary_button_waits_for_unsaved_edits_and_for_unread_parameters() {
        assert_eq!(primary_blocked(false, false), None);
        assert_eq!(primary_blocked(true, false), Some("Сначала сохраните параметры"));
        // Непрочитанный файл опаснее несохранённого поля: установка по
        // умолчаниям создала бы контейнер не с тем именем и портами.
        assert_eq!(primary_blocked(false, true), Some("Параметры не прочитаны: проверьте их и сохраните"));
        assert_eq!(primary_blocked(true, true), Some("Параметры не прочитаны: проверьте их и сохраните"));
    }

    #[test]
    fn the_plaque_speaks_both_when_the_file_is_missing_and_when_it_is_broken() {
        assert!(cfg_notice(true, None).is_none());
        let missing = cfg_notice(false, None).unwrap();
        assert!(missing.contains("Параметров ещё нет"), "{missing}");
        assert!(missing.contains("Параметры"), "{missing}");
        let broken = cfg_notice(true, Some("dev1c.toml: ошибка в файле параметров")).unwrap();
        assert!(broken.contains("ошибка в файле параметров"), "{broken}");
        assert!(broken.contains("Сохранить"), "о перезаписи файла надо предупредить: {broken}");
    }

    #[test]
    fn the_notice_points_at_the_tab_that_fixes_it() {
        let ops = vec![op("model", OpState::Blocked("модель не выбрана: [model] file".into()))];
        let text = notice(&ops).unwrap();
        assert!(text.starts_with("модель не выбрана: [model] file"), "{text}");
        assert!(text.contains("Параметры"), "{text}");
    }

    #[test]
    fn normalising_the_form_twice_changes_nothing_the_second_time() {
        // Иначе окно открывалось бы «изменённым» на честном файле: поля
        // сравниваются с сохранённым, и то, и другое нормализовано.
        let mut c = Cfg::default();
        c.model.serve = "host".into();
        c.model.key = "остался от remote".into();
        let once = for_form(c);
        assert_eq!(for_form(once.clone()), once);
    }

    #[test]
    fn a_panicking_worker_still_reports_the_end_of_work_and_a_line() {
        let sent = std::sync::Mutex::new(Vec::<String>::new());
        guarded(
            |e: String| sent.lock().unwrap().push(e),
            |l: Line| l.text,
            || panic!("boom"),
            || "done".to_string(),
        );
        assert_eq!(*sent.lock().unwrap(), vec![PANIC_TEXT.to_string(), "done".to_string()]);
    }

    #[test]
    fn a_working_worker_sends_only_its_own_event() {
        let sent = std::sync::Mutex::new(Vec::<String>::new());
        guarded(|e: String| sent.lock().unwrap().push(e), |l: Line| l.text, || "ok".to_string(), || "done".to_string());
        assert_eq!(*sent.lock().unwrap(), vec!["ok".to_string()]);
    }

    /// Карточка состояния коротко: заголовок и надпись главной кнопки.
    fn card(ops: &[OpView]) -> (String, Option<&'static str>) {
        let c = status_line(ops, "dev1c");
        (c.state, c.primary.map(|(_, label)| label))
    }

    #[test]
    fn the_status_card_names_the_state_and_the_next_step() {
        let missing = |id: &str| op(id, OpState::Missing);
        let done = |id: &str| op(id, OpState::Done("готово".into()));
        let drift = |id: &str| op(id, OpState::Drifted { from: "3390".into(), to: "4000".into() });
        let blocked = |id: &str| op(id, OpState::Blocked("модель не выбрана".into()));

        assert_eq!(card(&[missing("container"), missing("image")]), ("Среда не установлена".into(), Some("Установить")));
        assert_eq!(card(&[done("image"), missing("container")]), ("Среда установлена не полностью".into(), Some("Доустановить")));
        assert_eq!(card(&[done("container"), drift("image")]), ("Среда работает, параметры разошлись".into(), Some("Применить изменения")));
        assert_eq!(card(&[done("container"), blocked("model")]), ("Среда работает, не всё настроено".into(), None));
        assert_eq!(card(&[done("container"), done("image")]), ("Среда работает".into(), None));
    }

    #[test]
    fn a_blocked_environment_with_work_left_still_offers_to_finish_it() {
        let ops = vec![
            op("container", OpState::Done("запущен".into())),
            op("model", OpState::Blocked("модель не выбрана".into())),
            op("image", OpState::Missing),
        ];
        let c = status_line(&ops, "dev1c");
        assert_eq!(c.primary.map(|(w, l)| (w, l)), Some((Which::Install, "Доустановить")));
        assert_eq!(c.sub, "model: модель не выбрана");
    }

    #[test]
    fn the_subline_counts_what_is_done_until_the_container_runs() {
        let ops = vec![op("image", OpState::Done("загружен".into())), op("container", OpState::Missing)];
        assert_eq!(status_line(&ops, "dev1c").sub, "сделано 1 из 2");
    }

    #[test]
    fn a_running_environment_shows_the_container_name_and_its_detail() {
        let ops = vec![op("container", OpState::Done("запущен".into()))];
        assert_eq!(status_line(&ops, "dev1c").sub, "контейнер dev1c · запущен");
    }

    #[test]
    fn the_form_of_the_model_decides_which_fields_travel() {
        // Скрытое поле человек очистить не может, поэтому его чистит форма:
        // иначе validate увидит и file, и url. Правило из старого app.js.
        let mut remote = Cfg::default();
        remote.model.serve = "remote".into();
        remote.model.file = "C:\\m.gguf".into();
        remote.model.url = "https://m/v1".into();
        remote.model.name = "Q".into();
        let remote = for_form(remote);
        assert_eq!(remote.model.file, "");
        assert!(remote.validate().is_ok(), "{:?}", remote.validate().err());

        let mut host = Cfg::default();
        host.model.serve = "host".into();
        host.model.file = "C:\\m.gguf".into();
        host.model.url = "https://m/v1".into();
        host.model.key = "k".into();
        let host = for_form(host);
        assert_eq!((host.model.url.as_str(), host.model.key.as_str()), ("", ""));
        assert!(host.validate().is_ok(), "{:?}", host.validate().err());
    }

    #[test]
    fn unsaved_edits_are_seen_and_block_the_primary_button() {
        let saved = Cfg::default();
        let mut edited = saved.clone();
        edited.server.port = 4000;
        assert!(fields_differ(&saved, &edited));
        assert!(!fields_differ(&saved, &saved.clone()));
    }

    #[test]
    fn a_rejected_config_keeps_the_file_and_returns_the_russian_reason() {
        let kit = kit_with(Some("[server]\nname = \"keep\"\n"));
        let mut bad = Cfg::default();
        bad.model.agents = 9;
        let e = save_to(&kit, &bad).unwrap_err();
        assert!(e.contains("agents"), "{e}");
        assert_eq!(std::fs::read_to_string(kit.path(cfg::FILE)).unwrap(), "[server]\nname = \"keep\"\n");
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn an_unwritable_file_is_reported_not_swallowed() {
        let kit = kit_with(Some("[server]\nname = \"keep\"\n"));
        let path = kit.path(cfg::FILE);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();
        let e = save_to(&kit, &Cfg::default()).unwrap_err();
        assert!(e.contains(cfg::FILE), "в тексте ошибки должно быть имя файла: {e}");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&path, perms).unwrap();
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn a_saved_config_lands_in_the_file_with_the_sample_comments() {
        let kit = kit_with(None);
        let mut c = Cfg::default();
        c.server.port = 4000;
        save_to(&kit, &c).unwrap();
        let text = std::fs::read_to_string(kit.path(cfg::FILE)).unwrap();
        assert!(text.contains("4000"));
        assert!(text.contains('#'), "комментарии образца должны остаться");
        let _ = std::fs::remove_dir_all(&kit.root);
    }

    #[test]
    fn the_window_never_opens_taller_than_the_work_area() {
        // Просторный экран: берём желаемый размер как есть.
        assert_eq!(window_size((1040., 800.), (2560., 1400.), 48.), (1040., 800.));
        // Рабочая область ниже окна — ужимаем, иначе подвал с «Сохранить»
        // оказывается за краем экрана.
        let (w, h) = window_size((1040., 800.), (1536., 700.), 48.);
        assert_eq!(w, 1040.);
        assert_eq!(h, 652.);
        // Совсем маленький экран не делает окно меньше рабочего минимума.
        assert_eq!(window_size((1040., 800.), (300., 200.), 48.), (300. - 48., 240.));
    }

    #[test]
    fn closing_is_free_when_idle_and_asks_while_working() {
        assert!(should_close(false));
        assert!(!should_close(true));
    }

    #[test]
    fn a_picked_model_file_must_be_a_gguf() {
        assert!(gguf_warning("C:\\models\\q.gguf").is_none());
        assert!(gguf_warning("C:\\models\\q.bin").unwrap().contains(".gguf"));
    }

    #[test]
    fn the_window_stops_fitting_once_the_person_resized_it() {
        // Подгонка не спорит с человеком: он потянул за край — больше не лезем.
        assert!(should_fit(None, 800.));
        assert!(should_fit(Some(800.), 800.5));
        assert!(!should_fit(Some(800.), 900.));
    }

    #[test]
    fn fitting_follows_the_content_and_respects_the_screen() {
        // Содержимое выше видимой области — окно растёт на разницу с запасом.
        assert_eq!(fit_height(800., 700., 600., 1000., 24., 8.), Some(908.));
        // Пустота под содержимым — ужимается.
        assert_eq!(fit_height(900., 500., 700., 1000., 24., 8.), Some(708.));
        // Подогнали — и на следующем кадре видимая область уже выросла на ту
        // же разницу: второй подгонки не происходит, окно стоит.
        assert_eq!(fit_height(800., 600., 600., 1000., 24., 8.), Some(808.));
        assert_eq!(fit_height(808., 600., 608., 1000., 24., 8.), None);
        // Расти некуда: окно уже упирается в низ рабочей области.
        assert_eq!(fit_height(800., 2000., 600., 1000., 24., 8.), Some(976.));
        // И с учётом того, где у окна верх: ниже рабочей области не вылезаем.
        assert_eq!(fit_height_at(800., 2000., 600., 1000., 300., 24., 8.), Some(676.));
    }

    #[test]
    fn a_damaged_kit_names_at_most_twenty_files() {
        let ok = integrity_report(42, &[]).unwrap();
        assert!(ok.contains("42"), "{ok}");
        let many: Vec<String> = (0..50).map(|i| format!("нет: f{i}")).collect();
        let bad = integrity_report(0, &many).unwrap_err();
        assert!(bad.contains("f0") && bad.contains("f19"), "{bad}");
        assert!(!bad.contains("f20"), "перечень должен обрываться: {bad}");
        assert!(bad.contains("и ещё 30"), "{bad}");
    }

    #[test]
    fn memory_shows_as_a_number_and_goes_back_with_its_unit() {
        // В поле — число, единица стоит в подписи; в файл уходит то, что
        // понимает движок контейнеров.
        assert_eq!(memory_field("24g"), "24");
        assert_eq!(memory_field("16G"), "16");
        assert_eq!(memory_field("24576m"), "24");
        assert_eq!(memory_value("24"), "24g");
        assert_eq!(memory_value(" 32 "), "32g");
        // Значение не в целых гигабайтах не выдумываем: показываем как есть
        // и так же возвращаем.
        assert_eq!(memory_field("512m"), "512m");
        assert_eq!(memory_value("512m"), "512m");
        assert_eq!(memory_field(""), "");
        assert_eq!(memory_value(""), "");
    }

    #[test]
    fn the_lan_address_comes_before_the_tunnel_one() {
        // На машине с Tailscale наружу система ходит через 100.64/10, но
        // рабочие места в сети клиента подключаются по локальному адресу.
        let found = vec!["100.77.83.96".to_string(), "192.168.1.10".to_string(), "100.77.83.96".to_string()];
        assert_eq!(order_addresses(&found), vec!["192.168.1.10", "100.77.83.96"]);
        // Мосты Docker и Hyper-V живут в 172.16/12 и не должны обгонять
        // настоящую сеть: у них свой, второй ярус.
        assert_eq!(order_addresses(&["172.17.0.1".into(), "10.0.5.7".into()]), vec!["10.0.5.7", "172.17.0.1"]);
        assert_eq!(order_addresses(&["172.17.0.1".into(), "100.77.83.96".into()]), vec!["172.17.0.1", "100.77.83.96"]);
        // 172.32 — уже не частная сеть, в начало не лезет.
        assert_eq!(order_addresses(&["172.32.0.1".into(), "172.20.0.5".into()]), vec!["172.20.0.5", "172.32.0.1"]);
        // Адрес без DHCP (169.254/16) не адрес вовсе.
        assert_eq!(order_addresses(&["169.254.3.4".into(), "192.168.1.10".into()]), vec!["192.168.1.10"]);
        assert!(order_addresses(&[]).is_empty());
    }

    #[test]
    fn the_access_rows_follow_the_port_block_and_the_model_url() {
        let mut c = Cfg::default();
        c.server.port = 4000;
        let rows = access_rows(&c, None, "192.168.1.10");
        let values: Vec<_> = rows.iter().map(|r| r.value.clone()).collect();
        assert!(values.iter().any(|v| v.contains("192.168.1.10")), "настоящий адрес машины: {values:?}");
        assert!(values.iter().any(|v| v.contains("4000")), "RDP: {values:?}");
        assert!(values.iter().any(|v| v.contains("4001")), "Gitea: {values:?}");
        assert!(values.iter().any(|v| *v == c.model_url()), "модель: {values:?}");
        assert!(values.iter().any(|v| v.contains("4003")), "SSH для 1c-connect: {values:?}");
    }

    #[test]
    fn the_model_row_says_whether_the_server_was_measured() {
        let c = Cfg::default();
        let rows = access_rows(&c, None, ADDRESS_UNKNOWN);
        let model = rows.iter().find(|r| r.what == "Сервер модели").unwrap();
        assert_eq!(model.detail, "замера ещё не было");
        let rows = access_rows(&c, Some((65536, 8192)), ADDRESS_UNKNOWN);
        let model = rows.iter().find(|r| r.what == "Сервер модели").unwrap();
        assert_eq!(model.detail, "окно слота 65536 токенов, max_tokens 8192");
    }

    #[test]
    fn the_journal_line_of_an_event_keeps_the_kind_and_drops_the_command() {
        assert_eq!(line_of(&Event::Ok("готово".into())), Some(Line::ok("готово")));
        assert_eq!(line_of(&Event::Command("docker ps".into())), None);
    }

    #[test]
    fn how_long_it_took_reads_in_russian() {
        assert_eq!(took(Duration::from_secs(9)), "9 с");
        assert_eq!(took(Duration::from_secs(61)), "1 мин 1 с");
    }
}
