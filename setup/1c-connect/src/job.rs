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

#[cfg(not(windows))]
pub struct Job;

#[cfg(not(windows))]
impl Job {
    pub fn new() -> Option<Job> {
        Some(Job)
    }
    pub fn assign(&self, _child: &tokio::process::Child) -> bool {
        true
    }
    pub fn assign_std(&self, _child: &std::process::Child) -> bool {
        true
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
}
