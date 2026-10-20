//! У рабочего места один владелец (спека): окно (`--tray`/без аргументов),
//! `connect` и `serve` держат порты, мьютекс проекта (`project::LOCK`) и, с
//! недавнего, свою публикацию базы — второй долгоживущий процесс не должен
//! отнимать их у первого. Разовые команды (`status`, `export`, `import`,
//! `check`, `install`, `example` и прочие) этот замок не трогают: они и так
//! serialised через `project::LOCK`, а задержать их здесь означало бы
//! поломать обычную работу человека.
//!
//! Переживаемость краха — главное свойство замка:
//! - Windows: именованный мьютекс — объект ядра. `CreateMutexW` второй раз
//!   отдаёт «уже есть» (`ERROR_ALREADY_EXISTS`) ровно пока первый процесс
//!   жив — система сама снимает объект вместе с процессом, даже убитым
//!   (`taskkill /F`, обрыв питания), и руками снимать нечего.
//! - Linux: советный `flock` на файле в каталоге состояния. Замок висит на
//!   файловом дескрипторе живого процесса и снимается ядром при его закрытии
//!   — тоже независимо от того, вышел процесс штатно или был убит. Сам файл
//!   может остаться на диске (это не ошибка) — следующий запуск просто берёт
//!   замок поверх него, без ручной уборки.
//!
//! Ключ замка — пользователь ОС и рабочее место из конфигурации
//! (`cfg::Host::workstation`), а не машина: два разных человека на одной
//! машине или два рабочих места — не конфликт, оба замка свободны и берутся
//! одновременно (и так же по разным машинам — замок локален для каждой).
//!
//! Вторая копия не умирает молча. На Windows ищет окно первой — по её pid,
//! записанному рядом с остальным состоянием комплекта тем же вызовом, что
//! взял замок, — и поднимает его, тихо завершаясь; окна не нашлось (голова
//! `serve`, `connect`, или трей с ещё не открытым окном) — печатает причину
//! и выходит с ошибкой. На Linux — только вторая половина: подняться дешёвым
//! способом (без своего протокола между процессами) нечем, сразу сообщение
//! и отказ.

use common::kit::Kit;

/// Пользователь ОС: `USERNAME` на Windows, `USER` (резерв `LOGNAME`) на
/// Linux. Не нашли — общий `?` на всех без логина в окружении: замок при
/// этом не менее надёжен, просто не различил бы несколько таких учёток на
/// одной машине (случай, которого на наших рабочих местах не бывает).
fn current_user() -> String {
    let found = if cfg!(windows) { std::env::var("USERNAME") } else { std::env::var("USER").or_else(|_| std::env::var("LOGNAME")) };
    found.unwrap_or_else(|_| "?".into())
}

/// Ключ замка: пользователь и рабочее место, без символов, опасных для имени
/// мьютекса или файла (пробелы, кириллица, `\`/`/`) — они гасятся в `_`.
pub fn guard_key(user: &str, workstation: &str) -> String {
    fn clean(s: &str) -> String {
        s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
    }
    format!("{}-{}", clean(user), clean(workstation))
}

/// Файл с pid держателя замка, рядом с остальным состоянием комплекта
/// (`install.state`, `1c-connect.log`) — отдельного каталога под замок не
/// заводим. Содержимое — вспомогательное: используется только чтобы найти
/// окно первой копии на Windows, в самом решении «занято или нет» участия не
/// принимает (то решает `try_acquire`), так что устаревшее значение (прежний
/// процесс не успел его обновить или уже не существует) не опаснее, чем
/// отсутствие файла — `raise_other` просто не найдёт окно.
fn pid_file(kit: &Kit, key: &str) -> std::path::PathBuf {
    kit.path(&format!("instance-{key}.pid"))
}

fn remember_pid(kit: &Kit, key: &str) {
    let _ = std::fs::write(pid_file(kit, key), std::process::id().to_string());
}

#[cfg(windows)]
pub struct Guard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY: `self.0` — дескриптор, полученный этим же `Guard` из
        // `CreateMutexW` и ещё не закрытый (второй раз `Drop` не зовут).
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

// Дескриптор мьютекса — просто ссылка на объект ядра, переносить его между
// потоками безопасно (как и `Job` в job.rs).
#[cfg(windows)]
unsafe impl Send for Guard {}
#[cfg(windows)]
unsafe impl Sync for Guard {}

/// Взять замок по ключу или увидеть, что он уже занят. `state_dir` — каталог
/// состояния комплекта; на Windows не используется (там замок не файловый),
/// но остаётся параметром, чтобы вызывающему было не важно, какая платформа.
#[cfg(windows)]
pub fn try_acquire(key: &str, _state_dir: &std::path::Path) -> std::io::Result<Option<Guard>> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = format!("1c-connect-instance-{key}");
    let wide: Vec<u16> = OsStr::new(&name).encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: обычный вызов kernel32 с нуль-терминированной строкой, живущей
    // на стеке этого вызова; без атрибутов безопасности — именованный объект
    // по умолчанию виден только в сессии вызывающего, что уже отделяет
    // разных пользователей друг от друга.
    let h = unsafe { CreateMutexW(std::ptr::null(), 1, wide.as_ptr()) };
    if h.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // `GetLastError` — первым же следующим вызовом, до любого другого Win32
    // API: иначе он рассказал бы не про `CreateMutexW`.
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already {
        unsafe { CloseHandle(h) };
        return Ok(None);
    }
    Ok(Some(Guard(h)))
}

#[cfg(not(windows))]
pub struct Guard(std::fs::File);

