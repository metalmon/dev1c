//! Комплект: каталог, где лежит программа, файл параметров, образ и всё
//! остальное. Здесь же журнал и память между прогонами.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Kit {
    pub root: PathBuf,
    /// Куда писать своё — память между прогонами, журнал команд, файл
    /// параметров. Обычно это сам комплект: он свой и писуемый. В образе
    /// комплект один общий и смонтирован только для чтения, и тогда
    /// состояние уезжает к пользователю (`Kit::with_state`).
    state: PathBuf,
}

impl Kit {
    /// Корень — каталог программы; переменная `<ПРОГРАММА>_KIT` важнее
    /// (например `DEV1C_KIT`), для запуска из исходников.
    pub fn discover(env_var: &str) -> Result<Self> {
        if let Ok(p) = std::env::var(env_var) {
            return Ok(Self::at(p));
        }
        let exe = std::env::current_exe().context("не удалось узнать путь к программе")?;
        let dir = exe.parent().context("у программы нет каталога")?.to_path_buf();
        Ok(Self::at(dir))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self { state: root.clone(), root }
    }

    /// Комплект, из которого только читают: состояние лежит отдельно.
    pub fn with_state(root: impl Into<PathBuf>, state: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), state: state.into() }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// Путь для того, что программа пишет сама: см. поле `state`.
    pub fn state_path(&self, rel: &str) -> PathBuf {
        self.state.join(rel)
    }

    /// Сам каталог состояния — когда нужен он, а не файл в нём.
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// Журнал команд и их вывода: сюда идёт то, что не печатается в терминал.
    pub fn cmd_log(&self) -> PathBuf {
        self.state_path("setup.log")
    }

    /// Пишет в журнал то, что в терминал и в окно не идёт: команды и их
    /// сырой вывод. Возвращает true, если событие было только для журнала.
    pub fn journal(&self, e: &crate::Event) -> bool {
        let line = match e {
            crate::Event::Command(c) => format!("$ {c}"),
            crate::Event::Output { command, text } => format!("# {command}
{text}"),
            _ => return false,
        };
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.cmd_log()) {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
        true
    }

    fn state_file(&self) -> PathBuf {
        self.state_path("install.state")
    }

    /// Запомненное между прогонами, чтобы повторный запуск не спрашивал заново.
    pub fn state_get(&self, key: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.state_file()).ok()?;
        text.lines()
            .filter_map(|l| l.strip_prefix(&format!("{key}=")))
            .last()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    pub fn state_set(&self, key: &str, value: &str) -> Result<()> {
        let f = self.state_file();
        let mut lines: Vec<String> = std::fs::read_to_string(&f)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.starts_with(&format!("{key}=")))
            .map(|l| l.to_string())
            .collect();
        lines.push(format!("{key}={value}"));
        write_text(&f, &(lines.join("\n") + "\n"))
    }
}

/// Запись текста без BOM и с `\n`: файлы отсюда читают и Windows, и контейнеры.
pub fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("не удалось создать {}", dir.display()))?;
    }
    std::fs::write(path, text.replace("\r\n", "\n")).with_context(|| format!("не удалось записать {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> Kit {
        let d = std::env::temp_dir().join(format!("kit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Kit::at(d)
    }

    #[test]
    fn state_survives_and_last_value_wins() {
        let k = tmp();
        assert_eq!(k.state_get("PORT"), None);
        k.state_set("PORT", "3390").unwrap();
        k.state_set("PORT", "4000").unwrap();
        k.state_set("OTHER", "x").unwrap();
        assert_eq!(k.state_get("PORT").as_deref(), Some("4000"));
        assert_eq!(k.state_get("OTHER").as_deref(), Some("x"));
        let _ = std::fs::remove_dir_all(&k.root);
    }
}
