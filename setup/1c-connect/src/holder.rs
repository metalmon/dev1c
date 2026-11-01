//! Кто держит порт рабочего места, если не этот процесс: имя и pid, чтобы и
//! текст занятого порта в `listen.rs`, и факт «порты рабочего места» в
//! `verify.rs` могли назвать держателя вместо голого «занято». На Windows —
//! `netstat -ano` плюс `tasklist` за именем (тот же приём, что уже применяет
//! `bridge::configurator_count`). В контейнере нет ни `ss`, ни `netstat` —
//! там это ядро само: `/proc/net/tcp`(6) даёт inode слушающего сокета,
//! `/proc/<pid>/fd/*` отображает его на процесс, `/proc/<pid>/comm` — имя.
//! `ss`, если он всё же на машине есть, остаётся резервом после `/proc`.
//!
//! ponytail: при нескольких процессах, держащих один inode (fork без exec),
//! называем первый найденный — различать их незачем, порт всё равно один.

use std::process::Command;

#[cfg(windows)]
fn no_window(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn no_window(_c: &mut Command) {}

/// Pid процесса, слушающего `port` — системным вызовом/ядром, БЕЗ внешнего
/// процесса: эту пробу зовут часто (перед каждой отправкой пароля базы,
/// `publication::refuse_unless_ours`), а `of_port`/`others_of_port` выше
/// заводят `netstat`/`tasklist` на каждый вызов — приемлемо для редкого
/// диагностического текста, неприемлемо здесь (решение владельца, план
/// «тонкий срез ценности», задача 1, круг исправления).
///
/// Windows — `GetExtendedTcpTable` напрямую (тот же API, которым живёт
/// `netstat`, но без порождения процесса). Unix — то же ядро, что уже
/// использует `of_port_proc`: `/proc/net/tcp`(6) даёт inode, `/proc/<pid>/fd`
/// отображает его на процесс; второй копии этого пути здесь нет.
///
/// Несколько слушателей одного порта (разные семейства адресов) — здесь не
/// различаются, берётся первый найденный: эта проба — не диагностика
/// человеку (там это различие важно, `others_of_port`), а охрана перед
/// отправкой пароля, и достаточно знать ОДНОГО держателя, чтобы решить,
/// наш он или нет.
#[cfg(windows)]
pub fn listener_pid(port: u16) -> Option<u32> {
    use windows_sys::Win32::Foundation::NO_ERROR;
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER};
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let mut size: u32 = 0;
    // Первый вызов — только узнать нужный размер буфера (так документирован
    // GetExtendedTcpTable); код возврата здесь не смотрим, он и должен быть
    // "буфер мал" при нулевом буфере.
    unsafe {
        GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, AF_INET as u32, TCP_TABLE_OWNER_PID_LISTENER, 0);
    }
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let rc = unsafe { GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut size, 0, AF_INET as u32, TCP_TABLE_OWNER_PID_LISTENER, 0) };
    if rc != NO_ERROR {
        return None;
    }
    // `MIB_TCPTABLE_OWNER_PID`: dwNumEntries, затем массив строк
    // переменной длины — читаем как заголовок плюс срез по месту, не
    // копируя буфер.
    let header = buf.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
    let n = unsafe { (*header).dwNumEntries } as usize;
    let rows_ptr = unsafe { (*header).table.as_ptr() };
    let rows = unsafe { std::slice::from_raw_parts(rows_ptr, n) };
    // `dwLocalPort` несёт порт в сетевом порядке байт в младших двух байтах.
    let want = port.to_be();
    rows.iter().find(|r| (r.dwLocalPort as u16) == want).map(|r| r.dwOwningPid)
}

#[cfg(not(windows))]
pub fn listener_pid(port: u16) -> Option<u32> {
    let tcp = std::fs::read_to_string("/proc/net/tcp").unwrap_or_default();
    let tcp6 = std::fs::read_to_string("/proc/net/tcp6").unwrap_or_default();
    let inode = parse_proc_net_tcp(&tcp, port).or_else(|| parse_proc_net_tcp(&tcp6, port))?;
    pid_owning_inode(inode)
}

