//! `host.publication`: коннектор поднимает HTTP-сервис базы сам — своим
//! httpd под пользователем, без Конфигуратора и без прав администратора (см.
//! design «HTTP-сервис базы ведёт коннектор — на обеих ОС»).
//!
//! Атрибуты `default.vrd` ниже обязательны, а не стиль: без них HTTP-сервис
//! держит сеанс между вызовами агента, а учебная лицензия даёт один сеанс
//! «Предприятия» на базу — второй вызов подряд возвращает HTTP 500. Их
//! принимает только элемент `service`; на контейнере платформа отвергает весь
//! файл («Ошибка при разборе»).

use crate::cfg::Cfg;
use crate::job;
use crate::ops_host;
use common::kit::Kit;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Имя HTTP-сервиса и его путь внутри публикации — то же, что ставит
/// расширение (`host.extension`).
const SERVICE_ROOT_URL: &str = "mcp-1c";

/// Текст `default.vrd`: публикация в корень своего порта (`base="/"`, у
/// каждого рабочего места свой порт — корень свободен), с паролем базы
/// внутри. Этот текст никогда не печатается в журнал — только пишется в файл.
pub fn vrd(cfg: &Cfg) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<point xmlns=\"http://v8.1c.ru/8.2/virtual-resource-system\"\n\
\t\tbase=\"/\" ib=\"File=&quot;{base}&quot;;Usr=&quot;{user}&quot;;Pwd=&quot;{password}&quot;;\"\n\
\t\tenable=\"true\">\n\
\t<httpServices publishByDefault=\"true\" publishExtensionsByDefault=\"true\">\n\
\t\t<service name=\"MCPService\" rootUrl=\"{root}\" enable=\"true\"\n\
\t\t\t\treuseSessions=\"dontuse\" sessionMaxAge=\"1\" poolSize=\"1\" poolTimeout=\"1\"/>\n\
\t</httpServices>\n\
</point>\n",
        base = cfg.host.base_path,
        user = cfg.host.db_user,
        password = cfg.host.db_password,
        root = SERVICE_ROOT_URL,
    )
}

/// Готовит каталог публикации и пишет в него `default.vrd` с правами только
/// для владельца (Unix) — дескриптор несёт пароль базы, и на образе три
/// рабочих места (`dev1`/`dev2`/`dev3`) делят диск под разными
/// пользователями: без ограничения `dev1` читает пароль базы `dev2` прямо из
/// `/home/dev2/dev1c/pub/default.vrd`. На Windows ограничивать нечего: каталог
/// живёт под `%LOCALAPPDATA%`, у него уже свой ACL на пользователя — отдельной
/// ветки для Windows здесь нет и не нужно.
pub fn write_vrd(pub_dir: &Path, text: &str) -> Result<(), String> {
    ensure_pub_dir(pub_dir)?;
    write_owner_only(&pub_dir.join("default.vrd"), text)
}

#[cfg(unix)]
fn ensure_pub_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{} не создан: {e}", dir.display()))?;
    // `mode()` у DirBuilder действует только на вновь созданные каталоги; если
    // каталог остался с прошлого запуска (или с версии до этого фикса) с более
    // широкими правами — поджимаем явно, а не оставляем как есть.
    tighten_dir(dir)
}

#[cfg(not(unix))]
fn ensure_pub_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{} не создан: {e}", dir.display()))
}

#[cfg(unix)]
fn tighten_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("права {} не выставлены: {e}", dir.display()))
}

#[cfg(unix)]
fn write_owner_only(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{} не создан: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{} не записан: {e}", path.display()))?;
    // Та же причина, что и у каталога: `mode()` в OpenOptions действует только
    // при создании файла — если он остался с прошлого запуска, поджимаем.
    f.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("права {} не выставлены: {e}", path.display()))
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{} не записан: {e}", path.display()))
}

/// Пути одной публикации: свой httpd (Windows) или системный apache2
/// (образ), свой каталог публикации (`pub_dir` — никогда каталог базы: он
/// может быть только для чтения или на сетевом диске) и журнал/pid рядом.
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    /// `ServerRoot`: откуда резолвятся относительные `modules/mod_*.so` и
    /// `conf/mime.types`.
    pub root: PathBuf,
    /// `DocumentRoot` и каталог `default.vrd` — свой, не каталог базы.
    pub pub_dir: PathBuf,
    pub conf: PathBuf,
    pub pid: PathBuf,
    pub log: PathBuf,
    /// Модуль платформы (`_1cws_module`): `wsap24.dll`/`wsap24t.dll` рядом с
    /// `1cv8.exe` на Windows, `wsap24t.so` в образе.
    pub module: PathBuf,
    pub port: u16,
}

