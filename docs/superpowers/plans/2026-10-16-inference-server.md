# Сервер инференса в dev1c-setup — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `dev1c-setup` ставит сервер модели в одной из трёх форм (служба хоста по умолчанию, контейнер рядом, удалённый сервер), измеряет его и синхронизирует числа окна с настройками Zed в контейнере среды.

**Architecture:** Новый модуль `llama.rs` (чистые функции: командная строка сервера, расчёт бюджета, разбор `/props` и `nvidia-smi`; обёртки: токен, служба через `nssm`/systemd, HTTP-замеры) и `ops_llama.rs` (операции `llama.bin`, `llama.service`, `llama.container`, `llama.answers`) встают в список операций **до** контейнера среды; результат замера хранится в `install.state` и уходит в контейнер переменными `CUSTOM_*`, которые entrypoint подставляет в `settings.json`. Окно и `check` показывают те же числа.

**Tech Stack:** Rust 2021, каркас `common` (`Op<Ctx>`, `OpState`, `docker::{exec,probe,run,ask}`, `Kit::state_get/state_set`), `reqwest` blocking (уже в зависимостях), `nssm` 2.24 и `llama.cpp` b11606 в комплекте, `docker`/Docker Desktop, sh в entrypoint образа.

**Spec:** `docs/superpowers/specs/2026-10-16-setup-binaries-design.md`, разделы «Локальная модель: по спеке volt-llama» и «Сервер инференса (этап 5)»; источник правил — `e:\zeroclaw\_local\spec-volt-llama.md`.

## Global Constraints

- Язык сообщений, комментариев и документации — русский; идентификаторы — английские (как во всём `setup/`).
- Окно модели не задаётся: в командной строке сервера нет `-c`/`--ctx-size`; есть `--fit-ctx <fit_ctx>` (умолчание 65536) и `--fit-target 1024` (2048, если карта занята рабочим столом).
- `--parallel 3 --kv-unified`; `--cache-ram` — меньшее из 8192 и половины свободного ОЗУ, не меньше 1024, никогда `-1` и `0`.
- Токен уходит в сервер путём к файлу (`--api-key-file`), не значением; файл с закрытыми правами.
- `nssm` берётся только из комплекта (`llama/nssm.exe`); чужой на машине не используется.
- Установщик измеряет и говорит; системные настройки хоста (подкачка, драйвер) не трогает.
- Служба хоста слушает `127.0.0.1` на Windows и адрес docker-моста на Linux; контейнер среды ходит по `http://host.docker.internal:<порт>/v1` в обеих локальных формах.
- Операции модели идут до операции `container`; числа Zed: `G = 8192`, `max_tokens = W / agents − G − 2048`, не меньше 16384.
- Числа в прозе не нужны пользователю — но в сообщениях установщика числа обязательны («окно 131072, слотов 3»).
- Сборки без видеокарты (CI) обязаны проходить с `serve = "remote"`; с `serve = "host"` без карты шаг — `Blocked`, не ошибка.
- Коммит на задачу; `cargo test -p dev1c-setup` зелёный после каждой; CI собирает Windows с окном, Linux — `--no-default-features`.

## Review Focus

1. **`dev1c.toml` старого образца** (без `serve`, с `file` и пустым `url`) — должен читаться как `serve = "host"` c файлом относительно комплекта? Нет: старый смысл `file` был «файл в комплекте → том». Решение: `serve` обязателен по умолчанию `"host"`, а `file` при `host` — полный путь; относительный путь при `host` — ошибка `validate` с подсказкой «для файла в комплекте выберите serve = "docker"». Тест в Задаче 1.
2. **Порт службы занят** (другим `llama-server`, в т.ч. `volt-llama` на 1235) — `llama.service` должен быть `Blocked` с именем порта, а не падать на регистрации. Тест чистой функции `port_busy` + состояние в Задаче 4.
3. **Сервер не поместился**: `/props` отдал окно меньше `fit_ctx` — `llama.answers` обязан провалить установку с двумя числами, а не записать маленькое окно в Zed. Тест `budget`/`answers_verdict` в Задаче 2.
4. **Контейнер среды уже работает, а числа изменились** (переустановили модель) — `container` обязан показать `Drifted` с переменной в тексте (`CUSTOM_MAX_TOKENS=…`), пересоздание рвёт сессии RDP — об этом говорит план операции. Тест `drift_of` с новыми переменными в Задаче 4.
5. **Нет видеокарты, но `serve = "host"`** — `llama.service` `Blocked` «нет видеокарты…», `container` при этом не должен ссылаться на несуществующий сервер: `container_env` без фактов замера не включает `CUSTOM_API_URL` локальной формы, а `check` говорит «сервер модели: не поднят». Тест `container_env` без фактов в Задаче 1.

---

### Task 1: Параметры трёх форм (`cfg.rs`, `dev1c.example.toml`)

**Files:**
- Modify: `setup/dev1c-setup/src/cfg.rs`
- Modify: `setup/dev1c-setup/dev1c.example.toml`
- Modify: `setup/dev1c-setup/src/main.rs` (функция `apply_changes`: новые ключи `model.serve`, `model.port`, `model.agents`, `model.fit_ctx`)

**Interfaces:**
- Produces: `Model { serve: String, file, url, key, name, port: u16, agents: u16, fit_ctx: u32 }`; `Cfg::form(&self) -> Form` где `pub enum Form { Remote, Host, Docker }`; `Cfg::model_url(&self) -> String` (адрес для контейнера среды); `pub struct Facts { pub token: String, pub window: u32, pub max_tokens: u32 }` и `Cfg::container_env(&self, facts: Option<&Facts>) -> Vec<(String, String)>`; `Cfg::model_alias(&self) -> String` (имя файла без расширения или `name`).

- [ ] **Step 1: Тесты параметров (падают)**

В `cfg.rs`, модуль `tests`, заменить `local_model_gives_no_variables` и добавить:

```rust
    #[test]
    fn default_form_is_host_and_needs_an_absolute_path() {
        let mut c = Cfg::default();
        assert_eq!(c.form(), Form::Host);
        c.model.file = "model/x.gguf".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("serve = \"docker\""), "{e}");
        c.model.file = if cfg!(windows) { r"C:\models\x.gguf".into() } else { "/models/x.gguf".into() };
        c.validate().unwrap();
        assert_eq!(c.model_alias(), "x");
        assert_eq!(c.model_url(), "http://host.docker.internal:1235/v1");
    }

    #[test]
    fn docker_form_takes_the_file_from_the_kit_and_the_third_port() {
        let mut c = Cfg::default();
        c.model.serve = "docker".into();
        c.model.file = "model/x.gguf".into();
        c.validate().unwrap();
        assert_eq!(c.form(), Form::Docker);
        assert_eq!(c.model_url(), "http://host.docker.internal:3392/v1");
    }

    #[test]
    fn remote_form_is_chosen_by_serve_not_guessed() {
        let mut c = Cfg::default();
        c.model.url = "https://m".into();
        c.model.name = "Q".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("serve"), "{e}");
        c.model.serve = "remote".into();
        c.validate().unwrap();
        assert_eq!(c.model_url(), "https://m");
    }

    #[test]
    fn local_forms_send_the_measured_numbers_and_nothing_before_them() {
        let mut c = Cfg::default();
        c.model.file = "/models/x.gguf".into();
        // До замера фактов нет: контейнеру нечего сказать про модель.
        let env = c.container_env(None);
        assert!(env.iter().all(|(k, _)| k != "CUSTOM_API_URL"));
        assert!(env.contains(&("LLAMA_ENABLED".into(), "0".into())));
        let f = Facts { token: "t".into(), window: 131072, max_tokens: 120832 };
        let env = c.container_env(Some(&f));
        assert!(env.contains(&("CUSTOM_API_URL".into(), "http://host.docker.internal:1235/v1".into())));
        assert!(env.contains(&("CUSTOM_API_KEY".into(), "t".into())));
        assert!(env.contains(&("CUSTOM_MODEL".into(), "x".into())));
        assert!(env.contains(&("CUSTOM_MAX_TOKENS".into(), "120832".into())));
        assert!(env.contains(&("CUSTOM_MAX_OUTPUT_TOKENS".into(), "8192".into())));
    }

    #[test]
    fn agents_and_fit_ctx_have_sane_bounds() {
        let mut c = Cfg::default();
        c.model.agents = 0;
        assert!(c.validate().unwrap_err().to_string().contains("agents"));
        c.model.agents = 1;
        c.model.fit_ctx = 1000;
        assert!(c.validate().unwrap_err().to_string().contains("fit_ctx"));
    }
```

Существующие тесты `remote_model_gives_custom_variables_and_no_llama`, `url_without_key_is_allowed_and_sends_no_key`, `url_without_name_is_refused`, `both_file_and_url_is_refused_with_the_field_named` дополнить строкой `c.model.serve = "remote".into();` там, где задан `url`; `empty_model_is_allowed` оставить (пустой `file` при `host` — допустимо, шаг будет `Blocked`); в `change_keeps_the_comments` добавить `c.model.serve = "remote".into();`.

- [ ] **Step 2: Запустить — должны упасть**

