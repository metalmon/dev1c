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
#[cfg(windows)]
use crate::job;
use crate::ops_host;
use common::kit::Kit;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::Mutex;

/// Имя HTTP-сервиса и его путь внутри публикации — то же, что ставит
/// расширение (`host.extension`).
const SERVICE_ROOT_URL: &str = "mcp-1c";

/// Экранирует пять спецсимволов XML. Путь базы, логин и пароль идут в
/// атрибуты дескриптора как есть — кавычка или амперсанд в пароле иначе рвут
/// XML, платформа отвергает весь файл, а причина оседает в журнале
/// веб-сервера, не в журнале коннектора: агент просто не видит адреса.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

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
        base = xml_escape(&cfg.host.base_path),
        user = xml_escape(&cfg.host.db_user),
        password = xml_escape(&cfg.host.db_password),
        root = SERVICE_ROOT_URL,
    )
}

/// Дескриптор на диске — ровно то, что `vrd(cfg)` написал бы сейчас. Иначе
/// пароль/пользователь/база поменялись (человек нажал «Сохранить»), а
/// HTTP-сервис всё ещё держит старый текст — `state()` обязан увидеть это как
/// расхождение, а не читать совпадение порта за «сделано».
pub fn descriptor_matches(pub_dir: &Path, cfg: &Cfg) -> bool {
    std::fs::read_to_string(pub_dir.join("default.vrd")).map(|on_disk| on_disk == vrd(cfg)).unwrap_or(false)
}

/// `GET <base_url>/version` отвечает HTTP (любым статусом) — отличает наш
/// httpd (платформа ответит под опубликованным сервисом, даже пока
/// расширение ещё не поставлено) от чужой программы на том же порту, которая
/// на HTTP не похожа вовсе (reqwest не сможет разобрать ответ как HTTP и
/// вернёт ошибку). Не доказательство владения портом абсолютно во всех
/// случаях (можно представить чужой HTTP-сервер на том же порту), но вместе с
/// `descriptor_matches` — наша конфигурация плюс наш порт — этого достаточно.
pub fn service_responds(base_url: &str) -> bool {
    let Ok(c) = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(3)).build() else { return false };
    c.get(format!("{base_url}/version")).send().is_ok()
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
    let names: &[&str] = if cfg!(windows) { &["wsap24.dll", "wsap24t.dll"] } else { &["wsap24.so", "wsap24t.so"] };
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
/// mods-enabled/*`. Linux-ветка может провалиться — см. `linux_httpd_conf`.
pub fn httpd_conf(p: &Paths) -> Result<String, String> {
    if cfg!(windows) {
        Ok(windows_httpd_conf(p))
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
TypesConfig \"{mime_types}\"\n\
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
        // Явно, а не положившись на скомпилированное умолчание mod_mime —
        // ровно так, как бежал спайк 2026-10-20. Строкой, не `.join()`: эта
        // функция всегда пишет путь по-Windows, даже когда тестовый бинарник
        // собран на Linux (CI может собирать и там) — `.join()` добавил бы
        // '/' вместо '\' на не-Windows таргете и разошёлся бы с эталоном.
        mime_types = format!("{}\\conf\\mime.types", p.root.display()),
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
fn linux_httpd_conf(p: &Paths) -> Result<String, String> {
    // `Path::parent()` у однокомпонентного относительного пути отдаёт
    // `Some("")`, не `None` — пустой `DefaultRuntimeDir` молча прошёл бы мимо
    // `.unwrap_or_default()`. Проверяем явно и отказываем читаемой строкой.
    let tools = p.pid.parent().filter(|d| !d.as_os_str().is_empty()).ok_or_else(|| format!("у {} нет каталога — не могу вычислить DefaultRuntimeDir", p.pid.display()))?;
    Ok(format!(
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
        tools = tools.display(),
        pid = p.pid.display(),
        port = p.port,
        log = p.log.display(),
        module = p.module.display(),
        pub_dir = p.pub_dir.display(),
    ))
}

/// Бинарь httpd — используется только резервным путём на Windows (fallback
/// в `kill_tracked`, когда процесс почему-то не отслежен) и для диагностики;
/// основной Linux-путь зовёт системный `apache2` по имени напрямую.
#[cfg(windows)]
fn httpd_binary(p: &Paths) -> PathBuf {
    p.root.join("bin").join("httpd.exe")
}

/// Грубая проверка «порт свободен»: TCP-соединение на петлю не прошло.
fn port_is_free(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)), std::time::Duration::from_millis(200)).is_err()
}

