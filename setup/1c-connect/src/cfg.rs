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
    /// Адрес Gitea среды целиком; пусто — прежний вывод от `server` («порт
    /// SSH минус два», см. `Cfg::gitea_url`). Поле нужно локальному режиму:
    /// туннеля там нет, пробрасывать некому, и выведенный адрес указывает в
    /// пустоту — настоящий адрес Gitea образа пишет в toml сам стенд.
    pub gitea: String,
    pub password: String,
    pub workstation: String,
    pub base_path: String,
    pub db_user: String,
    pub db_password: String,
    pub base_url: String,
    pub dump: String,
    pub project: String,
    pub tools: Vec<String>,
    /// Режим работы: `tunnel` — база на этой машине и туннель в среду,
    /// `local` — база внутри образа. Пока только читается и сохраняется;
    /// переключатель и поведение — спека окна, раздел «Режим работы».
    pub mode: String,
    /// Кто публикует базу: `auto` — сама программа своим httpd (адрес считает
    /// `Cfg::base_url`), `webserver` — уже опубликовано вручную на Apache/IIS
    /// (адрес берётся из `base_url`).
    pub publication: String,
    /// Порт своего httpd в режиме `auto`; `None` — `8080 + N` (номер рабочего
    /// места). В toml хранится как число, `0` значит «не задан».
    #[serde(deserialize_with = "http_port_de", skip_serializing_if = "Option::is_none")]
    pub http_port: Option<u16>,
}

/// `http_port`: `0` в toml — «не задан» (`None`), иначе порт как есть.
fn http_port_de<'de, D>(d: D) -> std::result::Result<Option<u16>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let n = u16::deserialize(d)?;
    Ok((n != 0).then_some(n))
}

