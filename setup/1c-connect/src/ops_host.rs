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
/// Переменная среды как каталог: пустое значение — это не заданное.
/// `XDG_DATA_HOME=""` иначе дало бы относительный `dev1c` рядом с текущим
/// каталогом, и скрипт стенда (`${XDG_DATA_HOME:-$HOME/.local/share}`, где
/// пустое тоже уходит в умолчание) разошёлся бы с программой.
fn env_dir(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

pub fn tools_dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(env_dir("LOCALAPPDATA").unwrap_or_else(|| ".".into())).join("dev1c")
    } else {
        match env_dir("XDG_DATA_HOME") {
            Some(d) => PathBuf::from(d).join("dev1c"),
            None => PathBuf::from(env_dir("HOME").unwrap_or_else(|| ".".into())).join(".local/share/dev1c"),
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

/// Адрес Gitea среды, выведенный из адреса среды: тот же хост, порт на два
/// ниже SSH (блок из четырёх подряд у dev1c-setup). Вывод верен для
/// туннеля — там Gitea среды приезжает на машину проброшенным портом.
/// Зовётся **только при настройке** (`write_derived_gitea`): при каждом
/// чтении этот счёт давал мёртвый адрес в образе, где туннеля нет.
pub fn derived_gitea(server: &str) -> Option<String> {
    let (host, port) = server.rsplit_once(':')?;
    let port: u32 = port.parse().ok()?;
    (!host.is_empty() && port > 2).then(|| format!("http://{host}:{}", port - 2))
}

/// Настройка дописывает в toml всё, что программа выводила сама: адрес среды,
/// каталог выгрузки и порт своей публикации. После настройки файл параметров —
/// полная картина того, чем программа пользуется (решение заказчика
/// 2026-10-21). Пишется только пустое поле: написанное человеком (или `seed`
/// стенда) не трогается.
///
/// Правятся именно строки этих ключей в файле человека
/// (`toml_render::set`) — его комментарии, порядок ключей и пропущенные
/// ключи переживают настройку; перерисовки файла по встроенному образцу
/// здесь нет (находка ревью задачи 35).
///
/// `dry_run` — сухой прогон: `cfg` в памяти правится так же (иначе план
/// операций показал бы картину, которой после установки не будет), а файл не
/// пишется. Отдаёт записанные строки для журнала; пусто — писать было нечего.
pub fn write_derived_fields(kit: &Kit, cfg: &mut Cfg, dry_run: bool) -> Result<Vec<String>> {
    let path = kit.state_path(crate::cfg::FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut fields: Vec<(&str, String)> = Vec::new();
    if cfg.gitea_url().is_none() {
        // Выводить не из чего (`server` без порта) — поле остаётся пустым, и
        // проверка скажет про поле, а не подставит выдуманный адрес.
        if let Some(addr) = derived_gitea(&cfg.host.server) {
            cfg.host.gitea = addr.clone();
            fields.push(("gitea", format!("{addr:?}")));
        }
    }
    if cfg.host.dump.is_empty() {
        let dir = cfg.dump_dir();
        cfg.host.dump = dir.clone();
        fields.push(("dump", format!("{dir:?}")));
    }
    if cfg.host.http_port.is_none() {
        let port = cfg.http_port();
        cfg.host.http_port = Some(port);
        fields.push(("http_port", port.to_string()));
    }
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    if !dry_run {
        let mut text = std::fs::read_to_string(&path).with_context(|| format!("не прочитать {}", path.display()))?;
        for (key, value) in &fields {
            text = common::toml_render::set(&text, "[host]", key, value);
        }
        std::fs::write(&path, &text).with_context(|| format!("не записать {}", path.display()))?;
    }
    Ok(fields.into_iter().map(|(key, value)| format!("{key} = {value}")).collect())
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

/// Имя файла с отметкой версии — одно и то же в обоих местах, но то, ЧТО по
/// нему лежит, разное: `kit_root.join(VERSION_FILE)` — штамп КОМПЛЕКТА
/// (что было собрано: Dockerfile.1c на Linux, `pack.sh` на Windows, пишут
/// один раз и больше не трогают); `tools.join(VERSION_FILE)` — штамп
/// СОСТОЯНИЯ (что фактически разложено на этой машине: пишет `host.tools`
/// при раскладке, то есть `Tools::apply` ниже). `tools_state` — и есть
/// сверка этих двух величин.
const VERSION_FILE: &str = "tools.version";

/// Каталоги, которых мост и `host.tools` ждут от комплекта: vrunner и git —
/// на обеих ОС (сборка/публикация), httpd — только на Windows (свой
/// локальный веб-сервер, которым 1c-connect публикует базу без ручной
/// установки Apache/IIS). На Linux эту роль уже играет системный apache2
/// образа — второй веб-сервер в комплект незачем класть. Единственное место
/// в коде, где раскладка комплекта названа — остальные функции этого файла
/// (`mcp_exe`, `oscript_exe`, `vrunner_lib`, `git_exe`) описывают имена
/// внутри неё, но не то, какие каталоги обязаны быть.
fn required_kit_dirs() -> &'static [&'static str] {
    if cfg!(windows) { &["vrunner", "git", "httpd"] } else { &["vrunner", "git"] }
}

/// Состояние по отметке версии: комплект пишет `tools.version` при сборке
/// (штамп комплекта, не меняется), `host.tools` — свою копию в состоянии
/// при раскладке (штамп состояния). Расхождение этих двух и есть `Drifted`.
pub fn tools_state(kit_root: &Path, tools: &Path) -> OpState {
    let missing: Vec<&str> = required_kit_dirs().iter().copied().filter(|d| !kit_root.join(d).is_dir()).collect();
    if !missing.is_empty() {
        return OpState::Blocked(format!("в комплекте нет {} — скопируйте полный комплект заново", missing.iter().map(|d| format!("{d}/")).collect::<Vec<_>>().join(", ")));
    }
    let shipped = std::fs::read_to_string(kit_root.join(VERSION_FILE)).unwrap_or_default();
    let laid_out = std::fs::read_to_string(tools.join(VERSION_FILE)).unwrap_or_default();
    if laid_out.trim().is_empty() {
        OpState::Missing
    } else if laid_out.trim() == shipped.trim() {
        OpState::Done(format!("{} в {}", shipped.trim(), tools.display()))
    } else {
        OpState::Drifted { from: laid_out.trim().to_string(), to: shipped.trim().to_string() }
    }
}

/// Комплект, как его видит коннектор: читают из комплекта, пишут в состояние.
/// На Windows комплект приезжает на носителе, он свой и писуемый — состояние
/// лежит там же, как и было. На Linux комплект собран в образ по пути
/// `/opt/dev1c-kit`, он один на все рабочие места и только для чтения; всё,
/// что программа пишет — файл параметров, замок единственного экземпляра,
/// журнал, отчёт проверки, `known_hosts`, память между прогонами, — уезжает
/// в состояние пользователя рядом с публикацией.
pub fn wire_state(kit: Kit) -> Kit {
    if cfg!(windows) {
        return kit;
    }
    Kit::with_state(kit.root, tools_dir())
}

/// Каталог состояния на запуске: его ждут замок единственного экземпляра
/// (`flock` на самом каталоге) и те писатели, что открывают файл напрямую
/// (`Kit::journal`, журнал окна) — остальные идут через `kit::write_text`,
/// который родителя создаёт сам. Молча: ошибку увидят те, кому каталог
/// нужен, и назовут путь.
pub fn ensure_state(kit: &Kit) {
    // Только когда состояние — не сам комплект: при явном `--kit` создавать
    // каталог не наше дело, иначе опечатка в пути молча плодит каталоги
    // вместо внятного «нет файла параметров».
    if kit.state() != kit.root {
        let _ = std::fs::create_dir_all(kit.state());
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
        let dirs = required_kit_dirs().iter().map(|d| format!("{d}/")).collect::<Vec<_>>().join(", ");
        vec![Action::new(format!("скопировать {dirs} из {} в {} (≈ 100 МБ)", ctx.kit.root.display(), tools_dir().display()))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let to = tools_dir();
        if ctx.dry_run {
            return Ok(());
        }
        for d in required_kit_dirs() {
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
                        Err(e) => ctx.warn(format!("1С не увидит учебную платформу в списке версий, путь придётся добавить вручную: {e}")),
                    },
                    Ok(_) => {} // платформа уже обычная — тень не нужна
                    Err(e) => ctx.warn(format!("не удалось подготовить учебную платформу к запуску под обычным именем: {e}")),
                }
            }
        }
        Ok(())
    }
    fn revert(&self, _ctx: &Ctx, level: CleanLevel) -> Option<Vec<Action>> {
        // Поимённо, а не весь каталог: в состоянии рядом с инструментами
        // теперь живут файл параметров, журнал, отчёт проверки и публикация —
        // «очистка данных» уносила бы настройки человека, не сказав об этом.
        let t = tools_dir();
        let mut names: Vec<String> = required_kit_dirs().iter().map(|d| format!("удалить {}", t.join(d).display())).collect();
        names.push(format!("удалить {}", t.join(VERSION_FILE).display()));
        matches!(level, CleanLevel::Data).then(|| names.into_iter().map(Action::new).collect())
    }
    fn revert_apply(&self, ctx: &Ctx, level: CleanLevel) -> Result<()> {
        if matches!(level, CleanLevel::Data) {
            let t = tools_dir();
            for d in required_kit_dirs() {
                let _ = std::fs::remove_dir_all(t.join(d));
            }
            let _ = std::fs::remove_file(t.join(VERSION_FILE));
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

/// `xvfb-run` в `PATH`. Обходим каталоги сами, тем же приёмом, что
/// `platform_from` выше (первый существующий путь) — ради одной программы
/// крейт вокруг `which` не нужен.
pub fn find_xvfb_run() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("xvfb-run")).find(|p| p.exists())
}

/// `DISPLAY` как он задан в окружении (пустое значение — что не задан вовсе).
pub fn display_env() -> Option<String> {
    std::env::var("DISPLAY").ok().filter(|v| !v.is_empty())
}

/// Пригоден ли заданный `DISPLAY`. Чистая функция: значение ей называет
/// вызывающий.
///
/// Для местного дисплея вида `:N` (и `:N.S`) живой X-сервер — это сокет
/// `/tmp/.X11-unix/XN`: нет сокета — нет и сервера. Случай не выдуманный:
/// `DISPLAY=:0`, унаследованный через `su -` или юнит init, остаётся в
/// окружении, когда графического сеанса за ним уже нет; раньше обёртка тогда
/// не включалась, и человек получал всё тот же сырой отказ GTK+, а проверка
/// показывала по этому поводу зелёный факт. Нечисловые и сетевые адреса
/// (`host:0`) считаем пригодными как есть: дёшево их не проверить, а отнимать
/// у человека его же экран хуже, чем довериться.
pub fn display_alive(display: &str) -> bool {
    let Some(tail) = display.strip_prefix(':') else { return true };
    let n = tail.split('.').next().unwrap_or(tail);
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    Path::new("/tmp/.X11-unix").join(format!("X{n}")).exists()
}

/// Экран сеанса: значение `DISPLAY`, если оно задано и за ним есть сервер.
pub fn session_display() -> Option<String> {
    display_env().filter(|d| display_alive(d))
}

/// Нужен ли Конфигуратору экран и чем его дать: `None` — экран уже есть либо
/// обернуть нечем; путь к `xvfb-run` — если нужен и он найден. Чистая
/// функция, окружение и `PATH` ей называет вызывающий (`display_wrapper`).
///
/// Зачем вообще: на Linux выгрузка и загрузка идут через vanessa-runner, а он
/// поднимает Конфигуратор — оконную программу. Без экрана он не стартует, и
/// `1c-connect export` из командной строки падал с «Unable to initialize GTK+
/// or connect to the windowing system. Is DISPLAY set properly?», код 255
/// (замер 2026-10-20 в контейнере). Экран даёт коннектор сам — тем же
/// способом, каким стенд уже обходит эту беду для своего `vrunner-mcp`
/// (`config/1c/stand.sh`, `xvfb-run -a`). Только на Linux: на Windows
/// Конфигуратор рисует в сеансе пользователя, а на macOS платформы нет вовсе.
pub fn display_prefix(has_display: bool, xvfb: Option<&Path>) -> Option<PathBuf> {
    if !cfg!(target_os = "linux") || has_display {
        return None;
    }
    xvfb.map(Path::to_path_buf)
}

/// `display_prefix` на этой машине: сам спрашивает `DISPLAY` и `PATH`.
pub fn display_wrapper() -> Option<PathBuf> {
    display_prefix(session_display().is_some(), find_xvfb_run().as_deref())
}

/// Текст отказа до запуска Конфигуратора: экрана нет и дать его нечем.
/// Чистая функция — состояние машины ей называет вызывающий.
pub fn no_screen(has_display: bool, xvfb: Option<&Path>) -> Option<String> {
    (cfg!(target_os = "linux") && !has_display && xvfb.is_none())
        .then(|| format!("нет экрана для Конфигуратора: DISPLAY не задан (или за ним нет X-сервера), xvfb-run не найден — {SCREEN_HINT}"))
}

/// Отказ от имени коннектора вместо сырого «Unable to initialize GTK+…» через
/// минуту ожидания: подсказка про `xvfb` живёт не только в `check`, куда после
/// непонятного падения `export` человек не пойдёт.
pub fn screen_check() -> Result<(), String> {
    match no_screen(session_display().is_some(), find_xvfb_run().as_deref()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Чем лечить отсутствие экрана. Один текст на отказ до запуска и на факт
/// проверки. Пакета два: `xvfb-run` без `xauth` отвечает «xauth: command not
/// found» — в образе проекта и в CI `xauth` стоит отдельной строкой рядом.
pub const SCREEN_HINT: &str = "поставьте оба пакета: apt install xvfb xauth (без xauth сам xvfb-run отвечает «xauth: command not found»)";

/// Переписать команду в `xvfb-run -a <программа> <аргументы>`. Отдельно от
/// `with_display`, чтобы порядок аргументов проверялся тестом на любой машине,
/// а не только там, где `xvfb-run` есть.
///
/// `xvfb-run` потоки не сливает — проверено в образе 2026-10-20
/// (`xvfb-run -a sh -c "echo OUT; echo ERR >&2" 2>/dev/null` печатает только
/// `OUT`): stdout ребёнка остаётся каналом JSON-RPC, stderr — журналом, и
/// протокол с агентом обёртка не рвёт.
fn wrap_in_display(cmd: std::process::Command, xvfb: &Path) -> std::process::Command {
    let mut c = std::process::Command::new(xvfb);
    c.arg("-a").arg(cmd.get_program()).args(cmd.get_args());
    if let Some(d) = cmd.get_current_dir() {
        c.current_dir(d);
    }
    // Перенос окружения: `get_envs` отдаёт и снятые переменные (`None`).
    // Потоки (stdin/stdout/stderr) из `Command` не вычитываются вовсе — их
    // вызывающие ставят уже после этой обёртки (`local::call`, `bridge`).
    for (k, v) in cmd.get_envs() {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    c
}

/// Дать Конфигуратору экран, если его нет: `xvfb-run -a <программа> …`.
/// Экран — забота коннектора, а не вызывающего (см. `display_prefix`).
pub fn with_display(cmd: std::process::Command) -> std::process::Command {
    match display_wrapper() {
        Some(x) => wrap_in_display(cmd, &x),
        None => cmd,
    }
}

/// То же для `docker::exec`, который берёт программу и аргументы строками, а
/// не `Command`: отдаёт пару «программа, аргументы» уже с обёрткой. Обёртку
/// называет вызывающий (`display_wrapper()`), а не функция — так порядок
/// аргументов проверяется тестом на любой машине.
fn display_argv(program: &str, args: &[&str], xvfb: Option<&Path>) -> (String, Vec<String>) {
    let tail = args.iter().map(|s| s.to_string());
    match xvfb {
        Some(x) => (x.to_string_lossy().into_owned(), ["-a".to_string(), program.to_string()].into_iter().chain(tail).collect()),
        None => (program.to_string(), tail.collect()),
    }
}

/// Жёсткая ссылка на файл под новым именем; если том не позволяет ссылки
/// (съёмный диск, другой раздел), копирует файл — не проваливает всю раскладку
/// теневого каталога из-за одного файла. Старую ссылку/копию сперва убираем,
/// чтобы повторный запуск (например, после прерванного прошлого) не падал и
/// не плодил файлов.
fn link_or_copy(src: &Path, dst: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(dst);
    if std::fs::hard_link(src, dst).is_err() {
        std::fs::copy(src, dst).map_err(|e| format!("копирование {} в {}: {e}", src.display(), dst.display()))?;
    }
    Ok(())
}

/// Рекурсивно сворачивает каталог `src` в `dst`: подкаталоги зеркалируются,
/// файлы — жёсткая ссылка либо копия, причём `1cv8t.exe`/`1cv8ct.exe` получают
/// дополнительные ссылки под именами `1cv8.exe`/`1cv8c.exe`. У платформы в bin
/// есть подкаталоги с ресурсами (CEF, конфигурация) — без них она не стартует.
fn shadow_tree(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("каталог {} не создан: {e}", dst.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("каталог {} не читается: {e}", src.display()))?;
    for e in entries.filter_map(|e| e.ok()) {
        let Ok(file_type) = e.file_type() else { continue };
        let name = e.file_name();
        let from = e.path();
        let into = dst.join(&name);
        if file_type.is_dir() {
            shadow_tree(&from, &into)?;
            continue;
        }
        if !file_type.is_file() {
            continue; // символические ссылки и прочее — пропускаем, не падаем
        }
        link_or_copy(&from, &into)?;
        let renamed = match name.to_string_lossy().as_ref() {
            "1cv8t.exe" => Some("1cv8.exe"),
            "1cv8ct.exe" => Some("1cv8c.exe"),
            _ => None,
        };
        if let Some(r) = renamed {
            link_or_copy(&from, &dst.join(r))?;
        }
    }
    Ok(())
}

/// Теневой каталог `bin` учебной платформы под именами коммерческой версии:
/// на вход — каталог версии (`…\1cv8t\8.5.1.1150`), на выходе — каталог версии
/// тени (`<to>\platform\8.5.1.1150`) с `bin\1cv8.exe` и `bin\1cv8c.exe`,
/// указывающими (жёсткой ссылкой, либо копией) на `1cv8t.exe`/`1cv8ct.exe`, и
/// остальным деревом `bin` (вложенные каталоги включительно) рядом.
/// Если платформа уже коммерческая (рядом есть `1cv8.exe`), тень не нужна —
/// ничего не создаём и возвращаем исходный каталог платформы без изменений.
pub fn shadow_platform(platform: &Path, to: &Path) -> Result<PathBuf, String> {
    let bin = platform.join("bin");
    if bin.join("1cv8.exe").exists() {
        return Ok(platform.to_path_buf());
    }
    let version = platform.file_name().ok_or_else(|| format!("путь {} не похож на каталог версии платформы", platform.display()))?;
    let shadow_dir = to.join("platform").join(version);
    shadow_tree(&bin, &shadow_dir.join("bin"))?;
    Ok(shadow_dir)
}

/// Список установленных версий платформы у лаунчера 1С (1CEStart.exe): без
/// строки на наш теневой каталог он не предложит его в списке версий.
fn one_c_estart_cfg() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_else(|| ".".into())).join("1C").join("1CEStart").join("1CEStart.cfg")
}

const UTF16_BOM: [u8; 2] = [0xFF, 0xFE];

/// Читает файл лаунчера как UTF-16LE с BOM (так пишут инструменты 1С) и
/// возвращает его текст без BOM. Пустой/отсутствующий файл — пустая строка.
/// Файл неожиданной формы (нет BOM, нечётное число байт после него) не трогаем
/// и считаем ошибкой — испорченный список версий стоит человеку всех ярлыков баз.
fn read_1cestart_cfg(cfg_path: &Path) -> Result<String, String> {
    let bytes = match std::fs::read(cfg_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(format!("{} не читается: {e}", cfg_path.display())),
    };
    if bytes.is_empty() {
        return Ok(String::new());
    }
    if bytes.len() < 2 || &bytes[0..2] != &UTF16_BOM {
        return Err(format!("{} не похож на список версий 1С (ожидался BOM UTF-16LE)", cfg_path.display()));
    }
    if (bytes.len() - 2) % 2 != 0 {
        return Err(format!("{} повреждён (нечётное число байт)", cfg_path.display()));
    }
    let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Ok(String::from_utf16_lossy(&units))
}

/// Записывает текст обратно как UTF-16LE с BOM — ровно в том виде, в котором
/// лаунчер 1С сам пишет этот файл.
fn write_1cestart_cfg(cfg_path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = cfg_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("каталог {} не создан: {e}", parent.display()))?;
    }
    let mut bytes = UTF16_BOM.to_vec();
    for u in text.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(cfg_path, bytes).map_err(|e| format!("{} не записан: {e}", cfg_path.display()))
}

/// Добавляет `InstalledLocation=<location>` в файл лаунчера, если такой строки
/// там ещё нет; остальные строки не трогает. Строки разделены `\r\n` — как
/// пишут инструменты 1С на Windows. Файла может не быть — тогда создаём его с
/// BOM и единственной строкой.
fn add_installed_location(cfg_path: &Path, location: &Path) -> Result<(), String> {
    let line = format!("InstalledLocation={}", location.display());
    let existing = read_1cestart_cfg(cfg_path)?;
    if existing.lines().any(|l| l == line) {
        return Ok(());
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with("\r\n") && !out.ends_with('\n') {
        out.push_str("\r\n");
    }
    out.push_str(&line);
    out.push_str("\r\n");
    write_1cestart_cfg(cfg_path, &out)
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
    extension_version_in(base_url, user, password, std::time::Duration::from_secs(60))
}

/// То же, но со своим сроком: установка ждёт долго (сервис 1С поднимается не
/// мгновенно), а вердикт готовности — коротко, чтобы уложиться в бюджет
/// (`verify::Limits`).
pub fn extension_version_in(base_url: &str, user: &str, password: &str, timeout: std::time::Duration) -> Option<String> {
    let c = reqwest::blocking::Client::builder().timeout(timeout).build().ok()?;
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
            Layout::NoGitea => OpState::Blocked(crate::cfg::no_gitea()),
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

/// Пишет дескриптор и конфигурацию, затем стартует httpd с заданным временем
/// жизни (`publication::Lifetime`). Общая часть `Publication::apply` (разовая
/// установка — публикация должна пережить того, кто её поднял) и
/// `ensure_publication` (serve/окно — сервис живёт вместе с вызвавшим
/// процессом).
fn raise_publication(ctx: &Ctx, lifetime: crate::publication::Lifetime) -> Result<()> {
    let p = crate::publication::Paths::of(&ctx.cfg, &ctx.kit).map_err(|e| anyhow::anyhow!("{e}"))?;
    // Разовая команда (Detached — «Установить», headless install), нашедшая
    // уже отвечающий и верно настроенный сервис, его не трогает: он уже у
    // кого-то на привязи (этот же процесс как WithCaller, живое окно, serve
    // на соседнем процессе), а убить и поднять заново отвязанным значило бы
    // снять эту привязку впустую — крах настоящего владельца потом оставил
    // бы ровно того сироту, которого привязка и должна предотвращать (см.
    // publication::detached_should_leave_alone). Только подтверждаем, что
    // сервис отвечает, и выходим без kill_tracked()/start().
    let responds = crate::publication::service_responds(&ctx.cfg.base_url());
    let matches = crate::publication::descriptor_matches(&p.pub_dir, &ctx.cfg);
    if crate::publication::detached_should_leave_alone(lifetime, responds, matches) {
        ctx.ok(format!("HTTP-сервис базы уже поднят и отвечает на 127.0.0.1:{} — не трогаем", p.port));
        return Ok(());
    }
    // Дескриптор несёт пароль базы — в журнал не идёт, только в файл, и
    // только с правами на владельца (образ делит диск между рабочими
    // местами).
    crate::publication::write_vrd(&p.pub_dir, &crate::publication::vrd(&ctx.cfg)).map_err(|e| anyhow::anyhow!("{e}"))?;
    let conf_text = crate::publication::httpd_conf(&p).map_err(|e| anyhow::anyhow!("{e}"))?;
    std::fs::write(&p.conf, conf_text).context("httpd.conf не записан")?;
    // start() идемпотентен сам по себе (перезапускает уже поднятый сервис
    // поверх новой конфигурации) — второй вызов после смены пароля или базы
    // не падает на «порт занят».
    crate::publication::start(&p, lifetime).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.ok(format!("HTTP-сервис базы поднят на 127.0.0.1:{}", p.port));
    Ok(())
}

/// Поднимает публикацию базы, если она ещё не «сделано»: общая точка входа
/// для headless `serve` и для окна (включение MCP, сохранение изменившейся
/// базы, «Установить» — см. спеку «HTTP-сервис базы ведёт коннектор»).
///
/// В режиме `"webserver"` ничего не поднимает — как и `Publication::apply`,
/// только проверяет публикацию и предупреждает, если та не отвечает.
///
/// В режиме `"auto"` сервис стартует с `Lifetime::WithCaller` (в отличие от
/// `Publication::apply`, которая использует `Detached`): вызвавший эту
/// функцию процесс — это и есть сеанс (serve держит его до
/// Ctrl+C/SIGTERM, окно — пока открыто), и крах этого процесса обязан забрать
/// httpd с собой, а не оставить сироту на порту со старой конфигурацией.
pub fn ensure_publication(ctx: &Ctx) -> Result<()> {
    if ctx.cfg.host.publication == "webserver" {
        return Publication.apply(ctx);
    }
    if ctx.dry_run || matches!(Publication.state(ctx), OpState::Done(_)) {
        return Ok(());
    }
    raise_publication(ctx, crate::publication::Lifetime::WithCaller)
}

/// Поднимает HTTP-сервис базы: своим httpd под пользователем (`"auto"`) или
/// только проверяет уже существующую публикацию (`"webserver"`). Зовётся до
/// `host.extension` — иначе расширение ставить некуда.
struct Publication;
impl Op<Ctx> for Publication {
    fn id(&self) -> &'static str {
        "host.publication"
    }
    fn title(&self) -> &'static str {
        "Публикация базы (HTTP-сервис)"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        if ctx.cfg.host.publication == "webserver" {
            // Публикация не наша — показываем то же, что видит host.extension:
            // отдельного факта «свой httpd» здесь нет.
            return match extension_version(&ctx.cfg.base_url(), &ctx.cfg.host.db_user, &ctx.cfg.host.db_password) {
                Some(v) => OpState::Done(format!("публикация отвечает (расширение {v})")),
                None => OpState::Missing,
            };
        }
        let p = match crate::publication::Paths::of(&ctx.cfg, &ctx.kit) {
            Ok(p) => p,
            Err(e) => return OpState::Blocked(e),
        };
        // Раздельная проверка, не просто «порт занят»: иначе после «Сохранить»
        // со сменившимся паролем старый HTTP-сервис остаётся жив и слушает тот
        // же порт, а состояние молча читается как «сделано». responds — что
        // по адресу вообще отвечает HTTP (не чужая не-HTTP программа на том же
        // порту); matches — что дескриптор на диске это именно текущие
        // параметры базы, а не прежние.
        let responds = crate::publication::service_responds(&ctx.cfg.base_url());
        let matches = crate::publication::descriptor_matches(&p.pub_dir, &ctx.cfg);
        match (responds, matches) {
            (true, true) => OpState::Done(format!("HTTP-сервис базы поднят на 127.0.0.1:{}", p.port)),
            (true, false) => OpState::Drifted { from: "старый дескриптор на диске".into(), to: "текущие параметры базы".into() },
            (false, _) => OpState::Missing,
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        if ctx.cfg.host.publication == "webserver" {
            return vec![Action::new(format!("публикация на вашем веб-сервере ({}) — ничего не делаем, кроме проверки", ctx.cfg.base_url()))];
        }
        vec![Action::new(format!("записать default.vrd и httpd.conf, поднять httpd на 127.0.0.1:{}", ctx.cfg.http_port()))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        if ctx.cfg.host.publication == "webserver" {
            // Публикация — забота человека; только проверяем и предупреждаем.
            if extension_version(&ctx.cfg.base_url(), &ctx.cfg.host.db_user, &ctx.cfg.host.db_password).is_none() {
                ctx.warn(format!("публикация {} не отвечает — проверьте веб-сервер", ctx.cfg.base_url()));
            }
            return Ok(());
        }
        if ctx.dry_run {
            return Ok(());
        }
        // Lifetime::Detached: публикация базы, а не ребёнок установщика — ни
        // штатный выход, ни крах разовой команды (или окна, нажавшего
        // «Установить») не должны её касаться (см. publication::Lifetime).
        raise_publication(ctx, crate::publication::Lifetime::Detached)
    }
    fn revert(&self, ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        (ctx.cfg.host.publication != "webserver").then(|| vec![Action::new("остановить свой httpd")])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        if ctx.cfg.host.publication == "webserver" {
            return Ok(());
        }
        let p = crate::publication::Paths::of(&ctx.cfg, &ctx.kit).map_err(|e| anyhow::anyhow!("{e}"))?;
        crate::publication::stop(&p).map_err(|e| anyhow::anyhow!("{e}"))?;
        // Наш сервис точно остановлен (stop() это гарантирует) — если порт
        // всё равно занят, там чужая программа: не наша ошибка, а информация.
        if crate::publication::httpd_responds(p.port) {
            ctx.warn(format!("HTTP-сервис базы остановлен, но порт 127.0.0.1:{} всё ещё занят другим процессом", p.port));
        } else {
            ctx.ok("HTTP-сервис базы остановлен");
        }
        Ok(())
    }
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
        match extension_version(&ctx.cfg.base_url(), &ctx.cfg.host.db_user, &ctx.cfg.host.db_password) {
            Some(v) => OpState::Done(format!("расширение {v}, публикация отвечает")),
            None => OpState::Missing,
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        // План печатает то, что действительно будет запущено, обёртку включая:
        // из этих строк собирается ручной путь в инструкции администратора, и
        // команда без `xvfb-run` в ней не заработала бы.
        let prefix = display_wrapper().map(|x| format!("{} -a ", x.display())).unwrap_or_default();
        vec![Action::new(format!(
            "{prefix}mcp-1c --install {} --db-user {} --db-password … (вызовет конфигуратор, минута-две)",
            ctx.cfg.host.base_path, ctx.cfg.host.db_user
        ))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        ctx.progress(Phase::Install);
        // Предполёт: `mcp-1c --install` поднимает Конфигуратор, и если экрана
        // нет и дать его нечем — отказ словами и сразу, а не сырой GTK-текст
        // через минуту ожидания.
        if !ctx.dry_run {
            screen_check().map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let exe = mcp_exe(&ctx.kit);
        let h = &ctx.cfg.host;
        // Экран Конфигуратору даём здесь же (см. `display_prefix`): иначе на
        // Linux без `DISPLAY` он не стартует.
        let (prog, args) =
            display_argv(&exe.to_string_lossy(), &["--install", &h.base_path, "--db-user", &h.db_user, "--db-password", &h.db_password], display_wrapper().as_deref());
        let o = docker::exec(ctx, &prog, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
        // Хвост вывода идёт в журнал окна и в 1c-connect.log: пароль базы из
        // аргументов, если команда его эхом вернула, затирается.
        anyhow::ensure!(o.ok(), "расширение не установилось: {}", docker::redact_text(&o.tail(4).join(" ")));
        ctx.ok("расширение установлено");
        if ctx.dry_run {
            return Ok(());
        }
        let base_url = ctx.cfg.base_url();
        if extension_version(&base_url, &h.db_user, &h.db_password).is_none() {
            ctx.warn(format!(
                "публикация {} не отвечает. Опубликуйте базу: Конфигуратор → Администрирование → Публикация на веб-сервере, вкладка «HTTP-сервисы» → «Публиковать HTTP-сервисы расширений по умолчанию»",
                base_url
            ));
        }
        Ok(())
    }
    fn revert(&self, ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        let prefix = display_wrapper().map(|x| format!("{} -a ", x.display())).unwrap_or_default();
        Some(vec![
            Action::new(format!("{prefix}1cv8 DESIGNER {} /DeleteCfg -Extension MCP_HTTPService (публикацию не трогаем)", ib_switch(&ctx.cfg.host.base_path))),
            Action::new(format!("удалить {}", ctx.kit.state_path("known_hosts").display())),
        ])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        ctx.progress(Phase::Uninstall);
        if !ctx.dry_run {
            screen_check().map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let Some(v8) = find_1cv8() else { anyhow::bail!("1cv8.exe не найден") };
        let h = &ctx.cfg.host;
        let ib = ib_switch(&h.base_path);
        let n = format!("/N{}", h.db_user);
        let p = format!("/P{}", h.db_password);
        let args = ["DESIGNER", &ib, "/DisableStartupDialogs", "/DeleteCfg", "-Extension", "MCP_HTTPService", &n, &p];
        // Снятие расширения — тот же Конфигуратор, та же нужда в экране.
        let (prog, args) = display_argv(&v8.to_string_lossy(), &args, display_wrapper().as_deref());
        let o = docker::exec(ctx, &prog, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
        anyhow::ensure!(o.ok(), "конфигуратор не снял расширение: {}", docker::redact_text(&o.tail(3).join(" ")));
        let _ = std::fs::remove_file(ctx.kit.state_path("known_hosts"));
        ctx.ok("расширение снято, отпечаток сервера забыт");
        Ok(())
    }
}

pub fn host() -> Vec<Box<dyn Op<Ctx>>> {
    vec![Box::new(Env), Box::new(Tools), Box::new(ProjectOp), Box::new(Publication), Box::new(Extension)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_sits_between_project_and_extension() {
        // host.publication должна идти до host.extension — иначе расширение
        // ставить некуда (публикация готовит каталог и поднимает HTTP-сервис).
        let ids: Vec<&str> = host().iter().map(|op| op.id()).collect();
        let project = ids.iter().position(|&i| i == "host.project").unwrap();
        let publication = ids.iter().position(|&i| i == "host.publication").unwrap();
        let extension = ids.iter().position(|&i| i == "host.extension").unwrap();
        assert!(project < publication && publication < extension, "{ids:?}");
    }

    #[test]
    fn publication_in_webserver_mode_is_missing_when_nothing_answers() {
        // Режим "webserver": состояние — тот же факт, что у host.extension,
        // никакого своего httpd тут нет и подниматься нечему.
        let mut cfg = Cfg::default();
        cfg.host.publication = "webserver".into();
        cfg.host.base_url = "http://localhost:1/hs/mcp-1c".into(); // порт 1 никто не слушает
        let sink: common::Sink = std::sync::Arc::new(|_: common::Event| {});
        let ctx = Ctx { kit: Kit::at("."), cfg, dry_run: true, sink, prompt: Box::new(common::AlwaysDefault) };
        assert_eq!(Publication.state(&ctx), OpState::Missing);
    }

    /// Создаёт `Ctx` для тестов `ensure_publication`: не настоящая платформа,
    /// не настоящий httpd — только то, что решает сама функция, прежде чем
    /// дойти до файловой системы или сети.
    fn publication_ctx(cfg: Cfg, dry_run: bool) -> Ctx {
        let sink: common::Sink = std::sync::Arc::new(|_: common::Event| {});
        Ctx { kit: Kit::at("."), cfg, dry_run, sink, prompt: Box::new(common::AlwaysDefault) }
    }

    #[test]
    fn ensure_publication_in_webserver_mode_only_checks_and_never_starts_anything() {
        // "webserver": ensure_publication не должна пытаться поднять что-то
        // своё — делегирует в Publication::apply, которая лишь проверяет и
        // предупреждает. dry_run здесь нарочно false: с dry_run=true тест
        // проходил бы и после удаления всей ветки "webserver" — функция
        // вышла бы в "auto" и всё равно вернула бы Ok просто из-за самого
        // dry_run, не доходя до какой-либо настоящей ветки. Здесь нет
        // платформы 1С и httpd — если бы ветки "webserver" не было, функция
        // ушла бы в "auto" и либо провалилась на Paths::of (платформа не
        // найдена), либо не выдала бы предупреждение «не отвечает», которое
        // выдаёт только настоящая проверка webserver-режима.
        let mut cfg = Cfg::default();
        cfg.host.publication = "webserver".into();
        cfg.host.base_url = "http://localhost:1/hs/mcp-1c".into(); // порт 1 никто не слушает
        let warnings = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let w = warnings.clone();
        let sink: common::Sink = std::sync::Arc::new(move |e| {
            if let common::Event::Warn(t) = e {
                w.lock().unwrap().push(t);
            }
        });
        let ctx = Ctx { kit: Kit::at("."), cfg, dry_run: false, sink, prompt: Box::new(common::AlwaysDefault) };
        assert!(ensure_publication(&ctx).is_ok());
        let warnings = warnings.lock().unwrap();
        assert!(warnings.iter().any(|w| w.contains("не отвечает")), "{warnings:?}");
    }

    #[test]
    fn ensure_publication_is_a_dry_run_noop() {
        // --dry-run у serve: ensure_publication ничего не должна менять на
        // машине — ни писать файлы, ни поднимать httpd.
        let mut cfg = Cfg::default();
        cfg.host.publication = "auto".into();
        let ctx = publication_ctx(cfg, true);
        assert!(ensure_publication(&ctx).is_ok());
    }

    #[test]
    fn gitea_probe_on_a_closed_port_is_down() {
        assert!(matches!(gitea_probe("http://127.0.0.1:1"), Gitea::Down(_)));
    }

    #[test]
    fn derived_gitea_sits_two_ports_below_ssh() {
        assert_eq!(derived_gitea("192.168.1.10:3393").as_deref(), Some("http://192.168.1.10:3391"));
        assert_eq!(derived_gitea("10.0.0.5:4403").as_deref(), Some("http://10.0.0.5:4401"));
        // Выводить не из чего — и выдумывать нечего.
        assert_eq!(derived_gitea("192.168.1.10"), None);
        assert_eq!(derived_gitea("192.168.1.10:порт"), None);
        assert_eq!(derived_gitea(":3393"), None);
        assert_eq!(derived_gitea("192.168.1.10:1"), None);
    }

    /// Каталог состояния на свой тест (писать в чужой toml нельзя),
    /// убираемый и при панике — как `tempdir::Dir` в `project.rs`.
    struct TomlKit(Kit);
    impl TomlKit {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("dev1c-toml-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TomlKit(Kit::at(dir))
        }
        /// Положить файл параметров и сказать, где он лёг.
        fn put(&self, text: &str) -> PathBuf {
            let p = self.0.state_path(crate::cfg::FILE);
            std::fs::write(&p, text).unwrap();
            p
        }
    }
    impl std::ops::Deref for TomlKit {
        type Target = Kit;
        fn deref(&self) -> &Kit {
            &self.0
        }
    }
    impl Drop for TomlKit {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.state());
        }
    }

    /// Файл человека, а не образец: со своей заметкой, своим порядком ключей
    /// и без необязательных ключей вовсе. На образце сохранность «чужих
    /// строк» проверялась бы на тех же строках, что программа и пишет, —
    /// тавтология (находка ревью задачи 35).
    const MINE: &str = concat!(
        "# мои параметры, не трогать\n[host]\nworkstation = \"dev2\"\n",
        "# среда в офисе\nserver = \"10.0.0.5:4403\"\n",
        "project = \"C:\\\\proj\\\\Buh\"\nbase_url = \"http://localhost/Buh/hs/mcp-1c\"\n"
    );

    #[test]
    fn install_writes_the_derived_address_once() {
        let kit = TomlKit::new("derived");
        let path = kit.put(MINE);
        let mut cfg: Cfg = toml::from_str(MINE).unwrap();
        assert_eq!(cfg.gitea_url(), None);

        let wrote = write_derived_fields(&kit, &mut cfg, false).unwrap();
        assert!(wrote.iter().any(|w| w == "gitea = \"http://10.0.0.5:4401\""), "{wrote:?}");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("gitea = \"http://10.0.0.5:4401\""), "{written}");
        // Строки человека целы все до одной: правится значение, а не файл.
        for line in MINE.lines() {
            assert!(written.contains(line), "строка человека пропала: {line}\n{written}");
        }
        // И ни одного ключа из образца, которого человек не писал.
        assert!(!written.contains("tools = ") && !written.contains("db_user = "), "{written}");
        let back: Cfg = toml::from_str(&written).unwrap();
        assert_eq!(back.gitea_url(), Some("http://10.0.0.5:4401"));

        // Второй прогон настройки уже ничего не выводит и не пишет.
        assert!(write_derived_fields(&kit, &mut cfg, false).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    /// Сухой прогон не пишет, но и не врёт: `cfg` в памяти правится, и план
    /// операций показывает то, что случится (находка ревью задачи 35).
    #[test]
    fn a_dry_run_derives_in_memory_and_writes_nothing() {
        let kit = TomlKit::new("dry");
        let path = kit.put(crate::cfg::EXAMPLE);
        let mut cfg: Cfg = toml::from_str(crate::cfg::EXAMPLE).unwrap();

        let wrote = write_derived_fields(&kit, &mut cfg, true).unwrap();
        assert_eq!(wrote.len(), 3, "{wrote:?}");
        assert_eq!(cfg.gitea_url(), Some("http://192.168.1.10:3391"), "план считается по выведенному");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), crate::cfg::EXAMPLE, "сухой прогон файл не трогает");
    }

    /// Файла параметров нет — дописывать некуда: своего toml программа тут не
    /// заводит и параметров в памяти не меняет.
    #[test]
    fn without_a_parameters_file_nothing_is_derived() {
        let kit = TomlKit::new("nofile");
        let mut cfg = Cfg::default();
        assert!(write_derived_fields(&kit, &mut cfg, false).unwrap().is_empty());
        assert_eq!(cfg, Cfg::default());
        assert!(!kit.state_path(crate::cfg::FILE).exists());
    }

    /// Единообразие: после настройки файл — полная картина того, чем
    /// программа пользуется (решение заказчика 2026-10-21). Каталог выгрузки
    /// и порт публикации пишутся тем же правилом, что адрес среды.
    #[test]
    fn install_writes_the_derived_dump_and_http_port() {
        let kit = TomlKit::new("fields");
        let path = kit.put(MINE);
        let mut cfg: Cfg = toml::from_str(MINE).unwrap();
        let dump = cfg.dump_dir();
        let wrote = write_derived_fields(&kit, &mut cfg, false).unwrap();

        assert_eq!(cfg.host.dump, dump);
        assert_eq!(cfg.host.http_port, Some(8082), "8080 + номер рабочего места");
        assert!(wrote.iter().any(|w| w == &format!("dump = {dump:?}")) && wrote.iter().any(|w| w == "http_port = 8082"), "{wrote:?}");
        let back: Cfg = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.host.dump, dump);
        assert_eq!(back.host.http_port, Some(8082));
        // Записанное умолчание ничего не меняет ни в адресах, ни у моста:
        // ключ `--dump` остаётся выключенным (см. `Cfg::dump_override`).
        assert_eq!(back.base_url(), "http://localhost:8082/hs/mcp-1c");
        assert_eq!(back.dump_dir(), dump);
        assert_eq!(back.dump_override(), None, "записанное умолчание — не свой каталог");
    }

    #[test]
    fn install_never_touches_a_written_address() {
        let kit = TomlKit::new("written");
        let text = format!("{MINE}gitea = \"http://localhost:3000\"\ndump = \"D:\\\\dump\"\nhttp_port = 9000\n");
        let path = kit.put(&text);
        let mut cfg: Cfg = toml::from_str(&text).unwrap();
        assert!(write_derived_fields(&kit, &mut cfg, false).unwrap().is_empty());
        assert_eq!(cfg.gitea_url(), Some("http://localhost:3000"));
        assert_eq!(cfg.host.dump, "D:\\dump");
        assert_eq!(cfg.host.http_port, Some(9000));
        assert_eq!(cfg.dump_override(), Some("D:\\dump"), "свой каталог мост передаёт");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn media_kind_of_the_system_drive_is_plain() {
        let root = if cfg!(windows) { Path::new("C:\\") } else { Path::new("/") };
        assert_eq!(media_kind(root), None);
    }

    #[test]
    fn required_kit_dirs_need_httpd_only_on_windows() {
        let dirs = required_kit_dirs();
        assert!(dirs.contains(&"vrunner"), "{dirs:?}");
        assert!(dirs.contains(&"git"), "{dirs:?}");
        assert_eq!(dirs.contains(&"httpd"), cfg!(windows), "{dirs:?}");
    }

    #[test]
    fn the_tools_stamp_lives_in_the_state_not_in_the_kit() {
        // The state-side stamp (`host.tools` writes it while laying tools
        // out) must resolve under `tools_dir()`, never under the kit —
        // `tools_state` reads it from `tools.join(VERSION_FILE)`, and
        // nothing in this module ever joins VERSION_FILE onto `tools_dir()`
        // under a path that also starts with a kit root.
        let kit_root = Path::new("/opt/dev1c-kit");
        let state = tools_dir();
        let laid_out_stamp = state.join(VERSION_FILE);
        assert!(!laid_out_stamp.starts_with(kit_root), "{}", laid_out_stamp.display());
        assert!(laid_out_stamp.starts_with(&state), "{}", laid_out_stamp.display());
    }

    #[test]
    fn toml_path_never_resolves_under_the_kit_on_linux() {
        if cfg!(windows) {
            return; // Windows: untouched by this task, the toml stays with the kit.
        }
        let kit = wire_state(Kit::at(Path::new("/opt/dev1c-kit")));
        let p = kit.state_path(crate::cfg::FILE);
        assert!(!p.starts_with(&kit.root), "{}", p.display());
        assert!(p.starts_with(tools_dir()), "{}", p.display());
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_kit_still_starts() {
        // A genuinely read-only mount, simulated by directory permissions:
        // `tools_state` only ever reads from `kit_root` (checking the
        // required directories, reading its version stamp) — it must not
        // need write access there to decide the state.
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("dev1c-kit-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let kit = tmp.join("kit");
        let home = tmp.join("home");
        std::fs::create_dir_all(kit.join("vrunner")).unwrap();
        std::fs::create_dir_all(kit.join("git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(kit.join(VERSION_FILE), "x\n").unwrap();
        let ro = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(kit.join("vrunner"), ro.clone()).unwrap();
        std::fs::set_permissions(kit.join("git"), ro.clone()).unwrap();
        std::fs::set_permissions(&kit, ro).unwrap();

        let state = tools_state(&kit, &home);

        let rw = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&kit, rw.clone()).unwrap();
        std::fs::set_permissions(kit.join("vrunner"), rw.clone()).unwrap();
        std::fs::set_permissions(kit.join("git"), rw).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        // Nothing laid out in `home` yet — that is Missing, not a panic or
        // an io error from a kit we could not write to.
        assert!(matches!(state, OpState::Missing), "{state:?}");
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
        std::fs::create_dir_all(kit.join("httpd")).unwrap();
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
        // В образе /opt/1cv8 — символьная ссылка на /opt/1cv8t: два разных пути,
        // один файл. Символьную ссылку в тесте создать нельзя (на Windows обычной
        // учётке не дают прав), а жёсткая ссылка там даёт РАЗНЫЕ canonicalize —
        // проверено живьём. Поэтому берём два разных написания одного пути: это
        // ровно то, что сворачивает `dedup_canonical`.
        let tmp = std::env::temp_dir().join(format!("dev1c-1cv8-dedup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let real = tmp.join("1cv8t");
        std::fs::write(&real, "x").unwrap();
        let same_other_spelling = tmp.join(".").join("1cv8t");
        let found = vec![("8.5.1.1150".to_string(), real.clone()), ("8.5.1.1150".to_string(), same_other_spelling)];
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
        std::fs::create_dir_all(bin.join("CFlite.resources")).unwrap();
        std::fs::write(bin.join("CFlite.resources").join("icudtl.dat"), "ресурс").unwrap();
        let to = tmp.join("shadow-root");

        let shadow_dir = shadow_platform(&platform, &to).unwrap();

        let shadow_bin = shadow_dir.join("bin");
        assert_eq!(std::fs::read(shadow_bin.join("1cv8.exe")).unwrap(), std::fs::read(bin.join("1cv8t.exe")).unwrap());
        assert_eq!(std::fs::read(shadow_bin.join("1cv8c.exe")).unwrap(), std::fs::read(bin.join("1cv8ct.exe")).unwrap());
        assert!(shadow_bin.join("some.dll").exists());
        // Подкаталоги bin (ресурсы CEF и т.п.) зеркалируются, а не пропускаются.
        assert_eq!(std::fs::read(shadow_bin.join("CFlite.resources").join("icudtl.dat")).unwrap(), "ресурс".as_bytes());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn shadow_platform_twice_does_not_duplicate_files() {
        // Половина раскладки могла прерваться (сбой, Ctrl+C) — повторный вызов
        // над тем же источником не должен падать и не должен плодить файлы.
        let tmp = std::env::temp_dir().join(format!("dev1c-shadow-twice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let platform = tmp.join("src").join("1cv8t").join("8.5.1.1150");
        let bin = platform.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("1cv8t.exe"), "платформа").unwrap();
        std::fs::write(bin.join("1cv8ct.exe"), "тонкий клиент").unwrap();
        let to = tmp.join("shadow-root");

        shadow_platform(&platform, &to).unwrap();
        let shadow_dir = shadow_platform(&platform, &to).unwrap();

        let names: std::collections::BTreeSet<String> =
            std::fs::read_dir(shadow_dir.join("bin")).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
        let expect: std::collections::BTreeSet<String> =
            ["1cv8.exe", "1cv8c.exe", "1cv8t.exe", "1cv8ct.exe"].iter().map(|s| s.to_string()).collect();
        assert_eq!(names, expect);
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

    /// Кодирует текст как настоящий файл 1CEStart.cfg: BOM UTF-16LE + содержимое.
    fn utf16le_with_bom(text: &str) -> Vec<u8> {
        let mut bytes = UTF16_BOM.to_vec();
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn the_1cestart_line_is_added_once() {
        // Повторный вызов (повторный запуск host.tools) не плодит строк, а файл
        // остаётся тем же UTF-16LE с BOM, что пишут инструменты 1С.
        let tmp = std::env::temp_dir().join(format!("dev1c-1cestart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg_path = tmp.join("1CEStart.cfg");
        std::fs::write(&cfg_path, utf16le_with_bom("ОбщегоНазначения=значение\r\n")).unwrap();
        let location = tmp.join("platform").join("8.5.1.1150");

        add_installed_location(&cfg_path, &location).unwrap();
        add_installed_location(&cfg_path, &location).unwrap();

        let bytes = std::fs::read(&cfg_path).unwrap();
        assert_eq!(&bytes[0..2], &UTF16_BOM, "BOM потерян при перезаписи");
        let text = read_1cestart_cfg(&cfg_path).unwrap();
        let line = format!("InstalledLocation={}", location.display());
        assert_eq!(text.lines().filter(|l| *l == line).count(), 1);
        assert!(text.contains("ОбщегоНазначения=значение"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_non_utf16_1cestart_cfg_is_left_alone() {
        // Файл неожиданной формы (нет BOM) не переписываем — испорченный список
        // версий стоит человеку всех ярлыков баз.
        let tmp = std::env::temp_dir().join(format!("dev1c-1cestart-ansi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg_path = tmp.join("1CEStart.cfg");
        std::fs::write(&cfg_path, "это не UTF-16").unwrap();
        let before = std::fs::read(&cfg_path).unwrap();

        let location = tmp.join("platform").join("8.5.1.1150");
        assert!(add_installed_location(&cfg_path, &location).is_err());
        assert_eq!(std::fs::read(&cfg_path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn nothing_the_connector_writes_lands_in_the_kit() {
        // Правило задачи 20 целиком, на проводке `wire_state`: файл
        // параметров, журнал, отчёт проверки, `known_hosts` и память между
        // прогонами на Linux уходят в состояние пользователя, потому что
        // комплект в образе один на всех и смонтирован только для чтения.
        // На Windows наоборот — комплект переносной и свой, писать рядом с
        // ним и нужно.
        let root = if cfg!(windows) { r"C:\dev1c-kit" } else { "/opt/dev1c-kit" };
        let kit = wire_state(Kit::at(Path::new(root)));
        for name in [crate::cfg::FILE, "1c-connect.log", "last-check.txt", "known_hosts", "install.state"] {
            let p = kit.state_path(name);
            if cfg!(windows) {
                assert!(p.starts_with(&kit.root), "{}", p.display());
            } else {
                assert!(!p.starts_with(&kit.root), "{}", p.display());
                assert!(p.starts_with(tools_dir()), "{}", p.display());
            }
        }
    }

    /// Живой отказ, ради которого вся эта обвязка: в контейнере (замер
    /// 2026-10-20) `1c-connect export` без `DISPLAY` падал с «Unable to
    /// initialize GTK+ or connect to the windowing system» и кодом 255 —
    /// vrunner поднимает Конфигуратор, а тот оконный. Экран уже есть —
    /// обёртка не нужна, иначе она отрезала бы человека от своего же экрана.
    #[test]
    fn display_prefix_is_none_when_display_is_set() {
        let x = PathBuf::from("/usr/bin/xvfb-run");
        assert_eq!(display_prefix(true, Some(&x)), None);
        assert_eq!(display_prefix(true, None), None);
    }

    #[test]
    fn display_prefix_names_xvfb_when_display_is_missing() {
        let x = PathBuf::from("/usr/bin/xvfb-run");
        if cfg!(target_os = "linux") {
            assert_eq!(display_prefix(false, Some(&x)), Some(x));
        }
        // Оборачивать нечем — зовём напрямую: пусть падает со своим текстом,
        // а `screen_check` до запуска скажет, что поставить.
        assert_eq!(display_prefix(false, None), None);
    }

    /// На Windows Конфигуратор рисует в сеансе пользователя, никакого xvfb
    /// там нет: ни одна новая ветка не должна срабатывать.
    #[test]
    #[cfg(windows)]
    fn display_prefix_is_none_on_windows() {
        assert_eq!(display_prefix(false, Some(Path::new(r"C:\tools\xvfb-run"))), None);
        assert!(find_xvfb_run().is_none(), "xvfb на Windows не ищем вовсе");
        assert!(display_wrapper().is_none(), "обёртки на Windows быть не должно");
        assert!(no_screen(false, None).is_none(), "и отказа «нет экрана» на Windows быть не должно");
    }

    /// Мёртвый `DISPLAY` — тот самый случай, что раньше проходил зелёным:
    /// переменная задана (`su -`, юнит init, проброшенное окружение), а
    /// X-сервера за ней нет, и человек получал сырой отказ GTK+.
    #[test]
    fn a_local_display_is_alive_only_with_its_socket() {
        // :999 не бывает ни на одной машине — сокета /tmp/.X11-unix/X999 нет
        assert!(!display_alive(":999"));
        assert!(!display_alive(":999.0"));
        // нечисловые и сетевые адреса дёшево не проверить — верим им
        assert!(display_alive("host:0"));
        assert!(display_alive("localhost:10.0"));
        assert!(display_alive(":pw"));
        // а живой местный дисплей — ровно тот, у которого сокет есть
        let live = Path::new("/tmp/.X11-unix").exists() && Path::new("/tmp/.X11-unix/X0").exists();
        assert_eq!(display_alive(":0"), live);
    }

    #[test]
    fn no_screen_speaks_only_when_there_is_nothing_to_wrap_with() {
        let x = PathBuf::from("/usr/bin/xvfb-run");
        assert_eq!(no_screen(true, None), None, "экран сеанса есть — молчим");
        assert_eq!(no_screen(false, Some(&x)), None, "обернём сами — молчим");
        let said = no_screen(false, None);
        if cfg!(target_os = "linux") {
            let said = said.expect("на Linux без экрана и без xvfb-run отказ обязателен");
            assert!(said.contains("xvfb") && said.contains("xauth"), "{said}");
        } else {
            assert_eq!(said, None);
        }
    }

    /// Второе, руками собранное место с тем же порядком — и единственное, где
    /// программа и аргументы разъезжаются по разным значениям (`docker::exec`
    /// берёт их строками, а не `Command`).
    #[test]
    fn display_argv_puts_xvfb_first_and_leaves_the_rest_in_order() {
        let args = ["DESIGNER", "/F/home/dev3/base", "/DeleteCfg"];
        let (prog, got) = display_argv("/opt/1cv8/1cv8", &args, Some(Path::new("/usr/bin/xvfb-run")));
        assert_eq!(prog, "/usr/bin/xvfb-run");
        assert_eq!(got, ["-a", "/opt/1cv8/1cv8", "DESIGNER", "/F/home/dev3/base", "/DeleteCfg"]);
        // Обёртки нет — команда уходит как была, ни программа, ни аргументы
        // не трогаются.
        let (prog, got) = display_argv("/opt/1cv8/1cv8", &args, None);
        assert_eq!(prog, "/opt/1cv8/1cv8");
        assert_eq!(got, args);
    }

    #[test]
    fn with_display_keeps_program_and_args_in_order() {
        let mut c = std::process::Command::new("oscript");
        c.args(["mcp.os", "--once"]);
        c.env("VRUNNER_DBUSER", "Администратор");
        c.current_dir("/home/dev3/project");
        let w = wrap_in_display(c, Path::new("/usr/bin/xvfb-run"));
        assert_eq!(w.get_program(), "/usr/bin/xvfb-run");
        let args: Vec<String> = w.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["-a", "oscript", "mcp.os", "--once"]);
        // Каталог и окружение переживают обёртку: без них vrunner не найдёт
        // ни проект, ни базу.
        assert_eq!(w.get_current_dir(), Some(Path::new("/home/dev3/project")));
        let envs: Vec<(String, Option<String>)> =
            w.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        assert!(envs.contains(&("VRUNNER_DBUSER".to_string(), Some("Администратор".to_string()))), "{envs:?}");

        // Проводка самого `with_display`, по результату, а не по тому же
        // решению, которое на него влияло: экран есть (или оборачивать
        // нечем) — команда уходит как была; иначе она обёрнута.
        let mut c = std::process::Command::new("oscript");
        c.arg("mcp.os");
        let w = with_display(c);
        let args: Vec<String> = w.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        if w.get_program().to_string_lossy().ends_with("xvfb-run") {
            assert_eq!(args, ["-a", "oscript", "mcp.os"]);
        } else {
            assert_eq!(w.get_program(), "oscript");
            assert_eq!(args, ["mcp.os"]);
        }
    }
}
