//! Параметры комплекта: `dev1c.toml`. Единственный источник; переменные
//! контейнера, порты и имена томов выводятся отсюда.

use anyhow::{Context, Result};
use common::kit::Kit;
use serde::{Deserialize, Serialize};

/// Образец с комментариями: он же кладётся в комплект как dev1c.toml. Тест
/// ниже держит его и структуру в согласии.
pub const EXAMPLE: &str = include_str!("../dev1c.example.toml");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cfg {
    pub server: Server,
    pub model: Model,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    pub name: String,
    pub port: u16,
    pub memory: String,
    pub gpu: bool,
    pub engine: String,
}

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

impl Default for Server {
    fn default() -> Self {
        Self { name: "dev1c".into(), port: 3390, memory: "24g".into(), gpu: false, engine: "docker".into() }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self {
            serve: "host".into(),
            file: String::new(),
            url: String::new(),
            key: String::new(),
            name: String::new(),
            port: 1235,
            agents: 1,
            fit_ctx: 65536,
        }
    }
}

/// Форма сервера модели.
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

/// Ответ агента, токенов (≈30 КБ текста — хватает на ответ).
pub const OUTPUT_TOKENS: u32 = 8192;

impl Default for Cfg {
    fn default() -> Self {
        Self { server: Server::default(), model: Model::default() }
    }
}

pub const FILE: &str = "dev1c.toml";

