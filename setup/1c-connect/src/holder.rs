//! Кто держит порт рабочего места, если не этот процесс: имя и pid, чтобы и
//! текст занятого порта в `listen.rs`, и факт «порты рабочего места» в
//! `verify.rs` могли назвать держателя вместо голого «занято». В контейнере
//! (Linux) — `ss -ltnp`, на Windows — `netstat -ano` плюс `tasklist` за
//! именем (тот же приём, что уже применяет `bridge::configurator_count`).
//!
//! ponytail: не видит бинды на 0.0.0.0, которые тоже закрыли бы loopback —
//! апгрейд: парсить обе формы адреса, если это станет реальной проблемой.

use std::process::Command;

#[cfg(windows)]
fn no_window(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn no_window(_c: &mut Command) {}

/// Имя и pid процесса, слушающего `127.0.0.1:port`, в виде `1c-connect (pid
/// 812)` — `None`, если порт свободен, команда не нашлась, или вывод не
/// разобрать.
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
        let out = Command::new("ss").args(["-ltnp"]).output().ok()?;
        parse_ss(&String::from_utf8_lossy(&out.stdout), port)
    }
}

/// Строка `ss -ltnp` вида `LISTEN 0 4096 127.0.0.1:7771 0.0.0.0:*
/// users:(("1c-connect",pid=812,fd=9))` — имя и pid, или `None`, если порт не
/// слушается или строка не того вида, без паники.
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

/// Строка `netstat -ano` вида `TCP 127.0.0.1:7771 0.0.0.0:0 LISTENING 812` —
/// pid, или `None`, если порт не слушается или строка не того вида.
pub fn parse_netstat(out: &str, port: u16) -> Option<u32> {
    let local = format!("127.0.0.1:{port}");
    out.lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() >= 5 && f[0] == "TCP" && f[1] == local && f[3] == "LISTENING" { f[4].parse().ok() } else { None }
    })
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
    const NETSTAT_LINE: &str = "TCP 127.0.0.1:7771 0.0.0.0:0 LISTENING 812";

    #[test]
    fn parse_ss_names_the_holder_and_misses_other_ports() {
        assert_eq!(parse_ss(SS_LINE, 7771), Some("1c-connect (pid 812)".to_string()));
        assert_eq!(parse_ss(SS_LINE, 7772), None);
        assert_eq!(parse_ss("", 7771), None);
        assert_eq!(parse_ss("junk\nmore junk", 7771), None);
    }

    #[test]
    fn parse_netstat_gets_the_pid_and_misses_other_ports() {
        assert_eq!(parse_netstat(NETSTAT_LINE, 7771), Some(812));
        assert_eq!(parse_netstat(NETSTAT_LINE, 7781), None);
        assert_eq!(parse_netstat("", 7771), None);
        assert_eq!(parse_netstat("junk\nmore junk", 7771), None);
    }

    #[test]
    fn parse_tasklist_name_reads_the_quoted_first_column() {
        assert_eq!(parse_tasklist_name("\"1c-connect.exe\",\"812\",\"Console\",\"1\",\"12 345 K\"\n"), Some("1c-connect.exe".to_string()));
        assert_eq!(parse_tasklist_name("INFO: No tasks are running which match the specified criteria.\n"), None);
        assert_eq!(parse_tasklist_name(""), None);
    }
}