/// Путь к образу процесса по его номеру — ЛИЧНОСТЬ процесса, которую, в
/// отличие от номера, сосед подделать не может.
///
/// Зачем (находка автоматической проверки безопасности по кругу 4,
/// 2.11.2026). Номер процесса Windows переиспользует охотно, а pid-файл
/// нашего httpd после краха (или после того, как его сняли принудительно —
/// а снимают регулярно: `httpd -k stop` на консольном экземпляре ищет службу
/// `Apache2.4` и не останавливает ничего, замер ведущего) остаётся лежать с
/// номером мёртвого процесса. Сосед по терминальному серверу, плодя
/// процессы, однажды получит тот самый номер и займёт наш порт — и страж,
/// сравнивающий ТОЛЬКО номера, скажет «свой», а `MCP_1C_PASSWORD` уедет к
/// соседу. Образ это закрывает: оказаться нашим httpd по нашему пути сосед
/// не может.
///
/// Потребителей двое, и примитив живёт здесь ради второго. Первый — страж
/// порта публикации (`publication::refuse_unless_ours` и проба подбора
/// `publication::busy_for_choice`). Второй — задача 7 плана, где процесс по
/// pid-файлу надо будет ГАСИТЬ: там цена ошибки выше, чем утечка пароля
/// словами, — убить чужую программу вместо своего httpd.
///
/// Windows — `QueryFullProcessImageNameW` под
/// `PROCESS_QUERY_LIMITED_INFORMATION`: самое слабое из прав на процесс.
/// Хватает ли его обычному пользователю — ИЗМЕРЕНО, не предположено
/// (2.11.2026, эта машина, прогон под НЕповышенным токеном: `whoami /groups`
/// без High Mandatory Level): образ трёх процессов `svchost.exe`,
/// принадлежащих SYSTEM, добылся — `C:\Windows\System32\svchost.exe` для
/// pid 1148, 1236, 1364. `None` вернул только pid 4 (ядровой `System`,
/// защищённый) — нашим httpd он не бывает. То есть права достаточно и на
/// чужого пользователя, а значит «не опознали» в бою означает «процесса
/// нет», а не «не дали посмотреть».
///
/// Unix — `/proc/<pid>/exe`, то же ядро, что уже читает `listener_pid`
/// (ссылку на чужой процесс другого пользователя там как раз может не дать
/// прочитать — и это тоже «не опознали»).
///
/// `None` — процесса нет, права не хватило или путь не добыть; решать, что
/// это значит, остаётся вызывающему, и для стража это «не свой».
#[cfg(windows)]
pub fn image_of_pid(pid: u32) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};

    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    // С запасом: расширенный путь бывает длиннее MAX_PATH, но не бесконечно.
    let mut buf = vec![0u16; 4096];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) };
    unsafe {
        CloseHandle(h);
    }
    if ok == 0 || len == 0 {
        return None;
    }
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

