//! Скиллы агентов: готовые тексты про 1С, которые галочка в трее
//! («Локальные агенты») кладёт в каталог скилла агента — второе дело
//! галочки, рядом с записями MCP. Правило переносится, а не придумывается
//! заново: оно уже работает в точке входа образа с 14.10 (`config/entrypoint.sh`,
//! цикл по `/etc/skel/.agents/skills`) — «правь готовые и заводи новые»
//! значит, что затирать нельзя никогда, кроме случая «комплект обновился, а
//! человек место не трогал». Этот модуль — фундамент: дом агента, отпечаток
//! каталога скилла и вердикт по отпечатку. Саму раскладку (`install`/`remove`)
//! заводит следующая задача.
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
/// человека — какой каталог назначения туда класть, решает вызывающий код
/// следующей задачи (`install`/`remove` берут его параметром, а не зовут
/// `home_dir` сами).
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
pub fn dir_sum(dir: &Path) -> Result<String, String> {
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

/// Где лежит отпечаток последней раскладки — в состоянии программы, не в
/// доме агента. Имя файла несёт три вещи: дом (`agents`/`claude`), короткую
/// сумму каталога НАЗНАЧЕНИЯ и имя скилла — без каталога назначения в ключе
/// два разных места с одним домом делили бы один отпечаток, и чужой скилл
/// сочли бы нашим (Important 3 ревью 30.10); сам путь в имя файла не идёт
/// (длина, разделители) — только его сумма. Отказ, если имя скилла не
/// простое (Important 4).
pub fn stamp_path(state: &Path, h: Home, dst: &Path, skill: &str) -> Result<PathBuf, String> {
    if !valid_skill_name(skill) {
        return Err(format!("{skill:?} — не простое имя скилла, отпечаток не строю"));
    }
    let tag = match h {
        Home::Agents => "agents",
        Home::Claude => "claude",
    };
    let dst_sum = &sha256_text(&dst.to_string_lossy())[..12];
    Ok(state.join(format!("skill_{tag}_{dst_sum}_{skill}")))
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
pub fn read_stamp(path: &Path) -> Result<Option<String>, String> {
    refuse_if_link(path)?;
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("не прочитать {}: {e}", path.display())),
    }
}

/// Записать отпечаток после раскладки.
pub fn write_stamp(path: &Path, sum: &str) -> Result<(), String> {
    refuse_if_link(path)?;
    std::fs::write(path, sum).map_err(|e| format!("не записать {}: {e}", path.display()))
}

/// Три случая и отказ — как в точке входа образа: каталога нет → `New`; есть,
/// и отпечаток совпал с прежним → `Untouched`; есть, и не совпал → `Edited`
/// (человек правил скилл на месте — из комплекта его не трогают); на месте
/// скилла ссылка → `Link`, отказ раньше прочих случаев — висячая ссылка не
/// видна `exists()`, а запись пошла бы по ней в чужую цель.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    New,
    Untouched,
    Edited,
    Link,
}

/// Вердикт для каталога `dst` по его прежнему отпечатку `stamp`.
pub fn verdict(dst: &Path, stamp: Option<&str>) -> Result<Verdict, String> {
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
/// `look`/`install`/`remove` их переберут. `Ok(None)`, если раздела `skills/`
/// в комплекте вовсе нет — решение хозяина: комплект без скиллов не отказ,
/// записи MCP у него всё равно работают (эта разница видна в `Found::Missing`
/// у вызывающего, не в комментарии здесь). Имя скилла не по правилу
/// (`valid_skill_name`) — отказ словами сразу: по решению задания пропускать
/// его молча нельзя, а раз оно внутри самого комплекта, это неисправность
/// комплекта, а не дома агента.
fn kit_skill_names(kit: &Kit) -> Result<Option<Vec<String>>, String> {
    let dir = kit.path(KIT_DIR);
    if !dir.exists() {
        return Ok(None);
    }
    let mut names = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("не прочитать {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("не прочитать {}: {e}", dir.display()))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !valid_skill_name(&name) {
            return Err(format!("{name:?} в комплекте — не простое имя скилла, раскладку не веду"));
        }
        names.push(name);
    }
    names.sort();
    Ok(Some(names))
}

