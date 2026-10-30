//! Git-инструменты моста: состояние, pull, push — одним набором флагов, из
//! MinGit комплекта. Пароль не в URL, а в заголовке; вывод проходит затирку.
//! Мьютекс проекта: второй вызов получает «занято», не очередь.

use crate::cfg::Cfg;
use crate::single_instance;
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
    /// Адреса среды нет в toml — `origin` собирать не из чего, и выдумывать
    /// его программа не станет (спека, «программа не выдумывает адресов»).
    NoGitea,
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
    // Ветка и origin — только там, где они наши. В локальном режиме «нужной»
    // ветки нет (`p.branch` и есть текущая), а origin человека сравнивать не
    // с чем: «origin X, нужен » было бы требованием привязаться к среде,
    // которой нет.
    if !p.local {
        // После `.git`, а не до: у неразложенного каталога первопричина — что
        // он не разложен, и подменять её адресом среды нельзя (находка ревью
        // задачи 35). Совет в обоих случаях один — `install`.
        if p.remote.is_empty() {
            return Layout::NoGitea;
        }
        let branch = git(p, &["symbolic-ref", "--short", "HEAD"]).map(|o| o.text).unwrap_or_default();
        if branch != p.branch {
            return Layout::Drifted(format!("ветка {branch}, нужна {}", p.branch));
        }
        let origin = git(p, &["remote", "get-url", "origin"]).map(|o| o.text).unwrap_or_default();
        if origin != p.remote {
            return Layout::Drifted(format!("origin {origin}, нужен {}", p.remote));
        }
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
///
/// `analyzer` — строки платформы для `bsl-analyzer.toml`
/// (`crate::analyzer::expected`); пусто — версия платформы неизвестна, и
/// файла не будет вовсе.
pub fn init(p: &Project, tools: &Path, analyzer: &[(&'static str, String)], log: &dyn Fn(String)) -> Result<(), String> {
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
    for key in crate::analyzer::ensure(&p.dir, analyzer)? {
        log(format!("{}: {key}", crate::analyzer::FILE));
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
    /// Адрес, к которому мы приводим `origin`: `<gitea>/dev1c/<имя>.git`. Это
    /// конвенция среды — организация `dev1c` в её Gitea, — и пусто она бывает
    /// по двум разным причинам, которые различает `local`: адрес среды не
    /// задан (туннель) либо среды нет вовсе (локальный режим).
    pub remote: String,
    /// Логин рабочего места, которым подписываются коммиты и ходят в Gitea
    /// среды. Пусто — личность и учётку не навязываем вовсе, берётся то, что
    /// настроено у человека глобально в git (`git_flags`).
    pub login: String,
    /// Своего обмена со средой у этого проекта нет: ни адреса, который мы
    /// вправе навязать `origin`, ни ветки рабочего места, ни учётки среды.
    /// Обычный локальный репозиторий 1С-разработчика (решение владельца
    /// 30.10: коннектор — самостоятельное приложение, локальный режим его
    /// главный путь).
    pub local: bool,
}

impl Project {
    /// `tools` — каталог инструментов на машине (`%LOCALAPPDATA%\dev1c`).
    pub fn from_cfg(cfg: &Cfg, tools: &Path) -> Project {
        let dir = PathBuf::from(&cfg.host.project);
        // The last path segment by either separator: a Windows path read on
        // Linux (CI) has no separators as far as PathBuf is concerned.
        let name = cfg
            .host
            .project
            .trim_end_matches(['\\', '/'])
            .rsplit(['\\', '/'])
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or("project")
            .to_string();
        let git = if cfg!(windows) { tools.join("git").join("cmd").join("git.exe") } else { tools.join("git").join("bin").join("git") };
        let local = !crate::cfg::mode_needs_env(&cfg.host.mode);
        // Адреса среды нет — `remote` пуст, а не собран из пустоты: читатели
        // (`layout_state`, `ensure_origin`) говорят, чего не хватает. В
        // локальном режиме его не собирают вовсе: организации `dev1c` в чужой
        // Gitea не существует, а `origin` там принадлежит человеку.
        let remote = if local { String::new() } else { cfg.gitea_url().map(|g| format!("{g}/dev1c/{name}.git")).unwrap_or_default() };
        // Ветка в локальном режиме — та, на которой человек стоит: рабочих
        // мест нет, `workstation` пуст по замыслу, и «нужной» веткой стала бы
        // пустая строка («ветка main, нужна », `git init -b `). Читаем HEAD
        // файлом, а не процессом git: `from_cfg` зовут и проверка, и окно, и
        // мост на каждый вызов инструмента, а знание тут на одну строку.
        let branch = if local { head_branch(&dir) } else { cfg.host.workstation.clone() };
        let login = if local { String::new() } else { cfg.host.workstation.clone() };
        Project { dir, git, branch, remote, login, local }
    }
}

/// На какой ветке репозиторий сейчас: `.git/HEAD` — одна строка
/// `ref: refs/heads/<ветка>`. Нет репозитория, отделённый HEAD или `.git`
/// файлом (ворктри) — `main`: столько же правды, сколько у пустой строки, но
/// это рабочее имя для `git init -b`.
fn head_branch(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".git").join("HEAD"))
        .ok()
        .and_then(|h| h.trim().strip_prefix("ref: refs/heads/").map(str::to_string))
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "main".into())
}

/// Один набор флагов на все вызовы: личность, длинные пути, кириллица в
/// именах, никакого менеджера паролей, пароль заголовком (в пилоте равен логину).
///
/// Пустой `login` — локальный режим: личность и учётку не навязываем.
/// `user.name=`/`user.email=@dev1c.local` из пустого рабочего места дали бы
/// мусор в подписи коммита, а `Authorization: Basic` из того же пустого
/// логина — заведомо чужие учётные данные в чужой сервер (origin в этом
/// режиме принадлежит человеку: GitHub, своя Gitea, корпоративный git).
/// Менеджер паролей тоже не гасим — им человек в свой сервер и ходит.
/// Техническая половина остаётся вся: она безвредна и нужна одинаково в
/// обоих режимах.
pub fn git_flags(login: &str) -> Vec<String> {
    let identity = if login.is_empty() {
        Vec::new()
    } else {
        let auth = base64(format!("{login}:{login}").as_bytes());
        vec![
            format!("user.name={login}"),
            format!("user.email={login}@dev1c.local"),
            "credential.helper=".into(),
            format!("http.extraHeader=Authorization: Basic {auth}"),
        ]
    };
    identity
        .into_iter()
        .chain([
            "safe.directory=*".to_string(),
            "core.longpaths=true".into(),
            "core.quotepath=false".into(),
            // A configuration dump is ~90k small files. On Windows `git commit`
            // waits for its own auto-gc (it cannot detach), and repacking that
            // many objects took an hour on a laptop. Never gc from the bridge;
            // the two caches make status/add on 90k files bearable.
            "gc.auto=0".into(),
            "core.fscache=true".into(),
            "core.preloadindex=true".into(),
        ])
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
    let o =
        git_cmd(p, args).output().map_err(|e| format!("git не запускается ({}): {e} — выполните 1c-connect install", p.git.display()))?;
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
/// Почему круг не идёт, когда каталог проекта не разложен — словами и один
/// раз на всех. Живой случай 2026-10-21 (заказчик на стенде): `install` не
/// выполняли, `cf_decompile` отработал (ему хватает каталога), а на отправке
/// человек получил сырой git: «fatal: не найден git репозиторий … Останавливаю
/// поиск на границе файловой системы (GIT_DISCOVERY_ACROSS_FILESYSTEM не
/// установлен)». Проверка в это время говорила правду — «проект не разложен,
/// 1c-connect install (host.project)», — а операция сообщала о внутренностях
/// git вместо причины.
pub fn not_laid_out(dir: &std::path::Path) -> String {
    format!(
        "{} не разложен: нет git-репозитория — нажмите «Установить» в окне или выполните 1c-connect install (host.project)",
        dir.display()
    )
}

/// Общие ворота всех входов в git (`init`, `facts`, `pull`, `push`) и, в
/// туннельном режиме, приведение `origin` к нашему адресу.
///
/// В локальном режиме `origin` не трогается вовсе — ни `set-url`, ни `add`.
/// Он принадлежит человеку: 1С-разработчик заводит его обычным git, куда ему
/// нужно (GitHub, своя Gitea, корпоративный сервер), и `remote set-url` нашим
/// выдуманным адресом стирал бы чужую настройку. Пустой `remote` в этом
/// режиме — не «адрес среды не задан», а «адреса среды тут и не бывает»
/// (решение владельца 30.10).
fn ensure_origin(p: &Project) -> Result<(), String> {
    // Один заслон на `init`, `pull` и `push`: пустой адрес не уезжает в
    // `git remote set-url` и не превращается в «origin не отвечает».
    if !p.local && p.remote.is_empty() {
        return Err(crate::cfg::no_gitea());
    }
    // Каталог без `.git` — не «ошибка git», а невыполненная установка; `init`
    // сюда приходит уже после `git init`, поэтому заслон общий.
    if !p.dir.join(".git").exists() {
        return Err(not_laid_out(&p.dir));
    }
    if p.local {
        return Ok(());
    }
    match git(p, &["remote", "get-url", "origin"])? {
        Out { ok: true, text } if text.trim() == p.remote => Ok(()),
        Out { ok: true, .. } => must(p, &["remote", "set-url", "origin", p.remote.as_str()]).map(|_| ()),
        Out { ok: false, .. } => must(p, &["remote", "add", "origin", p.remote.as_str()]).map(|_| ()),
    }
}

/// Чем звать удалённый репозиторий в командах git: нашим адресом там, где он
/// наш, и именем `origin` в локальном режиме — адреса мы там не знаем и
/// знать не должны.
fn remote_arg(p: &Project) -> &str {
    if p.remote.is_empty() {
        "origin"
    } else {
        p.remote.as_str()
    }
}

fn remote_has(p: &Project, branch: &str) -> Result<bool, String> {
    let o = git(p, &["ls-remote", "--heads", remote_arg(p), branch])?;
    Ok(o.ok && o.text.contains(&format!("refs/heads/{branch}")))
}

/// Отказ обмена в локальном режиме: origin у репозитория нет, а выдумать его
/// нам не из чего — ни адреса среды, ни права навязать свой человеку. Слова
/// про обычный git, а не про поле `gitea`: в этом режиме человек заводит
/// origin сам и сам решает, куда (решение владельца 30.10).
pub const NO_ORIGIN: &str = "у репозитория нет origin — добавьте его обычным git (git remote add origin <адрес>), и обмен заработает";

/// Чем подписывать коммит, когда личность рабочего места не навязывается
/// (локальный режим, `git_flags` с пустым логином): у человека на новой
/// машине глобальной личности может не быть вовсе, и git отказывает своим
/// английским «Author identity unknown … Please tell me who you are» прямо в
/// журнал окна или в ответ агенту. Причина и способ — своими словами и один
/// раз; выдумывать личность за человека мы не станем, подпись в его истории
/// принадлежит ему.
const NO_IDENTITY: &str =
    "git не знает, кем подписывать коммиты — задайте один раз: git config --global user.name \"Имя\" и git config --global user.email \"you@example.com\"";

/// Личность для коммита есть. Спрашивается только в локальном режиме: в
/// туннельном её задаёт сам коннектор именем рабочего места (`git_flags`).
fn identity_ready(p: &Project) -> Result<(), String> {
    if !p.local {
        return Ok(());
    }
    let o = git(p, &["config", "--get", "user.email"])?;
    if o.ok && !o.text.trim().is_empty() {
        Ok(())
    } else {
        Err(NO_IDENTITY.into())
    }
}

/// Есть ли у репозитория origin, которым идти в обмен. Спрашивается только в
/// локальном режиме: в туннельном его уже поставил `ensure_origin`.
fn origin_url(p: &Project) -> Result<Option<String>, String> {
    let o = git(p, &["remote", "get-url", "origin"])?;
    Ok((o.ok && !o.text.trim().is_empty()).then(|| o.text.trim().to_string()))
}

/// Адрес без учётных данных — для показа, не для вызова git. Человек вправе
/// держать в origin токен (`https://имя:токен@host/путь.git`, обычная форма
/// для GitHub и корпоративных серверов), а этот адрес мы печатаем в журнал
/// окна и отдаём инструментом агенту — то есть модели на той стороне. Токен
/// в чужую модель не уедет (находка проверки безопасности 30.10; владелец
/// держит трансграничную передачу чувствительных данных главным условием
/// проекта). Форма `git@host:путь` паролей не носит вовсе — её не трогаем.
pub fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (rest, None),
    };
    let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    match path {
        Some(p) => format!("{scheme}://{host}/{p}"),
        None => format!("{scheme}://{host}"),
    }
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

/// Состояние проекта разобранным, а не текстом.
///
/// Заведено 25.10.2026, потому что окно показывало ТЕКСТОВЫЙ ОТЧЁТ ДЛЯ
/// ТЕРМИНАЛА, разбитый по строкам: семь одинаковых карточек с серой точкой у
/// каждой, и среди них «ветка dev1» и «ветка рабочего места: dev1» — для
/// человека одно слово дважды. Заказчик: «не очень понятно, что там пишется и
/// для чего». Текст остался для агента и командной строки
/// (`status_text`), а окно теперь само решает, что показать и чем это
/// сгруппировать.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Facts {
    /// Куда смотрит HEAD прямо сейчас.
    pub branch: String,
    /// Куда идёт отправка: `dev1c/<имя>` в Gitea среды, а в локальном режиме
    /// — адрес origin, какой человек завёл сам. Пусто — отправлять некуда
    /// (локальный репозиторий без origin).
    pub repo: String,
    /// Ветка рабочего места из настроек: в неё пойдёт отправка. С `branch`
    /// совпадает почти всегда, и расхождение — единственная причина, по
    /// которой их стоит показывать обе. Пусто в локальном режиме: ветку там
    /// выбирает человек, и «нужной» у нас нет.
    pub want_branch: String,
    /// Последний коммит одной строкой; пусто — коммитов ещё нет.
    pub last: String,
    /// Неотправленных коммитов: число, либо `все` — ветки на сервере ещё нет.
    /// Пусто — отправлять некуда, и считать нечего.
    pub ahead: String,
    /// Сколько файлов изменено и не закоммичено.
    pub changed: usize,
    /// `ConfigDumpInfo.xml` на месте: без него первая загрузка будет полной.
    pub index: bool,
    /// Правки доходят до базы только через обмен — так в туннельном режиме:
    /// агент на той стороне, база здесь, и пока ветка не отправлена, человеку
    /// её нечем загрузить. В локальном режиме `false`: агент правит те самые
    /// файлы, которые читает `cf_load`, и незакоммиченная работа загрузке не
    /// помеха.
    pub exchange: bool,
}

