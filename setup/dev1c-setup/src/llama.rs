//! Сервер модели: что ему передать, что он ответил, что из этого следует
//! для Zed. Здесь только чистые функции — их и проверяют тесты; обёртки над
//! командами и HTTP — ниже, в разделе «обёртки».

use serde::Deserialize;

/// Меньше этого на агента — не кодинг на 1С (модуль в 100 КБ ≈ 40 000 токенов).
pub const MIN_BUDGET: u32 = 16384;
/// Слотов у сервера (`--parallel`): по одному на рабочее место.
pub const SLOTS: u16 = 3;
/// Сколько тёплых сеансов держит один человек: его слот плюс два отложенных
/// разговора (расчёт стенда в спеке — три пользователя по три сеанса).
pub const WARM_PER_SLOT: u16 = 3;
/// Ниже этого предел контейнеру модели не ставим: сервер с таким пределом
/// убьёт OOM на первом же запросе, и причина будет непонятной.
pub const MIN_MODEL_MB: u32 = 4096;
/// Хосту и движку контейнеров — столько, сколько им нужно, чтобы не уйти в
/// подкачку вместе с 1С (решение заказчика 2026-10-19).
pub const HOST_RESERVE_MB: u64 = 2048;

pub struct ServeInput<'a> {
    pub model: &'a str,
    pub alias: &'a str,
    pub host: &'a str,
    pub port: u16,
    /// Окно НА СЛОТ: в строку уходит общий пул (`pool_for`) — окно вместе с
    /// ответом на каждом слоте, то есть примерно `fit_ctx × SLOTS × 6/5`.
    pub fit_ctx: u32,
    pub fit_target: u32,
    pub cache_ram: u32,
    /// Токен путём к файлу (служба хоста); контейнер получает его
    /// переменной LLAMA_API_KEY через --env-file, и в строке его нет.
    pub token_file: Option<&'a str>,
}

/// Командная строка по спеке volt-llama. Окно НЕ задаём: подбор уменьшит его
/// под карту, а заданное окно он уменьшить не может и выносит слои на
/// процессор — генерация падает с 32 до 4 токенов в секунду.
///
/// `--fit-ctx` получает ОБЩИЙ пул (`pool_for`): при `--kv-unified` кэш у
/// слотов один, и занятый слот держит не окно, а окно вместе с ответом.
pub fn serve_args(i: &ServeInput) -> Vec<String> {
    let port = i.port.to_string();
    let fit_ctx = pool_for(i.fit_ctx, SLOTS).to_string();
    let slots = SLOTS.to_string();
    let fit_target = i.fit_target.to_string();
    let cache_ram = i.cache_ram.to_string();
    let mut a: Vec<String> = [
        "-m", i.model, "--alias", i.alias, "--host", i.host, "--port", &port,
        "--parallel", &slots, "--kv-unified", "--fit", "on", "--fit-ctx", &fit_ctx, "--fit-target", &fit_target,
        "--cache-ram", &cache_ram, "--ctx-checkpoints", "32",
        "-b", "2048", "-ub", "512", "--flash-attn", "on", "-ctk", "q8_0", "-ctv", "q8_0",
        "--jinja", "--reasoning-budget", "0", "--temp", "0.7", "--top-k", "40", "--top-p", "0.95", "--min-p", "0.3", "--metrics",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(f) = i.token_file {
        a.push("--api-key-file".into());
        a.push(f.into());
    }
    a
}

/// Кэш подсказок: половина предела контейнера модели, от 1024 до 8192 МиБ.
/// Считается от предела, а не от того, сколько памяти случайно свободно в
/// минуту установки. Ноль ломает слоты (простаивающему слоту некуда
/// переехать), -1 — без предела, и кэш выдавливает всё в подкачку. Потолок
/// 8192 — число стенда из спеки (22 ГБ контейнеру, 8 ГБ кэшу).
pub fn cache_ram_from_limit(limit_mb: u32) -> u32 {
    (limit_mb / 2).clamp(1024, 8192)
}

/// Предел памяти контейнера модели: что осталось от машины после среды и
/// запаса хосту. Не ниже пола и никогда «в минус».
pub fn container_memory_mb(total_mb: u64, env_limit_mb: u64, host_reserve_mb: u64) -> u32 {
    let left = total_mb.saturating_sub(env_limit_mb).saturating_sub(host_reserve_mb);
    (left.min(u32::MAX as u64) as u32).max(MIN_MODEL_MB)
}

/// Предел контейнера среды: что стоит на машине (`docker inspect`) — и
/// только если его там нет, число из параметров. Ноль у движка значит
/// «предела нет вовсе», и тогда верить ему нельзя.
pub fn env_limit_mb(from_engine: Option<u64>, from_cfg: &str) -> u64 {
    from_engine.filter(|n| *n > 0).or_else(|| parse_size_mb(from_cfg)).unwrap_or(0)
}

/// `24g`, `512m`, `2048` (байты движка) — в МиБ.
pub fn parse_size_mb(s: &str) -> Option<u64> {
    let t = s.trim();
    let (digits, factor) = match t.chars().last()? {
        'g' | 'G' => (&t[..t.len() - 1], 1024.0),
        'm' | 'M' => (&t[..t.len() - 1], 1.0),
        'k' | 'K' => (&t[..t.len() - 1], 1.0 / 1024.0),
        'b' | 'B' => (&t[..t.len() - 1], 1.0 / (1024.0 * 1024.0)),
        _ => (t, 1.0 / (1024.0 * 1024.0)),
    };
    let n: f64 = digits.trim().parse().ok()?;
    Some((n * factor) as u64)
}

/// Сколько байт KV-кэша приходится на токен у ЭТОЙ модели: сервер сообщает
/// размер кэша и окно, одно делится на другое. Из таблицы это брать нельзя —
/// она устареет с первой же новой моделью.
pub fn kv_bytes_per_token(kv_cache_mb: u32, window: u32) -> u32 {
    if window == 0 {
        return 0;
    }
    (kv_cache_mb as u64 * 1024 * 1024 / window as u64) as u32
}

/// Сколько токенов НА СЛОТ помещается в видеопамять, оставшуюся под KV.
pub fn window_that_fits(free_vram_mb: u32, kv_bytes_per_token: u32, slots: u16) -> u32 {
    if kv_bytes_per_token == 0 {
        return 0;
    }
    let tokens = free_vram_mb as u64 * 1024 * 1024 / kv_bytes_per_token as u64;
    (tokens / slots.max(1) as u64).min(u32::MAX as u64) as u32
}

/// Сколько оперативной памяти (и подкачки) займут контрольные точки тёплых
/// сеансов: столько сеансов по столько токенов.
pub fn warm_sessions_mb(kv_bytes_per_token: u32, window: u32, sessions: u16) -> u32 {
    let b = kv_bytes_per_token as u64 * window as u64 * sessions as u64;
    (b / (1024 * 1024)).min(u32::MAX as u64) as u32
}

/// Что объявляется рабочему месту. `slot_window` — окно ОДНОГО слота:
/// на стенде у каждого пользователя свой слот, и объявлять общий пул нельзя.
/// Zed просит пятую часть объявленного окна под ответ, поэтому `max_tokens`
/// обязан быть суммой входа и ответа — иначе редактор молча упирается в
/// предел на длинном диалоге.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZedNumbers {
    pub window: u32,
    pub max_output: u32,
    pub max_tokens: u32,
}

