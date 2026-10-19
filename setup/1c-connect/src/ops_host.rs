//! Операции на машине с 1С: окружение и расширение в базе. Туннель и мост —
//! не операции с состоянием, а процесс `connect`.

use crate::cfg::Cfg;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use common::docker;
use common::kit::Kit;
use common::op::{Action, CleanLevel, Op, OpState};
use common::{Phase, Prompt, Runner, Sink};

pub struct Ctx {
    pub kit: Kit,
    pub cfg: Cfg,
    pub dry_run: bool,
    pub sink: Sink,
    pub prompt: Box<dyn Prompt>,
}

impl Runner for Ctx {
    fn dry_run(&self) -> bool {
        self.dry_run
    }
    fn sink(&self) -> &Sink {
        &self.sink
    }
    fn kit(&self) -> &Kit {
        &self.kit
    }
}

/// Каталог инструментов на машине: с носителя комплекта (флешка, FAT32,
/// Defender на каждый запуск) 48 МБ .NET не запускают.
pub fn tools_dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into())).join("dev1c")
    } else {
        match std::env::var_os("XDG_DATA_HOME") {
            Some(d) => PathBuf::from(d).join("dev1c"),
            None => PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".local/share/dev1c"),
        }
    }
}

/// OneScript лежит как в своём архиве: `bin/` и `lib/` рядом, `oscript.cfg`
/// говорит `lib.system = ../lib` — библиотеки (и `lib/add` для vrunner)
/// находятся без переменных окружения.
pub fn oscript_exe(tools: &Path) -> PathBuf {
    tools.join("vrunner").join("bin").join(if cfg!(windows) { "oscript.exe" } else { "oscript" })
}

pub fn vrunner_lib(tools: &Path) -> PathBuf {
    tools.join("vrunner").join("lib")
}

pub fn git_exe(tools: &Path) -> PathBuf {
    if cfg!(windows) {
        tools.join("git").join("cmd").join("git.exe")
    } else {
        tools.join("git").join("bin").join("git")
    }
}

pub enum Gitea {
    Down(String),
    NoOrg,
    Ok(String),
}

/// Gitea среды напрямую (без туннеля): версия и организация dev1c.
pub fn gitea_probe(url: &str) -> Gitea {
    let c = match reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(5)).build() {
        Ok(c) => c,
        Err(e) => return Gitea::Down(e.to_string()),
    };
    let version = match c.get(format!("{url}/api/v1/version")).send() {
        Ok(r) if r.status().is_success() => r.text().unwrap_or_default(),
        Ok(r) => return Gitea::Down(format!("HTTP {}", r.status())),
        Err(e) => return Gitea::Down(e.to_string()),
    };
    // {"version":"1.22.3"} — without reqwest's json feature.
    let version = version.split('"').nth(3).unwrap_or("?").to_string();
    match c.get(format!("{url}/api/v1/orgs/dev1c")).send() {
        Ok(r) if r.status().as_u16() == 404 => Gitea::NoOrg,
        _ => Gitea::Ok(version),
    }
}

/// Что не так с носителем комплекта: съёмный или FAT/exFAT (Defender на
/// каждый запуск, SmartScreen). Ничего — `None`.
#[cfg(windows)]
pub fn media_kind(path: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW};
    // DRIVE_REMOVABLE lives in Win32_System_WindowsProgramming; one feature for one constant is not worth it.
    const DRIVE_REMOVABLE: u32 = 2;
    let s = path.to_string_lossy();
    let drive = s.get(..3).filter(|d| d.as_bytes().get(1) == Some(&b':')).unwrap_or("C:\\");
    let root: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if GetDriveTypeW(root.as_ptr()) == DRIVE_REMOVABLE {
            return Some("съёмный носитель".into());
        }
        let mut fs = [0u16; 64];
        let ok = GetVolumeInformationW(root.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32);
        if ok != 0 {
            let name = String::from_utf16_lossy(&fs).trim_end_matches('\0').to_string();
            if name.starts_with("FAT") || name == "exFAT" {
                return Some(name);
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn media_kind(_path: &Path) -> Option<String> {
    None
}

pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            n += copy_tree(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), &dst)?;
            n += 1;
        }
    }
    Ok(n)
}

