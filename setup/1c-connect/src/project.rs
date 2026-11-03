//! Git-инструменты моста: состояние, pull, push — одним набором флагов, из
//! MinGit комплекта. Пароль не в URL, а в заголовке; вывод проходит затирку.
//! Мьютекс проекта: второй вызов получает «занято», не очередь.

use crate::cfg::Cfg;
use crate::single_instance;
use serde_json::Value;
use std::collections::HashMap;
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
    /// `origin` есть, и это не наш адрес: человек завёл его сам обычным git.
    /// Такой мы не переписываем — ни раскладкой, ни обменом, — а называем
    /// словами (правило 30.10: чужим мы не распоряжаемся). Внутри — адрес как
    /// он есть, уже без учётных данных: эта строка идёт на поверхности.
    ForeignOrigin(String),
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
    // ветки нет вовсе (`p.branch` пуст: ветка принадлежит человеку), а origin
    // человека сравнивать не с чем: «origin X, нужен » было бы требованием
    // привязаться к среде, которой нет.
    if !p.local {
        // После `.git`, а не до: у неразложенного каталога первопричина — что
        // он не разложен, и подменять её адресом среды нельзя (находка ревью
        // задачи 35). Совет в обоих случаях один — `install`.
        if p.remote.is_empty() {
            return Layout::NoGitea;
        }
        let branch = head_branch(p).unwrap_or_default();
        if branch != p.branch {
            return Layout::Drifted(format!("ветка {branch}, нужна {}", p.branch));
        }
        // Нет origin вовсе — расхождение, которое раскладка и лечит
        // (`remote add`). Есть, но чужой — не расхождение, а чужая настройка:
        // своё имя у неё и свои слова, потому что `install` её не правит.
        match origin_url(p) {
            Err(e) => return Layout::Drifted(e),
            Ok(None) => return Layout::Drifted("нет origin".into()),
            Ok(Some(u)) if u != p.remote => return Layout::ForeignOrigin(without_credentials(&u)),
            Ok(Some(_)) => {}
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
        must(p, &["init", "-q", "-b", init_branch(p)])?;
        log(format!("git init, ветка {}", init_branch(p)));
    } else if !p.local {
        // Ветку переключаем только там, где она наша: в туннельном режиме это
        // логин рабочего места, конвенция среды. В локальном она принадлежит
        // человеку, и «Установить» ему её не меняет — ни `checkout`, ни
        // `checkout -b`. До 30.10 меняло: ветка читалась из `.git/HEAD`
        // файлом, в ворктри и при отделённом HEAD выходило `main`, и человек,
        // нажавший «Установить» из своей ветки, оказывался на другой либо
        // получал отказ checkout поверх своих правок (находка ревью 30.10).
        let branch = head_branch(p).unwrap_or_default();
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
    /// Ветка, которую мы НАВЯЗЫВАЕМ: логин рабочего места в туннельном режиме
    /// — конвенция среды, и `init` к ней приводит репозиторий. В локальном
    /// режиме пусто: ветка там принадлежит человеку, мы её не создаём, не
    /// переключаем и не подменяем, а узнаём у git тогда, когда она нужна
    /// (`work_branch`). Единственное место, где имя всё-таки нужно нам, —
    /// `git init -b` на пустом каталоге (`init_branch`).
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
        let dir = PathBuf::from(&cfg.current().project);
        // The last path segment by either separator: a Windows path read on
        // Linux (CI) has no separators as far as PathBuf is concerned.
        let name = cfg
            .current()
            .project
            .trim_end_matches(['\\', '/'])
            .rsplit(['\\', '/'])
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or("project")
            .to_string();
        let git = if cfg!(windows) { tools.join("git").join("cmd").join("git.exe") } else { tools.join("git").join("bin").join("git") };
        let local = !crate::cfg::mode_needs_env(&cfg.current().mode);
        // Адреса среды нет — `remote` пуст, а не собран из пустоты: читатели
        // (`layout_state`, `ensure_origin`) говорят, чего не хватает. В
        // локальном режиме его не собирают вовсе: организации `dev1c` в чужой
        // Gitea не существует, а `origin` там принадлежит человеку.
        let remote = if local { String::new() } else { cfg.gitea_url().map(|g| format!("{g}/dev1c/{name}.git")).unwrap_or_default() };
        // Навязанной ветки в локальном режиме нет вовсе — ни из пустого
        // рабочего места, ни прочитанной заранее: кто на какой ветке стоит,
        // знает git, и спрашивать его надо в тот момент, когда ветка нужна.
        let branch = if local { String::new() } else { cfg.current().workstation.clone() };
        let login = if local { String::new() } else { cfg.current().workstation.clone() };
        Project { dir, git, branch, remote, login, local }
    }
}

/// Имя первой ветки нового репозитория: в туннельном режиме ветка рабочего
/// места (конвенция среды), в локальном — `main`. Это единственное место, где
/// имя ветки придумываем мы: на пустом каталоге её ещё нет ни у кого, а
/// `git init` без `-b` взял бы `init.defaultBranch` человека — что тоже
/// годится, но тогда план установки не смог бы назвать имя заранее. Одно
/// знание на `init` и на строку плана (`ops_host::ProjectOp::plan`).
pub fn init_branch(p: &Project) -> &str {
    if p.branch.is_empty() {
        "main"
    } else {
        p.branch.as_str()
    }
}

/// Отделённый HEAD — это состояние, а не ветка, и называть его надо им самим:
/// выдуманное имя («main») уводило отправку и приём не туда, куда человек
/// смотрит (находка ревью 30.10).
const DETACHED_STATE: &str = "HEAD отделён от ветки";

/// Что с этим делать — тем же текстом у всех, кто про это говорит.
const DETACHED_FIX: &str = "вернитесь на ветку обычным git (git switch <ветка>)";