/// Несколько агентов у одного человека делят его слот — и только тогда.
pub fn zed_numbers(slot_window: u32, agents: u16) -> ZedNumbers {
    let window = slot_window / agents.max(1) as u32;
    let max_output = window / 5;
    ZedNumbers { window, max_output, max_tokens: window + max_output }
}

/// Какой пул нужен серверу, чтобы окно слота было настоящим: рабочее место
/// вправе занять весь свой `max_tokens` — вход И ответ, — и так на каждом
/// слоте. Число берётся у `zed_numbers`, а не считается здесь заново: иначе
/// пул и объявленное Zed окно разошлись бы на пятую часть молча.
pub fn pool_for(slot_window: u32, slots: u16) -> u32 {
    // Пятая часть под ответ считалась с округлением вниз, и обратный счёт
    // (`slot_window_in`) возвращал на токен меньше: `verdict` отказывал верной
    // настройке словами «просят 65536, помещается 65535» — на умолчании
    // комплекта. Остаток добавляем назад, то есть просим пул с округлением
    // ВВЕРХ: лишний токен у карты безопасен, а объявить рабочему месту окно,
    // которого в пуле нет, нельзя.
    let per_slot = zed_numbers(slot_window, 1).max_tokens as u64 + u64::from(slot_window % 5 != 0);
    (per_slot * slots.max(1) as u64).min(u32::MAX as u64) as u32
}

/// Обратное к `pool_for`: какое окно слота честно помещается в пул, который
/// сервер действительно выделил. Делит вниз — объявленное окно не вправе
/// оказаться больше того, что пул держит; туда и назад проходит без потерь
/// (`slot_window_in(pool_for(w, s), s) == w`).
pub fn slot_window_in(pool: u32, slots: u16) -> u32 {
    let share = pool / slots.max(1) as u32;
    // Окно плюс его пятая часть должны уложиться в долю слота.
    (share as u64 * 5 / 6) as u32
}

/// Размер KV-кэша в журнале сервера, сложенный по устройствам: ни `/props`,
/// ни `/metrics` его не сообщают, а строку «KV buffer size» llama.cpp печатает
/// при загрузке всегда.
/// Берётся ПОСЛЕДНЯЯ загрузка, а не весь хвост: после перезапуска в журнале
/// лежит два таких блока, и сложенные вместе они дали бы вдвое больший кэш —
/// то есть вдвое больший размер KV на токен, провал проверки на здоровой
/// карте и неверное число, записанное в состояние комплекта.
pub fn kv_cache_mb_from_log(text: &str) -> Option<u32> {
    let mut sum = 0.0;
    for l in text.lines().rev() {
        if !l.contains("KV buffer size") {
            // Строка не из блока: если блок уже собран, он и есть последний.
            if sum > 0.0 {
                break;
            }
            continue;
        }
        if let Some(n) = l.rsplit('=').next().and_then(|t| t.split_whitespace().next()).and_then(|n| n.parse::<f64>().ok()) {
            sum += n;
        }
    }
    (sum > 0.0).then_some(sum as u32)
}

/// Карта с рабочим столом занята уже в покое: оставляем ей вдвое больше.
pub fn fit_target_mb(used_mb_idle: u64) -> u32 {
    if used_mb_idle > 300 {
        2048
    } else {
        1024
    }
}

pub struct Props {
    pub window: u32,
    pub slots: u16,
    pub model: String,
}

#[derive(Deserialize)]
struct PropsJson {
    default_generation_settings: Option<Gen>,
    total_slots: Option<u16>,
    model_path: Option<String>,
}
#[derive(Deserialize)]
struct Gen {
    n_ctx: Option<u32>,
}