const VERSION_FILE: &str = "tools.version";

/// Состояние по отметке версии: комплект пишет `tools.version` (pack.sh),
/// копия на машине несёт ту же строку.
pub fn tools_state(kit_root: &Path, tools: &Path) -> OpState {
    if !kit_root.join("vrunner").is_dir() || !kit_root.join("git").is_dir() {
        return OpState::Blocked("в комплекте нет vrunner/ или git/ — скопируйте полный комплект заново".into());
    }
    let want = std::fs::read_to_string(kit_root.join(VERSION_FILE)).unwrap_or_default();
    let have = std::fs::read_to_string(tools.join(VERSION_FILE)).unwrap_or_default();
    if have.trim().is_empty() {
        OpState::Missing
    } else if have.trim() == want.trim() {
        OpState::Done(format!("{} в {}", want.trim(), tools.display()))
    } else {
        OpState::Drifted { from: have.trim().to_string(), to: want.trim().to_string() }
    }
}

struct Tools;
impl Op<Ctx> for Tools {
    fn id(&self) -> &'static str {
        "host.tools"
    }
    fn title(&self) -> &'static str {
        "Инструменты комплекта на машине"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        tools_state(&ctx.kit.root, &tools_dir())
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!("скопировать vrunner/ и git/ из {} в {} (≈ 100 МБ)", ctx.kit.root.display(), tools_dir().display()))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let to = tools_dir();
        if ctx.dry_run {
            return Ok(());
        }
        for d in ["vrunner", "git"] {
            let _ = std::fs::remove_dir_all(to.join(d));
            let n = copy_tree(&ctx.kit.root.join(d), &to.join(d)).with_context(|| format!("копирование {d} в {}", to.display()))?;
            ctx.ok(format!("{d}: {n} файлов"));
        }
        let o = docker::probe(ctx, &oscript_exe(&to).to_string_lossy(), &["-version"])?;
        anyhow::ensure!(o.ok(), "oscript не запускается из {}: {}", to.display(), o.tail(2).join(" "));
        ctx.ok(format!("oscript {}", o.text.trim()));
        let o = docker::probe(ctx, &git_exe(&to).to_string_lossy(), &["--version"])?;
        anyhow::ensure!(o.ok(), "git не запускается из {}: {}", to.display(), o.tail(2).join(" "));
        ctx.ok(o.text.trim().to_string());
        std::fs::copy(ctx.kit.root.join(VERSION_FILE), to.join(VERSION_FILE)).context("отметка версии")?;
        if cfg!(windows) {
            let exe = find_1cv8();
            if let Some(platform) = exe.as_deref().and_then(|exe| exe.parent()).and_then(|bin| bin.parent()) {
                match shadow_platform(platform, &to) {
                    Ok(shadow_dir) if shadow_dir.starts_with(&to) => match add_installed_location(&one_c_estart_cfg(), &shadow_dir) {
                        Ok(()) => ctx.ok("учебная платформа зарегистрирована в 1С под обычным именем".to_string()),
                        Err(e) => ctx.warn(format!("путь к платформе не записан в список версий 1С: {e}")),
                    },
                    Ok(_) => {} // платформа уже обычная — тень не нужна
                    Err(e) => ctx.warn(format!("не удалось подготовить платформу к запуску: {e}")),
                }
            }
        }
        Ok(())
    }
    fn revert(&self, _ctx: &Ctx, level: CleanLevel) -> Option<Vec<Action>> {
        matches!(level, CleanLevel::Data).then(|| vec![Action::new(format!("удалить {}", tools_dir().display()))])
    }
    fn revert_apply(&self, ctx: &Ctx, level: CleanLevel) -> Result<()> {
        if matches!(level, CleanLevel::Data) {
            let _ = std::fs::remove_dir_all(tools_dir());
            ctx.ok("инструменты удалены с машины");
        }
        Ok(())
    }
}

pub fn mcp_exe(kit: &Kit) -> std::path::PathBuf {
    kit.path(if cfg!(windows) { "mcp-1c-windows-amd64.exe" } else { "mcp-1c-linux-amd64" })
}