/// Куда смотрит HEAD прямо сейчас — так, как это знает сам git, а не по
/// чтению `.git/HEAD` файлом: в ворктри `.git` вообще файл, а при отделённом
/// HEAD никакой ссылки там нет, и оба случая читались как `main`. `None` —
/// ветки нет (HEAD отделён) либо спросить не у кого (нет репозитория).
fn head_branch(p: &Project) -> Option<String> {
    let o = git(p, &["symbolic-ref", "--short", "HEAD"]).ok()?;
    let b = o.text.trim().to_string();
    (o.ok && !b.is_empty()).then_some(b)
}

/// Ветка, с которой идёт работа: в туннельном режиме — ветка рабочего места
/// (отправка идёт в неё, такова конвенция среды), в локальном — та, на
/// которой человек стоит. Отделённый HEAD в локальном режиме — отказ словами:
/// фиксировать в него можно, но отправлять и принимать нечего, а имя
/// выдумывать нельзя.
fn work_branch(p: &Project) -> Result<String, String> {
    if !p.local {
        return Ok(p.branch.clone());
    }
    head_branch(p).ok_or_else(|| format!("{DETACHED_STATE} — {DETACHED_FIX}"))
}

/// Чем назвать ветку человеку там, где поверхность показывает состояние, а не
/// действует: факт «проект», состояние шага установки, окно. Одно имя на все
/// поверхности — до 30.10 факт и окно показывали `main`, а карточка состояния
/// настоящую ветку, и два места о том же говорили разное.
pub fn branch_label(p: &Project) -> String {
    work_branch(p).unwrap_or_else(|_| DETACHED_STATE.to_string())
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
    match origin_url(p)? {
        Some(u) if u == p.remote => Ok(()),
        // Адрес есть, и он не наш. Переписать его `set-url` значит стереть
        // чужую настройку без следа: человек поработал в локальном режиме,
        // завёл origin сам — свой GitHub, свою Gitea, корпоративный сервер, —
        // переключился в туннельный, и первый же `install`/`push` молча увёл
        // отправку в среду (находка ревью 30.10). Говорим словами и не делаем;
        // работать дальше мы и не можем — `push origin` ушёл бы по ЧУЖОМУ
        // адресу, то есть выгрузка человека уехала бы не туда.
        Some(u) => Err(format!("{} — {}", foreign_origin(&u), foreign_origin_fix(&p.remote))),
        None => must(p, &["remote", "add", "origin", p.remote.as_str()]).map(|_| ()),
    }
}

/// Чужой `origin` — что увидели. Адрес без учётных данных: строка идёт в
/// журнал окна, в факт и в ответ агенту.
pub fn foreign_origin(origin: &str) -> String {
    format!("origin указывает на {}, а не на репозиторий среды", without_credentials(origin))
}

/// …и что сделать человеку. Чужой адрес мы не меняем даже «для его же
/// удобства»: решает он, а не мы.
pub fn foreign_origin_fix(ours: &str) -> String {
    format!("этот адрес мы не меняем: смените его сами (git remote set-url origin {ours}) или уберите origin (git remote remove origin), и программа заведёт его заново")
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

/// Есть ли у репозитория origin и какой: `None` — не заведён вовсе.
fn origin_url(p: &Project) -> Result<Option<String>, String> {
    let o = git(p, &["remote", "get-url", "origin"])?;
    Ok((o.ok && !o.text.trim().is_empty()).then(|| o.text.trim().to_string()))
}

/// Адрес без учётных данных — для показа, не для вызова git: берём общую
/// чистку журнала (`common::docker::without_credentials`), а не заводим
/// вторую такую же здесь. Человек вправе держать в origin токен
/// (`https://имя:токен@host/путь.git`), а этот адрес мы печатаем в журнал
/// окна и отдаём инструментом агенту — то есть модели на той стороне.
use common::docker::without_credentials;

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
    /// Куда смотрит HEAD прямо сейчас — так, как это знает git. Отделённый
    /// HEAD называет сам себя (`DETACHED_STATE`): это состояние, а не ветка, и
    /// имени ему мы не выдумываем.
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
    let branch = head_branch(p).unwrap_or_else(|| DETACHED_STATE.to_string());
    let last = must(p, &["log", "-1", "--format=%h %s (%cr)"]).unwrap_or_default();
    // Куда отправлять: в туннельном режиме это наш адрес и наша организация в
    // Gitea среды, в локальном — то, что человек завёл сам; не завёл —
    // отправлять некуда, и считать неотправленное незачем.
    // Адрес показываем без учётных данных: эта строка уходит и в журнал, и
    // агенту (`without_credentials`).
    let repo = if p.local { origin_url(p)?.map(|u| without_credentials(&u)).unwrap_or_default() } else { repo_name(p) };
    // Считаем от той ветки, с которой идёт работа, а не от навязанной:
    // отправлять некуда (нет origin) или не от чего (HEAD отделён) — считать
    // нечего, и пустая строка это говорит (`Facts::ahead`).
    let ahead = match work_branch(p) {
        Ok(b) if !repo.is_empty() => {
            git(p, &["rev-list", "--count", &format!("origin/{b}..HEAD")]).map(|o| if o.ok { o.text } else { "все".into() })?
        }
        _ => String::new(),
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
    // Отделённый HEAD называет сам себя: «ветка HEAD отделён от ветки» —
    // бессмыслица, а состояние назвать всё равно надо.
    let mut lines = vec![if f.branch == DETACHED_STATE { DETACHED_STATE.to_string() } else { format!("ветка {}", f.branch) }];
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
    // Перебазируем ту ветку, на которой человек стоит, а не `main`: до 30.10
    // в локальном режиме `pull --rebase origin main` уводил его работу на
    // чужую ветку (находка ревью 30.10).
    let branch = work_branch(p)?;
    // A fresh project has no branch on the server yet — that comes first:
    // its tree is "dirty" too, and the useful answer is "export first".
    if !remote_has(p, &branch)? {
        return Err(format!(
            "ветки {branch} {} ещё нет — сначала «Выгрузить из 1С» (cf_decompile, затем project_push)",
            if p.local { "у origin" } else { "в Gitea" }
        ));
    }
    if !must(p, &["status", "--porcelain"])?.is_empty() {
        return Err("в каталоге проекта есть незакоммиченные изменения — сначала project_push".into());
    }
    let attempt = || -> Result<String, String> {
        let o = git_wait(p, &["pull", "--rebase", "origin", &branch], cancel)?;
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
                    branch
                ));
            }
            Err(e)
        }
    }
}

