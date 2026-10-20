//! Git-инструменты моста: состояние, pull, push — одним набором флагов, из
//! MinGit комплекта. Пароль не в URL, а в заголовке; вывод проходит затирку.
//! Мьютекс проекта: второй вызов получает «занято», не очередь.

use crate::cfg::Cfg;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub const GITATTRIBUTES: &str = "* -text\n";
pub const GITIGNORE: &str = "build/\n*.log\nautumn-properties.local.json\n";
/// Настройки vrunner без секретов: выгрузка в src, загрузка оттуда инкрементом.
pub const PROPERTIES: &str = "{\n  \"vrunner\": {\n    \"src\": \"src\",\n    \"cf\": {\n      \"decompile\": { \"out\": \"src\" },\n      \"load\": { \"increment\": true }\n    }\n  }\n}\n";

pub enum Layout {
    Missing,
    /// В каталоге лежит база, не проект.
    BaseDir(String),
    Drifted(String),
    Ok,
}

fn looks_like_a_base(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|n| n.eq_ignore_ascii_case("1Cv8.1CD") || n.to_lowercase().ends_with(".dt"))
}

pub fn layout_state(p: &Project) -> Layout {
    if let Some(f) = looks_like_a_base(&p.dir) {
        return Layout::BaseDir(f);
    }
    if !p.dir.join(".git").exists() {
        return Layout::Missing;
    }
    let branch = git(p, &["symbolic-ref", "--short", "HEAD"]).map(|o| o.text).unwrap_or_default();
    if branch != p.branch {
        return Layout::Drifted(format!("ветка {branch}, нужна {}", p.branch));
    }
    let origin = git(p, &["remote", "get-url", "origin"]).map(|o| o.text).unwrap_or_default();
    if origin != p.remote {
        return Layout::Drifted(format!("origin {origin}, нужен {}", p.remote));
    }
    for f in [".gitattributes", ".gitignore", "autumn-properties.json"] {
        if !p.dir.join(f).exists() {
            return Layout::Drifted(format!("нет {f}"));
        }
    }
    Layout::Ok
}

