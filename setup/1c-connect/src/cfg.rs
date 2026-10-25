//! Параметры разработчика: `1c-connect.toml` на флешке. Сервер про базу не
//! знает ничего; всё здесь.

use anyhow::{Context, Result};
use common::kit::Kit;
use serde::{Deserialize, Serialize};

pub const EXAMPLE: &str = include_str!("../1c-connect.example.toml");
pub const FILE: &str = "1c-connect.toml";

/// Адреса среды в toml нет — и выдумывать его программа не станет. Одна
/// формулировка на всех, кто про это говорит: факт «Gitea», факт «Каталог
/// проекта», операция `host.project`, раскладка проекта и вкладка «Проект» —
/// одна пустая строка в файле не должна называться разными словами (находка
/// ревью задачи 35).
pub const NO_GITEA: &str = "адрес среды не задан: поле gitea в 1c-connect.toml пусто";

/// Как это исправить — тем же текстом у всех.
pub const NO_GITEA_FIX: &str = "нажмите «Установить» в окне или выполните 1c-connect install; либо задайте gitea в 1c-connect.toml";

/// Обе половины одной строкой: где отдельного места для подсказки нет
/// (`OpState::Blocked`, отказ инструмента).
pub fn no_gitea() -> String {
    format!("{NO_GITEA} — {NO_GITEA_FIX}")
}

/// Инструменты vrunner, видимые агенту по умолчанию: выгрузка/загрузка,
/// синтакс-контроль, тесты, фоновые задачи, кэш. Остальные 49 — по списку
/// в toml.
pub const DEFAULT_TOOLS: [&str; 12] = [
    "cf_decompile", "cf_load", "infobase_update", "validate_syntax_check", "test_xunit", "test_vanessa",
    "test_yaxunit", "task_status", "task_result", "task_cancel", "cache_show", "cache_clear",
];