pub fn parse_props(json: &str) -> Option<Props> {
    let p: PropsJson = serde_json::from_str(json).ok()?;
    let window = p.default_generation_settings?.n_ctx?;
    let path = p.model_path.unwrap_or_default();
    let model = path.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
    Some(Props { window, slots: p.total_slots.unwrap_or(1), model })
}

/// Скорость разбора из ответа /v1/chat/completions. Меньше 500 посчитанных
/// токенов — это кэш подсказок, а не модель.
pub fn prefill_from(json: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let t = v.get("timings")?;
    if t.get("prompt_n")?.as_u64()? < 500 {
        return None;
    }
    t.get("prompt_per_second")?.as_f64()
}

pub struct Gpu {
    pub name: String,
    pub total_mb: u64,
    pub used_mb: u64,
    pub driver: String,
}

/// `nvidia-smi --query-gpu=name,memory.total,memory.used,driver_version --format=csv,noheader`
pub fn parse_nvidia_smi(csv: &str) -> Option<Gpu> {
    let line = csv.lines().find(|l| l.contains(','))?;
    let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
    if f.len() < 4 {
        return None;
    }
    let mb = |s: &str| s.split_whitespace().next()?.parse::<u64>().ok();
    Some(Gpu { name: f[0].to_string(), total_mb: mb(f[1])?, used_mb: mb(f[2])?, driver: f[3].to_string() })
}

// ------------------------------------------------------------ обёртки

use crate::cfg::Form;
use crate::ops::Ctx;
use anyhow::{Context, Result};
use common::docker;
use common::kit::Kit;
use common::Runner;
use std::path::PathBuf;

pub const SERVICE: &str = "dev1c-llama";

/// Каталог службы хоста: его закрывают root/SYSTEM, потому что подложенный
/// туда `llama-server` выполнился бы от её имени. Только форма host кладёт
/// сюда секрет; форма docker — см. `token_path`.
pub fn dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\dev1c\llama")
    } else {
        PathBuf::from("/opt/dev1c/llama")
    }
}
pub fn server_exe() -> PathBuf {
    dir().join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" })
}
pub fn log_file() -> PathBuf {
    dir().join("llama.log")
}

/// Где лежит токен сервера. Форма host — в закрытом каталоге службы (его
/// открывает только root/SYSTEM, потому что от его имени выполняется
/// подложенный `llama-server`). Форма docker не трогает `/opt`: секрет
/// нужен только установщику и контейнеру, не службе, и лежит в комплекте —
/// у того, кто ставит, на него и так есть права.
pub fn token_path(kit: &Kit, form: Form) -> PathBuf {
    if form == Form::Docker {
        kit.path("dev1c-llama.token")
    } else {
        dir().join("dev1c-llama.token")
    }
}

/// Файл переменных для контейнера модели (LLAMA_API_KEY=…): только форма
/// docker его использует, и лежит он рядом с токеном — в комплекте.
pub fn env_path(kit: &Kit) -> PathBuf {
    kit.path("llama.env")
}

/// Записать файл переменных контейнера заново (токен мог смениться).
pub fn write_env_file(kit: &Kit, token: &str) -> Result<()> {
    let f = env_path(kit);
    let _ = std::fs::remove_file(&f);
    write_secret(&f, &format!("LLAMA_API_KEY={token}\n"))
}
/// Ряд бинарников под эту ОС в комплекте.
pub fn kit_bin(ctx: &Ctx) -> Option<PathBuf> {
    let p = ctx.kit.path(if cfg!(windows) { "llama/bin/windows-x64-cuda13" } else { "llama/bin/linux-x64-cuda13" });
    p.join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" }).exists().then_some(p)
}
/// nssm есть, откуда взять: в комплекте (llama/nssm.exe) или уже наша копия
/// в закрытом каталоге. Чужой файл на машине не берём.
pub fn nssm_available(ctx: &Ctx) -> bool {
    ctx.kit.path("llama/nssm.exe").exists() || dir().join("nssm.exe").exists()
}

/// nssm, которым регистрируется и управляется служба: только копия в
/// закрытом каталоге (её кладёт шаг llama.bin). Путь к нему запекается в
/// запись службы, и с флешки он быть не должен.
fn nssm_exe() -> Result<String> {
    let n = dir().join("nssm.exe");
    anyhow::ensure!(n.exists(), "нет {}: его туда кладёт шаг llama.bin", n.display());
    Ok(n.to_string_lossy().to_string())
}