/// Разложить проект: каталог, git с веткой рабочего места и origin на Gitea,
/// три служебных файла (существующие не трогаются). Доказывает круг на
/// месте: git и oscript отвечают из этого каталога.
pub fn init(p: &Project, tools: &Path, log: &dyn Fn(String)) -> Result<(), String> {
    if let Some(f) = looks_like_a_base(&p.dir) {
        return Err(format!("{} — это каталог базы ({f}), не проекта; укажите другой каталог в project", p.dir.display()));
    }
    std::fs::create_dir_all(p.dir.join("src")).map_err(|e| format!("не создать {}: {e}", p.dir.display()))?;
    if !p.dir.join(".git").exists() {
        must(p, &["init", "-q", "-b", &p.branch])?;
        log(format!("git init, ветка {}", p.branch));
    } else {
        let branch = must(p, &["symbolic-ref", "--short", "HEAD"]).unwrap_or_default();
        if branch != p.branch {
            // Switch to the workstation's branch; create it only if it does
            // not exist — `-B` would reset a branch that already has work.
            let exists = git(p, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}", p.branch)])?.ok;
            if exists {
                must(p, &["checkout", "-q", &p.branch])?;
            } else {
                must(p, &["checkout", "-q", "-b", &p.branch])?;
            }
            log(format!("ветка {}", p.branch));
        }
    }
    ensure_origin(p)?;
    for (name, body) in [(".gitattributes", GITATTRIBUTES), (".gitignore", GITIGNORE), ("autumn-properties.json", PROPERTIES)] {
        let f = p.dir.join(name);
        if !f.exists() {
            std::fs::write(&f, body).map_err(|e| format!("{name}: {e}"))?;
            log(format!("{name} создан"));
        }
    }
    must(p, &["status", "--porcelain"])?;
    let mut c = Command::new(crate::ops_host::oscript_exe(tools));
    c.arg("-version").current_dir(&p.dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    match c.output() {
        Ok(o) if o.status.success() => log(format!("oscript {} из {}", String::from_utf8_lossy(&o.stdout).trim(), p.dir.display())),
        _ => log(format!("oscript в {} не запустился — выполните 1c-connect install (host.tools)", tools.display())),
    }
    Ok(())
}

pub struct Project {
    pub dir: PathBuf,
    pub git: PathBuf,
    pub branch: String,
    pub remote: String,
    pub login: String,
}

impl Project {
    /// `tools` — каталог инструментов на машине (`%LOCALAPPDATA%\dev1c`).
    pub fn from_cfg(cfg: &Cfg, tools: &Path) -> Project {
        let dir = PathBuf::from(&cfg.host.project);
        // The last path segment by either separator: a Windows path read on
        // Linux (CI) has no separators as far as PathBuf is concerned.
        let name = cfg.host.project.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().filter(|n| !n.is_empty()).unwrap_or("project").to_string();
        let git = if cfg!(windows) { tools.join("git").join("cmd").join("git.exe") } else { tools.join("git").join("bin").join("git") };
        Project { dir, git, branch: cfg.host.workstation.clone(), remote: format!("{}/dev1c/{name}.git", cfg.gitea_url()), login: cfg.host.workstation.clone() }
    }
}

/// Один набор флагов на все вызовы: личность, длинные пути, кириллица в
/// именах, никакого менеджера паролей, пароль заголовком (в пилоте равен логину).
pub fn git_flags(login: &str) -> Vec<String> {
    let auth = base64(format!("{login}:{login}").as_bytes());
    [
        format!("user.name={login}"),
        format!("user.email={login}@dev1c.local"),
        "safe.directory=*".into(),
        "core.longpaths=true".into(),
        "core.quotepath=false".into(),
        "credential.helper=".into(),
        format!("http.extraHeader=Authorization: Basic {auth}"),
        // A configuration dump is ~90k small files. On Windows `git commit`
        // waits for its own auto-gc (it cannot detach), and repacking that
        // many objects took an hour on a laptop. Never gc from the bridge;
        // the two caches make status/add on 90k files bearable.
        "gc.auto=0".into(),
        "core.fscache=true".into(),
        "core.preloadindex=true".into(),
    ]
    .into_iter()
    .flat_map(|f| ["-c".to_string(), f])
    .collect()
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

struct Out {
    ok: bool,
    text: String,
}

/// Один набор флагов и окружения на все пути запуска — обычный и прерываемый.
fn git_cmd(p: &Project, args: &[&str]) -> Command {
    let mut c = Command::new(&p.git);
    c.args(git_flags(&p.login)).args(args).current_dir(&p.dir).env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

fn git(p: &Project, args: &[&str]) -> Result<Out, String> {
    let o = git_cmd(p, args).output().map_err(|e| format!("git не запускается ({}): {e} — выполните 1c-connect install", p.git.display()))?;
    let mut text = String::from_utf8_lossy(&o.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&o.stderr));
    Ok(Out { ok: o.status.success(), text: common::docker::redact_text(text.trim()) })
}

fn must(p: &Project, args: &[&str]) -> Result<String, String> {
    let o = git(p, args)?;
    if o.ok {
        Ok(o.text)
    } else {
        Err(format!("git {}: {}", args.join(" "), o.text))
    }
}

/// Как `git`, но прерываемо: `cancel` взведённый до старта — вообще не
/// запускает процесс, взведённый во время ожидания — убивает его. Оба пайпа
/// разбираются в своих потоках, пока `try_wait` опрашивает раз в 250 мс —
/// иначе `git`, заполнив ~8 КиБ буфер (тысячи строк конфликтов
/// `pull --rebase` или длинный `remote:` от хука push), блокируется на
/// записи и никогда не завершается, а `try_wait` крутится вечно.
fn git_wait(p: &Project, args: &[&str], cancel: &AtomicBool) -> Result<Out, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("прервано".into());
    }
    let mut child = git_cmd(p, args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git не запускается ({}): {e} — выполните 1c-connect install", p.git.display()))?;
    let (mut out_h, mut err_h) = (child.stdout.take(), child.stderr.take());
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(h) = out_h.as_mut() {
            let _ = std::io::Read::read_to_end(h, &mut buf);
        }
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(h) = err_h.as_mut() {
            let _ = std::io::Read::read_to_end(h, &mut buf);
        }
        buf
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            // Читающие потоки сами закончатся по закрытию пайпов убитого
            // процесса — ждать их здесь незачем, важно только «прервано».
            return Err("прервано".into());
        }
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let mut text = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).to_string();
    text.push_str(&String::from_utf8_lossy(&err_thread.join().unwrap_or_default()));
    Ok(Out { ok: status.success(), text: common::docker::redact_text(text.trim()) })
}

