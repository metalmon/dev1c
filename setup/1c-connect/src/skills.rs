//! Скиллы агентов: готовые тексты про 1С, которые галочка в трее
//! («Локальные агенты») кладёт в каталог скилла агента — второе дело
//! галочки, рядом с записями MCP. Правило переносится, а не придумывается
//! заново: оно уже работает в точке входа образа с 14.10 (`config/entrypoint.sh`,
//! цикл по `/etc/skel/.agents/skills`) — «правь готовые и заводи новые»
//! значит, что затирать нельзя никогда, кроме случая «комплект обновился, а
//! человек место не трогал». Дом агента, отпечаток каталога скилла и вердикт
//! по отпечатку — фундамент; `look`/`install`/`remove` — сама раскладка
//! поверх него.
//!
//! Каталог, в который мы лезем, принадлежит человеку и может содержать что
//! угодно — в том числе ссылку (на Windows и соединение `mklink /J`, его
//! заводит обычная учётка без прав администратора; на Unix — обычную
//! символьную ссылку, тоже без привилегий). Ревью 30.10 нашло две дыры
//! именно в этом: запись по ссылке на месте отпечатка и обход без защиты от
//! циклов. Обе закрыты здесь же, первоисточником (`[ -L ]` в `entrypoint.sh`)
//! и общим `common::integrity::walk`, который зовётся со строгой политикой
//! (`Links::Refuse`) — в отличие от сверки комплекта (`write_list`), где
//! ссылки свои и `Links::Follow` как и было всегда.

use crate::editors;
use common::integrity::{rel, sha256_file, sha256_text, walk, Links};
use common::kit::Kit;
use std::path::{Path, PathBuf};

/// Имя папки со скиллами внутри комплекта — раскладка найдёт её в комплекте
/// тем же путём, что и всё остальное привезённое (мост, инструменты).
pub const KIT_DIR: &str = "skills";

/// Чей дом: имя агента задаёт только то, какая подпапка у него в доме
/// человека — какой каталог назначения туда класть, решает вызывающий код.
/// `look`/`install`/`remove` этот признак не берут параметром вовсе (круг 1
/// ревью 31.10, Подпись): после того как отпечаток стал зависеть от суммы
/// самого каталога назначения (`stamp_path`), отдельный тег дома ничего не
/// различал, зато мог разойтись с реальным путём незаметно для компилятора.
/// `Home`/`home_dir` остаются — их зовёт вызывающий код, которому нужно
/// получить путь, прежде чем передать его сюда.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    Agents,
    Claude,
}

/// `~/.agents/skills` или `~/.claude/skills` — `~` это `USERPROFILE` на
/// Windows и `HOME` иначе, как везде в проекте (`editors.rs::home_dir` и
/// соседи). `None`, если переменная не задана: раскладывать скиллы тогда
/// попросту некуда, и это не повод паниковать здесь — решает вызывающий.
pub fn home_dir(h: Home) -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let sub = match h {
        Home::Agents => ".agents",
        Home::Claude => ".claude",
    };
    std::env::var_os(var).map(|home| PathBuf::from(home).join(sub).join("skills"))
}

/// Отпечаток каталога скилла — той же мыслью, что `dir_sum` в точке входа
/// образа (`find … -exec md5sum {} + | sort | md5sum`): сумма каждого файла
/// по имени относительно каталога, список отсортирован по имени — не по
/// порядку обхода диска, который ничем не гарантирован, — и уже
/// отсортированный список хешируется снова, той же функцией (`sha256_text`),
/// что и файлы (`sha256_file`): второй независимый алгоритм хеширования ради
/// одной сверки был бы лишней зависимостью без причины. Обход — общий
/// `common::integrity::walk` со строгой политикой (`Links::Refuse`, в
/// отличие от `write_list`, который доверяет собственным ссылкам комплекта):
/// он откажет, если внутри каталога скилла обнаружится ссылка или
/// циклическое соединение (Critical 2 ревью 30.10) — тот же отказ, что у
/// ссылки на месте скилла, и по той же причине. Верхний каталог проверяется
/// тем же способом отдельно: `walk` видит только то, что внутри, а функция
/// публичная — следующая задача будет звать её напрямую, минуя `verdict`,
/// где эта проверка уже есть (Important 4 ревью 31.10, круг 2).
///
/// Круг 4, пункт 3: приватная, а не `pub`. Вне `skills.rs` её не зовёт никто
/// (упоминания в `common/src/integrity.rs` — только в комментариях), а
/// барьер типа `State` полон лишь тогда, когда наружу не торчат и функции,
/// берущие голый `&Path`. Следующей задаче вернуть видимость — одна строка.
fn dir_sum(dir: &Path) -> Result<String, String> {
    if is_symlink(dir) {
        return Err(format!("{} — ссылка: отпечаток не считаю", dir.display()));
    }
    let mut files = Vec::new();
    // `{e:#}` — не `{e}`: у anyhow голый Display печатает только внешний
    // контекст (путь), а причина ОС («Отказано в доступе») живёт в цепочке
    // и видна лишь через альтернативный формат (как `cfg.rs`/`gui/mod.rs`,
    // Important 3 ревью 31.10, круг 2).
    walk(dir, Links::Refuse, &mut files).map_err(|e| format!("{e:#}"))?;
    let mut lines: Vec<(String, String)> = Vec::new();
    for p in &files {
        let sum = sha256_file(p).map_err(|e| format!("{e:#}"))?;
        lines.push((rel(dir, p), sum));
    }
    lines.sort();
    let text: String = lines.iter().map(|(r, s)| format!("{s}  {r}\n")).collect();
    Ok(sha256_text(&text))
}

/// Простое имя скилла — без разделителей пути и не «.»/«..»: оно идёт прямо
/// в имя файла отпечатка, и `../..` увело бы его из каталога состояния куда
/// угодно (Important 4 ревью 30.10). Разрешены буквы, цифры, дефис,
/// подчёркивание и точка (внутри имени, не как имя целиком) — только ASCII:
/// все имена скиллов в комплекте сейчас латиницей (`cf-edit`, `form-add` и
/// прочие), так что задаче 3 это не мешает, но русское имя скилла правило
/// придётся расширить (к сведению ревью 31.10, круг 2, правки не требует).
fn valid_skill_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Устойчивый вид пути — так, чтобы два разных написания одного каталога
/// (другой регистр `USERPROFILE` на Windows, лишний `.` в пути, подставленный
/// диск) давали один и тот же отпечаток, а не два разных (Critical 2 ревью
/// 31.10, круг 1). `fs::canonicalize` решает это полностью, но требует
/// существующего пути — а каталог назначения обычно ещё не создан при первой
/// раскладке. Тогда канонизируем самого глубокого СУЩЕСТВУЮЩЕГО предка (он у
/// любого пути внутри дома агента есть — сам дом человека), а остаток, пока
/// не созданный раскладкой, дописываем как есть: канонизировать там нечего,
/// он ничем не отличается от буквального текста, пока на диске не появился.
/// Результат приводим к нижнему регистру только на Windows: там путь
/// нечувствителен к регистру, и у двух разных написаний — то же самое место
/// на диске; на Unix регистр значащий, и смешивать «Путь» с «путь» было бы
/// уже дырой, а не починкой.
fn stable_path(path: &Path) -> String {
    let mut existing = path;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let canon = loop {
        if let Ok(found) = std::fs::canonicalize(existing) {
            break Some(found);
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                existing = parent;
            }
            _ => break None,
        }
    };
    let text = match canon {
        Some(mut base) => {
            for part in tail.into_iter().rev() {
                base.push(part);
            }
            base.to_string_lossy().into_owned()
        }
        // Ни один предок не существует (путь — обрывок без корня) —
        // канонизировать нечем, берём путь как есть.
        None => path.to_string_lossy().into_owned(),
    };
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// Каталог отпечатков раскладки — ТИП с приватным полем, не голый `PathBuf`
/// (круговое ревью задачи 5, круг 3, пункт 1): прежде барьер стоял только в
/// `verify.rs` поверх обёрток, а сами `look`/`install`/`remove` были
/// публичны и брали `&Path` — три строки в любом другом модуле (например
/// `gui/mod.rs`) собирали путь напрямую (`kit.state_path(skills::KIT_DIR)`)
/// и звали их, минуя барьер целиком; на Windows это сам комплект, где папка
/// `skills/` уже есть, и обе критические находки круга 1 вернулись бы.
/// Теперь тип и единственный публичный конструктор (`state()` ниже) живут В
/// ТОМ ЖЕ модуле, что сами `look`/`install`/`remove`, и эти функции берут
/// `&State`, а не `&Path`, — подставить свой путь невозможно совсем, не
/// только неудобно. Вне крейта `skills::` никто не зовёт, сужение ничего не
/// ломает.
#[derive(Debug, Clone)]
pub struct State(PathBuf);

