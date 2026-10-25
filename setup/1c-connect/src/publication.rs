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
#[cfg(not(windows))]
use crate::holder;
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
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
pub(crate) fn module_in(bin: &Path) -> Result<PathBuf, String> {
    let names: &[&str] = if cfg!(windows) { &["wsap24.dll", "wsap24t.dll"] } else { &["wsap24.so", "wsap24t.so"] };
    names
        .iter()
        .map(|n| bin.join(n))
        .find(|p| p.exists())
        .ok_or_else(|| format!("рядом с платформой в {} нет модуля веб-сервера 1С ({})", bin.display(), names.join("/")))
}

/// Пути, зависящие только от каталога состояния коннектора (`state`) — не от
/// настоящей платформы 1С: вынесены отдельно, чтобы тест считал их с
/// временным каталогом, а не с настоящим `%LOCALAPPDATA%`/`$HOME`.
fn paths_under(state: &Path, root: PathBuf, module: PathBuf, port: u16) -> Paths {
    Paths {
        root,
        pub_dir: state.join("pub"),
        conf: state.join("httpd.conf"),
        pid: state.join("httpd.pid"),
        log: state.join("httpd.log"),
        module,
        port,
    }
}

/// Две основы публикации, и путать их нельзя: откуда httpd ЗАПУСКАЕТСЯ и куда
/// публикация ПИШЕТ.
///
/// Запускается — из комплекта (`<комплект>/httpd` на Windows, системный
/// apache2 в образе): инструменты работают с места, в профиль не переносится
/// ничего (леджер комплекта, пункт 15). Пишет — только в состояние
/// пользователя, и это не вкусовщина: в `default.vrd` лежит строка
/// подключения с логином и паролем базы (`vrd` выше, пишется «только
/// владельцу»). В комплекте ему нельзя по двум причинам сразу — комплект
/// переезжает на флешке и в общей папке, а в стенде он один на три рабочих
/// места и только для чтения, так что запись туда либо не удастся, либо
/// отдаст пароль одного человека всем трём.
///
/// Чистая функция ровно затем, чтобы это держал тест: `Paths::of` ниже
/// проверить нечем — ему нужна настоящая платформа на диске.
pub(crate) fn bases(kit_root: &Path, state: &Path) -> (PathBuf, PathBuf) {
    let server = if cfg!(windows) { kit_root.join("httpd") } else { PathBuf::from("/etc/apache2") };
    (server, state.to_path_buf())
}