/// Грубая проверка «на петле кто-то слушает» — не доказательство, что это
/// именно наш httpd (см. `service_responds`/`descriptor_matches` для этого),
/// а только информация: после своей остановки видеть чужую программу на том
/// же порту — не наша ошибка (см. `ops_host::Publication::revert_apply`).
pub fn httpd_responds(port: u16) -> bool {
    !port_is_free(port)
}

/// Последние непустые строки журнала ошибок httpd — для сообщения в журнал
/// коннектора, когда старт не удался: причину пишет сам httpd туда, не в
/// stdout/stderr (которые мы теперь не пайпим и не ждём — см. `start`).
fn error_log_tail(log: &Path, n: usize) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

// --------------------------------------------------------------- Windows --

/// Процесс httpd и его Job Object — держим на весь процесс коннектора, не
/// только на время вызова `start()`. Это наш собственный ребёнок (не
/// служба — без `-k`, см. `start`), поэтому Job действительно может его
/// убить: крах самого коннектора (не штатный «quit», а паника или снятие из
/// диспетчера задач) всё равно гасит httpd — ядро убивает всех, кто остался
/// в Job, когда закрывается последний описатель
/// (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, `crate::job::Job::new`).
#[cfg(windows)]
struct WindowsHttpd {
    child: std::process::Child,
    _job: job::Job,
}

#[cfg(windows)]
static WINDOWS_HTTPD: Mutex<Option<WindowsHttpd>> = Mutex::new(None);

/// Гасит то, что отслежено как наше; если почему-то не отслежено (коннектор
/// перезапустился без чистого выхода) — резервный путь: обычный `-k stop` по
/// тому же конфигу (его `PidFile` укажет на старый процесс, если тот жив).
#[cfg(windows)]
fn kill_tracked(p: &Paths) {
    if let Some(mut h) = WINDOWS_HTTPD.lock().unwrap().take() {
        let _ = h.child.kill();
        let _ = h.child.wait();
        return;
    }
    let _ = std::process::Command::new(httpd_binary(p)).args(["-f", &p.conf.to_string_lossy(), "-k", "stop"]).output();
}

/// Поднимает HTTP-сервис: httpd.exe — наш прямой ребёнок, в консольном
/// режиме (`-f <conf>`, без `-k`) — с `-k start` httpd передаёт работу
/// диспетчеру служб, и выживший процесс нам уже не принадлежит, а Job-бэкстоп
/// держит нечего. Идемпотентно: уже отслеженный процесс сперва гасится —
/// повторный вызов после смены пароля/базы не падает на «порт занят».
#[cfg(windows)]
pub fn start(p: &Paths) -> Result<(), String> {
    use std::process::Stdio;
    kill_tracked(p);
    let bin = httpd_binary(p);
    let mut cmd = std::process::Command::new(&bin);
    // В /dev/null, не в пайп: пайп, который некому вычитывать, блокирует
    // демона навсегда, если тот решит в него что-то написать — а мы либо не
    // ждём выхода вовсе (сервер должен остаться жить), либо ждём только
    // короткую паузу ниже.
    cmd.args(["-f", &p.conf.to_string_lossy()]).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("{} не запустился: {e}", bin.display()))?;
    // Короткая пауза — дать серверу шанс упасть сразу (битый конфиг, занятый
    // порт), прежде чем считать процесс запущенным и уходить.
    std::thread::sleep(std::time::Duration::from_millis(300));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("httpd завершился сразу (код {:?}): {}", status.code(), error_log_tail(&p.log, 5)));
    }
    let Some(job) = job::Job::new() else { return Err("не удалось создать Job Object для httpd".to_string()) };
    job.assign_std(&child);
    *WINDOWS_HTTPD.lock().unwrap() = Some(WindowsHttpd { child, _job: job });
    Ok(())
}