impl State {
    /// `pub(crate)`, не только для этого модуля: читать уже готовый путь
    /// (вывести в сообщение, сравнить) безопасно откуда угодно в крейте —
    /// барьер стоит на КОНСТРУКТОРЕ (приватное поле), не на чтении.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

/// `#[cfg(test)]` — не компилируется в обычную сборку вовсе, значит ни
/// `gui/mod.rs`, ни любой другой боевой код этим путём барьер не обойдёт;
/// виден он только тестам этого крейта (`verify::tests` в их числе —
/// собрать `State` с временным каталогом им нужно, а переменные среды
/// настоящей машины трогать запрещают правила задачи).
#[cfg(test)]
pub(crate) fn state_for_test(path: PathBuf) -> State {
    State(path)
}

/// Где лежат отпечатки на самой машине — `ops_host::state_dir()`, НЕ каталог
/// кита (Critical 1 кругового ревью: на Windows `Kit::state()` окна — это
/// сам комплект) и НЕ его корень напрямую (Critical, круг 2, пункт 2: там же
/// `secret.key`, `params.toml`, `known_hosts`) — свой подкаталог, и имя у
/// него не `"skills"`, чтобы не совпасть с `KIT_DIR` даже по случайности.
///
/// `Result` (круг 4, решение владельца процесса): она зовёт `state_dir()`
/// САМА, отдельно от `kit.state()`, и на ЛЮБОЙ системе — значит своего
/// отказа ей не миновать, и переносить его на старт программы (где он
/// отказывал заодно `example` и `mcp`) было неверно. Пустая переменная среды
/// увела бы отпечатки в системный временный каталог, где их подменит любой
/// сосед по машине, а подменённый отпечаток — это «скилл не правили»
/// (`Verdict::Untouched`) про чужой текст в доме агента. Текст причины —
/// общий с `ensure_state` (`ops_host::STATE_DIR_REFUSAL`).
pub fn state() -> Result<State, String> {
    crate::ops_host::refuse_guessed_state_dir()?;
    Ok(State(crate::ops_host::state_dir().join("skill-stamps")))
}

/// Где лежит отпечаток последней раскладки — в состоянии программы, не в
/// доме агента. Имя файла несёт сумму устойчивого вида каталога НАЗНАЧЕНИЯ
/// (`stable_path`, не буквальный текст пути — Critical 2 ревью 31.10, круг 1)
/// и имя скилла — без каталога назначения в ключе два разных места делили бы
/// один отпечаток, и чужой скилл сочли бы нашим (Important 3 ревью 30.10).
/// Признака дома (`agents`/`claude`) в имени больше нет (круг 1, Подпись):
/// сумма пути уже различает любые два места сама, отдельный тег только мог
/// разойтись с ним незаметно. Отказ, если имя скилла не простое (Important 4).
///
/// `state: &State` (круговое ревью, круг 3, пункт 1) — та же защита, что у
/// `look`/`install`/`remove`.
fn stamp_path(state: &State, dst: &Path, skill: &str) -> Result<PathBuf, String> {
    if !valid_skill_name(skill) {
        return Err(format!("{skill:?} — не простое имя скилла, отпечаток не строю"));
    }
    let dst_sum = &sha256_text(&stable_path(dst))[..12];
    Ok(state.path().join(format!("skill_{dst_sum}_{skill}")))
}

/// `std::fs::symlink_metadata`, не `metadata`: вторая идёт по ссылке на цель,
/// первая — нет (тем же ходом, что `llama.rs::ensure_dir`).
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

/// Ссылка на месте файла отпечатка — отказ раньше любого чтения или записи:
/// `fs::read_to_string`/`fs::write` идут по ссылке в её цель, и висячая или
/// подставная ссылка на месте отпечатка привела бы нас в чужой файл (Critical
/// 1 ревью 30.10, тем же рассуждением, что у ссылки на месте скилла).
fn refuse_if_link(path: &Path) -> Result<(), String> {
    if is_symlink(path) {
        return Err(format!("{} — ссылка, а не файл отпечатка: не читаю и не пишу по ней", path.display()));
    }
    Ok(())
}

/// Прочитанный отпечаток — `Ok(None)`, если файла нет: первой раскладке так
/// и положено, прошлого отпечатка у неё нет.
/// Приватная (круг 4, пункт 3) — тем же доводом, что `dir_sum`.
fn read_stamp(path: &Path) -> Result<Option<String>, String> {
    refuse_if_link(path)?;
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("не прочитать {}: {e}", path.display())),
    }
}

/// Записать отпечаток после раскладки. Приватная (круг 4, пункт 3).
fn write_stamp(path: &Path, sum: &str) -> Result<(), String> {
    refuse_if_link(path)?;
    std::fs::write(path, sum).map_err(|e| format!("не записать {}: {e}", path.display()))
}

/// Три случая и отказ — как в точке входа образа: каталога нет → `New`; есть,
/// и отпечаток совпал с прежним → `Untouched`; есть, и не совпал (в том числе
/// отпечатка не было вовсе) → `Edited` (человек правил скилл на месте, или
/// отпечаток не наш — разницы между ними правило не видит, и не должно:
/// Important 3 ревью 31.10, круг 1); на месте скилла ссылка → `Link`, отказ
/// раньше прочих случаев — висячая ссылка не видна `exists()`, а запись
/// пошла бы по ней в чужую цель.
///
/// Приватный (круг 5, пункт 5): `pub` был снабжён обоснованием «его читают
/// тесты `verify.rs`» — неправдой, `grep` по крейту чистый, вне `skills.rs`
/// его не читает никто. Врущий комментарий дороже лишнего `pub`: по нему
/// принимают решения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    New,
    Untouched,
    Edited,
    Link,
}

/// Вердикт для каталога `dst` по его прежнему отпечатку `stamp`. Приватная
/// (круг 4, пункт 3) — как и сам `Verdict` (круг 5, пункт 5).
fn verdict(dst: &Path, stamp: Option<&str>) -> Result<Verdict, String> {
    if is_symlink(dst) {
        return Ok(Verdict::Link);
    }
    if !dst.exists() {
        return Ok(Verdict::New);
    }
    let now = dir_sum(dst)?;
    Ok(match stamp {
        Some(s) if s == now => Verdict::Untouched,
        _ => Verdict::Edited,
    })
}

