//! Мост уходит вместе с тем, кого он обслуживает, и с тем, кто его запустил.
//!
//! Хозяин жизни моста — КОННЕКТОР (решение владельца 31.10): мост живёт
//! ровно столько, сколько живёт окно (или `connect`/`serve`), которое его
//! обслуживает. Замер того же дня объяснил, почему родителя для этого не
//! хватает: восемь мостов, у каждого свой родитель, и все восемь родителей
//! мертвы уже на старте — Zed запускает мост через промежуточный процесс,
//! который тут же завершается. Правило «родителя не видно — живём» (ниже,
//! `watch_for`) оставляет такой мост жить вечно, и закрывает этот случай
//! только сторож по коннектору (`guard_owner`).
//!
//! Чем коннектор закреплён: не номером процесса и не отметкой на диске, а
//! тем самым замком рабочего места, который он и так держит объектом ядра
//! (`single_instance`: именованный мьютекс на Windows, `flock` на каталоге
//! состояния на Linux). Номера система переиспользует, отметка переживает
//! падение — а объект ядра система снимает вместе с процессом, даже убитым
//! `taskkill /F`, и переиспользовать его нечем. Поэтому своей отметки с
//! номером процесса здесь нет: закрепление описателем, которого требовало
//! решение, у этого замка есть по устройству, и читает его тот же
//! `single_instance::is_held`, которым живость коннектора уже проверяет
//! `allow::read`.
//!
//! Сторож по родителю остался вторым рубежом, а не уступил место: он
//! закрывает случай, которого сторож по коннектору не видит вовсе —
//! редактор умер, а коннектор жив; оба подают один и тот же сигнал
//! (`GONE`), так что второй механизм ухода отсюда не берётся.
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
//! Уходим не сразу силой: сторож сначала подаёт сигнал
//! (`launcher_gone` — арм `select!` в `bridge::pipe_jsonrpc`), и соединение
//! кончается своей обычной дорогой — `EpochEnd::AgentEof`, `grace_kill`,
//! `drop(job)`. Это важно именно на Linux: там дерево ребёнка убивает
//! `Job::drop` (`killpg`), а `std::process::exit` деструкторов не выполняет —
//! уйди мы силой, внуки (`oscript` → `1cv8`) остались бы жить. На Windows
//! ребёнка уносит ядро: оно закрывает описатель задания с
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
//!
//! Сила остаётся запасной дорогой, не основной: если уборка не кончилась за
//! `CLEANUP`, выходим сами. Иначе «сигнал некому слушать» (мост стоит не на
//! `select!`, а внутри чужого вызова) вернул бы нам ровно ту беду, против
//! которой весь круг, — процесс навсегда.
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

/// Сколько ждём обычную уборку, прежде чем уйти силой. Щедро нарочно:
/// `grace_kill` даёт ребёнку свои 5 с, а ждать этот выход уже некому — тот,
/// кто нас запустил, мёртв.
const CLEANUP: std::time::Duration = std::time::Duration::from_secs(15);

/// Процесс-глобальный сигнал «тот, кто нас запустил, ушёл». Глобальный, а не
/// параметр: это факт о процессе целиком, и тащить его через `Serve::shared`,
/// `dispatch` и `Rules` до цикла моста значило бы менять четыре подписи ради
/// одного бита.
///
/// `notify_one`, а не `notify_waiters`: разрешение сохраняется и достаётся
/// тому, кто начнёт слушать ПОСЛЕ сигнала. С `notify_waiters` сигнал,
/// поданный, пока мост занят вызовом, потерялся бы совсем (тест ниже).
static GONE: tokio::sync::Notify = tokio::sync::Notify::const_new();

fn notify_gone() {
    GONE.notify_one();
}

/// Арм для `select!` цикла моста: разрешается, когда ушёл тот, кто запустил
/// процесс. В режимах без сторожа (`serve`, туннель) не разрешается никогда.
pub async fn launcher_gone() {
    GONE.notified().await;
}