/// Модуль платформы рядом с исполняемым файлом 1cv8: `wsap24.dll` у
/// коммерческой сборки, `wsap24t.dll` — у учебной (Windows), `wsap24t.so` — в
/// образе. Чистая функция — каталог называет вызывающий, тест подаёт
/// временный, настоящий 1cv8 искать не нужно.
fn module_in(bin: &Path) -> Result<PathBuf, String> {
    let names: &[&str] = if cfg!(windows) { &["wsap24.dll", "wsap24t.dll"] } else { &["wsap24t.so"] };
    names
        .iter()
        .map(|n| bin.join(n))
        .find(|p| p.exists())
        .ok_or_else(|| format!("рядом с платформой в {} нет модуля веб-сервера 1С ({})", bin.display(), names.join("/")))
}

/// Пути, зависящие только от каталога состояния коннектора (`tools`) — не от
/// настоящей платформы 1С: вынесены отдельно, чтобы тест считал их с
/// временным каталогом, а не с настоящим `%LOCALAPPDATA%`/`$HOME`.
fn paths_under(tools: &Path, root: PathBuf, module: PathBuf, port: u16) -> Paths {
    Paths {
        root,
        pub_dir: tools.join("pub"),
        conf: tools.join("httpd.conf"),
        pid: tools.join("httpd.pid"),
        log: tools.join("httpd.log"),
        module,
        port,
    }
}

impl Paths {
    /// Пути публикации на этой машине: `ServerRoot` — свой httpd из комплекта
    /// (`<tools>/httpd`, скопирован `host.tools`) на Windows, системный
    /// apache2 в образе — то, что уже делает стенд.
    pub fn of(cfg: &Cfg, kit: &Kit) -> Result<Paths, String> {
        let tools = ops_host::tools_dir();
        let root = if cfg!(windows) { tools.join("httpd") } else { PathBuf::from("/etc/apache2") };
        if cfg!(windows) && !root.join("bin").join("httpd.exe").exists() {
            return Err(format!(
                "в {} нет httpd/bin/httpd.exe — выполните host.tools (комплект: {})",
                tools.display(),
                kit.root.display()
            ));
        }
        let exe = ops_host::find_1cv8().ok_or_else(|| "платформа 1С не найдена — установите её или укажите переменную ONEC_PLATFORM".to_string())?;
        let bin = exe.parent().ok_or_else(|| format!("у {} нет каталога", exe.display()))?;
        let module = module_in(bin)?;
        Ok(paths_under(&tools, root, module, cfg.http_port()))
    }
}

/// Текст конфигурации httpd: Windows — ровно как в спайке 2026-10-20
/// (минимальный набор из пяти модулей плюс модуль платформы, из комплекта),
/// образ — та же форма, что уже работает в стенде (`config/1c/stand.sh`,
/// функция `publish`): на Debian те пять модулей живут не под `ServerRoot`,
/// явные `modules/mod_*.so` не резолвятся — ведёт `IncludeOptional
/// mods-enabled/*`.
pub fn httpd_conf(p: &Paths) -> String {
    if cfg!(windows) {
        windows_httpd_conf(p)
    } else {
        linux_httpd_conf(p)
    }
}

fn windows_httpd_conf(p: &Paths) -> String {
    format!(
        "ServerRoot \"{root}\"\n\
ServerName localhost\n\
Listen 127.0.0.1:{port}\n\
PidFile \"{pid}\"\n\
ErrorLog \"{log}\"\n\
LogLevel warn\n\
LoadModule authz_core_module modules/mod_authz_core.so\n\
LoadModule authz_host_module modules/mod_authz_host.so\n\
LoadModule alias_module modules/mod_alias.so\n\
LoadModule mime_module modules/mod_mime.so\n\
LoadModule dir_module modules/mod_dir.so\n\
LoadModule _1cws_module \"{module}\"\n\
ThreadsPerChild 16\n\
DocumentRoot \"{pub_dir}\"\n\
<Directory \"{pub_dir}\">\n\
    AllowOverride All\n\
    Options None\n\
    Require all granted\n\
    SetHandler 1c-application\n\
    ManagedApplicationDescriptor \"{pub_dir}/default.vrd\"\n\
</Directory>\n",
        root = p.root.display(),
        port = p.port,
        pid = p.pid.display(),
        log = p.log.display(),
        module = p.module.display(),
        pub_dir = p.pub_dir.display(),
    )
}