Run: `cargo test -p dev1c-setup --manifest-path setup/Cargo.toml cfg::` — ожидание: ошибки компиляции (`serve`, `Form`, `Facts` не определены).

- [ ] **Step 3: Реализация**

В `cfg.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Model {
    /// "host" — служба этой машины, "docker" — контейнер рядом, "remote" — чужой сервер.
    pub serve: String,
    pub file: String,
    pub url: String,
    pub key: String,
    pub name: String,
    /// Порт службы хоста (на петле). Для docker наружу идёт третий порт среды.
    pub port: u16,
    /// На сколько агентов делить окно. 1 — не делить (см. спеку: окно на три не делим).
    pub agents: u16,
    /// Нижняя граница окна для --fit-ctx; меньше — установка проваливается.
    pub fit_ctx: u32,
}

impl Default for Model {
    fn default() -> Self {
        Self { serve: "host".into(), file: String::new(), url: String::new(), key: String::new(), name: String::new(), port: 1235, agents: 1, fit_ctx: 65536 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Form {
    Remote,
    Host,
    Docker,
}

/// Что установщик измерил у поднятого сервера; без этого контейнеру среды
/// нечего сказать про локальную модель.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub token: String,
    pub window: u32,
    pub max_tokens: u32,
}

pub const OUTPUT_TOKENS: u32 = 8192;
```

Методы `Cfg`:

```rust
    pub fn form(&self) -> Form {
        match self.model.serve.as_str() {
            "remote" => Form::Remote,
            "docker" => Form::Docker,
            _ => Form::Host,
        }
    }

    /// Имя модели для Zed: name, а без него — имя файла без расширения.
    pub fn model_alias(&self) -> String {
        if !self.model.name.is_empty() {
            return self.model.name.clone();
        }
        std::path::Path::new(&self.model.file).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    }

    /// Адрес сервера модели, как его видит контейнер среды.
    pub fn model_url(&self) -> String {
        match self.form() {
            Form::Remote => self.model.url.clone(),
            Form::Host => format!("http://host.docker.internal:{}/v1", self.model.port),
            Form::Docker => format!("http://host.docker.internal:{}/v1", self.ports()[2]),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let m = &self.model;
        anyhow::ensure!(matches!(m.serve.as_str(), "host" | "docker" | "remote"), "[model]: serve — host, docker или remote");
        anyhow::ensure!(m.agents >= 1 && m.agents <= 3, "[model]: agents — от 1 до 3");
        anyhow::ensure!(m.fit_ctx >= 16384, "[model]: fit_ctx — не меньше 16384");
        match self.form() {
            Form::Remote => {
                anyhow::ensure!(!m.url.is_empty(), "[model]: для serve = \"remote\" нужен url");
                anyhow::ensure!(!m.name.is_empty(), "[model]: для удалённого сервера нужно name — имя модели, как сервер его называет");
            }
            Form::Host => {
                anyhow::ensure!(m.url.is_empty(), "[model]: url задан, а serve не \"remote\" — выберите одно: serve = \"remote\" или пустой url");
                anyhow::ensure!(
                    m.file.is_empty() || std::path::Path::new(&m.file).is_absolute(),
                    "[model]: при serve = \"host\" file — полный путь к .gguf на этой машине; для файла в комплекте выберите serve = \"docker\""
                );
            }
            Form::Docker => {
                anyhow::ensure!(m.url.is_empty(), "[model]: url задан, а serve не \"remote\"");
                anyhow::ensure!(
                    m.file.is_empty() || !std::path::Path::new(&m.file).is_absolute(),
                    "[model]: при serve = \"docker\" file — путь относительно комплекта"
                );
            }
        }
        anyhow::ensure!(self.server.port >= 1024 && self.server.port <= 65532, "[server]: port должен быть от 1024 до 65532 (нужны четыре порта подряд)");
        anyhow::ensure!(matches!(self.server.engine.as_str(), "docker" | "podman"), "[server]: engine — docker или podman");
        Ok(())
    }

    pub fn container_env(&self, facts: Option<&Facts>) -> Vec<(String, String)> {
        let m = &self.model;
        let mut env = vec![("LLAMA_ENABLED".to_string(), "0".to_string())];
        match self.form() {
            Form::Remote => {
                env.push(("CUSTOM_API_URL".into(), m.url.clone()));
                if !m.key.is_empty() {
                    env.push(("CUSTOM_API_KEY".into(), m.key.clone()));
                }
                env.push(("CUSTOM_MODEL".into(), m.name.clone()));
            }
            Form::Host | Form::Docker => {
                let Some(f) = facts else { return env };
                env.push(("CUSTOM_API_URL".into(), self.model_url()));
                env.push(("CUSTOM_API_KEY".into(), f.token.clone()));
                env.push(("CUSTOM_MODEL".into(), self.model_alias()));
                env.push(("CUSTOM_MAX_TOKENS".into(), f.max_tokens.to_string()));
                env.push(("CUSTOM_MAX_OUTPUT_TOKENS".into(), OUTPUT_TOKENS.to_string()));
            }
        }
        env
    }
```

(Прежние `ensure` про `port`/`engine` сохранить дословно из текущего `validate`.) В `render` добавить ветки `("[model]", "serve")`, `("[model]", "port")`, `("[model]", "agents")`, `("[model]", "fit_ctx")`. В `dev1c.example.toml` секция `[model]`:

```toml
[model]
# Как подаётся модель для агентов:
#   "host"   — llama-server службой на этой машине (Windows: nssm из комплекта,
#              Linux: systemd); модель — файл на диске этой машины. Умолчание.
#   "docker" — контейнер рядом со средой из образа комплекта; модель — файл
#              в комплекте, копируется в том <имя>-models.
#   "remote" — чужой OpenAI-совместимый сервер: url, key, name ниже.
serve = "host"
# host: полный путь к .gguf на этой машине, например C:\models\Qwen3.5-4B.Q4_K_M.gguf
# docker: путь к файлу относительно комплекта. Пусто — модель ещё не выбрана.
file = ""
# remote: адрес с путём, но без завершающего /chat/completions — его добавляет редактор.
url = ""
# remote: ключ доступа. Может быть пустым.
key = ""
# Имя модели: remote — ровно так, как сервер его называет; host/docker — любое,
# пусто — имя файла без расширения.
name = ""
# host: порт службы на этой машине (слушает только петлю).
port = 1235
# На сколько агентов делить окно. 1 — не делить: простаивающий слот отдаёт
# окно занятому, деление — только страховка от вытеснения.
agents = 1
# Нижняя граница окна в токенах: сервер подбирает окно под карту сам, а если
# выйдет меньше этого — установка проваливается с числом.
fit_ctx = 65536
```

В `main.rs::apply_changes` добавить ветки:

```rust
            "model.serve" => ctx.cfg.model.serve = val.into(),
            "model.port" => ctx.cfg.model.port = val.parse().context("model.port — число")?,
            "model.agents" => ctx.cfg.model.agents = val.parse().context("model.agents — число")?,
            "model.fit_ctx" => ctx.cfg.model.fit_ctx = val.parse().context("model.fit_ctx — число")?,
```

и в текст ошибки «неизвестный параметр» — новые имена. Все вызовы `container_env()` в `ops.rs` временно → `container_env(None)` (Задача 4 заменит на факты); в `ops.rs::drift_of` сигнатура получает `facts: Option<&Facts>` и передаёт его дальше; тесты `run_args_*` в `ops.rs` дополнить `c.model.serve = "remote".into();`.

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p dev1c-setup --manifest-path setup/Cargo.toml` — ожидание: все `cfg::` и `ops::` тесты проходят; `example_parses_and_equals_defaults` держит образец и структуру в согласии.

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup
git commit -m "dev1c-setup: [model] serve — три формы модели, порт, agents, fit_ctx; факты замера в среде контейнера"
```

---

### Task 2: Чистые функции сервера (`llama.rs`): командная строка, бюджет, разбор ответов

**Files:**
- Create: `setup/dev1c-setup/src/llama.rs`
- Modify: `setup/dev1c-setup/src/main.rs` (`mod llama;`)

**Interfaces:**
- Produces: `pub struct ServeInput<'a> { pub model: &'a str, pub alias: &'a str, pub host: &'a str, pub port: u16, pub fit_ctx: u32, pub fit_target: u32, pub cache_ram: u32, pub token_file: Option<&'a str>, pub token_env: bool }`; `pub fn serve_args(i: &ServeInput) -> Vec<String>`; `pub fn cache_ram_mb(free_ram_mb: u64) -> u32`; `pub fn fit_target_mb(used_mb_idle: u64) -> u32`; `pub fn budget(window: u32, agents: u16) -> Option<u32>`; `pub struct Props { pub window: u32, pub slots: u16, pub model: String }` и `pub fn parse_props(json: &str) -> Option<Props>`; `pub struct Gpu { pub name: String, pub total_mb: u64, pub used_mb: u64, pub driver: String }` и `pub fn parse_nvidia_smi(csv: &str) -> Option<Gpu>`; `pub fn prefill_from(json: &str) -> Option<f64>`.

- [ ] **Step 1: Тесты (падают)**

```rust
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
```

- [ ] **Step 2: Запустить — падают**