pub fn push(p: &Project, message: &str, cancel: &AtomicBool) -> Result<String, String> {
    ensure_origin(p)?;
    // Ветку спрашиваем ДО того, как что-то добавить в индекс: при отделённом
    // HEAD коммит лёг бы в никуда (достать его потом только из reflog), а
    // отправлять было бы нечего. Отказ словами, и чужой индекс не тронут.
    let branch = work_branch(p)?;
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
    // Один и тот же pathspec на все три команды: что добавили, то и
    // смотрим, то и фиксируем. Без него `commit` брал ВЕСЬ индекс, и в
    // локальном режиме, где репозиторий принадлежит человеку, в наш коммит с
    // нашим сообщением про выгрузку уезжало то, что он приготовил к своему
    // (находка ревью 30.10). `--` отделяет пути от ревизий.
    let ours: Vec<&str> = std::iter::once("--").chain(roots).collect();
    must(p, &[&["add"][..], &ours].concat())?;
    let staged = !must(p, &[&["diff", "--cached", "--name-only"][..], &ours].concat())?.is_empty();
    let has_commits = git(p, &["rev-parse", "--verify", "HEAD"])?.ok;
    // Пустой первый коммит заводит ветку: без него в туннельном режиме первому
    // push нечего отправлять, а репозиторий в Gitea создаёт именно он. Такой
    // коммит идёт БЕЗ pathspec — пустой путь git в самый первый коммит взять
    // не умеет («pathspec did not match any file(s) known to git»), — и
    // потому только на пустом индексе: приготовленное человеком в него не
    // уедет, а в туннельном режиме репозиторий наш и готовить там некому.
    let empty_first = !staged && !has_commits && must(p, &["diff", "--cached", "--name-only"])?.is_empty();
    if staged || empty_first {
        identity_ready(p)?;
        let msg = if message.trim().is_empty() { "выгрузка" } else { message };
        let commit: Vec<&str> =
            if empty_first { vec!["commit", "-q", "--allow-empty", "-m", msg] } else { [&["commit", "-q", "-m", msg][..], &ours].concat() };
        must(p, &commit)?;
    }
    // Фиксация сделана — дальше обмен, и он бывает не всегда. В локальном
    // режиме отправлять некуда, пока человек не завёл origin сам, но работа
    // при этом не потеряна и базе уже доступна: `cf_load` читает те самые
    // файлы. Поэтому не отказ после сделанного коммита, а сделанное и
    // причина, по которой вторая половина не состоялась (решение владельца
    // 30.10: фиксация в локальном режиме работает всегда, без всякого
    // origin).
    let origin = if p.local { origin_url(p)? } else { Some(p.remote.clone()) };
    // Что именно случилось — то и говорим: пустой первый коммит мы ДЕЛАЕМ, и
    // «нечего коммитить» про него было неправдой (находка ревью 30.10).
    let committed = if staged {
        "зафиксировано"
    } else if empty_first {
        "зафиксирован пустой первый коммит"
    } else {
        "нечего коммитить"
    };
    let Some(origin) = origin else {
        return Ok(format!("{committed}: ветка {branch} — {NO_ORIGIN}"));
    };
    // Репозиторий в Gitea среды создаёт первый push, и вместе с ним туда
    // уезжает `main`: такова конвенция среды (`push-to-create` в организации
    // `dev1c`). К чужому серверу человека она не относится — там репозиторий
    // уже есть, а что делать с его `main`, решает он сам.
    let first = !p.local && !remote_has(p, "main")?;
    if first {
        must_wait(p, &["push", "-q", "origin", "HEAD:refs/heads/main"], cancel)?;
    }
    must_wait(p, &["push", "-q", "-u", "origin", &branch], cancel)?;
    Ok(format!(
        "{committed}, ветка {branch} отправлена в {}{}",
        without_credentials(&origin),
        if first { " (репозиторий создан, main тоже отправлен)" } else { "" }
    ))
}

/// Флаг отмены для CLI и моста: его никто не взводит — окно (Task 4) заведёт свой.
pub static NEVER: AtomicBool = AtomicBool::new(false);

/// Ключ замка папки: нормализованный путь (`cfg::normalize_base` — тот же
/// нормализатор, что держит запрет «одна база — одна карточка», второй
/// копии правила в крейте быть не должно), очищенный до безопасных для
/// имени файла и именованного мьютекса символов (`single_instance::clean`,
/// им уже строятся ключи замка рабочего места). Разное написание одной и
/// той же папки даёт один ключ, а разные папки — разные: отсюда «ключ
/// замка считается от папки проекта» (задача 7 плана «Модель соединений»).
pub(crate) fn lock_key(dir: &Path) -> String {
    single_instance::clean(&crate::cfg::normalize_base(&dir.to_string_lossy()))
}

/// Путь файла замка этой папки внутри каталога состояния — ОДНО место, где
/// это имя строится: круг починки задачи 7, находка 2 — `hold` и
/// `file_lock_busy` строили его по месту каждый сам, и расхождение сделало
/// бы страж (`file_lock_busy`) слепым к объекту, который держит `hold`, а
/// замок выглядел бы работающим, ничего не запирая. `single_instance.rs`
/// уже требует того же для своих собственных ключей (там, где он сам
/// называет этот урок, строка 132) — второго места в крейте, где строится
/// это конкретное имя, быть не должно.
pub(crate) fn lock_file_path(state: &Path, dir: &Path) -> PathBuf {
    state.join(format!("project-{}.lock", lock_key(dir)))
}