#[cfg(not(windows))]
pub fn image_of_pid(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// Один ли это образ — чистое правило, без всякого похода в систему (тем же
/// приёмом, что `ops_host::env_missing_of`/`ensure_state_decision`: решение
/// отдельно от добычи сведений, чтобы проверялось без них).
///
/// Сравнение не байтовое. `canonicalize` разворачивает короткие имена 8.3,
/// `..` и символические ссылки — без этого один и тот же файл, названный
/// двумя путями, читался бы как два разных процесса. На Windows поверх того
/// регистр не значит ничего: `C:\Kit\httpd.exe` и `c:\kit\HTTPD.EXE` — один
/// файл. Приведение юникодное (`to_lowercase`), не ASCII-овское: комплект
/// может лежать в каталоге с русским именем.
pub fn same_image(expected: &std::path::Path, actual: &std::path::Path) -> bool {
    fn normal(p: &std::path::Path) -> String {
        let full = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let s = full.to_string_lossy().to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    }
    normal(expected) == normal(actual)
}

/// Имя и pid процесса, слушающего `port` на loopback (или на всех адресах —
/// самый частый случай: Windows отдаёт занятые порты как `0.0.0.0:N`), в виде
/// `1c-connect (pid 812)` — `None`, если порт свободен, или сведений не
/// добыть.
pub fn of_port(port: u16) -> Option<String> {
    if cfg!(windows) {
        let mut c = Command::new("netstat");
        c.args(["-ano", "-p", "TCP"]);
        no_window(&mut c);
        let out = c.output().ok()?;
        let pid = parse_netstat(&String::from_utf8_lossy(&out.stdout), port)?;
        let mut t = Command::new("tasklist");
        t.args(["/NH", "/FO", "CSV", "/FI", &format!("PID eq {pid}")]);
        no_window(&mut t);
        let tout = t.output().ok()?;
        let name = parse_tasklist_name(&String::from_utf8_lossy(&tout.stdout))?;
        Some(format!("{name} (pid {pid})"))
    } else {
        of_port_proc(port).or_else(|| {
            let out = Command::new("ss").args(["-ltnp"]).output().ok()?;
            parse_ss(&String::from_utf8_lossy(&out.stdout), port)
        })
    }
}

/// Путь без внешних команд: ядро уже знает это через `/proc`. Образ среды не
/// содержит ни `ss`, ни `netstat` — это единственный путь, который там
/// работает.
fn of_port_proc(port: u16) -> Option<String> {
    let tcp = std::fs::read_to_string("/proc/net/tcp").unwrap_or_default();
    let tcp6 = std::fs::read_to_string("/proc/net/tcp6").unwrap_or_default();
    let inode = parse_proc_net_tcp(&tcp, port).or_else(|| parse_proc_net_tcp(&tcp6, port))?;
    let pid = pid_owning_inode(inode)?;
    let name = proc_comm(pid).unwrap_or_else(|| "?".to_string());
    Some(format!("{name} (pid {pid})"))
}

/// Строка `/proc/net/tcp`(6) после разбора по пробелам: `sl local_address
/// rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid timeout inode
/// ...`. Адрес — hex, порт — после `:`; `st == "0A"` — слушает. Отдаёт inode
/// сокета, или `None`, если порт не слушается этой таблицей, строка не того
/// вида (в т.ч. заголовок), или вход пуст.
pub fn parse_proc_net_tcp(out: &str, port: u16) -> Option<u64> {
    out.lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        // Заголовок начинается с "sl", не с "N:" — этим и отличается от строк данных.
        if f.len() < 10 || !f[0].ends_with(':') || f[3] != "0A" {
            return None;
        }
        let (_, hexport) = f[1].split_once(':')?;
        let p = u16::from_str_radix(hexport, 16).ok()?;
        if p != port {
            return None;
        }
        f[9].parse::<u64>().ok()
    })
}

/// Все inode слушателей этого порта в одной таблице `/proc/net/tcp`(6), а не
/// первый попавшийся: один порт слушают и по IPv4, и по IPv6 РАЗНЫЕ процессы
/// (живой случай 26.10 на стенде — наш коннектор на `127.0.0.1`, проброс SSH
/// соседа на `::1`), и «первый» скрывал чужого за нами же.
pub fn parse_proc_net_tcp_all(out: &str, port: u16) -> Vec<u64> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 10 || !f[0].ends_with(':') || f[3] != "0A" {
                return None;
            }
            let (_, hexport) = f[1].split_once(':')?;
            if u16::from_str_radix(hexport, 16).ok()? != port {
                return None;
            }
            f[9].parse::<u64>().ok()
        })
        .collect()
}

