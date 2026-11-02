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
//! - Linux: советный `flock` на КАТАЛОГЕ состояния рабочего места
//!   (`Kit::state`). Отдельного файла-замка нет: `flock` держится за
//!   дескриптор, открытый НА ЧТЕНИЕ, так что писать в каталог не нужно — это
//!   и даёт замку работать, когда комплект смонтирован только для чтения.
//!   Каталог, а не файл параметров: toml человеку прямо предлагают править, а
//!   редактор, сохраняющий через временный файл и `rename`, меняет inode —
//!   замок остался бы висеть на отвязанном inode и пустил бы вторую копию.
//!   Каталога это не касается: его никто не пересоздаёт, и он уже есть к
//!   этому моменту (`ops_host::ensure_state`), даже когда toml ещё не написан —
//!   иначе окно, которое toml и создаёт, не смогло бы открыться.
//!   Замок висит на дескрипторе живого процесса и снимается ядром при его
//!   закрытии — независимо от того, вышел процесс штатно или был убит.
//!
//! Ключ замка (`guard_key`) — пользователь ОС, не машина и не рабочее место
//! из конфигурации (задача 6 плана «тонкий срез ценности»: терминальный
//! сервер и несколько сеансов одного пользователя — обычный случай, а имя
//! рабочего места в toml больше не свойство замка). На Windows это имя
//! мьютекса; на Linux отдельный ключ не нужен — каталог состояния там один
//! на учётную запись ОС, а в образе стенда у каждого рабочего места своя
//! учётная запись (dev1, dev2, dev3), так что замок и так свой.
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
///
/// `pub(crate)`: подкоманде `1c-connect mcp` (`ops_host::run_mcp`) нужен тот
/// же пользователь, что и замку рабочего места — для `allow::watch`, который
/// проверяет живость того же владельца (`guard_key`), не свой второй способ
/// узнать логин.
pub(crate) fn current_user() -> String {
    let found = if cfg!(windows) { std::env::var("USERNAME") } else { std::env::var("USER").or_else(|_| std::env::var("LOGNAME")) };
    found.unwrap_or_else(|_| "?".into())
}

/// Без символов, опасных для имени мьютекса или файла (пробелы, кириллица,
/// `\`/`/`) — они гасятся в `_`. Общая для `guard_key` (пользователь и
/// рабочее место) и для ключа файлового замка проекта (сам путь).
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

