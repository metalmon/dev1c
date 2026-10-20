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
//!   (`taskkill /F`, обрыв питания), и руками снимать нечего. Файла на диске
//!   для этого замка не было и нет.
//! - Linux: советный `flock` на файле, который уже существует и который мы
//!   сами никогда не пишем, — `1c-connect.toml` (`ops_host::toml_path`).
//!   Отдельного файла-замка больше нет: общий комплект теперь один на все
//!   рабочие места и доступен только для чтения, а advisory-замок держится
//!   на дескрипторе, открытом НА ЧТЕНИЕ, — это работает даже если каталог,
//!   где лежит toml, вообще нельзя писать. Замок висит на дескрипторе живого
//!   процесса и снимается ядром при его закрытии — независимо от того,
//!   вышел процесс штатно или был убит.
//!
//! Ключ замка (`guard_key`) — пользователь ОС и рабочее место из
//! конфигурации (`cfg::Host::workstation`), а не машина: два разных
//! человека на одной машине или два рабочих места — не конфликт. На Windows
//! это имя мьютекса; на Linux отдельный ключ не нужен — свой toml у каждого
//! рабочего места и так свой файл, значит и свой замок.
//!
//! Вторая копия не умирает молча. На Windows ищет окно первой — перебирает
//! все верхнеуровневые окна и поднимает то, чей процесс — тот же исполняемый
//! файл, что и наш собственный (`same_image`/`process_image`), без единого
//! файла на диске для этого; окна не нашлось (голова `serve`, `connect`,
//! или трей с ещё не открытым окном) — печатает причину и выходит с
//! ошибкой. На Linux — только вторая половина: подняться дешёвым способом
//! (без своего протокола между процессами) нечем, сразу сообщение и отказ.

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

/// Взять замок по ключу или увидеть, что он уже занят. `lock_on` — файл,
/// который держит замок на Linux (`ops_host::toml_path`); на Windows не
/// используется (там замок не файловый, а именованный мьютекс), но остаётся
/// параметром, чтобы вызывающему было не важно, какая платформа.
#[cfg(windows)]
pub fn try_acquire(key: &str, _lock_on: &std::path::Path) -> std::io::Result<Option<Guard>> {
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
        // Та же причина, что и у файлового замка на Linux ниже (`lock_error`):
        // голый `Permission denied (os error N)` без единого слова о том, что
        // вообще делалось, ничего человеку не говорит.
        let e = std::io::Error::last_os_error();
        return Err(std::io::Error::new(e.kind(), format!("не удалось взять замок единственного экземпляра (системный мьютекс {name}): {e}")));
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

/// Оборачивает отказ открыть файл замка в читаемую причину: что делалось
/// (брали замок единственного экземпляра), по какому пути, и голый текст
/// системной ошибки в хвосте — для диагностики, не для первого взгляда.
/// Файл открывается только на чтение (ниже), так что обычная причина отказа
/// — не отсутствие прав на запись (их и не нужно), а то, что файла вообще
/// нет: `toml_path` зовут уже после проверки, что `1c-connect.toml`
/// существует, так что на практике это, скорее, незрелый вызов не оттуда.
#[cfg(not(windows))]
fn lock_error(path: &std::path::Path, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("не удалось взять замок единственного экземпляра: {} — файл не открылся: {e}", path.display()))
}

/// Советный `flock` на файле, который мы никогда не пишем (`1c-connect.toml`,
/// `ops_host::toml_path`) — занимается открытым на ЧТЕНИЕ дескриптором живого
/// процесса и снимается ядром, когда дескриптор закрывается — обычным уходом
/// `File` из области видимости (`Drop` ничего не пишет сам, добавлять его
/// незачем). Открытие на чтение, а не на запись, — единственное, что даёт
/// замку работать даже на общем комплекте, смонтированном только для чтения:
/// `flock` занимается самим дескриптором, а не правом писать через него.
/// `key` здесь не участвует — файл уже свой у каждого рабочего места (его
/// toml), так что отдельный ключ внутри него не нужен.
#[cfg(not(windows))]
pub fn try_acquire(_key: &str, lock_on: &std::path::Path) -> std::io::Result<Option<Guard>> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new().read(true).open(lock_on).map_err(|e| lock_error(lock_on, e))?;
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

