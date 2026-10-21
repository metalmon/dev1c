//! Операции сервера модели на этой машине (serve = "host"): файлы из
//! комплекта, служба, замер. Каждая умеет объясниться: без карты, без
//! модели, без nssm человек узнаёт причину, а не «шагов: 0».

use crate::barrier;
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
    let host = llama::bind_host(ctx);
    let token = llama::token_path(&ctx.kit, ctx.cfg.form()).to_string_lossy().to_string();
    let alias = ctx.cfg.model_alias();
    llama::serve_args(&llama::ServeInput {
        model: &ctx.cfg.model.file,
        alias: &alias,
        host: &host,
        port: ctx.cfg.model.port,
        slot_floor: ctx.cfg.model.fit_ctx,
        // Предел не узнан — кэш остаётся на полу: лучше потеря настройки, чем
        // число, выдуманное от неизвестной памяти.
        cache_ram: llama::cache_ram_from_limit(llama::limit_mb(ctx).unwrap_or(0)),
        token_file: Some(&token),
    })
}

/// Раздел памяти словами, оба числа: молча неверный раздел — это и есть
/// поломка (на стенде среде 8 ГБ, модели 22 ГБ), а названный человек увидит
/// и поправит до установки.
fn memory_action(ctx: &Ctx) -> Action {
    let env = llama::env_limit_mb(llama::engine_env_limit_mb(ctx), &ctx.cfg.server.memory);
    match llama::limit_mb(ctx) {
        Some(l) => Action::new(format!(
            "память: контейнеру среды {env} МиБ, серверу модели {l} МиБ (кэш подсказок {} МиБ, с подкачкой до {} МиБ)",
            llama::cache_ram_from_limit(l),
            l as u64 * 2
        )),
        None => Action::new(format!(
            "память: контейнеру среды {env} МиБ, а сколько её у машины — не ответила; предел серверу модели не ставится, кэш подсказок {} МиБ",
            llama::cache_ram_from_limit(0)
        )),
    }
}

/// Параметры запуска без `--cache-ram`: он считается от предела и может
/// гулять между запусками (предел не узнан — пол), а расхождение должно
/// означать настоящее изменение.
///
/// `drop_memory` — вычеркнуть и предел памяти. Так сравниваются строки, когда
/// предела в желаемой нет вовсе: машина не ответила, сколько у неё памяти.
/// Незнание — не причина пересоздавать работающий контейнер и уж точно не
/// причина снять с него предел: контейнер без предела и есть та беда, ради
/// которой всё это считается (кэш подсказок выдавит в подкачку машину вместе
/// с 1С). Предел, который администратор правда сменил, в желаемой строке
/// есть — и расхождением остаётся.
fn significant(line: &str, drop_memory: bool) -> String {
    const MEMORY: [&str; 3] = ["--memory", "--memory-swap", "--memory-reservation"];
    let mut out = Vec::new();
    let mut it = line.split_whitespace();
    while let Some(w) = it.next() {
        if w == "--cache-ram" || (drop_memory && MEMORY.contains(&w)) {
            it.next();
            continue;
        }
        out.push(w);
    }
    out.join(" ")
}

pub fn args_drift(want: &[String], baked: &str) -> Option<(String, String)> {
    let w = want.join(" ");
    let limit_unknown = !want.iter().any(|a| a == "--memory");
    (significant(&w, limit_unknown) != significant(baked, limit_unknown))
        .then(|| (format!("служба: {baked}"), format!("служба: {w}")))
}

/// Что из окна сервера следует для Zed. `window` — контекст ОДНОГО слота, как
/// его отдаёт `/props` (`slot_n_ctx`, не пул: `server-context.cpp:1355`), а
/// `slots` нужен только для внятного сообщения.
///
/// Сервер дал меньше, чем просили, — провал с обоими числами: молча поставить
/// окно меньше нельзя, человек, просивший 250 000, о подмене не узнает.
pub fn verdict(cfg: &crate::cfg::Cfg, window: u32, slots: u16) -> Result<u32> {
    // Сравниваем с тем числом, которое человек написал в настройках, а не с
    // выровненным: выравнивание живёт на стороне ЗАПРОСА (`slot_floor_aligned`
    // в serve_args), и сервер поэтому всегда отдаёт не меньше. Сравнение с
    // округлённым вверх полом отказывало бы серверу, отдавшему ровно
    // запрошенное.
    anyhow::ensure!(
        window >= cfg.model.fit_ctx,
        "сервер дал {window} токенов на слот при нижней границе fit_ctx = {} ({} слота): на этой карте модель с такими параметрами не помещается",
        cfg.model.fit_ctx,
        slots
    );
    // Одно число вместо трёх: окно агента. Со штатным провайдером `llama.cpp`
    // Zed забирает окно с `/props` и сам делит его на вход и ответ, поэтому
    // сходиться должно только то, что от нас зависит, — хватает ли окна
    // агенту на вход и ответ вместе.
    let per_agent = llama::per_agent(window, cfg.model.agents);
    anyhow::ensure!(
        per_agent >= llama::MIN_BUDGET + llama::ANSWER_FLOOR,
        "окно {window} на {} агентов даёт {per_agent} токенов на агента — для кодинга на 1С нужно хотя бы {} (вход {} и ответ {})",
        cfg.model.agents,
        llama::MIN_BUDGET + llama::ANSWER_FLOOR,
        llama::MIN_BUDGET,
        llama::ANSWER_FLOOR
    );
    Ok(per_agent)
}