impl Facts {
    /// Ждёт ли что-нибудь отправки. Пустой `ahead` — отправлять некуда, и
    /// повода говорить «не отправлено» он сам по себе не даёт.
    pub fn unsent(&self) -> bool {
        (!self.ahead.is_empty() && self.ahead != "0") || self.changed > 0
    }
}

pub fn facts(p: &Project) -> Result<Facts, String> {
    ensure_origin(p)?;
    let branch = must(p, &["symbolic-ref", "--short", "HEAD"]).unwrap_or_else(|_| "(нет коммитов)".into());
    let last = must(p, &["log", "-1", "--format=%h %s (%cr)"]).unwrap_or_default();
    // Куда отправлять: в туннельном режиме это наш адрес и наша организация в
    // Gitea среды, в локальном — то, что человек завёл сам; не завёл —
    // отправлять некуда, и считать неотправленное незачем.
    // Адрес показываем без учётных данных: эта строка уходит и в журнал, и
    // агенту (`without_credentials`).
    let repo = if p.local { origin_url(p)?.map(|u| without_credentials(&u)).unwrap_or_default() } else { repo_name(p) };
    let ahead = if repo.is_empty() {
        String::new()
    } else {
        git(p, &["rev-list", "--count", &format!("origin/{}..HEAD", p.branch)]).map(|o| if o.ok { o.text } else { "все".into() })?
    };
    let changed = must(p, &["status", "--porcelain"])?.lines().count();
    Ok(Facts {
        branch,
        repo,
        want_branch: if p.local { String::new() } else { p.branch.clone() },
        last,
        ahead,
        changed,
        index: p.dir.join("src").join("ConfigDumpInfo.xml").exists(),
        exchange: !p.local,
    })
}