impl Paths {
    /// Пути публикации на этой машине: `ServerRoot` — свой httpd из комплекта
    /// (`<комплект>/httpd`, запускается оттуда, где лежит) на Windows,
    /// системный apache2 в образе — то, что уже делает стенд. Всё, что
    /// публикация пишет, — в состоянии пользователя; почему именно так,
    /// сказано у `bases`.
    pub fn of(cfg: &Cfg, kit: &Kit) -> Result<Paths, String> {
        let (root, state) = bases(&kit.root, &ops_host::state_dir());
        if cfg!(windows) && !root.join("bin").join("httpd.exe").exists() {
            return Err(format!("в {} нет bin/httpd.exe — скопируйте полный комплект заново (комплект: {})", root.display(), kit.root.display()));
        }
        let exe = ops_host::find_1cv8(cfg).ok_or_else(|| "платформа 1С не найдена — установите её или укажите переменную ONEC_PLATFORM".to_string())?;
        let bin = exe.parent().ok_or_else(|| format!("у {} нет каталога", exe.display()))?;
        let module = module_in(bin)?;
        Ok(paths_under(&state, root, module, cfg.http_port()))
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

/// Время жизни поднятого httpd относительно вызвавшего процесса.
///
/// `Detached` — публикация обязана пережить того, кто её поднял: разовая
/// команда `1c-connect install` (или кнопка «Установить» в окне) делает своё
/// дело и выходит/продолжает жить дальше, но сервис — это публикация базы, а
/// не ребёнок установщика, и её не касается ни штатный выход процесса, ни его
/// крах.
///
/// `WithCaller` — сервис обязан умереть вместе с вызвавшим процессом: `serve`
/// держит сеанс до Ctrl+C/SIGTERM, окно — пока открыто, и падение любого из
/// них (не штатный выход, а крах) не должно оставлять сироту на порту со
/// старой конфигурацией, которую больше никто не обслуживает.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    Detached,
    WithCaller,
}

/// Правило владения: сервис, который уже отвечает под сегодняшней
/// конфигурацией, уже у кого-то на привязи — у этого же процесса
/// (`WithCaller`, живое окно или `serve`), у другого окна, у `serve` на
/// соседнем процессе — а разовая `Detached`-команда не может заглянуть в
/// чужую память и узнать, чья это привязка. Поэтому `Detached`, найдя живой
/// и верно настроенный сервис, не трогает его вовсе: убить и поднять заново
/// отвязанным сняло бы привязку впустую, и крах настоящего владельца потом
/// оставил бы ровно того сироту, которого привязка должна предотвращать.
/// `WithCaller` же всегда забирает сервис под себя — это и есть становление
/// владельцем, а не чужая кража.
pub fn detached_should_leave_alone(lifetime: Lifetime, responds: bool, descriptor_matches: bool) -> bool {
    lifetime == Lifetime::Detached && responds && descriptor_matches
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
/// Запуск без консольного окна: httpd — обычное консольное приложение, и
/// без этого флага каждый его запуск и остановка открывают человеку чёрное
/// окно поверх всего. Тот же приём, что у `common::docker::quiet`.
#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Гасит отслеженный httpd (или просит его остановиться), прежде чем
/// поднимать новый. Только Windows: на Linux остановкой ведает `-k stop`
/// по своему файлу идентификатора.
#[cfg(windows)]
fn kill_tracked(p: &Paths) {
    if let Some(mut h) = WINDOWS_HTTPD.lock().unwrap().take() {
        let _ = h.child.kill();
        let _ = h.child.wait();
        return;
    }
    let mut stop = std::process::Command::new(httpd_binary(p));
    stop.args(["-f", &p.conf.to_string_lossy(), "-k", "stop"]);
    #[cfg(windows)]
    no_window(&mut stop);
    let _ = stop.output();
}

/// Поднимает HTTP-сервис: httpd.exe — наш прямой ребёнок, в консольном
/// режиме (`-f <conf>`, без `-k`) — с `-k start` httpd передаёт работу
/// диспетчеру служб, и выживший процесс нам уже не принадлежит, а Job-бэкстоп
/// держит нечего. Идемпотентно: уже отслеженный процесс сперва гасится —
/// повторный вызов после смены пароля/базы не падает на «порт занят».
///
/// `lifetime` решает, что происходит с httpd, когда этот процесс
/// заканчивается (штатно или крахом): `Detached` — ничего, сервис остаётся
/// жить сам по себе (`child` просто роняется — `Drop` у `std::process::Child`
/// закрывает только наш хэндл, самого процесса не касается); `WithCaller` —
/// кладём в Job Object с `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, и уход
/// последнего хэндла (конец этого процесса) гасит httpd вместе с ним.
#[cfg(windows)]
pub fn start(p: &Paths, lifetime: Lifetime) -> Result<(), String> {
    use std::process::Stdio;
    kill_tracked(p);
    let bin = httpd_binary(p);
    let mut cmd = std::process::Command::new(&bin);
    // В /dev/null, не в пайп: пайп, который некому вычитывать, блокирует
    // демона навсегда, если тот решит в него что-то написать — а мы либо не
    // ждём выхода вовсе (сервер должен остаться жить), либо ждём только
    // короткую паузу ниже.
    cmd.args(["-f", &p.conf.to_string_lossy()]).stdout(Stdio::null()).stderr(Stdio::null());
    no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("{} не запустился: {e}", bin.display()))?;
    // Короткая пауза — дать серверу шанс упасть сразу (битый конфиг, занятый
    // порт), прежде чем считать процесс запущенным и уходить.
    std::thread::sleep(std::time::Duration::from_millis(300));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("httpd завершился сразу (код {:?}): {}", status.code(), error_log_tail(&p.log, 5)));
    }
    match lifetime {
        Lifetime::Detached => {
            // Публикация базы, не ребёнок установщика: намеренно не кладём
            // процесс ни в Job, ни в WINDOWS_HTTPD — выход этого процесса (в
            // т.ч. крах) httpd не касается. Остановить его позже сможет
            // kill_tracked() через резервный `-k stop` по тому же конфигу.
            drop(child);
        }
        Lifetime::WithCaller => {
            let Some(job) = job::Job::new() else { return Err("не удалось создать Job Object для httpd".to_string()) };
            job.assign_std(&child);
            *WINDOWS_HTTPD.lock().unwrap() = Some(WindowsHttpd { child, _job: job });
        }
    }
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
pub(crate) fn apache2_on_path() -> Option<PathBuf> {
    // Не только `$PATH`: apache2 лежит в `/usr/sbin`, а у окна, запущенного
    // рабочим столом, этого каталога в `$PATH` нет вовсе — сеанс xrdp даёт
    // `/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games` (замер на
    // стенде 2026-10-21). Человек при этом читал «установите пакет
    // apache2», хотя пакет стоял: `/usr/sbin/apache2` был на месте, просто
    // мы смотрели не туда. Системные каталоги службы проверяем сами —
    // программа там и живёт, а `$PATH` у графического сеанса ей не указ.
    let from_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|d| d.join("apache2"));
    let system = ["/usr/sbin", "/usr/local/sbin", "/sbin"].into_iter().map(|d| Path::new(d).join("apache2"));
    from_path.chain(system).find(|p| p.exists())
}