/// Переменная окружения, которая целиком перекрывает поиск платформы: указывает
/// каталог, где лежит исполняемый файл (а не каталог версии).
const ONEC_PLATFORM: &str = "ONEC_PLATFORM";

/// Имена исполняемого файла платформы: обычная и учебная версия (у учебной — суффикс `t`).
fn platform_exe_names() -> [&'static str; 2] {
    if cfg!(windows) { ["1cv8.exe", "1cv8t.exe"] } else { ["1cv8", "1cv8t"] }
}

/// Подкаталог версии, где лежит исполняемый файл: на Windows — `bin`, на Linux
/// файл лежит прямо в каталоге версии.
fn version_exe_dir(version_dir: &Path) -> PathBuf {
    if cfg!(windows) { version_dir.join("bin") } else { version_dir.to_path_buf() }
}

/// Версия как число по компонентам: текстовое сравнение "8.3.9.1" ставит выше "8.3.27.1786".
fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Выбирает путь к новейшей версии из найденных (версия, путь к исполняемому файлу).
/// Чистая функция — тесты подают список руками, платформа на диске не нужна.
pub fn pick_version(found: Vec<(String, PathBuf)>) -> Option<PathBuf> {
    found.into_iter().max_by_key(|(v, _)| version_key(v)).map(|(_, p)| p)
}

/// Один и тот же файл может быть найден по двум путям (в образе `/opt/1cv8` —
/// ссылка на `/opt/1cv8t`): сворачиваем по `canonicalize`.
fn dedup_canonical(found: Vec<(String, PathBuf)>) -> Vec<(String, PathBuf)> {
    let mut seen = std::collections::HashSet::new();
    found.into_iter().filter(|(_, p)| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))).collect()
}

/// Обходит один корень: подкаталоги — версии, в каждой ищем оба имени исполняемого файла.
fn scan_root(root: &Path, found: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.filter_map(|e| e.ok()) {
        let version = e.file_name().to_string_lossy().to_string();
        let dir = version_exe_dir(&e.path());
        for name in platform_exe_names() {
            let p = dir.join(name);
            if p.exists() {
                found.push((version.clone(), p));
            }
        }
    }
}

fn scan_1cv8() -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    if cfg!(windows) {
        for base in ["C:\\Program Files\\1cv8", "C:\\Program Files (x86)\\1cv8", "C:\\Program Files\\1cv8t", "C:\\Program Files (x86)\\1cv8t"] {
            scan_root(Path::new(base), &mut found);
        }
    } else {
        scan_root(Path::new("/opt/1cv8/x86_64"), &mut found);
        scan_root(Path::new("/opt/1cv8t/x86_64"), &mut found);
    }
    found
}

/// Исполняемый файл платформы в заданном каталоге (обычная или учебная версия).
/// Чистая функция — каталог ей называет вызывающий, а не переменная окружения.
fn platform_from(dir: &Path) -> Option<PathBuf> {
    platform_exe_names().into_iter().map(|n| dir.join(n)).find(|p| p.exists())
}

/// Путь к исполняемому файлу платформы: самая новая версия (обычная или учебная) из
/// стандартных каталогов, либо принудительно — из `ONEC_PLATFORM`.
pub fn find_1cv8() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ONEC_PLATFORM) {
        return platform_from(&PathBuf::from(dir));
    }
    pick_version(dedup_canonical(scan_1cv8()))
}

/// Жёсткая ссылка на файл под новым именем; если том не позволяет ссылки
/// (съёмный диск, другой раздел), копирует файл — не проваливает всю раскладку
/// теневого каталога из-за одного файла.
fn link_or_copy(src: &Path, dst: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(dst);
    if std::fs::hard_link(src, dst).is_err() {
        std::fs::copy(src, dst).map_err(|e| format!("копирование {} в {}: {e}", src.display(), dst.display()))?;
    }
    Ok(())
}