fn must_wait(p: &Project, args: &[&str], cancel: &AtomicBool) -> Result<String, String> {
    let o = git_wait(p, args, cancel)?;
    if o.ok {
        Ok(o.text)
    } else {
        Err(format!("git {}: {}", args.join(" "), o.text))
    }
}

/// Есть ли ветка на сервере. Репозитория ещё нет (push-to-create: его
/// создаст первый push) — тоже «нет ветки», а не ошибка.
/// `origin` всегда равен настроенному адресу: каталог мог быть git-инициализирован
/// руками или указывать на старую среду, а status/pull/push говорят про `origin`.
fn ensure_origin(p: &Project) -> Result<(), String> {
    match git(p, &["remote", "get-url", "origin"])? {
        Out { ok: true, text } if text.trim() == p.remote => Ok(()),
        Out { ok: true, .. } => must(p, &["remote", "set-url", "origin", p.remote.as_str()]).map(|_| ()),
        Out { ok: false, .. } => must(p, &["remote", "add", "origin", p.remote.as_str()]).map(|_| ()),
    }
}

fn remote_has(p: &Project, branch: &str) -> Result<bool, String> {
    let o = git(p, &["ls-remote", "--heads", p.remote.as_str(), branch])?;
    Ok(o.ok && o.text.contains(&format!("refs/heads/{branch}")))
}

/// `dev1c/<имя>` из `remote` (`.../dev1c/<имя>.git`) — агент это имя
/// угадать не может: его собственный каталог зовётся просто `project`.
fn repo_name(p: &Project) -> String {
    let s = p.remote.strip_suffix(".git").unwrap_or(&p.remote);
    let mut parts = s.rsplitn(3, '/');
    let name = parts.next().unwrap_or(s);
    let org = parts.next().unwrap_or("dev1c");
    format!("{org}/{name}")
}

const CANNOT_REACH_THE_BASE: &str = "эти правки в базу загрузить нельзя, пока они не отправлены";

pub fn status(p: &Project) -> Result<String, String> {
    ensure_origin(p)?;
    let branch = must(p, &["symbolic-ref", "--short", "HEAD"]).unwrap_or_else(|_| "(нет коммитов)".into());
    let last = must(p, &["log", "-1", "--format=%h %s (%cr)"]).unwrap_or_else(|_| "нет".into());
    let ahead = git(p, &["rev-list", "--count", &format!("origin/{}..HEAD", p.branch)]).map(|o| if o.ok { o.text } else { "все".into() })?;
    let changed = must(p, &["status", "--porcelain"])?.lines().count();
    let index = if p.dir.join("src").join("ConfigDumpInfo.xml").exists() { "есть" } else { "нет — первая загрузка будет полной" };
    let mut out = format!(
        "ветка {branch}\nрепозиторий: {}\nветка рабочего места: {}\nпоследний коммит: {last}\nнеотправленных коммитов: {ahead}\nизменено: {changed}\nConfigDumpInfo.xml: {index}",
        repo_name(p),
        p.branch
    );
    if ahead != "0" || changed > 0 {
        out.push('\n');
        out.push_str(CANNOT_REACH_THE_BASE);
    }
    Ok(out)
}

/// Пауза перед повтором `pull`: короткая, но настоящая — ровно то время,
/// которое Gitea занимает, чтобы разобрать только что принятый пакет.
const PULL_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

/// Повторяет `attempt` один раз после паузы, если первая попытка вернула
/// ошибку. Нужен ровно для pull сразу после чужой отправки: Gitea могла ещё
/// не отдать все нужные объекты («сервер не отдал все нужные объекты»), а
/// через пару секунд то же самое получается само. Первая неудача наружу не
/// идёт — сообщаем, только если не получилось и со второго раза.
fn retry_once<T>(pause: std::time::Duration, attempt: impl Fn() -> Result<T, String>) -> Result<T, String> {
    attempt().or_else(|_| {
        std::thread::sleep(pause);
        attempt()
    })
}