/// Строка замера. `window` — контекст слота от сервера, `per_agent` — его доля
/// на одного агента. Чисел для Zed в строке больше нет: редактор забирает окно
/// с `/props` сам, и объявлять ему нечего.
fn answers_line(window: u32, slots: u16, model: &str, per_agent: u32) -> String {
    format!("сервер отвечает: {model}, {slots} слота по {window} токенов; на агента {per_agent}")
}

/// Что остаётся в каталоге службы при раскладке из комплекта: файлы
/// комплекта, nssm, токен и журналы. Всё остальное удаляется — подложенная
/// заранее библиотека загрузилась бы в процесс службы.
pub fn keep_in_service_dir(name: &str, kit_files: &[String]) -> bool {
    kit_files.iter().any(|k| k == name)
        || name == "nssm.exe"
        || name == "dev1c-llama.token"
        || (name.starts_with("llama") && name.ends_with(".log"))
}

/// `docker run` контейнера модели: образ из комплекта, том моделей, карта,
/// токен переменной из файла в закрытом каталоге (в аргументах его нет),
/// порт наружу — только на адрес, который видят контейнеры и эта машина
/// (петля на Windows, мост на Linux); внутри сервер слушает 8080.
///
/// Предел памяти — свой: иначе кэш подсказок выдавливает в подкачку всю
/// машину, включая 1С. `--memory-swap` вдвое больше предела — переполнение
/// уезжает в файл подкачки хоста, а не убивает сервер со всеми сеансами;
/// запрещать подкачку здесь означало бы менять задержку редкого сеанса на
/// гибель всех. `--memory-reservation` — половина: под давлением движок
/// сжимает контейнер до неё, а не убивает.
///
/// `limit_mb` = None — машина не ответила, сколько у неё памяти: пределов не
/// ставим вовсе. Пол в этом месте выглядел бы как решение, а на деле это
/// 4 ГБ работающему серверу и пересоздание контейнера на пустом месте.
pub fn llama_run_args(cfg: &crate::cfg::Cfg, env_file: &str, publish_host: &str, limit_mb: Option<u32>) -> Vec<String> {
    let base = cfg.model.file.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
    let model = format!("/opt/models/{base}");
    let alias = cfg.model_alias();
    let cache_ram = llama::cache_ram_from_limit(limit_mb.unwrap_or(0));
    let mut a: Vec<String> = [
        "run",
        "-d",
        "--name",
        &cfg.llama_container(),
        "--restart",
        "unless-stopped",
        "--gpus",
        "all",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(l) = limit_mb {
        a.extend(
            ["--memory", &format!("{l}m"), "--memory-swap", &format!("{}m", l as u64 * 2), "--memory-reservation", &format!("{}m", l / 2)]
                .iter()
                .map(|s| s.to_string()),
        );
    }
    a.extend(
        [
            "-p",
            &format!("{publish_host}:{}:8080", cfg.ports()[2]),
            "-v",
            &format!("{}:/opt/models", cfg.volume("models")),
            "--env-file",
            env_file,
            &cfg.llama_image(),
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    a.extend(llama::serve_args(&llama::ServeInput {
        model: &model,
        alias: &alias,
        host: "0.0.0.0",
        port: 8080,
        slot_floor: cfg.model.fit_ctx,
        cache_ram,
        token_file: None,
    }));
    a
}

/// Два файла с токеном, которые установка пишет сама (`llama::ensure_token` +
/// `llama::write_env_file`), — командами оболочки: токен в своём файле (его
/// читает `check`) и тот же токен переменной LLAMA_API_KEY для контейнера.
/// Ручной путь без них не доходит до конца: сервер отвечает 401, и `check`
/// проваливает четыре факта на здоровой машине. Печатаются они здесь, вместе
/// с остальными командами, а не рядом со страницей руками — иначе разошлись
/// бы с кодом при первой же правке имён файлов.
pub fn token_commands(ctx: &Ctx) -> Vec<String> {
    let token = llama::token_path(&ctx.kit, ctx.cfg.form()).to_string_lossy().to_string();
    let env = llama::env_path(&ctx.kit).to_string_lossy().to_string();
    vec![
        format!("sh -c 'tr -dc a-f0-9 </dev/urandom | head -c 32 > {token} && chmod 600 {token}'"),
        format!("sh -c 'printf \"LLAMA_API_KEY=%s\\n\" \"$(cat {token})\" > {env} && chmod 600 {env}'"),
    ]
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
            return OpState::Blocked("в комплекте нет llama/bin — скопируйте полный комплект заново".into());
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
            Action::new(format!("три слота, общий кэш, окно подбирает сервер (не ниже {} на слот)", ctx.cfg.model.fit_ctx)),
            memory_action(ctx),
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
        let name = ctx.cfg.llama_container();
        let container_state = docker::container_state(ctx, &name);
        let running = container_state == "running";
        // Драйвер на хосте — дешёвая проверка (nvidia-smi), и без неё дальше
        // смотреть нечего: без драйвера карту не увидит и контейнер.
        let driver_ok = llama::gpu(ctx).is_some();
        // Настоящий запуск образа с --gpus all — дорогая проверка (NVIDIA
        // runtime поднимает и роняет контейнер на каждый вызов): не нужна без
        // драйвера (ответ и так ясен) и не нужна, когда наш контейнер уже
        // работает — значит, карта до него уже дотянулась. Пробуем только
        // перед тем, как контейнер собираются создавать, а не как пульс на
        // каждый status.
        let container_ok = driver_ok
            && (running || docker::ask(ctx, &["run", "--rm", "--gpus", "all", &ctx.cfg.llama_image(), "--version"]).map(|o| o.ok()).unwrap_or(false));
        if let Some(msg) = barrier::gpu_in_container(driver_ok, container_ok, ctx.kit.path("nvidia-container-toolkit").is_dir()) {
            return OpState::Blocked(msg);
        }
        if let Some(st) = crate::ops::state_before_drift(&container_state) {
            return st;
        }
        let baked = ctx.kit.state_get("llama.args").unwrap_or_default();
        if let Some(d) = args_drift(&self.wanted(ctx), &baked) {
            return OpState::Drifted { from: d.0, to: d.1 };
        }
        OpState::Done(format!("работает, порт {} на {}", ctx.cfg.ports()[2], llama::bind_host(ctx)))
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![
            Action::new(format!(
                "{} run … {} (три слота, общий кэш, окно не ниже {} на слот)",
                ctx.engine(),
                ctx.cfg.llama_image(),
                ctx.cfg.model.fit_ctx
            )),
            memory_action(ctx),
        ]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        // В режиме показа команда только печатается: токен, файл переменных и
        // запомненные параметры тогда не нужны — и не должны появляться в
        // комплекте от одной печати плана (`install --dry-run --commands`).
        // Но ручному пути они нужны, и там это шаг, а не файл: печатаем его
        // командами (`token_commands`) на том же месте, где установка пишет
        // файлы сама — перед запуском контейнера.
        if ctx.dry_run {
            for c in token_commands(ctx) {
                ctx.emit(common::Event::Command(c));
            }
        } else {
            let token = llama::ensure_token(ctx)?;
            llama::write_env_file(&ctx.kit, &token)?;
        }
        let _ = docker::run(ctx, &["rm", "-f", &ctx.cfg.llama_container()]);
        let args = self.wanted(ctx);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let o = docker::run(ctx, &refs)?;
        anyhow::ensure!(o.ok(), "контейнер модели не запустился: {}", o.tail(3).join(" "));
        if ctx.dry_run {
            return Ok(());
        }
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
            let env = llama::env_path(&ctx.kit).to_string_lossy().to_string();
        llama_run_args(&ctx.cfg, &env, &llama::bind_host(ctx), llama::limit_mb(ctx))
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
        let per_agent = verdict(&ctx.cfg, p.window, p.slots)?;
        ctx.ok(answers_line(p.window, p.slots, &p.model, per_agent));
        // Размер KV на токен — число этой модели, а не таблицы: дальше он
        // считает и то, что помещается в карту, и то, сколько нужно подкачки.
        if let Some(mb) = llama::kv_cache_mb_from_log(&llama::server_log(ctx)) {
            // Журнал печатает размер ВСЕГО кэша, а /props — долю слота:
            // делим на общее окно, иначе байты на токен выйдут втрое больше.
            let whole = llama::total_window(p.window, p.slots);
            let kv = llama::kv_bytes_per_token(mb, whole);
            ctx.kit.state_set("llama.kv_bytes_per_token", &kv.to_string())?;
            let fits = llama::window_that_fits(llama::vram_for_kv_mb(ctx, kv, whole), kv, p.slots);
            let sessions = p.slots.max(1) * llama::WARM_PER_SLOT;
            let warm = llama::warm_sessions_mb(kv, per_agent, sessions);
            ctx.ok(format!(
                "KV-кэш {mb} МиБ на {} токенов всех слотов — {kv} байт на токен; карта держит {fits} токенов на слот, {sessions} тёплых сеансов по {} — {warm} МиБ оперативной памяти и подкачки",
                whole, per_agent
            ));
        } else {
            ctx.warn("размер KV-кэша не прочитан из журнала сервера: проверки окна по видеопамяти и подкачке будут пропущены");
        }
        let prefill = llama::measure_prefill(&url, &token);
        match prefill {
            Some(s) => ctx.ok(format!("скорость разбора {s:.0} токенов в секунду")),
            None => ctx.warn("скорость разбора замерить не вышло (ответ без timings)"),
        }
        llama::facts_save(ctx, &Facts { token, window: p.window }, prefill)?;
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
        let a = llama_run_args(&c, "/k/llama.env", "127.0.0.1", Some(22528)).join(" ");
        assert!(a.starts_with("run -d --name dev1c-llama --restart unless-stopped --gpus all --memory 22528m --memory-swap 45056m --memory-reservation 11264m -p 127.0.0.1:3392:8080 -v dev1c-models:/opt/models --env-file /k/llama.env oktaplus/dev1c-llama:latest -m /opt/models/x.gguf"), "{a}");
        // Кэш подсказок — половина предела контейнера, с потолком 8 ГиБ. При
        // 22528 МиБ это ровно 8192, то есть умолчание сервера, и флага в
        // строке быть не должно (решение 2026-10-21).
        assert!(!a.contains("--cache-ram"), "{a}");
        // Машина не ответила, сколько у неё памяти: пределов нет вовсе —
        // пол выглядел бы как решение и пересоздавал бы живой контейнер.
        let none = llama_run_args(&c, "/k/llama.env", "127.0.0.1", None).join(" ");
        assert!(!none.contains("--memory"), "{none}");
        assert!(none.contains("--cache-ram 1024"), "{none}");
        // The image now has an ENTRYPOINT; a bare "llama-server" after the image
        // name would land as a stray argument the server does not understand.
        assert!(!a.contains("latest llama-server"), "{a}");
        assert!(a.contains("-m /opt/models/x.gguf --alias x --host 0.0.0.0 --port 8080"));
        assert!(!a.contains("--api-key-file") && !a.contains("LLAMA_API_KEY="), "токен не в аргументах");
        let ids = ops(Form::Docker).iter().map(|o| o.id().to_string()).collect::<Vec<_>>();
        assert_eq!(ids, ["llama.container", "llama.answers"]);
    }

    #[test]
    fn foreign_files_in_the_service_dir_are_pruned_but_ours_stay() {
        let keep = |n: &str| keep_in_service_dir(n, &["llama-server.exe".to_string(), "cudart64_13.dll".to_string()]);
        assert!(keep("llama-server.exe") && keep("cudart64_13.dll"));
        assert!(keep("nssm.exe") && keep("dev1c-llama.token") && keep("llama.log") && keep("llama-20261016.log"));
        assert!(!keep("evil.dll") && !keep("version.dll"));
        // llama.env живёт в комплекте (llama::env_path), а не в каталоге
        // службы: беречь его здесь было нечего.
        assert!(!keep("llama.env"));
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
        // Машина не ответила, сколько у неё памяти: предела в желаемой строке
        // нет. Это не повод пересоздать работающий контейнер — и тем более не
        // повод оставить его совсем без предела.
        let baked_limited = "run --memory 22528m --memory-swap 45056m --memory-reservation 11264m -m x --cache-ram 8192 --port 1";
        let unknown: Vec<String> = ["run", "-m", "x", "--cache-ram", "1024", "--port", "1"].iter().map(|s| s.to_string()).collect();
        assert!(args_drift(&unknown, baked_limited).is_none(), "неизвестный предел — не расхождение");
        // А настоящая смена предела расхождением остаётся.
        let changed: Vec<String> =
            ["run", "--memory", "6144m", "--memory-swap", "12288m", "--memory-reservation", "3072m", "-m", "x", "--cache-ram", "3072", "--port", "1"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let (from, to) = args_drift(&changed, baked_limited).unwrap();
        assert!(from.contains("22528m") && to.contains("6144m"), "{from} → {to}");
        // И контейнер, у которого предела ещё нет, его получает.
        assert!(args_drift(&changed, "run -m x --cache-ram 3072 --port 1").is_some());
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
        // Слот дал меньше запрошенного — провал с обоими числами, а не молча
        // поставленное окно поменьше.
        assert!(verdict(&c, 36408, 3).is_err());
        let e = verdict(&c, 36408, 3).unwrap_err().to_string();
        assert!(e.contains("65536") && e.contains("36408"), "{e}");
        // Стенд: сервер отдал 250k на слот — на одного агента это весь слот.
        c.model.fit_ctx = 250_000;
        assert_eq!(verdict(&c, 250_000, 3).unwrap(), 250_000);
        // Слот, которого не хватает даже на один модуль 1С, — отказ по окну
        // агента, а не по окну слота: 49152 на три агента это 16384, то есть
        // на вход и ответ вместе не хватает.
        c.model.fit_ctx = 16384;
        c.model.agents = 3;
        let e = verdict(&c, 49152, 3).unwrap_err().to_string();
        assert!(e.contains("16384"), "{e}");
    }

    #[test]
    fn the_window_the_server_reports_always_passes_the_verdict() {
        // Что сервер дал на слот, то и объявляется: отказ возможен только по
        // нижней границе fit_ctx или по слишком малому входу, но не из-за
        // нашей арифметики — её больше нет.
        let mut c = Cfg::default();
        c.model.agents = 1;
        // Ниже 24576 окно нерабочее по построению (вход и ответ), и такие
        // значения не пускает уже cfg::validate — здесь только рабочие.
        for fit in [24_576u32, 65_536, 65_537, 131_072, 250_000, 262_144] {
            c.model.fit_ctx = fit;
            let per = verdict(&c, fit, llama::SLOTS).unwrap_or_else(|e| panic!("fit_ctx {fit}: {e}"));
            assert_eq!(per, fit, "fit_ctx {fit}");
            assert!(per >= llama::MIN_BUDGET + llama::ANSWER_FLOOR, "окно агента обязано вмещать вход и ответ");
        }
    }

    #[test]
    fn the_manual_path_gets_the_token_step_with_the_same_key_in_both_files() {
        // Ручной путь собирается из печатаемых команд, и токен в нём — шаг, а
        // не файл из комплекта: один и тот же ключ в dev1c-llama.token (его
        // читает check) и в llama.env (его получает контейнер).
        let mut c = Cfg::default();
        c.model.serve = "docker".into();
        let dir = std::env::temp_dir().join(format!("dev1c-token-cmds-{}", std::process::id()));
        let ctx = Ctx { kit: common::kit::Kit::at(&dir), cfg: c, dry_run: true, sink: std::sync::Arc::new(|_| {}), prompt: Box::new(common::AlwaysDefault) };
        let cmds = token_commands(&ctx);
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        let token = llama::token_path(&ctx.kit, Form::Docker).to_string_lossy().to_string();
        assert!(cmds[0].contains(&token) && cmds[0].contains("chmod 600"), "{:?}", cmds[0]);
        // Второй файл берёт ключ из первого, а не придумывает свой.
        assert!(cmds[1].contains(&format!("cat {token}")), "{:?}", cmds[1]);
        assert!(cmds[1].contains("LLAMA_API_KEY=%s") && cmds[1].contains("llama.env"), "{:?}", cmds[1]);
    }

    #[test]
    fn the_measurement_line_names_the_slot_window_and_the_agent_share() {
        // Чисел для Zed в строке больше нет — объявлять их некому; но доля
        // агента обязана стоять рядом с окном слота, иначе при agents > 1
        // администратор сверяет не то с не тем.
        let slot = 250_000;
        let line = answers_line(slot, llama::SLOTS, "x.gguf", llama::per_agent(slot, 3));
        assert!(line.contains("3 слота по 250000"), "{line}");
        assert!(line.contains(&format!("на агента {}", 250_000 / 3)), "{line}");
        assert!(!line.contains("вход") && !line.contains("запас"), "{line}");
    }
}