/// Инструменты, которые **меняют состояние**: базу, дерево проекта или
/// среду. Список нужен не самому коннектору, а согласованию с профилями
/// агента в Zed (`config/zed-settings.json`): на шаге «разбор» агент
/// собирает факты и ничего не менять не обещает — он просто не должен иметь
/// таких инструментов. У профилей `enable_all_context_servers: true`, то есть
/// новый инструмент моста приходит агенту **включённым**, и забыть погасить
/// его можно молча. Тест рядом с этим списком превращает «не забыть» в
/// падение.
///
/// Чтение (`validate_syntax_check`, `task_status`, `task_result`,
/// `cache_show`, `project_status`) сюда не входит: разбору оно и нужно.
pub const CHANGES_STATE: [&str; 10] = [
    // Перезаписывает `src/` — десятки тысяч файлов и минуты работы.
    "cf_decompile",
    // Грузит дерево в базу и обновляет её.
    "cf_load",
    "infobase_update",
    // Прогон кода по базе: данные после тестов не те, что до.
    "test_xunit",
    "test_vanessa",
    "test_yaxunit",
    // Снимает уже идущую задачу — чужую работу.
    "task_cancel",
    // Удаляет кеш vanessa-runner.
    "cache_clear",
    // Пишет в дерево и в Gitea (`jsonrpc::OUR_TOOLS`).
    "project_pull",
    "project_push",
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

/// Номер рабочего места: `dev1`, `dev2` или `dev3`. От него считаются порты.
pub fn number_of(workstation: &str) -> Option<u16> {
    workstation.strip_prefix("dev")?.parse::<u16>().ok().filter(|n| (1..=3).contains(n))
}

/// Порт своего httpd по умолчанию: `8080 + N` рабочего места. Поля в окне
/// нет — умолчание идёт за рабочим местом, а правку переносит
/// `gui::carried_http_port`.
pub fn derived_http_port(workstation: &str) -> u16 {
    8080 + number_of(workstation).unwrap_or(1)
}

/// Каталог выгрузки по умолчанию: `<project>/src`. Поля в окне нет —
/// умолчание идёт за каталогом проекта, а правку переносит
/// `gui::carried_dump` (тот же приём, что у адреса публикации).
pub fn derived_dump(project: &str) -> String {
    format!("{}{}src", project.trim_end_matches(['\\', '/']), std::path::MAIN_SEPARATOR)
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
    /// Адрес Gitea среды целиком. Пусто — адрес не задан, и программа его не
    /// выдумывает: вывод «порт SSH минус два» верен только в туннеле, а в
    /// образе давал мёртвый адрес, о котором человек читал «Gitea не
    /// отвечает». Поле записывают один раз при настройке, и только там, где
    /// вывод верен: `1c-connect install` в туннельном режиме (из `server`,
    /// см. `ops_host::gitea_to_write`) либо `seed` стенда (настоящий адрес
    /// Gitea образа). В локальном режиме пусто так и остаётся, пока человек
    /// не задаст адрес сам.
    pub gitea: String,
    pub password: String,
    pub workstation: String,
    pub base_path: String,
    /// Версия платформы 1С или её маска (`8.3`), которой работать. Пусто —
    /// версию выбирает лестница `platform::resolve` (пин базы, общий пин,
    /// самая новая). Названная, но не установленная версия — отказ со
    /// словами, а не молчаливый откат на новейшую.
    #[serde(default)]
    pub platform: String,
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
            platform: String::new(),
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
        number_of(&self.host.workstation)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.number().is_some(), "[host]: workstation — dev1, dev2 или dev3 (ваш логин в среде)");
        anyhow::ensure!(self.host.server.contains(':'), "[host]: server — адрес и порт через двоеточие, например 192.168.1.10:3393");
        let g = self.host.gitea.trim();
        anyhow::ensure!(
            g.is_empty() || g.starts_with("http://") || g.starts_with("https://"),
            "[host]: gitea — полный адрес Gitea среды, например http://localhost:3000 (пусто — адрес не задан, его пишет 1c-connect install)"
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

    /// Адрес Gitea среды — только то, что написано в поле `gitea`; пусто —
    /// `None`. Вывода здесь нет: адрес смотрит наружу этой машины, и
    /// ошибиться в нём легко, поэтому его выводят один раз при настройке
    /// (`ops_host::derived_gitea`) и записывают в toml.
    pub fn gitea_url(&self) -> Option<&str> {
        let explicit = self.host.gitea.trim().trim_end_matches('/');
        (!explicit.is_empty()).then_some(explicit)
    }

    /// Свой каталог выгрузки для `mcp-1c` — только когда человек выбрал его
    /// сам: поле непусто и отличается от выведенного `<project>/src`.
    /// Записанное настройкой умолчание ключ `--dump` не включает: иначе наша
    /// запись молча сменила бы агенту корень поиска по коду на уже
    /// настроенной машине, где поле было пустым (находка ревью задачи 35).
    pub fn dump_override(&self) -> Option<&str> {
        let d = self.host.dump.trim();
        (!d.is_empty() && d != derived_dump(&self.host.project)).then_some(d)
    }

    /// Каталог выгрузки для поиска mcp-1c: `dump`, а пусто — `<project>/src`.
    pub fn dump_dir(&self) -> String {
        if self.host.dump.is_empty() {
            derived_dump(&self.host.project)
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
        self.host.http_port.unwrap_or_else(|| derived_http_port(&self.host.workstation))
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
                "platform" => format!("{:?}", h.platform),
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

    /// Настройки Zed из образа — рядом, не копией: имена инструментов
    /// согласованы между нашим списком и профилями агента, а разойтись им
    /// нельзя молча (просьба соседней сессии 2026-10-21).
    const ZED: &str = include_str!("../../../config/zed-settings.json");

    /// Настройки Zed — не JSON, а **шаблон** JSON с комментариями: entrypoint
    /// подставляет в него значения (`CUSTOM_MAX_TOKENS_PLACEHOLDER` стоит без
    /// кавычек, то есть на месте числа). Чтобы прочитать его разбором, а не
    /// глазами, приводим к JSON: срезаем `//` **вне строк** (в файле есть
    /// адреса `http://localhost:3000`, и наивная резка сломала бы их) и
    /// заменяем голые подстановки на 0 — нас интересуют профили, а не числа.
    fn as_json(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            let chars: Vec<char> = line.chars().collect();
            let (mut in_string, mut escaped, mut i) = (false, false, 0);
            while i < chars.len() {
                let c = chars[i];
                if escaped {
                    escaped = false;
                } else if in_string && c == '\\' {
                    escaped = true;
                } else if c == '\"' {
                    in_string = !in_string;
                } else if !in_string {
                    if c == '/' && chars.get(i + 1) == Some(&'/') {
                        break; // строка комментария — остаток не нужен
                    }
                    if c.is_ascii_uppercase() {
                        let run: String = chars[i..].iter().take_while(|c| c.is_ascii_uppercase() || **c == '_' || c.is_ascii_digit()).collect();
                        if run.ends_with("PLACEHOLDER") {
                            out.push('0');
                            i += run.chars().count();
                            continue;
                        }
                    }
                }
                out.push(c);
                i += 1;
            }
            out.push('\n');
        }
        out
    }

    fn zed_json() -> serde_json::Value {
        serde_json::from_str(&as_json(ZED)).expect("config/zed-settings.json разбирается как JSON после снятия комментариев")
    }

    /// Инструменты сервера `vrunner`, перечисленные в профиле: имя → включён.
    fn profile_tools(profile: &str) -> serde_json::Map<String, serde_json::Value> {
        zed_json()["agent"]["profiles"][profile]["context_servers"]["vrunner"]["tools"]
            .as_object()
            .unwrap_or_else(|| panic!("в профиле {profile} нет инструментов сервера vrunner"))
            .clone()
    }

    fn exposable() -> Vec<&'static str> {
        DEFAULT_TOOLS.iter().copied().chain(crate::jsonrpc::OUR_TOOLS).collect()
    }

    /// Имя, которого мост не отдаёт, в профиле ничего не гасит — и молчит об
    /// этом. Ловит переименование и удаление инструмента у нас.
    #[test]
    fn every_name_in_the_zed_profiles_is_a_tool_we_actually_expose() {
        let known = exposable();
        for profile in ["разбор", "работа"] {
            for name in profile_tools(profile).keys() {
                assert!(known.contains(&name.as_str()), "профиль «{profile}» гасит {name}, которого мост не отдаёт: ни в DEFAULT_TOOLS, ни в OUR_TOOLS");
            }
        }
    }

    /// Обратная сторона, и она важнее: у профилей
    /// `enable_all_context_servers: true`, то есть новый инструмент приходит
    /// агенту включённым. На шаге «разбор» меняющих состояние быть не должно.
    #[test]
    fn the_review_profile_mutes_every_tool_that_changes_state() {
        let tools = profile_tools("разбор");
        // Собираем все недостачи, а не первую: иначе правка файла идёт по
        // одному падению за прогон.
        let gaps: Vec<String> = CHANGES_STATE
            .iter()
            .filter_map(|name| match tools.get(*name) {
                None => Some(format!("{name}: записи нет, значит инструмент включён")),
                Some(serde_json::Value::Bool(false)) => None,
                Some(v) => Some(format!("{name}: {v}, а нужно false")),
            })
            .collect();
        assert!(gaps.is_empty(), "профиль «разбор» в config/zed-settings.json не гасит меняющие состояние:\n  {}", gaps.join("\n  "));
    }

    /// Сам список меняющих состояние не должен разойтись с тем, что мост
    /// отдаёт: иначе он требовал бы гасить несуществующее.
    #[test]
    fn every_state_changing_tool_is_one_we_expose() {
        let known = exposable();
        for name in CHANGES_STATE {
            assert!(known.contains(&name), "{name} в CHANGES_STATE, но мост его не отдаёт");
        }
    }

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

    /// Что написано в поле, то и адрес среды: счёта от порта SSH тут нет.
    #[test]
    fn a_written_gitea_is_returned_as_is() {
        let mut c = sample();
        c.host.gitea = "http://localhost:3000".into();
        assert_eq!(c.gitea_url(), Some("http://localhost:3000"));
        c.host.server = "10.0.0.5:4403".into();
        assert_eq!(c.gitea_url(), Some("http://localhost:3000"));
        // Хвостовой слэш не рождает двойного в адресе репозитория.
        c.host.gitea = "http://localhost:3000/".into();
        assert_eq!(c.gitea_url(), Some("http://localhost:3000"));
        c.host.gitea = "localhost:3000".into();
        assert!(c.validate().unwrap_err().to_string().contains("gitea"));
    }

    /// Пусто — значит не задано. Программа не подставляет «порт SSH минус
    /// два»: в образе этот счёт давал мёртвый адрес, которого человек
    /// никогда не задавал. Toml без ключа читается, и `validate` молчит —
    /// адрес дописывает настройка (`1c-connect install`).
    #[test]
    fn an_empty_gitea_is_none_not_a_guess() {
        let without: Cfg = toml::from_str(&EXAMPLE.replace("gitea = \"\"", "")).unwrap();
        assert!(without.host.gitea.is_empty());
        assert_eq!(without.gitea_url(), None);
        assert_eq!(sample().gitea_url(), None);
        let mut c = sample();
        c.host.gitea = "   ".into();
        assert_eq!(c.gitea_url(), None);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn example_prints_gitea_with_its_rule() {
        let lines: Vec<&str> = EXAMPLE.lines().collect();
        let i = lines.iter().position(|l| l.starts_with("gitea = ")).unwrap_or_else(|| panic!("в образце нет поля gitea"));
        assert_eq!(lines[i], "gitea = \"\"", "в образце поле пустое — иначе туннельное рабочее место заметило бы правку");
        let rule: String = lines[..i].iter().rev().take_while(|l| l.starts_with('#')).fold(String::new(), |a, l| format!("{l}\n{a}"));
        assert!(rule.contains("Пусто") && rule.contains("install"), "правило записи не напечатано: {rule}");
        // Заполненное поле доходит до toml через render() и читается обратно.
        let filled = EXAMPLE.replace("gitea = \"\"", "gitea = \"http://localhost:3000\"");
        let c: Cfg = toml::from_str(&filled).unwrap();
        assert_eq!(c.render(), filled);
        assert_eq!(c.gitea_url(), Some("http://localhost:3000"));
    }

    /// Записанное настройкой умолчание ключ `--dump` у моста не включает:
    /// наша запись не должна молча менять агенту корень поиска по коду.
    #[test]
    fn only_a_hand_picked_dump_counts_as_an_override() {
        let mut c = sample();
        c.host.project = "C:\\proj\\Buh".into();
        assert_eq!(c.dump_override(), None, "пусто — умолчание");
        c.host.dump = derived_dump(&c.host.project);
        assert_eq!(c.dump_override(), None, "записанное умолчание — то же умолчание");
        c.host.dump = "D:\\dump".into();
        assert_eq!(c.dump_override(), Some("D:\\dump"));
        // Переехал проект, а записанное умолчание — нет: тогда это свой каталог.
        c.host.dump = derived_dump("C:\\proj\\Torg");
        assert_eq!(c.dump_override(), Some(derived_dump("C:\\proj\\Torg").as_str()));
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

    /// Серверы MCP у pi и у Zed — одно и то же, иначе агенты разойдутся в
    /// умениях и никто не поймёт, почему один знает про выгрузку, а другой
    /// нет. Проверяется не только список имён: `command`, `args` и предел
    /// времени — это и есть всё содержимое записи о сервере.
    ///
    /// Форма у pi своя (`mcpServers` вместо `context_servers`, нет поля
    /// `source`), а `timeout` у обоих в секундах — значит числа переносятся
    /// один в один.
    #[test]
    fn pi_declares_the_same_mcp_servers_as_zed() {
        const PI: &str = include_str!("../../../config/pi/mcp.json");
        let pi: serde_json::Value = serde_json::from_str(PI).expect("config/pi/mcp.json — обычный JSON, без комментариев");
        let zed = zed_json();
        let z = zed["context_servers"].as_object().expect("в настройках Zed есть context_servers");
        let p = pi["mcpServers"].as_object().expect("в mcp.json есть mcpServers");

        let mut zn: Vec<&str> = z.keys().map(String::as_str).collect();
        let mut pn: Vec<&str> = p.keys().map(String::as_str).collect();
        zn.sort();
        pn.sort();
        assert_eq!(zn, pn, "серверы MCP у pi и у Zed разошлись");

        for name in zn {
            // `env` сверяется тоже, хотя сейчас его нет ни у кого:
            // появится он сперва у Zed, и расхождение прошло бы молча.
            for field in ["command", "args", "timeout", "env"] {
                assert_eq!(
                    z[name][field], p[name][field],
                    "сервер {name}: поле {field} у pi и у Zed разошлось"
                );
            }
        }
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