/// Какие папки заняты прямо сейчас В ЭТОМ ПРОЦЕССЕ, и чем — ключ (`lock_key`)
/// на имя операции; пустая строка — папка зарезервирована, но имя ещё не
/// записано (см. `hold`). Замена одному `LOCK: Mutex<()>` на машину —
/// решение владельца 6 (план «Модель соединений», задача 7): замок на
/// машину значил, что работа по одному заказчику отказывала четырём
/// остальным, а внутрипроцессный «один на машину» для нескольких папок —
/// та же болезнь, что лечим снаружи, только внутри процесса.
static BUSY: std::sync::LazyLock<Mutex<HashMap<String, String>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Тестовый серилизатор: cargo гоняет тесты в одном бинарнике параллельно,
/// а `BUSY`/`LOCK_TARGET` — глобалы на процесс. Берётся первым во всех
/// тестах этого файла и `flow.rs`, что держат или оспаривают замок папки
/// либо делят общую `LOCK_TARGET`, — иначе сосед получает «занято» и падает
/// на `.unwrap()`.
#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());

/// Что сейчас идёт в этой папке под замком (пусто — ничего). Ключ тот же,
/// что строит и берёт сам замок (`lock_key`) — иначе имя могло бы разойтись
/// с тем, что держит занятость.
pub fn doing(dir: &Path) -> String {
    BUSY.lock().unwrap_or_else(|e| e.into_inner()).get(&lock_key(dir)).cloned().unwrap_or_default()
}

/// Занята ли эта папка прямо сейчас в этом процессе — быстрая проба без
/// удержания, как раньше отвечал `LOCK.try_lock()`. Нужна `verify::busy_now`
/// отдельно от файла: до файла дело может и не дойти (разовые команды без
/// цели, `LOCK_TARGET` не задана), а до памяти — всегда.
pub fn folder_busy(dir: &Path) -> bool {
    BUSY.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&lock_key(dir))
}

/// Готовая строка отказа — одна формулировка на все пути: мьютекс в памяти,
/// файл на диске, агент моста.
fn busy(what: &str) -> String {
    format!("занято: идёт {}", if what.is_empty() { "другая операция" } else { what })
}

/// Каталог, в котором `hold` держит файлы замка — один на папку проекта
/// (`<dir>/project-<ключ>.lock`, `lock_key`), а не один на всех: иначе
/// работа по одной папке снова отказывала бы четырём остальным. Сам каталог
/// общий — состояние рабочего места (`Kit::state`), выставляется один раз
/// при старте долгоживущего процесса (окна, `serve`, `connect`, будущей
/// подкоманды `mcp`). Разовые команды (`status`, `example`, `check`) её не
/// зовут: межпроцессный замок им не нужен (спека, «программа не
/// задерживает обычную работу человека»), а в памяти их и так разделяет
/// `BUSY` по ключу папки.
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
/// вынесенным туда, чтобы его звали оба замка. Путь — свой у каждой папки
/// проекта (`<state>/project-<ключ>.lock`), не каталог состояния целиком: тот
/// уже держит замок рабочего места, и окно заперло бы само себя.
pub type FileLock = single_instance::Guard;

/// `None` — файл уже занят другим процессом.
pub fn file_lock(path: &Path) -> std::io::Result<Option<FileLock>> {
    single_instance::file_lock(path)
}

/// Держит ли межпроцессную часть замка этой папки СЕЙЧАС какой-то другой
/// процесс — как бы спросил `hold`, но без удержания: взялся — значит
/// свободен, и отпускаем сразу же. Важно 4 финального ревью задачи 2:
/// `verify::busy_now` знал только про мьютекс в памяти, своего же процесса —
/// с тех пор, как замок стал межпроцессным, это расходилось с
/// `project::facts_locked`, которая под тем же замком честно отказывает
/// «занято» на чужом файле. Две поверхности про один факт не должны
/// расходиться молча. Без цели (`set_lock_target` не звали) — занятости и
/// спрашивать не у кого, `false`.
pub fn file_lock_busy(dir: &Path) -> bool {
    let target = LOCK_TARGET.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match target {
        None => false,
        Some(state) => matches!(file_lock(&lock_file_path(&state, dir)), Ok(None)),
    }
}

/// Взятый замок проекта: мьютекс в памяти (`BUSY`, по ключу папки) и, если
/// задана цель, файл на диске. Держите до конца операции — ранний `drop`
/// снимает оба раньше срока.
///
/// `Drop` снимает файл ПЕРВЫМ и только потом убирает запись из `BUSY` — явно
/// в теле, а не порядком полей (прежняя версия полагалась на порядок
/// объявления, и прозу под ней правили дважды, Minor 6 финального ревью
/// задачи 2 — порядок стоит называть там, где он на самом деле решается).
/// Будь порядок перевёрнут, сосед по этому же процессу и той же папке увидел
/// бы её свободной в `BUSY` раньше, чем файл успел освободиться, упёрся бы в
/// ещё не снятый файл и получил бы ложное «другая операция».
pub struct Hold {
    file: Option<FileLock>,
    key: String,
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.file = None;
        BUSY.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
    }
}

/// Берёт замок проекта: сперва место в `BUSY` по ключу папки — быстрый путь,
/// и он же ловит соседний поток этого же процесса НА ТОЙ ЖЕ ПАПКЕ раньше,
/// чем дело дойдёт до файла, а другую папку не трогает вовсе (решение
/// владельца 6); затем, если `set_lock_target` звали, файл на диске —
/// межпроцессная часть, тоже по ключу папки. Без цели (разовые команды)
/// замок остаётся в памяти, как и был до этой задачи.
pub fn hold(what: &str, dir: &Path) -> Result<Hold, String> {
    let key = lock_key(dir);
    {
        let mut busy_map = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(doing) = busy_map.get(&key) {
            return Err(busy(doing));
        }
        // Резерв места сразу — имя пишется ниже, только когда оба слоя
        // взяты (как и раньше: до этого момента соперник видит папку занятой,
        // но не читает имя чужой операции как своё).
        busy_map.insert(key.clone(), String::new());
    }
    let target = LOCK_TARGET.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let file = match target {
        None => None,
        Some(state) => match file_lock(&lock_file_path(&state, dir)) {
            Ok(Some(g)) => Some(g),
            // Другой процесс держит файл — чей именно, отсюда не видно,
            // поэтому «другая операция»: готовая, не выдуманная формулировка.
            Ok(None) => {
                BUSY.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
                return Err(busy(""));
            }
            Err(e) => {
                BUSY.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
                return Err(e.to_string());
            }
        },
    };
    BUSY.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), what.to_string());
    Ok(Hold { file, key })
}

