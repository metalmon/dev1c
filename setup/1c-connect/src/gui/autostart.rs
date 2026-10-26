//! Автозапуск при входе в систему: галочка в трее не хранит своего
//! состояния — она читает то, что реально стоит в системе (реестр на
//! Windows, файл `.desktop` иначе) и пишет туда же.

use std::path::Path;

/// Значение автозапуска: путь к программе в кавычках и `--tray` — именно
/// этот аргумент переводит `gui::launch` в скрытый старт (см. `main.rs`).
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

/// Содержимое `.desktop`-файла на не-Windows: тот же `run_value` в `Exec=`.
pub fn desktop_entry(exe: &Path) -> String {
    format!("[Desktop Entry]\nType=Application\nName=1c-connect\nExec={}\n", run_value(exe))
}

#[cfg(windows)]
mod imp {
    use super::run_value;
    use anyhow::{bail, Context, Result};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    };

    const SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const VALUE: &str = "1c-connect";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Значение есть в реестре → включён; отсутствует раздел или значение —
    /// выключен (а не ошибка).
    pub fn is_on() -> bool {
        let subkey = wide(SUBKEY);
        let value = wide(VALUE);
        let mut size: u32 = 0;
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        r == ERROR_SUCCESS
    }

    pub fn set(on: bool) -> Result<()> {
        if on {
            enable()
        } else {
            disable()
        }
    }

    fn enable() -> Result<()> {
        let exe = std::env::current_exe().context("не удалось узнать путь к программе")?;
        let data = wide(&run_value(&exe));
        let subkey = wide(SUBKEY);
        let mut hkey: HKEY = std::ptr::null_mut();
        let r = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            )
        };
        if r != ERROR_SUCCESS {
            bail!("не открыть раздел автозапуска в реестре (код {r})");
        }
        let value = wide(VALUE);
        // REG_SZ — байты UTF-16 с замыкающим нулём, который уже есть в `data`.
        let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2) };
        let set_r = unsafe { RegSetValueExW(hkey, value.as_ptr(), 0, REG_SZ, bytes.as_ptr(), bytes.len() as u32) };
        unsafe { RegCloseKey(hkey) };
        if set_r != ERROR_SUCCESS {
            bail!("не записать значение автозапуска (код {set_r})");
        }
        Ok(())
    }

    fn disable() -> Result<()> {
        let subkey = wide(SUBKEY);
        let mut hkey: HKEY = std::ptr::null_mut();
        let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_SET_VALUE | KEY_QUERY_VALUE, &mut hkey) };
        if r == ERROR_FILE_NOT_FOUND {
            // Раздела нет — автозапуска и не было, это не ошибка.
            return Ok(());
        }
        if r != ERROR_SUCCESS {
            bail!("не открыть раздел автозапуска в реестре (код {r})");
        }
        let value = wide(VALUE);
        let del_r = unsafe { RegDeleteValueW(hkey, value.as_ptr()) };
        unsafe { RegCloseKey(hkey) };
        if del_r != ERROR_SUCCESS && del_r != ERROR_FILE_NOT_FOUND {
            bail!("не удалить значение автозапуска (код {del_r})");
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::desktop_entry;
    use anyhow::{Context, Result};
    use common::kit::write_text;
    use std::path::PathBuf;

    fn file_path() -> PathBuf {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
        base.join("autostart").join("1c-connect.desktop")
    }

    pub fn is_on() -> bool {
        file_path().exists()
    }

    pub fn set(on: bool) -> Result<()> {
        let path = file_path();
        if on {
            let exe = std::env::current_exe().context("не удалось узнать путь к программе")?;
            return write_text(&path, &desktop_entry(&exe));
        }
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            // Файла нет — автозапуска и не было, это не ошибка.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).context("не удалить файл автозапуска"),
        }
    }
}

/// Включён ли автозапуск прямо сейчас — читает систему, своего хранения нет.
pub fn is_on() -> bool {
    imp::is_on()
}

/// Включить/выключить автозапуск.
pub fn set(on: bool) -> anyhow::Result<()> {
    imp::set(on)
}

#[cfg(test)]
mod tests {
    use super::{desktop_entry, run_value};
    use std::path::Path;

    #[test]
    fn run_value_quotes_the_exe_and_appends_tray() {
        assert_eq!(run_value(Path::new(r"C:\dev1c\1c-connect.exe")), "\"C:\\dev1c\\1c-connect.exe\" --tray");
    }

    #[test]
    fn desktop_entry_is_the_fixed_four_lines() {
        assert_eq!(
            desktop_entry(Path::new("/opt/dev1c/1c-connect")),
            "[Desktop Entry]\nType=Application\nName=1c-connect\nExec=\"/opt/dev1c/1c-connect\" --tray\n"
        );
    }
}