/// Ключ замка: пользователь ОС, без символов, опасных для имени мьютекса или
/// файла (пробелы, кириллица, `\`/`/`) — они гасятся в `_`. С задачи 6 плана
/// «тонкий срез ценности» рабочее место в ключе больше не участвует: у
/// рабочего места был один владелец, а терминальный сервер и несколько
/// сеансов одного пользователя — обычный случай, и замок обязан различать
/// СЕАНСЫ, а не имя, вписанное в toml. Один пользователь — один коннектор,
/// сколько бы баз он ни обслуживал внутри (контракт подключения: несколько
/// баз живут карточками в одном приложении, а не копиями программы).
pub fn guard_key(user: &str) -> String {
    clean(user)
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

/// Общая часть приёма замка на Windows: `CreateMutexW` по готовому имени.
/// Вынесена, чтобы её звали оба замка, различающиеся только тем, как
/// построено имя — `try_acquire` (ключ рабочего места) и `project::file_lock`
/// (ключ — сам путь, через `pub(crate) fn file_lock` ниже).
#[cfg(windows)]
fn acquire_mutex(name: &str) -> std::io::Result<Option<Guard>> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let wide: Vec<u16> = OsStr::new(name).encode_wide().chain(std::iter::once(0)).collect();
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
        return Err(std::io::Error::new(e.kind(), format!("не удалось взять замок (системный мьютекс {name}): {e}")));
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

/// Имя системного мьютекса замка рабочего места — единственное место, где оно строится; `try_acquire` и `is_held` обязаны звать именно эту функцию, а не собирать свою строку (находка ревью задачи 2: вторая формулировка имени смотрела бы не на тот объект, что держит замок).
#[cfg(windows)]
fn instance_mutex_name(key: &str) -> String {
    format!("1c-connect-instance-{key}")
}

/// Взять замок по ключу или увидеть, что он уже занят. `lock_on` — файл,
/// который держит замок на Linux (`Kit::state_path`); на Windows не
/// используется (там замок не файловый, а именованный мьютекс), но остаётся
/// параметром, чтобы вызывающему было не важно, какая платформа.
#[cfg(windows)]
pub fn try_acquire(key: &str, _lock_on: &std::path::Path) -> std::io::Result<Option<Guard>> {
    acquire_mutex(&instance_mutex_name(key))
}

/// Живость владельца рабочего места БЕЗ попытки занять замок: `OpenMutexW` по тому же имени, что строит `instance_mutex_name` — дескриптор открылся, значит объект жив и держит его чей-то процесс (закрываем тотчас, нам нужен только факт), `ERROR_FILE_NOT_FOUND` — владельца нет. В отличие от `try_acquire`/`CreateMutexW` здесь нет стороны «занял — замок мой»: `is_held` ничем не владеет, и поэтому два читателя (`allow::watch` и прямой `allow::read`, или два процесса подкоманды `mcp`) не заверяют живость друг друга, застав один тик другого (находка ревью задачи 2: раньше проверка сама звала `try_acquire`, и та доля секунды, что один проверяющий держал замок, была для соседнего проверяющего «занято — значит жив», хотя настоящий владелец уже мёртв).
#[cfg(windows)]
pub fn is_held(key: &str, _lock_on: &std::path::Path) -> std::io::Result<bool> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND};
    use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let name = instance_mutex_name(key);
    let wide: Vec<u16> = OsStr::new(&name).encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: обычный вызов kernel32 с нуль-терминированной строкой, живущей на стеке этого вызова; `OpenMutexW` не создаёт объект и не берёт владение им — только открывает дескриптор на уже существующий, если такой есть.
    let h = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide.as_ptr()) };
    if h.is_null() {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
            return Ok(false);
        }
        return Err(std::io::Error::new(e.kind(), format!("не удалось проверить замок (системный мьютекс {name}): {e}")));
    }
    // SAFETY: `h` — дескриптор, который только что отдал `OpenMutexW`.
    unsafe { CloseHandle(h) };
    Ok(true)
}

/// Имя мьютекса «этот процесс обслуживает данный комплект» — отдельный
/// объект от замка рабочего места (`instance_mutex_name`), и нарочно: ключ —
/// пользователь ОС и ПУТЬ комплекта (`lock_target`), рабочего места в нём
/// нет вовсе.
///
/// Круг исправления после ревью: сторож по коннектору (`orphan::guard_owner`)
/// сперва смотрел в тот же замок, что держит окно — `instance_mutex_name`,
/// ключ которого несёт имя рабочего места. Оно не заморожено: «Сохранить»
/// меняет его у открытого окна без перезапуска (спека, `gui/save.rs`,
/// `moved_lines` — ветка проекта, порты и прочее едут за новым именем).
/// Сторож спрашивал замок уже под новым именем, видел «не держат» и уводил
/// мост насмерть при живом коннекторе — ровно та беда, против которой
/// заведена спячка (`bridge::EpochEnd::Dormant`), только с другой стороны.
/// Замораживать имя на старте моста не лечит: мост, запущенный ПОСЛЕ
/// переименования, смотрел бы на замок, которого никто не держит, увидел
/// `Owner::NotYet` и остался бы жить вечно.
///
/// Значит привязка не должна зависеть от того, что меняется на ходу. Путь
/// комплекта (`lock_target`) не меняется без перезапуска самого коннектора
/// — вот и весь ключ.
#[cfg(windows)]
fn serving_mutex_name(user: &str, kit: &Kit) -> String {
    format!("1c-connect-serving-{}-{}", clean(user), clean(&lock_target(kit).display().to_string()))
}

/// Закрепить «этот процесс обслуживает данный комплект» — объектом ядра,
/// держать до выхода (та же дисциплина, что `acquire`: ранний `drop` снимает
/// признак раньше срока). Зовётся РЯДОМ с `acquire` (`main.rs::connect`/
/// `serve`, `gui::launch`), не вместо него — у двух объектов разный смысл:
/// `acquire` — «этот коннектор единственный на рабочее место», этот —
/// «комплект, который сторож моста обязан увидеть живым».
///
/// `None` — тот же комплект уже обслуживает другой процесс этого
/// пользователя (включая случай, не защищённый замком рабочего места: тот
/// же комплект поднят дважды под разными именами, потому что toml
/// переименовали руками между запусками, не закрыв первый) — редкий и
/// предсуществующий случай, который эта правка не обязана новым отказом
/// запрещать; вызывающий просто не держит признак, и сторож у НЕГО не
/// заведётся на пользу (увидит `Owner::NotYet` и будет жить заглушкой).
#[cfg(windows)]
pub fn acquire_serving(kit: &Kit, user: &str) -> std::io::Result<Option<Guard>> {
    acquire_mutex(&serving_mutex_name(user, kit))
}