/// Все pid, слушающие порт по данным `netstat -ano`, обе семьи адресов.
pub fn parse_netstat_all(out: &str, port: u16) -> Vec<u32> {
    let port_s = port.to_string();
    let mut pids: Vec<u32> = Vec::new();
    for l in out.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 5 || f[0] != "TCP" || f[3] != "LISTENING" {
            continue;
        }
        let Some((_, p)) = f[1].rsplit_once(':') else { continue };
        if p != port_s {
            continue;
        }
        let Ok(pid) = f[4].parse::<u32>() else { continue };
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

/// Держатели порта, КРОМЕ нас самих (`me`) — по одному описанию на процесс.
///
/// Зачем не `of_port`: тот отдаёт первого, и когда первым оказываемся мы
/// (наш слушатель на `127.0.0.1`), чужой на `::1` исчезает из виду вовсе.
/// Так на стенде 26.10 два рабочих места с одним именем `dev1` — коннектор
/// в контейнере и проброс SSH с машины человека — разошлись по семействам
/// адресов и оба показали зелёное: `sshd` сдаётся только если не смог НИ
/// ОДНО семейство, а `::1` был свободен. К какой базе попадёт агент, решал
/// после этого порядок в резолвере, а не настройка.
pub fn others_of_port(port: u16, me: u32) -> Vec<String> {
    let pids: Vec<u32> = if cfg!(windows) {
        let mut c = Command::new("netstat");
        c.args(["-ano", "-p", "TCP"]);
        no_window(&mut c);
        match c.output() {
            Ok(out) => parse_netstat_all(&String::from_utf8_lossy(&out.stdout), port),
            Err(_) => Vec::new(),
        }
    } else {
        let tcp = std::fs::read_to_string("/proc/net/tcp").unwrap_or_default();
        let tcp6 = std::fs::read_to_string("/proc/net/tcp6").unwrap_or_default();
        let mut inodes = parse_proc_net_tcp_all(&tcp, port);
        inodes.extend(parse_proc_net_tcp_all(&tcp6, port));
        let mut pids: Vec<u32> = inodes.into_iter().filter_map(pid_owning_inode).collect();
        pids.dedup();
        pids
    };
    pids.into_iter()
        .filter(|pid| *pid != me)
        .map(|pid| {
            let name = name_of_pid(pid);
            format!("{} (pid {pid})", name.unwrap_or_else(|| "?".to_string()))
        })
        .collect()
}

/// Имя процесса по pid: на Windows — через `tasklist` с фильтром по pid, на
/// остальных — `/proc/<pid>/comm`. Своей таблицы имён не держим: оба пути уже
/// есть, здесь только выбор по системе.
fn name_of_pid(pid: u32) -> Option<String> {
    if cfg!(windows) {
        let mut t = Command::new("tasklist");
        t.args(["/NH", "/FO", "CSV", "/FI", &format!("PID eq {pid}")]);
        no_window(&mut t);
        let out = t.output().ok()?;
        parse_tasklist_name(&String::from_utf8_lossy(&out.stdout))
    } else {
        proc_comm(pid)
    }
}

/// Какой pid держит сокет с этим inode: сканирует `/proc/<pid>/fd/*` в
/// поисках символьной ссылки `socket:[<inode>]`. Процесс мог исчезнуть между
/// `read_dir` и чтением, или быть недоступен для чтения (чужой) — такие
/// просто пропускаются, не обрывая поиск по остальным.
fn pid_owning_inode(inode: u64) -> Option<u32> {
    let target = format!("socket:[{inode}]");
    let Ok(procfs) = std::fs::read_dir("/proc") else { return None };
    for entry in procfs.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else { continue };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).is_ok_and(|l| l.to_string_lossy() == target.as_str()) {
                return Some(pid);
            }
        }
    }
    None
}

/// Имя процесса по pid — `/proc/<pid>/comm` уже без перевода строки.
fn proc_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string())
}

/// Строка `ss -ltnp` вида `LISTEN 0 4096 127.0.0.1:7771 0.0.0.0:*
/// users:(("1c-connect",pid=812,fd=9))` — имя и pid, или `None`, если порт не
/// слушается или строка не того вида, без паники. Резерв на случай, если на
/// машине всё же есть `ss`; в образе среды его нет — там работает
/// `of_port_proc`.
pub fn parse_ss(out: &str, port: u16) -> Option<String> {
    let local = format!("127.0.0.1:{port}");
    let line = out.lines().find(|l| l.split_whitespace().any(|f| f == local))?;
    let users = line.split("users:((").nth(1)?;
    let name = users.split('"').nth(1)?;
    let pid = users.split("pid=").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?;
    if name.is_empty() || pid.is_empty() {
        return None;
    }
    Some(format!("{name} (pid {pid})"))
}

/// Строка `netstat -ano` вида `TCP 0.0.0.0:7771 0.0.0.0:0 LISTENING 812` —
/// pid, или `None`, если порт нигде не слушается. Адрес может быть любым
/// (`127.0.0.1`, `0.0.0.0`, `*`, `[::1]`, `[::]`) — это норма на Windows, где
/// большинство служб слушает `0.0.0.0`; порт сравнивается по хвосту после
/// последнего `:`, чтобы `17771` не совпал с `7771`. Несколько строк с одним
/// портом (IPv4 и IPv6 слушают раздельно) — loopback (`127.0.0.1`/`[::1]`)
/// предпочтён прочим адресам.
pub fn parse_netstat(out: &str, port: u16) -> Option<u32> {
    let port_s = port.to_string();
    let mut any: Option<u32> = None;
    for l in out.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 5 || f[0] != "TCP" || f[3] != "LISTENING" {
            continue;
        }
        let Some((addr, p)) = f[1].rsplit_once(':') else { continue };
        if p != port_s {
            continue;
        }
        let Ok(pid) = f[4].parse::<u32>() else { continue };
        if matches!(addr, "127.0.0.1" | "[::1]") {
            return Some(pid);
        }
        any.get_or_insert(pid);
    }
    any
}

