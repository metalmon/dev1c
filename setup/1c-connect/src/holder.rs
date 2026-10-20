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
    if name.is_empty() { None } else { Some(name.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SS_LINE: &str = "LISTEN 0 4096 127.0.0.1:7771 0.0.0.0:* users:((\"1c-connect\",pid=812,fd=9))";

    // Образ `/proc/net/tcp`: заголовок, затем loopback:7779 (0x1E63)
    // слушающий (st 0A, inode 12345), и 0.0.0.0:80 (0x0050) тоже слушающий
    // (inode 22222) — адрес не разбирается, только порт и состояние.
    const PROC_TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
   0: 0100007F:1E63 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0
   1: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 22222 1 0000000000000000 100 0 0 10 0
";
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

    #[test]
    fn parse_tasklist_name_reads_the_quoted_first_column() {
        assert_eq!(parse_tasklist_name("\"1c-connect.exe\",\"812\",\"Console\",\"1\",\"12 345 K\"\n"), Some("1c-connect.exe".to_string()));
        assert_eq!(parse_tasklist_name("INFO: No tasks are running which match the specified criteria.\n"), None);
        assert_eq!(parse_tasklist_name(""), None);
    }
}
