//! Операции на машине с 1С: окружение и расширение в базе. Туннель и мост —
//! не операции с состоянием, а процесс `connect`.

use crate::cfg::Cfg;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
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

/// Каталог инструментов на машине: с носителя комплекта (флешка, FAT32,
/// Defender на каждый запуск) 48 МБ .NET не запускают.
pub fn tools_dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into())).join("dev1c")
    } else {
        match std::env::var_os("XDG_DATA_HOME") {
            Some(d) => PathBuf::from(d).join("dev1c"),
            None => PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".local/share/dev1c"),
        }
    }
}

/// OneScript лежит как в своём архиве: `bin/` и `lib/` рядом, `oscript.cfg`
/// говорит `lib.system = ../lib` — библиотеки (и `lib/add` для vrunner)
/// находятся без переменных окружения.
pub fn oscript_exe(tools: &Path) -> PathBuf {
    tools.join("vrunner").join("bin").join(if cfg!(windows) { "oscript.exe" } else { "oscript" })
}

pub fn vrunner_lib(tools: &Path) -> PathBuf {
    tools.join("vrunner").join("lib")
}

pub fn git_exe(tools: &Path) -> PathBuf {
    if cfg!(windows) {
        tools.join("git").join("cmd").join("git.exe")
    } else {
        tools.join("git").join("bin").join("git")
    }
}

pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            n += copy_tree(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), &dst)?;
            n += 1;
        }
    }
    Ok(n)
}

const VERSION_FILE: &str = "tools.version";

/// Состояние по отметке версии: комплект пишет `tools.version` (pack.sh),
/// копия на машине несёт ту же строку.
pub fn tools_state(kit_root: &Path, tools: &Path) -> OpState {
    if !kit_root.join("vrunner").is_dir() || !kit_root.join("git").is_dir() {
        return OpState::Blocked("в комплекте нет vrunner/ или git/ — комплект собран без инструментов (pack.sh)".into());
    }
    let want = std::fs::read_to_string(kit_root.join(VERSION_FILE)).unwrap_or_default();
    let have = std::fs::read_to_string(tools.join(VERSION_FILE)).unwrap_or_default();
    if have.trim().is_empty() {
        OpState::Missing
    } else if have.trim() == want.trim() {
        OpState::Done(format!("{} в {}", want.trim(), tools.display()))
    } else {
        OpState::Drifted { from: have.trim().to_string(), to: want.trim().to_string() }
    }
}

struct Tools;
impl Op<Ctx> for Tools {
    fn id(&self) -> &'static str {
        "host.tools"
    }
    fn title(&self) -> &'static str {
        "Инструменты комплекта на машине"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        tools_state(&ctx.kit.root, &tools_dir())
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!("скопировать vrunner/ и git/ из {} в {} (≈ 100 МБ)", ctx.kit.root.display(), tools_dir().display()))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let to = tools_dir();
        if ctx.dry_run {
            return Ok(());
        }
        for d in ["vrunner", "git"] {
            let _ = std::fs::remove_dir_all(to.join(d));
            let n = copy_tree(&ctx.kit.root.join(d), &to.join(d)).with_context(|| format!("копирование {d} в {}", to.display()))?;
            ctx.ok(format!("{d}: {n} файлов"));
        }
        let o = docker::probe(ctx, &oscript_exe(&to).to_string_lossy(), &["--version"])?;
        anyhow::ensure!(o.ok(), "oscript не запускается из {}: {}", to.display(), o.tail(2).join(" "));
        ctx.ok(format!("oscript {}", o.text.trim()));
        let o = docker::probe(ctx, &git_exe(&to).to_string_lossy(), &["--version"])?;
        anyhow::ensure!(o.ok(), "git не запускается из {}: {}", to.display(), o.tail(2).join(" "));
        ctx.ok(o.text.trim().to_string());
        std::fs::copy(ctx.kit.root.join(VERSION_FILE), to.join(VERSION_FILE)).context("отметка версии")?;
        Ok(())
    }
    fn revert(&self, _ctx: &Ctx, level: CleanLevel) -> Option<Vec<Action>> {
        matches!(level, CleanLevel::Data).then(|| vec![Action::new(format!("удалить {}", tools_dir().display()))])
    }
    fn revert_apply(&self, ctx: &Ctx, level: CleanLevel) -> Result<()> {
        if matches!(level, CleanLevel::Data) {
            let _ = std::fs::remove_dir_all(tools_dir());
            ctx.ok("инструменты удалены с машины");
        }
        Ok(())
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
    vec![Box::new(Env), Box::new(Tools), Box::new(Extension)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_live_under_the_local_app_data_not_the_kit() {
        let t = tools_dir();
        assert!(t.ends_with("dev1c"), "{}", t.display());
        assert!(oscript_exe(Path::new("T")).ends_with(if cfg!(windows) { "vrunner\\bin\\oscript.exe" } else { "vrunner/bin/oscript" }));
        assert!(vrunner_lib(Path::new("T")).ends_with("lib"));
    }

    #[test]
    fn copy_tree_copies_nested_files_and_counts_them() {
        let tmp = std::env::temp_dir().join(format!("dev1c-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src/a/b")).unwrap();
        std::fs::write(tmp.join("src/a/b/f.txt"), "1").unwrap();
        std::fs::write(tmp.join("src/g.txt"), "2").unwrap();
        let n = copy_tree(&tmp.join("src"), &tmp.join("dst")).unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read_to_string(tmp.join("dst/a/b/f.txt")).unwrap(), "1");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tools_op_state_follows_the_version_stamp() {
        let tmp = std::env::temp_dir().join(format!("dev1c-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let kit = tmp.join("kit");
        let home = tmp.join("home");
        std::fs::create_dir_all(kit.join("vrunner")).unwrap();
        std::fs::create_dir_all(kit.join("git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(kit.join("tools.version"), "OneScript 2.2.0; vanessa-runner 3.0.2; MinGit 2.56.0.2\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Missing));
        std::fs::write(home.join("tools.version"), "старая\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Drifted { .. }));
        std::fs::write(home.join("tools.version"), "OneScript 2.2.0; vanessa-runner 3.0.2; MinGit 2.56.0.2\n").unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Done(d) if d.contains("3.0.2")));
        std::fs::remove_dir_all(kit.join("git")).unwrap();
        assert!(matches!(tools_state(&kit, &home), OpState::Blocked(_)));
        let _ = std::fs::remove_dir_all(&tmp);
    }

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