/// Имя процесса из первой колонки `tasklist /FO CSV`, уже отфильтрованного по
/// pid — строка вида `"1c-connect.exe","812","Console","1","12 345 K"`.
fn parse_tasklist_name(csv: &str) -> Option<String> {
    let line = csv.lines().next()?;
    if !line.starts_with('"') {
        // "INFO: No tasks are running..." — pid уже не живёт, не CSV.
        return None;
    }
    let name = line.split(',').next()?.trim_matches('"');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Настоящий слушатель в этом же процессе: `listener_pid` обязан назвать
    /// pid самого тестового процесса — без `netstat`/`tasklist`, прямым ядром
    /// или системным вызовом. Красное было до самой функции: `listener_pid`
    /// не существовал.
    #[test]
    fn listener_pid_names_the_process_holding_a_real_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(listener_pid(port), Some(std::process::id()), "слушатель в этом же процессе — pid обязан совпасть");
        drop(listener);
    }

    /// Закрытый порт — `None`, не угаданный чужой pid.
    #[test]
    fn listener_pid_is_none_on_a_closed_port() {
        assert_eq!(listener_pid(1), None);
    }

    const SS_LINE: &str = "LISTEN 0 4096 127.0.0.1:7771 0.0.0.0:* users:((\"1c-connect\",pid=812,fd=9))";

    // Образ `/proc/net/tcp`: заголовок, затем loopback:7779 (0x1E63)
    // слушающий (st 0A, inode 12345), и 0.0.0.0:80 (0x0050) тоже слушающий
    // (inode 22222) — адрес не разбирается, только порт и состояние.
    const PROC_TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
   0: 0100007F:1E63 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0
   1: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 22222 1 0000000000000000 100 0 0 10 0
";
    /// Тот же порт 7779 в таблице IPv6 — другой процесс, другой inode.
    /// Живой случай 26.10: наш коннектор на `127.0.0.1`, проброс SSH соседа
    /// на `::1`.
    const PROC_TCP6: &str = "  sl  local_address                         rem_address                        st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
   0: 00000000000000000000000001000000:1E63 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 54321 1 0000000000000000 100 0 0 10 0
";

    /// Один порт слушают ДВА процесса — по одному на семейство адресов, — и
    /// оба обязаны попасть в список. Пока брали первого (`of_port`), чужой
    /// прятался за нами же: на стенде два рабочих места с именем `dev1`
    /// разошлись по IPv4 и IPv6 и оба показали зелёное, а к какой базе
    /// попадёт агент, решал порядок в резолвере.
    #[test]
    fn both_address_families_are_counted_not_just_the_first() {
        assert_eq!(parse_proc_net_tcp_all(PROC_TCP, 7779), vec![12345]);
        assert_eq!(parse_proc_net_tcp_all(PROC_TCP6, 7779), vec![54321]);
        // Вместе — два держателя, а не один.
        let mut both = parse_proc_net_tcp_all(PROC_TCP, 7779);
        both.extend(parse_proc_net_tcp_all(PROC_TCP6, 7779));
        assert_eq!(both, vec![12345, 54321], "чужой слушатель на другом семействе не должен теряться");
        // Не LISTEN по-прежнему не считается ни в одной таблице.
        assert!(parse_proc_net_tcp_all(PROC_TCP_NOT_LISTENING, 7779).is_empty());
    }

    /// То же правило на Windows: `netstat -ano` печатает обе семьи
    /// отдельными строками, и обе идут в список.
    #[test]
    fn netstat_lists_every_holder_of_the_port() {
        let out = "  TCP    127.0.0.1:7771         0.0.0.0:0              LISTENING       812
  TCP    [::1]:7771             [::]:0                 LISTENING       4242
  TCP    127.0.0.1:7772         0.0.0.0:0              LISTENING       999
";
        assert_eq!(parse_netstat_all(out, 7771), vec![812, 4242]);
        assert_eq!(parse_netstat_all(out, 7772), vec![999]);
    }

    // Тот же порт 7779, но ESTABLISHED (st 06), не LISTEN — не должен считаться.
    const PROC_TCP_NOT_LISTENING: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
   0: 0100007F:1E63 00000000:0000 06 00000000:00000000 00:00000000 00000000     0        0 99999 1 0000000000000000 100 0 0 10 0