impl Cfg {
    /// Читает `dev1c.toml` из комплекта. Нет файла — значения по умолчанию,
    /// и `status` скажет об этом.
    pub fn load(kit: &Kit) -> Result<Cfg> {
        let p = kit.path(FILE);
        if !p.exists() {
            return Ok(Cfg::default());
        }
        let text = std::fs::read_to_string(&p).with_context(|| format!("не прочитать {}", p.display()))?;
        let cfg: Cfg = toml::from_str(&text).with_context(|| format!("{FILE}: ошибка в файле параметров"))?;
        cfg.validate()?;
        Ok(cfg)
    }

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
        // Path::file_stem on Linux does not split a Windows path: do it by hand.
        let tail = self.model.file.rsplit(['/', '\\']).next().unwrap_or_default();
        tail.rsplit_once('.').map(|(s, _)| s).unwrap_or(tail).to_string()
    }

    /// Адрес сервера модели, как его видит контейнер среды.
    pub fn model_url(&self) -> String {
        match self.form() {
            Form::Remote => self.model.url.clone(),
            Form::Host => format!("http://host.docker.internal:{}/v1", self.model.port),
            Form::Docker => format!("http://host.docker.internal:{}/v1", self.ports()[2]),
        }
    }

    /// Ошибка называет поле и что с ним делать.
    pub fn validate(&self) -> Result<()> {
        let m = &self.model;
        anyhow::ensure!(matches!(m.serve.as_str(), "host" | "docker" | "remote"), "[model]: serve — host, docker или remote");
        anyhow::ensure!((1..=3).contains(&m.agents), "[model]: agents — от 1 до 3");
        anyhow::ensure!(m.fit_ctx >= 16384, "[model]: fit_ctx — не меньше 16384");
        anyhow::ensure!(
            m.file.is_empty() || m.url.is_empty(),
            "[model]: задано и file, и url — оставьте одно: файл (serve = \"host\" или \"docker\") или удалённый сервер (serve = \"remote\")"
        );
        // Is the path absolute on either OS? The kit is written on Windows and read on Linux too.
        let absolute = |p: &str| p.starts_with('/') || p.starts_with('\\') || p.get(1..3).is_some_and(|s| s == ":\\" || s == ":/");
        match self.form() {
            Form::Remote => {
                anyhow::ensure!(!m.url.is_empty(), "[model]: для serve = \"remote\" нужен url");
                anyhow::ensure!(!m.name.is_empty(), "[model]: для удалённого сервера нужно name — имя модели, как сервер его называет");
                // key may be empty: a server inside the network often has none.
            }
            Form::Host => {
                anyhow::ensure!(m.url.is_empty(), "[model]: url задан, а serve не \"remote\" — выберите одно: serve = \"remote\" или пустой url");
                anyhow::ensure!(
                    m.file.is_empty() || absolute(&m.file),
                    "[model]: при serve = \"host\" file — полный путь к .gguf на этой машине; для файла в комплекте выберите serve = \"docker\""
                );
            }
            Form::Docker => {
                anyhow::ensure!(m.url.is_empty(), "[model]: url задан, а serve не \"remote\" — выберите одно: serve = \"remote\" или пустой url");
                anyhow::ensure!(m.file.is_empty() || !absolute(&m.file), "[model]: при serve = \"docker\" file — путь относительно комплекта");
            }
        }
        anyhow::ensure!(
            self.server.port >= 1024 && self.server.port <= 65532,
            "[server]: port должен быть от 1024 до 65532 (нужны четыре порта подряд)"
        );
        anyhow::ensure!(matches!(self.server.engine.as_str(), "docker" | "podman"), "[server]: engine — docker или podman");
        Ok(())
    }

    /// RDP, Gitea, модель, SSH.
    pub fn ports(&self) -> [u16; 4] {
        let p = self.server.port;
        [p, p + 1, p + 2, p + 3]
    }

    /// Что уходит в контейнер через -e. Локальные формы говорят про модель
    /// только после замера (`facts`): адрес, токен, имя и числа окна для Zed.
    pub fn container_env(&self, facts: Option<&Facts>) -> Vec<(String, String)> {
        let m = &self.model;
        let mut env = Vec::new();
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

    /// `dev1c.toml` с текущими значениями и всеми комментариями образца:
    /// `change` правит значения на месте, а не пересобирает файл с нуля.
    pub fn render(cfg: &Cfg) -> String {
        common::toml_render::render(EXAMPLE, |section, key| {
            Some(match (section, key) {
                ("[server]", "name") => format!("{:?}", cfg.server.name),
                ("[server]", "port") => cfg.server.port.to_string(),
                ("[server]", "memory") => format!("{:?}", cfg.server.memory),
                ("[server]", "gpu") => cfg.server.gpu.to_string(),
                ("[server]", "engine") => format!("{:?}", cfg.server.engine),
                ("[model]", "serve") => format!("{:?}", cfg.model.serve),
                ("[model]", "file") => format!("{:?}", cfg.model.file),
                ("[model]", "url") => format!("{:?}", cfg.model.url),
                ("[model]", "key") => format!("{:?}", cfg.model.key),
                ("[model]", "name") => format!("{:?}", cfg.model.name),
                ("[model]", "port") => cfg.model.port.to_string(),
                ("[model]", "agents") => cfg.model.agents.to_string(),
                ("[model]", "fit_ctx") => cfg.model.fit_ctx.to_string(),
                _ => return None,
            })
        })
    }

    /// Образец с одной подставленной формой обслуживания: умалчивание
    /// (`Cfg::default()`) не трогаем — только `model.serve` для показа.
    pub fn example_for(serve: &str) -> String {
        let mut cfg = Cfg::default();
        cfg.model.serve = serve.into();
        Cfg::render(&cfg)
    }

    pub fn volume(&self, suffix: &str) -> String {
        format!("{}-{}", self.server.name, suffix)
    }

    pub fn image(&self) -> String {
        "oktaplus/dev1c:latest".into()
    }

    /// Образ сервера модели (serve = "docker"); едет в том же архиве комплекта.
    pub fn llama_image(&self) -> String {
        "oktaplus/dev1c-llama:latest".into()
    }

    pub fn llama_container(&self) -> String {
        format!("{}-llama", self.server.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_parses_and_equals_defaults() {
        let c: Cfg = toml::from_str(EXAMPLE).expect("example toml parses");
        assert_eq!(c, Cfg::default(), "dev1c.example.toml and Cfg::default drifted apart");
    }

    #[test]
    fn ports_follow_the_first_one() {
        let mut c = Cfg::default();
        c.server.port = 4000;
        assert_eq!(c.ports(), [4000, 4001, 4002, 4003]);
    }

    #[test]
    fn remote_model_gives_custom_variables_and_no_llama() {
        let mut c = Cfg::default();
        c.model.serve = "remote".into();
        c.model.url = "https://m.example.com/v1".into();
        c.model.key = "k".into();
        c.model.name = "Q".into();
        c.validate().unwrap();
        let env = c.container_env(None);
        assert!(env.contains(&("CUSTOM_API_URL".into(), "https://m.example.com/v1".into())));
        assert!(env.contains(&("CUSTOM_API_KEY".into(), "k".into())));
        assert!(env.contains(&("CUSTOM_MODEL".into(), "Q".into())));
        assert!(env.iter().all(|(k, _)| k != "LLAMA_ENABLED"), "внутренней лламы больше нет");
    }

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
        c.model.file = if cfg!(windows) { r"C:\models\x.gguf".into() } else { "/models/x.gguf".into() };
        // До замера фактов нет: контейнеру нечего сказать про модель.
        let env = c.container_env(None);
        assert!(env.is_empty(), "{env:?}");
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

    #[test]
    fn both_file_and_url_is_refused_with_the_field_named() {
        let mut c = Cfg::default();
        c.model.file = "model/x.gguf".into();
        c.model.url = "https://m".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("[model]"), "{e}");
    }

    #[test]
    fn url_without_key_is_allowed_and_sends_no_key() {
        // A server inside the network (llama-server, vLLM) has no key at all.
        let mut c = Cfg::default();
        c.model.serve = "remote".into();
        c.model.url = "https://m".into();
        c.model.name = "Q".into();
        c.validate().unwrap();
        let env = c.container_env(None);
        assert!(env.iter().all(|(k, _)| k != "CUSTOM_API_KEY"));
        assert!(env.contains(&("CUSTOM_MODEL".into(), "Q".into())));
    }

    #[test]
    fn change_keeps_the_comments() {
        let mut c = Cfg::default();
        assert_eq!(Cfg::render(&c), EXAMPLE, "defaults render to the example itself");
        c.server.port = 4000;
        c.model.serve = "remote".into();
        c.model.url = "https://m".into();
        c.model.name = "Q".into();
        c.model.agents = 3;
        let t = Cfg::render(&c);
        assert!(t.contains("port = 4000"));
        assert!(t.contains("serve = \"remote\""));
        assert!(t.contains("url = \"https://m\""));
        assert!(t.contains("agents = 3"));
        // `name` exists in both sections: only [model] name changes.
        assert!(t.contains("name = \"dev1c\"") && t.contains("name = \"Q\""));
        assert!(t.contains("# Первый из четырёх портов подряд"));
        let back: Cfg = toml::from_str(&t).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn the_sample_can_be_printed_for_any_form() {
        for form in ["host", "docker", "remote"] {
            let text = Cfg::example_for(form);
            assert!(text.contains(&format!("serve = \"{form}\"")));
            // Комментарии образца на месте: файл остаётся человеческим.
            assert!(text.contains('#'));
            // И он читается обратно без потерь.
            let back: Cfg = toml::from_str(&text).unwrap();
            assert_eq!(back.model.serve, form);
        }
    }

    #[test]
    fn url_without_name_is_refused() {
        let mut c = Cfg::default();
        c.model.serve = "remote".into();
        c.model.url = "https://m".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("name"), "{e}");
    }

    #[test]
    fn empty_model_is_allowed() {
        // Пустая модель — допустимо (файл зальют позже), агент просто молчит.
        assert!(Cfg::default().validate().is_ok());
    }

    #[test]
    fn volumes_use_the_container_name() {
        let c = Cfg::default();
        assert_eq!(c.volume("home"), "dev1c-home");
    }
}