/// Советный `flock`: занимается открытым дескриптором живого процесса и
/// снимается ядром, когда дескриптор закрывается — обычным уходом `File` из
/// области видимости (`Drop` ничего не пишет сам, добавлять его незачем).
#[cfg(not(windows))]
pub fn try_acquire(key: &str, state_dir: &std::path::Path) -> std::io::Result<Option<Guard>> {
    use std::os::unix::io::AsRawFd;
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(format!("instance-{key}.lock"));
    let file = std::fs::OpenOptions::new().create(true).write(true).open(&path)?;
    // SAFETY: обычный вызов libc на дескрипторе только что открытого файла;
    // LOCK_NB не блокирует поток — занято, значит просто отказ, а не ожидание.
    let r = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if r == 0 {
        return Ok(Some(Guard(file)));
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(None)
    } else {
        Err(err)
    }
}

/// Найти окно уже идущей копии по её pid и поднять его на передний план.
/// По классу окна искать нельзя: GPUI (`gpui-kit`) регистрирует общий класс
/// `Zed::Window` для любого приложения на этом тулките — на рабочем месте
/// может оказаться настоящий Zed, и значение «нашли окно» означало бы «нашли
/// чьё угодно окно». По pid, записанному самим держателем замка, двусмысленности
/// нет: `EnumWindows` видит все верхнеуровневые окна, а нам годится только то,
/// что принадлежит именно этому процессу.
#[cfg(windows)]
fn raise_window_of(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, SW_RESTORE};

    struct Search {
        pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: `lparam` — адрес `Search` на стеке `raise_window_of`, живой
        // на всё время `EnumWindows` (вызов синхронный, ниже на этом же стеке).
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut owner = 0u32;
        // SAFETY: обычный вызов user32, `hwnd` — валидная ручка, переданная
        // самим `EnumWindows`; указатель на локальную переменную этой функции.
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == search.pid {
            search.found = hwnd;
            return 0; // хватит, нашли
        }
        1 // продолжить перечисление
    }

    let mut search = Search { pid, found: std::ptr::null_mut() };
    // SAFETY: `visit` не хранит `lparam` за пределами своего вызова, `search`
    // переживает весь вызов `EnumWindows` (он синхронный).
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    if search.found.is_null() {
        return false;
    }
    // SAFETY: `search.found` — ручка, которую только что отдал сам Windows
    // через `EnumWindows`; оба вызова — обычные user32 без побочных эффектов
    // на память этого процесса.
    unsafe {
        ShowWindow(search.found, SW_RESTORE);
        SetForegroundWindow(search.found);
    }
    true
}

#[cfg(windows)]
fn raise_other(kit: &Kit, key: &str) -> bool {
    match std::fs::read_to_string(pid_file(kit, key)).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(pid) => raise_window_of(pid),
        None => false,
    }
}

/// На Linux поднимать окно дешёвым способом нечем (ни общего протокола между
/// процессами, ни того же GPUI-окна в headless-сборке без фичи `gui`) — вторая
/// копия сразу получает отказ с сообщением, без попытки.
#[cfg(not(windows))]
fn raise_other(_kit: &Kit, _key: &str) -> bool {
    false
}

/// Взять замок рабочего места или решить, что делать со второй копией:
/// бесшумно уступить (нашли и подняли окно первой — Windows) либо вернуть
/// читаемую причину отказа. Держите возвращённый `Guard` до конца процесса —
/// ранний `drop` снимает замок раньше срока.
pub fn acquire(kit: &Kit, workstation: &str) -> anyhow::Result<Guard> {
    let key = guard_key(&current_user(), workstation);
    match try_acquire(&key, &kit.root)? {
        Some(guard) => {
            remember_pid(kit, &key);
            Ok(guard)
        }
        None => {
            if raise_other(kit, &key) {
                std::process::exit(0);
            }
            anyhow::bail!("рабочее место {workstation} занято: коннектор для него уже запущен — закройте его перед повторным запуском")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ключ теста: уникален на каждый вызов (счётчик плюс pid процесса
    /// `cargo test`) — как `tempdir::Dir` в `project.rs`, иначе параллельные
    /// тесты этого файла спорили бы за один и тот же мьютекс/файл.
    fn unique(tag: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        format!("{tag}-{}-{n}", std::process::id())
    }

    #[test]
    fn guard_key_sanitises_spaces_and_cyrillic() {
        let k = guard_key("dev 1", "рабочее место");
        assert!(!k.contains(' ') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{k}");
    }

    #[test]
    fn a_second_acquire_of_the_same_key_fails_while_the_first_is_held_and_succeeds_after_it_drops() {
        let dir = std::env::temp_dir();
        let key = unique("same-key");
        let first = try_acquire(&key, &dir).unwrap().expect("ключ свободен");
        let second = try_acquire(&key, &dir).unwrap();
        assert!(second.is_none(), "второй захват должен увидеть занятый замок");
        drop(first);
        let third = try_acquire(&key, &dir).unwrap();
        assert!(third.is_some(), "после ухода держателя замок должен снова взяться");
    }

    #[test]
    fn two_different_workstations_never_collide() {
        let dir = std::env::temp_dir();
        let key_a = unique("dev1");
        let key_b = unique("dev2");
        let a = try_acquire(&key_a, &dir).unwrap().expect("первое рабочее место свободно");
        let b = try_acquire(&key_b, &dir).unwrap();
        assert!(b.is_some(), "второе рабочее место не должно видеть замок первого");
        drop(a);
        drop(b);
    }
}