/// Имена скиллов в комплекте — подкаталоги `kit.path(KIT_DIR)`, отсортированы
/// по имени, чтобы порядок обхода диска не влиял на то, в каком порядке
/// `look`/`install`/`remove` их переберут. Комплекта без каталога `skills/` —
/// отказ словами, а не `Found::Missing` (Important 5 ревью 31.10, круг 1):
/// это значение окно печатает как «агента на машине нет» про РЕДАКТОР
/// (`panel.rs::found_text`) — тот же тип результата, совсем другой смысл, и
/// читатель журнала получил бы неправду про установленную программу. Записи
/// MCP этот отказ не касается — их половина галочки считается отдельно
/// (решение хозяина). Имя скилла не по правилу (`valid_skill_name`) — тоже
/// отказ словами: пропускать его молча нельзя, а раз оно внутри самого
/// комплекта, это неисправность комплекта, а не дома агента.
fn kit_skill_names(kit: &Kit) -> Result<Vec<String>, String> {
    let dir = kit.path(KIT_DIR);
    if !dir.exists() {
        return Err(format!("{} в комплекте нет — скиллов не ставим и не снимаем, записи MCP это не касается", dir.display()));
    }
    let mut names = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("не прочитать {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("не прочитать {}: {e}", dir.display()))?;
        // `DirEntry::file_type` смотрит на саму запись и не идёт по ссылке —
        // этим и ловим ссылку на месте скилла В КОМПЛЕКТЕ. Молчать здесь
        // нельзя (находка ре-ревью, круг 3): такой скилл просто исчез бы из
        // списка имён — `look` решил бы, что всё наше на месте, `remove`
        // потерял бы его из виду, и каталог с отпечатком остался бы сиротой
        // в доме человека. Отказ словами, тем же приёмом, что чуть ниже для
        // неправильного имени скилла.
        let file_type = entry.file_type().map_err(|e| format!("не прочитать тип {}: {e}", entry.path().display()))?;
        if file_type.is_symlink() {
            return Err(format!("{} в комплекте — ссылка на месте скилла, раскладку не веду", entry.path().display()));
        }
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !valid_skill_name(&name) {
            return Err(format!("{name:?} в комплекте — не простое имя скилла, раскладку не веду"));
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

/// Итоговое слово о скиллах в доме — тем же словарём, что у MCP-записей
/// редактора (`editors::Found`), чтобы окно показывало обе половины галочки
/// одним языком: хоть один скилл занят чужим — `Foreign` с именами,
/// раскладку это не останавливает, но и не трогает их; иначе все наши и на
/// месте — `Ours`; иначе (что-то ещё не поставлено) — `Absent`. Общая для
/// `look`/`install`/`remove`, чтобы три функции не придумывали три разных
/// способа сказать одно и то же.
fn summarize(total: usize, foreign: Vec<String>, ours: usize) -> editors::Found {
    if !foreign.is_empty() {
        editors::Found::Foreign(foreign)
    } else if total > 0 && ours == total {
        editors::Found::Ours
    } else {
        editors::Found::Absent
    }
}

/// Имя скилла в `Foreign`: короткое, если отпечаток был и просто не совпал —
/// человек правил на месте, ожидаемая и самоочевидная причина, напоминать
/// нечего. С подсказкой, если отпечатка не было вовсе: эту причину от
/// «человек сам положил сюда что-то своё» различить нельзя (Important 3
/// ревью 31.10, круг 1 — совпадение содержимого с комплектом ничего не
/// решает, отпечатка нет и точка), а одна из них — стёртое состояние
/// программы после переустановки (Important 6), и без подсказки человек
/// застревает без выхода: не наш скилл ни обновить, ни снять. Подсказка
/// называет каталог и говорит, что его можно удалить руками, после чего
/// галочка поставит скилл заново.
///
/// `pub(crate)` (круговое ревью, круг 3, пункт 3): тест `panel::skill_found_text`
/// обязан собирать длинную и короткую метку ЭТОЙ функцией, а не рукописной
/// строкой со скобкой — иначе шов между модулями (где текст делится на
/// «короткую» и «длинную» форму по наличию `(`) остаётся непроверенным, и
/// смена формата здесь (например, тире вместо скобки) тихо разошлась бы с
/// панелью при зелёных тестах.
pub(crate) fn foreign_label(name: &str, dst: &Path, had_stamp: bool) -> String {
    if had_stamp {
        name.to_string()
    } else {
        format!(
            "{name} (своего отпечатка нет; если это наш скилл после переустановки программы — удалите {} руками, и галочка поставит его заново)",
            dst.display()
        )
    }
}

/// Куда отнести скилл по вердикту — общее для чтения (`look`) и для снятия
/// (`remove`): обеим нужно одно и то же трёхстороннее деление, разница лишь
/// в том, что `remove` после него ещё и действует. `install` сюда не
/// заходит — ему, в отличие от чтения и снятия, нужно решать по вердикту
/// ДЕЙСТВИЕМ (копировать, писать отпечаток), а не просто его назвать.
enum Seen {
    New,
    Ours,
    Foreign(String),
}

/// Разбор одного скилла: путь к его отпечатку и то, к чему он отнесён.
/// Путь возвращается вместе с разбором, чтобы `remove` не считал его
/// заново — `stamp_path` и `verdict` зовутся здесь ровно один раз.
fn classify(state: &State, dst: &Path, name: &str) -> Result<(PathBuf, Seen), String> {
    let stamp = stamp_path(state, dst, name)?;
    let prev = read_stamp(&stamp)?;
    let seen = match verdict(dst, prev.as_deref())? {
        Verdict::Link => return Err(format!("{} — ссылка на месте скилла, не трогаю", dst.display())),
        Verdict::New => Seen::New,
        Verdict::Untouched => Seen::Ours,
        Verdict::Edited => Seen::Foreign(foreign_label(name, dst, prev.is_some())),
    };
    Ok((stamp, seen))
}

/// Копия каталога скилла из комплекта в дом агента. `Links::Follow`, не
/// `Links::Refuse`: это содержимое комплекта, не чужой каталог — тем же
/// доверием, каким `write_list` сверяет сам комплект (комментарий модуля).
/// Старое на месте назначения удаляется целиком перед копией — не поверх:
/// иначе файл, пропавший в новой версии скилла, пережил бы обновление.
fn copy_skill(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        std::fs::remove_dir_all(dst).map_err(|e| format!("не удалить {}: {e}", dst.display()))?;
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("не создать {}: {e}", dst.display()))?;
    let mut files = Vec::new();
    walk(src, Links::Follow, &mut files).map_err(|e| format!("{e:#}"))?;
    for f in &files {
        let to = dst.join(rel(src, f));
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("не создать {}: {e}", parent.display()))?;
        }
        std::fs::copy(f, &to).map_err(|e| format!("не скопировать {}: {e}", f.display()))?;
    }
    Ok(())
}