/// Тот же отчёт текстом — для агента (`project_status`) и командной строки.
/// Формат не менялся: на него смотрят и правила агента, и привычка человека.
///
/// Пустые поля строк не дают вовсе: в локальном режиме агент читал бы
/// «репозиторий: » и «ветка рабочего места: » про Gitea и рабочее место,
/// которых на этой машине нет (находка аудита 30.10). Отсутствие строки он
/// понимает как «этого тут не бывает» — в отличие от пустого двоеточия,
/// которое выглядит сломанным инструментом.
pub fn status_text(f: &Facts) -> String {
    let index = if f.index { "есть" } else { "нет — первая загрузка будет полной" };
    let last = if f.last.is_empty() { "нет" } else { &f.last };
    let mut lines = vec![format!("ветка {}", f.branch)];
    if !f.repo.is_empty() {
        lines.push(format!("репозиторий: {}", f.repo));
    }
    if !f.want_branch.is_empty() {
        lines.push(format!("ветка рабочего места: {}", f.want_branch));
    }
    lines.push(format!("последний коммит: {last}"));
    if !f.ahead.is_empty() {
        lines.push(format!("неотправленных коммитов: {}", f.ahead));
    }
    lines.push(format!("изменено: {}", f.changed));
    lines.push(format!("ConfigDumpInfo.xml: {index}"));
    // Про недосягаемую базу — только там, где она и правда за обменом.
    if f.exchange && f.unsent() {
        lines.push(CANNOT_REACH_THE_BASE.to_string());
    }
    lines.join("\n")
}