/// Теневой каталог `bin` учебной платформы под именами коммерческой версии:
/// на вход — каталог версии (`…\1cv8t\8.5.1.1150`), на выходе — каталог версии
/// тени (`<to>\platform\8.5.1.1150`) с `bin\1cv8.exe` и `bin\1cv8c.exe`,
/// указывающими (жёсткой ссылкой, либо копией) на `1cv8t.exe`/`1cv8ct.exe`.
/// Если платформа уже коммерческая (рядом есть `1cv8.exe`), тень не нужна —
/// ничего не создаём и возвращаем исходный каталог платформы без изменений.
pub fn shadow_platform(platform: &Path, to: &Path) -> Result<PathBuf, String> {
    let bin = platform.join("bin");
    if bin.join("1cv8.exe").exists() {
        return Ok(platform.to_path_buf());
    }
    let version = platform.file_name().ok_or_else(|| format!("не удалось определить версию платформы из {}", platform.display()))?;
    let shadow_dir = to.join("platform").join(version);
    let shadow_bin = shadow_dir.join("bin");
    std::fs::create_dir_all(&shadow_bin).map_err(|e| format!("каталог {}: {e}", shadow_bin.display()))?;
    let entries = std::fs::read_dir(&bin).map_err(|e| format!("чтение {}: {e}", bin.display()))?;
    for e in entries.filter_map(|e| e.ok()) {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = e.file_name();
        let src = e.path();
        link_or_copy(&src, &shadow_bin.join(&name))?;
        let renamed = match name.to_string_lossy().as_ref() {
            "1cv8t.exe" => Some("1cv8.exe"),
            "1cv8ct.exe" => Some("1cv8c.exe"),
            _ => None,
        };
        if let Some(r) = renamed {
            link_or_copy(&src, &shadow_bin.join(r))?;
        }
    }
    Ok(shadow_dir)
}

/// Список установленных версий платформы у лаунчера 1С (1CEStart.exe): без
/// строки на наш теневой каталог он не предложит его в списке версий.
fn one_c_estart_cfg() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_else(|| ".".into())).join("1C").join("1CEStart").join("1CEStart.cfg")
}

/// Добавляет `InstalledLocation=<location>` в файл лаунчера, если такой строки
/// там ещё нет; остальные строки не трогает. Файла может не быть — тогда
/// создаёт его с единственной строкой.
fn add_installed_location(cfg_path: &Path, location: &Path) -> std::io::Result<()> {
    let line = format!("InstalledLocation={}", location.display());
    let existing = std::fs::read_to_string(cfg_path).unwrap_or_default();
    if existing.lines().any(|l| l == line) {
        return Ok(());
    }
    if let Some(parent) = cfg_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&line);
    out.push('\n');
    std::fs::write(cfg_path, out)
}

/// Ключ подключения конфигуратора: /F для файловой базы, /S для сервер\база.
pub fn ib_switch(base_path: &str) -> String {
    if base_path.contains('\\') && !base_path.contains(':') {
        format!("/S{base_path}")
    } else {
        format!("/F{base_path}")
    }
}

/// Версия расширения, которую отдаёт публикация; пусто — не отвечает.
pub fn extension_version(base_url: &str, user: &str, password: &str) -> Option<String> {
    let c = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(60)).build().ok()?;
    let r = c.get(format!("{base_url}/version")).basic_auth(user, Some(password)).send().ok()?;
    if !r.status().is_success() {
        return None;
    }
    // reqwest's `json` feature is not enabled: parse the body ourselves.
    let v: serde_json::Value = serde_json::from_str(&r.text().ok()?).ok()?;
    v.get("version").and_then(|s| s.as_str()).map(|s| s.to_string())
}

struct Env;
impl Op<Ctx> for Env {
    fn id(&self) -> &'static str {
        "host.env"
    }
    fn title(&self) -> &'static str {
        "Окружение"
    }
    fn always_run(&self) -> bool {
        true
    }
    fn state(&self, _ctx: &Ctx) -> OpState {
        OpState::Missing
    }
    fn plan(&self, _ctx: &Ctx) -> Vec<Action> {
        vec![Action::new("проверить платформу 1С, mcp-1c в комплекте и адрес среды")]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        match find_1cv8() {
            Some(p) => ctx.ok(format!("платформа: {}", p.display())),
            None => anyhow::bail!("платформа 1С не найдена — установите её или укажите переменную ONEC_PLATFORM"),
        }
        let exe = mcp_exe(&ctx.kit);
        anyhow::ensure!(exe.exists(), "в комплекте нет {} — комплект неполный", exe.display());
        let o = docker::probe(ctx, &exe.to_string_lossy(), &["--version"])?;
        anyhow::ensure!(o.ok(), "mcp-1c не запускается: {}", o.tail(2).join(" "));
        ctx.ok(o.text.trim().to_string());
        let addr: Result<Vec<_>, _> = std::net::ToSocketAddrs::to_socket_addrs(&ctx.cfg.host.server.as_str()).map(|a| a.collect());
        anyhow::ensure!(addr.is_ok(), "адрес среды {} не разбирается — проверьте server в 1c-connect.toml", ctx.cfg.host.server);
        ctx.ok(format!("среда: {}", ctx.cfg.host.server));
        Ok(())
    }
}

