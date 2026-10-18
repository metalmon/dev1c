//! Проверки на машине с 1С, от основания к вершине: платформа → комплект →
//! инструменты и проект → база → веб-сервер → публикация и расширение →
//! среда (Gitea) → что мешает загрузке → туннель. Человек читает сверху
//! вниз и останавливается на первом провале.

use crate::ops_host::{extension_version, find_1cv8, mcp_exe, tools_dir, tools_state, Ctx, Gitea};
use common::check::Check;
use common::op::OpState;

/// Порядок фактов — часть контракта (см. тест).
pub const ORDER: [&str; 13] = [
    "параметры",
    "платформа 1С",
    "mcp-1c в комплекте",
    "инструменты комплекта",
    "vrunner-mcp отвечает",
    "проект",
    "база",
    "веб-сервер",
    "публикация и расширение",
    "Gitea",
    "Конфигуратор",
    "комплект на носителе",
    "туннель",
];

pub fn gitea_fact(g: &Gitea) -> Check {
    match g {
        Gitea::Down(e) => Check::fail(ORDER[9], format!("не отвечает: {e}"), "среда не поднята или адрес не тот — dev1c-setup check на сервере"),
        Gitea::NoOrg => Check::fail(ORDER[9], "отвечает, но организации dev1c в среде нет", "образ среды старый — dev1c-setup install с новым комплектом"),
        Gitea::Ok(v) => Check::pass(ORDER[9], format!("Gitea {v}, организация dev1c есть")),
    }
}

pub fn configurator_fact(n: usize) -> Check {
    if n == 0 {
        Check::pass(ORDER[10], "не запущен")
    } else {
        Check::fail(ORDER[10], format!("запущено процессов 1cv8: {n}"), "закройте Конфигуратор и клиентов 1С перед загрузкой: cf_load и import откажут, пока он открыт")
    }
}

pub fn media_fact(kind: Option<String>, root: &str) -> Check {
    match kind {
        Some(k) => Check::note(ORDER[11], format!("{root}: {k} — инструменты работают из %LOCALAPPDATA%, комплект можно вынуть после install")),
        None => Check::pass(ORDER[11], format!("{root}: обычный диск")),
    }
}

/// База: каталог есть (файловая), либо сервер\база (не проверяется); сетевой
/// путь — предупреждение, не провал.
pub fn base_fact(path: &str, exists: bool) -> Check {
    if path.starts_with("\\\\") {
        return Check::note(ORDER[6], format!("{path} — сетевая база, одна на двоих: cf_load второго затирает работу первого"));
    }
    let server_base = path.contains('\\') && !path.contains(':');
    if server_base || exists {
        Check::pass(ORDER[6], path.to_string())
    } else {
        Check::fail(ORDER[6], format!("каталога {path} нет"), "base_path в 1c-connect.toml")
    }
}

/// Локальный `initialize` + безвредный инструмент: процесс поднимается из
/// копии на машине с cwd = project; туннель не нужен.
fn vrunner_fact(ctx: &Ctx, tools: &std::path::Path) -> Check {
    let t0 = std::time::Instant::now();
    let cmd = crate::bridge::vrunner_command(&ctx.cfg, tools).into_std();
    match crate::local::call(cmd, "cache_show", serde_json::json!({}), std::time::Duration::from_secs(60), &|_| {}) {
        Ok(r) if !r.is_error => Check::pass(ORDER[4], format!("AutumnMCP, {} с до ответа", t0.elapsed().as_secs())),
        Ok(r) => Check::fail(ORDER[4], format!("отвечает ошибкой: {}", r.text), "каталог проекта и инструменты: 1c-connect install"),
        Err(e) => Check::fail(ORDER[4], e, "1c-connect install (host.tools); журнал — setup.log в комплекте"),
    }
}

fn project_fact(ctx: &Ctx, tools: &std::path::Path) -> Check {
    use crate::project::Layout;
    let p = crate::project::Project::from_cfg(&ctx.cfg, tools);
    match crate::project::layout_state(&p) {
        Layout::Ok => Check::pass(ORDER[5], format!("{}, ветка {}", p.dir.display(), p.branch)),
        Layout::Missing => Check::fail(ORDER[5], format!("{} не разложен", p.dir.display()), "1c-connect install (host.project)"),
        Layout::BaseDir(f) => Check::fail(ORDER[5], format!("{} — каталог базы ({f})", p.dir.display()), "project в 1c-connect.toml — другой каталог"),
        Layout::Drifted(d) => Check::fail(ORDER[5], d, "1c-connect install — поправит ветку, origin и файлы"),
    }
}

/// Файл параметров есть. Адрес среды не проверяется: в проде он может
/// совпасть с образцом.
pub fn params_fact(file_exists: bool, server: &str) -> Check {
    if file_exists {
        Check::pass(ORDER[0], format!("среда {server}"))
    } else {
        Check::fail(ORDER[0], "1c-connect.toml нет в комплекте", "1c-connect example > 1c-connect.toml, затем впишите свои значения")
    }
}

pub fn tunnel_fact(state: Option<&str>) -> Check {
    match state {
        Some(s) => Check::pass("туннель", s.to_string()),
        None => Check::note("туннель", "не поднят (поднимается командой connect)"),
    }
}

