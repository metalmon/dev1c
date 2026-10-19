//! Дерево процессов моста (`oscript.exe` → `1cv8.exe`) в Job Object: закрылся
//! канал или вышел `1c-connect` — Конфигуратор не остаётся держать базу.

#[cfg(windows)]
pub struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Job {
    pub fn new() -> Option<Job> {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                windows_sys::Win32::Foundation::CloseHandle(h);
                return None;
            }
            Some(Job(h))
        }
    }

    /// Процесс и всё, что он породит, — в задании. `false` — описателя нет
    /// (процесс уже вышел) или система отказала.
    pub fn assign(&self, child: &tokio::process::Child) -> bool {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        let Some(h) = child.raw_handle() else { return false };
        unsafe { AssignProcessToJobObject(self.0, h as _) != 0 }
    }

    /// То же для процесса из `std` (локальный вызов без tokio).
    pub fn assign_std(&self, child: &std::process::Child) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as _) != 0 }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

// A job handle is just a kernel object reference; moving it between threads is fine.
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
pub fn group(_cmd: &mut tokio::process::Command) {}

#[cfg(windows)]
pub fn group_std(_cmd: &mut std::process::Command) {}

/// Своя группа процессов для ребёнка: без неё внуки (`oscript.exe` →
/// `1cv8.exe`) остаются в нашей группе и `killpg` в `Drop` их не достанет
/// отдельно от нас самих.
#[cfg(not(windows))]
pub fn group(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

#[cfg(not(windows))]
pub fn group_std(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(windows))]
pub struct Job(std::sync::Mutex<Vec<i32>>);

#[cfg(not(windows))]
impl Job {
    pub fn new() -> Option<Job> {
        Some(Job(std::sync::Mutex::new(Vec::new())))
    }

    /// 0 и отрицательные — не pgid ребёнка, а "моя группа"/ошибка; сигнал на
    /// них ушёл бы нам самим.
    fn remember(&self, pgid: i32) {
        if pgid > 0 {
            self.0.lock().unwrap().push(pgid);
        }
    }

    /// `child.id()` и есть pgid: группу процессу выставил `group`/`group_std`
    /// ещё до `spawn`.
    pub fn assign(&self, child: &tokio::process::Child) -> bool {
        if let Some(pid) = child.id() {
            self.remember(pid as i32);
        }
        true
    }
    pub fn assign_std(&self, child: &std::process::Child) -> bool {
        self.remember(child.id() as i32);
        true
    }
}

#[cfg(not(windows))]
impl Drop for Job {
    fn drop(&mut self) {
        let pgids = std::mem::take(&mut *self.0.lock().unwrap());
        for &pgid in &pgids {
            unsafe { libc::killpg(pgid, libc::SIGTERM) };
        }
        if pgids.is_empty() {
            return;
        }
        // Поток детачится: drop не ждёт 5 секунд эскалации до SIGKILL.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            for &pgid in &pgids {
                unsafe { libc::killpg(pgid, libc::SIGKILL) };
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn dropping_the_job_kills_the_tree() {
        let job = Job::new().expect("job object");
        let mut cmd = tokio::process::Command::new("cmd.exe");
        cmd.args(["/c", "ping -n 30 127.0.0.1 >nul"]);
        cmd.creation_flags(0x0800_0000);
        let mut child = cmd.spawn().unwrap();
        assert!(job.assign(&child));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(child.try_wait().unwrap().is_none(), "still running before the drop");
        drop(job);
        let r = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await;
        assert!(r.is_ok(), "cmd and its ping must die with the job");
    }

    #[cfg(not(windows))]
    #[test]
    fn job_is_a_noop_elsewhere() {
        let job = Job::new();
        assert!(job.is_some());
    }

    /// Первая строка из вывода ребёнка — pid внука, который он сам напечатал.
    #[cfg(not(windows))]
    fn read_first_line(out: &mut std::process::ChildStdout) -> u32 {
        use std::io::BufRead;
        let mut line = String::new();
        std::io::BufReader::new(out).read_line(&mut line).unwrap();
        line.trim().parse().unwrap()
    }

    /// Ждёт до 8 с, пока `/proc/<pid>` не исчезнет.
    #[cfg(not(windows))]
    fn waits_for_death(pid: u32) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }

    #[cfg(all(test, not(windows)))]
    #[test]
    fn dropping_the_job_kills_the_whole_group() {
        // sh порождает внука: умереть должен и он.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 30 & echo $!; wait"]).stdout(std::process::Stdio::piped());
        group_std(&mut cmd);
        let job = Job::new().unwrap();
        let mut child = cmd.spawn().unwrap();
        assert!(job.assign_std(&child));
        let grandchild = read_first_line(child.stdout.as_mut().unwrap());
        drop(job);
        assert!(waits_for_death(child.id()), "sh жив");
        assert!(waits_for_death(grandchild), "внук жив");
    }
}