";

    // Образ `netstat -ano`: 7771 занят дважды (0.0.0.0 и 127.0.0.1 — loopback
    // предпочтён), 7781 — только IPv6 (`[::]` и `[::1]` — тоже loopback
    // предпочтён), 17771 — соседний порт, не должен путаться с 7771.
    const NETSTAT_SAMPLE: &str = "\
TCP    0.0.0.0:7771           0.0.0.0:0              LISTENING       500
TCP    127.0.0.1:7771         0.0.0.0:0              LISTENING       812
TCP    [::]:7781              [::]:0                 LISTENING       600
TCP    [::1]:7781             [::1]:0                LISTENING       999
TCP    127.0.0.1:17771        0.0.0.0:0              LISTENING       123
";

    #[test]
    fn parse_ss_names_the_holder_and_misses_other_ports() {
        assert_eq!(parse_ss(SS_LINE, 7771), Some("1c-connect (pid 812)".to_string()));
        assert_eq!(parse_ss(SS_LINE, 7772), None);
        assert_eq!(parse_ss("", 7771), None);
        assert_eq!(parse_ss("junk\nmore junk", 7771), None);
    }

    #[test]
    fn parse_proc_net_tcp_reads_the_hex_port_and_inode() {
        assert_eq!(parse_proc_net_tcp(PROC_TCP, 7779), Some(12345));
        assert_eq!(parse_proc_net_tcp(PROC_TCP, 80), Some(22222));
        assert_eq!(parse_proc_net_tcp(PROC_TCP, 81), None);
        assert_eq!(parse_proc_net_tcp("", 7779), None);
    }

    #[test]
    fn parse_proc_net_tcp_ignores_a_non_listening_socket() {
        assert_eq!(parse_proc_net_tcp(PROC_TCP_NOT_LISTENING, 7779), None);
    }

    #[test]
    fn parse_netstat_matches_any_local_address_and_prefers_loopback() {
        assert_eq!(parse_netstat(NETSTAT_SAMPLE, 7771), Some(812), "0.0.0.0 занят тоже, но loopback 812 предпочтён 500");
        assert_eq!(parse_netstat(NETSTAT_SAMPLE, 7781), Some(999), "[::1] предпочтён [::]");
        assert_eq!(parse_netstat(NETSTAT_SAMPLE, 17771), Some(123), "17771 не должен спутаться с 7771");
        assert_eq!(parse_netstat(NETSTAT_SAMPLE, 7772), None);
        assert_eq!(parse_netstat("", 7771), None);
    }

    /// Примитив личности процесса работает на обеих платформах — это нижняя
    /// проверка, без которой `same_image` сравнивал бы `None` с `None`:
    /// образ СВОЕГО процесса обязан добываться и совпадать с
    /// `current_exe()`, а номера, которого нет, — не паника, а `None`.
    #[test]
    fn image_of_pid_names_our_own_image_and_refuses_a_number_that_is_not_there() {
        let image = image_of_pid(std::process::id()).expect("образ своего процесса обязан добываться");
        assert!(same_image(&std::env::current_exe().unwrap(), &image), "{}", image.display());
        assert_eq!(image_of_pid(4_294_967_290), None, "номера нет — None, а не паника и не чужой образ");
    }

    /// Правило сравнения — чистое, без похода в систему: один файл, названный
    /// по-разному, это один процесс. Регистр на Windows не значит ничего,
    /// `.` и `..` разворачиваются.
    #[test]
    fn same_image_sees_through_case_and_dot_segments() {
        let me = std::env::current_exe().unwrap();
        let dir = me.parent().unwrap();
        let name = me.file_name().unwrap();
        let roundabout = dir.join(".").join(name);
        assert!(same_image(&me, &roundabout), "{}", roundabout.display());
        if cfg!(windows) {
            let shouting = std::path::PathBuf::from(me.to_string_lossy().to_uppercase());
            assert!(same_image(&me, &shouting), "{}", shouting.display());
        }
        assert!(!same_image(&me, &dir.join("не-наш-httpd.exe")));
    }

    #[test]
    fn parse_tasklist_name_reads_the_quoted_first_column() {
        assert_eq!(parse_tasklist_name("\"1c-connect.exe\",\"812\",\"Console\",\"1\",\"12 345 K\"\n"), Some("1c-connect.exe".to_string()));
        assert_eq!(parse_tasklist_name("INFO: No tasks are running which match the specified criteria.\n"), None);
        assert_eq!(parse_tasklist_name(""), None);
    }
}
