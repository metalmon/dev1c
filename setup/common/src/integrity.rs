//! Целость комплекта: сверка файлов с контрольными суммами.
//!
//! Комплект едет к заказчику архивом, копируется на флешку и переносится по
//! сети; любой шаг способен обрезать файл. Сверка называет виновника до того,
//! как на машине что-то изменилось.

use crate::kit::{write_text, Kit};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const LIST: &str = "checksums.txt";

#[derive(Debug, Clone, PartialEq)]
pub enum FileState {
    Ok,
    Missing,
    Changed,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub state: FileState,
}

#[derive(Debug, Default)]
pub struct Report {
    pub entries: Vec<Entry>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.entries.iter().all(|e| e.state == FileState::Ok)
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("не открыть {}", path.display()))?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

/// Сумма строки, а не файла: когда отпечаток считается из уже собранного в
/// памяти текста (список «сумма файла — его имя», например), писать его во
/// временный файл только ради того, чтобы `sha256_file` его перечитал, —
/// лишняя запись на диск, гонка с соседним процессом и riskует самой той же
/// бедой со ссылкой, от которой спасаемся. Ошибки тут нет: строка уже в
/// памяти, считать с неё нечему не удаться.
pub fn sha256_text(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

/// Что входит в список: всё, кроме самого списка, того, что программа пишет сама, файлов параметров — их человек правит по назначению, — секрета сервера модели формы docker вместе с его файлом переменных (оба появляются только на установке, в комплекте их при сборке поставки нет), и ключа шифрования паролей (`secrets::KEY_FILE` в `onec-connect` — крейты разные, имя продублировано литералом, как и у остальных файлов этого списка). На Linux каталог состояния может совпасть с корнем комплекта (`Store::system` там — тот же файловый ключ, что и `Store::file`), и без исключения ключ попал бы в `checksums.txt`, снятый до установки, — и сверка живого ключа с той суммой проваливалась бы всегда зря (Minor ревью задачи 7).
fn listed(rel: &str) -> bool {
    !(rel == LIST
        || rel.ends_with(".log")
        || rel == "install.state"
        || rel == "last-check.txt"
        || rel == "dev1c.toml"
        || rel == "1c-connect.toml"
        || rel == "dev1c-llama.token"
        || rel == "llama.env"
        || rel == "secret.key")
}

/// Что делать с найденной ссылкой — у двух вызывающих разная политика, и она
/// называется параметром, а не зашита в обходе (ревью 30.10, круг 2).
/// Комплект — наш собственный каталог: ссылки в нём заводим мы сами при
/// сборке образа (`Dockerfile.1c:170-174` кладёт туда пять ссылок, включая
/// `vrunner/lib` на каталог) — подписи комплекта (`write_list`/`verify`)
/// ссылка не помеха, и обход идёт по ней, как шёл всегда. Дом агента на
/// машине человека — чужой каталог (`onec_connect::skills::dir_sum`), и там
/// ссылка — отказ: соединение каталогов (`mklink /J` на Windows, обычная
/// учётка без прав администратора) на родителя иначе уводит обход в
/// бесконечную рекурсию, а висячая ссылка — в чужую цель.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Links {
    Follow,
    Refuse,
}