/// Что сейчас в доме агента — ничего не меняет, только читает. Дом назначения
/// (`home`) — уже решённый путь (`~/.agents/skills` или `~/.claude/skills`):
/// звонить `home_dir` здесь значило бы читать переменную среды, общую для
/// всего набора тестов, и получать гонку с соседями — решили отдать это
/// вызывающему. Одна ссылка на месте скилла не прячет состояние остальных
/// (Important 4 ревью 31.10, круг 1) — ошибки копятся, а не обрывают цикл на
/// первой.
pub fn look(kit: &Kit, state: &State, home: &Path) -> Result<editors::Found, Vec<String>> {
    let names = kit_skill_names(kit).map_err(|e| vec![e])?;
    let mut errors = Vec::new();
    let mut foreign = Vec::new();
    let mut ours = 0usize;
    for name in &names {
        let dst = home.join(name);
        match classify(state, &dst, name) {
            Ok((_, Seen::New)) => {}
            Ok((_, Seen::Ours)) => ours += 1,
            Ok((_, Seen::Foreign(label))) => foreign.push(label),
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(summarize(names.len(), foreign, ours))
}

/// Поставить и обновить — разницы в правиле нет, ровно так устроен
/// контейнер (`container-rule.txt`): каждый скилл по своему вердикту. Нет →
/// копируем и пишем отпечаток. Не тронут (`Untouched`) — первая ветка
/// первоисточника, которой не было до круга 1: отпечаток и есть сумма
/// образа на момент прошлой раскладки, и если она совпала с суммой образа
/// СЕЙЧАС — делать нечего, кроме как ничего не делать (Critical 1 ревью
/// 31.10). Это не только экономия: без этой ветки `install` на каждый
/// щелчок сносил бы каталог и копировал заново даже когда менять нечего, и
/// обрыв между удалением и копией (ошибка диска, закрытие окна) оставлял бы
/// каталог без отпечатка — а без отпечатка скилл не наш, точка (Important 3
/// ревью 31.10, круг 1), и следующий `install` объявлял бы его чужим
/// навсегда: ни обновить, ни снять. Правлен человеком — не трогаем, называем
/// в `Foreign`. Ссылка на его месте — отказ словами, но только для этого
/// скилла: ошибка одного не должна останавливать раскладку остальных,
/// поэтому они собираются в список, а не обрывают цикл.
pub fn install(kit: &Kit, state: &State, home: &Path) -> Result<editors::Found, Vec<String>> {
    // `write_stamp` ниже пишет отпечаток прямо в `state` — каталог обязан
    // существовать, иначе первая же раскладка откажет пустым местом,
    // которого ещё никто не создавал (круговое ревью, круг 2, пункт 5:
    // без этой строки `write_stamp` честно отказывает словами, но раскладка
    // откажет на первом же щелчке с пустым состоянием вместо того, чтобы
    // завести каталог самой).
    std::fs::create_dir_all(state.path()).map_err(|e| vec![format!("не создать {}: {e}", state.path().display())])?;
    let names = kit_skill_names(kit).map_err(|e| vec![e])?;
    let mut errors = Vec::new();
    let mut foreign = Vec::new();
    let mut ours = 0usize;
    for name in &names {
        let src = kit.path(KIT_DIR).join(name);
        let dst = home.join(name);
        let outcome = (|| -> Result<Option<String>, String> {
            let stamp = stamp_path(state, &dst, name)?;
            let prev = read_stamp(&stamp)?;
            match verdict(&dst, prev.as_deref())? {
                Verdict::Link => Err(format!("{} — ссылка на месте скилла, не трогаю", dst.display())),
                Verdict::New => {
                    let sum = dir_sum(&src)?;
                    copy_skill(&src, &dst)?;
                    write_stamp(&stamp, &sum)?;
                    Ok(None)
                }
                Verdict::Untouched => {
                    let sum = dir_sum(&src)?;
                    if prev.as_deref() != Some(sum.as_str()) {
                        copy_skill(&src, &dst)?;
                        write_stamp(&stamp, &sum)?;
                    }
                    Ok(None)
                }
                Verdict::Edited => Ok(Some(foreign_label(name, &dst, prev.is_some()))),
            }
        })();
        match outcome {
            Ok(None) => ours += 1,
            Ok(Some(label)) => foreign.push(label),
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(summarize(names.len(), foreign, ours))
}

/// Снять наши скиллы — только те, что не тронуты: правленые остаются на
/// месте и названы в `Foreign` («remove не уносит»), ссылка на месте скилла —
/// отказ словами, не обрывая снятие остальных. Отпечаток уходит вместе с
/// каталогом: иначе он пережил бы собственный скилл и соврал бы при
/// следующей раскладке (хотя `verdict` этой лжи и не слушает — каталога
/// уже нет, вердикт `New` раньше, чем отпечаток был бы прочитан).
pub fn remove(kit: &Kit, state: &State, home: &Path) -> Result<editors::Found, Vec<String>> {
    let names = kit_skill_names(kit).map_err(|e| vec![e])?;
    let mut errors = Vec::new();
    let mut foreign = Vec::new();
    for name in &names {
        let dst = home.join(name);
        let outcome = classify(state, &dst, name).and_then(|(stamp, seen)| match seen {
            Seen::New => Ok(None),
            Seen::Foreign(label) => Ok(Some(label)),
            Seen::Ours => {
                std::fs::remove_dir_all(&dst).map_err(|e| format!("не удалить {}: {e}", dst.display()))?;
                // Отпечаток уходит следом за каталогом — иначе он переживёт
                // собственный скилл. Отсутствующего файла не пугаемся не
                // потому, что повторное снятие законно (сюда доходим только
                // через Seen::Ours, то есть отпечаток был прочитан только
                // что и существовал) — а потому, что между тем чтением и
                // этим удалением файл мог исчезнуть сам: человек стёр его
                // руками, или рядом работает ещё один процесс (гонка).
                match std::fs::remove_file(&stamp) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("не удалить {}: {e}", stamp.display())),
                }
                Ok(None)
            }
        });
        match outcome {
            Ok(None) => {}
            Ok(Some(label)) => foreign.push(label),
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(summarize(names.len(), foreign, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::tempdir::Dir;

    /// Оборачивает временный каталог теста в `State` — поле приватно для
    /// внешних модулей (круговое ревью, круг 3, пункт 1), но этот тест —
    /// потомок модуля `skills`, ему видно. Один маленький помощник вместо
    /// повторения `State(d.path().to_path_buf())` на каждый вызов.
    fn ts(d: &Dir) -> State {
        State(d.path().to_path_buf())
    }

    /// Ссылка `link` на `target`, настоящая на обеих системах: на Windows —
    /// соединение каталогов (`mklink /J`, не требует прав администратора, но
    /// работает только на каталог), на Unix — `std::os::unix::fs::symlink`
    /// (тоже без привилегий, и на файл, и на каталог). CI гоняет тесты на
    /// ubuntu (`.github/workflows/build.yml`), и до круга 2 эти тесты там
    /// ничего не проверяли (`cfg!(windows)` сразу отдавал `false`) — находка
    /// ревью 31.10. `false`, если создать не вышло: тест, который зовёт,
    /// тогда себя пропускает с объяснением, а не падает. А если команда
    /// отчиталась об успехе, а ссылки на самом деле нет — падает здесь же,
    /// а не тихо: `eprintln!` в прошедшем тесте не виден в логе CI.
    #[cfg(windows)]
    fn try_link(link: &Path, target: &Path) -> bool {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", &link.display().to_string(), &target.display().to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if made {
            assert!(is_symlink(link), "{} — соединение не появилось после создания", link.display());
        }
        made
    }

    #[cfg(unix)]
    fn try_link(link: &Path, target: &Path) -> bool {
        let made = std::os::unix::fs::symlink(target, link).is_ok();
        if made {
            assert!(is_symlink(link), "{} — ссылка не появилась после создания", link.display());
        }
        made
    }

    /// Запрет на удаление каталога назначения — чтобы доказать, что `install`
    /// на неизменившемся комплекте не пытается его снести (Critical 1 ревью
    /// 31.10, круг 1). На Unix удаление файла запрещает отсутствие права
    /// записи у каталога, который его содержит, — не у самого файла; на
    /// Windows — атрибут «только для чтения» у файла, `remove_file` его сам
    /// не снимает. `false`, если не вышло поставить запрет: тест тогда себя
    /// пропускает, как и с `try_link`.
    #[cfg(unix)]
    fn make_undeletable(dst: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o555)).is_ok()
    }

    #[cfg(unix)]
    fn make_deletable_again(dst: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o755));
    }

    /// На Windows атрибут «только для чтения» не всегда останавливает
    /// `remove_dir_all` — замечено на этой самой машине: библиотека иногда
    /// сама снимает атрибут перед удалением. Поэтому перед тем как доверять
    /// запрету в настоящем тесте, проверяем его на одноразовой декорации —
    /// если удаление всё равно прошло, механизм здесь не держит, и тест
    /// должен себя пропустить, а не давать ложный зелёный свет.
    #[cfg(windows)]
    fn make_undeletable(dst: &Path) -> bool {
        if !windows_readonly_actually_blocks_removal(dst) {
            return false;
        }
        let file = dst.join("SKILL.md");
        let Ok(meta) = std::fs::metadata(&file) else { return false };
        let mut perm = meta.permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&file, perm).is_ok()
    }

    /// Проба на одноразовой декорации рядом с `near` (не на самом `dst`,
    /// чтобы ничего не сломать, если проба провалится по пути). Чистит за
    /// собой руками: если удаление и правда не сработало, каталог пробы сам
    /// по себе никуда не делся бы.
    // Про `set_readonly(false)` — то же обоснование, что у
    // `make_deletable_again` ниже: ветка только для Windows, и расширять там
    // нечего.
    #[cfg(windows)]
    #[allow(clippy::permissions_set_readonly_false)]
    fn windows_readonly_actually_blocks_removal(near: &Path) -> bool {
        let probe_dir = near.with_file_name(format!("probe-{}", std::process::id()));
        if std::fs::create_dir_all(&probe_dir).is_err() {
            return false;
        }
        let probe_file = probe_dir.join("f");
        let staged = std::fs::write(&probe_file, "x").is_ok() && {
            let mut perm = std::fs::metadata(&probe_file).unwrap().permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(&probe_file, perm).is_ok()
        };
        let removal_failed = staged && std::fs::remove_dir_all(&probe_dir).is_err();
        if std::fs::metadata(&probe_file).is_ok() {
            let mut perm = std::fs::metadata(&probe_file).unwrap().permissions();
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(&probe_file, perm);
        }
        let _ = std::fs::remove_dir_all(&probe_dir);
        staged && removal_failed
    }

    // `set_readonly(false)` на Unix сделал бы файл доступным всем — clippy
    // предупреждает именно про это; ветка только для Windows, там атрибут
    // «только для чтения» не несёт в себе прав доступа, которые бы при этом
    // расширились.
    #[cfg(windows)]
    #[allow(clippy::permissions_set_readonly_false)]
    fn make_deletable_again(dst: &Path) {
        let file = dst.join("SKILL.md");
        if let Ok(meta) = std::fs::metadata(&file) {
            let mut perm = meta.permissions();
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(&file, perm);
        }
    }

    #[test]
    fn the_three_cases_are_the_ones_the_container_uses() {
        let t = Dir::new();
        let dst = t.path().join("1c");

        // Нет каталога → New.
        assert_eq!(verdict(&dst, None).unwrap(), Verdict::New);

        // Есть, и сумма совпала с отпечатком → Untouched.
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("SKILL.md"), "привет").unwrap();
        let sum = dir_sum(&dst).unwrap();
        assert_eq!(verdict(&dst, Some(&sum)).unwrap(), Verdict::Untouched);

        // Есть, и не совпала (человек правил на месте) → Edited.
        std::fs::write(dst.join("SKILL.md"), "правлено человеком").unwrap();
        assert_eq!(verdict(&dst, Some(&sum)).unwrap(), Verdict::Edited);

        // Отпечатка вовсе не было, а каталог уже есть — тоже Edited: взять
        // его прошлым не из чего.
        assert_eq!(verdict(&dst, None).unwrap(), Verdict::Edited);
    }

    /// На месте скилла — настоящая ссылка (соединение на Windows, симлинк на
    /// Unix): `verdict` отказывает `Link` раньше, чем заглянул бы внутрь.
    #[test]
    fn a_junction_where_a_skill_should_be_is_refused() {
        let t = Dir::new();
        let elsewhere = t.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let dst = t.path().join("1c");
        if !try_link(&dst, &elsewhere) {
            eprintln!("пропущено: ссылку создать не вышло на этой машине");
            return;
        }
        assert_eq!(verdict(&dst, None).unwrap(), Verdict::Link);
    }

    /// Ссылка ВНУТРИ скилла — не только на его месте: обход не идёт по ней и
    /// отказывает словами, а не уходит в бесконечную рекурсию (Critical 2
    /// ревью 30.10 — ссылка на самого себя даёт цикл, прежний обход шёл бы
    /// по нему вечно).
    #[test]
    fn a_junction_inside_a_skill_directory_is_refused_not_followed() {
        let t = Dir::new();
        let skill = t.path().join("1c");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "текст скилла").unwrap();
        let loop_link = skill.join("loop");
        if !try_link(&loop_link, &skill) {
            eprintln!("пропущено: ссылку создать не вышло на этой машине");
            return;
        }
        let err = dir_sum(&skill).unwrap_err();
        assert!(err.contains("ссылка"), "{err}");
    }

    /// На месте файла отпечатка — ссылка. `write_stamp` обязан отказать, а
    /// не последовать по ней и записать в чужую цель (Critical 1 ревью
    /// 30.10); `read_stamp` — не прочитать чужое молча.
    #[test]
    fn a_junction_where_a_stamp_should_be_is_refused_not_followed() {
        let t = Dir::new();
        let elsewhere = t.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("захваченное"), "чужое").unwrap();
        let stamp = t.path().join("stamp");
        if !try_link(&stamp, &elsewhere) {
            eprintln!("пропущено: ссылку создать не вышло на этой машине");
            return;
        }
        assert!(write_stamp(&stamp, "новая сумма").is_err());
        assert!(read_stamp(&stamp).is_err());
        // Чужой каталог не тронут.
        assert!(elsewhere.join("захваченное").exists());
    }

    #[test]
    fn the_fingerprint_follows_the_content_not_the_order_of_the_walk() {
        let a = Dir::new();
        let b = Dir::new();

        // Одно и то же содержимое, в разном порядке создания файлов — сумма
        // обязана совпасть: `dir_sum` сортирует список сам.
        std::fs::create_dir_all(a.path().join("sub")).unwrap();
        std::fs::write(a.path().join("SKILL.md"), "текст скилла").unwrap();
        std::fs::write(a.path().join("sub/extra.md"), "подробности").unwrap();

        std::fs::create_dir_all(b.path().join("sub")).unwrap();
        std::fs::write(b.path().join("sub/extra.md"), "подробности").unwrap();
        std::fs::write(b.path().join("SKILL.md"), "текст скилла").unwrap();

        let sum_a = dir_sum(a.path()).unwrap();
        let sum_b = dir_sum(b.path()).unwrap();
        assert_eq!(sum_a, sum_b);

        // Правка одного байта в любом файле — сумма обязана измениться.
        std::fs::write(b.path().join("sub/extra.md"), "подробности!").unwrap();
        let sum_b_changed = dir_sum(b.path()).unwrap();
        assert_ne!(sum_a, sum_b_changed);
    }

    #[test]
    fn stamp_path_refuses_a_skill_name_that_is_not_simple() {
        let t = Dir::new();
        let state = ts(&t);
        assert!(stamp_path(&state, &t.path().join("1c"), "../../etc").is_err());
        assert!(stamp_path(&state, &t.path().join("1c"), "..").is_err());
        assert!(stamp_path(&state, &t.path().join("1c"), "1c").is_ok());
    }

    /// Круг 4, вторая находка ре-ревью: 763 теста проверяли только КАТАЛОГ,
    /// собранный `state()`, и ни один — что сам файл отпечатка лежит ВНУТРИ
    /// него. Подмена `state.path().join(...)` на
    /// `state.path().parent().unwrap().join(...)` оставляла всё зелёным, а
    /// отпечатки ложились ровно туда, куда круг 1 их запрещал, — в корень
    /// состояния рядом с `secret.key`.
    ///
    /// Круг 5, пункт 1: `starts_with` этого не ловит до конца —
    /// `Path::starts_with` сравнивает компоненты и `..` не раскрывает,
    /// поэтому `…\skill-stamps\..\skill_ab_1c` считается лежащим внутри
    /// `…\skill-stamps`, и подмена `join("skill_…")` на `join("../skill_…")`
    /// проходила мимо. Утверждение о РОДИТЕЛЕ (`parent() == state.path()`)
    /// убивает обе подмены разом: лишний `..` меняет родителя, подъём к
    /// `parent().unwrap()` — тоже.
    #[test]
    fn stamp_path_tells_apart_two_destinations_with_the_same_skill() {
        let dir = Dir::new();
        let state = ts(&dir);
        let one = stamp_path(&state, &PathBuf::from("/home/a/.agents/skills"), "1c").unwrap();
        let other = stamp_path(&state, &PathBuf::from("/home/b/.agents/skills"), "1c").unwrap();
        assert_ne!(one, other, "разные каталоги назначения не должны делить один отпечаток");
        assert_eq!(one.parent(), Some(state.path()), "{} не прямо в {}", one.display(), state.path().display());
        assert_eq!(other.parent(), Some(state.path()), "{} не прямо в {}", other.display(), state.path().display());
    }

    /// Точка 2 ревью, круг 1: другое написание того же самого каталога —
    /// лишний `.` в пути — обязано дать тот же отпечаток, не другой.
    #[test]
    fn stamp_path_is_stable_across_spellings_of_the_same_destination() {
        let state = Dir::new();
        let home = Dir::new();
        let dst = home.path().join("1c");
        std::fs::create_dir_all(&dst).unwrap();
        let messy = home.path().join(".").join("1c");

        let one = stamp_path(&ts(&state), &dst, "1c").unwrap();
        let other = stamp_path(&ts(&state), &messy, "1c").unwrap();
        assert_eq!(one, other, "два написания одного каталога должны делить один отпечаток");
    }

    /// Тот же вопрос, но ДО появления каталога назначения — ровно первая
    /// раскладка, где `fs::canonicalize` самого `dst` обречён на неудачу и
    /// обязан отступить к существующему предку (`home.path()`), а не отдать
    /// буквальный текст как есть: иначе лишний `.` в написании дал бы другой
    /// отпечаток, хотя место на диске то же самое. Несуществующих уровней —
    /// не меньше двух (`.agents` и `.agents/skills`), ровно как в настоящей
    /// первой раскладке (`~/.agents/skills/<имя>`): при одном недостающем
    /// уровне длина хвоста восхождения — единица, и порядок в нём не виден
    /// (находка ре-ревью, круг 3).
    #[test]
    fn stamp_path_is_stable_across_spellings_before_the_destination_exists() {
        let state = Dir::new();
        let home = Dir::new();
        let dst = home.path().join(".agents").join("skills").join("1c");
        let messy = home.path().join(".agents").join("skills").join(".").join("1c");

        let one = stamp_path(&ts(&state), &dst, "1c").unwrap();
        let other = stamp_path(&ts(&state), &messy, "1c").unwrap();
        assert_eq!(one, other, "до появления каталога написания всё равно должны совпадать");
    }

    /// Главное свойство отступления к существующему предку (находка
    /// ре-ревью, круг 3): ключ отпечатка, посчитанный ДО появления каталога
    /// назначения, обязан совпасть с ключом, посчитанным ПОСЛЕ — иначе
    /// отпечаток первой раскладки осиротеет в тот самый момент, когда
    /// каталог появляется на диске, и скилл станет чужим навсегда (ни
    /// обновить, ни снять). Несуществующих уровней — не меньше двух, той же
    /// причиной, что и выше: при одном уровне восхождение и прямая
    /// канонизация дают один и тот же хвост без разницы в порядке, и
    /// подмена, ломающая порядок (например, выкинутый `.rev()`), осталась бы
    /// незамеченной.
    #[test]
    fn stamp_path_survives_the_destination_coming_into_existence() {
        let state = Dir::new();
        let home = Dir::new();
        let dst = home.path().join(".agents").join("skills").join("1c");

        let before = stamp_path(&ts(&state), &dst, "1c").unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        let after = stamp_path(&ts(&state), &dst, "1c").unwrap();

        assert_eq!(before, after, "появление каталога назначения не должно менять ключ его отпечатка");
    }

    /// Другое написание РЕГИСТРОМ — тоже до появления каталога, тот же
    /// хвост, который `canonicalize` ещё не может подтвердить. На Windows
    /// это то же самое место на диске («другой регистр `USERPROFILE`» —
    /// находка ревью 31.10, круг 1), и отпечаток обязан совпасть; на Unix
    /// регистр в имени значащий — это буквально другое имя, и отпечатки
    /// обязаны разойтись, а не притвориться одним.
    #[test]
    fn stamp_path_case_sensitivity_matches_the_platform() {
        let state = Dir::new();
        let home = Dir::new();
        let dst = home.path().join("1c");
        let shouted = home.path().join("1C");

        let one = stamp_path(&ts(&state), &dst, "1c").unwrap();
        let other = stamp_path(&ts(&state), &shouted, "1c").unwrap();

        if cfg!(windows) {
            assert_eq!(one, other, "на Windows регистр не значащий даже для ещё не созданного пути");
        } else {
            assert_ne!(one, other, "на Unix регистр значащий — «1c» и «1C» разные имена");
        }
    }

    /// Комплект с несколькими скиллами внутри `skills/` — своя небольшая
    /// обёртка, чтобы проверки задачи 3 не повторяли одно и то же устройство
    /// комплекта по несколько строк каждая. `kit_with_skill` — частный случай
    /// с одним скиллом, для тестов, которым смесь не нужна.
    fn kit_with_skills(skills: &[(&str, &str)]) -> (Dir, Kit) {
        let root = Dir::new();
        for (name, text) in skills {
            std::fs::create_dir_all(root.path().join(KIT_DIR).join(name)).unwrap();
            std::fs::write(root.path().join(KIT_DIR).join(name).join("SKILL.md"), text).unwrap();
        }
        let kit = Kit::at(root.path());
        (root, kit)
    }

    fn kit_with_skill(name: &str, text: &str) -> (Dir, Kit) {
        kit_with_skills(&[(name, text)])
    }

    #[test]
    fn install_copies_the_kit_skills_and_writes_their_stamps() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();

        let found = install(&kit, &ts(&state), home.path()).unwrap();
        assert_eq!(found, editors::Found::Ours);

        let dst = home.path().join("1c");
        assert_eq!(std::fs::read_to_string(dst.join("SKILL.md")).unwrap(), "текст скилла");
        let stamp = stamp_path(&ts(&state), &dst, "1c").unwrap();
        assert_eq!(read_stamp(&stamp).unwrap().unwrap(), dir_sum(&dst).unwrap());
    }

    /// Папка с нашим именем уже есть, но мы её не клали — отпечатка для неё
    /// никогда не было. `look` называет её чужой, с подсказкой (у отпечатка
    /// нет прошлого, различить «чужое» от «наше после переустановки» нельзя
    /// — Important 3 и 6 ревью 31.10, круг 1); `install` её не перезаписывает;
    /// `remove` не уносит.
    #[test]
    fn a_skill_we_never_put_there_is_left_alone() {
        let (_root, kit) = kit_with_skill("1c", "из комплекта");
        let state = Dir::new();
        let home = Dir::new();
        std::fs::create_dir_all(home.path().join("1c")).unwrap();
        std::fs::write(home.path().join("1c/SKILL.md"), "чужое содержимое").unwrap();
        let label = foreign_label("1c", &home.path().join("1c"), false);

        assert_eq!(look(&kit, &ts(&state), home.path()).unwrap(), editors::Found::Foreign(vec![label.clone()]));

        let after_install = install(&kit, &ts(&state), home.path()).unwrap();
        assert_eq!(after_install, editors::Found::Foreign(vec![label.clone()]));
        assert_eq!(std::fs::read_to_string(home.path().join("1c/SKILL.md")).unwrap(), "чужое содержимое");

        let after_remove = remove(&kit, &ts(&state), home.path()).unwrap();
        assert_eq!(after_remove, editors::Found::Foreign(vec![label]));
        assert!(home.path().join("1c").exists());
    }

    /// Точка 3 ревью, круг 1: отпечатка нет — скилл не наш, даже если его
    /// содержимое сейчас байт-в-байт совпадает с тем, что даёт комплект.
    /// Раньше `install` считал такое совпадение доказательством «это наша
    /// недоделанная раскладка» и дописывал отпечаток сам — ровно та дыра,
    /// через которую чужой каталог стал бы нашим без повода: человек мог
    /// скопировать наш скилл себе руками, и следующее снятие унесло бы его.
    #[test]
    fn content_matching_the_kit_is_not_enough_without_a_stamp() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();
        std::fs::create_dir_all(home.path().join("1c")).unwrap();
        std::fs::write(home.path().join("1c/SKILL.md"), "текст скилла").unwrap();

        let found = install(&kit, &ts(&state), home.path()).unwrap();
        let label = foreign_label("1c", &home.path().join("1c"), false);
        assert_eq!(found, editors::Found::Foreign(vec![label]));

        // install не притронулся и не дописал отпечаток.
        let stamp = stamp_path(&ts(&state), &home.path().join("1c"), "1c").unwrap();
        assert_eq!(read_stamp(&stamp).unwrap(), None);
    }

    /// `look` после чистой раскладки — все скиллы `Untouched`, вердикт
    /// `Found::Ours`. Единственный путь внутрь `Seen::Ours => ours += 1` у
    /// самого `look` (три прежних вызова в тестах давали только `Foreign`
    /// или `Err` — находка ревью 31.10, круг 2): без счёта `ours` общий счёт
    /// не сравнялся бы с числом скиллов, и `summarize` отдал бы `Absent`.
    #[test]
    fn look_reports_ours_right_after_a_clean_install() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();
        install(&kit, &ts(&state), home.path()).unwrap();

        assert_eq!(look(&kit, &ts(&state), home.path()).unwrap(), editors::Found::Ours);
    }

    /// `look` на пустом доме — скилл в комплекте есть, в доме его вообще
    /// нет: `Found::Absent`, не `Ours` и не `Foreign`. Этот исход `look`
    /// тоже не был проверен ни разу.
    #[test]
    fn look_reports_absent_before_anything_is_installed() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();

        assert_eq!(look(&kit, &ts(&state), home.path()).unwrap(), editors::Found::Absent);
    }

    /// Критично, круг 2: `copy_skill` удаляет старый каталог целиком перед
    /// копией, не копирует поверх — иначе файл, пропавший в новой версии
    /// скилла, пережил бы обновление (докстрока `copy_skill`). Доказываем
    /// не словами, а фактом: кладём версию с лишним файлом, потом версию без
    /// него, и смотрим, остался ли он в доме после второго `install`.
    #[test]
    fn a_file_dropped_from_a_newer_kit_version_does_not_survive_the_update() {
        let (root, kit) = kit_with_skill("1c", "первая версия");
        std::fs::write(root.path().join(KIT_DIR).join("1c/extra.md"), "лишний файл").unwrap();
        let state = Dir::new();
        let home = Dir::new();
        install(&kit, &ts(&state), home.path()).unwrap();
        assert!(home.path().join("1c/extra.md").exists());

        // Новая версия скилла — файл extra.md пропал из комплекта.
        std::fs::remove_file(root.path().join(KIT_DIR).join("1c/extra.md")).unwrap();
        std::fs::write(root.path().join(KIT_DIR).join("1c/SKILL.md"), "вторая версия").unwrap();
        install(&kit, &ts(&state), home.path()).unwrap();

        assert!(!home.path().join("1c/extra.md").exists(), "файл, пропавший из новой версии, не должен выжить");
    }

    #[test]
    fn an_edited_skill_survives_both_update_and_removal_and_is_named() {
        let (root, kit) = kit_with_skill("1c", "версия раз");
        let state = Dir::new();
        let home = Dir::new();
        install(&kit, &ts(&state), home.path()).unwrap();

        // Человек правит поставленный скилл на месте.
        std::fs::write(home.path().join("1c/SKILL.md"), "правка человека").unwrap();

        // Комплект обновился — но правленое install не трогает.
        std::fs::write(root.path().join(KIT_DIR).join("1c/SKILL.md"), "версия два").unwrap();
        let after_install = install(&kit, &ts(&state), home.path()).unwrap();
        assert_eq!(after_install, editors::Found::Foreign(vec!["1c".to_string()]));
        assert_eq!(std::fs::read_to_string(home.path().join("1c/SKILL.md")).unwrap(), "правка человека");

        let after_remove = remove(&kit, &ts(&state), home.path()).unwrap();
        assert_eq!(after_remove, editors::Found::Foreign(vec!["1c".to_string()]));
        assert!(home.path().join("1c").exists(), "правленый скилл не должен уноситься");
    }

    #[test]
    fn a_new_kit_replaces_the_untouched_skills() {
        let (root, kit) = kit_with_skill("1c", "версия раз");
        let state = Dir::new();
        let home = Dir::new();
        install(&kit, &ts(&state), home.path()).unwrap();

        // Скилл в доме не тронут — комплект меняет версию.
        std::fs::write(root.path().join(KIT_DIR).join("1c/SKILL.md"), "версия два").unwrap();
        let found = install(&kit, &ts(&state), home.path()).unwrap();
        assert_eq!(found, editors::Found::Ours);
        assert_eq!(std::fs::read_to_string(home.path().join("1c/SKILL.md")).unwrap(), "версия два");
    }

    /// Критично, круг 1: если комплект НЕ менялся, повторный `install` не
    /// должен даже пытаться снести каталог назначения. Доказываем это не
    /// словами, а запретом: ставим скилл, запрещаем его удаление и зовём
    /// `install` ещё раз на том же, неизменившемся комплекте — без ветки
    /// «образ не менялся, делать нечего» снос упёрся бы в запрет и вернул
    /// ошибку.
    #[test]
    fn install_does_nothing_when_the_kit_has_not_changed() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();
        install(&kit, &ts(&state), home.path()).unwrap();

        let dst = home.path().join("1c");
        if !make_undeletable(&dst) {
            eprintln!("пропущено: запретить удаление не вышло на этой машине");
            return;
        }
        let result = install(&kit, &ts(&state), home.path());
        make_deletable_again(&dst);
        let found = result.unwrap();
        assert_eq!(found, editors::Found::Ours);
        assert_eq!(std::fs::read_to_string(dst.join("SKILL.md")).unwrap(), "текст скилла");
    }

    /// Ссылка на месте скилла не снаружи, а В САМОМ КОМПЛЕКТЕ (находка
    /// ре-ревью, круг 3): молчаливый пропуск был бы хуже тихой неисправности
    /// — `kit_skill_names` просто не назвал бы этот скилл, `look` отрапортовал
    /// бы «всё наше на месте», и, если этот скилл когда-то ставился
    /// каталогом, `remove` больше не увидел бы его вовсе — каталог и
    /// отпечаток остались бы сиротами в доме человека. Отказ словами, как и
    /// у неправильного имени скилла в комплекте.
    #[test]
    fn a_link_in_place_of_a_skill_inside_the_kit_is_refused_in_words() {
        let (root, kit) = kit_with_skill("1c", "текст скилла");
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let link = root.path().join(KIT_DIR).join("linked");
        if !try_link(&link, &elsewhere) {
            eprintln!("пропущено: ссылку создать не вышло на этой машине");
            return;
        }
        let state = Dir::new();
        let home = Dir::new();

        let look_err = look(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(look_err.len(), 1, "{look_err:?}");
        assert!(look_err[0].contains("ссылка"), "{look_err:?}");
        let install_err = install(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(install_err.len(), 1, "{install_err:?}");
        assert!(install_err[0].contains("ссылка"), "{install_err:?}");
        let remove_err = remove(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(remove_err.len(), 1, "{remove_err:?}");
        assert!(remove_err[0].contains("ссылка"), "{remove_err:?}");
    }

    #[test]
    fn a_symlink_in_place_of_a_skill_is_refused_in_words() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();
        let elsewhere = home.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let dst = home.path().join("1c");
        if !try_link(&dst, &elsewhere) {
            eprintln!("пропущено: ссылку создать не вышло на этой машине");
            return;
        }

        let look_err = look(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(look_err.len(), 1);
        assert!(look_err[0].contains("ссылка"), "{look_err:?}");
        let install_err = install(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(install_err.len(), 1);
        assert!(install_err[0].contains("ссылка"), "{install_err:?}");
        let remove_err = remove(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(remove_err.len(), 1);
        assert!(remove_err[0].contains("ссылка"), "{remove_err:?}");
    }

    /// Снятие убирает и отпечаток, не только каталог — и поставленный заново
    /// скилл после этого идёт по ветке `New`, не застревая на старом файле
    /// (покрытие, которого не было: ревью 31.10, круг 1, пункт 8).
    #[test]
    fn install_then_remove_then_install_leaves_a_clean_new_copy() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();

        install(&kit, &ts(&state), home.path()).unwrap();
        let dst = home.path().join("1c");
        let stamp = stamp_path(&ts(&state), &dst, "1c").unwrap();
        assert!(stamp.exists());

        remove(&kit, &ts(&state), home.path()).unwrap();
        assert!(!dst.exists());
        assert!(!stamp.exists(), "отпечаток обязан уйти вместе со скиллом");

        let found = install(&kit, &ts(&state), home.path()).unwrap();
        assert_eq!(found, editors::Found::Ours);
        assert_eq!(std::fs::read_to_string(dst.join("SKILL.md")).unwrap(), "текст скилла");
    }

    /// Пункт 5 ревью, круг 1: комплект без `skills/` — отказ словами у всех
    /// трёх функций, а не `Found::Missing` (которое окно печатает про
    /// отсутствующий РЕДАКТОР — неправда про скиллы).
    #[test]
    fn a_kit_without_a_skills_folder_is_refused_in_words_not_called_missing() {
        let root = Dir::new();
        let kit = Kit::at(root.path());
        let state = Dir::new();
        let home = Dir::new();

        let look_err = look(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(look_err.len(), 1);
        assert!(look_err[0].contains(KIT_DIR), "{look_err:?}");

        let install_err = install(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(install_err.len(), 1);
        assert!(install_err[0].contains(KIT_DIR), "{install_err:?}");

        let remove_err = remove(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(remove_err.len(), 1);
        assert!(remove_err[0].contains(KIT_DIR), "{remove_err:?}");
    }

    /// Пункт 8 ревью, круг 1: комплект из двух скиллов разом — одна часть
    /// наша и не тронута, другая правлена человеком. Без разбора по каждому
    /// скиллу «часть наша, часть чужая» было бы неотличимо от «всё наше» или
    /// «всё чужое»: единственный скилл в комплекте, как было до круга 1, эту
    /// развилку в `summarize` (`total > 0 && ours == total`) не проверял.
    #[test]
    fn a_kit_can_be_half_ours_and_half_foreign_at_once() {
        let (root, kit) = kit_with_skills(&[("alpha", "альфа раз"), ("beta", "бета раз")]);
        let state = Dir::new();
        let home = Dir::new();
        install(&kit, &ts(&state), home.path()).unwrap();

        // beta правлена человеком, alpha не тронута.
        std::fs::write(home.path().join("beta/SKILL.md"), "правка человека").unwrap();

        // Комплект обновил обе — alpha обновится, beta останется как есть.
        std::fs::write(root.path().join(KIT_DIR).join("alpha/SKILL.md"), "альфа два").unwrap();
        std::fs::write(root.path().join(KIT_DIR).join("beta/SKILL.md"), "бета два").unwrap();

        let found = install(&kit, &ts(&state), home.path()).unwrap();
        assert_eq!(found, editors::Found::Foreign(vec!["beta".to_string()]));
        assert_eq!(std::fs::read_to_string(home.path().join("alpha/SKILL.md")).unwrap(), "альфа два");
        assert_eq!(std::fs::read_to_string(home.path().join("beta/SKILL.md")).unwrap(), "правка человека");
    }

    /// Пункт 4 и 8 ревью, круг 1: два скилла, оба — ссылки. `look` раньше
    /// обрывался на первой ссылке и отдавал одну ошибку, спрятав вторую;
    /// теперь копит, как `install`/`remove` всегда и копили. Единственный
    /// скилл в комплекте эту развилку вообще не мог проверить.
    #[test]
    fn two_broken_skills_both_get_named_not_just_the_first() {
        let (_root, kit) = kit_with_skills(&[("alpha", "альфа"), ("beta", "бета")]);
        let state = Dir::new();
        let home = Dir::new();
        let elsewhere = home.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let alpha_ok = try_link(&home.path().join("alpha"), &elsewhere);
        let beta_ok = try_link(&home.path().join("beta"), &elsewhere);
        if !alpha_ok || !beta_ok {
            eprintln!("пропущено: ссылку создать не вышло на этой машине");
            return;
        }

        let look_err = look(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(look_err.len(), 2, "{look_err:?}");
        let install_err = install(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(install_err.len(), 2, "{install_err:?}");
        let remove_err = remove(&kit, &ts(&state), home.path()).unwrap_err();
        assert_eq!(remove_err.len(), 2, "{remove_err:?}");
    }
}
