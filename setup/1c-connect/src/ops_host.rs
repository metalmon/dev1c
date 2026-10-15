//! Операции на машине с 1С: окружение и расширение в базе. Туннель и мост —
//! не операции с состоянием, а процесс `connect`.

use crate::cfg::Cfg;
use anyhow::Result;
use common::docker;
use common::kit::Kit;
use common::op::{Action, CleanLevel, Op, OpState};
use common::{Prompt, Runner, Sink};

pub struct Ctx {
    pub kit: Kit,
    pub cfg: Cfg,
    pub dry_run: bool,
    pub sink: Sink,
    pub prompt: Box<dyn Prompt>,
}

impl Runner for Ctx {
    fn dry_run(&self) -> bool {
        self.dry_run
    }
    fn sink(&self) -> &Sink {
        &self.sink
    }
    fn kit(&self) -> &Kit {
        &self.kit
    }
}

pub fn mcp_exe(kit: &Kit) -> std::path::PathBuf {
    kit.path(if cfg!(windows) { "mcp-1c-windows-amd64.exe" } else { "mcp-1c-linux-amd64" })
}

/// `1cv8.exe`: the newest version under the standard roots.
pub fn find_1cv8() -> Option<std::path::PathBuf> {
    let roots = ["C:\\Program Files\\1cv8", "C:\\Program Files (x86)\\1cv8"];
    let mut found: Vec<std::path::PathBuf> = roots
        .iter()
        .filter_map(|r| std::fs::read_dir(r).ok())
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("bin").join("1cv8.exe"))
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.pop()
}

/// Ключ подключения конфигуратора: /F для файловой базы, /S для сервер\база.
pub fn ib_switch(base_path: &str) -> String {
    if base_path.contains('\\') && !base_path.contains(':') {
        format!("/S{base_path}")
    } else {
        format!("/F{base_path}")
    }
}

/// Версия расширения, которую отдаёт публикация; пусто — не отвечает.
pub fn extension_version(base_url: &str, user: &str, password: &str) -> Option<String> {
    let c = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(60)).build().ok()?;
    let r = c.get(format!("{base_url}/version")).basic_auth(user, Some(password)).send().ok()?;
    if !r.status().is_success() {
        return None;
    }
    // reqwest's `json` feature is not enabled: parse the body ourselves.
    let v: serde_json::Value = serde_json::from_str(&r.text().ok()?).ok()?;
    v.get("version").and_then(|s| s.as_str()).map(|s| s.to_string())
}

struct Env;
impl Op<Ctx> for Env {
    fn id(&self) -> &'static str {
        "host.env"
    }
    fn title(&self) -> &'static str {
        "Окружение"
    }
    fn always_run(&self) -> bool {
        true
    }
    fn state(&self, _ctx: &Ctx) -> OpState {
        OpState::Missing
    }
    fn plan(&self, _ctx: &Ctx) -> Vec<Action> {
        vec![Action::new("проверить платформу 1С, mcp-1c в комплекте и адрес среды")]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        match find_1cv8() {
            Some(p) => ctx.ok(format!("платформа: {}", p.display())),
            None => anyhow::bail!("1cv8.exe не найден в C:\\Program Files\\1cv8 — установите платформу 1С на эту машину"),
        }
        let exe = mcp_exe(&ctx.kit);
        anyhow::ensure!(exe.exists(), "в комплекте нет {} — комплект неполный", exe.display());
        let o = docker::probe(ctx, &exe.to_string_lossy(), &["--version"])?;
        anyhow::ensure!(o.ok(), "mcp-1c не запускается: {}", o.tail(2).join(" "));
        ctx.ok(o.text.trim().to_string());
        let addr: Result<Vec<_>, _> = std::net::ToSocketAddrs::to_socket_addrs(&ctx.cfg.host.server.as_str()).map(|a| a.collect());
        anyhow::ensure!(addr.is_ok(), "адрес среды {} не разбирается — проверьте server в 1c-connect.toml", ctx.cfg.host.server);
        ctx.ok(format!("среда: {}", ctx.cfg.host.server));
        Ok(())
    }
}

struct Extension;
impl Op<Ctx> for Extension {
    fn id(&self) -> &'static str {
        "host.extension"
    }
    fn title(&self) -> &'static str {
        "Расширение mcp-1c в базе"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        match extension_version(&ctx.cfg.host.base_url, &ctx.cfg.host.db_user, &ctx.cfg.host.db_password) {
            Some(v) => OpState::Done(format!("расширение {v}, публикация отвечает")),
            None => OpState::Missing,
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!(
            "mcp-1c --install {} --db-user {} --db-password … (вызовет конфигуратор, минута-две)",
            ctx.cfg.host.base_path, ctx.cfg.host.db_user
        ))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let exe = mcp_exe(&ctx.kit);
        let h = &ctx.cfg.host;
        let o = docker::exec(ctx, &exe.to_string_lossy(), &["--install", &h.base_path, "--db-user", &h.db_user, "--db-password", &h.db_password])?;
        anyhow::ensure!(o.ok(), "расширение не установилось: {}", o.tail(4).join(" "));
        ctx.ok("расширение установлено");
        if ctx.dry_run {
            return Ok(());
        }
        if extension_version(&h.base_url, &h.db_user, &h.db_password).is_none() {
            ctx.warn(format!(
                "публикация {} не отвечает. Опубликуйте базу: Конфигуратор → Администрирование → Публикация на веб-сервере, вкладка «HTTP-сервисы» → «Публиковать HTTP-сервисы расширений по умолчанию»",
                h.base_url
            ));
        }
        Ok(())
    }
    fn revert(&self, ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        Some(vec![
            Action::new(format!("1cv8 DESIGNER {} /DeleteCfg -Extension MCP_HTTPService (публикацию не трогаем)", ib_switch(&ctx.cfg.host.base_path))),
            Action::new("удалить known_hosts из комплекта"),
        ])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        let Some(v8) = find_1cv8() else { anyhow::bail!("1cv8.exe не найден") };
        let h = &ctx.cfg.host;
        let ib = ib_switch(&h.base_path);
        let n = format!("/N{}", h.db_user);
        let p = format!("/P{}", h.db_password);
        let args = ["DESIGNER", &ib, "/DisableStartupDialogs", "/DeleteCfg", "-Extension", "MCP_HTTPService", &n, &p];
        let o = docker::exec(ctx, &v8.to_string_lossy(), &args)?;
        anyhow::ensure!(o.ok(), "конфигуратор не снял расширение: {}", o.tail(3).join(" "));
        let _ = std::fs::remove_file(ctx.kit.path("known_hosts"));
        ctx.ok("расширение снято, отпечаток сервера забыт");
        Ok(())
    }
}

pub fn host() -> Vec<Box<dyn Op<Ctx>>> {
    vec![Box::new(Env), Box::new(Extension)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_base_uses_s_and_file_base_uses_f() {
        // The configurator's connection switch: /F for a file base, /S for server\base.
        assert_eq!(ib_switch("C:\\Bases\\Buh"), "/FC:\\Bases\\Buh");
        assert_eq!(ib_switch("srv-1c\\buh_prod"), "/Ssrv-1c\\buh_prod");
    }

    #[test]
    fn the_kit_names_the_binary_for_this_platform() {
        let k = Kit::at("X");
        let name = mcp_exe(&k).file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("mcp-1c-"), "{name}");
    }
}