/// Текст отказа подъёма сервиса из вывода apache2: сам httpd говорит по-английски
/// и ни слова не говорит о базе — переводим суть на русский. Когда причина
/// действительно конфликт порта (apache2 метит её фирменным «Address already
/// in use»), называем порт и, если видно, его держателя (`holder::of_port` —
/// параметром, а не вызовом внутри: так текст проверяется без реального
/// занятого порта); иначе — только то, что сказал сам apache2. Текст apache2
/// остаётся в хвосте сообщения целиком — для поддержки, уже после того, как
/// человек прочитал по-русски, что случилось.
#[cfg(not(windows))]
fn start_failure_text(port: u16, apache_text: &str, holder: Option<&str>) -> String {
    if apache_text.contains("Address already in use") {
        let who = holder.map(|h| format!(" ({h})")).unwrap_or_default();
        format!("HTTP-сервис базы не поднят: порт 127.0.0.1:{port} уже занят{who} — {apache_text}")
    } else {
        format!("HTTP-сервис базы не поднят: {apache_text}")
    }
}

/// Поднимает HTTP-сервис: системный `apache2`, как уже делает стенд.
/// `apache2 -k start` демонизируется в свою сессию — никогда не наш ребёнок и
/// не наша группа процессов, поэтому здесь нет Job/pgid-бэкстопа (в отличие
/// от Windows выше): сам pid-файл (`p.pid`, которым управляет httpd) —
/// единственный надёжный хэндл, и убивать по когда-то виденному pgid было бы
/// опасно — к моменту вызова ядро могло отдать тот же номер другому
/// процессу. Идемпотентно: сначала глушим то, что уже могло быть поднято
/// (ошибку «не запущено» игнорируем), затем стартуем заново.
///
/// `lifetime` здесь ничего не решает (сигнатура — общая с Windows): apache2
/// и так всегда выживает вызвавший процесс, какой бы он ни был — разница
/// Detached/WithCaller — чисто windows-овская, Job Object там не бывает.
#[cfg(not(windows))]
pub fn start(p: &Paths, _lifetime: Lifetime) -> Result<(), String> {
    apache2_on_path().ok_or_else(|| "apache2 не найден ни в $PATH, ни в /usr/sbin, /usr/local/sbin, /sbin — установите пакет apache2 или переключите publication на \"webserver\"".to_string())?;
    let _ = std::process::Command::new("apache2").args(["-f", &p.conf.to_string_lossy(), "-k", "stop"]).output();
    // Дать прежнему серверу отпустить порт. Без этой паузы `-k start` видит
    // ещё живой pid, печатает «httpd (pid N) already running», ВЫХОДИТ С
    // НУЛЁМ — и ничего не запускает; через секунду прежний дожил своё, и на
    // порту пусто. Ровно это заказчик и поймал 25.10, переключив
    // обслуживание дважды подряд: в журнале «HTTP-сервис базы поднят», а
    // порт мёртвый.
    wait_until(STOP_WAIT, || !port_listens(p.port));
    let out = std::process::Command::new("apache2")
        .args(["-f", &p.conf.to_string_lossy(), "-k", "start"])
        .output()
        .map_err(|e| format!("apache2 не запустился: {e}"))?;
    if !out.status.success() {
        let tail = error_log_tail(&p.log, 5);
        let detail = if tail.is_empty() { String::from_utf8_lossy(&out.stderr).trim().to_string() } else { tail };
        let apache_text = format!("apache2 -k start: {detail}");
        return Err(start_failure_text(p.port, &apache_text, holder::of_port(p.port).as_deref()));
    }
    // Код возврата доказательством не считаем — он нулевой и у «already
    // running», и у запуска, который упадёт секундой позже (битый модуль,
    // занятый порт: apache к этому времени уже ушёл в фон). Доказательство
    // одно — на порту отвечают.
    if !wait_until(START_WAIT, || port_listens(p.port)) {
        let tail = error_log_tail(&p.log, 5);
        let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if tail.is_empty() { said } else { format!("{said} {tail}").trim().to_string() };
        let apache_text = format!("apache2 -k start отработал, но на 127.0.0.1:{} никто не отвечает: {detail}", p.port);
        return Err(start_failure_text(p.port, &apache_text, holder::of_port(p.port).as_deref()));
    }
    Ok(())
}

