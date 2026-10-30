//! Скиллы агентов: готовые тексты про 1С, которые галочка в трее
//! («Локальные агенты») кладёт в каталог скилла агента — второе дело
//! галочки, рядом с записями MCP. Правило переносится, а не придумывается
//! заново: оно уже работает в точке входа образа с 14.10 (`config/entrypoint.sh`,
//! цикл по `/etc/skel/.agents/skills`) — «правь готовые и заводи новые»
//! значит, что затирать нельзя никогда, кроме случая «комплект обновился, а
//! человек место не трогал». Этот модуль — фундамент: дом агента, отпечаток
//! каталога скилла и вердикт по отпечатку. Саму раскладку (`install`/`remove`)
//! заводит следующая задача.

use common::integrity::sha256_file;
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

/// Имена файлов каталога `dir`, относительно `root`, со слэшем вперёд —
/// обход диска читает их в произвольном порядке, а `dir_sum` ниже должен
/// давать одну и ту же сумму независимо от него.
fn relative_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            relative_files(root, &path, out)?;
        } else {
            out.push(path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Отпечаток каталога скилла — той же мыслью, что `dir_sum` в точке входа
/// образа (`find … -exec md5sum {} + | sort | md5sum`): сумма каждого файла
/// по имени относительно каталога, список отсортирован по имени — не по
/// порядку обхода диска, который ничем не гарантирован, — и уже
/// отсортированный список хешируется снова, через временный файл: второй
/// независимый хеш-алгоритм ради одной сверки — лишняя зависимость без
/// причины, а `sha256_file` считает файлы, не строки.
pub fn dir_sum(dir: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    relative_files(dir, dir, &mut files).map_err(|e| e.to_string())?;
    files.sort();
    let mut text = String::new();
    for rel in &files {
        let sum = sha256_file(&dir.join(rel)).map_err(|e| e.to_string())?;
        text.push_str(&sum);
        text.push_str("  ");
        text.push_str(rel);
        text.push('\n');
    }
    let tmp = std::env::temp_dir().join(format!(
        "dev1c-dirsum-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::write(&tmp, &text).map_err(|e| e.to_string())?;
    let sum = sha256_file(&tmp).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    sum
}

/// Где лежит отпечаток последней раскладки — в состоянии программы, не в
/// доме агента: один файл на пару (дом, имя скилла), чтобы раскладка для
/// Zed/pi (`Home::Agents`) и для Claude Code (`Home::Claude`) не путала
/// отпечатки друг друга в общем каталоге состояния.
pub fn stamp_path(state: &Path, h: Home, skill: &str) -> PathBuf {
    let tag = match h {
        Home::Agents => "agents",
        Home::Claude => "claude",
    };
    state.join(format!("skill_{tag}_{skill}"))
}

/// Прочитанный отпечаток — `None`, если файла нет: первой раскладке так и
/// положено, прошлого отпечатка у неё нет.
pub fn read_stamp(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Записать отпечаток после раскладки.
pub fn write_stamp(path: &Path, sum: &str) -> Result<(), String> {
    std::fs::write(path, sum).map_err(|e| e.to_string())
}

/// Три случая и отказ — как в точке входа образа: каталога нет → `New`; есть,
/// и отпечаток совпал с прежним → `Untouched`; есть, и не совпал → `Edited`
/// (человек правил скилл на месте — из комплекта его не трогают); на месте
/// скилла ссылка → `Link`, отказ раньше прочих случаев.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    New,
    Untouched,
    Edited,
    Link,
}

/// Решение по ссылке — от одного факта метаданных (`is_symlink`), не от
/// самого пути: выделено отдельно ради теста. Висячая ссылка на месте
/// скилла опаснее прочего — `exists()` на ней ложно, и запись пошла бы по
/// ней в чужую цель, — поэтому в `verdict` этот случай проверяется первым,
/// раньше `New`.
fn link_verdict(is_symlink: bool) -> Option<Verdict> {
    is_symlink.then_some(Verdict::Link)
}

/// `std::fs::symlink_metadata`, не `metadata`: вторая идёт по ссылке на цель,
/// первая — нет (тем же ходом, что `llama.rs::ensure_dir`).
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

/// Вердикт для каталога `dst` по его прежнему отпечатку `stamp`.
pub fn verdict(dst: &Path, stamp: Option<&str>) -> Result<Verdict, String> {
    if let Some(v) = link_verdict(is_symlink(dst)) {
        return Ok(v);
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

        // На месте скилла ссылка → Link. Настоящую ссылку на этой машине не
        // создать без прав администратора (тот же потолок, что у
        // `ops_host.rs::the_same_tree_seen_through_a_symlink_appears_once` и
        // `editors.rs::setup_canonicalizes_an_existing_path_before_writing`),
        // поэтому здесь доказывается механизм — функция решения по факту
        // метаданных, — а не весь сценарий целиком.
        assert_eq!(link_verdict(true), Some(Verdict::Link));
        assert_eq!(link_verdict(false), None);
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
}