/// Обход каталога — общий для сверки комплекта и для раскладки скиллов:
/// открыт наружу, чтобы не держать вторую копию того же обхода (расхождение
/// двух копий мы уже ловили в этом проекте). `DirEntry::file_type()` типа
/// записи не идёт по ссылке сам, поэтому решение по `links` ничего не стоит.
pub fn walk(dir: &Path, links: Links, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("не прочитать {}", dir.display()))? {
        let entry = entry.with_context(|| format!("не прочитать запись в {}", dir.display()))?;
        let p = entry.path();
        let ft = entry.file_type().with_context(|| format!("не узнать тип {}", p.display()))?;
        if ft.is_symlink() && links == Links::Refuse {
            anyhow::bail!("{} — ссылка: обходить такие небезопасно", p.display());
        } else if p.is_dir() {
            walk(&p, links, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Собрать список сумм (при сборке поставки). Возвращает число файлов.
pub fn write_list(kit: &Kit, mut progress: impl FnMut(&str)) -> Result<usize> {
    let mut files = Vec::new();
    walk(&kit.root, Links::Follow, &mut files)?;
    files.sort();
    let mut lines = Vec::new();
    for p in files {
        let r = rel(&kit.root, &p);
        if !listed(&r) {
            continue;
        }
        progress(&r);
        lines.push(format!("{}  {}", sha256_file(&p)?, r));
    }
    write_text(&kit.path(LIST), &(lines.join("\n") + "\n"))?;
    Ok(lines.len())
}

/// Сверить комплект со списком.
pub fn verify(kit: &Kit, mut progress: impl FnMut(usize, usize, &str)) -> Result<Report> {
    let text = std::fs::read_to_string(kit.path(LIST)).context("в комплекте нет checksums.txt")?;
    let items: Vec<(String, String)> =
        text.lines().filter_map(|l| l.split_once("  ")).map(|(h, p)| (h.to_string(), p.to_string())).collect();
    let mut report = Report::default();
    for (i, (hash, path)) in items.iter().enumerate() {
        progress(i + 1, items.len(), path);
        let p = kit.path(path);
        let state = if !p.exists() {
            FileState::Missing
        } else if sha256_file(&p)? == *hash {
            FileState::Ok
        } else {
            FileState::Changed
        };
        report.entries.push(Entry { path: path.clone(), state });
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ссылка `link` на `target`, настоящая на обеих системах (соединение
    /// каталогов на Windows, символьная ссылка на Unix) — без прав
    /// администратора на любой. `false`, если не вышло: тест себя пропускает
    /// с объяснением, а не падает.
    #[cfg(windows)]
    fn try_link(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", &link.display().to_string(), &target.display().to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn try_link(link: &Path, target: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// Regression ревью 31.10, круг 2: защита `walk` от ссылок не имела права
    /// сломать `write_list` в контейнере — там `/opt/dev1c-kit` несёт свои
    /// же ссылки (`Dockerfile.1c:170-174`, например `vrunner/lib` на
    /// каталог), и их заводим мы сами при сборке образа. `Links::Follow`
    /// проходит ссылку, как и раньше; `Links::Refuse` (политика
    /// `onec_connect::skills::dir_sum` для чужого каталога) на той же ссылке
    /// отказывает.
    #[test]
    fn links_follow_for_the_kit_and_refuse_for_a_foreign_directory() {
        let d = std::env::temp_dir().join(format!("integrity-walk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("real")).unwrap();
        std::fs::write(d.join("real/f.txt"), "x").unwrap();
        if !try_link(&d.join("linked"), &d.join("real")) {
            eprintln!("пропущено: ссылку создать не вышло на этой машине");
            let _ = std::fs::remove_dir_all(&d);
            return;
        }

        let mut followed = Vec::new();
        walk(&d, Links::Follow, &mut followed).unwrap();
        assert!(followed.iter().any(|p| p.ends_with("f.txt")), "{followed:?}");

        let mut refused = Vec::new();
        let err = walk(&d, Links::Refuse, &mut refused).unwrap_err();
        assert!(err.to_string().contains("ссылка"), "{err}");

        let _ = std::fs::remove_dir_all(&d);
    }

    fn tmp_kit() -> Kit {
        let d = std::env::temp_dir().join(format!("integrity-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), "a").unwrap();
        std::fs::write(d.join("sub/b.bin"), [1u8, 2, 3]).unwrap();
        std::fs::write(d.join("setup.log"), "noise").unwrap();
        // The parameter files are the customer's to edit: never listed.
        std::fs::write(d.join("dev1c.toml"), "[server]\nport = 3390\n").unwrap();
        std::fs::write(d.join("1c-connect.toml"), "[host]\n").unwrap();
        // The docker form's model-server secret and its env file are
        // generated at install time, not shipped in the kit: never listed.
        std::fs::write(d.join("dev1c-llama.token"), "secret").unwrap();
        std::fs::write(d.join("llama.env"), "LLAMA_API_KEY=secret\n").unwrap();
        // Ключ шифрования паролей (`secrets::KEY_FILE` в onec-connect): на Linux каталог состояния может совпасть с корнем комплекта, и этот файл не должен попасть в checksums.txt (Minor ревью задачи 7).
        std::fs::write(d.join("secret.key"), [0u8; 32]).unwrap();
        Kit::at(d)
    }

    #[test]
    fn list_then_verify_then_detect_damage() {
        let k = tmp_kit();
        let n = write_list(&k, |_| {}).unwrap();
        assert_eq!(n, 2, "the log, the parameter files, the generated secret files and the encryption key are not listed");
        std::fs::write(k.path("dev1c.toml"), "[server]\nport = 4000\n").unwrap();
        assert!(verify(&k, |_, _, _| {}).unwrap().ok(), "editing parameters is not damage");
        assert!(verify(&k, |_, _, _| {}).unwrap().ok());
        std::fs::write(k.path("a.txt"), "changed").unwrap();
        std::fs::remove_file(k.path("sub/b.bin")).unwrap();
        let r = verify(&k, |_, _, _| {}).unwrap();
        assert!(!r.ok());
        let st = |p: &str| r.entries.iter().find(|e| e.path == p).unwrap().state.clone();
        assert_eq!(st("a.txt"), FileState::Changed);
        assert_eq!(st("sub/b.bin"), FileState::Missing);
        let _ = std::fs::remove_dir_all(&k.root);
    }
}