/// Сколько ждать, пока прежний сервер отпустит порт, и сколько — пока новый
/// на нём отзовётся. Секунды, а не доли: apache уходит и приходит не мгновенно,
/// а человек всё равно смотрит на «включаю MCP».
#[cfg(not(windows))]
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(not(windows))]
const START_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Слушает ли кто-нибудь порт на loopback. Вопрос именно «слушает», а не
/// «отвечает ли 1С»: сервер поднимается раньше, чем платформа открывает базу
/// (первый ответ по HTTP на стенде — 7,7 с, замер 25.10), и ждать базу здесь
/// значило бы держать человека ради того, что проверит факт готовности.
#[cfg(not(windows))]
fn port_listens(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), std::time::Duration::from_millis(300)).is_ok()
}

/// Ждать выполнения условия до срока, опрашивая его каждые 200 мс. `true` —
/// дождались. Чистая по отношению к условию: тест подаёт своё.
#[cfg(not(windows))]
fn wait_until(limit: std::time::Duration, mut ready: impl FnMut() -> bool) -> bool {
    let until = std::time::Instant::now() + limit;
    loop {
        if ready() {
            return true;
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
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

    /// Пароль базы не уезжает в комплект. `default.vrd` несёт строку
    /// подключения с `Usr`/`Pwd`, а комплект переезжает на флешке и в общей
    /// папке, в стенде же он один на три рабочих места и только для чтения.
    /// Поэтому httpd запускается ИЗ комплекта, а пишет публикация только в
    /// состояние — ровно это и держит тест. Живой случай: 25.10, уводя
    /// инструменты в комплект, я увёл туда заодно и дескриптор с паролем.
    /// Ожидание условия — единственная часть подъёма, которую можно
    /// проверить без apache: дождались, не дождались, и условие спрашивают
    /// повторно, а не один раз.
    #[cfg(not(windows))]
    #[test]
    fn waiting_returns_as_soon_as_the_condition_holds_and_gives_up_on_time() {
        use std::time::{Duration, Instant};
        let mut calls = 0;
        assert!(super::wait_until(Duration::from_secs(1), || {
            calls += 1;
            calls >= 2
        }));
        assert_eq!(calls, 2, "условие обязано спрашиваться заново, а не один раз");

        let t0 = Instant::now();
        assert!(!super::wait_until(Duration::from_millis(400), || false));
        assert!(t0.elapsed() >= Duration::from_millis(400), "срок не выдержан");
        assert!(t0.elapsed() < Duration::from_secs(3), "ждали дольше срока: {:?}", t0.elapsed());
    }

    #[test]
    fn the_descriptor_with_the_password_never_lands_in_the_kit() {
        let kit = Path::new("K:\\kit");
        let state = Path::new("S:\\state");
        let (server, writes) = bases(kit, state);
        assert_eq!(writes, state, "писать публикация обязана в состояние");
        let p = paths_under(&writes, server.clone(), PathBuf::from("P:\\wsap24t.dll"), 8083);
        for path in [&p.pub_dir, &p.conf, &p.pid, &p.log] {
            assert!(path.starts_with(state), "{} должен лежать в состоянии", path.display());
            assert!(!path.starts_with(kit), "{} уехал в комплект", path.display());
        }
        // А вот сам httpd — из комплекта: его мы никуда не копируем.
        if cfg!(windows) {
            assert_eq!(server, kit.join("httpd"));
        } else {
            assert_eq!(server, PathBuf::from("/etc/apache2"));
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
    fn detached_leaves_a_live_owners_server_alone_but_with_caller_always_takes_over() {
        // Живой владелец, верная конфигурация — разовая команда не трогает.
        assert!(detached_should_leave_alone(Lifetime::Detached, true, true));
        // Отвечает, но под старым дескриптором (пароль/база сменились) —
        // разовая команда обязана перезапустить, а не оставить стоять старое.
        assert!(!detached_should_leave_alone(Lifetime::Detached, true, false));
        // Ничего не отвечает — разовой команде есть что поднимать.
        assert!(!detached_should_leave_alone(Lifetime::Detached, false, true));
        // WithCaller становится владельцем сам — правило его не касается.
        assert!(!detached_should_leave_alone(Lifetime::WithCaller, true, true));
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

    #[cfg(not(windows))]
    #[test]
    fn start_failure_names_the_port_and_the_holder_when_known() {
        // Ровно живой случай из отчёта: контейнерный apache2 уже держит порт.
        let apache_text = "apache2 -k start: (98)Address already in use: AH00072: make_sock: could not bind to address 127.0.0.1:8083\nno listening sockets available, shutting down";
        let msg = start_failure_text(8083, apache_text, Some("apache2 (pid 456)"));
        assert!(msg.contains("порт 127.0.0.1:8083"), "{msg}");
        assert!(msg.contains("уже занят"), "{msg}");
        assert!(msg.contains("apache2 (pid 456)"), "{msg}");
        // Текст apache2 остаётся в хвосте целиком — для поддержки.
        assert!(msg.ends_with(apache_text), "{msg}");
    }

    #[cfg(not(windows))]
    #[test]
    fn start_failure_without_a_known_holder_still_names_the_port() {
        let apache_text = "apache2 -k start: (98)Address already in use: AH00072: make_sock: could not bind to address 127.0.0.1:8083";
        let msg = start_failure_text(8083, apache_text, None);
        assert!(msg.contains("порт 127.0.0.1:8083") && msg.contains("уже занят"), "{msg}");
        assert!(!msg.contains("()"), "держатель неизвестен — пустых скобок быть не должно: {msg}");
    }

    #[cfg(not(windows))]
    #[test]
    fn start_failure_for_another_cause_keeps_apaches_own_text_without_claiming_a_busy_port() {
        let apache_text = "apache2 -k start: Syntax error on line 5 of httpd.conf";
        let msg = start_failure_text(8083, apache_text, Some("apache2 (pid 456)"));
        assert!(!msg.contains("уже занят"), "{msg}");
        assert!(msg.contains(apache_text), "{msg}");
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
