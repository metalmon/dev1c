//! Параметры разработчика: `1c-connect.toml` на флешке. Сервер про базу не
//! знает ничего; всё здесь.

use anyhow::{Context, Result};
use common::kit::Kit;
use serde::{Deserialize, Serialize};

pub const EXAMPLE: &str = include_str!("../1c-connect.example.toml");
pub const FILE: &str = "1c-connect.toml";

/// Инструменты vrunner, видимые агенту по умолчанию: выгрузка/загрузка,
/// синтакс-контроль, тесты, фоновые задачи, кэш. Остальные 49 — по списку
/// в toml.
pub const DEFAULT_TOOLS: [&str; 12] = [
    "cf_decompile", "cf_load", "infobase_update", "validate_syntax_check", "test_xunit", "test_vanessa",
    "test_yaxunit", "task_status", "task_result", "task_cancel", "cache_show", "cache_clear",
];

/// Адрес публикации по умолчанию: имя последней папки `base_path`. Поля в
/// окне нет — адрес следует за каталогом базы, а правку в toml переносит
/// `gui::carried_base_url`. Имя, которое `validate` не пропустит (пусто,
/// «C:», кириллица), заменяется на `Buh` из образца.
pub fn derived_base_url(base_path: &str) -> String {
    let name = base_path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or("");
    let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c));
    format!("http://localhost/{}/hs/mcp-1c", if ok { name } else { "Buh" })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cfg {
    pub host: Host,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Host {
    pub server: String,
    pub password: String,
    pub workstation: String,
    pub base_path: String,
    pub db_user: String,
    pub db_password: String,
    pub base_url: String,
    pub dump: String,
    pub project: String,
    pub tools: Vec<String>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            server: "192.168.1.10:3393".into(),
            password: "dev1".into(),
            workstation: "dev1".into(),
            base_path: "C:\\Bases\\Buh".into(),
            db_user: "Администратор".into(),
            db_password: String::new(),
            base_url: "http://localhost/Buh/hs/mcp-1c".into(),
            dump: String::new(),
            project: "C:\\proj\\Buh".into(),
            tools: DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Default for Cfg {
    fn default() -> Self {
        Self { host: Host::default() }
    }
}

pub struct Ports {
    pub mcp: u16,
    pub vrunner: u16,
    pub web: u16,
}

impl Cfg {
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

    pub fn number(&self) -> Option<u16> {
        self.host.workstation.strip_prefix("dev")?.parse::<u16>().ok().filter(|n| (1..=3).contains(n))
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.number().is_some(), "[host]: workstation — dev1, dev2 или dev3 (ваш логин в среде)");
        anyhow::ensure!(self.host.server.contains(':'), "[host]: server — адрес и порт через двоеточие, например 192.168.1.10:3393");
        let u = &self.host.base_url;
        anyhow::ensure!(
            u.starts_with("http://localhost") || u.starts_with("http://127.0.0.1"),
            "[host]: base_url — адрес публикации на ЭТОЙ машине, начинается с http://localhost"
        );
        anyhow::ensure!(u.contains("/hs/mcp-1c"), "[host]: base_url должен заканчиваться на /hs/mcp-1c");
        anyhow::ensure!(
            u.chars().all(|c| c.is_ascii_alphanumeric() || "-._~:/".contains(c)),
            "[host]: base_url — только латиница, цифры и -._~:/ (путь публикации без пробелов и спецсимволов)"
        );
        anyhow::ensure!(!self.host.base_path.is_empty(), "[host]: base_path — путь к базе или сервер\\база");
        let p = &self.host.project;
        let full = p.len() > 2 && (p.as_bytes()[1] == b':' || p.starts_with('/') || p.starts_with("\\\\"));
        anyhow::ensure!(full && !p.contains('"') && !p.contains('%'), "[host]: project — полный путь к каталогу проекта без кавычек и %, например C:\\proj\\Buh");
        Ok(())
    }

    /// Gitea среды: на два порта ниже SSH (блок из четырёх подряд у dev1c-setup).
    pub fn gitea_url(&self) -> String {
        let (host, port) = self.host.server.rsplit_once(':').unwrap_or((&self.host.server, "3393"));
        let port: u32 = port.parse().unwrap_or(3393);
        format!("http://{host}:{}", port.saturating_sub(2))
    }

    /// Каталог выгрузки для поиска mcp-1c: `dump`, а пусто — `<project>/src`.
    pub fn dump_dir(&self) -> String {
        if self.host.dump.is_empty() {
            format!("{}{}src", self.host.project.trim_end_matches(['\\', '/']), std::path::MAIN_SEPARATOR)
        } else {
            self.host.dump.clone()
        }
    }

    pub fn ports(&self) -> Ports {
        let n = self.number().unwrap_or(1);
        Ports { mcp: 7770 + n, vrunner: 7780 + n, web: 7790 + n }
    }

    /// Порт веб-сервера из base_url; 80, если не указан.
    pub fn web_port(&self) -> u16 {
        let rest = self.host.base_url.trim_start_matches("http://");
        let hostport = rest.split('/').next().unwrap_or("");
        hostport.split_once(':').and_then(|(_, p)| p.parse().ok()).unwrap_or(80)
    }

    /// Путь публикации с косыми чертами: `/Buh/` из `http://localhost/Buh/hs/mcp-1c`.
    pub fn publication(&self) -> String {
        let rest = self.host.base_url.trim_start_matches("http://");
        let path = &rest[rest.find('/').unwrap_or(rest.len())..];
        let before_hs = path.split("/hs/").next().unwrap_or("/");
        format!("{}/", before_hs.trim_end_matches('/'))
    }

    pub fn render(&self) -> String {
        let h = &self.host;
        common::toml_render::render(EXAMPLE, |s, k| {
            if s != "[host]" {
                return None;
            }
            Some(match k {
                "server" => format!("{:?}", h.server),
                "password" => format!("{:?}", h.password),
                "workstation" => format!("{:?}", h.workstation),
                "base_path" => format!("{:?}", h.base_path),
                "db_user" => format!("{:?}", h.db_user),
                "db_password" => format!("{:?}", h.db_password),
                "base_url" => format!("{:?}", h.base_url),
                "dump" => format!("{:?}", h.dump),
                "project" => format!("{:?}", h.project),
                "tools" => format!("[{}]", h.tools.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>().join(", ")),
                _ => return None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Cfg {
        toml::from_str(EXAMPLE).unwrap()
    }

    #[test]
    fn example_parses_and_equals_defaults() {
        assert_eq!(sample(), Cfg::default());
    }

    #[test]
    fn ports_follow_the_workstation() {
        let mut c = sample();
        assert_eq!((c.ports().mcp, c.ports().vrunner, c.ports().web), (7771, 7781, 7791));
        c.host.workstation = "dev3".into();
        assert_eq!(c.ports().web, 7793);
        c.host.workstation = "dev4".into();
        assert!(c.validate().unwrap_err().to_string().contains("workstation"));
    }

    #[test]
    fn web_port_and_publication_come_from_the_url() {
        let mut c = sample();
        assert_eq!(c.web_port(), 80);
        assert_eq!(c.publication(), "/Buh/");
        c.host.base_url = "http://localhost:8081/InfoBase/hs/mcp-1c".into();
        assert_eq!(c.web_port(), 8081);
        assert_eq!(c.publication(), "/InfoBase/");
        c.host.base_url = "http://other-host/Buh/hs/mcp-1c".into();
        assert!(c.validate().unwrap_err().to_string().contains("localhost"));
    }

    #[test]
    fn derived_base_url_follows_the_base_folder() {
        assert_eq!(derived_base_url("C:\\Bases\\Buh"), "http://localhost/Buh/hs/mcp-1c");
        assert_eq!(derived_base_url("C:\\Bases\\Buh\\"), "http://localhost/Buh/hs/mcp-1c");
        assert_eq!(derived_base_url(""), "http://localhost/Buh/hs/mcp-1c");
        // Имя папки, которое `validate` не пропустит, — то же умолчание.
        assert_eq!(derived_base_url("C:\\"), "http://localhost/Buh/hs/mcp-1c");
        assert_eq!(derived_base_url("C:\\Базы\\Бух"), "http://localhost/Buh/hs/mcp-1c");
    }

    #[test]
    fn gitea_sits_two_ports_below_ssh() {
        let mut c = sample();
        assert_eq!(c.gitea_url(), "http://192.168.1.10:3391");
        c.host.server = "10.0.0.5:4403".into();
        assert_eq!(c.gitea_url(), "http://10.0.0.5:4401");
    }

    #[test]
    fn dump_defaults_to_src_under_the_project() {
        let mut c = sample();
        c.host.project = "C:\\proj\\Buh".into();
        assert_eq!(c.dump_dir(), format!("C:\\proj\\Buh{}src", std::path::MAIN_SEPARATOR));
        c.host.dump = "D:\\dump".into();
        assert_eq!(c.dump_dir(), "D:\\dump");
    }

    #[test]
    fn project_must_be_a_full_path_without_quotes_or_percent() {
        let mut c = sample();
        c.host.project = "proj".into();
        assert!(c.validate().unwrap_err().to_string().contains("project"));
        c.host.project = "C:\\proj\\\"x\"".into();
        assert!(c.validate().unwrap_err().to_string().contains("project"));
        c.host.project = "C:\\proj\\Buh".into();
        assert!(c.validate().is_ok());
        c.host.project = "/home/dev/proj".into();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn tools_default_to_the_working_set() {
        let c = sample();
        assert_eq!(c.host.tools, DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(c.host.tools.contains(&"cf_load".to_string()) && !c.host.tools.contains(&"xunit".to_string()));
    }

    #[test]
    fn password_is_the_workstation_login_by_default() {
        assert_eq!(Host::default().password, "dev1");
    }

    #[test]
    fn render_keeps_comments_and_round_trips() {
        let mut c = sample();
        assert_eq!(c.render(), EXAMPLE);
        c.host.workstation = "dev2".into();
        let t = c.render();
        assert!(t.contains("workstation = \"dev2\"") && t.contains("# Ваш логин в среде"));
        assert_eq!(toml::from_str::<Cfg>(&t).unwrap(), c);
    }
}