/// Видит ли сторож коннектора (`orphan::guard_owner`), что комплект сейчас
/// кем-то обслуживается — без попытки занять признак, тем же приёмом, что
/// `is_held` у замка рабочего места.
#[cfg(windows)]
pub fn is_serving(kit: &Kit, user: &str) -> std::io::Result<bool> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND};
    use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let name = serving_mutex_name(user, kit);
    let wide: Vec<u16> = OsStr::new(&name).encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: см. `is_held` — тот же приём, другое имя.
    let h = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide.as_ptr()) };
    if h.is_null() {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
            return Ok(false);
        }
        return Err(std::io::Error::new(e.kind(), format!("не удалось проверить признак обслуживания (мьютекс {name}): {e}")));
    }
    // SAFETY: `h` — дескриптор, который только что отдал `OpenMutexW`.
    unsafe { CloseHandle(h) };
    Ok(true)
}

/// Замок проекта (`project::file_lock`) на Windows: тот же приём, что у
/// замка рабочего места — именованный мьютекс, живущий, пока жив процесс,
/// независимо от того, как он кончился. Ключ строится из пути, а не из
/// пользователя и рабочего места: файл замка и так свой на каждое рабочее
/// место (`<state>/project.lock`), второй ключ не добавил бы ничего, а
/// собственный префикс (`1c-connect-file-`, в отличие от
/// `1c-connect-instance-` у замка рабочего места) не даёт случайно столкнуться
/// именам из двух разных целей.
/// ponytail: ключ — путь, прогнанный через `clean`, без канонизации; два
/// разных пути, отличающихся только заменяемыми символами, теоретически
/// могли бы совпасть. Другого пути к файлу замка, кроме как из
/// `set_lock_target`, у программы нет, так что потолок не достигается.
#[cfg(windows)]
pub(crate) fn file_lock(path: &std::path::Path) -> std::io::Result<Option<Guard>> {
    acquire_mutex(&format!("1c-connect-file-{}", clean(&path.display().to_string())))
}

/// Замок живёт, пока жив дескриптор: файл здесь держат, а не читают —
/// `flock` снимается закрытием. Поэтому поле и не читается никем.
#[cfg(not(windows))]
pub struct Guard(#[allow(dead_code)] std::fs::File);

/// Оборачивает отказ открыть замок в читаемую причину: по какому пути, и
/// голый текст системной ошибки в хвосте — для диагностики, не для первого
/// взгляда. Общая для обоих вызовов `open` ниже: `try_acquire` открывает
/// существующий каталог состояния только на чтение (прав на запись там и не
/// нужно, отказ обычно значит, что `ops_host::ensure_state` не создал сам
/// каталог), а `file_lock` создаёт свой файл и пишет в него.
#[cfg(not(windows))]
fn lock_error(path: &std::path::Path, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("не удалось взять замок: {} — файл не открылся: {e}", path.display()))
}