pub fn pull(p: &Project, cancel: &AtomicBool) -> Result<String, String> {
    ensure_origin(p)?;
    // A fresh project has no branch on the server yet — that comes first:
    // its tree is "dirty" too, and the useful answer is "export first".
    if !remote_has(p, &p.branch)? {
        return Err(format!("ветки {} в Gitea ещё нет — сначала «Выгрузить из 1С» (cf_decompile, затем project_push)", p.branch));
    }
    if !must(p, &["status", "--porcelain"])?.is_empty() {
        return Err("в каталоге проекта есть незакоммиченные изменения — сначала project_push".into());
    }
    let attempt = || -> Result<String, String> {
        let o = git_wait(p, &["pull", "--rebase", "origin", &p.branch], cancel)?;
        if o.ok {
            Ok(format!("{}\nтеперь cf_load", o.text))
        } else {
            Err(o.text)
        }
    };
    match retry_once(PULL_RETRY_PAUSE, attempt) {
        Ok(v) => Ok(v),
        Err(e) => {
            let conflicted = must(p, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            if !conflicted.is_empty() {
                // Half-applied rebase is worse than none: cf_load would read a tree
                // with conflict markers. Back out and hand the merge to a human.
                let _ = git(p, &["rebase", "--abort"]);
                return Err(format!(
                    "конфликт в {} — разрешите в каталоге проекта {} (git pull --rebase origin {} и правка файлов); дерево возвращено в состояние до pull",
                    conflicted.replace('\n', ", "),
                    p.dir.display(),
                    p.branch
                ));
            }
            Err(e)
        }
    }
}

pub fn push(p: &Project, message: &str, cancel: &AtomicBool) -> Result<String, String> {
    ensure_origin(p)?;
    // Only the roots that exist: a pathspec that matches nothing fails the
    // whole add, and before host.project (6B) some of them are not there yet.
    let roots: Vec<&str> = ["src", ".gitattributes", ".gitignore", "autumn-properties.json"].into_iter().filter(|r| p.dir.join(r).exists()).collect();
    if roots.is_empty() {
        return Err(format!("в {} нет src — сначала cf_decompile", p.dir.display()));
    }
    let mut add = vec!["add", "--"];
    add.extend(roots);
    must(p, &add)?;
    let staged = !must(p, &["diff", "--cached", "--name-only"])?.is_empty();
    let has_commits = git(p, &["rev-parse", "--verify", "HEAD"])?.ok;
    if staged || !has_commits {
        must(p, &["commit", "-q", "--allow-empty", "-m", if message.trim().is_empty() { "выгрузка" } else { message }])?;
    }
    let first = !remote_has(p, "main")?;
    if first {
        must_wait(p, &["push", "-q", "origin", "HEAD:refs/heads/main"], cancel)?;
    }
    must_wait(p, &["push", "-q", "-u", "origin", &p.branch], cancel)?;
    let what = if staged { "отправлено" } else { "нечего коммитить, ветка отправлена" };
    Ok(format!("{what}: ветка {} в {}{}", p.branch, p.remote, if first { " (репозиторий создан, main тоже отправлен)" } else { "" }))
}

/// Флаг отмены для CLI и моста: его никто не взводит — окно (Task 4) заведёт свой.
pub static NEVER: AtomicBool = AtomicBool::new(false);

/// Один проект — один процесс за раз.
pub static LOCK: Mutex<()> = Mutex::new(());

/// Тестовый серилизатор: cargo гоняет тесты в одном бинарнике параллельно,
/// а `LOCK` — один глобал на процесс. Берётся первым во всех тестах этого
/// файла и `flow.rs`, что держат или оспаривают `LOCK` — иначе сосед,
/// попавший в то же окно, получает «занято» и падает на `.unwrap()`.
#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());

static DOING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const DOING_NAMES: [&str; 4] = ["", "project_status", "project_pull", "project_push"];

/// Что сейчас идёт под мьютексом (пусто — ничего).
pub fn doing() -> &'static str {
    DOING_NAMES[DOING.load(std::sync::atomic::Ordering::Relaxed)]
}