pub fn status(p: &Project) -> Result<String, String> {
    Ok(status_text(&facts(p)?))
}

/// Пауза перед повтором `pull`: короткая, но настоящая — ровно то время,
/// которое Gitea занимает, чтобы разобрать только что принятый пакет.
const PULL_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

/// Повторяет `attempt` один раз после паузы, если первая попытка вернула
/// ошибку. Нужен ровно для pull сразу после чужой отправки: Gitea могла ещё
/// не отдать все нужные объекты («сервер не отдал все нужные объекты»), а
/// через пару секунд то же самое получается само. Первая неудача наружу не
/// идёт — сообщаем, только если не получилось и со второго раза.
///
/// Отмена проверяется до паузы, не после: первая неудача из-за того, что
/// человек прервал операцию (а не из-за временного сбоя сервера), не должна
/// стоить ещё одной паузы и попытки — отмена обязана вернуть «прервано»
/// немедленно, а не вдобавок ждать `pause`.
fn retry_once<T>(pause: std::time::Duration, cancel: &AtomicBool, attempt: impl Fn() -> Result<T, String>) -> Result<T, String> {
    match attempt() {
        Ok(v) => Ok(v),
        Err(e) if cancel.load(Ordering::Relaxed) => Err(e),
        Err(_) => {
            std::thread::sleep(pause);
            attempt()
        }
    }
}

