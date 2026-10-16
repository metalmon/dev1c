//! Сервер модели: что ему передать, что он ответил, что из этого следует
//! для Zed. Здесь только чистые функции — их и проверяют тесты; обёртки над
//! командами и HTTP — ниже, в разделе «обёртки».

use serde::Deserialize;

pub const OUTPUT_TOKENS: u32 = crate::cfg::OUTPUT_TOKENS;
/// Запас на приблизительный счёт токенов у Zed.
pub const MARGIN: u32 = 2048;
/// Меньше этого на агента — не кодинг на 1С (модуль в 100 КБ ≈ 40 000 токенов).
pub const MIN_BUDGET: u32 = 16384;

pub struct ServeInput<'a> {
    pub model: &'a str,
    pub alias: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub fit_ctx: u32,
    pub fit_target: u32,
    pub cache_ram: u32,
    /// Токен путём к файлу (служба хоста) …
    pub token_file: Option<&'a str>,
    /// … или переменной LLAMA_API_KEY (контейнер): тогда в строке его нет.
    pub token_env: bool,
}

/// Командная строка по спеке volt-llama. Окно НЕ задаём: подбор уменьшит его
/// под карту, а заданное окно он уменьшить не может и выносит слои на
/// процессор — генерация падает с 32 до 4 токенов в секунду.
pub fn serve_args(i: &ServeInput) -> Vec<String> {
    let port = i.port.to_string();
    let fit_ctx = i.fit_ctx.to_string();
    let fit_target = i.fit_target.to_string();
    let cache_ram = i.cache_ram.to_string();
    let mut a: Vec<String> = [
        "-m", i.model, "--alias", i.alias, "--host", i.host, "--port", &port,
        "--parallel", "3", "--kv-unified", "--fit", "on", "--fit-ctx", &fit_ctx, "--fit-target", &fit_target,
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

/// Кэш подсказок: половина свободного ОЗУ, от 1024 до 8192 МиБ. Ноль ломает
/// слоты (простаивающему слоту некуда переехать), -1 — без предела, и кэш
/// выдавливает всё в подкачку.
pub fn cache_ram_mb(free_ram_mb: u64) -> u32 {
    ((free_ram_mb / 2) as u32).clamp(1024, 8192)
}

/// Карта с рабочим столом занята уже в покое: оставляем ей вдвое больше.
pub fn fit_target_mb(used_mb_idle: u64) -> u32 {
    if used_mb_idle > 300 {
        2048
    } else {
        1024
    }
}

/// Бюджет одного агента в Zed из окна сервера. Окно на агентов делится
/// только если так задано (agents > 1).
pub fn budget(window: u32, agents: u16) -> Option<u32> {
    let per = window / agents.max(1) as u32;
    let b = per.checked_sub(OUTPUT_TOKENS + MARGIN)?;
    (b >= MIN_BUDGET).then_some(b)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(token_file: Option<&'a str>) -> ServeInput<'a> {
        ServeInput { model: "/m/x.gguf", alias: "x", host: "127.0.0.1", port: 1235, fit_ctx: 65536, fit_target: 1024, cache_ram: 4096, token_file, token_env: false }
    }

    #[test]
    fn the_window_is_left_to_the_fit_and_the_token_goes_by_path() {
        let a = serve_args(&input(Some("/k/dev1c-llama.token")));
        assert!(!a.iter().any(|x| x == "-c" || x == "--ctx-size"), "окно не задаём: {a:?}");
        let at = |k: &str| a.iter().position(|x| x == k).map(|i| a[i + 1].clone());
        assert_eq!(at("--fit-ctx").as_deref(), Some("65536"));
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
    fn the_container_form_takes_the_token_from_the_environment() {
        let mut i = input(None);
        i.token_env = true;
        let a = serve_args(&i);
        assert!(!a.iter().any(|x| x == "--api-key-file"));
    }

    #[test]
    fn cache_ram_is_half_of_free_memory_within_bounds() {
        assert_eq!(cache_ram_mb(32000), 8192);
        assert_eq!(cache_ram_mb(6000), 3000);
        assert_eq!(cache_ram_mb(500), 1024);
    }

    #[test]
    fn a_desktop_on_the_card_doubles_the_fit_target() {
        assert_eq!(fit_target_mb(0), 1024);
        assert_eq!(fit_target_mb(200), 1024);
        assert_eq!(fit_target_mb(1250), 2048);
    }

    #[test]
    fn the_budget_subtracts_the_answer_and_the_margin_and_refuses_small_windows() {
        assert_eq!(budget(131072, 1), Some(131072 - 8192 - 2048));
        assert_eq!(budget(131072, 3), Some(131072 / 3 - 8192 - 2048));
        assert_eq!(budget(16384, 1), None, "меньше 16384 на агента — не бюджет");
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
    fn nvidia_smi_csv_is_parsed_and_absence_is_none() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 3060 Ti, 8192 MiB, 1250 MiB, 610.88\n").unwrap();
        assert_eq!((g.name.as_str(), g.total_mb, g.used_mb, g.driver.as_str()), ("NVIDIA GeForce RTX 3060 Ti", 8192, 1250, "610.88"));
        assert!(parse_nvidia_smi("NVIDIA-SMI has failed").is_none());
    }
}