/// Каталог службы — во владении администраторов и закрыт: SYSTEM и
/// администраторы, больше никто (Linux: root, 755). Сброс рекурсивный и при
/// каждой установке: иначе подмена llama-server.exe — это выполнение кода от
/// службы, а каталог, созданный кем-то заранее, остался бы его.
pub fn ensure_dir(ctx: &Ctx) -> Result<()> {
    let d = dir();
    std::fs::create_dir_all(&d)?;
    // Перенаправление (junction, символическая ссылка) на месте каталога
    // службы — подмена: закрывать права мы стали бы у чужого каталога.
    for p in [d.parent().map(|p| p.to_path_buf()), Some(d.clone())].into_iter().flatten() {
        if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            anyhow::bail!("{} — ссылка, а не каталог: удалите её и повторите", p.display());
        }
    }
    if cfg!(windows) {
        let mut paths = vec![d.to_string_lossy().to_string()];
        if let Some(parent) = d.parent() {
            paths.insert(0, parent.to_string_lossy().to_string());
        }
        for path in &paths {
            let o = docker::exec(ctx, "takeown", &["/F", path, "/A", "/R", "/D", "Y"])?;
            anyhow::ensure!(o.ok(), "не забрать {path} во владение администраторов: {}", o.tail(2).join(" "));
            // Права — каталогу, без /T: наследуемые записи (OI)(CI), прописанные
            // файлам напрямую, на файл не действуют, и он остаётся без единой
            // записи — недоступен даже администратору (проверено: nssm.exe не
            // запускался, токен не читался). Детям — /reset: они наследуют.
            let o = docker::exec(ctx, "icacls", &[path, "/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/Q"])?;
            anyhow::ensure!(o.ok(), "не закрыть права на {path}: {}", o.tail(2).join(" "));
            let children = format!("{path}\\*");
            let o = docker::exec(ctx, "icacls", &[&children, "/reset", "/T", "/Q"])?;
            anyhow::ensure!(o.ok(), "не передать права файлам в {path}: {}", o.tail(2).join(" "));
        }
    } else {
        let path = d.to_string_lossy().to_string();
        let o = docker::exec(ctx, "chown", &["-R", "root:root", &path])?;
        anyhow::ensure!(o.ok(), "не сменить владельца {path}: {}", o.tail(2).join(" "));
        let o = docker::exec(ctx, "chmod", &["755", &path])?;
        anyhow::ensure!(o.ok(), "не закрыть права на {path}: {}", o.tail(2).join(" "));
    }
    Ok(())
}

/// Файл с секретом: создаётся сразу с закрытыми правами, не после записи.
fn write_secret(path: &std::path::Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path).with_context(|| format!("не создать {}", path.display()))?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

/// 128 бит из генератора ОС: BCryptGenRandom на Windows, /dev/urandom на
/// Linux. Хэш-функция std для токена не годится — она не криптографическая.
pub fn random_hex() -> String {
    let mut buf = [0u8; 16];
    os_random(&mut buf).expect("системный генератор случайных чисел недоступен");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(windows)]
fn os_random(buf: &mut [u8]) -> Result<()> {
    use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    anyhow::ensure!(status == 0, "BCryptGenRandom: статус {status:#x}");
    Ok(())
}

#[cfg(not(windows))]
fn os_random(buf: &mut [u8]) -> Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)?;
    Ok(())
}