pub fn pull(p: &Project, cancel: &AtomicBool) -> Result<String, String> {
    ensure_origin(p)?;
    // В локальном режиме обмен идёт через origin человека — нет его, нет и
    // обмена, и сказать это надо его словами, а не про поле `gitea`.
    if p.local && origin_url(p)?.is_none() {
        return Err(NO_ORIGIN.into());
    }
    // A fresh project has no branch on the server yet — that comes first:
    // its tree is "dirty" too, and the useful answer is "export first".
    if !remote_has(p, &p.branch)? {
        return Err(format!(
            "ветки {} {} ещё нет — сначала «Выгрузить из 1С» (cf_decompile, затем project_push)",
            p.branch,
            if p.local { "у origin" } else { "в Gitea" }
        ));
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
    match retry_once(PULL_RETRY_PAUSE, cancel, attempt) {
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
    // `bsl-analyzer.toml` — в списке наравне со служебными файлами: он нужен
    // не нам, а **второй стороне**. Корень проекта у агента становится
    // пригодным для анализатора сам, с приходом ветки, без действий человека
    // (требование заказчика 2026-10-21); не попади файл в коммит, он остался
    // бы лежать на машине, где никакой анализатор его не читает.
    let roots: Vec<&str> = ["src", ".gitattributes", ".gitignore", "autumn-properties.json", crate::analyzer::FILE]
        .into_iter()
        .filter(|r| p.dir.join(r).exists())
        .collect();
    if roots.is_empty() {
        return Err(format!("в {} нет src — сначала cf_decompile", p.dir.display()));
    }
    let mut add = vec!["add", "--"];
    add.extend(roots);
    must(p, &add)?;
    let staged = !must(p, &["diff", "--cached", "--name-only"])?.is_empty();
    let has_commits = git(p, &["rev-parse", "--verify", "HEAD"])?.ok;
    if staged || !has_commits {
        identity_ready(p)?;
        must(p, &["commit", "-q", "--allow-empty", "-m", if message.trim().is_empty() { "выгрузка" } else { message }])?;
    }
    // Фиксация сделана — дальше обмен, и он бывает не всегда. В локальном
    // режиме отправлять некуда, пока человек не завёл origin сам, но работа
    // при этом не потеряна и базе уже доступна: `cf_load` читает те самые
    // файлы. Поэтому не отказ после сделанного коммита, а сделанное и
    // причина, по которой вторая половина не состоялась (решение владельца
    // 30.10: фиксация в локальном режиме работает всегда, без всякого
    // origin).
    let origin = if p.local { origin_url(p)? } else { Some(p.remote.clone()) };
    let committed = if staged { "зафиксировано" } else { "нечего коммитить" };
    let Some(origin) = origin else {
        return Ok(format!("{committed}: ветка {} — {NO_ORIGIN}", p.branch));
    };
    // Репозиторий в Gitea среды создаёт первый push, и вместе с ним туда
    // уезжает `main`: такова конвенция среды (`push-to-create` в организации
    // `dev1c`). К чужому серверу человека она не относится — там репозиторий
    // уже есть, а что делать с его `main`, решает он сам.
    let first = !p.local && !remote_has(p, "main")?;
    if first {
        must_wait(p, &["push", "-q", "origin", "HEAD:refs/heads/main"], cancel)?;
    }
    must_wait(p, &["push", "-q", "-u", "origin", &p.branch], cancel)?;
    let what = if staged { "отправлено" } else { "нечего коммитить, ветка отправлена" };
    Ok(format!(
        "{what}: ветка {} в {}{}",
        p.branch,
        without_credentials(&origin),
        if first { " (репозиторий создан, main тоже отправлен)" } else { "" }
    ))
}

/// Флаг отмены для CLI и моста: его никто не взводит — окно (Task 4) заведёт свой.
pub static NEVER: AtomicBool = AtomicBool::new(false);

/// Один проект — один процесс за раз. Межпроцессная часть этого же обещания
/// — `LOCK_TARGET`/`hold` ниже: сам `LOCK` видит только потоки одного
/// процесса, а у рабочего места их скоро станет несколько (окно, `serve`,
/// `connect`, подкоманда `mcp`).
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

/// Готовая строка отказа — одна формулировка на все пути: мьютекс в памяти,
/// файл на диске, агент моста.
fn busy(what: &str) -> String {
    format!("занято: идёт {}", if what.is_empty() { "другая операция" } else { what })
}

/// Каталог, в котором `hold` держит файл замка (`<dir>/project.lock`).
/// Выставляется один раз при старте долгоживущего процесса — окна, `serve`,
/// `connect`, будущей подкоманды `mcp`. Разовые команды (`status`, `example`,
/// `check`) её не зовут: они и так serialised через `LOCK` в своём же
/// процессе, а межпроцессный замок им не нужен (спека, «программа не
/// задерживает обычную работу человека»).
static LOCK_TARGET: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_lock_target(dir: &Path) {
    *LOCK_TARGET.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.to_path_buf());
}

/// Снять цель — глобал на процесс, и тест, поставивший её, обязан не
/// оставить её следующему (тот же приём, что уже был у соседних тестов
/// этого файла: `without_a_target_the_lock_is_in_memory_only` рассчитывает
/// на `None`). Открыт тестам другого модуля (`verify::tests`) — `LOCK_TARGET`
/// им не виден вовсе.
#[cfg(test)]
pub(crate) fn clear_lock_target_for_test() {
    *LOCK_TARGET.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Замок проекта, пережививший второй процесс: обёртка над тем же приёмом,
/// что `single_instance` (именованный мьютекс на Windows, `flock` на Linux),
/// вынесенным туда, чтобы его звали оба замка. Файл — свой
/// (`<state>/project.lock`), не каталог состояния: тот уже держит замок
/// рабочего места, и окно заперло бы само себя.
pub type FileLock = single_instance::Guard;

/// `None` — файл уже занят другим процессом.
pub fn file_lock(path: &Path) -> std::io::Result<Option<FileLock>> {
    single_instance::file_lock(path)
}

/// Держит ли межпроцессную часть замка СЕЙЧАС какой-то другой процесс — как
/// бы спросил `hold`, но без удержания: взялся — значит свободен, и
/// отпускаем сразу же. Important 4 финального ревью: `verify::busy_now`
/// знал только про мьютекс в памяти, своего же процесса — с тех пор, как
/// замок стал межпроцессным (`set_lock_target`/`project.lock`), это
/// разошлось с `project::facts_locked`, которая под тем же замком честно
/// отказывает «занято» на чужом файле. Две поверхности про один факт не
/// должны расходиться молча — это расхождение завела сама эта ветка.
/// Без цели (`set_lock_target` не звали) — занятости и спрашивать не у
/// кого, `false`.
pub fn file_lock_busy() -> bool {
    let target = LOCK_TARGET.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match target {
        None => false,
        Some(dir) => matches!(file_lock(&dir.join("project.lock")), Ok(None)),
    }
}

/// Взятый замок проекта: мьютекс в памяти и, если задана цель, файл на
/// диске. Держите до конца операции — ранний `drop` снимает оба раньше
/// срока. Поля объявлены файлом первым, мьютексом последним: Rust роняет их
/// в порядке объявления, и внутренний мьютекс обязан освободиться ПОЗЖЕ
/// внешнего файла (Minor 6 финального ревью: прежняя прозы утверждала
/// обратное — что внешние ворота открываются позже внутренних, хотя код
/// всегда делал наоборот). Будь порядок перевёрнут, сосед по этому же
/// процессу взял бы `LOCK` раньше, чем файл успел освободиться, упёрся бы в
/// ещё не снятый файл и получил бы ложное «другая операция».
pub struct Hold {
    #[allow(dead_code)]
    file: Option<FileLock>,
    #[allow(dead_code)]
    mem: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        DOING.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Берёт замок проекта: сперва мьютекс в памяти — быстрый путь и он же
/// ловит соседний поток этого же процесса раньше, чем дело дойдёт до файла;
/// затем, если `set_lock_target` звали, файл на диске — межпроцессная часть.
/// Без цели (разовые команды) замок остаётся в памяти, как и был до этой
/// задачи.
pub fn hold(what: &str) -> Result<Hold, String> {
    use std::sync::atomic::Ordering::Relaxed;
    let mem = match LOCK.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return Err(busy(doing())),
    };
    let target = LOCK_TARGET.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let file = match target {
        None => None,
        Some(dir) => match file_lock(&dir.join("project.lock")) {
            Ok(Some(g)) => Some(g),
            // Другой процесс держит файл — чей именно, отсюда не видно
            // (`doing()` знает только про этот процесс), поэтому «другая
            // операция»: готовая, не выдуманная вторая формулировка.
            Ok(None) => return Err(busy(doing())),
            Err(e) => return Err(e.to_string()),
        },
    };
    DOING.store(DOING_NAMES.iter().position(|n| *n == what).unwrap_or(1), Relaxed);
    Ok(Hold { mem, file })
}

/// Состояние проекта для окна: те же факты и под тем же замком, что отдаёт
/// агенту `project_status`. Под замком — чтобы во время выгрузки вкладка
/// говорила «занято», а не читала полузаписанный индекс; `Err` здесь и есть
/// эта строка, окно показывает её как есть.
pub fn facts_locked(p: &Project) -> Result<Facts, String> {
    let _h = hold("project_status")?;
    facts(p)
}

/// Вызов инструмента моста: `(текст, isError)`. Занято — ответ, не очередь.
pub fn call(p: &Project, tool: &str, args: &Value) -> (String, bool) {
    // A panic inside a tool poisons the mutex; the project is not broken by
    // that, so a poisoned lock is simply taken (see `hold`).
    let _h = match hold(tool) {
        Ok(h) => h,
        Err(e) => return (e, true),
    };
    let r = match tool {
        "project_status" => status(p),
        "project_pull" => pull(p, &NEVER),
        "project_push" => push(p, args.get("message").and_then(Value::as_str).unwrap_or(""), &NEVER),
        other => Err(format!("нет инструмента {other}")),
    };
    match r {
        Ok(t) => (t, false),
        Err(t) => (t, true),
    }
}

#[cfg(test)]
pub(crate) mod tempdir {
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

    /// Находка проверки безопасности 30.10: origin человека вправе носить
    /// токен, а мы печатаем этот адрес в журнал и отдаём агенту — то есть
    /// модели на той стороне. Форма `git@host:путь` паролей не носит и
    /// остаётся как есть.
    #[test]
    fn a_shown_origin_carries_no_credentials() {
        assert_eq!(without_credentials("https://bucher:ghp_SECRET@github.com/me/repo.git"), "https://github.com/me/repo.git");
        assert_eq!(without_credentials("https://github.com/me/repo.git"), "https://github.com/me/repo.git");
        assert_eq!(without_credentials("ssh://dev1@gitea.local:2222/dev1c/UNF.git"), "ssh://gitea.local:2222/dev1c/UNF.git");
        // Без пути — всё ещё адрес, и всё ещё без учётных данных.
        assert_eq!(without_credentials("https://token@host"), "https://host");
        // scp-подобная форма пароля не несёт: трогать её незачем.
        assert_eq!(without_credentials("git@github.com:me/repo.git"), "git@github.com:me/repo.git");
        // Ни в одном ответе не осталось самого секрета.
        for u in ["https://x:ghp_SECRET@h/p.git", "ssh://x:ghp_SECRET@h/p"] {
            assert!(!without_credentials(u).contains("ghp_SECRET"), "{u}");
        }
    }

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
        let p = Project {
            dir: dir.clone(),
            git: system_git(),
            branch: "dev2".into(),
            remote: origin.to_string_lossy().into(),
            login: "dev2".into(),
            local: false,
        };
        (tmp, p)
    }

    #[test]
    fn flags_carry_identity_and_auth_header_without_a_password_in_the_url() {
        let f = git_flags("dev2");
        assert!(f.iter().any(|a| a == "user.name=dev2"));
        assert!(f.iter().any(|a| a == "http.extraHeader=Authorization: Basic ZGV2MjpkZXYy"));
        assert!(f.iter().any(|a| a == "credential.helper="));
        assert!(f.iter().any(|a| a == "gc.auto=0"), "no auto-gc from the bridge: a 90k-file commit must not repack");
        let mut cfg = crate::cfg::filled_for_test();
        cfg.host.gitea = "http://192.168.1.10:3391".into();
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
            c.get_envs().any(|(k, v)| k.to_string_lossy().into_owned() == "GIT_TERMINAL_PROMPT"
                && v.map(|v| v.to_string_lossy().into_owned()) == Some("0".into())),
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

    /// Адрес среды задан полем — `origin` собирается от него. И у уже
    /// разложенного проекта расхождение адреса раскладка лечит, а не только
    /// называет: в локальном режиме прежний адрес указывает в пустоту.
    /// Адреса среды нет — `origin` не собирается из пустоты: раскладка это
    /// называет, а `pull`/`push`/`init` отказывают текстом про поле.
    #[test]
    fn without_a_gitea_address_no_origin_is_made_up() {
        let cfg = crate::cfg::filled_for_test();
        assert_eq!(cfg.gitea_url(), None);
        let (_t, mut p) = fixture();
        p.remote = Project::from_cfg(&cfg, Path::new("/tools")).remote;
        assert_eq!(p.remote, "");
        assert!(matches!(layout_state(&p), Layout::NoGitea));
        let e = init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_err();
        assert!(e.contains("адрес среды не задан") && e.contains("gitea"), "{e}");
        // Все четыре входа в git за одним заслоном — `status` в том числе:
        // через него идут вкладка «Проект» и инструмент агента.
        assert!(push(&p, "проба", &NEVER).unwrap_err().contains("адрес среды не задан"));
        assert!(pull(&p, &NEVER).unwrap_err().contains("адрес среды не задан"));
        assert!(status(&p).unwrap_err().contains("адрес среды не задан"));
        // А неразложенный каталог называет свою причину, а не адрес среды.
        let t = tempdir::Dir::new();
        p.dir = t.path().join("нет такого");
        std::fs::create_dir_all(&p.dir).unwrap();
        assert!(matches!(layout_state(&p), Layout::Missing));
    }

    /// Главное правило локального режима: `origin` принадлежит человеку.
    /// 1С-разработчик заводит его обычным git, куда ему нужно, и ни
    /// раскладка, ни состояние, ни фиксация не вправе его переписать — а
    /// переписывали: `ensure_origin` приводил `origin` к
    /// `<gitea>/dev1c/<имя>.git`, собранному из пустого поля (вопрос
    /// владельца 30.10). Заодно: ветка берётся из репозитория, а не из
    /// пустого рабочего места, и личность коммита — человека, не `dev1`.
    #[test]
    fn in_local_mode_a_foreign_origin_survives_everything_we_do() {
        let (t, mut p) = fixture();
        // Свой сервер человека — голый репозиторий рядом, а не наш
        // `origin.git` фикстуры: именно его адрес мы и не вправе трогать.
        let theirs = t.path().join("мой-сервер.git").to_string_lossy().to_string();
        git(t.path(), &["init", "-q", "--bare", "-b", "main", &theirs]);
        git(&p.dir, &["remote", "set-url", "origin", &theirs]);
        git(&p.dir, &["checkout", "-q", "-b", "feature/моя"]);
        p = Project { branch: "feature/моя".into(), remote: String::new(), login: String::new(), local: true, ..p };

        // Раскладка не требует ни адреса среды, ни «нужной» ветки.
        init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(layout_state(&p), Layout::Ok));
        assert_eq!(git(&p.dir, &["remote", "get-url", "origin"]), theirs, "чужой origin переписан раскладкой");

        // Состояние — без слов про Gitea и рабочее место, с адресом человека.
        let f = facts(&p).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(f.repo, theirs);
        assert!(f.want_branch.is_empty() && !f.exchange, "{f:?}");
        let text = status_text(&f);
        assert!(!text.contains("ветка рабочего места") && !text.contains("Gitea"), "{text}");
        assert!(text.contains("ветка feature/моя"), "{text}");

        // Фиксация: чужой origin цел, коммит подписан не нами. Личность —
        // как у человека на своей машине (у него она глобальная, у теста —
        // в самом репозитории): своей мы в этом режиме не навязываем.
        git(&p.dir, &["config", "user.email", "me@example.com"]);
        git(&p.dir, &["config", "user.name", "Я"]);
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();
        let out = push(&p, "моя правка", &NEVER).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&p.dir, &["remote", "get-url", "origin"]), theirs, "чужой origin переписан отправкой: {out}");
        assert_eq!(git(&p.dir, &["log", "-1", "--format=%s"]), "моя правка");
        assert!(
            !git(&p.dir, &["log", "-1", "--format=%ae"]).contains("dev1c.local"),
            "личность рабочего места навязана локальному коммиту"
        );
    }

    /// Тот же режим, но origin человек не заводил. Фиксация работает — это
    /// обычный локальный репозиторий, и правки базе уже доступны; обмен
    /// отказывает, и словами про origin, а не про «адрес среды не задан» и
    /// кнопку «Установить», которая этого поля не меняет.
    #[test]
    fn without_an_origin_a_local_project_still_commits_and_says_why_it_cannot_send() {
        let t = tempdir::Dir::new();
        let dir = t.path().join("Проект без origin");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let p = Project {
            dir: dir.clone(),
            git: system_git(),
            branch: "main".into(),
            remote: String::new(),
            login: String::new(),
            local: true,
        };
        init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(layout_state(&p), Layout::Ok), "локальная раскладка не требует origin");

        let f = facts(&p).unwrap_or_else(|e| panic!("{e}"));
        assert!(f.repo.is_empty() && f.ahead.is_empty(), "{f:?}");
        let text = status_text(&f);
        assert!(!text.contains("репозиторий") && !text.contains("неотправленных"), "{text}");

        // Личности у человека на машине может не быть вовсе — тогда отказ
        // наш, а не английский текст git про «tell me who you are».
        std::fs::write(dir.join("src").join("M.bsl"), "1").unwrap();
        let no_id = push(&p, "первая", &NEVER).unwrap_err();
        assert!(no_id.contains("git config --global user.name") && !no_id.contains("Author identity"), "{no_id}");
        git(&dir, &["config", "user.email", "me@example.com"]);
        git(&dir, &["config", "user.name", "Я"]);
        let out = push(&p, "первая", &NEVER).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&dir, &["log", "-1", "--format=%s"]), "первая", "фиксация обязана состояться: {out}");
        assert!(out.contains("git remote add origin") && !out.contains("адрес среды"), "{out}");
        let e = pull(&p, &NEVER).unwrap_err();
        assert!(e.contains("git remote add origin") && !e.contains("адрес среды"), "{e}");
    }

    /// Ветка и личность в локальном режиме — не из пустого рабочего места:
    /// иначе «нужной» веткой становилась пустая строка (`git init -b `,
    /// «ветка main, нужна »), а подписью коммита — `@dev1c.local`.
    #[test]
    fn a_local_project_takes_its_branch_from_the_repository_and_imposes_no_identity() {
        let mut cfg = crate::cfg::filled_for_test();
        cfg.host.mode = "local".into();
        cfg.host.workstation.clear();
        cfg.host.gitea = "http://localhost:3000".into();
        let t = tempdir::Dir::new();
        cfg.host.project = t.path().join("Бух").display().to_string();
        // Пустого репозитория ещё нет — ветка `main`, рабочее имя для `git init -b`.
        let p = Project::from_cfg(&cfg, Path::new("/tools"));
        assert_eq!(p.branch, "main");
        assert!(p.remote.is_empty(), "адрес среды в локальном режиме не собирается даже из заполненного поля");
        assert!(p.local && p.login.is_empty());
        // Репозиторий есть и стоит на своей ветке — берём её.
        std::fs::create_dir_all(&p.dir).unwrap();
        git(&p.dir, &["init", "-q", "-b", "рабочая"]);
        assert_eq!(Project::from_cfg(&cfg, Path::new("/tools")).branch, "рабочая");

        let f = git_flags("");
        assert!(!f.iter().any(|a| a.starts_with("user.name") || a.starts_with("http.extraHeader") || a == "credential.helper="), "{f:?}");
        assert!(f.iter().any(|a| a == "safe.directory=*") && f.iter().any(|a| a == "gc.auto=0"), "{f:?}");
    }

    #[test]
    fn the_remote_follows_the_explicit_gitea() {
        let mut cfg = crate::cfg::filled_for_test();
        cfg.host.gitea = "http://localhost:3000".into();
        assert_eq!(Project::from_cfg(&cfg, Path::new("/tools")).remote, "http://localhost:3000/dev1c/Buh.git");
        let (_t, mut p) = fixture();
        let old = p.remote.clone();
        p.remote = "http://localhost:3000/dev1c/project.git".into();
        assert!(matches!(layout_state(&p), Layout::Drifted(d) if d.contains("origin")), "расхождение адреса обязано называться");
        init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&p.dir, &["remote", "get-url", "origin"]), p.remote);
        assert_ne!(p.remote, old);
    }

    /// Файл анализатора уезжает вместе с выгрузкой: он нужен второй стороне,
    /// и корень проекта у агента обязан быть пригодным сам, без его действий.
    /// Отправка на неразложенном проекте объясняет причину, а не пересказывает
    /// внутренности git. Живой случай 2026-10-21: выгрузка прошла, а человек
    /// прочитал «не найден git репозиторий … GIT_DISCOVERY_ACROSS_FILESYSTEM».
    #[test]
    fn push_on_an_unprepared_project_names_the_cause() {
        let t = tempdir::Dir::new();
        let dir = t.path().join("project");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let p = Project {
            dir: dir.clone(),
            git: system_git(),
            branch: "dev1".into(),
            remote: "http://localhost:3000/dev1c/project.git".into(),
            login: "dev1".into(),
            local: false,
        };
        let e = push(&p, "выгрузка", &AtomicBool::new(false)).unwrap_err();
        assert!(e.contains("не разложен") && e.contains("install"), "{e}");
        assert!(!e.contains("GIT_DISCOVERY"), "сырой git наружу не идёт: {e}");
        // Та же причина и у приёма ветки.
        let e = pull(&p, &AtomicBool::new(false)).unwrap_err();
        assert!(e.contains("не разложен"), "{e}");
    }

    #[test]
    fn the_analyzer_file_goes_into_the_commit() {
        let (_t, p) = fixture();
        let want = crate::analyzer::expected("8.5.1.1150", "8.3.27");
        init(&p, Path::new("nowhere"), &want, &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(p.dir.join("src")).unwrap();
        std::fs::write(p.dir.join("src").join("ConfigDumpInfo.xml"), "<ConfigDumpInfo/>").unwrap();
        push(&p, "выгрузка", &AtomicBool::new(false)).unwrap_or_else(|e| panic!("{e}"));
        let tracked = git(&p.dir, &["ls-files"]);
        assert!(
            tracked.contains(crate::analyzer::FILE),
            "файл анализатора обязан быть в коммите:
{tracked}"
        );
    }

    /// Строка платформы для анализатора появляется при раскладке — и только
    /// когда её передали: платформы нет, файла нет.
    #[test]
    fn init_writes_the_analyzer_lines_it_is_given() {
        let (_t, p) = fixture();
        init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        assert!(!p.dir.join(crate::analyzer::FILE).exists(), "без версии платформы файла быть не должно");
        let want = crate::analyzer::expected("8.5.1.1150", "8.3.27");
        init(&p, Path::new("nowhere"), &want, &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(crate::analyzer::found(&p.dir), want);
    }

    #[test]
    fn init_lays_out_a_project_with_a_cyrillic_path() {
        let t = tempdir::Dir::new();
        let origin = t.path().join("o.git");
        git(t.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        let dir = t.path().join("Проект Бух");
        let p = Project {
            dir: dir.clone(),
            git: system_git(),
            branch: "dev3".into(),
            remote: origin.to_string_lossy().into(),
            login: "dev3".into(),
            local: false,
        };
        assert!(matches!(layout_state(&p), Layout::Missing));
        let lines = std::cell::RefCell::new(Vec::new());
        init(&p, Path::new("nowhere"), &[], &|s| lines.borrow_mut().push(s)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), "dev3");
        assert_eq!(git(&dir, &["remote", "get-url", "origin"]), p.remote);
        assert_eq!(std::fs::read_to_string(dir.join(".gitattributes")).unwrap(), GITATTRIBUTES);
        assert!(std::fs::read_to_string(dir.join("autumn-properties.json")).unwrap().contains("\"increment\": true"));
        assert!(dir.join("src").is_dir());
        assert!(matches!(layout_state(&p), Layout::Ok));
        assert!(lines.borrow().iter().any(|l| l.contains("git init")), "{:?}", lines.borrow());
        // A second init changes nothing and keeps a hand-edited file.
        std::fs::write(dir.join(".gitignore"), "mine\n").unwrap();
        init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap();
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
        init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap();
        assert_eq!(git(&p.dir, &["symbolic-ref", "--short", "HEAD"]), "dev2");
        assert_eq!(git(&p.dir, &["log", "-1", "--format=%s"]), "work on dev2");
    }

    #[test]
    fn a_base_directory_is_refused_and_drift_is_named() {
        let t = tempdir::Dir::new();
        let dir = t.path().join("base");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1Cv8.1CD"), "").unwrap();
        let p =
            Project { dir: dir.clone(), git: system_git(), branch: "dev1".into(), remote: "x".into(), login: "dev1".into(), local: false };
        assert!(matches!(layout_state(&p), Layout::BaseDir(_)));
        assert!(init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_err().contains("каталог базы"));
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
        let r = retry_once(std::time::Duration::from_millis(1), &AtomicBool::new(false), || {
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
        let r = retry_once(std::time::Duration::from_millis(1), &AtomicBool::new(false), || {
            calls.set(calls.get() + 1);
            Err::<String, _>("сервер не отдал все нужные объекты".to_string())
        });
        assert_eq!(r, Err("сервер не отдал все нужные объекты".to_string()));
        assert_eq!(calls.get(), 2, "ровно одна попытка и ровно один повтор — не больше");
    }

    #[test]
    fn retry_once_does_not_pause_or_retry_a_cancelled_attempt() {
        // Отмена, взведённая до или во время первой попытки: вторая попытка
        // и, главное, пауза перед ней не должны случиться вовсе — иначе
        // отмена стоит человеку лишних двух секунд ожидания перед «прервано».
        let calls = std::cell::Cell::new(0u32);
        let cancel = AtomicBool::new(true);
        let started = std::time::Instant::now();
        let r = retry_once(std::time::Duration::from_secs(2), &cancel, || {
            calls.set(calls.get() + 1);
            Err::<String, _>("прервано".to_string())
        });
        assert_eq!(r, Err("прервано".to_string()));
        assert_eq!(calls.get(), 1, "отмена не должна приводить к повтору");
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "отмена не должна платить паузу повтора");
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

    #[test]
    fn a_second_process_is_refused_not_queued() {
        // Два захвата по одному пути — то же, что два процесса: flock держится
        // за открытый дескриптор, именованный мьютекс — за объект ядра.
        let d = tempdir::Dir::new();
        let first = file_lock(&d.path().join("project.lock")).unwrap().unwrap();
        assert!(file_lock(&d.path().join("project.lock")).unwrap().is_none());
        drop(first);
        assert!(file_lock(&d.path().join("project.lock")).unwrap().is_some());
    }

    #[test]
    fn a_busy_lock_names_what_is_running() {
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempdir::Dir::new();
        set_lock_target(d.path());
        let g = hold("project_push").unwrap();
        assert_eq!(hold("project_status").err(), Some("занято: идёт project_push".to_string()));
        drop(g);
        // Цель — глобал на процесс: следующий тест её не должен застать —
        // ставит её только сам (`without_a_target_the_lock_is_in_memory_only`).
        *LOCK_TARGET.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    #[test]
    fn without_a_target_the_lock_is_in_memory_only() {
        // Разовые команды (`status`, `example`) цели не ставят: отказа быть не должно.
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _g = hold("project_status").unwrap();
    }
}