/// Не возвращается: либо процесс уйдёт сам, обычной уборкой, либо мы уведём
/// его силой по сроку. `why` — что именно ушло, для журнала: сторожа два
/// (родитель и коннектор), а дорога ухода у них одна.
fn leaving(log: &Sink, why: &str) -> ! {
    log(Event::Ok(format!("{why} — ухожу за ним")));
    notify_gone();
    std::thread::sleep(CLEANUP);
    log(Event::Warn(format!("уборка не кончилась за {} с после ухода — выхожу силой", CLEANUP.as_secs())));
    std::process::exit(0);
}

/// Что сторож по коннектору видит на одном такте.
#[derive(Debug, PartialEq)]
pub enum Owner {
    /// Коннектор на месте — живём.
    Alive,
    /// Коннектора ещё не было ни разу за всю жизнь моста.
    NotYet,
    /// Коннектор был и ушёл — уходим за ним.
    Gone,
}

/// Чистое решение сторожа по коннектору. `seen` — видели ли мы коннектор
/// живым хоть раз, `alive` — виден ли он сейчас.
///
/// Коннектора не было ни разу — **не выходим**, хотя решение владельца и
/// предлагало выйти («работать всё равно не на кого»). Законный случай
/// нашёлся в самом коде: редактор держит одно соединение весь сеанс,
/// спрашивает `tools/list` один раз и не обрабатывает
/// `notifications/tools/list_changed` (см. заголовок `ops_host::run_mcp`).
/// Редактор, запущенный раньше коннектора, — обычное дело; выйди мост на
/// старте, Zed показал бы упавший сервер и сам его не поднял, и человеку
/// пришлось бы перезапускать редактор после каждого запуска коннектора.
/// Висеть при этом нечему: при выключенных MCP мост отпускает ребёнка
/// (`bridge::EpochEnd::Dormant`), то есть ждёт он заглушкой, без единого
/// процесса 1С за собой.
///
/// Коннектор, который был и ушёл, — ровно тот случай, которого требовал
/// владелец: «закрыли коннектор — и всё гарантированно ушло».
pub fn owner_now(seen: bool, alive: bool) -> Owner {
    match (seen, alive) {
        (_, true) => Owner::Alive,
        (true, false) => Owner::Gone,
        (false, false) => Owner::NotYet,
    }
}

/// Держит ли кто-нибудь замок рабочего места прямо сейчас — то есть виден
/// ли коннектор, который этот мост обслуживает. Ключ строится той же
/// `guard_key`, которой его строит тот, кто замок берёт
/// (`single_instance::acquire`): разойдись они, мост уходил бы при живом
/// коннекторе на первом же такте.
fn owner_alive(kit: &common::kit::Kit, user: &str, workstation: &str) -> std::io::Result<bool> {
    let key = crate::single_instance::guard_key(user, workstation);
    crate::single_instance::is_held(&key, &crate::single_instance::lock_target(kit))
}