Run: `cargo test -p dev1c-setup --manifest-path setup/Cargo.toml llama::` — ожидание: модуль не найден.

- [ ] **Step 3: Реализация**

```rust
//! Сервер модели: что ему передать, что он ответил, что из этого следует
//! для Zed. Здесь только чистые функции — их и проверяют тесты; обёртки над
//! командами и HTTP — в ops_llama.rs.

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
    let mut a: Vec<String> = [
        "-m", i.model, "--alias", i.alias, "--host", i.host, "--port", &i.port.to_string(),
        "--parallel", "3", "--kv-unified", "--fit", "on", "--fit-ctx", &i.fit_ctx.to_string(), "--fit-target", &i.fit_target.to_string(),
        "--cache-ram", &i.cache_ram.to_string(), "--ctx-checkpoints", "32",
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
    let _ = i.token_env;
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
    if used_mb_idle > 300 { 2048 } else { 1024 }
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
    let model = p.model_path.unwrap_or_default();
    let model = std::path::Path::new(&model).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(model);
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
```

Добавить `mod llama;` в `main.rs`.

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p dev1c-setup --manifest-path setup/Cargo.toml llama::` — все проходят.

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup/src/llama.rs setup/dev1c-setup/src/main.rs
git commit -m "dev1c-setup: llama.rs — командная строка сервера, бюджет Zed, разбор /props и nvidia-smi (чистые функции)"
```

---

### Task 3: Обёртки хоста: токен, карта, ОЗУ, служба (`nssm`/systemd), HTTP-замеры

**Files:**
- Modify: `setup/dev1c-setup/src/llama.rs` (добавить раздел «обёртки»)

**Interfaces:**
- Consumes: `common::docker::{exec, probe, Out}`, `common::kit::write_text`, `crate::ops::Ctx`, `reqwest::blocking`.
- Produces: `pub const SERVICE: &str = "dev1c-llama"`; `pub fn dir() -> PathBuf` (`C:\dev1c\llama` / `/opt/dev1c/llama`); `pub fn server_exe() -> PathBuf`; `pub fn token_file() -> PathBuf`; `pub fn log_file() -> PathBuf`; `pub fn ensure_token(ctx: &Ctx) -> Result<String>`; `pub fn gpu(ctx: &Ctx) -> Option<Gpu>`; `pub fn free_ram_mb(ctx: &Ctx) -> u64`; `pub fn bind_host(ctx: &Ctx) -> String`; `pub fn port_busy(port: u16) -> bool`; `pub fn kit_bin(ctx: &Ctx) -> Option<PathBuf>` (`llama/bin/windows-x64-cuda13` или `linux-x64-cuda13`); `pub fn nssm(ctx: &Ctx) -> Option<PathBuf>`; `pub struct Installed { pub present: bool, pub running: bool }` и `pub fn installed(ctx: &Ctx) -> Installed`; `pub fn service_install(ctx: &Ctx, args: &[String]) -> Result<()>`; `pub fn service_start/service_stop/service_remove(ctx: &Ctx) -> Result<()>`; `pub fn wait_health(url: &str, token: &str, secs: u64) -> Result<()>`; `pub fn props(url: &str, token: &str) -> Option<Props>`; `pub fn measure_prefill(url: &str, token: &str) -> Option<f64>`; `pub fn facts_save(ctx: &Ctx, f: &crate::cfg::Facts, prefill: Option<f64>) -> Result<()>` и `pub fn facts_load(ctx: &Ctx) -> Option<crate::cfg::Facts>` (ключи `install.state`: `llama.token`, `llama.window`, `llama.max_tokens`, `llama.prefill`).

- [ ] **Step 1: Тест (падает) — факты в состоянии комплекта, токен из системной случайности**

```rust
    #[test]
    fn facts_round_trip_through_the_kit_state_and_the_token_is_random_hex() {
        let dir = std::env::temp_dir().join(format!("dev1c-llama-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kit = common::kit::Kit::at(&dir);
        let ctx = crate::ops::Ctx { kit, cfg: crate::cfg::Cfg::default(), dry_run: false, sink: std::sync::Arc::new(|_| {}), prompt: Box::new(common::AlwaysDefault) };
        assert!(facts_load(&ctx).is_none());
        let f = crate::cfg::Facts { token: "abc".into(), window: 131072, max_tokens: 120832 };
        facts_save(&ctx, &f, Some(2600.0)).unwrap();
        assert_eq!(facts_load(&ctx), Some(f));
        let t1 = random_hex();
        let t2 = random_hex();
        assert_eq!(t1.len(), 32);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()) && t1 != t2);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Запустить — падает** (`facts_load` не определена).

- [ ] **Step 3: Реализация обёрток**

```rust
// ------------------------------------------------------------ обёртки

use crate::ops::Ctx;
use anyhow::{Context, Result};
use common::docker;
use common::Runner;
use std::path::PathBuf;

pub const SERVICE: &str = "dev1c-llama";

pub fn dir() -> PathBuf {
    if cfg!(windows) { PathBuf::from(r"C:\dev1c\llama") } else { PathBuf::from("/opt/dev1c/llama") }
}
pub fn server_exe() -> PathBuf {
    dir().join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" })
}
pub fn token_file() -> PathBuf {
    dir().join("dev1c-llama.token")
}
pub fn log_file() -> PathBuf {
    dir().join("llama.log")
}
pub fn kit_bin(ctx: &Ctx) -> Option<PathBuf> {
    let p = ctx.kit.path(if cfg!(windows) { "llama/bin/windows-x64-cuda13" } else { "llama/bin/linux-x64-cuda13" });
    p.join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" }).exists().then_some(p)
}
/// Только из комплекта или уже скопированный нами; чужой на машине не берём.
pub fn nssm(ctx: &Ctx) -> Option<PathBuf> {
    let mine = dir().join("nssm.exe");
    if mine.exists() {
        return Some(mine);
    }
    let kit = ctx.kit.path("llama/nssm.exe");
    kit.exists().then_some(kit)
}

