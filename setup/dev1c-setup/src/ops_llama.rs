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
        Form::Docker => vec![Box::new(Container), Box::new(Answers)],
        Form::Remote => vec![],
    }
}

/// Адрес, по которому сервер спрашивает сам установщик (с этой машины).
pub fn local_url(ctx: &Ctx) -> String {
    match ctx.cfg.form() {
        Form::Host => format!("http://{}:{}", llama::bind_host(ctx), ctx.cfg.model.port),
        _ => format!("http://{}:{}", llama::bind_host(ctx), ctx.cfg.ports()[2]),
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
    })
}

/// Параметры запуска без `--cache-ram`: он считается от свободной памяти и
/// гуляет между запусками, а расхождение должно означать настоящее изменение.
fn significant(line: &str) -> String {
    let mut out = Vec::new();
    let mut it = line.split_whitespace();
    while let Some(w) = it.next() {
        if w == "--cache-ram" {
            it.next();
            continue;
        }
        out.push(w);
    }
    out.join(" ")
}

pub fn args_drift(want: &[String], baked: &str) -> Option<(String, String)> {
    let w = want.join(" ");
    (significant(&w) != significant(baked)).then(|| (format!("служба: {baked}"), format!("служба: {w}")))
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

/// Что остаётся в каталоге службы при раскладке из комплекта: файлы
/// комплекта, nssm, токен и журналы. Всё остальное удаляется — подложенная
/// заранее библиотека загрузилась бы в процесс службы.
pub fn keep_in_service_dir(name: &str, kit_files: &[String]) -> bool {
    kit_files.iter().any(|k| k == name)
        || name == "nssm.exe"
        || name == "dev1c-llama.token"
        || name == "llama.env"
        || (name.starts_with("llama") && name.ends_with(".log"))
}

/// `docker run` контейнера модели: образ из комплекта, том моделей, карта,
/// токен переменной из файла в закрытом каталоге (в аргументах его нет),
/// порт наружу — только на адрес, который видят контейнеры и эта машина
/// (петля на Windows, мост на Linux); внутри сервер слушает 8080.
pub fn llama_run_args(cfg: &crate::cfg::Cfg, env_file: &str, publish_host: &str, cache_ram: u32, fit_target: u32) -> Vec<String> {
    let base = cfg.model.file.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
    let model = format!("/opt/models/{base}");
    let alias = cfg.model_alias();
    let mut a: Vec<String> = [
        "run",
        "-d",
        "--name",
        &cfg.llama_container(),
        "--restart",
        "unless-stopped",
        "--gpus",
        "all",
        "-p",
        &format!("{publish_host}:{}:8080", cfg.ports()[2]),
        "-v",
        &format!("{}:/opt/models", cfg.volume("models")),
        "--env-file",
        env_file,
        &cfg.llama_image(),
        "llama-server",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend(llama::serve_args(&llama::ServeInput {
        model: &model,
        alias: &alias,
        host: "0.0.0.0",
        port: 8080,
        fit_ctx: cfg.model.fit_ctx,
        fit_target,
        cache_ram,
        token_file: None,
    }));
    a
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
            return OpState::Blocked("в комплекте нет папки llama/bin — комплект собран без сервера модели, возьмите полный".into());
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
        // Поверх работающей службы файлы не перепишутся: сначала остановить.
        if llama::installed(ctx).running {
            llama::service_stop(ctx)?;
            ctx.warn("служба модели остановлена на время обновления файлов; шаг llama.service запустит её снова");
        }
        llama::ensure_dir(ctx)?;
        let mut kit_files = Vec::new();
        for e in std::fs::read_dir(&src)? {
            let e = e?;
            if e.file_type()?.is_file() {
                std::fs::copy(e.path(), llama::dir().join(e.file_name()))?;
                kit_files.push(e.file_name().to_string_lossy().to_string());
            }
        }
        // Каталог зеркалит комплект: чужих файлов в нём не остаётся.
        for e in std::fs::read_dir(llama::dir())? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            if !keep_in_service_dir(&name, &kit_files) {
                if e.file_type()?.is_dir() {
                    std::fs::remove_dir_all(e.path())?;
                } else {
                    std::fs::remove_file(e.path())?;
                }
                ctx.warn(format!("удалён посторонний файл {name} из каталога службы"));
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
        if !llama::is_admin(ctx) {
            return OpState::Blocked("нужны права администратора: служба и её каталог регистрируются от имени системы — запустите программу от администратора".into());
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
            if llama::port_busy(&llama::bind_host(ctx), m.port) {
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

struct Container;
impl Op<Ctx> for Container {
    fn id(&self) -> &'static str {
        "llama.container"
    }
    fn title(&self) -> &'static str {
        "Контейнер модели"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        if ctx.cfg.model.file.is_empty() {
            return OpState::Blocked("модель не выбрана: [model] file — файл в комплекте".into());
        }
        if !docker::image_loaded(ctx, &ctx.cfg.llama_image()) {
            return OpState::Blocked(format!("образ {} не загружен: он едет в том же архиве, что и образ среды", ctx.cfg.llama_image()));
        }
        if llama::gpu(ctx).is_none() {
            return OpState::Blocked("нет видеокарты (nvidia-smi не отвечает): модель для агентов на процессоре неработоспособна".into());
        }
        let name = ctx.cfg.llama_container();
        if let Some(st) = crate::ops::state_before_drift(&docker::container_state(ctx, &name)) {
            return st;
        }
        let baked = ctx.kit.state_get("llama.args").unwrap_or_default();
        if let Some(d) = args_drift(&self.wanted(ctx), &baked) {
            return OpState::Drifted { from: d.0, to: d.1 };
        }
        OpState::Done(format!("работает, порт {} на {}", ctx.cfg.ports()[2], llama::bind_host(ctx)))
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!(
            "{} run … {} llama-server (три слота, общий кэш, окно не ниже {})",
            ctx.engine(),
            ctx.cfg.llama_image(),
            ctx.cfg.model.fit_ctx
        ))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let token = llama::ensure_token(ctx)?;
        llama::write_env_file(&token)?;
        let _ = docker::run(ctx, &["rm", "-f", &ctx.cfg.llama_container()]);
        let args = self.wanted(ctx);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let o = docker::run(ctx, &refs)?;
        anyhow::ensure!(o.ok(), "контейнер модели не запустился: {}", o.tail(3).join(" "));
        ctx.kit.state_set("llama.args", &args.join(" "))?;
        ctx.ok("контейнер модели запущен");
        Ok(())
    }
    fn revert(&self, ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        Some(vec![Action::new(format!("{} rm -f {}", ctx.engine(), ctx.cfg.llama_container()))])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        let _ = docker::run(ctx, &["rm", "-f", &ctx.cfg.llama_container()]);
        ctx.ok("контейнер модели снят");
        Ok(())
    }
}

impl Container {
    fn wanted(&self, ctx: &Ctx) -> Vec<String> {
        let used = llama::gpu(ctx).map(|g| g.used_mb).unwrap_or(0);
        let env = llama::env_file().to_string_lossy().to_string();
        llama_run_args(&ctx.cfg, &env, &llama::bind_host(ctx), llama::cache_ram_mb(llama::free_ram_mb(ctx)), llama::fit_target_mb(used))
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
    fn state(&self, ctx: &Ctx) -> OpState {
        // Сервер не может подняться — замеру нечего ждать пять минут.
        let server: Box<dyn Op<Ctx>> = match ctx.cfg.form() {
            Form::Docker => Box::new(Container),
            _ => Box::new(Service),
        };
        match server.state(ctx) {
            OpState::Blocked(why) => OpState::Blocked(format!("сервер модели не поднимается: {why}")),
            _ => OpState::Missing,
        }
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
    fn the_container_form_runs_the_server_from_the_models_volume() {
        let mut c = Cfg::default();
        c.model.serve = "docker".into();
        c.model.file = "models/x.gguf".into();
        let a = llama_run_args(&c, "/k/llama.env", "127.0.0.1", 4096, 1024).join(" ");
        assert!(a.starts_with("run -d --name dev1c-llama --restart unless-stopped --gpus all -p 127.0.0.1:3392:8080 -v dev1c-models:/opt/models --env-file /k/llama.env oktaplus/dev1c-llama:latest llama-server"), "{a}");
        assert!(a.contains("-m /opt/models/x.gguf --alias x --host 0.0.0.0 --port 8080"));
        assert!(!a.contains("--api-key-file") && !a.contains("LLAMA_API_KEY="), "токен не в аргументах");
        let ids = ops(Form::Docker).iter().map(|o| o.id().to_string()).collect::<Vec<_>>();
        assert_eq!(ids, ["llama.container", "llama.answers"]);
    }

    #[test]
    fn foreign_files_in_the_service_dir_are_pruned_but_ours_stay() {
        let keep = |n: &str| keep_in_service_dir(n, &["llama-server.exe".to_string(), "cudart64_13.dll".to_string()]);
        assert!(keep("llama-server.exe") && keep("cudart64_13.dll"));
        assert!(keep("nssm.exe") && keep("dev1c-llama.token") && keep("llama.env") && keep("llama.log") && keep("llama-20261016.log"));
        assert!(!keep("evil.dll") && !keep("version.dll"));
    }

    #[test]
    fn baked_arguments_are_compared_as_one_line() {
        // Служба запекает параметры: разошлись — пересоздание.
        assert!(args_drift(&["a".into(), "b".into()], "a b").is_none());
        let (from, to) = args_drift(&["a".into(), "c".into()], "a b").unwrap();
        assert!(from.contains("a b") && to.contains("a c"));
        // Кэш подсказок считается от свободной памяти и гуляет между запусками:
        // это настройка, а не тождество службы, и расхождением не считается.
        let want: Vec<String> = ["-m", "x", "--cache-ram", "3072", "--port", "1"].iter().map(|s| s.to_string()).collect();
        assert!(args_drift(&want, "-m x --cache-ram 8192 --port 1").is_none());
        assert!(args_drift(&want, "-m x --cache-ram 8192 --port 2").is_some());
    }

    #[test]
    fn the_measurement_is_blocked_when_the_server_step_is() {
        // Пустой file: служба заблокирована, и замер не должен пять минут
        // стучаться в сервер, которого нет, обрывая установку.
        let c = Cfg::default();
        let dir = std::env::temp_dir().join(format!("dev1c-answers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = Ctx { kit: common::kit::Kit::at(&dir), cfg: c, dry_run: false, sink: std::sync::Arc::new(|_| {}), prompt: Box::new(common::AlwaysDefault) };
        let st = Answers.state(&ctx);
        assert!(matches!(st, OpState::Blocked(ref w) if w.contains("модель")), "{st:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn service_arguments_with_spaces_are_quoted_for_nssm() {
        let line = llama::quote_args(&["-m".into(), r"C:\Program Files\m\x.gguf".into(), "--port".into(), "1236".into()]);
        assert_eq!(line, r#"-m "C:\Program Files\m\x.gguf" --port 1236"#);
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