/// Вызов инструмента моста: `(текст, isError)`. Занято — ответ, не очередь.
pub fn call(p: &Project, tool: &str, args: &Value) -> (String, bool) {
    use std::sync::atomic::Ordering::Relaxed;
    // A panic inside a tool poisons the mutex; the project is not broken by
    // that, so a poisoned lock is simply taken.
    let _guard = match LOCK.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            let what = doing();
            return (format!("занято: идёт {}", if what.is_empty() { "другая операция" } else { what }), true);
        }
    };
    DOING.store(DOING_NAMES.iter().position(|n| *n == tool).unwrap_or(1), Relaxed);
    let r = match tool {
        "project_status" => status(p),
        "project_pull" => pull(p, &NEVER),
        "project_push" => push(p, args.get("message").and_then(Value::as_str).unwrap_or(""), &NEVER),
        other => Err(format!("нет инструмента {other}")),
    };
    DOING.store(0, Relaxed);
    match r {
        Ok(t) => (t, false),
        Err(t) => (t, true),
    }
}

#[cfg(test)]
mod tempdir {
    pub struct Dir(std::path::PathBuf);
    impl Dir {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "dev1c-proj-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Настоящий git с машины — для тестов; в комплекте будет MinGit.
    fn system_git() -> PathBuf {
        PathBuf::from(if cfg!(windows) { "git.exe" } else { "git" })
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = Command::new(system_git())
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "safe.directory=*"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// Голый origin и проект с веткой dev2, как положит host.project (6B).
    fn fixture() -> (tempdir::Dir, Project) {
        let tmp = tempdir::Dir::new();
        let origin = tmp.path().join("origin.git");
        git(tmp.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        let dir = tmp.path().join("Проект с пробелом");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q", "-b", "dev2"]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        std::fs::write(dir.join(".gitattributes"), "* -text\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "build/\n*.log\n").unwrap();
        std::fs::write(dir.join("autumn-properties.json"), "{}").unwrap();
        let p = Project { dir: dir.clone(), git: system_git(), branch: "dev2".into(), remote: origin.to_string_lossy().into(), login: "dev2".into() };
        (tmp, p)
    }

    #[test]
    fn flags_carry_identity_and_auth_header_without_a_password_in_the_url() {
        let f = git_flags("dev2");
        assert!(f.iter().any(|a| a == "user.name=dev2"));
        assert!(f.iter().any(|a| a == "http.extraHeader=Authorization: Basic ZGV2MjpkZXYy"));
        assert!(f.iter().any(|a| a == "credential.helper="));
        assert!(f.iter().any(|a| a == "gc.auto=0"), "no auto-gc from the bridge: a 90k-file commit must not repack");
        let mut cfg = Cfg::default();
        let p = Project::from_cfg(&cfg, Path::new("/tools"));
        assert_eq!(p.remote, "http://192.168.1.10:3391/dev1c/Buh.git");
        cfg.host.project = "/home/dev/proj/Бух/".into();
        assert_eq!(Project::from_cfg(&cfg, Path::new("/tools")).remote, "http://192.168.1.10:3391/dev1c/Бух.git");
        assert_eq!(p.branch, "dev1");
        assert!(p.git.to_string_lossy().contains("git"));
    }

    #[test]
    fn git_cmd_carries_the_same_flags_as_git() {
        let (_t, p) = fixture();
        let c = git_cmd(&p, &["status"]);
        let flags = git_flags(&p.login);
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[..flags.len()], flags[..]);
        assert_eq!(c.get_current_dir(), Some(p.dir.as_path()));
        assert!(
            c.get_envs().any(|(k, v)| k.to_string_lossy().into_owned() == "GIT_TERMINAL_PROMPT" && v.map(|v| v.to_string_lossy().into_owned()) == Some("0".into())),
            "{:?}",
            c.get_envs().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_raised_cancel_stops_push_before_the_push_and_a_clear_one_pushes() {
        let (_t, p) = fixture();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        let cancel = AtomicBool::new(true);
        assert_eq!(push(&p, "m", &cancel), Err("прервано".into()));
        // The commit happens through `must`, before the cancellable push — it stays.
        assert_eq!(git(&p.dir, &["log", "-1", "--format=%s"]), "m");
        let heads = git(&p.dir, &["ls-remote", "--heads", "origin"]);
        assert!(heads.is_empty(), "{heads}");
        // push() itself never takes LOCK — that's the caller's (flow::lock,
        // project::call) RAII guard, dropped on any return including the
        // cancelled one — so there is nothing about the mutex to prove here;
        // a clear flag simply pushes.
        push(&p, "m", &AtomicBool::new(false)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(remote_has(&p, "dev2"), Ok(true));
    }

    #[test]
    fn first_push_creates_main_and_the_workstation_branch() {
        let (_t, p) = fixture();
        std::fs::write(p.dir.join("src").join("Module.bsl"), "Процедура А() КонецПроцедуры").unwrap();
        std::fs::write(p.dir.join("secret.log"), "x").unwrap();
        let text = push(&p, "первая выгрузка", &NEVER).unwrap();
        assert!(text.contains("dev2"), "{text}");
        let heads = git(&p.dir, &["ls-remote", "--heads", "origin"]);
        assert!(heads.contains("refs/heads/main") && heads.contains("refs/heads/dev2"), "{heads}");
        // .log is ignored, and only the four roots are added.
        assert!(!git(&p.dir, &["ls-files"]).contains("secret.log"));
        // Nothing new → push still succeeds and says so.
        let again = push(&p, "пусто", &NEVER).unwrap();
        assert!(again.contains("нечего"), "{again}");
    }

    #[test]
    fn pull_refuses_a_dirty_tree_and_reports_conflicts_by_file() {
        let (t, p) = fixture();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "a", &NEVER).unwrap();
        // A second clone pushes a conflicting change to dev2.
        let other = t.path().join("other");
        git(t.path(), &["clone", "-q", "-b", "dev2", &p.remote, other.to_str().unwrap()]);
        std::fs::write(other.join("src").join("M.bsl"), "2").unwrap();
        git(&other, &["commit", "-qam", "theirs"]);
        git(&other, &["push", "-q", "origin", "dev2"]);
        // Dirty tree here → refused before touching git.
        std::fs::write(p.dir.join("src").join("M.bsl"), "3").unwrap();
        let e = pull(&p, &NEVER).unwrap_err();
        assert!(e.contains("project_push"), "{e}");
        // Commit locally without pushing: the push would be rejected (non-fast-forward).
        let err = push(&p, "mine", &NEVER).unwrap_err();
        assert!(err.contains("push"), "{err}");
        let e = pull(&p, &NEVER).unwrap_err();
        assert!(e.contains("конфликт") && e.contains("M.bsl"), "{e}");
        // The rebase is aborted: the tree is clean and nothing is half-applied,
        // the local commit still exists for a human to merge by hand.
        assert!(git(&p.dir, &["status", "--porcelain"]).is_empty());
        assert!(!p.dir.join(".git").join("rebase-merge").exists());
        assert_eq!(git(&p.dir, &["log", "-1", "--format=%s"]), "mine");
    }

    #[test]
    fn a_repository_gitea_has_not_created_yet_counts_as_empty() {
        // Push-to-create: before the first push there is no repository at all,
        // and `ls-remote` fails with "repository not found". That is "no
        // branch", not an error — otherwise the first push can never happen.
        let (t, mut p) = fixture();
        p.remote = t.path().join("absent.git").to_string_lossy().into();
        assert_eq!(remote_has(&p, "main"), Ok(false));
        let e = pull(&p, &NEVER).unwrap_err();
        assert!(e.contains("Выгрузить из 1С"), "{e}");
    }

    #[test]
    fn push_tolerates_roots_that_do_not_exist_yet() {
        let (_t, p) = fixture();
        std::fs::remove_file(p.dir.join("autumn-properties.json")).unwrap();
        std::fs::remove_file(p.dir.join(".gitignore")).unwrap();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "без служебных файлов", &NEVER).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn pull_before_any_push_says_export_first() {
        let (_t, p) = fixture();
        let e = pull(&p, &NEVER).unwrap_err();
        assert!(e.contains("Выгрузить из 1С"), "{e}");
    }

    #[test]
    fn status_names_branch_unpushed_and_the_dump_index() {
        let (_t, p) = fixture();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "a", &NEVER).unwrap();
        std::fs::write(p.dir.join("src").join("ConfigDumpInfo.xml"), "<x/>").unwrap();
        let s = status(&p).unwrap();
        assert!(s.contains("ветка dev2") && s.contains("ConfigDumpInfo.xml: есть") && s.contains("изменено: 1"), "{s}");
    }

    /// Агент не знает и не может угадать имя своего репозитория в Gitea
    /// (его каталог зовётся просто `project`) — `status` обязан его назвать.
    #[test]
    fn status_names_the_repository_and_the_branch() {
        let (_t, mut p) = fixture();
        p.remote = "http://127.0.0.1:3000/dev1c/UNF.git".into();
        let s = status(&p).unwrap();
        assert!(s.contains("репозиторий: dev1c/UNF"), "{s}");
        assert!(s.contains("ветка рабочего места: dev2"), "{s}");
    }

    /// В туннельном режиме `cf_load` грузит каталог проекта на машине
    /// человека, куда файлы попадают только через Gitea — незакоммиченная
    /// или неотправленная правка недоступна никакому вызову, и агент
    /// обязан это узнать из текста, а не по молчанию базы.
    #[test]
    fn status_warns_that_unsent_work_cannot_reach_the_base() {
        let (_t, p) = fixture();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "a", &NEVER).unwrap();
        assert!(!status(&p).unwrap().contains(CANNOT_REACH_THE_BASE), "clean tree must not warn");
        std::fs::write(p.dir.join("src").join("M.bsl"), "2").unwrap();
        assert!(status(&p).unwrap().contains(CANNOT_REACH_THE_BASE), "changed file must warn");
        let (_t2, p2) = fixture();
        std::fs::write(p2.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p2, "unpushed", &NEVER).unwrap();
        git(&p2.dir, &["commit", "-q", "--allow-empty", "-m", "local only"]);
        assert!(status(&p2).unwrap().contains(CANNOT_REACH_THE_BASE), "unpushed commit must warn");
    }

