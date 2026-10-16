//! Операции сервера модели на этой машине (serve = "host"): файлы из
//! комплекта, служба, замер. Каждая умеет объясниться: без карты, без
//! модели, без nssm человек узнаёт причину, а не «шагов: 0».

use crate::cfg::{Facts, Form};
use crate::llama;
use crate::ops::Ctx;
use anyhow::Result;
use common::docker;
use common::op::{Action, CleanLevel, Op, OpState};
use common::Runner;

pub fn ops(form: Form) -> Vec<Box<dyn Op<Ctx>>> {
    match form {
        Form::Host => vec![Box::new(Bin), Box::new(Service), Box::new(Answers)],
        Form::Docker => vec![Box::new(Answers)], // контейнер — следующей задачей
        Form::Remote => vec![],
    }
}

/// Адрес, по которому сервер спрашивает сам установщик (с этой машины).
pub fn local_url(ctx: &Ctx) -> String {
    match ctx.cfg.form() {
        Form::Host => format!("http://{}:{}", llama::bind_host(ctx), ctx.cfg.model.port),
        _ => format!("http://127.0.0.1:{}", ctx.cfg.ports()[2]),
    }
}

/// Командная строка, какой она должна быть сейчас (служба хоста).
pub fn desired_args(ctx: &Ctx) -> Vec<String> {
    let used = llama::gpu(ctx).map(|g| g.used_mb).unwrap_or(0);
    let host = llama::bind_host(ctx);
    let token = llama::token_file().to_string_lossy().to_string();
    let alias = ctx.cfg.model_alias();
    llama::serve_args(&llama::ServeInput {
        model: &ctx.cfg.model.file,
        alias: &alias,
        host: &host,
        port: ctx.cfg.model.port,
        fit_ctx: ctx.cfg.model.fit_ctx,
        fit_target: llama::fit_target_mb(used),
        cache_ram: llama::cache_ram_mb(llama::free_ram_mb(ctx)),
        token_file: Some(&token),
        token_env: false,
    })
}

pub fn args_drift(want: &[String], baked: &str) -> Option<(String, String)> {
    let w = want.join(" ");
    (w != baked).then(|| (format!("служба: {baked}"), format!("служба: {w}")))
}

/// Что из окна сервера следует для Zed; меньше нижней границы — провал с числами.
pub fn verdict(cfg: &crate::cfg::Cfg, window: u32) -> Result<u32> {
    anyhow::ensure!(
        window >= cfg.model.fit_ctx,
        "сервер выделил окно {window} токенов при нижней границе fit_ctx = {}: на этой карте модель с такими параметрами не помещается",
        cfg.model.fit_ctx
    );
    llama::budget(window, cfg.model.agents).ok_or_else(|| {
        anyhow::anyhow!("окно {window} на {} агентов даёт меньше {} токенов на агента — для кодинга на 1С мало", cfg.model.agents, llama::MIN_BUDGET)
    })
}