/// Найти окно уже идущей копии и поднять его на передний план — без единого
/// файла на диске: перебирает все верхнеуровневые окна (`EnumWindows`) и для
/// каждого проверяет образ процесса-владельца против своего собственного
/// (`same_image`/`process_image`), пропуская самого себя. По классу окна
/// искать нельзя: GPUI (`gpui-kit`) регистрирует общий класс `Zed::Window`
/// для любого приложения на этом тулките — на рабочем месте может оказаться
/// настоящий Zed, и значение «нашли окно» означало бы «нашли чьё угодно
/// окно»; по образу исполняемого файла двусмысленности нет.
#[cfg(windows)]
fn raise_window_of_our_other_instance(ours: &std::path::Path) -> bool {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, SW_RESTORE};

    struct Search {
        ours: std::path::PathBuf,
        my_pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: `lparam` — адрес `Search` на стеке вызывающей функции,
        // живой на всё время `EnumWindows` (вызов синхронный, ниже на этом
        // же стеке).
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut owner = 0u32;
        // SAFETY: обычный вызов user32, `hwnd` — валидная ручка, переданная
        // самим `EnumWindows`; указатель на локальную переменную этой функции.
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner != search.my_pid && same_image(process_image(owner).as_deref(), &search.ours) {
            search.found = hwnd;
            return 0; // хватит, нашли
        }
        1 // продолжить перечисление
    }

    let mut search = Search { ours: ours.to_path_buf(), my_pid: std::process::id(), found: std::ptr::null_mut() };
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

/// Решение «процесс за этим pid — правда наша первая копия», отдельно от
/// того, как добыт путь образа — чтобы его можно было проверить без
/// Win32-вызовов. `candidate` — что система называет образом этого pid
/// (`None` — процесса нет, или прочитать не вышло: закрыт доступ, слишком
/// быстро вышел); `ours` — свой образ (`std::env::current_exe`). Пустого
/// совпадения «оба `None`» нет и не может быть — `ours` всегда `&Path`, не
/// `Option`.
fn same_image(candidate: Option<&std::path::Path>, ours: &std::path::Path) -> bool {
    candidate == Some(ours)
}