/// 128 бит системной случайности без лишней зависимости: RandomState сеется
/// из ОС при каждом создании.
pub fn random_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|_| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u8(0);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Токен службы: файл с закрытыми правами, в команду попадает путь.
pub fn ensure_token(ctx: &Ctx) -> Result<String> {
    let f = token_file();
    if let Ok(t) = std::fs::read_to_string(&f) {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    std::fs::create_dir_all(dir())?;
    let token = random_hex();
    common::kit::write_text(&f, &token)?;
    let path = f.to_string_lossy().to_string();
    if cfg!(windows) {
        let user = std::env::var("USERNAME").unwrap_or_default();
        anyhow::ensure!(!user.is_empty(), "не узнать имя пользователя: права на токен не закрыть");
        let grant = format!("{user}:(R,W)");
        docker::exec(ctx, "icacls", &[&path, "/inheritance:r", "/grant:r", &grant, "*S-1-5-18:(R)"])?;
    } else {
        docker::exec(ctx, "chmod", &["600", &path])?;
    }
    Ok(token)
}

pub fn gpu(ctx: &Ctx) -> Option<Gpu> {
    let o = docker::probe(ctx, "nvidia-smi", &["--query-gpu=name,memory.total,memory.used,driver_version", "--format=csv,noheader"]).ok()?;
    o.ok().then(|| parse_nvidia_smi(&o.text)).flatten()
}

pub fn free_ram_mb(ctx: &Ctx) -> u64 {
    if cfg!(windows) {
        docker::probe(ctx, "powershell", &["-NoProfile", "-Command", "(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory"])
            .ok()
            .and_then(|o| o.text.trim().lines().next()?.trim().parse::<u64>().ok())
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    } else {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|t| t.lines().find(|l| l.starts_with("MemAvailable:"))?.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    }
}

/// Где слушать: на Windows петля (Docker Desktop доставляет host.docker.internal
/// до неё — замерено), на Linux адрес моста — виден контейнерам и машине, не сети.
pub fn bind_host(ctx: &Ctx) -> String {
    if cfg!(windows) {
        return "127.0.0.1".into();
    }
    docker::probe(ctx, &ctx.engine(), &["network", "inspect", "bridge", "-f", "{{(index .IPAM.Config 0).Gateway}}"])
        .ok()
        .map(|o| o.text.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "172.17.0.1".into())
}

pub fn port_busy(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

pub struct Installed {
    pub present: bool,
    pub running: bool,
}

pub fn installed(ctx: &Ctx) -> Installed {
    if cfg!(windows) {
        let o = docker::probe(ctx, "sc", &["query", SERVICE]);
        match o {
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
        let nssm = nssm(ctx).context("нет nssm: его кладут в комплект как llama/nssm.exe")?.to_string_lossy().to_string();
        let mut cmd: Vec<&str> = vec!["install", SERVICE, &exe];
        cmd.extend(args.iter().map(|a| a.as_str()));
        let o = docker::exec(ctx, &nssm, &cmd)?;
        anyhow::ensure!(o.ok(), "служба не зарегистрирована: {}", o.tail(3).join(" "));
        let d = dir().to_string_lossy().to_string();
        let log = log_file().to_string_lossy().to_string();
        for pair in [
            vec!["set", SERVICE, "AppDirectory", &d],
            vec!["set", SERVICE, "AppStdout", &log],
            vec!["set", SERVICE, "AppStderr", &log],
            vec!["set", SERVICE, "AppRotateFiles", "1"],
            vec!["set", SERVICE, "AppRotateBytes", "10485760"],
            vec!["set", SERVICE, "AppExit", "Default", "Restart"],
            vec!["set", SERVICE, "Start", "SERVICE_AUTO_START"],
            vec!["set", SERVICE, "DisplayName", "dev1c — сервер модели"],
        ] {
            let o = docker::exec(ctx, &nssm, &pair)?;
            anyhow::ensure!(o.ok(), "не настроить службу: {}", o.tail(2).join(" "));
        }
    } else {
        let line = args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ");
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
        let nssm = nssm(ctx).context("нет nssm")?.to_string_lossy().to_string();
        docker::exec(ctx, &nssm, &[verb, SERVICE])
    } else {
        docker::exec(ctx, "systemctl", &[verb, SERVICE])
    }
}
pub fn service_start(ctx: &Ctx) -> Result<()> {
    let o = service_cmd(ctx, "start")?;
    anyhow::ensure!(o.ok(), "служба не запустилась: {}", o.tail(3).join(" "));
    Ok(())
}
pub fn service_stop(ctx: &Ctx) -> Result<()> {
    let o = service_cmd(ctx, "stop")?;
    anyhow::ensure!(o.ok(), "служба не остановилась: {}", o.tail(3).join(" "));
    Ok(())
}
pub fn service_remove(ctx: &Ctx) -> Result<()> {
    let _ = service_cmd(ctx, "stop");
    if cfg!(windows) {
        if let Some(n) = nssm(ctx) {
            let _ = docker::exec(ctx, &n.to_string_lossy(), &["remove", SERVICE, "confirm"]);
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
    let mut last = String::from("нет ответа");
    loop {
        match c.get(format!("{url}/health")).bearer_auth(token).send() {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => last = format!("ответ {}", r.status().as_u16()),
            Err(e) => last = e.to_string(),
        }
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
    let r = client(600).post(format!("{url}/v1/chat/completions")).bearer_auth(token).json(&body).send().ok()?;
    prefill_from(&r.text().ok()?)
}

pub fn facts_save(ctx: &Ctx, f: &crate::cfg::Facts, prefill: Option<f64>) -> Result<()> {
    ctx.kit.state_set("llama.token", &f.token)?;
    ctx.kit.state_set("llama.window", &f.window.to_string())?;
    ctx.kit.state_set("llama.max_tokens", &f.max_tokens.to_string())?;
    ctx.kit.state_set("llama.prefill", &prefill.map(|p| format!("{p:.0}")).unwrap_or_default())
}

pub fn facts_load(ctx: &Ctx) -> Option<crate::cfg::Facts> {
    Some(crate::cfg::Facts {
        token: ctx.kit.state_get("llama.token")?,
        window: ctx.kit.state_get("llama.window")?.parse().ok()?,
        max_tokens: ctx.kit.state_get("llama.max_tokens")?.parse().ok()?,
    })
}
```

`reqwest` в `dev1c-setup/Cargo.toml` получает feature `json` (для `.json(&body)`); `Facts` — `#[derive(Debug, Clone, PartialEq)]` уже в Задаче 1.

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p dev1c-setup --manifest-path setup/Cargo.toml llama::`

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup
git commit -m "dev1c-setup: обёртки сервера модели — токен, карта, ОЗУ, служба nssm/systemd, /health, /props, замер префила"
```

---

### Task 4: Операции хостовой формы и связка с контейнером (`ops_llama.rs`, `ops.rs`)

**Files:**
- Create: `setup/dev1c-setup/src/ops_llama.rs`
- Modify: `setup/dev1c-setup/src/ops.rs` (`server()`, `Model`, `Container`, `run_args`, `drift_of`, `Services`)
- Modify: `setup/dev1c-setup/src/main.rs` (`mod ops_llama;`)

**Interfaces:**
- Consumes: Задачи 1–3.
- Produces: `ops_llama::ops(form: Form) -> Vec<Box<dyn Op<Ctx>>>` — `[Bin, Service, Answers]` для `Host`, `[Container, Answers]` для `Docker` (контейнер — Задача 7, пока пусто), `[]` для `Remote`; `ops_llama::desired_args(ctx: &Ctx) -> Vec<String>`; `ops::run_args(cfg: &Cfg, facts: Option<&Facts>)`; `ops::drift_of(cfg, image, first_port, env, facts)`.

- [ ] **Step 1: Тесты (падают)**

В `ops_llama.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{Cfg, Form};

    #[test]
    fn the_form_chooses_the_operations_and_their_order() {
        let ids = |f: Form| ops(f).iter().map(|o| o.id().to_string()).collect::<Vec<_>>();
        assert_eq!(ids(Form::Host), ["llama.bin", "llama.service", "llama.answers"]);
        assert_eq!(ids(Form::Remote), Vec::<String>::new());
    }

    #[test]
    fn baked_arguments_are_compared_as_one_line() {
        // Служба запекает параметры: разошлись — пересоздание.
        assert!(args_drift(&["a".into(), "b".into()], "a b").is_none());
        let (from, to) = args_drift(&["a".into(), "c".into()], "a b").unwrap();
        assert!(from.contains("a b") && to.contains("a c"));
    }

    #[test]
    fn the_verdict_of_answers_needs_the_window_and_the_budget() {
        let mut c = Cfg::default();
        c.model.fit_ctx = 65536;
        assert!(verdict(&c, 32768).is_err(), "окно меньше fit_ctx — провал с числами");
        let e = verdict(&c, 32768).unwrap_err().to_string();
        assert!(e.contains("32768") && e.contains("65536"), "{e}");
        assert_eq!(verdict(&c, 131072).unwrap(), 131072 - 8192 - 2048);
    }
}
```

В `ops.rs` тесты: `run_args_carry_ports_volumes_and_environment` — `run_args(&c, None)`; добавить:

```rust
    #[test]
    fn a_change_of_the_measured_numbers_is_a_drift_of_the_container() {
        let mut c = Cfg::default();
        c.model.file = "/m/x.gguf".into();
        let f = Facts { token: "t".into(), window: 131072, max_tokens: 120832 };
        let env: Vec<String> = c.container_env(Some(&f)).into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert!(drift_of(&c, &c.image(), "3390", &env, Some(&f)).is_none());
        let f2 = Facts { max_tokens: 100000, ..f.clone() };
        let (from, to) = drift_of(&c, &c.image(), "3390", &env, Some(&f2)).unwrap();
        assert!(from.contains("CUSTOM_MAX_TOKENS=120832") && to.contains("CUSTOM_MAX_TOKENS=100000"));
    }

    #[test]
    fn docker_engine_gets_the_host_alias_for_the_model_server() {
        let c = Cfg::default();
        assert!(run_args(&c, None).join(" ").contains("--add-host host.docker.internal:host-gateway"));
    }
```

- [ ] **Step 2: Запустить — падают.**

- [ ] **Step 3: Реализация `ops_llama.rs`**

```rust
//! Операции сервера модели на этой машине (serve = "host"): файлы из
//! комплекта, служба, замер. Каждая умеет объясниться: без карты, без
//! модели, без nssm человек узнаёт причину, а не «шагов: 0».

use crate::cfg::{Facts, Form};
use crate::llama;
use crate::ops::Ctx;
use anyhow::Result;
use common::docker;
use common::op::{Action, CleanLevel, Op, OpState};
use common::Runner;

pub fn ops(form: Form) -> Vec<Box<dyn Op<Ctx>>> {
    match form {
        Form::Host => vec![Box::new(Bin), Box::new(Service), Box::new(Answers)],
        Form::Docker => vec![Box::new(Answers)], // контейнер — следующей задачей
        Form::Remote => vec![],
    }
}

/// Адрес, по которому сервер спрашивает сам установщик (с этой машины).
pub fn local_url(ctx: &Ctx) -> String {
    match ctx.cfg.form() {
        Form::Host => format!("http://{}:{}", llama::bind_host(ctx), ctx.cfg.model.port),
        _ => format!("http://127.0.0.1:{}", ctx.cfg.ports()[2]),
    }
}

/// Командная строка, какой она должна быть сейчас (служба хоста).
pub fn desired_args(ctx: &Ctx) -> Vec<String> {
    let used = llama::gpu(ctx).map(|g| g.used_mb).unwrap_or(0);
    let host = llama::bind_host(ctx);
    let token = llama::token_file().to_string_lossy().to_string();
    let alias = ctx.cfg.model_alias();
    llama::serve_args(&llama::ServeInput {
        model: &ctx.cfg.model.file,
        alias: &alias,
        host: &host,
        port: ctx.cfg.model.port,
        fit_ctx: ctx.cfg.model.fit_ctx,
        fit_target: llama::fit_target_mb(used),
        cache_ram: llama::cache_ram_mb(llama::free_ram_mb(ctx)),
        token_file: Some(&token),
        token_env: false,
    })
}

pub fn args_drift(want: &[String], baked: &str) -> Option<(String, String)> {
    let w = want.join(" ");
    (w != baked).then(|| (format!("служба: {baked}"), format!("служба: {w}")))
}

/// Что из окна сервера следует для Zed; меньше нижней границы — провал с числами.
pub fn verdict(cfg: &crate::cfg::Cfg, window: u32) -> Result<u32> {
    anyhow::ensure!(
        window >= cfg.model.fit_ctx,
        "сервер выделил окно {window} токенов при нижней границе fit_ctx = {}: на этой карте модель с такими параметрами не помещается",
        cfg.model.fit_ctx
    );
    llama::budget(window, cfg.model.agents).ok_or_else(|| {
        anyhow::anyhow!("окно {window} на {} агентов даёт меньше {} токенов на агента — для кодинга на 1С мало", cfg.model.agents, llama::MIN_BUDGET)
    })
}

struct Bin;
impl Op<Ctx> for Bin {
    fn id(&self) -> &'static str { "llama.bin" }
    fn title(&self) -> &'static str { "Сервер модели (файлы)" }
    fn state(&self, ctx: &Ctx) -> OpState {
        let Some(src) = llama::kit_bin(ctx) else {
            return OpState::Blocked("в комплекте нет llama/bin: pack.sh кладёт его рядом с образом".into());
        };
        if !llama::server_exe().exists() || (cfg!(windows) && !llama::dir().join("nssm.exe").exists()) {
            return OpState::Missing;
        }
        let ver = |p: &std::path::Path| docker::probe(ctx, &p.to_string_lossy(), &["--version"]).map(|o| o.text.trim().to_string()).unwrap_or_default();
        let have = ver(&llama::server_exe());
        let want = ver(&src.join(llama::server_exe().file_name().unwrap()));
        if !want.is_empty() && have != want {
            return OpState::Drifted { from: have, to: want };
        }
        OpState::Done(llama::dir().display().to_string())
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!("скопировать llama-server и библиотеки CUDA из комплекта в {}", llama::dir().display()))]
            .into_iter()
            .chain(cfg!(windows).then(|| Action::new("положить рядом nssm.exe из комплекта")))
            .chain(ctx.cfg.model.file.is_empty().then(|| Action::new("(модель ещё не выбрана)")))
            .collect()
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let src = llama::kit_bin(ctx).ok_or_else(|| anyhow::anyhow!("в комплекте нет llama/bin"))?;
        std::fs::create_dir_all(llama::dir())?;
        for e in std::fs::read_dir(&src)? {
            let e = e?;
            if e.file_type()?.is_file() {
                std::fs::copy(e.path(), llama::dir().join(e.file_name()))?;
            }
        }
        if cfg!(windows) {
            let n = ctx.kit.path("llama/nssm.exe");
            anyhow::ensure!(n.exists(), "в комплекте нет llama/nssm.exe");
            std::fs::copy(&n, llama::dir().join("nssm.exe"))?;
        }
        ctx.ok(format!("файлы сервера в {}", llama::dir().display()));
        Ok(())
    }
    fn revert(&self, _ctx: &Ctx, level: CleanLevel) -> Option<Vec<Action>> {
        (level == CleanLevel::Data).then(|| vec![Action::new(format!("удалить {}", llama::dir().display()))])
    }
    fn revert_apply(&self, ctx: &Ctx, level: CleanLevel) -> Result<()> {
        if level == CleanLevel::Data {
            let _ = std::fs::remove_dir_all(llama::dir());
            ctx.ok("файлы сервера удалены");
        }
        Ok(())
    }
    fn clean_level(&self) -> CleanLevel { CleanLevel::Data }
}

struct Service;
impl Op<Ctx> for Service {
    fn id(&self) -> &'static str { "llama.service" }
    fn title(&self) -> &'static str { "Служба модели" }
    fn state(&self, ctx: &Ctx) -> OpState {
        let m = &ctx.cfg.model;
        if m.file.is_empty() {
            return OpState::Blocked("модель не выбрана: [model] file — путь к .gguf на этой машине".into());
        }
        if !std::path::Path::new(&m.file).exists() {
            return OpState::Blocked(format!("нет файла модели {}", m.file));
        }
        if llama::gpu(ctx).is_none() {
            return OpState::Blocked("нет видеокарты (nvidia-smi не отвечает): модель для агентов на процессоре неработоспособна".into());
        }
        if llama::kit_bin(ctx).is_none() && !llama::server_exe().exists() {
            return OpState::Blocked("нет сервера модели: в комплекте нет llama/bin".into());
        }
        if cfg!(windows) && llama::nssm(ctx).is_none() {
            return OpState::Blocked("нет nssm: его кладут в комплект как llama/nssm.exe".into());
        }
        let got = llama::installed(ctx);
        if !got.present {
            if llama::port_busy(m.port) {
                return OpState::Blocked(format!("порт {} занят другой программой — [model] port", m.port));
            }
            return OpState::Missing;
        }
        let baked = ctx.kit.state_get("llama.args").unwrap_or_default();
        if let Some(d) = args_drift(&desired_args(ctx), &baked) {
            return OpState::Drifted { from: d.0, to: d.1 };
        }
        if !got.running {
            return OpState::Drifted { from: "служба остановлена".into(), to: "работает".into() };
        }
        OpState::Done(format!("{} на {}:{}", ctx.cfg.model_alias(), llama::bind_host(ctx), m.port))
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![
            Action::new(format!("служба {} ({}): {}", llama::SERVICE, if cfg!(windows) { "nssm" } else { "systemd" }, ctx.cfg.model.file)),
            Action::new(format!("три слота, общий кэш, окно подбирает сервер (не ниже {})", ctx.cfg.model.fit_ctx)),
        ]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let _ = llama::ensure_token(ctx)?;
        if llama::installed(ctx).present {
            llama::service_remove(ctx)?;
        }
        let args = desired_args(ctx);
        llama::service_install(ctx, &args)?;
        ctx.kit.state_set("llama.args", &args.join(" "))?;
        llama::service_start(ctx)?;
        ctx.ok("служба модели зарегистрирована и запущена");
        Ok(())
    }
    fn revert(&self, _ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        Some(vec![Action::new(format!("снять службу {} (модель и файлы остаются)", llama::SERVICE))])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        llama::service_remove(ctx)?;
        ctx.ok("служба модели снята");
        Ok(())
    }
}

/// Сервер отвечает, и мы знаем его настоящие числа. Всегда выполняется:
/// это и есть синхронизация с Zed.
struct Answers;
impl Op<Ctx> for Answers {
    fn id(&self) -> &'static str { "llama.answers" }
    fn title(&self) -> &'static str { "Замер сервера модели" }
    fn always_run(&self) -> bool { true }
    fn state(&self, _ctx: &Ctx) -> OpState { OpState::Missing }
    fn plan(&self, _ctx: &Ctx) -> Vec<Action> {
        vec![Action::new("дождаться ответа сервера и прочитать выбранное окно (до пяти минут)"), Action::new("замерить скорость разбора и записать числа для Zed")]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        if ctx.dry_run {
            return Ok(());
        }
        let token = llama::ensure_token(ctx)?;
        let url = local_url(ctx);
        llama::wait_health(&url, &token, 300).map_err(|e| anyhow::anyhow!("{e:#} — смотрите {}", llama::log_file().display()))?;
        let p = llama::props(&url, &token).ok_or_else(|| anyhow::anyhow!("сервер не отдал /props"))?;
        let max_tokens = verdict(&ctx.cfg, p.window)?;
        ctx.ok(format!("окно {} токенов, слотов {}, модель {}; Zed получает max_tokens = {max_tokens}", p.window, p.slots, p.model));
        let prefill = llama::measure_prefill(&url, &token);
        match prefill {
            Some(s) => ctx.ok(format!("скорость разбора {s:.0} токенов в секунду")),
            None => ctx.warn("скорость разбора замерить не вышло (ответ без timings)"),
        }
        llama::facts_save(ctx, &Facts { token, window: p.window, max_tokens }, prefill)?;
        if let Some(g) = llama::gpu(ctx) {
            if g.total_mb > 0 && g.used_mb * 100 / g.total_mb > 95 {
                ctx.warn(format!("карта занята на {} из {} МиБ: соседняя нагрузка будет вытеснять модель", g.used_mb, g.total_mb));
            }
        }
        Ok(())
    }
}
```

В `ops.rs`:
- `server()`: `pub fn server(cfg: &Cfg) -> Vec<Box<dyn Op<Ctx>>>` → `Env, Image, Volumes` + `crate::ops_llama::ops(cfg.form())` + `Model` (только `Form::Docker`) + `Container, Services`. Вызовы `ops::server()` в `main.rs` и `gui.rs` → `ops::server(&ctx.cfg)`.
- `Model` (`"model"`): при `Form::Host` — `Done(file)` если файл есть, иначе `Blocked`; при `Form::Remote` — `Done("удалённая: name")`; при `Form::Docker` — как сейчас.
- `run_args(cfg, facts)`: после `--device /dev/fuse` при `engine == "docker"` добавить `--add-host host.docker.internal:host-gateway`; переменные из `cfg.container_env(facts)`.
- `Container::state/plan/apply`: `let facts = crate::llama::facts_load(ctx);` и передавать `facts.as_ref()`; в `plan` при существующем контейнере текст действия: `"{engine} rm -f {name} (тома не трогаются; сессии RDP разорвутся)"`.
- `drift_of(cfg, image, first_port, env, facts)` — `cfg.container_env(facts)`.
- `Services`: убрать ветку `llama-server` (внутренняя ллама всегда выключена).

- [ ] **Step 4: Тесты зелёные; статус без карты**

Run: `cargo test -p dev1c-setup --manifest-path setup/Cargo.toml` и `cargo run -p dev1c-setup --manifest-path setup/Cargo.toml --no-default-features -- --kit <каталог с dev1c.toml, serve="host", file=путь к .gguf> status` — `llama.service` показывает причину (`Blocked`/`Missing`), ничего не меняется.

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup
git commit -m "dev1c-setup: операции сервера модели на хосте — файлы, служба, замер; числа замера идут в контейнер среды"
```

---

### Task 5: Образ принимает числа окна (`entrypoint.sh`, `zed-settings.json`)

**Files:**
- Modify: `config/zed-settings.json` (строки `"max_tokens": 55296`, `"max_output_tokens": 8192` → плейсхолдеры)
- Modify: `config/entrypoint.sh` (подстановка `CUSTOM_MAX_TOKENS`, `CUSTOM_MAX_OUTPUT_TOKENS`)
- Modify: `config/test-entrypoint-ports.sh` (проверка, что плейсхолдеров в домашних каталогах не осталось)

**Interfaces:**
- Consumes: переменные контейнера из Задачи 1.
- Produces: `settings.json` рабочих мест с числами из переменных; умолчания 55296/8192, если переменные не заданы (удалённый сервер без замера).

- [ ] **Step 1: Проверка в тесте образа (падает до правки)**

В `config/test-entrypoint-ports.sh` после существующих проверок:

```sh
for u in dev1 dev2 dev3; do
    f=/home/$u/.config/zed/settings.json
    grep -q 'CUSTOM_MAX_TOKENS_PLACEHOLDER\|CUSTOM_MAX_OUTPUT_TOKENS_PLACEHOLDER' "$f" && fail "$u: плейсхолдер окна не подставлен"
    grep -q '"max_tokens": [0-9]' "$f" || fail "$u: max_tokens не число"
done
```

(функция `fail` — та, что уже есть в скрипте.)

- [ ] **Step 2: Правка настроек и entrypoint**

В `zed-settings.json`: `"max_tokens": CUSTOM_MAX_TOKENS_PLACEHOLDER,` и `"max_output_tokens": CUSTOM_MAX_OUTPUT_TOKENS_PLACEHOLDER,` (комментарий над ними дополнить: «числа подставляет entrypoint: из замера dev1c-setup или умолчания 55296/8192»). В `entrypoint.sh` в цикле по `/home/*/.config/zed/settings.json` перед веткой `CUSTOM_API_URL`:

```sh
    sed -i \
        -e "s|CUSTOM_MAX_TOKENS_PLACEHOLDER|${CUSTOM_MAX_TOKENS:-55296}|" \
        -e "s|CUSTOM_MAX_OUTPUT_TOKENS_PLACEHOLDER|${CUSTOM_MAX_OUTPUT_TOKENS:-8192}|" \
        "$cfg"
```

и добавить `CUSTOM_MAX_TOKENS CUSTOM_MAX_OUTPUT_TOKENS` в список переменных, которые уходят в `/etc/environment` (по той же причине, что `CUSTOM_API_URL`: сессия RDP строит окружение заново — но в settings.json они уже подставлены, поэтому в `/etc/environment` они нужны только для диагностики; оставить в списке).

- [ ] **Step 3: Локальная проверка подстановки без сборки образа**

Run: `bash -c 'f=$(mktemp); cp config/zed-settings.json $f; CUSTOM_MAX_TOKENS=120832 sed -i -e "s|CUSTOM_MAX_TOKENS_PLACEHOLDER|${CUSTOM_MAX_TOKENS:-55296}|" -e "s|CUSTOM_MAX_OUTPUT_TOKENS_PLACEHOLDER|${CUSTOM_MAX_OUTPUT_TOKENS:-8192}|" $f; grep -n "max_tokens\|max_output_tokens" $f'` — ожидание: `"max_tokens": 120832,` и `"max_output_tokens": 8192,`.

- [ ] **Step 4: Commit** (CI соберёт образ и прогонит `test-entrypoint-ports.sh` внутри)

```bash
git add config/zed-settings.json config/entrypoint.sh config/test-entrypoint-ports.sh
git commit -m "Образ: окно модели в настройках Zed подставляет entrypoint из CUSTOM_MAX_TOKENS и CUSTOM_MAX_OUTPUT_TOKENS"
```

---

### Task 6: Факты проверки: видеокарта, сервер модели, три числа (`verify.rs`)

**Files:**
- Modify: `setup/dev1c-setup/src/verify.rs`

**Interfaces:**
- Consumes: `llama::{gpu, props, facts_load}`, `ops_llama::local_url`, `cfg::Form`.
- Produces: `pub fn numbers_fact(window: u32, zed_max_tokens: Option<u32>, agents: u16) -> Check` (чистая, с тестом); три новых факта в `run` между «порты» и «модель в настройках Zed».

- [ ] **Step 1: Тест (падает)**

```rust
    #[test]
    fn the_three_numbers_must_agree() {
        let c = numbers_fact(131072, Some(120832), 1);
        assert!(c.ok, "{c:?}");
        let c = numbers_fact(131072, Some(130000), 1);
        assert!(!c.ok && c.detail.contains("130000") && c.detail.contains("131072"), "{c:?}");
        let c = numbers_fact(131072, None, 1);
        assert!(!c.ok && c.hint.contains("install"), "{c:?}");
        // На три агента бюджет × 3 + ответ не должен превышать окно.
        assert!(!numbers_fact(131072, Some(60000), 3).ok);
    }
```

- [ ] **Step 2: Запустить — падает.**

- [ ] **Step 3: Реализация**

```rust
/// Окно сервера, бюджет агента в Zed и слоты обязаны сойтись:
/// max_tokens × agents + ответ ≤ окно.
pub fn numbers_fact(window: u32, zed_max_tokens: Option<u32>, agents: u16) -> Check {
    let Some(mt) = zed_max_tokens else {
        return Check::fail("три числа", "в настройках Zed нет max_tokens", "dev1c-setup install — замер запишет числа в настройки");
    };
    let need = mt as u64 * agents.max(1) as u64 + crate::cfg::OUTPUT_TOKENS as u64;
    if need <= window as u64 {
        Check::pass("три числа", format!("окно {window}, max_tokens {mt} × {agents} + ответ {} ≤ окна", crate::cfg::OUTPUT_TOKENS))
    } else {
        Check::fail("три числа", format!("окно {window}, а max_tokens {mt} × {agents} + ответ {} = {need}", crate::cfg::OUTPUT_TOKENS), "dev1c-setup install — пересчитает и пересоздаст контейнер")
    }
}
```

В `run` для `Form::Host | Form::Docker`:

```rust
    if ctx.cfg.form() != Form::Remote {
        v.push(match llama::gpu(ctx) {
            Some(g) => Check::pass("видеокарта", format!("{}, {} МиБ, занято {} МиБ, драйвер {}", g.name, g.total_mb, g.used_mb, g.driver)),
            None => Check::fail("видеокарта", "nvidia-smi не отвечает", "драйвер NVIDIA на этой машине"),
        });
        let facts = llama::facts_load(ctx);
        let token = facts.as_ref().map(|f| f.token.clone()).unwrap_or_default();
        let props = llama::props(&crate::ops_llama::local_url(ctx), &token);
        v.push(match &props {
            Some(p) => Check::pass("сервер модели", format!("{}: окно {} токенов, слотов {}, модель {}", ctx.cfg.model_url(), p.window, p.slots, p.model)),
            None => Check::fail("сервер модели", format!("{} не отвечает", crate::ops_llama::local_url(ctx)), "dev1c-setup install; журнал службы — llama.log в каталоге сервера"),
        });
        if let Some(p) = props {
            v.push(numbers_fact(p.window, zed_max_tokens(&settings), ctx.cfg.model.agents));
        }
    }
```

где `settings` — уже прочитанный `settings.json` dev1 (перенести чтение выше), а

```rust
fn zed_max_tokens(settings: &str) -> Option<u32> {
    settings.lines().find_map(|l| l.trim().strip_prefix("\"max_tokens\":")).and_then(|v| v.trim().trim_end_matches(',').parse().ok())
}
```

Факт «модель в настройках Zed» для локальных форм ждёт провайдера `custom` и модель `model_alias()` (переменная `want_provider`/`want_model`: `Form::Remote` → `("custom", name)`, иначе `("custom", model_alias())`).

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p dev1c-setup --manifest-path setup/Cargo.toml verify::`

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup/src/verify.rs
git commit -m "dev1c-setup check: видеокарта, сервер модели, три числа"
```

---

### Task 7: Контейнерная форма (`ops_llama::Container`)

**Files:**
- Modify: `setup/dev1c-setup/src/ops_llama.rs`
- Modify: `setup/dev1c-setup/src/cfg.rs` (`pub fn llama_image(&self) -> String { "oktaplus/dev1c-llama:latest".into() }`, `pub fn llama_container(&self) -> String { format!("{}-llama", self.server.name) }`)

**Interfaces:**
- Produces: `pub fn llama_run_args(cfg: &Cfg, token: &str, cache_ram: u32, fit_target: u32) -> Vec<String>`; `ops(Form::Docker)` = `[Container, Answers]`.

- [ ] **Step 1: Тест (падает)**

```rust
    #[test]
    fn the_container_form_runs_the_server_from_the_models_volume() {
        let mut c = Cfg::default();
        c.model.serve = "docker".into();
        c.model.file = "models/x.gguf".into();
        let a = llama_run_args(&c, "tok", 4096, 1024).join(" ");
        assert!(a.starts_with("run -d --name dev1c-llama --restart unless-stopped --gpus all -p 3392:8080 -v dev1c-models:/opt/models -e LLAMA_API_KEY=tok oktaplus/dev1c-llama:latest llama-server"), "{a}");
        assert!(a.contains("-m /opt/models/x.gguf --alias x --host 0.0.0.0 --port 8080"));
        assert!(!a.contains("--api-key-file"));
    }
```

- [ ] **Step 2: Запустить — падает.**

- [ ] **Step 3: Реализация**

```rust
pub fn llama_run_args(cfg: &crate::cfg::Cfg, token: &str, cache_ram: u32, fit_target: u32) -> Vec<String> {
    let base = std::path::Path::new(&cfg.model.file).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let model = format!("/opt/models/{base}");
    let alias = cfg.model_alias();
    let mut a: Vec<String> = [
        "run", "-d", "--name", &cfg.llama_container(), "--restart", "unless-stopped", "--gpus", "all",
        "-p", &format!("{}:8080", cfg.ports()[2]), "-v", &format!("{}:/opt/models", cfg.volume("models")),
        "-e", &format!("LLAMA_API_KEY={token}"), &cfg.llama_image(), "llama-server",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend(llama::serve_args(&llama::ServeInput { model: &model, alias: &alias, host: "0.0.0.0", port: 8080, fit_ctx: cfg.model.fit_ctx, fit_target, cache_ram, token_file: None, token_env: true }));
    a
}

struct Container;
impl Op<Ctx> for Container {
    fn id(&self) -> &'static str { "llama.container" }
    fn title(&self) -> &'static str { "Контейнер модели" }
    fn state(&self, ctx: &Ctx) -> OpState {
        if ctx.cfg.model.file.is_empty() {
            return OpState::Blocked("модель не выбрана: [model] file — файл в комплекте".into());
        }
        if !docker::image_loaded(ctx, &ctx.cfg.llama_image()) {
            return OpState::Blocked(format!("образ {} не загружен: он едет в том же архиве, что и образ среды", ctx.cfg.llama_image()));
        }
        if llama::gpu(ctx).is_none() {
            return OpState::Blocked("нет видеокарты (nvidia-smi не отвечает)".into());
        }
        let name = ctx.cfg.llama_container();
        if let Some(st) = crate::ops::state_before_drift(&docker::container_state(ctx, &name)) {
            return st;
        }
        let baked = ctx.kit.state_get("llama.args").unwrap_or_default();
        let want = llama_run_args(&ctx.cfg, "…", llama::cache_ram_mb(llama::free_ram_mb(ctx)), llama::fit_target_mb(llama::gpu(ctx).map(|g| g.used_mb).unwrap_or(0)));
        if let Some(d) = args_drift(&want, &baked) {
            return OpState::Drifted { from: d.0, to: d.1 };
        }
        OpState::Done(format!("работает, порт {}", ctx.cfg.ports()[2]))
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!("{} run … {} llama-server (три слота, окно не ниже {})", ctx.engine(), ctx.cfg.llama_image(), ctx.cfg.model.fit_ctx))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let token = llama::ensure_token(ctx)?;
        let _ = docker::run(ctx, &["rm", "-f", &ctx.cfg.llama_container()]);
        let used = llama::gpu(ctx).map(|g| g.used_mb).unwrap_or(0);
        let args = llama_run_args(&ctx.cfg, &token, llama::cache_ram_mb(llama::free_ram_mb(ctx)), llama::fit_target_mb(used));
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let o = docker::run(ctx, &refs)?;
        anyhow::ensure!(o.ok(), "контейнер модели не запустился: {}", o.tail(3).join(" "));
        // В состоянии токен заменён на «…»: сравнение не должно зависеть от него.
        let baked = llama_run_args(&ctx.cfg, "…", llama::cache_ram_mb(llama::free_ram_mb(ctx)), llama::fit_target_mb(used));
        ctx.kit.state_set("llama.args", &baked.join(" "))?;
        ctx.ok("контейнер модели запущен");
        Ok(())
    }
    fn revert(&self, ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        Some(vec![Action::new(format!("{} rm -f {}", ctx.engine(), ctx.cfg.llama_container()))])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        let _ = docker::run(ctx, &["rm", "-f", &ctx.cfg.llama_container()]);
        ctx.ok("контейнер модели снят");
        Ok(())
    }
}
```

`ops(Form::Docker)` → `vec![Box::new(Container), Box::new(Answers)]`; в `Answers::apply` токен для контейнерной формы — тот же `ensure_token` (файл в `dir()` на хосте, значение ушло в `-e`). `Model` (копия в том) идёт **перед** `llama.container`: в `ops::server` порядок `Env, Image, Volumes, Model, [llama ops], Container, Services`.

- [ ] **Step 4: Тесты зелёные**

Run: `cargo test -p dev1c-setup --manifest-path setup/Cargo.toml`

- [ ] **Step 5: Commit**

```bash
git add setup/dev1c-setup
git commit -m "dev1c-setup: контейнерная форма сервера модели — образ dev1c-llama, том моделей, токен переменной"
```

---

### Task 8: Окно: выбор формы модели, поля по форме, адрес в «Доступе»

**Files:**
- Modify: `setup/dev1c-setup/ui/index.html` (группа «Модель для агентов»)
- Modify: `setup/dev1c-setup/ui/app.js` (`KEYS`, показ/скрытие полей по `serve`, строка «Сервер модели» в «Доступе»)
- Modify: `setup/dev1c-setup/src/gui.rs` (`Machine` получает `model_url: String`, `facts: Option<Facts>`; `Facts` — `#[derive(Serialize)]`)

**Interfaces:**
- Consumes: `Cfg::model_url`, `llama::facts_load`.

- [ ] **Step 1: Разметка**

Заменить группу «Модель для агентов» на:

```html
      <div class="group">
        <p class="group-title">Модель для агентов</p>
        <div class="field">
          <label for="f-model.serve">Где работает сервер модели</label>
          <select id="f-model.serve" data-key="model.serve">
            <option value="host">Служба на этой машине (видеокарта, nssm)</option>
            <option value="docker">Контейнер рядом со средой</option>
            <option value="remote">Удалённый сервер</option>
          </select>
        </div>
        <div data-form="host docker" class="field">
          <label for="f-model.file">Файл модели (.gguf)</label>
          <input id="f-model.file" data-key="model.file" spellcheck="false">
          <div class="hint" id="file-hint"></div>
        </div>
        <div data-form="remote" class="field">
          <label for="f-model.url">Адрес сервера</label>
          <input id="f-model.url" data-key="model.url" placeholder="https://host/v1" spellcheck="false">
          <div class="hint">С путём, но без завершающего /chat/completions.</div>
        </div>
        <div class="cols2">
          <div class="field">
            <label for="f-model.name">Имя модели</label>
            <input id="f-model.name" data-key="model.name" spellcheck="false">
            <div class="hint" data-form="host docker">Пусто — имя файла.</div>
          </div>
          <div data-form="remote" class="field">
            <label for="f-model.key">Ключ</label>
            <input id="f-model.key" data-key="model.key" type="password">
          </div>
          <div data-form="host" class="field">
            <label for="f-model.port">Порт службы</label>
            <input id="f-model.port" data-key="model.port" type="number" min="1024" max="65535">
          </div>
        </div>
        <div class="cols2" data-form="host docker">
          <div class="field">
            <label for="f-model.agents">Делить окно на агентов</label>
            <select id="f-model.agents" data-key="model.agents"><option>1</option><option>2</option><option>3</option></select>
            <div class="hint">1 — не делить (рекомендуется).</div>
          </div>
          <div class="field">
            <label for="f-model.fit_ctx">Окно не меньше, токенов</label>
            <input id="f-model.fit_ctx" data-key="model.fit_ctx" type="number" min="16384" step="1024">
          </div>
        </div>
      </div>
```

- [ ] **Step 2: Поведение**

В `app.js`: `KEYS` += `'model.serve', 'model.port', 'model.agents', 'model.fit_ctx'`; в `refresh` после заполнения полей — `syncForm()`; обработчик `change` на `#f-model.serve` → `syncForm()`:

```js
function syncForm() {
  const form = $('f-model.serve').value;
  for (const el of document.querySelectorAll('[data-form]')) el.hidden = !el.dataset.form.split(' ').includes(form);
  $('file-hint').textContent = form === 'host'
    ? 'Полный путь на этой машине, например C:\\models\\Qwen3.5-4B.Q4_K_M.gguf. В комплект не кладётся.'
    : 'Путь относительно комплекта; при установке копируется в том моделей.';
}
```

В `save`: числовые поля (`port`, `agents`, `fit_ctx`) → `Number(...)` (уже есть по `type=number`; для `select` agents — `Number(f.value)` явно). В «Доступе» строка `['Сервер модели', machine.model_url, machine.facts ? `окно ${machine.facts.window}, max_tokens ${machine.facts.max_tokens}` : 'замера ещё не было']` вместо `http://<адрес>:${model}`. В `gui.rs::machine`: `model_url: ctx.cfg.model_url(), facts: crate::llama::facts_load(&ctx)`.

- [ ] **Step 3: Проверка руками**

Run: `cargo build -p dev1c-setup --manifest-path setup/Cargo.toml` (отладочная сборка с окном), запустить с комплектом; переключить форму — поля меняются; сохранить `host` с относительным путём — сообщение `validate` в строке статуса; «Доступ» показывает адрес модели.

- [ ] **Step 4: Commit**

```bash
git add setup/dev1c-setup/ui setup/dev1c-setup/src/gui.rs
git commit -m "dev1c-setup window: выбор формы сервера модели, поля по форме, адрес и числа замера в «Доступе»"
```

---

### Task 9: Комплект: llama и nssm в `pack.sh`

**Files:**
- Modify: `pack.sh`
- Modify: `.github/workflows/build.yml` (шаг `pack.sh` получает `--llama windows`)
- Modify: `setup/common/src/integrity.rs` — ничего (бинарники попадают в `checksums.txt` как есть)

**Interfaces:**
- Produces: `dist/dev1c-kit/llama/bin/windows-x64-cuda13/*` (содержимое `llama-b11606-bin-win-cuda-13.4-x64.zip` + `cudart-llama-bin-win-cuda-13.4-x64.zip`), `dist/dev1c-kit/llama/nssm.exe` (из `nssm-2.24.zip`, `win64/nssm.exe`); параметр `--llama windows|linux|none` (умолчание `windows`).

- [ ] **Step 1: Правка `pack.sh`**

После раскладки `dev1c-kit` (перед `checksums`):

```sh
LLAMA_BUILD=b11606
NSSM_URL=https://nssm.cc/release/nssm-2.24.zip
llama_row() { # $1 = windows|linux
    local dst="$KIT/llama/bin/$1-x64-cuda13" tmp
    tmp=$(mktemp -d)
    mkdir -p "$dst"
    case "$1" in
        windows)
            curl -fsSL -o "$tmp/l.zip" "https://github.com/ggml-org/llama.cpp/releases/download/$LLAMA_BUILD/llama-$LLAMA_BUILD-bin-win-cuda-13.4-x64.zip"
            curl -fsSL -o "$tmp/c.zip" "https://github.com/ggml-org/llama.cpp/releases/download/$LLAMA_BUILD/cudart-llama-bin-win-cuda-13.4-x64.zip"
            unzip -qo "$tmp/l.zip" -d "$dst" && unzip -qo "$tmp/c.zip" -d "$dst"
            curl -fsSL -o "$tmp/n.zip" "$NSSM_URL" && unzip -qoj "$tmp/n.zip" 'nssm-2.24/win64/nssm.exe' -d "$KIT/llama"
            ;;
        linux)
            curl -fsSL -o "$tmp/l.tgz" "https://github.com/ggml-org/llama.cpp/releases/download/$LLAMA_BUILD/llama-$LLAMA_BUILD-bin-ubuntu-cuda-13.4-x64.tar.gz"
            curl -fsSL -o "$tmp/c.tgz" "https://github.com/ggml-org/llama.cpp/releases/download/$LLAMA_BUILD/cudart-llama-$LLAMA_BUILD-bin-ubuntu-cuda-13.4-x64.tar.gz"
            tar -xzf "$tmp/l.tgz" -C "$dst" --strip-components=1 && tar -xzf "$tmp/c.tgz" -C "$dst" --strip-components=1
            ;;
    esac
    rm -rf "$tmp"
    # Архив кладёт файлы во вложенный каталог build/bin — поднять их наверх.
    if [ -d "$dst/build/bin" ]; then mv "$dst"/build/bin/* "$dst"/ && rm -rf "$dst/build"; fi
    echo "llama $1: $(ls "$dst" | wc -l) файлов"
}
case "${LLAMA_ROW:-windows}" in windows|linux) llama_row "$LLAMA_ROW";; none) ;; esac
```

Параметр `--llama X` разбирается в начале скрипта в `LLAMA_ROW`; `README`-строка в шапке `pack.sh` дополнена. Проверить фактическую раскладку архива (`unzip -l`) перед коммитом: имя каталога внутри может отличаться от `build/bin` — тогда поправить условие по факту.

- [ ] **Step 2: Локальная проверка раскладки**

Run: `./pack.sh --llama windows <image.tar.gz> dist/ci` (образ можно взять из артефакта CI или пустой файл-заглушку для проверки раскладки) — ожидание: `dist/dev1c-kit/llama/bin/windows-x64-cuda13/llama-server.exe`, `.../cudart64_*.dll`, `dist/dev1c-kit/llama/nssm.exe`; `checksums.txt` содержит их.

- [ ] **Step 3: CI**

В `build.yml` шаг с `pack.sh` → `./pack.sh --llama windows dev1c-image.tar.gz bins`; артефакт `dev1c-kit` растёт примерно на 550 МБ — указать в комментарии шага.

- [ ] **Step 4: Commit**

```bash
git add pack.sh .github/workflows/build.yml
git commit -m "pack.sh: llama b11606 (win-cuda-13.4 + cudart) и nssm 2.24 в комплект администратора"
```

---

### Task 10: Образ без CUDA и внутренней лламы

**Files:**
- Modify: `Dockerfile` (`FROM debian:13` вместо `ARG LLAMA_IMAGE`/`FROM $LLAMA_IMAGE`; убрать копирование/ссылки на llama)
- Modify: `config/entrypoint.sh` (удалить блок `LLAMA_ENABLED`, поиск `.gguf`, `MODEL_ALIAS`; ветка настроек без `CUSTOM_API_URL` → предупреждение «модель не задана»)
- Modify: `config/zed-settings.json` (провайдер `llama.cpp` убрать; `default_model.provider` → `custom`)
- Modify: `config/test-entrypoint-ports.sh`, `config/test-skills.sh` — проверки, упоминающие `llama-server`
- Modify: `setup/dev1c-setup/src/cfg.rs` (`container_env` без `LLAMA_ENABLED`) и тесты; `verify.rs` факт «модель в настройках Zed» — провайдер всегда `custom`
- Modify: `docs/STATUS.md` («Where it runs»: образ без CUDA, порт 3392 — контейнер модели или свободен)

- [ ] **Step 1: Снять тесты на `LLAMA_ENABLED`** (`cfg::tests`: убрать `assert!(env.contains(&("LLAMA_ENABLED"…`), `ops::tests` — строку `-e LLAMA_ENABLED=0`); запустить — падают из-за оставшегося кода.
- [ ] **Step 2: Правки Dockerfile/entrypoint/settings** по списку выше; `grep -rn 'LLAMA\|llama' Dockerfile config/ setup/dev1c-setup/src` оставляет только новые операции модели.
- [ ] **Step 3: `cargo test -p dev1c-setup` зелёный; `bash -n config/entrypoint.sh`.**
- [ ] **Step 4: Commit и CI** — образ пересобирается (без CUDA — быстрее и на ~630 МБ меньше), `dev1c-setup install` в CI с `serve = "remote"`, проверки проходят.

```bash
git add Dockerfile config setup/dev1c-setup docs/STATUS.md
git commit -m "Образ без CUDA и llama.cpp: модель для Zed всегда провайдер custom, сервер — у dev1c-setup"
```

---

### Task 11: Проверка на этой машине и ledger

**Files:**
- Modify: `docs/STATUS.md` («Verified by running it», «Known traps», «Next»)

- [ ] **Step 1: Хостовая форма** — `dev1c.toml` с `serve = "host"`, `file = <путь к Qwen3.5-4B.Q4_K_M.gguf на этом диске>`, `port = 1235` (убедиться, что `volt-llama` остановлен: порт свободен); `dev1c-setup install` из комплекта с `llama/`; ожидание: служба `dev1c-llama` в `services.msc`, `llama.answers` печатает окно и max_tokens, контейнер пересоздан с `CUSTOM_MAX_TOKENS`, `check` — три новых факта зелёные; в Zed (RDP dev1) агент отвечает.
- [ ] **Step 2: Повторный `install`** — «Менять нечего»; `change model.agents=3` — `llama.service` `Drifted`, служба пересоздана, контейнер пересоздан с новым `max_tokens`.
- [ ] **Step 3: `clean --stack`** — служба снята, файлы остались; `install` возвращает.
- [ ] **Step 4: Записать в STATUS** что проверено (числа окна и скорости на 3060 Ti), ловушки по дороге, следующий шаг (контейнерная форма проверена только в CI-сборке — на этой машине GPU в Docker Desktop занят volt-llama), и закоммитить.

```bash
git add docs/STATUS.md
git commit -m "Ledger: сервер инференса на хосте проверен на этой машине"
```