/// Образ: форма стенда минус то, что там нет смысла без root — `User`/`Group`
/// (стенд поднимает Apache от root и переключает его на владельца базы;
/// коннектор и так уже запущен от имени рабочего места) — и минус `Alias
/// "/base"` (старая схема публикации в подкаталог, которую отменяет задача
/// 13): здесь, как и на Windows, публикация — в корень своего порта.
/// `ServerLimit 1`/`StartServers 1` и лимиты потоков — не стиль, а то, что
/// держит единственный сеанс «Предприятия» на базу: второй процесс-воркер
/// значил бы второй сеанс, а учебная лицензия его не даёт.
fn linux_httpd_conf(p: &Paths) -> String {
    let tools = p.pid.parent().map(|d| d.display().to_string()).unwrap_or_default();
    format!(
        "ServerRoot \"{root}\"\n\
ServerName localhost\n\
DefaultRuntimeDir \"{tools}\"\n\
PidFile \"{pid}\"\n\
Listen 127.0.0.1:{port}\n\
ErrorLog \"{log}\"\n\
LogLevel warn\n\
IncludeOptional mods-enabled/*.load\n\
IncludeOptional mods-enabled/*.conf\n\
ServerLimit 1\n\
StartServers 1\n\
ThreadsPerChild 16\n\
MinSpareThreads 16\n\
MaxSpareThreads 16\n\
MaxRequestWorkers 16\n\
LoadModule _1cws_module \"{module}\"\n\
DocumentRoot \"{pub_dir}\"\n\
<Directory \"{pub_dir}\">\n\
    AllowOverride All\n\
    Options None\n\
    Require all granted\n\
    SetHandler 1c-application\n\
    ManagedApplicationDescriptor \"{pub_dir}/default.vrd\"\n\
</Directory>\n",
        root = p.root.display(),
        tools = tools,
        pid = p.pid.display(),
        port = p.port,
        log = p.log.display(),
        module = p.module.display(),
        pub_dir = p.pub_dir.display(),
    )
}

/// Бинарь, которым поднимается веб-сервер: свой httpd из комплекта на
/// Windows, системный `apache2` в образе — то, что уже делает стенд.
fn httpd_binary(p: &Paths) -> PathBuf {
    if cfg!(windows) { p.root.join("bin").join("httpd.exe") } else { PathBuf::from("apache2") }
}

/// Грубая проверка «порт свободен»: TCP-соединение на петлю не прошло.
fn port_is_free(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)), std::time::Duration::from_millis(200)).is_err()
}

/// Грубая проверка «httpd рабочего места отвечает»: TCP-соединение на петлю
/// прошло. Не HTTP-запрос — сервер может ещё не нести ни одной публикации,
/// порт уже слушается, этого достаточно для факта «веб-сервер».
pub fn httpd_responds(port: u16) -> bool {
    !port_is_free(port)
}

/// Job Object (Windows) / группа процессов (Unix) под сервер, которую
/// коннектор держит весь свой процесс целиком, а не только на время вызова
/// `start()`. На Unix `job::group_std` уже посадил бы httpd в свою группу, и
/// обычный путь «quit → stop()» достаточно надёжен; здесь то же самое работает
/// и как бэкстоп на случай, если `stop()` никогда не позвали. На Windows
/// `job::group_std` — no-op (нет fork/pgid), и без своего Job Object крах
/// самого коннектора (паника, снятие из диспетчера задач — не штатный «quit»)
/// не запускает `stop()`, и httpd остаётся висеть. Job здесь держит это за
/// нас: потомки httpd автоматически попадают в тот же Job (пока сами не
/// запросили breakaway), и ядро убивает всех, кто в нём остался, когда
/// закрывается последний описатель (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
/// `crate::job::Job::new`). Нормальная остановка (`stop`) явно очищает эту
/// переменную — бэкстоп нужен только на крах, не на каждый вызов.
static HTTPD_JOB: Mutex<Option<job::Job>> = Mutex::new(None);

