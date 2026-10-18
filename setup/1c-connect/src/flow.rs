//! Выгрузка и загрузка: те же инструменты, что у агента, в том же порядке
//! и под тем же мьютексом. Чистая часть — через `Deps`, чтобы проверять
//! порядок шагов без платформы.

use crate::local::Reply;
use crate::ops_host::Ctx;
use common::Runner;
use serde_json::{json, Value};

pub struct Deps<'a> {
    pub call: &'a dyn Fn(&str, Value) -> Result<Reply, String>,
    pub platform_running: &'a dyn Fn() -> usize,
    pub push: &'a dyn Fn(&str) -> Result<String, String>,
    pub pull: &'a dyn Fn() -> Result<String, String>,
    pub dump_index_exists: bool,
    /// `--dry-run`: только план, ни одного вызова.
    pub dry_run: bool,
}

fn lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    match crate::project::LOCK.try_lock() {
        Ok(g) => Ok(g),
        Err(std::sync::TryLockError::Poisoned(p)) => Ok(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => {
            let what = crate::project::doing();
            Err(format!("занято: идёт {}", if what.is_empty() { "другая операция" } else { what }))
        }
    }
}

/// cf_decompile в src, затем project_push.
pub fn export_with(d: &Deps, message: &str, log: &dyn Fn(String)) -> Result<String, String> {
    let _g = lock()?;
    if d.dry_run {
        log("план: cf_decompile → src, затем git add/commit/push ветки рабочего места (project_push)".into());
        return Ok("план напечатан, на машине ничего не менялось".into());
    }
    log("cf_decompile → src (минуты; Конфигуратор работает в фоне)".into());
    let r = (d.call)("cf_decompile", json!({"OUT": "src", "background": false}))?;
    if r.is_error {
        return Err(format!("выгрузка не удалась: {}", r.text));
    }
    if !r.text.trim().is_empty() {
        log(r.text);
    }
    log("git: add, commit, push в Gitea — первая выгрузка это десятки тысяч файлов, минуты".into());
    (d.push)(message)
}

/// project_pull, затем cf_load инкрементом.
pub fn import_with(d: &Deps, log: &dyn Fn(String)) -> Result<String, String> {
    let _g = lock()?;
    if d.dry_run {
        log("план: project_pull (git pull --rebase), затем cf_load --increment из src".into());
        return Ok("план напечатан, на машине ничего не менялось".into());
    }
    let n = (d.platform_running)();
    if n > 0 {
        return Err(format!("на машине запущен 1cv8.exe ({n}) — закройте Конфигуратор и клиентов 1С, затем повторите"));
    }
    log((d.pull)()?);
    if !d.dump_index_exists {
        log("src/ConfigDumpInfo.xml нет — загрузка будет полной, не инкрементом (первая после клонирования)".into());
    }
    log("cf_load ← src (минуты)".into());
    let r = (d.call)("cf_load", json!({"SRC": "src", "background": false, "increment": true}))?;
    if r.is_error {
        return Err(format!(
            "загрузка не удалась: {} — в Конфигураторе: Конфигурация → Вернуться к конфигурации БД, затем повторите; ветка уже обновлена",
            r.text
        ));
    }
    Ok(if r.text.trim().is_empty() { "загружено".into() } else { r.text })
}

/// Настоящие части: процесс `vrunner-mcp` из инструментов на машине, git проекта.
fn with_real<R>(ctx: &Ctx, f: impl FnOnce(&Deps, &dyn Fn(String)) -> R) -> R {
    let tools = crate::ops_host::tools_dir();
    let p = crate::project::Project::from_cfg(&ctx.cfg, &tools);
    let log = |s: String| ctx.say(s);
    let cfg = ctx.cfg.clone();
    let call = |tool: &str, mut args: Value| {
        // The same connection arguments the bridge injects for the agent.
        if let (Some(a), Some(d)) = (args.as_object_mut(), crate::bridge::connection_args(&cfg).as_object()) {
            for (k, v) in d {
                a.insert(k.clone(), v.clone());
            }
        }
        let cmd = crate::bridge::vrunner_command(&cfg, &tools).into_std();
        crate::local::call(cmd, tool, args, std::time::Duration::from_secs(3600), &log)
    };
    let push = |m: &str| crate::project::push(&p, m);
    let pull = || crate::project::pull(&p);
    let d = Deps {
        call: &call,
        platform_running: &crate::bridge::configurator_count,
        push: &push,
        pull: &pull,
        dump_index_exists: p.dir.join("src").join("ConfigDumpInfo.xml").exists(),
        dry_run: ctx.dry_run,
    };
    f(&d, &log)
}

pub fn export(ctx: &Ctx, message: &str) -> Result<String, String> {
    with_real(ctx, |d, log| export_with(d, message, log))
}