/// Как часто спрашиваем замок. Секунда, как у опроса родителя на Linux:
/// коннектор закрывают руками, и разницы между «сразу» и «через секунду»
/// человек не заметит, а дешевле этого опроса ничего нет.
const OWNER_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// Завести сторожа по коннектору. Зовётся один раз на старте
/// `ops_host::run_mcp`, рядом с `guard`.
///
/// Имя рабочего места берётся не параметром, замороженным на старте, а из
/// общей ячейки `live`, которую держит свежей тот же `watch_mcp`: человек
/// вправе переименовать рабочее место и сохранить, и сторож обязан спросить
/// замок уже под новым именем (та же находка, что Important 2 у `watch_mcp`).
pub fn guard_owner(kit: common::kit::Kit, user: String, live: crate::session::Live, log: Sink) {
    std::thread::spawn(move || {
        let mut seen = false;
        let mut said = false;
        loop {
            std::thread::sleep(OWNER_TICK);
            let (_, cfg) = live.get();
            // Замок не прочитался (права, каталог состояния не готов) — это
            // не «коннектора нет»: решение здесь — уходить или нет, и
            // неизвестность обязана значить «живём». Такт пропускаем целиком,
            // чтобы она не записалась и в «коннектор уже видели».
            let Ok(alive) = owner_alive(&kit, &user, &cfg.host.workstation) else { continue };
            match owner_now(seen, alive) {
                Owner::Alive => seen = true,
                Owner::Gone => leaving(&log, "коннектор рабочего места закрылся"),
                Owner::NotYet => {
                    if !said {
                        said = true;
                        log(Event::Warn(
                            "коннектор рабочего места не запущен — отвечаю отказом и жду его; уйду вместе с ним, когда он появится и закроется".into(),
                        ));
                    }
                }
            }
        }
    });
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
                leaving(&log, "родитель моста завершился");
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
                    leaving(&log, "родитель моста завершился");
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

    /// Сигнал подаёт поток сторожа, а слушает цикл моста — и слушать он
    /// может начать уже ПОСЛЕ сигнала (пока сторож его подавал, мост стоял
    /// внутри чужого вызова, а не на `select!`). Потерянный здесь сигнал
    /// означал бы процесс, живущий вечно, — ту самую беду круга.
    #[tokio::test]
    async fn the_signal_waits_for_the_one_who_starts_listening_later() {
        notify_gone();
        tokio::time::timeout(std::time::Duration::from_millis(500), launcher_gone())
            .await
            .expect("сигнал ухода родителя потерян: ждать начали позже, чем его подали");
    }

    /// Родитель, запущенный позже нас, — чужой процесс с нашим бывшим номером
    /// родителя: сторожить его нельзя, иначе уйдём посреди живого разговора.
    #[test]
    fn a_parent_younger_than_us_is_someone_elses_reused_number() {
        assert!(matches!(watch_for(Some(300), 200), Watch::Alone(_)));
    }

    /// Все четыре клетки решения сторожа по коннектору. `(seen, alive)`:
    /// коннектор виден сейчас — живём всегда, независимо от того, видели ли
    /// мы его раньше; не видели никогда — заглушка, а не выход (редактор,
    /// запущенный раньше коннектора, законен); видели и ушёл — уходим за
    /// ним, ровно то, что требовал владелец.
    #[test]
    fn owner_now_covers_all_four_cells() {
        assert_eq!(owner_now(false, true), Owner::Alive);
        assert_eq!(owner_now(true, true), Owner::Alive);
        assert_eq!(owner_now(false, false), Owner::NotYet);
        assert_eq!(owner_now(true, false), Owner::Gone);
    }

    /// Каждой проверке — свежее рабочее место: `guard_key` на Windows — это
    /// имя мьютекса, общее на сессию, а не на каталог состояния, и две такие
    /// проверки под одним пользователем и одним именем столкнулись бы, пока
    /// тесты пакета идут параллельно (замок берёт ещё и `single_instance.rs`,
    /// и `ops_host.rs` своими тестами).
    fn unique_workstation() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!("orphan-owner-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
    }

    /// `owner_alive` обязана спрашивать ТОТ ЖЕ замок, что держит коннектор:
    /// ту же `guard_key`/`lock_target`, что строит `single_instance::acquire`
    /// (решение владельца 31.10 — закрепление коннектора описателем замка, а
    /// не отдельной отметкой). Разойдись они хоть в одном параметре, сторож
    /// либо не увидел бы живой коннектор (ушёл бы посреди работы), либо не
    /// заметил бы его уход (повис бы вечно) — ровно та беда, ради которой
    /// заводился весь круг.
    #[test]
    fn owner_alive_sees_the_same_lock_single_instance_acquire_takes() {
        let dir = crate::project::tempdir::Dir::new();
        let kit = common::kit::Kit::at(dir.path());
        let user = crate::single_instance::current_user();
        let workstation = unique_workstation();
        assert!(!owner_alive(&kit, &user, &workstation).unwrap(), "замок свободен — коннектора ещё нет");
        let guard = crate::single_instance::acquire(&kit, &workstation, "test").expect("замок свободен в начале теста");
        assert!(owner_alive(&kit, &user, &workstation).unwrap(), "коннектор держит замок — сторож обязан его увидеть");
        drop(guard);
        assert!(!owner_alive(&kit, &user, &workstation).unwrap(), "коннектор ушёл — замок снят вместе с ним");
    }
}