struct ProjectOp;
impl Op<Ctx> for ProjectOp {
    fn id(&self) -> &'static str {
        "host.project"
    }
    fn title(&self) -> &'static str {
        "Каталог проекта"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        use crate::project::Layout;
        let p = crate::project::Project::from_cfg(&ctx.cfg, &tools_dir());
        match crate::project::layout_state(&p) {
            Layout::BaseDir(f) => OpState::Blocked(format!("{} — каталог базы ({f}); укажите другой project", p.dir.display())),
            Layout::Missing => OpState::Missing,
            Layout::Drifted(d) => OpState::Drifted { from: d, to: format!("ветка {}, origin {}", p.branch, p.remote) },
            Layout::Ok => OpState::Done(format!("{}, ветка {}", p.dir.display(), p.branch)),
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        let p = crate::project::Project::from_cfg(&ctx.cfg, &tools_dir());
        vec![Action::new(format!("{}: git init -b {}, origin {}, .gitattributes/.gitignore/autumn-properties.json", p.dir.display(), p.branch, p.remote))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        if ctx.dry_run {
            return Ok(());
        }
        let p = crate::project::Project::from_cfg(&ctx.cfg, &tools_dir());
        crate::project::init(&p, &tools_dir(), &|s| ctx.ok(s)).map_err(|e| anyhow::anyhow!("{e}"))
    }
    // uninstall leaves the project alone: it is the person's work.
}

struct Extension;
impl Op<Ctx> for Extension {
    fn id(&self) -> &'static str {
        "host.extension"
    }
    fn title(&self) -> &'static str {
        "Расширение mcp-1c в базе"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        match extension_version(&ctx.cfg.host.base_url, &ctx.cfg.host.db_user, &ctx.cfg.host.db_password) {
            Some(v) => OpState::Done(format!("расширение {v}, публикация отвечает")),
            None => OpState::Missing,
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!(
            "mcp-1c --install {} --db-user {} --db-password … (вызовет конфигуратор, минута-две)",
            ctx.cfg.host.base_path, ctx.cfg.host.db_user
        ))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        ctx.progress(Phase::Install);
        let exe = mcp_exe(&ctx.kit);
        let h = &ctx.cfg.host;
        let o = docker::exec(ctx, &exe.to_string_lossy(), &["--install", &h.base_path, "--db-user", &h.db_user, "--db-password", &h.db_password])?;
        // Хвост вывода идёт в журнал окна и в 1c-connect.log: пароль базы из
        // аргументов, если команда его эхом вернула, затирается.
        anyhow::ensure!(o.ok(), "расширение не установилось: {}", docker::redact_text(&o.tail(4).join(" ")));
        ctx.ok("расширение установлено");
        if ctx.dry_run {
            return Ok(());
        }
        if extension_version(&h.base_url, &h.db_user, &h.db_password).is_none() {
            ctx.warn(format!(
                "публикация {} не отвечает. Опубликуйте базу: Конфигуратор → Администрирование → Публикация на веб-сервере, вкладка «HTTP-сервисы» → «Публиковать HTTP-сервисы расширений по умолчанию»",
                h.base_url
            ));
        }
        Ok(())
    }
    fn revert(&self, ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        Some(vec![
            Action::new(format!("1cv8 DESIGNER {} /DeleteCfg -Extension MCP_HTTPService (публикацию не трогаем)", ib_switch(&ctx.cfg.host.base_path))),
            Action::new("удалить known_hosts из комплекта"),
        ])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        ctx.progress(Phase::Uninstall);
        let Some(v8) = find_1cv8() else { anyhow::bail!("1cv8.exe не найден") };
        let h = &ctx.cfg.host;
        let ib = ib_switch(&h.base_path);
        let n = format!("/N{}", h.db_user);
        let p = format!("/P{}", h.db_password);
        let args = ["DESIGNER", &ib, "/DisableStartupDialogs", "/DeleteCfg", "-Extension", "MCP_HTTPService", &n, &p];
        let o = docker::exec(ctx, &v8.to_string_lossy(), &args)?;
        anyhow::ensure!(o.ok(), "конфигуратор не снял расширение: {}", docker::redact_text(&o.tail(3).join(" ")));
        let _ = std::fs::remove_file(ctx.kit.path("known_hosts"));
        ctx.ok("расширение снято, отпечаток сервера забыт");
        Ok(())
    }
}