struct Bin;
impl Op<Ctx> for Bin {
    fn id(&self) -> &'static str {
        "llama.bin"
    }
    fn title(&self) -> &'static str {
        "Сервер модели (файлы)"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        let Some(src) = llama::kit_bin(ctx) else {
            return OpState::Blocked("в комплекте нет llama/bin: pack.sh кладёт его рядом с образом".into());
        };
        if !llama::server_exe().exists() || (cfg!(windows) && !llama::dir().join("nssm.exe").exists()) {
            return OpState::Missing;
        }
        // Сверяем сборку: иначе ряд из комплекта никогда не доедет до машины,
        // на которой сервер уже лежит.
        let ver = |p: &std::path::Path| docker::probe(ctx, &p.to_string_lossy(), &["--version"]).map(|o| o.text.trim().to_string()).unwrap_or_default();
        let have = ver(&llama::server_exe());
        let want = ver(&src.join(llama::server_exe().file_name().unwrap_or_default()));
        if !want.is_empty() && have != want {
            return OpState::Drifted { from: have, to: want };
        }
        OpState::Done(llama::dir().display().to_string())
    }
    fn plan(&self, _ctx: &Ctx) -> Vec<Action> {
        let mut v = vec![Action::new(format!("скопировать llama-server и библиотеки CUDA из комплекта в {}", llama::dir().display()))];
        if cfg!(windows) {
            v.push(Action::new("положить рядом nssm.exe из комплекта"));
        }
        v
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let src = llama::kit_bin(ctx).ok_or_else(|| anyhow::anyhow!("в комплекте нет llama/bin"))?;
        llama::ensure_dir(ctx)?;
        for e in std::fs::read_dir(&src)? {
            let e = e?;
            if e.file_type()?.is_file() {
                std::fs::copy(e.path(), llama::dir().join(e.file_name()))?;
            }
        }
        if cfg!(windows) {
            let n = ctx.kit.path("llama/nssm.exe");
            anyhow::ensure!(n.exists(), "в комплекте нет llama/nssm.exe");
            std::fs::copy(&n, llama::dir().join("nssm.exe"))?;
        }
        ctx.ok(format!("файлы сервера в {}", llama::dir().display()));
        Ok(())
    }
    fn revert(&self, _ctx: &Ctx, level: CleanLevel) -> Option<Vec<Action>> {
        (level == CleanLevel::Data).then(|| vec![Action::new(format!("удалить {}", llama::dir().display()))])
    }
    fn revert_apply(&self, ctx: &Ctx, level: CleanLevel) -> Result<()> {
        if level == CleanLevel::Data {
            let _ = std::fs::remove_dir_all(llama::dir());
            ctx.ok("файлы сервера удалены");
        }
        Ok(())
    }
}

struct Service;
impl Op<Ctx> for Service {
    fn id(&self) -> &'static str {
        "llama.service"
    }
    fn title(&self) -> &'static str {
        "Служба модели"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        let m = &ctx.cfg.model;
        if m.file.is_empty() {
            return OpState::Blocked("модель не выбрана: [model] file — путь к .gguf на этой машине".into());
        }
        if !std::path::Path::new(&m.file).exists() {
            return OpState::Blocked(format!("нет файла модели {}", m.file));
        }
        if llama::gpu(ctx).is_none() {
            return OpState::Blocked("нет видеокарты (nvidia-smi не отвечает): модель для агентов на процессоре неработоспособна".into());
        }
        if llama::kit_bin(ctx).is_none() && !llama::server_exe().exists() {
            return OpState::Blocked("нет сервера модели: в комплекте нет llama/bin".into());
        }
        if cfg!(windows) && !llama::nssm_available(ctx) {
            return OpState::Blocked("нет nssm: его кладут в комплект как llama/nssm.exe".into());
        }
        let got = llama::installed(ctx);
        if !got.present {
            // Порт занимать будем мы: узнать о чужом лучше до регистрации.
            if llama::port_busy(m.port) {
                return OpState::Blocked(format!("порт {} занят другой программой — [model] port", m.port));
            }
            return OpState::Missing;
        }
        let baked = ctx.kit.state_get("llama.args").unwrap_or_default();
        if let Some(d) = args_drift(&desired_args(ctx), &baked) {
            return OpState::Drifted { from: d.0, to: d.1 };
        }
        if !got.running {
            return OpState::Drifted { from: "служба остановлена".into(), to: "работает".into() };
        }
        OpState::Done(format!("{} на {}:{}", ctx.cfg.model_alias(), llama::bind_host(ctx), m.port))
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![
            Action::new(format!("служба {} ({}): {}", llama::SERVICE, if cfg!(windows) { "nssm" } else { "systemd" }, ctx.cfg.model.file)),
            Action::new(format!("три слота, общий кэш, окно подбирает сервер (не ниже {})", ctx.cfg.model.fit_ctx)),
        ]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let _ = llama::ensure_token(ctx)?;
        if llama::installed(ctx).present {
            llama::service_remove(ctx)?;
        }
        let args = desired_args(ctx);
        llama::service_install(ctx, &args)?;
        ctx.kit.state_set("llama.args", &args.join(" "))?;
        llama::service_start(ctx)?;
        ctx.ok("служба модели зарегистрирована и запущена");
        Ok(())
    }
    fn revert(&self, _ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        Some(vec![Action::new(format!("снять службу {} (модель и файлы остаются)", llama::SERVICE))])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        llama::service_remove(ctx)?;
        ctx.ok("служба модели снята");
        Ok(())
    }
}

