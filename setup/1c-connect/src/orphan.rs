//! Мост уходит вместе с тем, кто его запустил.
//!
//! Замер на машине владельца 31.10: четыре процесса `1c-connect.exe mcp 1c`
//! висели час двадцать, у каждого свой мёртвый родитель-агент и живой
//! ребёнок `mcp-1c-windows-amd64.exe`. Значит, канал к агенту не отдал EOF,
//! когда агент умер, и ветка `bridge::pipe_jsonrpc` `Ok(0) =>
//! EpochEnd::AgentEof` не сработала вовсе. Как именно агент отдаёт нам
//! стандартный ввод — чужая сторона, и рассчитывать на её EOF нельзя.
//!
//! Поэтому у нас свой сторож: закрепляем родителя на старте и уходим, когда
//! ушёл он. Обычный приём для серверов MCP по стандартному вводу.
//!
//! Ребёнка добивать здесь нечем и не нужно: его дерево держит
//! `crate::job::Job` с `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, а выход процесса
//! закрывает описатель задания руками ядра — последний описатель закрылся,
//! задание убило всех своих.
//!
//! Срока бездействия вторым сторожем тут нет нарочно: долгая пауза в работе
//! агента законна, и выход по ней убивал бы живые разговоры.

use common::{Event, Sink};

/// Что делать со сторожем, когда старт уже выяснил, кто нам родитель.
#[derive(Debug, PartialEq)]
pub enum Watch {
    /// Ждать конца этого родителя и уйти за ним.
    Guard,
    /// Жить без сторожа; строка — причина для журнала.
    Alone(&'static str),
}

/// Чистое решение сторожа. `parent_born` — момент запуска закреплённого
/// родителя, `me_born` — наш собственный; единицы любые, лишь бы одни и те же
/// (на Windows — `GetProcessTimes`).
///
/// Родителя не видно — **не выходим**: запуск через промежуточный запускатель,
/// который сам сразу же завершился, — законный случай, и выход в нём ломал бы
/// работу там, где она работала.
///
/// Родитель «моложе» нас — это не наш родитель, а чужой процесс, которому
/// система отдала освободившийся номер; ждать его конца значило бы уйти
/// посреди живого разговора.
pub fn watch_for(parent_born: Option<u64>, me_born: u64) -> Watch {
    match parent_born {
        None => Watch::Alone("родителя не видно на старте"),
        Some(born) if born > me_born => Watch::Alone("номер родителя переиспользован системой"),
        Some(_) => Watch::Guard,
    }
}

fn alone(log: &Sink, why: &str) {
    log(Event::Warn(format!("сторож родителя не заведён: {why} — мост живёт, пока его не закроют")));
}

fn leaving(log: &Sink) {
    log(Event::Ok("родитель моста завершился — ухожу за ним".into()));
}

/// Номер родителя. Список процессов берёт `sysinfo` — он уже в зависимостях и
/// уже читает процессы на каждом такте (`bridge::configurator_count`), своего
/// снимка через WinAPI заводить незачем.
///
/// ponytail: 10–30 мс один раз на старте (замер того же `sysinfo` в
/// `configurator_count`) — наш потолок; станет дорого — `CreateToolhelp32Snapshot`
/// напрямую, фичей того же `windows-sys`.
#[cfg(windows)]
fn parent_pid() -> Option<u32> {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
    let sys = System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.process(Pid::from_u32(std::process::id()))?.parent().map(|p| p.as_u32())
}

/// Момент запуска процесса одним числом (100 нс с 1601 года).
#[cfg(windows)]
fn born(h: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    unsafe {
        let mut c: FILETIME = std::mem::zeroed();
        let (mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
        if windows_sys::Win32::System::Threading::GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 {
            return None;
        }
        Some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    }
}

/// Описатель родителя, который переживает переезд в поток сторожа. Описатель
/// процесса — обычная ссылка на объект ядра, и гонять её между потоками можно.
#[cfg(windows)]
struct Pinned(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for Pinned {}

/// Завести сторожа. Зовётся один раз на старте `ops_host::run_mcp`; при живом
/// родителе не пишет в журнал ни строки и ничего не ждёт.
///
/// Родителя закрепляем **описателем**, а не номером: номер система
/// переиспользует, и сторож ждал бы чужой процесс. Описатель берём сразу за
/// снимком, а что он и правда наш — проверяем моментом запуска (`watch_for`).
#[cfg(windows)]
pub fn guard(log: Sink) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, WaitForSingleObject, INFINITE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    let Some(ppid) = parent_pid() else {
        return alone(&log, "родителя не видно на старте");
    };
    // SAFETY: обычный вызов WinAPI, указателей не передаём; неудача — нулевой
    // описатель, его и проверяем.
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, ppid) };
    if h.is_null() {
        return alone(&log, "родителя не удалось закрепить описателем");
    }
    let pinned = Pinned(h);
    // Свой момент не прочитаться не может; если всё же — считаем себя
    // «старше всех», и единственная проверка, которую это снимает, — на
    // переиспользованный номер.
    let me = born(unsafe { GetCurrentProcess() }).unwrap_or(u64::MAX);
    match watch_for(born(h), me) {
        Watch::Alone(why) => {
            // SAFETY: описатель наш, только что открытый, больше не нужен.
            unsafe { CloseHandle(h) };
            alone(&log, why);
        }
        Watch::Guard => {
            std::thread::spawn(move || {
                let p = pinned;
                // SAFETY: описатель открыт с PROCESS_SYNCHRONIZE и жив всё
                // ожидание — он переехал сюда вместе с `Pinned`.
                unsafe { WaitForSingleObject(p.0, INFINITE) };
                leaving(&log);
                // Обычный выход, не паника: журнал уже дописан, а ребёнка
                // уносит закрытие описателя задания (см. `crate::job`).
                std::process::exit(0);
            });
        }
    }
}

/// То же смыслом, средствами системы: `getppid` не врёт о переиспользованном
/// номере (ядро отвечает за того, кто нам родитель прямо сейчас), поэтому
/// закреплять описателем нечего и моменты запуска не нужны — оба нулевые.
///
/// `PR_SET_PDEATHSIG` не взят нарочно: он убил бы нас сигналом молча, без
/// строки в журнале, а опрос раз в секунду не стоит ничего.
#[cfg(not(windows))]
pub fn guard(log: Sink) {
    // SAFETY: getppid не принимает указателей и не может завершиться неудачей.
    let ppid = unsafe { libc::getppid() };
    // 1 — нас уже усыновил init: родителя на старте нет.
    match watch_for(if ppid > 1 { Some(0) } else { None }, 0) {
        Watch::Alone(why) => alone(&log, why),
        Watch::Guard => {
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                // SAFETY: см. выше.
                if unsafe { libc::getppid() } != ppid {
                    leaving(&log);
                    std::process::exit(0);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_parent_means_we_leave_with_it() {
        assert_eq!(watch_for(Some(100), 200), Watch::Guard);
    }

    #[test]
    fn no_parent_at_start_means_we_live_on() {
        assert!(matches!(watch_for(None, 200), Watch::Alone(_)));
    }

    /// Родитель, запущенный позже нас, — чужой процесс с нашим бывшим номером
    /// родителя: сторожить его нельзя, иначе уйдём посреди живого разговора.
    #[test]
    fn a_parent_younger_than_us_is_someone_elses_reused_number() {
        assert!(matches!(watch_for(Some(300), 200), Watch::Alone(_)));
    }
}