pub fn host() -> Vec<Box<dyn Op<Ctx>>> {
    vec![Box::new(Env), Box::new(Tools), Box::new(ProjectOp), Box::new(Extension)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitea_probe_on_a_closed_port_is_down() {
        assert!(matches!(gitea_probe("http://127.0.0.1:1"), Gitea::Down(_)));
    }

    #[test]
    fn media_kind_of_the_system_drive_is_plain() {
        let root = if cfg!(windows) { Path::new("C:\\") } else { Path::new("/") };
        assert_eq!(media_kind(root), None);
    }

    #[test]
    fn tools_live_under_the_local_app_data_not_the_kit() {
        let t = tools_dir();
        assert!(t.ends_with("dev1c"), "{}", t.display());
        assert!(oscript_exe(Path::new("T")).ends_with(if cfg!(windows) { "vrunner\\bin\\oscript.exe" } else { "vrunner/bin/oscript" }));
        assert!(vrunner_lib(Path::new("T")).ends_with("lib"));
    }

    #[test]
    fn copy_tree_copies_nested_files_and_counts_them() {
        let tmp = std::env::temp_dir().join(format!("dev1c-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src/a/b")).unwrap();
        std::fs::write(tmp.join("src/a/b/f.txt"), "1").unwrap();
        std::fs::write(tmp.join("src/g.txt"), "2").unwrap();
        let n = copy_tree(&tmp.join("src"), &tmp.join("dst")).unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read_to_string(tmp.join("dst/a/b/f.txt")).unwrap(), "1");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tools_op_state_follows_the_version_stamp() {
        let tmp = std::env::temp_dir().join(format!("dev1c-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let kit = tmp.join("kit");
        let home = tmp.join("home");
        std::fs::create_dir_all(kit.join("vrunner")).unwrap();
        std::fs::create_dir_all(kit.join("git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(kit.join("tools.version"), "OneScript 2.2.0; vanessa-runner 3.0.2; MinGit 2.56.0.2\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Missing));
        std::fs::write(home.join("tools.version"), "старая\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Drifted { .. }));
        std::fs::write(home.join("tools.version"), "OneScript 2.2.0; vanessa-runner 3.0.2; MinGit 2.56.0.2\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Done(d) if d.contains("3.0.2")));
        std::fs::remove_dir_all(kit.join("git")).unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Blocked(_)));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn server_base_uses_s_and_file_base_uses_f() {
        // The configurator's connection switch: /F for a file base, /S for server\base.
        assert_eq!(ib_switch("C:\\Bases\\Buh"), "/FC:\\Bases\\Buh");
        assert_eq!(ib_switch("srv-1c\\buh_prod"), "/Ssrv-1c\\buh_prod");
    }

    #[test]
    fn the_kit_names_the_binary_for_this_platform() {
        let k = Kit::at("X");
        let name = mcp_exe(&k).file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("mcp-1c-"), "{name}");
    }

    #[test]
    fn versions_sort_as_numbers_not_as_text() {
        // Текстовое сравнение ставит "8.3.9.1" выше "8.3.27.1786" (символ '9' > '2'
        // на третьей позиции), а числом 27 > 9 — новее должна выиграть 8.5.1.1150.
        let found = vec![
            ("8.3.9.1".to_string(), PathBuf::from("v1")),
            ("8.3.27.1786".to_string(), PathBuf::from("v2")),
            ("8.5.1.1150".to_string(), PathBuf::from("v3")),
        ];
        assert_eq!(pick_version(found), Some(PathBuf::from("v3")));
    }

    #[test]
    fn the_same_tree_seen_through_a_symlink_appears_once() {
        // В образе /opt/1cv8 — ссылка на /opt/1cv8t: два разных пути, один файл.
        // Проверяем сворачивание через hard-link — он не требует прав на символьную
        // ссылку (на Windows обычная учётка их не даёт), но даёт то же canonicalize.
        let tmp = std::env::temp_dir().join(format!("dev1c-1cv8-dedup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let real = tmp.join("1cv8t");
        std::fs::write(&real, "x").unwrap();
        let via_symlink = tmp.join("1cv8");
        std::fs::hard_link(&real, &via_symlink).unwrap();
        let found = vec![("8.5.1.1150".to_string(), real.clone()), ("8.5.1.1150".to_string(), via_symlink)];
        let deduped = dedup_canonical(found);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].1, real);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn onec_platform_override_picks_whichever_exe_name_is_present() {
        // Без переменной окружения: platform_from — чистая функция над каталогом,
        // который ей называет тест, а не процесс целиком (тесты в этом бинарнике
        // идут в разных потоках одного процесса — process-wide env var была бы гонкой).
        let tmp = std::env::temp_dir().join(format!("dev1c-1cv8-override-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let exe = tmp.join(platform_exe_names()[1]); // учебная платформа — суффикс t
        std::fs::write(&exe, "x").unwrap();
        assert_eq!(platform_from(&tmp), Some(exe));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn shadow_renames_the_training_binaries() {
        // Теневой bin содержит 1cv8.exe/1cv8c.exe — те же файлы, что 1cv8t.exe/1cv8ct.exe,
        // плюс остальные файлы bin под своими именами (платформе нужны соседние библиотеки).
        let tmp = std::env::temp_dir().join(format!("dev1c-shadow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let platform = tmp.join("src").join("1cv8t").join("8.5.1.1150");
        let bin = platform.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("1cv8t.exe"), "платформа").unwrap();
        std::fs::write(bin.join("1cv8ct.exe"), "тонкий клиент").unwrap();
        std::fs::write(bin.join("some.dll"), "библиотека").unwrap();
        let to = tmp.join("shadow-root");

        let shadow_dir = shadow_platform(&platform, &to).unwrap();

        let shadow_bin = shadow_dir.join("bin");
        assert_eq!(std::fs::read(shadow_bin.join("1cv8.exe")).unwrap(), std::fs::read(bin.join("1cv8t.exe")).unwrap());
        assert_eq!(std::fs::read(shadow_bin.join("1cv8c.exe")).unwrap(), std::fs::read(bin.join("1cv8ct.exe")).unwrap());
        assert!(shadow_bin.join("some.dll").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_commercial_platform_needs_no_shadow() {
        // 1cv8.exe уже называется как нужно — тень не создаём вообще.
        let tmp = std::env::temp_dir().join(format!("dev1c-shadow-commercial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let platform = tmp.join("src").join("1cv8").join("8.5.1.1150");
        let bin = platform.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("1cv8.exe"), "платформа").unwrap();
        let to = tmp.join("shadow-root");

        let result = shadow_platform(&platform, &to).unwrap();

        assert_eq!(result, platform);
        assert!(!to.join("platform").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_1cestart_line_is_added_once() {
        // Повторный вызов (повторный запуск host.tools) не плодит строк.
        let tmp = std::env::temp_dir().join(format!("dev1c-1cestart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg_path = tmp.join("1CEStart.cfg");
        std::fs::write(&cfg_path, "ОбщегоНазначения=значение\n").unwrap();
        let location = tmp.join("platform").join("8.5.1.1150");

        add_installed_location(&cfg_path, &location).unwrap();
        add_installed_location(&cfg_path, &location).unwrap();

        let text = std::fs::read_to_string(&cfg_path).unwrap();
        let line = format!("InstalledLocation={}", location.display());
        assert_eq!(text.lines().filter(|l| *l == line).count(), 1);
        assert!(text.contains("ОбщегоНазначения=значение"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