/// Сервер отвечает, и мы знаем его настоящие числа. Всегда выполняется:
/// это и есть синхронизация с Zed.
struct Answers;
impl Op<Ctx> for Answers {
    fn id(&self) -> &'static str {
        "llama.answers"
    }
    fn title(&self) -> &'static str {
        "Замер сервера модели"
    }
    fn always_run(&self) -> bool {
        true
    }
    fn state(&self, _ctx: &Ctx) -> OpState {
        OpState::Missing
    }
    fn plan(&self, _ctx: &Ctx) -> Vec<Action> {
        vec![
            Action::new("дождаться ответа сервера и прочитать выбранное окно (до пяти минут)"),
            Action::new("замерить скорость разбора и записать числа для Zed"),
        ]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        if ctx.dry_run {
            return Ok(());
        }
        let token = llama::ensure_token(ctx)?;
        let url = local_url(ctx);
        llama::wait_health(&url, &token, 300).map_err(|e| anyhow::anyhow!("{e:#} — смотрите {}", llama::log_file().display()))?;
        let p = llama::props(&url, &token).ok_or_else(|| anyhow::anyhow!("сервер не отдал /props"))?;
        let max_tokens = verdict(&ctx.cfg, p.window)?;
        ctx.ok(format!("окно {} токенов, слотов {}, модель {}; Zed получает max_tokens = {max_tokens}", p.window, p.slots, p.model));
        let prefill = llama::measure_prefill(&url, &token);
        match prefill {
            Some(s) => ctx.ok(format!("скорость разбора {s:.0} токенов в секунду")),
            None => ctx.warn("скорость разбора замерить не вышло (ответ без timings)"),
        }
        llama::facts_save(ctx, &Facts { token, window: p.window, max_tokens }, prefill)?;
        if let Some(g) = llama::gpu(ctx) {
            if g.total_mb > 0 && g.used_mb * 100 / g.total_mb > 95 {
                ctx.warn(format!("карта занята на {} из {} МиБ: соседняя нагрузка будет вытеснять модель", g.used_mb, g.total_mb));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{Cfg, Form};

    #[test]
    fn the_form_chooses_the_operations_and_their_order() {
        let ids = |f: Form| ops(f).iter().map(|o| o.id().to_string()).collect::<Vec<_>>();
        assert_eq!(ids(Form::Host), ["llama.bin", "llama.service", "llama.answers"]);
        assert_eq!(ids(Form::Remote), Vec::<String>::new());
    }

    #[test]
    fn baked_arguments_are_compared_as_one_line() {
        // Служба запекает параметры: разошлись — пересоздание.
        assert!(args_drift(&["a".into(), "b".into()], "a b").is_none());
        let (from, to) = args_drift(&["a".into(), "c".into()], "a b").unwrap();
        assert!(from.contains("a b") && to.contains("a c"));
    }

    #[test]
    fn the_verdict_of_answers_needs_the_window_and_the_budget() {
        let mut c = Cfg::default();
        c.model.fit_ctx = 65536;
        assert!(verdict(&c, 32768).is_err(), "окно меньше fit_ctx — провал с числами");
        let e = verdict(&c, 32768).unwrap_err().to_string();
        assert!(e.contains("32768") && e.contains("65536"), "{e}");
        assert_eq!(verdict(&c, 131072).unwrap(), 131072 - 8192 - 2048);
    }
}