impl Default for Host {
    fn default() -> Self {
        Self {
            server: "192.168.1.10:3393".into(),
            gitea: String::new(),
            password: "dev1".into(),
            workstation: "dev1".into(),
            base_path: "C:\\Bases\\Buh".into(),
            db_user: "Администратор".into(),
            db_password: String::new(),
            base_url: "http://localhost/Buh/hs/mcp-1c".into(),
            dump: String::new(),
            project: "C:\\proj\\Buh".into(),
            tools: DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect(),
            mode: "tunnel".into(),
            publication: "auto".into(),
            http_port: None,
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
        // На Windows это тот же путь, что и раньше (toml при комплекте); на
        // Linux — состояние пользователя, не общий только-для-чтения
        // комплект (см. `ops_host::wire_state`, задача «комплект в образе»).
        let p = kit.state_path(crate::cfg::FILE);
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
        let g = self.host.gitea.trim();
        anyhow::ensure!(
            g.is_empty() || g.starts_with("http://") || g.starts_with("https://"),
            "[host]: gitea — полный адрес Gitea среды, например http://localhost:3000 (пусто — считается от server)"
        );
        anyhow::ensure!(matches!(self.host.mode.as_str(), "tunnel" | "local"), "[host]: mode — \"tunnel\" (туннель в среду) или \"local\" (база внутри образа)");
        anyhow::ensure!(
            matches!(self.host.publication.as_str(), "auto" | "webserver"),
            "[host]: publication — \"auto\" (публикует сама программа) или \"webserver\" (уже опубликовано на Apache/IIS)"
        );
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

    /// Gitea среды: поле `gitea`, если непусто; иначе на два порта ниже SSH
    /// (блок из четырёх подряд у dev1c-setup). Вывод верен для туннеля —
    /// там Gitea среды приезжает на машину проброшенным портом; в локальном
    /// режиме адрес задаёт поле.
    pub fn gitea_url(&self) -> String {
        let explicit = self.host.gitea.trim().trim_end_matches('/');
        if !explicit.is_empty() {
            return explicit.to_string();
        }
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

    /// Порт веб-сервера из действующего адреса публикации (`base_url()`, не
    /// хранимое поле `host.base_url`); 80, если не указан. В режиме `auto`
    /// действующий адрес считается от `http_port()` — читать хранимое поле
    /// напрямую означало форвардить веб-канал туннеля не туда (шестой
    /// читатель адреса публикации из спеки — см. тест ниже).
    pub fn web_port(&self) -> u16 {
        let url = self.base_url();
        let rest = url.trim_start_matches("http://");
        let hostport = rest.split('/').next().unwrap_or("");
        hostport.split_once(':').and_then(|(_, p)| p.parse().ok()).unwrap_or(80)
    }

    /// Порт своего httpd в режиме `auto`: `http_port`, иначе `8080 + N`.
    pub fn http_port(&self) -> u16 {
        self.host.http_port.unwrap_or(8080 + self.number().unwrap_or(1))
    }

    /// Адрес публикации: в режиме `auto` считает от своего httpd (`http_port`),
    /// в `webserver` отдаёт хранимое поле `base_url` как есть.
    pub fn base_url(&self) -> String {
        if self.host.publication == "auto" {
            format!("http://localhost:{}/hs/mcp-1c", self.http_port())
        } else {
            self.host.base_url.clone()
        }
    }

    pub fn render(&self) -> String {
        let h = &self.host;
        common::toml_render::render(EXAMPLE, |s, k| {
            if s != "[host]" {
                return None;
            }
            Some(match k {
                "server" => format!("{:?}", h.server),
                "gitea" => format!("{:?}", h.gitea),
                "password" => format!("{:?}", h.password),
                "workstation" => format!("{:?}", h.workstation),
                "base_path" => format!("{:?}", h.base_path),
                "db_user" => format!("{:?}", h.db_user),
                "db_password" => format!("{:?}", h.db_password),
                "base_url" => format!("{:?}", h.base_url),
                "dump" => format!("{:?}", h.dump),
                "project" => format!("{:?}", h.project),
                "tools" => format!("[{}]", h.tools.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>().join(", ")),
                "mode" => format!("{:?}", h.mode),
                "publication" => format!("{:?}", h.publication),
                "http_port" => h.http_port.unwrap_or(0).to_string(),
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
    fn mode_defaults_to_tunnel_accepts_local_and_rejects_the_rest() {
        let without: Cfg = toml::from_str(&EXAMPLE.replace("mode = \"tunnel\"", "")).unwrap();
        assert_eq!(without.host.mode, "tunnel");
        let local: Cfg = toml::from_str(&EXAMPLE.replace("mode = \"tunnel\"", "mode = \"local\"")).unwrap();
        assert_eq!(local.host.mode, "local");
        local.validate().unwrap();
        assert!(local.render().contains("mode = \"local\""));
        let mut odd = Cfg::default();
        odd.host.mode = "remote".into();
        assert!(odd.validate().unwrap_err().to_string().contains("mode"));
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
    fn web_port_follows_cfg_base_url_not_the_stored_field() {
        // Регрессия: web_port() раньше читал host.base_url напрямую. В режиме
        // "auto" действующий адрес (Cfg::base_url()) считается от http_port()
        // и у него другой порт, чем у хранимого поля (там по умолчанию 80) —
        // веб-канал туннеля форвардил не туда.
        let mut c = sample();
        assert_eq!(c.host.publication, "auto");
        assert_eq!(c.web_port(), 8081); // http_port() для dev1, не 80 из host.base_url
        c.host.publication = "webserver".into();
        assert_eq!(c.web_port(), 80); // webserver: читаем хранимое поле как есть
        c.host.base_url = "http://localhost:8081/InfoBase/hs/mcp-1c".into();
        assert_eq!(c.web_port(), 8081);
        c.host.base_url = "http://other-host/Buh/hs/mcp-1c".into();
        assert!(c.validate().unwrap_err().to_string().contains("localhost"));
    }

    #[test]
    fn auto_computes_the_base_url() {
        let mut c = sample();
        assert_eq!(c.host.publication, "auto");
        c.host.workstation = "dev2".into();
        assert_eq!(c.base_url(), "http://localhost:8082/hs/mcp-1c");
        assert_eq!(c.host.base_url, Host::default().base_url);
    }

    #[test]
    fn webserver_keeps_the_field() {
        let mut c = sample();
        c.host.publication = "webserver".into();
        assert_eq!(c.base_url(), c.host.base_url);
    }

    #[test]
    fn an_explicit_port_wins() {
        let mut c = sample();
        c.host.http_port = Some(9000);
        assert_eq!(c.base_url(), "http://localhost:9000/hs/mcp-1c");
    }

    #[test]
    fn validate_rejects_an_unknown_publication() {
        let mut c = sample();
        c.host.publication = "iis".into();
        assert!(c.validate().unwrap_err().to_string().contains("publication"));
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

    /// Заполненное поле — и есть адрес среды; вывод от порта SSH больше не
    /// участвует (в локальном режиме он указывает в пустоту).
    #[test]
    fn an_explicit_gitea_wins_over_the_derived_one() {
        let mut c = sample();
        c.host.gitea = "http://localhost:3000".into();
        assert_eq!(c.gitea_url(), "http://localhost:3000");
        c.host.server = "10.0.0.5:4403".into();
        assert_eq!(c.gitea_url(), "http://localhost:3000");
        // Хвостовой слэш не рождает двойного в адресе репозитория.
        c.host.gitea = "http://localhost:3000/".into();
        assert_eq!(c.gitea_url(), "http://localhost:3000");
        c.host.gitea = "localhost:3000".into();
        assert!(c.validate().unwrap_err().to_string().contains("gitea"));
    }

    /// Пусто — прежний вывод, так что ни одно существующее туннельное
    /// рабочее место ничего не замечает; и toml без ключа читается.
    #[test]
    fn an_empty_gitea_keeps_the_old_derivation() {
        let without: Cfg = toml::from_str(&EXAMPLE.replace("gitea = \"\"", "")).unwrap();
        assert!(without.host.gitea.is_empty());
        assert_eq!(without.gitea_url(), "http://192.168.1.10:3391");
        assert_eq!(sample().gitea_url(), "http://192.168.1.10:3391");
        let mut c = sample();
        c.host.gitea = "   ".into();
        assert_eq!(c.gitea_url(), "http://192.168.1.10:3391");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn example_prints_gitea_with_its_rule() {
        let lines: Vec<&str> = EXAMPLE.lines().collect();
        let i = lines.iter().position(|l| l.starts_with("gitea = ")).unwrap_or_else(|| panic!("в образце нет поля gitea"));
        assert_eq!(lines[i], "gitea = \"\"", "в образце поле пустое — иначе туннельное рабочее место заметило бы правку");
        let rule: String = lines[..i].iter().rev().take_while(|l| l.starts_with('#')).fold(String::new(), |a, l| format!("{l}\n{a}"));
        assert!(rule.contains("Пусто") && rule.contains("на два ниже"), "правило вывода не напечатано: {rule}");
        // Заполненное поле доходит до toml через render() и читается обратно.
        let filled = EXAMPLE.replace("gitea = \"\"", "gitea = \"http://localhost:3000\"");
        let c: Cfg = toml::from_str(&filled).unwrap();
        assert_eq!(c.render(), filled);
        assert_eq!(c.gitea_url(), "http://localhost:3000");
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
