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

/// Что входит в список: всё, кроме самого списка, того, что программа пишет
/// сама, файлов параметров — их человек правит по назначению, — и секрета
/// сервера модели формы docker вместе с его файлом переменных: оба
/// появляются только на установке, в комплекте их при сборке поставки нет.
fn listed(rel: &str) -> bool {
    !(rel == LIST
        || rel.ends_with(".log")
        || rel == "install.state"
        || rel == "last-check.txt"
        || rel == "dev1c.toml"
        || rel == "1c-connect.toml"
        || rel == "dev1c-llama.token"
        || rel == "llama.env")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir).with_context(|| format!("не прочитать {}", dir.display()))? {
        let p = e?.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Собрать список сумм (при сборке поставки). Возвращает число файлов.
pub fn write_list(kit: &Kit, mut progress: impl FnMut(&str)) -> Result<usize> {
    let mut files = Vec::new();
    walk(&kit.root, &mut files)?;
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
        Kit::at(d)
    }

    #[test]
    fn list_then_verify_then_detect_damage() {
        let k = tmp_kit();
        let n = write_list(&k, |_| {}).unwrap();
        assert_eq!(n, 2, "the log, the parameter files and the generated secret files are not listed");
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