/// Общая часть приёма замка на Linux: советный `flock` на уже открытом
/// дескрипторе. Вынесена, чтобы её звали оба замка — `try_acquire` (открывает
/// только на чтение то, что не пишет сам) и `project::file_lock` (создаёт
/// свой файл, если его ещё нет, — см. ниже).
#[cfg(not(windows))]
fn acquire_flock(file: std::fs::File) -> std::io::Result<Option<Guard>> {
    use std::os::unix::io::AsRawFd;
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

/// Советный `flock` на том, что мы никогда не пишем — каталоге состояния
/// рабочего места (`Kit::state`). Занимается открытым на ЧТЕНИЕ дескриптором
/// живого процесса и снимается ядром, когда дескриптор закрывается — обычным
/// уходом `File` из области видимости (`Drop` ничего не пишет сам, добавлять
/// его незачем). Открытие на чтение, а не на запись, — единственное, что
/// даёт замку работать даже на общем комплекте, смонтированном только для
/// чтения: `flock` занимается самим дескриптором, а не правом писать через
/// него. `key` здесь не участвует — каталог уже свой у каждого рабочего
/// места, так что отдельный ключ внутри него не нужен. Каталог открывается
/// тем же `OpenOptions`, что файл: `flock` всё равно, что за дескриптором.
#[cfg(not(windows))]
pub fn try_acquire(_key: &str, lock_on: &std::path::Path) -> std::io::Result<Option<Guard>> {
    let file = std::fs::OpenOptions::new().read(true).open(lock_on).map_err(|e| lock_error(lock_on, e))?;
    acquire_flock(file)
}

/// Живость владельца БЕЗ попытки занять замок: `/proc/locks` по устройству и inode `lock_on` — тем же приёмом, что `holder.rs` берёт держателя порта в образе среды, где нет ни `ss`, ни `netstat`. В отличие от `try_acquire` здесь не открывается даже совместный `flock` на основном пути — чтение `/proc/locks` не трогает замок вовсе, и два читателя (`allow::watch` и прямой `allow::read`, два процесса подкоманды `mcp`) не видят тика друг друга как «владелец жив» (находка ревью задачи 2: раньше проверка сама звала `try_acquire`, и доля секунды, что один проверяющий держал `flock`, была для соседнего проверяющего «занято — значит жив»).
#[cfg(not(windows))]
pub fn is_held(_key: &str, lock_on: &std::path::Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(lock_on).map_err(|e| lock_error(lock_on, e))?;
    if let Ok(locks) = std::fs::read_to_string("/proc/locks") {
        return Ok(parse_proc_locks(&locks, meta.dev(), meta.ino()));
    }
    // ponytail: без `/proc/locks` (не Linux, песочница без `/proc`) резерв — разделяемый `flock`: не конфликтует с другим читателем вроде нас самих, но конфликтует с `LOCK_EX` владельца. Потолок — своё окно между этой попыткой и самым стартом владельца (до его собственного `flock`), в которое проверка отдаст «свободно» при уже бегущем, но не успевшем взять замок процессе; решает его только настоящий `/proc/locks`.
    let file = std::fs::OpenOptions::new().read(true).open(lock_on).map_err(|e| lock_error(lock_on, e))?;
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` — дескриптор только что открытого файла; `LOCK_NB` не блокирует поток.
    let r = unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) };
    if r == 0 {
        // SAFETY: тот же дескриптор — снимаем свою же пробную отметку.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        Ok(false)
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(true)
        } else {
            Err(err)
        }
    }
}

/// Видит ли сторож коннектора (`orphan::guard_owner`), что комплект сейчас
/// кем-то обслуживается. На Linux отдельного объекта под это не заводим —
/// и не потому, что лениво, а потому, что заводить нечего: `lock_target`
/// (каталог состояния, на котором стоит `flock` замка рабочего места) и так
/// не зависит от имени рабочего места — оно живёт только в содержимом toml и
/// никогда не входит ни в путь, ни в ключ `is_held` (параметр `_key` здесь
/// игнорируется — круговое разделение рабочих мест на стенде идёт через
/// отдельный `HOME`/`XDG_DATA_HOME` каждой учётной записи, не через текст в
/// имени объекта, см. заголовок модуля). Значит тот же `flock`, что держит
/// `acquire`, уже и есть признак «этот процесс обслуживает данный
/// комплект» — второй объект или вторая проверка ничего не добавили бы,
/// только разошлись бы с первой при следующей правке. `user` не участвует
/// по той же причине, что и у `is_held` на этой платформе.
#[cfg(not(windows))]
pub fn is_serving(kit: &Kit, _user: &str) -> std::io::Result<bool> {
    is_held("", &lock_target(kit))
}

/// Строка `/proc/locks` вида `1: FLOCK ADVISORY WRITE 2206 08:01:7092817 0 EOF` — `true`, если устройство и inode совпадают с тем, что держит `try_acquire` (тот берёт только `LOCK_EX`, значит режим в строке всегда `WRITE`): `major:minor` ядро печатает раздельно, `libc::makedev` складывает их в то же число, которым `MetadataExt::dev` называет устройство — собирать формулу самим незачем.
#[cfg(not(windows))]
pub fn parse_proc_locks(out: &str, dev: u64, ino: u64) -> bool {
    out.lines().any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 6 || f[1] != "FLOCK" || f[3] != "WRITE" {
            return false;
        }
        let mut id = f[5].split(':');
        let (Some(maj), Some(min), Some(inode)) = (id.next(), id.next(), id.next()) else { return false };
        let (Ok(maj), Ok(min), Ok(inode)) = (maj.parse::<u32>(), min.parse::<u32>(), inode.parse::<u64>()) else { return false };
        libc::makedev(maj, min) as u64 == dev && inode == ino
    })
}

/// Замок проекта (`project::file_lock`) на Linux: `flock` на файле
/// `<state>/project.lock` — своём, не на каталоге состояния, который уже
/// держит замок рабочего места (занять его второй раз значило бы запереть
/// само окно). В отличие от `try_acquire` файл создаём, если его ещё нет:
/// каталог состояния гарантирован `ops_host::ensure_state`, а этот файл —
/// нет, его первым создаёт первый же процесс, который взял замок.
#[cfg(not(windows))]
pub(crate) fn file_lock(path: &std::path::Path) -> std::io::Result<Option<Guard>> {
    let file =
        std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path).map_err(|e| lock_error(path, e))?;
    acquire_flock(file)
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
///
/// Только Windows: единственный, кто его зовёт, — поиск окна первой копии, а
/// он сам под `cfg(windows)`; без этого признака на Linux осталась бы
/// неиспользуемая функция (предупреждение сборки).
#[cfg(windows)]
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
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
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

/// На чём держим замок: каталог состояния рабочего места. На Windows он в
/// деле не участвует (замок — именованный объект ядра, файловую систему там
/// не трогают), но путь всё равно нужен единой подписи `try_acquire`.
pub fn lock_target(kit: &Kit) -> std::path::PathBuf {
    kit.state().to_path_buf()
}

/// Взять замок — один коннектор на СЕАНС пользователя, не на рабочее место
/// (задача 6) — или решить, что делать со второй копией: бесшумно уступить
/// (нашли и подняли окно первой — Windows) либо вернуть читаемую причину
/// отказа. Держите возвращённый `Guard` до конца процесса — ранний `drop`
/// снимает замок раньше срока.
pub fn acquire(kit: &Kit) -> anyhow::Result<Guard> {
    let key = guard_key(&current_user());
    let lock_on = lock_target(kit);
    match try_acquire(&key, &lock_on)? {
        Some(guard) => Ok(guard),
        None => {
            if raise_other() {
                std::process::exit(0);
            }
            anyhow::bail!("{}", taken())
        }
    }
}

/// Взять признак обслуживания лучшим усилием, рядом с `acquire`
/// (`main.rs::connect`/`serve`, `gui::launch`) — держать до конца процесса,
/// переменная остаётся на стеке вызывающей функции. Пользователя не просит
/// параметром (тем же приёмом, что `acquire` берёт `current_user()` сам): он
/// `pub(crate)`, а `main.rs`/`gui::launch` — другой крейт (бинарь), не этот.
///
/// Неудача (тот же комплект уже обслуживает другой процесс этого
/// пользователя под другим именем рабочего места — предсуществующий случай,
/// см. `acquire_serving`; или системная ошибка) не останавливает запуск:
/// признака без него нет, и сторож моста по коннектору
/// (`orphan::guard_owner`) просто не заведётся ему на пользу — увидит
/// `Owner::NotYet` и будет жить заглушкой, не хуже поведения без сторожа
/// вовсе. Один вызов на оба `main.rs` и `gui/mod.rs` — им не нужно знать,
/// что на Linux держать нечего (см. `is_serving`).
#[cfg(windows)]
pub fn hold_serving(kit: &Kit) -> Option<Guard> {
    match acquire_serving(kit, &current_user()) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("признак «обслуживает этот комплект» не закрепился: {e} — сторож моста по коннектору не заведётся ему на пользу");
            None
        }
    }
}

/// На Linux отдельного признака нет — `is_serving` там смотрит в тот же
/// `flock`, что уже держит `acquire`; держать отдельно нечего.
#[cfg(not(windows))]
pub fn hold_serving(_kit: &Kit) -> Option<Guard> {
    None
}

/// Отказ второй копии словами. С задачи 6 один текст на оба режима: замок
/// держит СЕАНС пользователя, не рабочее место (раньше в туннельном режиме
/// текст называл его по имени — «рабочее место dev2 занято», — а в локальном
/// поле пусто по замыслу, и человек читал «рабочее место  занято», находка
/// аудита 30.10; теперь называть поле нечем и не для чего, замок на него не
/// смотрит вовсе). Чистая функция — замок в тесте не берётся.
fn taken() -> String {
    "коннектор уже запущен в этом сеансе — закройте его перед повторным запуском".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Красный тест задачи 6, буквально из брифа: ключ замка смотрит на
    /// пользователя, а не на рабочее место — два коннектора одного человека
    /// теперь конфликтуют всегда, независимо от того, что вписано в
    /// `workstation`.
    #[test]
    fn one_connector_per_user_session_not_per_workstation() {
        assert_eq!(guard_key("owner"), guard_key("owner"));
        assert_ne!(guard_key("owner"), guard_key("someone"));
    }

    /// Текст отказа второй копии не называет ни рабочего места, ни пустой
    /// строки на его месте: прежде в туннельном режиме он называл имя
    /// («рабочее место dev2 занято»), а в локальном поле пусто по замыслу, и
    /// человек читал «рабочее место  занято» (находка аудита 30.10). С
    /// задачи 6 замок смотрит на сеанс пользователя в обоих режимах — и
    /// текст один на оба.
    #[test]
    fn the_second_copy_refuses_without_naming_a_workstation() {
        let t = taken();
        assert!(t.contains("коннектор уже запущен"), "{t}");
        assert!(!t.contains("рабоч"), "{t}");
    }

    fn unique(tag: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        format!("{tag}-{}-{n}", std::process::id())
    }

    /// Только Windows — вместе с самой `same_image` (на Linux окно первой
    /// копии не ищут, и функции там нет).
    #[cfg(windows)]
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
        let k = guard_key("Пользователь 1");
        assert!(!k.contains(' ') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{k}");
    }

    /// Файл, который якобы уже существует: на Linux `try_acquire` ничего не
    /// создаёт сам — он открывает то, что ему дали. Нужен одному тесту про
    /// права, а он только для Unix.
    #[cfg(unix)]
    fn existing_file(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("{}.toml", unique(tag)));
        std::fs::write(&p, "").unwrap();
        p
    }

    /// То, на чём замок держится в деле, — каталог состояния рабочего места
    /// (`lock_target`).
    fn existing_dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(unique(tag));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_second_acquire_of_the_same_key_fails_while_the_first_is_held_and_succeeds_after_it_drops() {
        let dir = existing_dir("same-key");
        let key = unique("same-key");
        let first = try_acquire(&key, &dir).unwrap().expect("каталог свободен");
        let second = try_acquire(&key, &dir).unwrap();
        assert!(second.is_none(), "второй захват должен увидеть занятый замок");
        drop(first);
        let third = try_acquire(&key, &dir).unwrap();
        assert!(third.is_some(), "после ухода держателя замок должен снова взяться");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Та самая проверка, которую раньше делали через `try_acquire`
    /// (находка ревью задачи 2) — `is_held` обязана видеть владельца и его
    /// уход, но без собственного захвата: держатель остаётся держателем всё
    /// время теста, `is_held` не отнимает замок.
    #[test]
    fn is_held_sees_the_owner_without_taking_the_lock() {
        let dir = existing_dir("is-held");
        let key = unique("is-held");
        assert!(!is_held(&key, &dir).unwrap(), "никто не держал — владельца нет");
        let guard = try_acquire(&key, &dir).unwrap().expect("каталог свободен");
        assert!(is_held(&key, &dir).unwrap(), "владелец держит — is_held обязана увидеть это без захвата");
        // Замок остался у `guard`: если бы `is_held` сама его забирала, второй
        // `try_acquire` здесь бы это обнаружил. Проверяем ровно то, от чего
        // предостерегало ревью, — что `is_held` ничего не трогает.
        assert!(try_acquire(&key, &dir).unwrap().is_none(), "is_held не должна была отнять замок у guard");
        drop(guard);
        assert!(!is_held(&key, &dir).unwrap(), "владелец ушёл — is_held обязана это увидеть");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_permission_failure_names_the_path() {
        // Замок открывается только на чтение (ни файл параметров, ни каталог
        // состояния мы для него не пишем), так что обычная причина отказа —
        // не отсутствие прав на запись, а то, что открыть вообще не вышло;
        // `try_acquire` обязан назвать путь, а не отдать голый
        // `Permission denied (os error 13)`.
        use std::os::unix::fs::PermissionsExt;
        let file = existing_file("perm");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        // ponytail: в контейнере тесты иногда бегут от root — тогда права не
        // проверяются вовсе, и это не повод валить тест: просто ничего не
        // утверждаем про текст ошибки, раз её и не было.
        if let Err(err) = try_acquire(&unique("perm"), &file) {
            let msg = err.to_string();
            assert!(msg.contains(&file.display().to_string()), "путь должен быть назван: {msg}");
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = std::fs::remove_file(&file);
    }

    /// Пример строки взят из замечания ревью буквально. Чужое устройство,
    /// чужой inode, режим `READ` (разделяемый, не наш `LOCK_EX`) — ничего из
    /// этого не должно совпасть.
    #[cfg(unix)]
    #[test]
    fn parse_proc_locks_matches_device_and_inode_not_just_presence() {
        let line = "1: FLOCK ADVISORY WRITE 2206 08:01:7092817 0 EOF\n";
        // 08:01 — major 8, minor 1.
        let dev = libc::makedev(8, 1) as u64;
        assert!(parse_proc_locks(line, dev, 7092817), "устройство и inode совпадают — держат");
        assert!(!parse_proc_locks(line, dev, 999), "другой inode на том же устройстве — не держат");
        assert!(!parse_proc_locks(line, libc::makedev(8, 2) as u64, 7092817), "другое устройство — не держат");
        assert!(!parse_proc_locks("1: FLOCK ADVISORY READ 2206 08:01:7092817 0 EOF\n", dev, 7092817), "READ — не наш LOCK_EX");
        assert!(!parse_proc_locks("", dev, 7092817));
    }

    // Два разных пользователя (или два разных сеанса) не конфликтуют.
    // Проверяется на обеих ОС, но разным: на Linux их различает сам каталог
    // замка (ключ там не участвует), на Windows — ключ (`guard_key`), а
    // файловую систему замок не трогает вовсе. Это единственное свойство,
    // отделяющее один замок от другого, и ломается оно первым, если тронуть
    // `guard_key`. До задачи 6 ключ несёл и имя рабочего места — теперь
    // только пользователя ОС, а проверка «разные ключи не видят друг
    // друга» на уровне `try_acquire` этого не замечает вовсе.
    #[cfg(unix)]
    #[test]
    fn two_different_directories_never_collide() {
        let a = existing_dir("dev1");
        let b = existing_dir("dev2");
        let ga = try_acquire(&unique("dev1"), &a).unwrap().expect("первый каталог свободен");
        let gb = try_acquire(&unique("dev2"), &b).unwrap();
        assert!(gb.is_some(), "замок на другом каталоге не должен видеть замок первого");
        drop(ga);
        drop(gb);
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[cfg(windows)]
    #[test]
    fn two_different_keys_never_collide() {
        // Один и тот же каталог, разные ключи: на Windows замок — объект
        // ядра по имени ключа, путь в деле не участвует.
        let dir = existing_dir("two-keys");
        let ga = try_acquire(&unique("dev1"), &dir).unwrap().expect("первый ключ свободен");
        let gb = try_acquire(&unique("dev2"), &dir).unwrap();
        assert!(gb.is_some(), "замок с другим ключом не должен видеть замок первого");
        drop(ga);
        drop(gb);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_lives_in_the_state_not_in_the_kit() {
        // Замок на Linux держится на каталоге состояния (`lock_target` — тот
        // же путь, что `acquire` передаёт в `try_acquire`). Он обязан лежать
        // в состоянии пользователя и никогда внутри общего комплекта (тот —
        // только для чтения и один на всех). Windows эта задача не трогает:
        // там состояние по-прежнему при комплекте.
        if cfg!(windows) {
            return;
        }
        let kit = crate::ops_host::wire_state(Kit::at(std::path::Path::new("/opt/dev1c-kit")));
        let lock_on = lock_target(&kit);
        assert!(!lock_on.starts_with(&kit.root), "{}", lock_on.display());
        assert!(lock_on.starts_with(crate::ops_host::state_dir()), "{}", lock_on.display());
    }
}