pub fn run(ctx: &Ctx, tunnel_state: Option<&str>) -> Vec<Check> {
    let mut v = Vec::new();
    let h = &ctx.cfg.host;
    v.push(params_fact(ctx.kit.path(crate::cfg::FILE).exists(), &h.server));
    v.push(match find_1cv8() {
        Some(p) => Check::pass(ORDER[1], p.display().to_string()),
        None => Check::fail(ORDER[1], "1cv8.exe не найден", "установите платформу 1С на эту машину"),
    });
    let exe = mcp_exe(&ctx.kit);
    v.push(if exe.exists() {
        Check::pass(ORDER[2], exe.file_name().unwrap_or_default().to_string_lossy().to_string())
    } else {
        Check::fail(ORDER[2], "файла нет", "комплект неполный — скопируйте заново")
    });
    let tools = tools_dir();
    let tools_ok = matches!(tools_state(&ctx.kit.root, &tools), OpState::Done(_));
    v.push(match tools_state(&ctx.kit.root, &tools) {
        OpState::Done(d) => Check::pass(ORDER[3], d),
        OpState::Missing => Check::fail(ORDER[3], format!("в {} ничего нет", tools.display()), "1c-connect install"),
        OpState::Drifted { from, to } => Check::fail(ORDER[3], format!("на машине {from}, в комплекте {to}"), "1c-connect install — обновит копию"),
        OpState::Blocked(d) => Check::fail(ORDER[3], d, "возьмите полный комплект у администратора среды"),
    });
    v.push(if tools_ok { vrunner_fact(ctx, &tools) } else { Check::fail(ORDER[4], "инструментов на машине нет", "1c-connect install") });
    v.push(project_fact(ctx, &tools));
    v.push(base_fact(&h.base_path, std::path::Path::new(&h.base_path).exists()));
    let web = ctx.cfg.web_port();
    let web_ok = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], web).into(), std::time::Duration::from_secs(3)).is_ok();
    v.push(if web_ok {
        Check::pass(ORDER[7], format!("порт {web} отвечает"))
    } else {
        Check::fail(ORDER[7], format!("порт {web} не отвечает"), "запустите Apache/IIS, на котором опубликована база")
    });
    v.push(match extension_version(&h.base_url, &h.db_user, &h.db_password) {
        Some(ver) => Check::pass(ORDER[8], format!("{} → расширение {ver}", h.base_url)),
        None => Check::fail(ORDER[8], format!("{}/version не отвечает", h.base_url), "1c-connect install, затем публикация в Конфигураторе (см. install)"),
    });
    v.push(gitea_fact(&crate::ops_host::gitea_probe(&ctx.cfg.gitea_url())));
    v.push(configurator_fact(crate::bridge::configurator_count()));
    let root = ctx.kit.root.display().to_string();
    v.push(media_fact(crate::ops_host::media_kind(&ctx.kit.root), root.get(..3).unwrap_or(&root)));
    v.push(tunnel_fact(tunnel_state));
    v
}

/// Параметры для отчёта без паролей.
pub fn cfg_for_report(cfg: &crate::cfg::Cfg) -> String {
    let mut c = cfg.clone();
    c.host.password = "…".into();
    if !c.host.db_password.is_empty() {
        c.host.db_password = "…".into();
    }
    toml::to_string(&c).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_server_fact_comes_before_tunnel_fact() {
        // Order is the contract: a dead web port must not look like a tunnel fault.
        let names = ORDER;
        let at = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(at("веб-сервер") < at("публикация и расширение"));
        assert!(at("публикация и расширение") < at("туннель"));
    }

    #[test]
    fn order_has_thirteen_facts_with_the_tunnel_last() {
        assert_eq!(ORDER.len(), 13);
        assert_eq!(ORDER[12], "туннель");
        assert_eq!(ORDER[4], "vrunner-mcp отвечает");
        assert_eq!(ORDER[9], "Gitea");
    }

    #[test]
    fn gitea_fact_distinguishes_no_server_from_no_org() {
        let down = gitea_fact(&crate::ops_host::Gitea::Down("timeout".into()));
        assert!(!down.ok && down.hint.contains("dev1c-setup"), "{down:?}");
        let noorg = gitea_fact(&crate::ops_host::Gitea::NoOrg);
        assert!(!noorg.ok && noorg.detail.contains("организации dev1c"), "{}", noorg.detail);
        let ok = gitea_fact(&crate::ops_host::Gitea::Ok("1.22".into()));
        assert!(ok.ok && ok.detail.contains("1.22"));
    }

    #[test]
    fn configurator_and_media_facts_say_what_to_do() {
        let c = configurator_fact(2);
        assert!(!c.ok && c.detail.contains('2') && c.hint.contains("загруз"), "{c:?}");
        assert!(configurator_fact(0).ok);
        let m = media_fact(Some("FAT32".into()), "E:\\");
        assert!(m.ok && m.info && m.detail.contains("FAT32"), "{m:?}");
        assert!(!media_fact(None, "C:\\").info);
    }

    #[test]
    fn a_network_base_is_flagged_as_shared() {
        let c = base_fact("\\\\srv\\bases\\Buh", true);
        assert!(c.ok && c.info && c.detail.contains("одна на двоих"), "{}", c.detail);
        assert!(!base_fact("C:\\Bases\\Buh", false).ok);
        assert!(base_fact("srv\\Buh", false).ok, "a server base is not a directory to look for");
    }

    #[test]
    fn a_missing_parameters_file_fails_first_and_says_what_to_do() {
        assert_eq!(ORDER[0], "параметры");
        let c = params_fact(false, "192.168.1.10:3393");
        assert!(!c.ok && c.hint.contains("example"), "{c:?}");
        // Адрес образца больше не повод для красного: в проде он может совпасть.
        assert!(params_fact(true, "192.168.1.10:3393").ok);
        assert!(params_fact(true, "10.0.0.5:3393").ok);
    }

    #[test]
    fn the_tunnel_fact_is_information_when_nobody_connected() {
        let c = tunnel_fact(None);
        assert!(c.info && c.ok, "{c:?}");
        let c = tunnel_fact(Some("работает с 12:40"));
        assert!(!c.info && c.ok);
    }
}