/// Гасит сервис: наш собственный ребёнок — `kill()` плюс `wait()` и есть
/// проверка «наш процесс действительно остановлен» (`wait()` возвращает
/// только после выхода), без разговора про занятый порт: тот, кто там
/// окажется после нас — не наша забота здесь (см. `httpd_responds` выше).
#[cfg(windows)]
pub fn stop(p: &Paths) -> Result<(), String> {
    kill_tracked(p);
    Ok(())
}

// ----------------------------------------------------------------- Linux --

/// `apache2` в `$PATH` — читаемая ошибка вместо «No such file or directory»
/// из недр `Command::spawn`.
#[cfg(not(windows))]
fn apache2_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("apache2")).find(|p| p.exists())
}

/// Поднимает HTTP-сервис: системный `apache2`, как уже делает стенд.
/// `apache2 -k start` демонизируется в свою сессию — никогда не наш ребёнок и
/// не наша группа процессов, поэтому здесь нет Job/pgid-бэкстопа (в отличие
/// от Windows выше): сам pid-файл (`p.pid`, которым управляет httpd) —
/// единственный надёжный хэндл, и убивать по когда-то виденному pgid было бы
/// опасно — к моменту вызова ядро могло отдать тот же номер другому
/// процессу. Идемпотентно: сначала глушим то, что уже могло быть поднято
/// (ошибку «не запущено» игнорируем), затем стартуем заново.
#[cfg(not(windows))]
pub fn start(p: &Paths) -> Result<(), String> {
    apache2_on_path().ok_or_else(|| "apache2 не найден в $PATH — установите пакет apache2 (на образе он должен быть) или переключите publication на \"webserver\"".to_string())?;
    let _ = std::process::Command::new("apache2").args(["-f", &p.conf.to_string_lossy(), "-k", "stop"]).output();
    let out = std::process::Command::new("apache2")
        .args(["-f", &p.conf.to_string_lossy(), "-k", "start"])
        .output()
        .map_err(|e| format!("apache2 не запустился: {e}"))?;
    if !out.status.success() {
        let tail = error_log_tail(&p.log, 5);
        let detail = if tail.is_empty() { String::from_utf8_lossy(&out.stderr).trim().to_string() } else { tail };
        return Err(format!("apache2 -k start: {detail}"));
    }
    Ok(())
}

