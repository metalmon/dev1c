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
    pub file: String,
    pub url: String,
    pub key: String,
    pub name: String,
}

impl Default for Server {
    fn default() -> Self {
        Self { name: "dev1c".into(), port: 3390, memory: "24g".into(), gpu: true, engine: "docker".into() }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self { file: String::new(), url: String::new(), key: String::new(), name: String::new() }
    }
}

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

    /// Ошибка называет поле и что с ним делать.
    pub fn validate(&self) -> Result<()> {
        let m = &self.model;
        anyhow::ensure!(
            m.file.is_empty() || m.url.is_empty(),
            "[model]: задано и file, и url — оставьте одно: файл в комплекте или удалённый сервер"
        );
        if !m.url.is_empty() {
            anyhow::ensure!(!m.name.is_empty(), "[model]: для удалённого сервера нужно name — имя модели, как сервер его называет");
            // key may be empty: a server inside the network often has none.
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

    /// Что уходит в контейнер через -e. Локальная модель не требует ничего:
    /// entrypoint находит файл в томе сам.
    pub fn container_env(&self) -> Vec<(String, String)> {
        let m = &self.model;
        if m.url.is_empty() {
            return Vec::new();
        }
        let mut env = vec![("CUSTOM_API_URL".to_string(), m.url.clone())];
        if !m.key.is_empty() {
            env.push(("CUSTOM_API_KEY".into(), m.key.clone()));
        }
        env.push(("CUSTOM_MODEL".into(), m.name.clone()));
        env.push(("LLAMA_ENABLED".into(), "0".into()));
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
                ("[model]", "file") => format!("{:?}", cfg.model.file),
                ("[model]", "url") => format!("{:?}", cfg.model.url),
                ("[model]", "key") => format!("{:?}", cfg.model.key),
                ("[model]", "name") => format!("{:?}", cfg.model.name),
                _ => return None,
            })
        })
    }

    pub fn volume(&self, suffix: &str) -> String {
        format!("{}-{}", self.server.name, suffix)
    }

    pub fn image(&self) -> String {
        "oktaplus/dev1c:latest".into()
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
        c.model.url = "https://m.example.com/v1".into();
        c.model.key = "k".into();
        c.model.name = "Q".into();
        c.validate().unwrap();
        let env = c.container_env();
        assert!(env.contains(&("CUSTOM_API_URL".into(), "https://m.example.com/v1".into())));
        assert!(env.contains(&("CUSTOM_API_KEY".into(), "k".into())));
        assert!(env.contains(&("CUSTOM_MODEL".into(), "Q".into())));
        assert!(env.contains(&("LLAMA_ENABLED".into(), "0".into())));
    }

    #[test]
    fn local_model_gives_no_variables() {
        let mut c = Cfg::default();
        c.model.file = "model/x.gguf".into();
        c.validate().unwrap();
        assert!(c.container_env().is_empty());
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
        c.model.url = "https://m".into();
        c.model.name = "Q".into();
        c.validate().unwrap();
        let env = c.container_env();
        assert!(env.iter().all(|(k, _)| k != "CUSTOM_API_KEY"));
        assert!(env.contains(&("CUSTOM_MODEL".into(), "Q".into())));
    }

    #[test]
    fn change_keeps_the_comments() {
        let mut c = Cfg::default();
        assert_eq!(Cfg::render(&c), EXAMPLE, "defaults render to the example itself");
        c.server.port = 4000;
        c.model.url = "https://m".into();
        c.model.name = "Q".into();
        let t = Cfg::render(&c);
        assert!(t.contains("port = 4000"));
        assert!(t.contains("url = \"https://m\""));
        // `name` exists in both sections: only [model] name changes.
        assert!(t.contains("name = \"dev1c\"") && t.contains("name = \"Q\""));
        assert!(t.contains("# Первый из четырёх портов подряд"));
        let back: Cfg = toml::from_str(&t).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn url_without_name_is_refused() {
        let mut c = Cfg::default();
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