/// Состояние проекта для окна: те же факты и под тем же замком, что отдаёт
/// агенту `project_status`. Под замком — чтобы во время выгрузки вкладка
/// говорила «занято», а не читала полузаписанный индекс; `Err` здесь и есть
/// эта строка, окно показывает её как есть.
pub fn facts_locked(p: &Project) -> Result<Facts, String> {
    // Замок берём БЕЗ имени операции, в отличие от всех прочих мест. Имя
    // видит не только соперник за замок, но и набор готовности
    // (`verify::busy_reason` читает `doing()`), а он по непустому имени
    // перестаёт спрашивать базу и говорит «занято: на машине идёт …». Это
    // чтение: оно ничего не меняет, и прятать за ним пробу публикации
    // незачем. Пока имя стояло, собственное обновление вкладки «Инструменты»
    // красило весь набор в «Занято» и объявляло человеку, что на машине идёт
    // `project_status`, — хотя тот ничего не звал и агента рядом не было
    // (наблюдение владельца 30.10: «я только включил, какой project_status?»).
    // Соперник за замок по-прежнему получает отказ, просто «другая операция»,
    // и это правда: имени у чтения нет.
    let _h = hold_read(&p.dir)?;
    facts(p)
}

/// Замок на время чтения — отдельной функцией, а не `hold("", dir)` по
/// месту: правило «чтение себя операцией не объявляет» держится тогда
/// сигнатурой, а не внимательностью следующего читателя.
fn hold_read(dir: &Path) -> Result<Hold, String> {
    hold("", dir)
}