/// Итоговое слово о скиллах в доме — тем же словарём, что у MCP-записей
/// редактора (`editors::Found`), чтобы окно показывало обе половины галочки
/// одним языком: хоть один скилл занят чужим (правлен человеком или вовсе не
/// наш) — `Foreign` с именами, раскладку это не останавливает, но и не
/// трогает их; иначе все наши и на месте — `Ours`; иначе (что-то ещё не
/// поставлено) — `Absent`. Общая для `look`/`install`/`remove`, чтобы три
/// функции не придумывали три разных способа сказать одно и то же.
fn summarize(total: usize, foreign: Vec<String>, ours: usize) -> editors::Found {
    if !foreign.is_empty() {
        editors::Found::Foreign(foreign)
    } else if total > 0 && ours == total {
        editors::Found::Ours
    } else {
        editors::Found::Absent
    }
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
/// вызывающему.
pub fn look(kit: &Kit, state: &Path, h: Home, home: &Path) -> Result<editors::Found, String> {
    let Some(names) = kit_skill_names(kit)? else { return Ok(editors::Found::Missing) };
    let mut foreign = Vec::new();
    let mut ours = 0usize;
    for name in &names {
        let dst = home.join(name);
        let stamp = stamp_path(state, h, &dst, name)?;
        let prev = read_stamp(&stamp)?;
        match verdict(&dst, prev.as_deref())? {
            Verdict::Link => return Err(format!("{} — ссылка на месте скилла, не трогаю", dst.display())),
            Verdict::Untouched => ours += 1,
            Verdict::Edited => foreign.push(name.clone()),
            Verdict::New => {}
        }
    }
    Ok(summarize(names.len(), foreign, ours))
}

/// Поставить и обновить — разницы в правиле нет, ровно так устроен
/// контейнер (`container-rule.txt`): каждый скилл по своему вердикту. Нет
/// или не тронут — копируем свежее из комплекта и пишем отпечаток (это и
/// есть обновление, когда комплект успел поменяться, и безвредный повтор,
/// когда не успел). Правлен человеком — не трогаем, называем в `Foreign`.
/// Ссылка на его месте — отказ словами, но только для этого скилла: ошибка
/// одного не должна останавливать раскладку остальных, поэтому они собираются
/// в список, а не обрывают цикл.
///
/// Внутри «правлен» — особый случай: содержимое СЕЙЧАС совпадает с тем, что
/// даёт комплект, хотя своего отпечатка нет вовсе. Это не правка человека —
/// это недоделанная прошлая раскладка (процесс прервался между копией и
/// записью отпечатка). Такую докладываем отпечатком и считаем своей, а не
/// отказываем — иначе половина раскладки застряла бы навсегда, будучи
/// неотличимой словами от настоящей правки.
pub fn install(kit: &Kit, state: &Path, h: Home, home: &Path) -> Result<editors::Found, Vec<String>> {
    let names = match kit_skill_names(kit) {
        Ok(Some(n)) => n,
        Ok(None) => return Ok(editors::Found::Missing),
        Err(e) => return Err(vec![e]),
    };
    let mut errors = Vec::new();
    let mut foreign = Vec::new();
    let mut ours = 0usize;
    for name in &names {
        let src = kit.path(KIT_DIR).join(name);
        let dst = home.join(name);
        let outcome = (|| -> Result<bool, String> {
            let stamp = stamp_path(state, h, &dst, name)?;
            let prev = read_stamp(&stamp)?;
            match verdict(&dst, prev.as_deref())? {
                Verdict::Link => Err(format!("{} — ссылка на месте скилла, не трогаю", dst.display())),
                Verdict::New | Verdict::Untouched => {
                    copy_skill(&src, &dst)?;
                    let sum = dir_sum(&dst)?;
                    write_stamp(&stamp, &sum)?;
                    Ok(true)
                }
                Verdict::Edited => {
                    let src_sum = dir_sum(&src)?;
                    let dst_sum = dir_sum(&dst)?;
                    if dst_sum == src_sum {
                        write_stamp(&stamp, &src_sum)?;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
            }
        })();
        match outcome {
            Ok(true) => ours += 1,
            Ok(false) => foreign.push(name.clone()),
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
pub fn remove(kit: &Kit, state: &Path, h: Home, home: &Path) -> Result<editors::Found, Vec<String>> {
    let names = match kit_skill_names(kit) {
        Ok(Some(n)) => n,
        Ok(None) => return Ok(editors::Found::Missing),
        Err(e) => return Err(vec![e]),
    };
    let mut errors = Vec::new();
    let mut foreign = Vec::new();
    for name in &names {
        let dst = home.join(name);
        let outcome = (|| -> Result<bool, String> {
            let stamp = stamp_path(state, h, &dst, name)?;
            let prev = read_stamp(&stamp)?;
            match verdict(&dst, prev.as_deref())? {
                Verdict::Link => Err(format!("{} — ссылка на месте скилла, не трогаю", dst.display())),
                Verdict::Untouched => {
                    std::fs::remove_dir_all(&dst).map_err(|e| format!("не удалить {}: {e}", dst.display()))?;
                    if stamp.exists() {
                        std::fs::remove_file(&stamp).map_err(|e| format!("не удалить {}: {e}", stamp.display()))?;
                    }
                    Ok(true)
                }
                Verdict::New => Ok(true),
                Verdict::Edited => Ok(false),
            }
        })();
        match outcome {
            Ok(true) => {}
            Ok(false) => foreign.push(name.clone()),
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
        assert!(stamp_path(t.path(), Home::Agents, &t.path().join("1c"), "../../etc").is_err());
        assert!(stamp_path(t.path(), Home::Agents, &t.path().join("1c"), "..").is_err());
        assert!(stamp_path(t.path(), Home::Agents, &t.path().join("1c"), "1c").is_ok());
    }

    #[test]
    fn stamp_path_tells_apart_two_destinations_with_the_same_home_and_skill() {
        let state = Dir::new();
        let one = stamp_path(state.path(), Home::Agents, &PathBuf::from("/home/a/.agents/skills"), "1c").unwrap();
        let other = stamp_path(state.path(), Home::Agents, &PathBuf::from("/home/b/.agents/skills"), "1c").unwrap();
        assert_ne!(one, other, "разные каталоги назначения не должны делить один отпечаток");
    }

    /// Комплект с одним скиллом внутри `skills/` — своя небольшая обёртка,
    /// чтобы шесть проверок задачи 3 не повторяли одно и то же устройство
    /// комплекта по пять строк каждая.
    fn kit_with_skill(name: &str, text: &str) -> (Dir, common::kit::Kit) {
        let root = Dir::new();
        std::fs::create_dir_all(root.path().join(KIT_DIR).join(name)).unwrap();
        std::fs::write(root.path().join(KIT_DIR).join(name).join("SKILL.md"), text).unwrap();
        let kit = common::kit::Kit::at(root.path());
        (root, kit)
    }

    #[test]
    fn install_copies_the_kit_skills_and_writes_their_stamps() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();

        let found = install(&kit, state.path(), Home::Agents, home.path()).unwrap();
        assert_eq!(found, editors::Found::Ours);

        let dst = home.path().join("1c");
        assert_eq!(std::fs::read_to_string(dst.join("SKILL.md")).unwrap(), "текст скилла");
        let stamp = stamp_path(state.path(), Home::Agents, &dst, "1c").unwrap();
        assert_eq!(read_stamp(&stamp).unwrap().unwrap(), dir_sum(&dst).unwrap());
    }

    /// Папка с нашим именем уже есть, но мы её не клали — отпечатка для неё
    /// никогда не было. `look` называет её чужой; `install` её не
    /// перезаписывает; `remove` не уносит.
    #[test]
    fn a_skill_we_never_put_there_is_left_alone() {
        let (_root, kit) = kit_with_skill("1c", "из комплекта");
        let state = Dir::new();
        let home = Dir::new();
        std::fs::create_dir_all(home.path().join("1c")).unwrap();
        std::fs::write(home.path().join("1c/SKILL.md"), "чужое содержимое").unwrap();

        assert_eq!(look(&kit, state.path(), Home::Agents, home.path()).unwrap(), editors::Found::Foreign(vec!["1c".to_string()]));

        let after_install = install(&kit, state.path(), Home::Agents, home.path()).unwrap();
        assert_eq!(after_install, editors::Found::Foreign(vec!["1c".to_string()]));
        assert_eq!(std::fs::read_to_string(home.path().join("1c/SKILL.md")).unwrap(), "чужое содержимое");

        let after_remove = remove(&kit, state.path(), Home::Agents, home.path()).unwrap();
        assert_eq!(after_remove, editors::Found::Foreign(vec!["1c".to_string()]));
        assert!(home.path().join("1c").exists());
    }

    #[test]
    fn an_edited_skill_survives_both_update_and_removal_and_is_named() {
        let (_root, kit) = kit_with_skill("1c", "версия раз");
        let state = Dir::new();
        let home = Dir::new();
        install(&kit, state.path(), Home::Agents, home.path()).unwrap();

        // Человек правит поставленный скилл на месте.
        std::fs::write(home.path().join("1c/SKILL.md"), "правка человека").unwrap();

        // Комплект обновился — но правленое install не трогает.
        std::fs::write(_root.path().join(KIT_DIR).join("1c/SKILL.md"), "версия два").unwrap();
        let after_install = install(&kit, state.path(), Home::Agents, home.path()).unwrap();
        assert_eq!(after_install, editors::Found::Foreign(vec!["1c".to_string()]));
        assert_eq!(std::fs::read_to_string(home.path().join("1c/SKILL.md")).unwrap(), "правка человека");

        let after_remove = remove(&kit, state.path(), Home::Agents, home.path()).unwrap();
        assert_eq!(after_remove, editors::Found::Foreign(vec!["1c".to_string()]));
        assert!(home.path().join("1c").exists(), "правленый скилл не должен уноситься");
    }

    #[test]
    fn a_new_kit_replaces_the_untouched_skills() {
        let (root, kit) = kit_with_skill("1c", "версия раз");
        let state = Dir::new();
        let home = Dir::new();
        install(&kit, state.path(), Home::Agents, home.path()).unwrap();

        // Скилл в доме не тронут — комплект меняет версию.
        std::fs::write(root.path().join(KIT_DIR).join("1c/SKILL.md"), "версия два").unwrap();
        let found = install(&kit, state.path(), Home::Agents, home.path()).unwrap();
        assert_eq!(found, editors::Found::Ours);
        assert_eq!(std::fs::read_to_string(home.path().join("1c/SKILL.md")).unwrap(), "версия два");
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

        let look_err = look(&kit, state.path(), Home::Agents, home.path()).unwrap_err();
        assert!(look_err.contains("ссылка"), "{look_err}");
        let install_err = install(&kit, state.path(), Home::Agents, home.path()).unwrap_err();
        assert_eq!(install_err.len(), 1);
        assert!(install_err[0].contains("ссылка"), "{install_err:?}");
        let remove_err = remove(&kit, state.path(), Home::Agents, home.path()).unwrap_err();
        assert_eq!(remove_err.len(), 1);
        assert!(remove_err[0].contains("ссылка"), "{remove_err:?}");
    }

    /// Раскладка прервалась между копией и записью отпечатка: в доме лежит
    /// каталог, байт в байт совпадающий с тем, что даёт комплект СЕЙЧАС, а
    /// своего отпечатка для него нет вовсе. Это не правка человека — это
    /// недоделанное дело, и `install` обязан его завершить (дописать
    /// отпечаток), а не отказать, сочтя чужим.
    #[test]
    fn a_half_deleted_install_is_completed_not_refused() {
        let (_root, kit) = kit_with_skill("1c", "текст скилла");
        let state = Dir::new();
        let home = Dir::new();
        std::fs::create_dir_all(home.path().join("1c")).unwrap();
        std::fs::write(home.path().join("1c/SKILL.md"), "текст скилла").unwrap();

        let found = install(&kit, state.path(), Home::Agents, home.path()).unwrap();
        assert_eq!(found, editors::Found::Ours);

        let dst = home.path().join("1c");
        let stamp = stamp_path(state.path(), Home::Agents, &dst, "1c").unwrap();
        assert_eq!(read_stamp(&stamp).unwrap().unwrap(), dir_sum(&dst).unwrap());
    }
}