/// Путь образа процесса `pid`, как его называет сама система —
/// `QueryFullProcessImageNameW` по `OpenProcess` с минимальными правами
/// (`PROCESS_QUERY_LIMITED_INFORMATION` хватает даже для процесса не под
/// текущим пользователем, в отличие от `PROCESS_QUERY_INFORMATION`). `None`
/// — процесса с таким pid уже нет, он нам недоступен, или путь не прочитать:
/// во всех случаях вызывающему должно быть всё равно, это не наша копия.
#[cfg(windows)]
fn process_image(pid: u32) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: обычный вызов kernel32, pid — число из файла (может называть
    // не существующий или чужой процесс — на этот случай и есть проверка
    // `is_null`), прав запрошено по минимуму.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` — буфер на стеке этой функции нужного размера, `len` —
    // указатель на локальную переменную с его вместимостью в элементах;
    // `h` только что получен от `OpenProcess` и ещё не закрыт.
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    unsafe { CloseHandle(h) };
    if ok == 0 {
        return None;
    }
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// Поднять окно первой копии — но только после того, как сам процесс
/// подтвердил, что это действительно мы, а не чужая программа: без этой
/// проверки раньше можно было тихо (`exit(0)`, без единой строки) поднять
/// окно случайного соседа — ровно та молчаливая смерть, которую спека
/// запрещает; любой другой исход (процесса нет, образ не совпал, окна у
/// подтверждённого процесса не нашлось) — это «не нашли», а не «нашли
/// что-то».
#[cfg(windows)]
fn raise_other() -> bool {
    let Ok(ours) = std::env::current_exe() else { return false };
    raise_window_of_our_other_instance(&ours)
}

/// На Linux поднимать окно дешёвым способом нечем (ни общего протокола между
/// процессами, ни того же GPUI-окна в headless-сборке без фичи `gui`) — вторая
/// копия сразу получает отказ с сообщением, без попытки.
#[cfg(not(windows))]
fn raise_other() -> bool {
    false
}

/// Взять замок рабочего места или решить, что делать со второй копией:
/// бесшумно уступить (нашли и подняли окно первой — Windows) либо вернуть
/// читаемую причину отказа. Держите возвращённый `Guard` до конца процесса —
/// ранний `drop` снимает замок раньше срока.
pub fn acquire(kit: &Kit, workstation: &str) -> anyhow::Result<Guard> {
    let key = guard_key(&current_user(), workstation);
    let lock_on = crate::ops_host::toml_path(kit);
    match try_acquire(&key, &lock_on)? {
        Some(guard) => Ok(guard),
        None => {
            if raise_other() {
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
    fn same_image_accepts_our_own_and_rejects_a_different_or_missing_process() {
        let ours = std::path::Path::new(r"C:\kit\1c-connect.exe");
        assert!(same_image(Some(ours), ours), "наш собственный образ должен совпасть");
        let other = std::path::Path::new(r"C:\Windows\notepad.exe");
        assert!(!same_image(Some(other), ours), "чужой образ не должен приниматься за нашу копию");
        assert!(!same_image(None, ours), "процесса нет — не наша копия");
    }

    #[test]
    fn guard_key_sanitises_spaces_and_cyrillic() {
        let k = guard_key("dev 1", "рабочее место");
        assert!(!k.contains(' ') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{k}");
    }

    /// Файл, который якобы уже существует (toml): на Linux `try_acquire`
    /// больше не создаёт файл замка сам — он открывает то, что ему дали.
    fn existing_file(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("{}.toml", unique(tag)));
        std::fs::write(&p, "").unwrap();
        p
    }

    #[test]
    fn a_second_acquire_of_the_same_key_fails_while_the_first_is_held_and_succeeds_after_it_drops() {
        let file = existing_file("same-key");
        let first = try_acquire("unused", &file).unwrap().expect("файл свободен");
        let second = try_acquire("unused", &file).unwrap();
        assert!(second.is_none(), "второй захват должен увидеть занятый замок");
        drop(first);
        let third = try_acquire("unused", &file).unwrap();
        assert!(third.is_some(), "после ухода держателя замок должен снова взяться");
        let _ = std::fs::remove_file(&file);
    }

    #[cfg(unix)]
    #[test]
    fn a_permission_failure_names_the_path() {
        // Файл замка теперь открывается только на чтение (toml мы не пишем),
        // так что обычная причина отказа — не отсутствие прав на запись, а
        // то, что открыть файл вообще не вышло; `try_acquire` обязан назвать
        // путь, а не отдать голый `Permission denied (os error 13)`.
        use std::os::unix::fs::PermissionsExt;
        let file = existing_file("perm");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        // ponytail: в контейнере тесты иногда бегут от root — тогда права не
        // проверяются вовсе, и это не повод валить тест: просто ничего не
        // утверждаем про текст ошибки, раз её и не было.
        if let Err(err) = try_acquire("unused", &file) {
            let msg = err.to_string();
            assert!(msg.contains(&file.display().to_string()), "путь должен быть назван: {msg}");
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn two_different_files_never_collide() {
        // В деле ключ больше не участвует на Linux — два рабочих места
        // отличаются своим файлом (своим toml), не строкой-ключом.
        let a = existing_file("dev1");
        let b = existing_file("dev2");
        let ga = try_acquire("unused", &a).unwrap().expect("первый файл свободен");
        let gb = try_acquire("unused", &b).unwrap();
        assert!(gb.is_some(), "замок на другом файле не должен видеть замок первого");
        drop(ga);
        drop(gb);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn the_lock_lives_in_the_state_not_in_the_kit() {
        // Замок на Linux держится на toml, найденном через
        // `ops_host::toml_path` — тот же путь, что `acquire` передаёт в
        // `try_acquire`. Он обязан лежать в состоянии пользователя и никогда
        // внутри общего комплекта (тот — только для чтения и один на всех).
        // Windows эта задача не трогает: там toml по-прежнему при комплекте.
        if cfg!(windows) {
            return;
        }
        let kit = Kit::at(std::path::Path::new("/opt/dev1c-kit"));
        let lock_on = crate::ops_host::toml_path(&kit);
        assert!(!lock_on.starts_with(&kit.root), "{}", lock_on.display());
        assert!(lock_on.starts_with(crate::ops_host::tools_dir()), "{}", lock_on.display());
    }
}