/// Вызов инструмента моста: `(текст, isError)`. Занято — ответ, не очередь.
pub fn call(p: &Project, tool: &str, args: &Value) -> (String, bool) {
    // A panic inside a tool poisons the mutex; the project is not broken by
    // that, so a poisoned lock is simply taken (see `hold`).
    let _h = match hold(tool, &p.dir) {
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

    /// Увести git в свой домик: глобальный и системный конфиг — в файлы,
    /// которых нет. Без этого тесты про личность коммита проверяли бы не наш
    /// код, а настройки машины: `identity_ready` читает `user.email` ИЗ ВСЕХ
    /// областей, и на машине, где личность задана глобально (у разработчика и
    /// в CI), отказ `NO_IDENTITY` не состоялся бы вовсе — тест зеленел только
    /// здесь, где глобальная личность пуста (находка ревью 30.10).
    ///
    /// Переменные процесса, не теста: соседи по бинарнику их тоже увидят, но
    /// им это безразлично — каждый задаёт личность сам, флагами `-c` или
    /// конфигом своего репозитория.
    fn git_in_its_own_home() {
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev1c-no-such-gitconfig");
        std::env::set_var("GIT_CONFIG_SYSTEM", "/dev1c-no-such-gitconfig");
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = Command::new(system_git())
            // `core.quotepath=false` — как в `git_flags`: иначе кириллица в
            // именах файлов возвращается тесту в октальных escape-ах.
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "safe.directory=*", "-c", "core.quotepath=false"])
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
        cfg.current_mut().gitea = "http://192.168.1.10:3391".into();
        let p = Project::from_cfg(&cfg, Path::new("/tools"));
        assert_eq!(p.remote, "http://192.168.1.10:3391/dev1c/Buh.git");
        cfg.current_mut().project = "/home/dev/proj/Бух/".into();
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
        // Адрес меняем и в самом репозитории: привести к нему чужой `origin`
        // мы больше не вправе, и без этого отказ был бы про чужой адрес.
        git(&p.dir, &["remote", "set-url", "origin", &p.remote]);
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
        git(&p.dir, &["remote", "set-url", "origin", &p.remote]);
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
            let _g = BUSY.lock().unwrap();
            panic!("tool blew up while holding the lock");
        })
        .join();
        let (text, err) = call(&p, "project_status", &Value::Null);
        assert!(!err, "{text}");
    }

    /// Смена режима не распоряжается чужим адресом. Сценарий ревью 30.10:
    /// человек поработал в локальном режиме, завёл origin сам — свой GitHub,
    /// своя Gitea, корпоративный сервер, — потом переключился в туннельный, и
    /// первый же `install`/`push` делал `remote set-url origin <gitea>/dev1c/…`.
    /// Адрес, который он завёл сам, исчезал без следа, а выгрузка уезжала не
    /// туда, куда он смотрел. Теперь — слова и ни одного действия.
    ///
    /// Своего origin нет вовсе (каталог git-инициализирован руками) — это не
    /// чужая настройка, и его мы по-прежнему заводим сами.
    #[test]
    fn a_foreign_origin_is_not_replaced_when_the_mode_changes() {
        let (t, mut p) = fixture();
        let theirs = t.path().join("мой-сервер.git").to_string_lossy().to_string();
        git(t.path(), &["init", "-q", "--bare", "-b", "main", &theirs]);
        git(&p.dir, &["remote", "set-url", "origin", &theirs]);
        let ours = t.path().join("среда.git");
        git(t.path(), &["init", "-q", "--bare", "-b", "main", ours.to_str().unwrap()]);
        p.remote = ours.to_string_lossy().into();
        std::fs::write(p.dir.join("src").join("M.bsl"), "1").unwrap();

        // Раскладка, состояние и отправка — все три говорят одно и то же и
        // ничего не делают.
        assert!(matches!(layout_state(&p), Layout::ForeignOrigin(u) if u == theirs));
        for e in [
            init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_err(),
            push(&p, "a", &NEVER).unwrap_err(),
            pull(&p, &NEVER).unwrap_err(),
            status(&p).unwrap_err(),
        ] {
            assert!(e.contains("не меняем") && e.contains("мой-сервер") && e.contains("git remote set-url"), "{e}");
        }
        assert_eq!(git(&p.dir, &["remote", "get-url", "origin"]), theirs, "чужой адрес переписан");
        assert!(git(&ours, &["branch", "--list"]).is_empty(), "в среду уехало то, чего человек не просил");

        // Убрал origin сам — дальше как раньше: заводим свой и работаем.
        git(&p.dir, &["remote", "remove", "origin"]);
        assert!(matches!(layout_state(&p), Layout::Drifted(d) if d.contains("нет origin")));
        push(&p, "a", &NEVER).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&p.dir, &["remote", "get-url", "origin"]), p.remote);
        assert!(git(&ours, &["branch", "--list"]).contains("dev2"));
    }

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
        // через него идут вкладка «Инструменты» и инструмент агента.
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
        // Навязанной ветки в локальном режиме нет: её узнаёт git.
        p = Project { branch: String::new(), remote: String::new(), login: String::new(), local: true, ..p };

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
        git_in_its_own_home();
        let t = tempdir::Dir::new();
        let dir = t.path().join("Проект без origin");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let p = Project {
            dir: dir.clone(),
            git: system_git(),
            branch: String::new(),
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

        // Человек завёл origin сам — и дальше всё про него: слова приёма тоже
        // (прежде `pull` говорил «ветки … в Gitea ещё нет»).
        let theirs = t.path().join("его-сервер.git");
        git(t.path(), &["init", "-q", "--bare", "-b", "main", theirs.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", theirs.to_str().unwrap()]);
        let e = pull(&p, &NEVER).unwrap_err();
        assert!(e.contains("у origin ещё нет") && !e.contains("Gitea"), "{e}");
        let out = push(&p, "вторая", &NEVER).unwrap_or_else(|e| panic!("{e}"));
        assert!(out.contains("отправлена") && out.contains("ветка main"), "{out}");
        // Конвенция среды — её дело: отдельной `main` в чужой репозиторий мы
        // не создаём и «репозиторий создан» не рассказываем.
        assert!(!out.contains("репозиторий создан"), "{out}");
    }

    /// Пустой первый коммит МЫ ДЕЛАЕМ (`--allow-empty`: без него в туннельном
    /// режиме первому push нечего отправлять), и ответ обязан говорить
    /// именно это, а не «нечего коммитить» (находка ревью 30.10).
    #[test]
    fn an_empty_first_commit_is_called_what_it_is() {
        git_in_its_own_home();
        let t = tempdir::Dir::new();
        let dir = t.path().join("Пустой проект");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let p = Project {
            dir: dir.clone(),
            git: system_git(),
            branch: String::new(),
            remote: String::new(),
            login: String::new(),
            local: true,
        };
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "me@example.com"]);
        git(&dir, &["config", "user.name", "Я"]);
        // Ни служебных файлов, ни выгрузки — добавлять нечего, но ветку
        // коммит заводит.
        let out = push(&p, "", &NEVER).unwrap();
        assert!(out.contains("пустой первый коммит"), "{out}");
        assert_eq!(git(&dir, &["log", "-1", "--format=%s"]), "выгрузка");
        // Второй раз коммита уже не будет — и вот теперь «нечего коммитить».
        let out = push(&p, "", &NEVER).unwrap();
        assert!(out.contains("нечего коммитить"), "{out}");
        assert_eq!(git(&dir, &["rev-list", "--count", "HEAD"]), "1");

        // И тот же пустой коммит НЕ делается, когда в индексе лежит чужое:
        // без pathspec он увёз бы приготовленное человеком в наш коммит.
        let his = t.path().join("Его пустой проект");
        std::fs::create_dir_all(his.join("src")).unwrap();
        let p = Project { dir: his.clone(), ..p };
        git(&his, &["init", "-q", "-b", "main"]);
        git(&his, &["config", "user.email", "me@example.com"]);
        git(&his, &["config", "user.name", "Я"]);
        std::fs::write(his.join("его-заметки.md"), "черновик").unwrap();
        git(&his, &["add", "--", "его-заметки.md"]);
        let out = push(&p, "", &NEVER).unwrap();
        assert!(out.contains("нечего коммитить"), "{out}");
        assert_eq!(git(&his, &["rev-list", "--count", "--all"]), "0", "коммита быть не должно");
        assert_eq!(git(&his, &["diff", "--cached", "--name-only"]), "его-заметки.md");
    }

    /// Навязанной ветки и личности в локальном режиме нет вовсе: иначе
    /// «нужной» веткой становилась пустая строка (`git init -b `, «ветка
    /// main, нужна »), а подписью коммита — `@dev1c.local`. Имя `main`
    /// остаётся ровно для одного: завести ветку на пустом месте.
    #[test]
    fn a_local_project_imposes_neither_a_branch_nor_an_identity() {
        let mut cfg = crate::cfg::filled_for_test();
        cfg.current_mut().mode = "local".into();
        cfg.current_mut().workstation.clear();
        cfg.current_mut().gitea = "http://localhost:3000".into();
        let t = tempdir::Dir::new();
        cfg.current_mut().project = t.path().join("Бух").display().to_string();
        let p = Project::from_cfg(&cfg, Path::new("/tools"));
        assert!(p.branch.is_empty(), "ветку человека мы не называем заранее: её знает git");
        assert_eq!(init_branch(&p), "main");
        assert!(p.remote.is_empty(), "адрес среды в локальном режиме не собирается даже из заполненного поля");
        assert!(p.local && p.login.is_empty());
        // В туннельном режиме ветка — конвенция среды, и она же имя первой.
        cfg.current_mut().mode = "tunnel".into();
        cfg.current_mut().workstation = "dev2".into();
        assert_eq!(init_branch(&Project::from_cfg(&cfg, Path::new("/tools"))), "dev2");

        let f = git_flags("");
        assert!(!f.iter().any(|a| a.starts_with("user.name") || a.starts_with("http.extraHeader") || a == "credential.helper="), "{f:?}");
        assert!(f.iter().any(|a| a == "safe.directory=*") && f.iter().any(|a| a == "gc.auto=0"), "{f:?}");
    }

    /// Ветка человека — его, и «Установить» её не меняет. Два случая, на
    /// которых чтение `.git/HEAD` файлом давало `main` и уводило работу:
    /// ворктри (`.git` там файл) и отделённый HEAD. Сценарий ревью 30.10:
    /// разработчик держит проект в ворктри, факт «проект» красный из-за наших
    /// служебных файлов, он жмёт «Установить» — и оказывается на другой
    /// ветке либо получает отказ checkout поверх своих правок.
    #[test]
    fn in_local_mode_the_branch_belongs_to_the_person_in_a_worktree_too() {
        git_in_its_own_home();
        let t = tempdir::Dir::new();
        let main_dir = t.path().join("Бух");
        std::fs::create_dir_all(main_dir.join("src")).unwrap();
        git(&main_dir, &["init", "-q", "-b", "main"]);
        git(&main_dir, &["config", "user.email", "me@example.com"]);
        git(&main_dir, &["config", "user.name", "Я"]);
        std::fs::write(main_dir.join("src").join("M.bsl"), "1").unwrap();
        git(&main_dir, &["add", "-A"]);
        git(&main_dir, &["commit", "-qm", "первая"]);
        // Ворктри человека на своей ветке — там `.git` файл, а не каталог.
        let tree = t.path().join("Бух-ветка");
        git(&main_dir, &["worktree", "add", "-q", "-b", "моя", tree.to_str().unwrap()]);
        assert!(tree.join(".git").is_file(), "тест держится на том, что в ворктри .git — файл");
        let p = Project {
            dir: tree.clone(),
            git: system_git(),
            branch: String::new(),
            remote: String::new(),
            login: String::new(),
            local: true,
        };

        // Раскладка служебных файлов — да, ветка — нет.
        init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(git(&tree, &["symbolic-ref", "--short", "HEAD"]), "моя", "«Установить» переключило ветку человека");
        assert_eq!(branch_label(&p), "моя");
        let out = push(&p, "правка в ворктри", &NEVER).unwrap_or_else(|e| panic!("{e}"));
        assert!(out.contains("ветка моя") && !out.contains("main"), "{out}");
        assert_eq!(git(&tree, &["symbolic-ref", "--short", "HEAD"]), "моя");

        // Отделённый HEAD — состояние, а не ветка: имя ему не выдумываем, и
        // индекс человека не трогаем вовсе. Origin тут уже есть — иначе
        // `pull` отказал бы раньше и по другой причине.
        let theirs = t.path().join("его-сервер.git");
        git(t.path(), &["init", "-q", "--bare", "-b", "main", theirs.to_str().unwrap()]);
        git(&tree, &["remote", "add", "origin", theirs.to_str().unwrap()]);
        git(&tree, &["checkout", "-q", "--detach", "HEAD"]);
        std::fs::write(tree.join("src").join("M.bsl"), "2").unwrap();
        for e in [push(&p, "x", &NEVER).unwrap_err(), pull(&p, &NEVER).unwrap_err()] {
            assert!(e.contains("HEAD отделён") && e.contains("git switch"), "{e}");
        }
        assert!(git(&tree, &["diff", "--cached", "--name-only"]).is_empty(), "при отделённом HEAD мы ничего не добавили в индекс");
        let s = status(&p).unwrap();
        assert!(s.contains("HEAD отделён") && !s.contains("ветка main"), "{s}");
    }

    /// Фиксируем ровно то, что добавили сами. В локальном режиме репозиторий
    /// принадлежит человеку, и приготовленное им к своему коммиту уезжало в
    /// наш, с нашим сообщением про выгрузку (находка ревью 30.10):
    /// `git add -- src …`, а затем `commit` ВСЕГО индекса.
    #[test]
    fn our_commit_takes_only_what_we_added() {
        git_in_its_own_home();
        let t = tempdir::Dir::new();
        let dir = t.path().join("Его проект");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let p = Project {
            dir: dir.clone(),
            git: system_git(),
            branch: String::new(),
            remote: String::new(),
            login: String::new(),
            local: true,
        };
        init(&p, Path::new("nowhere"), &[], &|_| {}).unwrap_or_else(|e| panic!("{e}"));
        git(&dir, &["config", "user.email", "me@example.com"]);
        git(&dir, &["config", "user.name", "Я"]);

        // Человек приготовил к своему коммиту свой файл, мы — выгрузку.
        std::fs::write(dir.join("его-заметки.md"), "черновик").unwrap();
        git(&dir, &["add", "--", "его-заметки.md"]);
        std::fs::write(dir.join("src").join("M.bsl"), "1").unwrap();
        push(&p, "выгрузка", &NEVER).unwrap_or_else(|e| panic!("{e}"));

        let committed = git(&dir, &["show", "--pretty=", "--name-only", "HEAD"]);
        assert!(committed.contains("src/M.bsl"), "{committed}");
        assert!(!committed.contains("его-заметки"), "в наш коммит уехало приготовленное человеком: {committed}");
        // И остался приготовленным: индекс человека мы не разбирали.
        assert_eq!(git(&dir, &["diff", "--cached", "--name-only"]), "его-заметки.md");
    }

    #[test]
    fn the_remote_follows_the_explicit_gitea() {
        let mut cfg = crate::cfg::filled_for_test();
        cfg.current_mut().gitea = "http://localhost:3000".into();
        assert_eq!(Project::from_cfg(&cfg, Path::new("/tools")).remote, "http://localhost:3000/dev1c/Buh.git");
        // Адрес поля разошёлся с тем, что в репозитории: это не «поправим», а
        // «скажем» — см. `a_foreign_origin_is_not_replaced_when_the_mode_changes`.
        let (_t, mut p) = fixture();
        p.remote = "http://localhost:3000/dev1c/project.git".into();
        assert!(matches!(layout_state(&p), Layout::ForeignOrigin(_)));
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
        let r = retry_once(std::time::Duration::from_secs(2), &cancel, || {
            calls.set(calls.get() + 1);
            Err::<String, _>("прервано".to_string())
        });
        assert_eq!(r, Err("прервано".to_string()));
        // Смысл проверяется счётчиком вызовов, а не секундомером: пауза
        // повтора не взята ровно тогда, когда вызов был один. Абсолютный
        // порог здесь ничего не добавлял и мог мигнуть на медленном раннере
        // (STATUS 30).
        assert_eq!(calls.get(), 1, "отмена не должна приводить к повтору, а значит и платить паузу");
    }

    #[test]
    fn second_caller_is_told_busy_not_queued() {
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_t, p) = fixture();
        let guard = hold("project_push", &p.dir).unwrap();
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
        let (_t, p) = fixture();
        let g = hold("project_push", &p.dir).unwrap();
        assert_eq!(hold("project_status", &p.dir).err(), Some("занято: идёт project_push".to_string()));
        drop(g);
        // Цель — глобал на процесс: следующий тест её не должен застать —
        // ставит её только сам (`without_a_target_the_lock_is_in_memory_only`).
        *LOCK_TARGET.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Наблюдение владельца 30.10: «я только включил, какой project_status?».
    /// Набор готовности читает `doing()` и по непустому имени перестаёт
    /// спрашивать базу — значит собственное чтение состояния проекта,
    /// объявившее себя операцией, красило весь набор в «Занято» и называло
    /// человеку имя инструмента, которого тот не звал.
    #[test]
    fn a_read_does_not_announce_itself_as_an_operation() {
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_t, p) = fixture();
        let g = hold_read(&p.dir).unwrap();
        assert_eq!(doing(&p.dir), "", "чтение не вправе объявлять себя операцией");
        drop(g);
        // А настоящая операция по-прежнему называется: отказ соперника и
        // строка набора на этом и держатся.
        let g = hold("project_push", &p.dir).unwrap();
        assert_eq!(doing(&p.dir), "project_push");
        drop(g);
    }

    #[test]
    fn without_a_target_the_lock_is_in_memory_only() {
        // Разовые команды (`status`, `example`) цели не ставят: отказа быть не должно.
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_t, p) = fixture();
        let _g = hold("project_status", &p.dir).unwrap();
    }

    #[test]
    fn two_folders_do_not_wait_for_each_other() {
        // `file_lock` — файл, не каталог: на Linux `OpenOptions::create().write(true)`
        // на СУЩЕСТВУЮЩЕЙ папке падает `EISDIR`, а не отдаёт «занято»/«свободно»
        // (круг починки задачи 7, находка 1 — бриф передавал сюда сам каталог,
        // и буквальный тест был бы заведомо красным на второй платформе).
        // Честная проверка — файл замка ВНУТРИ каждой папки, тем же путём, что
        // строит `lock_file_path` для настоящего `hold` (находка 2 того же круга).
        let (ta, tb) = (tempdir::Dir::new(), tempdir::Dir::new());
        let file_a = lock_file_path(ta.path(), ta.path());
        let file_b = lock_file_path(tb.path(), tb.path());
        let a = file_lock(&file_a).unwrap().expect("первая папка свободна");
        let b = file_lock(&file_b).unwrap();
        assert!(b.is_some(), "замок другой папки не должен ждать первую");
        assert!(file_lock(&file_a).unwrap().is_none(), "а своя папка занята, пока держат");
        drop(a);
        drop(b);
    }

    /// Та же гарантия, но на межпроцессном слое и под настоящей целью
    /// (`set_lock_target`): круг починки задачи 7, находка 3 — до этого оба
    /// новых теста доказывали только слой памяти (`BUSY`, через `hold`), а
    /// дисковый слой, тот самый, где имя строилось дважды (находка 2), не
    /// проверял никто. Берём файл A напрямую, как бы его держал ЧУЖОЙ
    /// процесс (не наш `hold`, у которого и так есть слой памяти) — и видим,
    /// что `hold` на папке A честно отказывает, а на папке B проходит.
    #[test]
    fn two_projects_on_disk_do_not_block_each_other() {
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let state = tempdir::Dir::new();
        set_lock_target(state.path());
        let (_ta, pa) = fixture();
        let (_tb, pb) = fixture();
        let other = file_lock(&lock_file_path(state.path(), &pa.dir)).unwrap().expect("файл папки A свободен в начале теста");
        assert!(hold("project_push", &pa.dir).is_err(), "папка A занята на диске чужим процессом");
        let g = hold("project_push", &pb.dir).unwrap_or_else(|e| panic!("папка B не должна видеть занятость папки A: {e}"));
        drop(g);
        drop(other);
        assert!(hold("project_push", &pa.dir).is_ok(), "файл отпущен — папка A снова свободна");
        clear_lock_target_for_test();
    }

    /// Та же гарантия, но через настоящую поверхность (`call`, та же, что у
    /// агента и моста), а не через низкоуровневый `file_lock` напрямую:
    /// решение владельца 6 — работа по одному заказчику не отказывает
    /// четырём остальным, потому что у соединений разные папки, и замок
    /// делит их по папке, а не по машине.
    #[test]
    fn two_projects_do_not_block_each_other() {
        let _s = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_ta, pa) = fixture();
        let (_tb, pb) = fixture();
        let guard = hold("project_push", &pa.dir).unwrap();
        // Папка B свободна, хотя папка A сейчас занята.
        let (text, err) = call(&pb, "project_status", &Value::Null);
        assert!(!err, "{text}");
        // А своя папка всё ещё занята.
        assert_eq!(hold("project_status", &pa.dir).err(), Some("занято: идёт project_push".to_string()));
        drop(guard);
    }
}