    #[test]
    fn a_panic_inside_a_tool_does_not_jam_the_project_forever() {
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_t, p) = fixture();
        let _ = std::thread::spawn(|| {
            let _g = LOCK.lock().unwrap();
            panic!("tool blew up while holding the lock");
        })
        .join();
        let (text, err) = call(&p, "project_status", &Value::Null);
        assert!(!err, "{text}");
    }

    #[test]
    fn origin_is_pointed_at_the_configured_remote_before_any_push_or_pull() {
        let (t, mut p) = fixture();
        git(&p.dir, &["remote", "set-url", "origin", "C:/nowhere/old.git"]);
        let fresh = t.path().join("fresh.git");
        git(t.path(), &["init", "-q", "--bare", "-b", "main", fresh.to_str().unwrap()]);
        p.remote = fresh.to_string_lossy().into();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "a", &NEVER).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&p.dir, &["remote", "get-url", "origin"]), p.remote);
        assert!(git(&fresh, &["branch", "--list"]).contains("dev2"));
        // No origin at all (a directory somebody git-inited by hand): added.
        git(&p.dir, &["remote", "remove", "origin"]);
        assert!(status(&p).unwrap().contains("ветка dev2"));
        assert_eq!(git(&p.dir, &["remote", "get-url", "origin"]), p.remote);
    }

    #[test]
    fn init_lays_out_a_project_with_a_cyrillic_path() {
        let t = tempdir::Dir::new();
        let origin = t.path().join("o.git");
        git(t.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        let dir = t.path().join("Проект Бух");
        let p = Project { dir: dir.clone(), git: system_git(), branch: "dev3".into(), remote: origin.to_string_lossy().into(), login: "dev3".into() };
        assert!(matches!(layout_state(&p), Layout::Missing));
        let lines = std::cell::RefCell::new(Vec::new());
        init(&p, Path::new("nowhere"), &|s| lines.borrow_mut().push(s)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), "dev3");
        assert_eq!(git(&dir, &["remote", "get-url", "origin"]), p.remote);
        assert_eq!(std::fs::read_to_string(dir.join(".gitattributes")).unwrap(), GITATTRIBUTES);
        assert!(std::fs::read_to_string(dir.join("autumn-properties.json")).unwrap().contains("\"increment\": true"));
        assert!(dir.join("src").is_dir());
        assert!(matches!(layout_state(&p), Layout::Ok));
        assert!(lines.borrow().iter().any(|l| l.contains("git init")), "{:?}", lines.borrow());
        // A second init changes nothing and keeps a hand-edited file.
        std::fs::write(dir.join(".gitignore"), "mine\n").unwrap();
        init(&p, Path::new("nowhere"), &|_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).unwrap(), "mine\n");
    }

    #[test]
    fn init_switches_to_an_existing_workstation_branch_without_resetting_it() {
        // The person merged into main by hand and is still on it; dev2 has
        // its own commit. install must switch back to dev2, not reset it.
        let (_t, p) = fixture();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "work on dev2", &NEVER).unwrap();
        git(&p.dir, &["checkout", "-q", "-b", "main"]);
        git(&p.dir, &["commit", "-q", "--allow-empty", "-m", "main moved on"]);
        init(&p, Path::new("nowhere"), &|_| {}).unwrap();
        assert_eq!(git(&p.dir, &["symbolic-ref", "--short", "HEAD"]), "dev2");
        assert_eq!(git(&p.dir, &["log", "-1", "--format=%s"]), "work on dev2");
    }

    #[test]
    fn a_base_directory_is_refused_and_drift_is_named() {
        let t = tempdir::Dir::new();
        let dir = t.path().join("base");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1Cv8.1CD"), "").unwrap();
        let p = Project { dir: dir.clone(), git: system_git(), branch: "dev1".into(), remote: "x".into(), login: "dev1".into() };
        assert!(matches!(layout_state(&p), Layout::BaseDir(_)));
        assert!(init(&p, Path::new("nowhere"), &|_| {}).unwrap_err().contains("каталог базы"));
        let (_t2, p2) = fixture(); // branch dev2, origin set, three files present
        git(&p2.dir, &["checkout", "-q", "-b", "main"]);
        assert!(matches!(layout_state(&p2), Layout::Drifted(d) if d.contains("ветка")));
    }

    /// Фальшивый «бегунок»: первая попытка отказывает так, как отказывает
    /// git сразу после чужой отправки («сервер не отдал все нужные
    /// объекты»), вторая — как будто Gitea уже дообработала пакет.
    #[test]
    fn retry_once_recovers_after_a_single_transient_failure() {
        let calls = std::cell::Cell::new(0u32);
        let r = retry_once(std::time::Duration::from_millis(1), || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Err("сервер не отдал все нужные объекты".to_string())
            } else {
                Ok("готово".to_string())
            }
        });
        assert_eq!(r, Ok("готово".to_string()));
        assert_eq!(calls.get(), 2, "первая неудача должна была привести ровно к одному повтору");
    }

    #[test]
    fn retry_once_reports_when_the_second_attempt_fails_too() {
        let calls = std::cell::Cell::new(0u32);
        let r = retry_once(std::time::Duration::from_millis(1), || {
            calls.set(calls.get() + 1);
            Err::<String, _>("сервер не отдал все нужные объекты".to_string())
        });
        assert_eq!(r, Err("сервер не отдал все нужные объекты".to_string()));
        assert_eq!(calls.get(), 2, "ровно одна попытка и ровно один повтор — не больше");
    }

    #[test]
    fn second_caller_is_told_busy_not_queued() {
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_t, p) = fixture();
        let guard = LOCK.try_lock().unwrap_or_else(|e| match e {
            std::sync::TryLockError::Poisoned(g) => g.into_inner(),
            e => panic!("{e}"),
        });
        let (text, err) = call(&p, "project_status", &Value::Null);
        assert!(err && text.contains("занято"), "{text}");
        drop(guard);
        let (text, err) = call(&p, "project_status", &Value::Null);
        assert!(!err, "{text}");
    }
}
