//! Проверки на машине с 1С, от основания к вершине: платформа → комплект →
//! база → веб-сервер → публикация и расширение → туннель. Человек читает
//! сверху вниз и останавливается на первом провале.

use crate::ops_host::{extension_version, find_1cv8, mcp_exe, Ctx};
use common::check::Check;

/// Порядок фактов — часть контракта (см. тест).
pub const ORDER: [&str; 7] = ["параметры", "платформа 1С", "mcp-1c в комплекте", "база", "веб-сервер", "публикация и расширение", "туннель"];

/// Файл параметров есть и адрес среды в нём не из образца.
pub fn params_fact(file_exists: bool, is_sample: bool, server: &str) -> Check {
    if !file_exists {
        Check::fail(ORDER[0], "1c-connect.toml нет в комплекте", "1c-connect example > 1c-connect.toml, затем впишите свои значения")
    } else if is_sample {
        Check::fail(ORDER[0], format!("server = {server} — адрес из образца"), "впишите адрес своей среды: его печатает dev1c-setup install")
    } else {
        Check::pass(ORDER[0], format!("среда {server}"))
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
    v.push(params_fact(ctx.kit.path(crate::cfg::FILE).exists(), ctx.cfg.is_sample(), &h.server));
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
    let server_base = h.base_path.contains('\\') && !h.base_path.contains(':');
    v.push(if server_base || std::path::Path::new(&h.base_path).exists() {
        Check::pass(ORDER[3], h.base_path.clone())
    } else {
        Check::fail(ORDER[3], format!("каталога {} нет", h.base_path), "base_path в 1c-connect.toml")
    });
    let web = ctx.cfg.web_port();
    let web_ok = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], web).into(), std::time::Duration::from_secs(3)).is_ok();
    v.push(if web_ok {
        Check::pass(ORDER[4], format!("порт {web} отвечает"))
    } else {
        Check::fail(ORDER[4], format!("порт {web} не отвечает"), "запустите Apache/IIS, на котором опубликована база")
    });
    v.push(match extension_version(&h.base_url, &h.db_user, &h.db_password) {
        Some(ver) => Check::pass(ORDER[5], format!("{} → расширение {ver}", h.base_url)),
        None => Check::fail(ORDER[5], format!("{}/version не отвечает", h.base_url), "1c-connect install, затем публикация в Конфигураторе (см. install)"),
    });
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
    fn untouched_parameters_fail_first_and_say_what_to_do() {
        assert_eq!(ORDER[0], "параметры");
        let c = params_fact(false, true, "192.168.1.10:3393");
        assert!(!c.ok && c.hint.contains("example"), "{c:?}");
        let c = params_fact(true, true, "192.168.1.10:3393");
        assert!(!c.ok && c.detail.contains("образца"), "{c:?}");
        let c = params_fact(true, false, "10.0.0.5:3393");
        assert!(c.ok);
    }

    #[test]
    fn the_tunnel_fact_is_information_when_nobody_connected() {
        let c = tunnel_fact(None);
        assert!(c.info && c.ok, "{c:?}");
        let c = tunnel_fact(Some("работает с 12:40"));
        assert!(!c.info && c.ok);
    }
}
