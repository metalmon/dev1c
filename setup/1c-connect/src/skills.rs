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

use common::integrity::{rel, sha256_file, sha256_text, walk, Links};
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
}