pub fn import(ctx: &Ctx) -> Result<String, String> {
    with_real(ctx, |d, log| import_with(d, log))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<String>>,
        reply: RefCell<Vec<(String, bool)>>, // answers in order
        running: usize,
        pushed: RefCell<bool>,
        pulled: RefCell<bool>,
        index: bool,
        dry: bool,
    }
    impl Fake {
        fn new(replies: &[(&str, bool)]) -> Fake {
            Fake {
                calls: RefCell::new(vec![]),
                reply: RefCell::new(replies.iter().map(|(t, e)| (t.to_string(), *e)).collect()),
                running: 0,
                pushed: RefCell::new(false),
                pulled: RefCell::new(false),
                index: true,
                dry: false,
            }
        }
        fn run<R>(&self, f: impl FnOnce(&Deps) -> R) -> R {
            let call = |tool: &str, args: Value| {
                self.calls.borrow_mut().push(format!("{tool} {args}"));
                let (t, e) = self.reply.borrow_mut().remove(0);
                Ok(Reply { text: t, is_error: e })
            };
            let platform_running = || self.running;
            let push = |m: &str| {
                *self.pushed.borrow_mut() = true;
                Ok(format!("отправлено: {m}"))
            };
            let pull = || {
                *self.pulled.borrow_mut() = true;
                Ok("pulled".to_string())
            };
            f(&Deps { call: &call, platform_running: &platform_running, push: &push, pull: &pull, dump_index_exists: self.index, dry_run: self.dry })
        }
    }

    #[test]
    fn export_decompiles_then_pushes() {
        let f = Fake::new(&[("разобрано", false)]);
        let r = f.run(|d| export_with(d, "выгрузка", &|_| {})).unwrap();
        assert!(r.contains("отправлено"), "{r}");
        assert_eq!(f.calls.borrow()[0], r#"cf_decompile {"OUT":"src","background":false}"#);
        assert!(*f.pushed.borrow());
    }

    #[test]
    fn a_failed_decompile_does_not_push() {
        let f = Fake::new(&[("платформа: ошибка", true)]);
        let e = f.run(|d| export_with(d, "x", &|_| {})).unwrap_err();
        assert!(e.contains("платформа") && !*f.pushed.borrow(), "{e}");
    }

    #[test]
    fn import_refuses_before_pulling_while_the_platform_runs() {
        let mut f = Fake::new(&[]);
        f.running = 1;
        let e = f.run(|d| import_with(d, &|_| {})).unwrap_err();
        assert!(e.contains("закройте Конфигуратор") && !*f.pulled.borrow(), "{e}");
    }

    #[test]
    fn import_pulls_then_loads_incrementally_and_warns_without_the_index() {
        let mut f = Fake::new(&[("загружено", false)]);
        f.index = false;
        let log = RefCell::new(Vec::new());
        let r = f.run(|d| import_with(d, &|s| log.borrow_mut().push(s))).unwrap();
        assert!(*f.pulled.borrow() && r.contains("загружено"), "{r}");
        assert_eq!(f.calls.borrow()[0], r#"cf_load {"SRC":"src","background":false,"increment":true}"#);
        assert!(log.borrow().iter().any(|l| l.contains("ConfigDumpInfo.xml")), "{:?}", log.borrow());
    }

    #[test]
    fn failed_load_names_the_way_back() {
        let f = Fake::new(&[("Конфигуратор: ошибка загрузки", true)]);
        let e = f.run(|d| import_with(d, &|_| {})).unwrap_err();
        assert!(e.contains("Вернуться к конфигурации БД") && *f.pulled.borrow(), "{e}");
    }

    #[test]
    fn dry_run_prints_the_plan_and_touches_nothing() {
        // --dry-run is global and promises «ничего не делать»: no decompile,
        // no push, no pull, no load — only the plan in the log.
        let mut f = Fake::new(&[]);
        f.dry = true;
        let log = RefCell::new(Vec::new());
        let r = f.run(|d| export_with(d, "x", &|s| log.borrow_mut().push(s))).unwrap();
        assert!(f.calls.borrow().is_empty() && !*f.pushed.borrow(), "{r}");
        assert!(log.borrow().iter().any(|l| l.contains("cf_decompile")), "{:?}", log.borrow());
        let r = f.run(|d| import_with(d, &|_| {})).unwrap();
        assert!(f.calls.borrow().is_empty() && !*f.pulled.borrow(), "{r}");
    }

    #[test]
    fn the_git_phase_of_export_is_announced_before_it_starts() {
        let f = Fake::new(&[("разобрано", false)]);
        let log = RefCell::new(Vec::new());
        f.run(|d| export_with(d, "m", &|s| log.borrow_mut().push(s))).unwrap();
        assert!(log.borrow().iter().any(|l| l.contains("git") && l.contains("минут")), "{:?}", log.borrow());
    }

    #[test]
    fn export_is_told_busy_like_any_tool() {
        let _g = crate::project::LOCK.try_lock().unwrap_or_else(|e| match e {
            std::sync::TryLockError::Poisoned(g) => g.into_inner(),
            e => panic!("{e}"),
        });
        let f = Fake::new(&[]);
        let e = f.run(|d| export_with(d, "x", &|_| {})).unwrap_err();
        assert!(e.contains("занято"), "{e}");
    }
}