/// Токен, если он уже выписан. Живёт только в своём файле.
pub fn read_token(kit: &Kit, form: Form) -> Option<String> {
    let t = std::fs::read_to_string(token_path(kit, form)).ok()?;
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Токен сервера: файл в закрытом каталоге службы (Linux: 0600, права
/// наследуются от каталога — SYSTEM и администраторы) для формы host, и в
/// комплекте для формы docker (её каталог не закрываем — см. `token_path`).
/// Каталог host закрывается заново и до чтения существующего файла: так
/// старый токен тоже оказывается под замком.
pub fn ensure_token(ctx: &Ctx) -> Result<String> {
    let form = ctx.cfg.form();
    if form != Form::Docker {
        ensure_dir(ctx)?;
    }
    if let Some(t) = read_token(&ctx.kit, form) {
        return Ok(t);
    }
    let f = token_path(&ctx.kit, form);
    anyhow::ensure!(!f.exists(), "{} есть, но не читается: проверьте права на каталог", f.display());
    let token = random_hex();
    write_secret(&f, &token)?;
    Ok(token)
}

pub fn gpu(ctx: &Ctx) -> Option<Gpu> {
    let o = docker::probe(ctx, "nvidia-smi", &["--query-gpu=name,memory.total,memory.used,driver_version", "--format=csv,noheader"]).ok()?;
    if !o.ok() {
        return None;
    }
    parse_nvidia_smi(&o.text)
}

/// Строка `/proc/meminfo` в МиБ (значения там в КиБ).
#[cfg(not(windows))]
fn meminfo_mb(key: &str) -> Option<u64> {
    let t = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = t.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

/// Вся память машины: от неё считается предел контейнера модели.
pub fn total_ram_mb(ctx: &Ctx) -> u64 {
    #[cfg(windows)]
    {
        docker::probe(ctx, "powershell", &["-NoProfile", "-Command", "(Get-CimInstance Win32_OperatingSystem).TotalVisibleMemorySize"])
            .ok()
            .and_then(|o| o.text.trim().lines().next()?.trim().parse::<u64>().ok())
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    }
    #[cfg(not(windows))]
    {
        let _ = ctx;
        meminfo_mb("MemTotal:").unwrap_or(0)
    }
}

/// Файл подкачки, как его видит ядро. На Windows его размером не управляем
/// и проверку не показываем вовсе — отсюда None.
pub fn swap_total_mb() -> Option<u64> {
    #[cfg(windows)]
    {
        None
    }
    #[cfg(not(windows))]
    {
        meminfo_mb("SwapTotal:")
    }
}

/// Предел памяти контейнера среды, который стоит на машине: байты из
/// `docker inspect`, переведённые в МиБ. Контейнера нет — None.
pub fn engine_env_limit_mb(ctx: &Ctx) -> Option<u64> {
    let t = docker::inspect(ctx, &ctx.cfg.server.name, "{{.HostConfig.Memory}}");
    t.trim().parse::<u64>().ok().map(|b| b / (1024 * 1024))
}

/// Предел памяти контейнера модели: заданный в параметрах, иначе остаток
/// машины после контейнера среды и запаса хосту.
///
/// None — машина не ответила, сколько у неё памяти. Тогда предела не ставим
/// вовсе: пол (`MIN_MODEL_MB`) выглядел бы как решение, а на деле это 4 ГБ
/// работающему серверу и пересоздание контейнера на пустом месте.
pub fn limit_mb(ctx: &Ctx) -> Option<u32> {
    if let Some(n) = parse_size_mb(&ctx.cfg.model.memory) {
        return Some((n.min(u32::MAX as u64) as u32).max(MIN_MODEL_MB));
    }
    let total = total_ram_mb(ctx);
    if total == 0 {
        return None;
    }
    let env = env_limit_mb(engine_env_limit_mb(ctx), &ctx.cfg.server.memory);
    Some(container_memory_mb(total, env, HOST_RESERVE_MB))
}

/// Сколько видеопамяти может пойти под KV-кэш: свободное сейчас плюс то, что
/// кэш нашего сервера уже держит (иначе занятая им память читалась бы как
/// «карта больше не вмещает окно, которое сама же и вмещает»).
pub fn vram_for_kv_mb(ctx: &Ctx, kv_bytes_per_token: u32, window: u32) -> u32 {
    let free = gpu(ctx).map(|g| g.total_mb.saturating_sub(g.used_mb)).unwrap_or(0);
    let held = kv_bytes_per_token as u64 * window as u64 / (1024 * 1024);
    (free + held).min(u32::MAX as u64) as u32
}

/// Журнал сервера модели: там и только там llama.cpp называет размер
/// KV-кэша. Форма docker — журнал контейнера, форма host — файл службы
/// (Windows, nssm) или журнал systemd (Linux).
pub fn server_log(ctx: &Ctx) -> String {
    if ctx.cfg.form() == Form::Docker {
        return docker::ask(ctx, &["logs", "--tail", "400", &ctx.cfg.llama_container()]).map(|o| o.text).unwrap_or_default();
    }
    let file = std::fs::read_to_string(log_file()).unwrap_or_default();
    if !file.is_empty() || cfg!(windows) {
        return file;
    }
    docker::probe(ctx, "journalctl", &["-u", SERVICE, "-n", "400", "--no-pager"]).map(|o| o.text).unwrap_or_default()
}

/// Где слушать: на Windows петля (Docker Desktop доставляет host.docker.internal
/// до неё — замерено), на Linux адрес моста — виден контейнерам и машине, не сети.
pub fn bind_host(ctx: &Ctx) -> String {
    if cfg!(windows) {
        return "127.0.0.1".into();
    }
    docker::probe(ctx, &ctx.engine(), &["network", "inspect", "bridge", "-f", "{{(index .IPAM.Config 0).Gateway}}"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.text.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "172.17.0.1".into())
}

/// Занят ли порт на том адресе, где будет слушать служба.
pub fn port_busy(host: &str, port: u16) -> bool {
    std::net::TcpListener::bind((host, port)).is_err()
}

/// Запущена ли программа с правами администратора (root на Linux):
/// без них не зарегистрировать службу и не закрыть каталог.
pub fn is_admin(ctx: &Ctx) -> bool {
    if cfg!(windows) {
        docker::probe(ctx, "net", &["session"]).map(|o| o.ok()).unwrap_or(false)
    } else {
        docker::probe(ctx, "id", &["-u"]).map(|o| o.ok() && o.text.trim() == "0").unwrap_or(false)
    }
}

/// Строка параметров службы: аргументы с пробелами в кавычках — nssm и
/// systemd читают её как одну строку.
pub fn quote_args(args: &[String]) -> String {
    args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ")
}

pub struct Installed {
    pub present: bool,
    pub running: bool,
}

pub fn installed(ctx: &Ctx) -> Installed {
    if cfg!(windows) {
        match docker::probe(ctx, "sc", &["query", SERVICE]) {
            Ok(o) if o.ok() => Installed { present: true, running: o.text.contains("RUNNING") },
            _ => Installed { present: false, running: false },
        }
    } else {
        let present = docker::probe(ctx, "systemctl", &["cat", SERVICE]).map(|o| o.ok()).unwrap_or(false);
        let running = present && docker::probe(ctx, "systemctl", &["is-active", SERVICE]).map(|o| o.text.trim() == "active").unwrap_or(false);
        Installed { present, running }
    }
}

pub fn service_install(ctx: &Ctx, args: &[String]) -> Result<()> {
    let exe = server_exe().to_string_lossy().to_string();
    if cfg!(windows) {
        let nssm = nssm_exe()?;
        let o = docker::exec(ctx, &nssm, &["install", SERVICE, &exe])?;
        anyhow::ensure!(o.ok(), "служба не зарегистрирована: {}", o.tail(3).join(" "));
        let d = dir().to_string_lossy().to_string();
        let log = log_file().to_string_lossy().to_string();
        // Параметры — одной строкой с кавычками: переданные по одному, nssm
        // склеивает их пробелами, и путь с пробелом распадается надвое.
        let params = quote_args(args);
        // Каждая настройка отдельной командой: nssm их так и принимает.
        for pair in [
            vec!["set", SERVICE, "AppParameters", &params],
            vec!["set", SERVICE, "AppDirectory", &d],
            vec!["set", SERVICE, "AppStdout", &log],
            vec!["set", SERVICE, "AppStderr", &log],
            vec!["set", SERVICE, "AppRotateFiles", "1"],
            vec!["set", SERVICE, "AppRotateBytes", "10485760"],
            // Упавший сервер поднимается сам: иначе агенты молчат до прихода администратора.
            vec!["set", SERVICE, "AppExit", "Default", "Restart"],
            vec!["set", SERVICE, "Start", "SERVICE_AUTO_START"],
            vec!["set", SERVICE, "DisplayName", "dev1c — сервер модели"],
        ] {
            let o = docker::exec(ctx, &nssm, &pair)?;
            anyhow::ensure!(o.ok(), "не настроить службу: {}", o.tail(2).join(" "));
        }
    } else {
        let line = quote_args(args);
        let unit = format!(
            "[Unit]\nDescription=dev1c — сервер модели\nAfter=network-online.target docker.service\n\n[Service]\nType=simple\nExecStart={exe} {line}\nRestart=on-failure\nRestartSec=5\nWorkingDirectory={}\n\n[Install]\nWantedBy=multi-user.target\n",
            dir().display()
        );
        common::kit::write_text(std::path::Path::new(&format!("/etc/systemd/system/{SERVICE}.service")), &unit)?;
        let o = docker::exec(ctx, "systemctl", &["daemon-reload"])?;
        anyhow::ensure!(o.ok(), "systemd не перечитал юниты: {}", o.tail(2).join(" "));
        let o = docker::exec(ctx, "systemctl", &["enable", SERVICE])?;
        anyhow::ensure!(o.ok(), "служба не включена: {}", o.tail(2).join(" "));
    }
    Ok(())
}

fn service_cmd(ctx: &Ctx, verb: &str) -> Result<docker::Out> {
    if cfg!(windows) {
        let nssm = nssm_exe()?;
        docker::exec(ctx, &nssm, &[verb, SERVICE])
    } else {
        docker::exec(ctx, "systemctl", &[verb, SERVICE])
    }
}
pub fn service_stop(ctx: &Ctx) -> Result<()> {
    let o = service_cmd(ctx, "stop")?;
    anyhow::ensure!(o.ok(), "служба не остановилась: {}", o.tail(3).join(" "));
    Ok(())
}
pub fn service_start(ctx: &Ctx) -> Result<()> {
    let o = service_cmd(ctx, "start")?;
    anyhow::ensure!(o.ok(), "служба не запустилась: {}", o.tail(3).join(" "));
    Ok(())
}
pub fn service_remove(ctx: &Ctx) -> Result<()> {
    let _ = service_cmd(ctx, "stop");
    if cfg!(windows) {
        if let Ok(n) = nssm_exe() {
            let _ = docker::exec(ctx, &n, &["remove", SERVICE, "confirm"]);
        }
    } else {
        let _ = docker::exec(ctx, "systemctl", &["disable", SERVICE]);
        let _ = std::fs::remove_file(format!("/etc/systemd/system/{SERVICE}.service"));
        let _ = docker::exec(ctx, "systemctl", &["daemon-reload"]);
    }
    Ok(())
}

fn client(secs: u64) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(secs)).build().expect("reqwest client")
}

/// Модель читается с диска: на гигабайтах это минуты.
pub fn wait_health(url: &str, token: &str, secs: u64) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let c = client(10);
    loop {
        let last = match c.get(format!("{url}/health")).bearer_auth(token).send() {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => format!("ответ {}", r.status().as_u16()),
            Err(e) => e.to_string(),
        };
        anyhow::ensure!(std::time::Instant::now() < deadline, "сервер модели не готов за {secs} с: {last}");
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

pub fn props(url: &str, token: &str) -> Option<Props> {
    let r = client(10).get(format!("{url}/props")).bearer_auth(token).send().ok()?;
    parse_props(&r.text().ok()?)
}

/// Один длинный запрос с меткой в начале (иначе сервер отдаст скорость кэша,
/// а не модели) и запретом кэша подсказок.
pub fn measure_prefill(url: &str, token: &str) -> Option<f64> {
    let filler = format!("{} {}", random_hex(), "Регламент перевозки опасных грузов определяет порядок погрузки, крепления и сопровождения. ".repeat(110));
    let body = serde_json::json!({ "messages": [{"role": "user", "content": filler}], "max_tokens": 1, "stream": false, "cache_prompt": false });
    let r = client(600)
        .post(format!("{url}/v1/chat/completions"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .ok()?;
    prefill_from(&r.text().ok()?)
}

/// Числа замера — в состоянии комплекта; токен туда не пишется, он
/// читается из своего файла (комплект лежит на флешке с обычными правами).
pub fn facts_save(ctx: &Ctx, f: &crate::cfg::Facts, prefill: Option<f64>) -> Result<()> {
    ctx.kit.state_set("llama.window", &f.window.to_string())?;
    ctx.kit.state_set("llama.max_tokens", &f.max_tokens.to_string())?;
    ctx.kit.state_set("llama.max_output", &f.max_output.to_string())?;
    ctx.kit.state_set("llama.prefill", &prefill.map(|p| format!("{p:.0}")).unwrap_or_default())
}

pub fn facts_load(ctx: &Ctx) -> Option<crate::cfg::Facts> {
    Some(crate::cfg::Facts {
        token: read_token(&ctx.kit, ctx.cfg.form())?,
        window: ctx.kit.state_get("llama.window")?.parse().ok()?,
        max_tokens: ctx.kit.state_get("llama.max_tokens")?.parse().ok()?,
        max_output: ctx.kit.state_get("llama.max_output")?.parse().ok()?,
    })
}

/// Байт KV на токен, посчитанный замером: число модели, а не таблицы.
pub fn kv_per_token(ctx: &Ctx) -> Option<u32> {
    ctx.kit.state_get("llama.kv_bytes_per_token")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(token_file: Option<&'a str>) -> ServeInput<'a> {
        ServeInput { model: "/m/x.gguf", alias: "x", host: "127.0.0.1", port: 1235, fit_ctx: 65536, fit_target: 1024, cache_ram: 4096, token_file }
    }

    #[test]
    fn the_window_is_left_to_the_fit_and_the_token_goes_by_path() {
        let a = serve_args(&input(Some("/k/dev1c-llama.token")));
        assert!(!a.iter().any(|x| x == "-c" || x == "--ctx-size"), "окно не задаём: {a:?}");
        let at = |k: &str| a.iter().position(|x| x == k).map(|i| a[i + 1].clone());
        // Окно задаётся на слот, а серверу уходит общий пул: три слота по
        // 65536 вместе с ответом (65536 + 13108) × 3 — пятая часть с
        // округлением вверх, иначе обратный счёт теряет токен.
        assert_eq!(at("--fit-ctx").as_deref(), Some("235932"));
        assert_eq!(at("--fit-target").as_deref(), Some("1024"));
        assert_eq!(at("--parallel").as_deref(), Some("3"));
        assert!(a.iter().any(|x| x == "--kv-unified"));
        assert_eq!(at("--cache-ram").as_deref(), Some("4096"));
        assert_eq!(at("--api-key-file").as_deref(), Some("/k/dev1c-llama.token"));
        assert_eq!(at("--host").as_deref(), Some("127.0.0.1"));
        assert_eq!(at("--alias").as_deref(), Some("x"));
        assert!(!a.iter().any(|x| x == "--api-key"), "значение токена в строке недопустимо");
    }

    #[test]
    fn without_a_token_file_nothing_about_the_token_is_in_the_line() {
        let a = serve_args(&input(None));
        assert!(!a.iter().any(|x| x == "--api-key-file"));
    }

    #[test]
    fn the_window_that_fits_comes_from_the_model_not_from_a_table() {
        // Сервер сообщил: окно 65 536 токенов заняло 640 МиБ кэша →
        // 10 КиБ на токен (гибридная модель класса Qwen3.6-35B-A3B).
        assert_eq!(kv_bytes_per_token(640, 65536), 10 * 1024);
        // 13 ГиБ свободной видеопамяти, три слота: по 250k на слот и ещё остаётся.
        assert!(window_that_fits(13312, 10 * 1024, 3) >= 250_000);
        // 32 КиБ на токен (класс Qwen3.8-27B): три слота по 250k уже не помещаются.
        assert!(window_that_fits(13312, 32 * 1024, 3) < 250_000);
        // Девять тёплых сеансов по 250k при 10 КиБ — это 22 ГиБ.
        assert_eq!(warm_sessions_mb(10 * 1024, 250_000, 9), 21_972);
    }

    #[test]
    fn what_zed_is_told_matches_one_slot_and_carries_the_answer_too() {
        // Слот 250k, один агент: окно слота целиком, ответ — пятая часть,
        // max_tokens — их сумма, иначе Zed упрётся в предел на длинном диалоге.
        let z = zed_numbers(250_000, 1);
        assert_eq!(z.window, 250_000);
        assert_eq!(z.max_output, 50_000);
        assert_eq!(z.max_tokens, 300_000);
        // Три агента у одного человека делят его слот.
        assert_eq!(zed_numbers(250_000, 3).window, 83_333);
    }

    #[test]
    fn the_pool_holds_what_zed_is_allowed_to_ask_on_every_slot() {
        // Zed вправе занять весь max_tokens на каждом слоте: пул обязан быть
        // не окном × слоты, а max_tokens × слоты — иначе объявленное окно на
        // пятую часть больше, чем сервер может держать.
        assert_eq!(pool_for(250_000, 3), 300_000 * 3);
        assert_eq!(pool_for(250_000, 3), zed_numbers(250_000, 1).max_tokens * 3);
        // И обратно: в выделенный пул помещается ровно то окно, из которого он
        // посчитан — туда и назад без потерь.
        assert_eq!(slot_window_in(900_000, 3), 250_000);
        // Это и есть свойство, на которое опирается ops_llama::verdict (он
        // отказывает, если окно слота вышло меньше fit_ctx): пул, посчитанный
        // из окна, обязан вернуть ТО ЖЕ окно — не на токен меньше. Деление на
        // пять вниз теряло его при fit_ctx = 65536, то есть на умолчании
        // комплекта, и верная настройка получала отказ.
        for w in [1u32, 16_384, 65_536, 65_537, 131_072, 250_000, 262_144, 300_001] {
            for s in [1u16, 2, 3, 8] {
                assert_eq!(slot_window_in(pool_for(w, s), s), w, "окно {w}, слотов {s}");
            }
        }
    }

    #[test]
    fn the_model_container_gets_what_is_left_of_the_machine() {
        // Стенд: 32 ГБ всего, 8 ГБ среде, 2 ГБ хосту.
        assert_eq!(container_memory_mb(32768, 8192, 2048), 22528);
        // Маленькая машина не уходит в минус и не получает предел ниже пола.
        assert_eq!(container_memory_mb(8192, 8192, 2048), MIN_MODEL_MB);
    }

    #[test]
    fn the_environment_limit_is_read_from_the_engine_not_assumed() {
        // Контейнер среды мог поднять provision.sh со своим числом: берём то,
        // что стоит на самом деле, а параметры — только когда его ещё нет.
        assert_eq!(env_limit_mb(Some(12288), "8g"), 12288);
        assert_eq!(env_limit_mb(None, "8g"), 8192);
        assert_eq!(env_limit_mb(Some(0), "8g"), 8192); // 0 — предела нет вовсе
    }

    #[test]
    fn the_prompt_cache_is_half_the_container_not_half_the_machine() {
        assert_eq!(cache_ram_from_limit(22528), 8192); // упирается в потолок
        assert_eq!(cache_ram_from_limit(4096), 2048);
        assert_eq!(cache_ram_from_limit(1024), 1024); // не ниже пола
    }

    #[test]
    fn the_size_of_the_kv_cache_is_read_from_the_server_log() {
        // Размера кэша нет ни в /props, ни в /metrics: llama.cpp называет его
        // только в журнале, и по устройствам — значит складываем.
        let block = "llama_kv_cache: CUDA0 KV buffer size =   320.00 MiB\nllama_kv_cache: CUDA1 KV buffer size =   320.00 MiB\n";
        let log = format!("{block}llama_context: n_ctx = 65536\n");
        assert_eq!(kv_cache_mb_from_log(&log), Some(640));
        assert_eq!(kv_cache_mb_from_log("llama_context: n_ctx = 65536"), None);
        // Перезапуск: в хвосте журнала два блока, и считается последний, а не
        // сумма — иначе кэш удваивается и здоровая карта «перестаёт вмещать».
        let twice = format!("{log}srv    operator(): operator called, restarting\n{log}");
        assert_eq!(kv_cache_mb_from_log(&twice), Some(640));
        // Предел памяти движка приходит в байтах; ноль — предела нет.
        assert_eq!(parse_size_mb("22528m"), Some(22528));
        assert_eq!(parse_size_mb("22g"), Some(22528));
        assert_eq!(parse_size_mb(""), None);
    }

    #[test]
    fn a_desktop_on_the_card_doubles_the_fit_target() {
        assert_eq!(fit_target_mb(0), 1024);
        assert_eq!(fit_target_mb(200), 1024);
        assert_eq!(fit_target_mb(1250), 2048);
    }

    #[test]
    fn props_and_timings_are_read_from_the_server_json() {
        let p = parse_props(r#"{"default_generation_settings":{"n_ctx":131072},"total_slots":3,"model_path":"/m/x.gguf"}"#).unwrap();
        assert_eq!((p.window, p.slots, p.model.as_str()), (131072, 3, "x.gguf"));
        assert!(parse_props("{}").is_none());
        assert_eq!(prefill_from(r#"{"timings":{"prompt_n":1800,"prompt_per_second":2600.5}}"#), Some(2600.5));
        assert_eq!(prefill_from(r#"{"timings":{"prompt_n":4,"prompt_per_second":9000.0}}"#), None, "горстка токенов — это кэш, не замер");
    }

    #[test]
    fn facts_round_trip_through_the_kit_state_and_the_token_is_random_hex() {
        let dir = std::env::temp_dir().join(format!("dev1c-llama-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir_for_test = dir.clone();
        let kit = common::kit::Kit::at(&dir);
        let ctx = crate::ops::Ctx { kit, cfg: crate::cfg::Cfg::default(), dry_run: false, sink: std::sync::Arc::new(|_| {}), prompt: Box::new(common::AlwaysDefault) };
        assert!(ctx.kit.state_get("llama.window").is_none());
        let f = crate::cfg::Facts { token: "abc".into(), window: 250_000, max_tokens: 300_000, max_output: 50_000 };
        facts_save(&ctx, &f, Some(2600.0)).unwrap();
        // Токен в состоянии комплекта не хранится: он только в своём файле.
        assert!(ctx.kit.state_get("llama.token").is_none());
        assert_eq!(ctx.kit.state_get("llama.window").as_deref(), Some("250000"));
        assert_eq!(ctx.kit.state_get("llama.max_tokens").as_deref(), Some("300000"));
        assert_eq!(ctx.kit.state_get("llama.max_output").as_deref(), Some("50000"));
        if let Some(got) = facts_load(&ctx) {
            assert_eq!((got.window, got.max_tokens, got.max_output), (250_000, 300_000, 50_000));
        }
        let secret = dir_for_test.join("s.token");
        write_secret(&secret, "x").unwrap();
        assert!(write_secret(&secret, "y").is_err(), "существующий файл секрета не перевыписывается");
        let t1 = random_hex();
        let t2 = random_hex();
        assert_eq!(t1.len(), 32);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()) && t1 != t2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_container_form_keeps_its_secret_in_the_kit() {
        let kit = common::kit::Kit::at(std::env::temp_dir().join(format!("dev1c-llama-token-path-{}", std::process::id())));
        assert_eq!(token_path(&kit, Form::Docker), kit.path("dev1c-llama.token"));
        assert_ne!(token_path(&kit, Form::Host), token_path(&kit, Form::Docker));
        assert_eq!(env_path(&kit), kit.path("llama.env"));
    }

    #[test]
    fn the_secret_is_written_closed_to_its_owner() {
        let dir = std::env::temp_dir().join(format!("dev1c-llama-secret-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.token");
        write_secret(&f, "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "секрет не закрыт: {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nvidia_smi_csv_is_parsed_and_absence_is_none() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 3060 Ti, 8192 MiB, 1250 MiB, 610.88\n").unwrap();
        assert_eq!((g.name.as_str(), g.total_mb, g.used_mb, g.driver.as_str()), ("NVIDIA GeForce RTX 3060 Ti", 8192, 1250, "610.88"));
        assert!(parse_nvidia_smi("NVIDIA-SMI has failed").is_none());
    }
}