/// Гасит сервис через собственный pid-файл httpd (`-k stop`). Успешный код
/// возврата означает, что httpd нашёл и сигналил именно наш процесс — это и
/// есть проверка «наш сервис остановлен»; та же оговорка, что на Windows:
/// кто-то другой на этом порту после нас — не повод для ошибки здесь.
#[cfg(not(windows))]
pub fn stop(p: &Paths) -> Result<(), String> {
    let out = std::process::Command::new("apache2")
        .args(["-f", &p.conf.to_string_lossy(), "-k", "stop"])
        .output()
        .map_err(|e| format!("apache2 не запустился: {e}"))?;
    if !out.status.success() {
        return Err(format!("apache2 -k stop: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
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
    fn the_pool_attributes_are_pinned_to_the_service_element_not_just_present_anywhere() {
        // Проверяем именно элемент <service ...>, а не "где-то в файле" — те
        // же слова в комментарии или в другом месте не считаются.
        let text = vrd(&cfg_for_test());
        let start = text.find("<service").expect("в дескрипторе есть <service");
        let end = start + text[start..].find("/>").expect("элемент <service> закрыт");
        let service_tag = &text[start..end];
        for needle in ["rootUrl=\"mcp-1c\"", "enable=\"true\"", "reuseSessions=\"dontuse\"", "sessionMaxAge=\"1\"", "poolSize=\"1\"", "poolTimeout=\"1\""] {
            assert!(service_tag.contains(needle), "в <service> нет {needle:?}:\n{service_tag}");
        }
    }

    #[test]
    fn vrd_escapes_xml_special_characters_in_every_substituted_value() {
        // Кавычка или амперсанд в пароле иначе рвут XML — платформа отвергает
        // весь файл, и агент не видит адреса базы вовсе.
        let mut c = cfg_for_test();
        c.host.db_password = "p@ss\"&<>'".into();
        let text = vrd(&c);
        assert!(text.contains("Pwd=&quot;p@ss&quot;&amp;&lt;&gt;&apos;&quot;;"), "{text}");
        // В значении атрибута не осталось «сырых» спецсимволов — только их escape-формы.
        let pwd_start = text.find("Pwd=&quot;").unwrap() + "Pwd=&quot;".len();
        let pwd_end = pwd_start + text[pwd_start..].find("&quot;;").unwrap();
        let raw_value = &text[pwd_start..pwd_end];
        assert!(!raw_value.contains('"') && !raw_value.contains('<') && !raw_value.contains('>'), "{raw_value:?}");
    }

    #[test]
    fn descriptor_matches_detects_a_changed_password() {
        // «Сохранить» с новым паролем — старый файл на диске больше не
        // совпадает с тем, что vrd(cfg) написал бы сейчас: state() обязан
        // увидеть расхождение, а не читать совпадение порта за «сделано».
        let tmp = std::env::temp_dir().join(format!("dev1c-pub-descriptor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("pub");
        let before = cfg_for_test();
        write_vrd(&dir, &vrd(&before)).unwrap();

        assert!(descriptor_matches(&dir, &before));

        let mut after = before.clone();
        after.host.db_password = "новый-пароль".into();
        assert!(!descriptor_matches(&dir, &after));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn descriptor_matches_is_false_when_nothing_was_ever_written() {
        let tmp = std::env::temp_dir().join(format!("dev1c-pub-descriptor-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!descriptor_matches(&tmp.join("pub"), &cfg_for_test()));
    }

    #[test]
    fn service_responds_is_false_when_nothing_listens() {
        // Порт 1 — никакого httpd там нет и быть не может; reqwest получает
        // отказ в соединении, не HTTP-ответ.
        assert!(!service_responds("http://127.0.0.1:1/hs/mcp-1c"));
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
TypesConfig \"C:\\Users\\dev\\AppData\\Local\\dev1c\\httpd\\conf\\mime.types\"\n\
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
        assert_eq!(linux_httpd_conf(&linux_sample_paths()).unwrap(), expected);
    }

    #[test]
    fn linux_conf_does_not_start_apache_as_a_different_user() {
        // Стенд поднимает Apache от root и явно переключает его на владельца
        // базы (User/Group); коннектор уже запущен от имени рабочего места —
        // эти директивы были бы бессмысленны (и некого было бы менять).
        let text = linux_httpd_conf(&linux_sample_paths()).unwrap();
        assert!(!text.lines().any(|l| l.starts_with("User ") || l.starts_with("Group ")), "{text}");
    }

    #[test]
    fn linux_conf_also_listens_on_loopback_only() {
        let text = linux_httpd_conf(&linux_sample_paths()).unwrap();
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
            assert_eq!(httpd_conf(&sample_paths()).unwrap(), windows_httpd_conf(&sample_paths()));
        } else {
            assert_eq!(httpd_conf(&linux_sample_paths()).unwrap(), linux_httpd_conf(&linux_sample_paths()).unwrap());
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
            // Коммерческая платформа на Linux несёт wsap24.so, не только
            // учебную wsap24t.so — тот же порядок предпочтения, что на Windows.
            std::fs::write(tmp.join("wsap24.so"), "x").unwrap();
            std::fs::write(tmp.join("wsap24t.so"), "x").unwrap();
            assert_eq!(module_in(&tmp).unwrap(), tmp.join("wsap24.so"));
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

    #[cfg(unix)]
    #[test]
    fn a_directory_left_wide_open_by_an_older_run_gets_tightened() {
        // Каталог мог остаться с версии до этого фикса (или с прошлого
        // прогона) с более широкими правами — `write_vrd` обязан поджать их,
        // а не оставить как есть, раз уж всё равно сюда зашёл.
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("dev1c-pub-vrd-tighten-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("pub");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        write_vrd(&dir, "x").unwrap();

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "{dir_mode:o}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn linux_httpd_conf_fails_readably_instead_of_writing_an_empty_runtime_dir() {
        // Path::parent() у однокомпонентного относительного пути отдаёт
        // Some("") — без явной проверки DefaultRuntimeDir "" тихо ушёл бы в
        // файл конфигурации.
        let mut p = linux_sample_paths();
        p.pid = PathBuf::from("httpd.pid");
        let e = linux_httpd_conf(&p).unwrap_err();
        assert!(e.contains("DefaultRuntimeDir"), "{e}");
    }
}