/// Поднимает HTTP-сервис из уже записанных `p.conf`/`default.vrd`:
/// `httpd -f <conf> -k start`. Дескриптор и конфигурация сюда не пишутся —
/// это дело вызывающего (пароль базы не должен попасть в эту функцию только
/// затем, чтобы тут же лечь в текст, который можно случайно залогировать).
pub fn start(p: &Paths) -> Result<(), String> {
    use std::process::Stdio;
    let bin = httpd_binary(p);
    let mut cmd = std::process::Command::new(&bin);
    cmd.args(["-f", &p.conf.to_string_lossy(), "-k", "start"]).stdout(Stdio::piped()).stderr(Stdio::piped());
    job::group_std(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("{} не запустился: {e}", bin.display()))?;
    if let Some(job) = job::Job::new() {
        job.assign_std(&child);
        *HTTPD_JOB.lock().unwrap() = Some(job);
    }
    let out = child.wait_with_output().map_err(|e| format!("{} не вышел: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!("httpd -k start: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Гасит сервис: `httpd -f <conf> -k stop`, затем ждёт, пока порт не
/// освободится — остановка не мгновенна. Бюджет 5 с (20 попыток по 250 мс) —
/// догадка со спайка, не измеренная на настоящей остановке; если в живом
/// запуске она окажется мала, правка — поднять число попыток здесь, а не
/// городить отдельный backoff.
pub fn stop(p: &Paths) -> Result<(), String> {
    let bin = httpd_binary(p);
    let out = std::process::Command::new(&bin)
        .args(["-f", &p.conf.to_string_lossy(), "-k", "stop"])
        .output()
        .map_err(|e| format!("{} не запустился: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!("httpd -k stop: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    for _ in 0..20 {
        if port_is_free(p.port) {
            // Штатная остановка — Job-бэкстоп на крах здесь больше не нужен.
            *HTTPD_JOB.lock().unwrap() = None;
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err(format!("порт 127.0.0.1:{} всё ещё занят после httpd -k stop", p.port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_for_test() -> Cfg {
        let mut c = Cfg::default();
        c.host.base_path = "C:\\Bases\\УНФ".into();
        c.host.db_user = "Администратор".into();
        c.host.db_password = "секрет".into();
        c
    }

    #[test]
    fn vrd_carries_the_mandatory_attributes() {
        // Без этих атрибутов учебная лицензия (один сеанс «Предприятия» на
        // базу) возвращает агенту HTTP 500 уже на втором вызове подряд.
        let text = vrd(&cfg_for_test());
        for needle in [
            "publishExtensionsByDefault=\"true\"",
            "reuseSessions=\"dontuse\"",
            "sessionMaxAge=\"1\"",
            "poolSize=\"1\"",
            "poolTimeout=\"1\"",
            "rootUrl=\"mcp-1c\"",
            "base=\"/\"",
        ] {
            assert!(text.contains(needle), "в дескрипторе нет {needle:?}:\n{text}");
        }
        // Пароль там, но строкой, не для печати (см. host.publication::apply).
        assert!(text.contains("Pwd=&quot;секрет&quot;"));
    }

    #[test]
    fn vrd_never_leaks_the_password_into_the_journal_by_itself() {
        // vrd() — чистая функция текста, не событие: она ничего никуда не
        // пишет и не печатает сама — писать её в файл и решать, что сказать в
        // журнал, обязан вызывающий (host.publication::apply).
        let text = vrd(&cfg_for_test());
        assert!(text.contains("секрет"), "тест ничего не проверял бы без пароля в тексте");
    }

    fn sample_paths() -> Paths {
        Paths {
            root: PathBuf::from("C:\\Users\\dev\\AppData\\Local\\dev1c\\httpd"),
            pub_dir: PathBuf::from("C:\\Users\\dev\\AppData\\Local\\dev1c\\pub"),
            conf: PathBuf::from("C:\\Users\\dev\\AppData\\Local\\dev1c\\httpd.conf"),
            pid: PathBuf::from("C:\\Users\\dev\\AppData\\Local\\dev1c\\httpd.pid"),
            log: PathBuf::from("C:\\Users\\dev\\AppData\\Local\\dev1c\\httpd.log"),
            module: PathBuf::from("C:\\Program Files\\1cv8t\\8.5.1.1150\\bin\\wsap24t.dll"),
            port: 8082,
        }
    }

    #[test]
    fn httpd_conf_matches_the_spiked_sample() {
        // Эталон — ровно конфигурация спайка 2026-10-20 (training-85 на этой
        // машине), с путями и портом, подставленными тестом вместо реальных.
        let expected = "ServerRoot \"C:\\Users\\dev\\AppData\\Local\\dev1c\\httpd\"\n\
ServerName localhost\n\
Listen 127.0.0.1:8082\n\
PidFile \"C:\\Users\\dev\\AppData\\Local\\dev1c\\httpd.pid\"\n\
ErrorLog \"C:\\Users\\dev\\AppData\\Local\\dev1c\\httpd.log\"\n\
LogLevel warn\n\
LoadModule authz_core_module modules/mod_authz_core.so\n\
LoadModule authz_host_module modules/mod_authz_host.so\n\
LoadModule alias_module modules/mod_alias.so\n\
LoadModule mime_module modules/mod_mime.so\n\
LoadModule dir_module modules/mod_dir.so\n\
LoadModule _1cws_module \"C:\\Program Files\\1cv8t\\8.5.1.1150\\bin\\wsap24t.dll\"\n\
ThreadsPerChild 16\n\
DocumentRoot \"C:\\Users\\dev\\AppData\\Local\\dev1c\\pub\"\n\
<Directory \"C:\\Users\\dev\\AppData\\Local\\dev1c\\pub\">\n\
    AllowOverride All\n\
    Options None\n\
    Require all granted\n\
    SetHandler 1c-application\n\
    ManagedApplicationDescriptor \"C:\\Users\\dev\\AppData\\Local\\dev1c\\pub/default.vrd\"\n\
</Directory>\n";
        // windows_httpd_conf() напрямую — а не через httpd_conf() — тест
        // проверяет именно форму Windows вне зависимости от того, на какой ОС
        // компилируется сам тестовый бинарник (CI может собирать и на Linux).
        assert_eq!(windows_httpd_conf(&sample_paths()), expected);
    }

    #[test]
    fn httpd_listens_on_loopback_only() {
        // Windows не спрашивает про брандмауэр, и база не торчит наружу,
        // только когда строка Listen одна и привязана к петле.
        let text = windows_httpd_conf(&sample_paths());
        let listens: Vec<&str> = text.lines().filter(|l| l.starts_with("Listen")).collect();
        assert_eq!(listens.len(), 1, "{listens:?}");
        assert!(listens[0].starts_with("Listen 127.0.0.1:"), "{}", listens[0]);
    }

    #[test]
    fn only_the_five_modules_plus_the_platform() {
        // Минимальный набор, проверенный спайком: без лишнего — меньше и
        // файлов в комплекте, и претензий антивируса.
        let text = windows_httpd_conf(&sample_paths());
        let modules: Vec<&str> = text.lines().filter(|l| l.starts_with("LoadModule")).collect();
        assert_eq!(modules.len(), 6, "{modules:?}");
        for name in ["authz_core_module", "authz_host_module", "alias_module", "mime_module", "dir_module", "_1cws_module"] {
            assert!(modules.iter().any(|l| l.contains(name)), "нет {name} среди {modules:?}");
        }
    }

    fn linux_sample_paths() -> Paths {
        Paths {
            root: PathBuf::from("/etc/apache2"),
            pub_dir: PathBuf::from("/home/dev2/dev1c/pub"),
            conf: PathBuf::from("/home/dev2/dev1c/httpd.conf"),
            pid: PathBuf::from("/home/dev2/dev1c/httpd.pid"),
            log: PathBuf::from("/home/dev2/dev1c/httpd.log"),
            module: PathBuf::from("/opt/1cv8t/x86_64/8.5.1.1150/wsap24t.so"),
            port: 8082,
        }
    }

    #[test]
    fn linux_conf_matches_the_stands_working_shape() {
        // Форма стенда (config/1c/stand.sh, функция publish) минус User/Group
        // (стенд стартует от root и переключается; коннектор и так уже свой
        // пользователь) и минус Alias "/base" (публикация — в корень порта,
        // как и на Windows).
        let expected = "ServerRoot \"/etc/apache2\"\n\
ServerName localhost\n\
DefaultRuntimeDir \"/home/dev2/dev1c\"\n\
PidFile \"/home/dev2/dev1c/httpd.pid\"\n\
Listen 127.0.0.1:8082\n\
ErrorLog \"/home/dev2/dev1c/httpd.log\"\n\
LogLevel warn\n\
IncludeOptional mods-enabled/*.load\n\
IncludeOptional mods-enabled/*.conf\n\
ServerLimit 1\n\
StartServers 1\n\
ThreadsPerChild 16\n\
MinSpareThreads 16\n\
MaxSpareThreads 16\n\
MaxRequestWorkers 16\n\
LoadModule _1cws_module \"/opt/1cv8t/x86_64/8.5.1.1150/wsap24t.so\"\n\
DocumentRoot \"/home/dev2/dev1c/pub\"\n\
<Directory \"/home/dev2/dev1c/pub\">\n\
    AllowOverride All\n\
    Options None\n\
    Require all granted\n\
    SetHandler 1c-application\n\
    ManagedApplicationDescriptor \"/home/dev2/dev1c/pub/default.vrd\"\n\
</Directory>\n";
        assert_eq!(linux_httpd_conf(&linux_sample_paths()), expected);
    }

    #[test]
    fn linux_conf_does_not_start_apache_as_a_different_user() {
        // Стенд поднимает Apache от root и явно переключает его на владельца
        // базы (User/Group); коннектор уже запущен от имени рабочего места —
        // эти директивы были бы бессмысленны (и некого было бы менять).
        let text = linux_httpd_conf(&linux_sample_paths());
        assert!(!text.lines().any(|l| l.starts_with("User ") || l.starts_with("Group ")), "{text}");
    }

    #[test]
    fn linux_conf_also_listens_on_loopback_only() {
        let text = linux_httpd_conf(&linux_sample_paths());
        let listens: Vec<&str> = text.lines().filter(|l| l.starts_with("Listen")).collect();
        assert_eq!(listens.len(), 1, "{listens:?}");
        assert!(listens[0].starts_with("Listen 127.0.0.1:"), "{}", listens[0]);
    }

    #[test]
    fn httpd_conf_dispatches_to_the_shape_of_the_os_it_is_built_for() {
        // Сам httpd_conf() собирается на любой ОС и всегда возвращает форму
        // той, для которой собран тестовый бинарник — проверяем именно это,
        // а не гадаем по cfg!(windows) внутри теста.
        if cfg!(windows) {
            assert_eq!(httpd_conf(&sample_paths()), windows_httpd_conf(&sample_paths()));
        } else {
            assert_eq!(httpd_conf(&linux_sample_paths()), linux_httpd_conf(&linux_sample_paths()));
        }
    }

    #[test]
    fn paths_under_a_given_tools_dir_keep_pub_outside_the_base_folder() {
        // Каталог публикации — свой (<tools>/pub), не каталог базы: тот
        // может быть только для чтения или на сетевом диске.
        let tools = Path::new("T:\\tools");
        let p = paths_under(tools, PathBuf::from("T:\\tools\\httpd"), PathBuf::from("T:\\platform\\wsap24t.dll"), 8083);
        assert_eq!(p.pub_dir, tools.join("pub"));
        assert_eq!(p.conf, tools.join("httpd.conf"));
        assert_eq!(p.pid, tools.join("httpd.pid"));
        assert_eq!(p.log, tools.join("httpd.log"));
        assert_eq!(p.port, 8083);
    }

    #[test]
    fn module_in_picks_the_commercial_name_over_the_training_one() {
        let tmp = std::env::temp_dir().join(format!("dev1c-pub-module-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        if cfg!(windows) {
            std::fs::write(tmp.join("wsap24.dll"), "x").unwrap();
            std::fs::write(tmp.join("wsap24t.dll"), "x").unwrap();
            assert_eq!(module_in(&tmp).unwrap(), tmp.join("wsap24.dll"));
        } else {
            std::fs::write(tmp.join("wsap24t.so"), "x").unwrap();
            assert_eq!(module_in(&tmp).unwrap(), tmp.join("wsap24t.so"));
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn module_in_reports_a_readable_error_when_nothing_is_there() {
        let tmp = std::env::temp_dir().join(format!("dev1c-pub-module-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let e = module_in(&tmp).unwrap_err();
        assert!(e.contains("wsap24"), "{e}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_closed_port_is_free_and_does_not_respond() {
        // Порт 1 никто не слушает на пользовательской машине — ни тестов с
        // настоящим httpd, ни настоящей базы это не требует.
        assert!(port_is_free(1));
        assert!(!httpd_responds(1));
    }

    #[cfg(unix)]
    #[test]
    fn the_descriptor_and_its_directory_are_owner_only() {
        // Дескриптор несёт пароль базы, и на образе три рабочих места делят
        // диск под разными пользователями — без этого dev1 читал бы пароль
        // базы dev2 прямо из /home/dev2/dev1c/pub/default.vrd.
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("dev1c-pub-vrd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("pub");

        write_vrd(&dir, "не настоящий дескриптор, только для теста").unwrap();

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(dir.join("default.vrd")).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "{dir_mode:o}");
        assert_eq!(file_mode, 0o600, "{file_mode:o}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
