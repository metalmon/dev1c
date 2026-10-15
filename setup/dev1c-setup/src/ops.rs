//! Операции сервера: образ, тома, модель, контейнер, службы. Аргументы
//! `docker run` переехали сюда из dev1c.ps1 вместе с причинами.

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
    fn engine(&self) -> String {
        self.cfg.server.engine.clone()
    }
}

/// Аргументы `docker run`. Те же, что были в dev1c.ps1, по тем же причинам:
/// --init — настоящий init как PID 1, иначе xrdp не жнёт сирот; SYS_ADMIN и
/// seccomp/apparmor unconfined — xrdp-sesman открывает PAM-сессию; /dev/fuse —
/// проброс дисков Windows по RDP идёт через FUSE; shm 2g — браузер и Zed.
pub fn run_args(cfg: &Cfg) -> Vec<String> {
    let s = |x: &str| x.to_string();
    let [rdp, gitea, model, ssh] = cfg.ports();
    let mut a = vec![
        s("run"),
        s("-d"),
        s("--name"),
        cfg.server.name.clone(),
        s("--restart"),
        s("unless-stopped"),
        s("--init"),
        s("-p"),
        format!("{rdp}:3389"),
        s("-p"),
        format!("{gitea}:3000"),
        s("-p"),
        format!("{model}:8080"),
        s("-p"),
        format!("{ssh}:2022"),
        s("--shm-size"),
        s("2g"),
        s("--memory"),
        cfg.server.memory.clone(),
        s("--cap-add"),
        s("SYS_ADMIN"),
        s("--security-opt"),
        s("seccomp=unconfined"),
        s("--security-opt"),
        s("apparmor=unconfined"),
        s("--device"),
        s("/dev/fuse"),
        s("-v"),
        format!("{}:/home", cfg.volume("home")),
        s("-v"),
        format!("{}:/var/lib/gitea", cfg.volume("gitea")),
        s("-v"),
        format!("{}:/opt/models", cfg.volume("models")),
    ];
    if cfg.server.gpu {
        // Флаги GPU у движков разные.
        if cfg.server.engine == "podman" {
            a.extend([s("--device"), s("nvidia.com/gpu=all")]);
        } else {
            a.extend([s("--gpus"), s("all")]);
        }
    }
    for (k, v) in cfg.container_env() {
        a.extend([s("-e"), format!("{k}={v}")]);
    }
    a.push(cfg.image());
    a
}

/// Чем работающий контейнер отличается от заданного: образ, первый порт,
/// переменные. `None` — ничем. Чистая функция ради теста; значения ей даёт
/// `docker inspect`.
pub fn drift_of(cfg: &Cfg, image: &str, first_port: &str, env: &[String]) -> Option<(String, String)> {
    if image != cfg.image() {
        return Some((format!("образ {image}"), format!("образ {}", cfg.image())));
    }
    let want_port = cfg.server.port.to_string();
    if first_port != want_port {
        return Some((format!("порт {first_port}"), format!("порт {want_port}")));
    }
    for (k, v) in cfg.container_env() {
        let want = format!("{k}={v}");
        let have = env.iter().find(|e| e.starts_with(&format!("{k}=")));
        if have != Some(&want) {
            let have = have.cloned().unwrap_or_else(|| format!("{k} не задан"));
            // Ключ в тексте состояния не показываем.
            let hide = |s: String| if k == "CUSTOM_API_KEY" { format!("{k}=…") } else { s };
            return Some((hide(have), hide(want)));
        }
    }
    None
}

/// Состояние контейнера до сравнения с заданным. `created` — след неудачного
/// `docker run` (например, `--gpus all` без видеокарты): это «не сделано», а
/// не «разошлось», иначе план предлагал бы чинить то, что не запускалось.
pub fn state_before_drift(status: &str) -> Option<OpState> {
    match status {
        "" | "created" => Some(OpState::Missing),
        "running" => None,
        other => Some(OpState::Drifted { from: other.to_string(), to: "running".into() }),
    }
}

/// Контейнер собран из образа, который уже не тот, что загружен под тегом:
/// так выглядит новый комплект поверх старой среды.
pub fn image_drift(container_image_id: &str, loaded_image_id: &str) -> Option<(String, String)> {
    if loaded_image_id.is_empty() || container_image_id == loaded_image_id {
        return None;
    }
    Some((
        format!("контейнер из старого образа {}", short_id(container_image_id)),
        format!("загружен новый образ {}", short_id(loaded_image_id)),
    ))
}

fn short_id(id: &str) -> String {
    id.trim_start_matches("sha256:").chars().take(12).collect()
}

/// Отпечаток архива образа в комплекте: размер и время правки. Хватает, чтобы
/// заметить новый комплект, и не требует читать полтора гигабайта на каждый
/// `status`.
fn archive_stamp(path: &std::path::Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{}:{t}", m.len()))
}

struct Env;
impl Op<Ctx> for Env {
    fn id(&self) -> &'static str {
        "env"
    }
    fn title(&self) -> &'static str {
        "Окружение"
    }
    fn always_run(&self) -> bool {
        true
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        if !docker::available(ctx) {
            return OpState::Blocked(format!("{} не отвечает: запустите Docker Desktop или установите движок", ctx.engine()));
        }
        OpState::Missing
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!(
            "проверить движок, файл параметров и порты {}–{}",
            ctx.cfg.ports()[0],
            ctx.cfg.ports()[3]
        ))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        match docker::version(ctx) {
            Some(v) => ctx.ok(format!("{} {v}", ctx.engine())),
            None => anyhow::bail!("{} не отвечает", ctx.engine()),
        }
        if !ctx.kit.path(crate::cfg::FILE).exists() {
            ctx.warn(format!("{} нет в комплекте — действуют значения по умолчанию", crate::cfg::FILE));
        }
        // Порт занят не нашим контейнером — установка упрётся в docker run с
        // невнятной ошибкой; лучше сказать здесь и назвать выход.
        let ours = docker::container_state(ctx, &ctx.cfg.server.name) == "running";
        for p in ctx.cfg.ports() {
            if !ours && std::net::TcpListener::bind(("0.0.0.0", p)).is_err() {
                anyhow::bail!("порт {p} занят другой программой — задайте другой блок: dev1c-setup change port=4000");
            }
        }
        ctx.ok(format!("порты {}–{} свободны или наши", ctx.cfg.ports()[0], ctx.cfg.ports()[3]));
        Ok(())
    }
}

struct Image;
impl Op<Ctx> for Image {
    fn id(&self) -> &'static str {
        "image"
    }
    fn title(&self) -> &'static str {
        "Образ среды"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        let archive = ctx.kit.path("dev1c-image.tar.gz");
        let loaded = docker::image_loaded(ctx, &ctx.cfg.image());
        match (loaded, archive_stamp(&archive)) {
            // A new kit over an old environment: the archive is not the one
            // that was loaded last time, so it is loaded again.
            (true, Some(stamp)) if ctx.kit.state_get("IMAGE_ARCHIVE").is_some_and(|s| s != stamp) => {
                OpState::Drifted { from: "образ из прежнего комплекта".into(), to: "образ из этого комплекта".into() }
            }
            (true, _) => OpState::Done(ctx.cfg.image()),
            (false, None) => OpState::Blocked("в комплекте нет dev1c-image.tar.gz".into()),
            (false, Some(_)) => OpState::Missing,
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!("{} load -i dev1c-image.tar.gz (несколько минут)", ctx.engine()))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let archive = ctx.kit.path("dev1c-image.tar.gz");
        let o = docker::load_image(ctx, &archive.to_string_lossy())?;
        anyhow::ensure!(o.ok(), "образ не загрузился: {}", o.tail(3).join(" "));
        if let (false, Some(stamp)) = (ctx.dry_run, archive_stamp(&archive)) {
            ctx.kit.state_set("IMAGE_ARCHIVE", &stamp)?;
        }
        ctx.ok("образ загружен");
        Ok(())
    }
    fn revert(&self, ctx: &Ctx, level: CleanLevel) -> Option<Vec<Action>> {
        (level >= CleanLevel::Data).then(|| vec![Action::new(format!("удалить образ {}", ctx.cfg.image()))])
    }
    fn revert_apply(&self, ctx: &Ctx, level: CleanLevel) -> Result<()> {
        if level >= CleanLevel::Data {
            let _ = docker::run(ctx, &["rmi", &ctx.cfg.image()]);
        }
        Ok(())
    }
}

struct Volumes;
impl Op<Ctx> for Volumes {
    fn id(&self) -> &'static str {
        "volumes"
    }
    fn title(&self) -> &'static str {
        "Тома данных"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        let missing: Vec<String> = ["home", "gitea", "models"]
            .iter()
            .map(|s| ctx.cfg.volume(s))
            .filter(|v| !docker::volume_exists(ctx, v))
            .collect();
        if missing.is_empty() {
            OpState::Done("home, gitea, models".into())
        } else {
            OpState::Missing
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        ["home", "gitea", "models"]
            .iter()
            .map(|s| Action::new(format!("{} volume create {}", ctx.engine(), ctx.cfg.volume(s))))
            .collect()
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        for s in ["home", "gitea", "models"] {
            let v = ctx.cfg.volume(s);
            if !docker::volume_exists(ctx, &v) {
                let o = docker::run(ctx, &["volume", "create", &v])?;
                anyhow::ensure!(o.ok(), "том {v} не создался: {}", o.tail(2).join(" "));
            }
        }
        ctx.ok("тома на месте");
        Ok(())
    }
    fn revert(&self, ctx: &Ctx, level: CleanLevel) -> Option<Vec<Action>> {
        (level >= CleanLevel::Data).then(|| {
            vec![Action::new(format!(
                "удалить тома {}: репозитории, проекты, настройки, модель",
                ctx.cfg.server.name
            ))]
        })
    }
    fn revert_apply(&self, ctx: &Ctx, level: CleanLevel) -> Result<()> {
        if level >= CleanLevel::Data {
            for s in ["home", "gitea", "models"] {
                let _ = docker::run(ctx, &["volume", "rm", &ctx.cfg.volume(s)]);
            }
            ctx.ok("тома удалены");
        }
        Ok(())
    }
}

struct Model;
impl Op<Ctx> for Model {
    fn id(&self) -> &'static str {
        "model"
    }
    fn title(&self) -> &'static str {
        "Файл модели"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        let f = &ctx.cfg.model.file;
        if f.is_empty() {
            return if ctx.cfg.model.url.is_empty() {
                OpState::Blocked("модель не задана: ни file, ни url в [model] — агент будет молчать".into())
            } else {
                OpState::Done(format!("удалённая: {}", ctx.cfg.model.name))
            };
        }
        let src = ctx.kit.path(f);
        let Ok(meta) = std::fs::metadata(&src) else {
            return OpState::Blocked(format!("файла {f} нет в комплекте"));
        };
        let base = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !docker::volume_exists(ctx, &ctx.cfg.volume("models")) {
            return OpState::Missing;
        }
        let o = docker::sh_in_volume_ro(
            ctx,
            &ctx.cfg.image(),
            &ctx.cfg.volume("models"),
            "/m",
            &format!("stat -c %s /m/{base} 2>/dev/null"),
        );
        match o {
            Ok(o) if o.ok() && o.text.trim() == meta.len().to_string() => OpState::Done(base),
            _ => OpState::Missing,
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!(
            "скопировать {} в том {} (файл большой, это долго)",
            ctx.cfg.model.file,
            ctx.cfg.volume("models")
        ))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        // Через работающий контейнер нельзя (его ещё нет), поэтому одноразовый
        // контейнер с томом и docker cp в него. Bind-mount каталога хоста не
        // используется намеренно: на Windows антивирус рвёт чтение больших файлов.
        let src = ctx.kit.path(&ctx.cfg.model.file);
        let vol = format!("{}:/m", ctx.cfg.volume("models"));
        let o = docker::run(ctx, &["create", "--name", "dev1c-model-copy", "-v", &vol, &ctx.cfg.image()])?;
        anyhow::ensure!(o.ok(), "не создать контейнер для копирования: {}", o.tail(2).join(" "));
        let dst = "dev1c-model-copy:/m/".to_string();
        let o = docker::run(ctx, &["cp", &src.to_string_lossy(), &dst]);
        let _ = docker::run(ctx, &["rm", "-f", "dev1c-model-copy"]);
        let o = o?;
        anyhow::ensure!(o.ok(), "модель не скопировалась: {}", o.tail(2).join(" "));
        ctx.ok("модель в томе");
        Ok(())
    }
}

struct Container;
impl Op<Ctx> for Container {
    fn id(&self) -> &'static str {
        "container"
    }
    fn title(&self) -> &'static str {
        "Контейнер среды"
    }
    fn state(&self, ctx: &Ctx) -> OpState {
        let name = &ctx.cfg.server.name;
        if let Some(st) = state_before_drift(&docker::container_state(ctx, name)) {
            return st;
        }
        let image = docker::inspect(ctx, name, "{{.Config.Image}}");
        let port = docker::inspect(ctx, name, "{{(index (index .HostConfig.PortBindings \"3389/tcp\") 0).HostPort}}");
        let env: Vec<String> = docker::inspect(ctx, name, "{{range .Config.Env}}{{println .}}{{end}}")
            .lines()
            .map(|s| s.to_string())
            .collect();
        if let Some((from, to)) = drift_of(&ctx.cfg, &image, &port, &env) {
            return OpState::Drifted { from, to };
        }
        let have_id = docker::inspect(ctx, name, "{{.Image}}");
        let want_id = docker::ask(ctx, &["images", "-q", "--no-trunc", &ctx.cfg.image()])
            .map(|o| o.text.trim().to_string())
            .unwrap_or_default();
        match image_drift(&have_id, &want_id) {
            Some((from, to)) => OpState::Drifted { from, to },
            None => OpState::Done(format!("работает, порты {}–{}", ctx.cfg.ports()[0], ctx.cfg.ports()[3])),
        }
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        let mut v = Vec::new();
        if !docker::container_state(ctx, &ctx.cfg.server.name).is_empty() {
            v.push(Action::new(format!("{} rm -f {} (тома не трогаются)", ctx.engine(), ctx.cfg.server.name)));
        }
        let args = run_args(&ctx.cfg);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        v.push(Action::new(format!("{} {}", ctx.engine(), docker::redact(&refs))));
        v
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        let name = ctx.cfg.server.name.clone();
        let _ = docker::run(ctx, &["rm", "-f", &name]);
        let args = run_args(&ctx.cfg);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let o = docker::run(ctx, &refs)?;
        anyhow::ensure!(
            o.ok(),
            "контейнер не запустился: {}\nДве обычные причины: порты {}–{} заняты (dev1c-setup change port=4000) или нет видеокарты (gpu = false в dev1c.toml)",
            o.tail(3).join(" "),
            ctx.cfg.ports()[0],
            ctx.cfg.ports()[3]
        );
        ctx.ok("контейнер запущен");
        Ok(())
    }
    fn revert(&self, ctx: &Ctx, _level: CleanLevel) -> Option<Vec<Action>> {
        Some(vec![Action::new(format!("{} rm -f {}", ctx.engine(), ctx.cfg.server.name))])
    }
    fn revert_apply(&self, ctx: &Ctx, _level: CleanLevel) -> Result<()> {
        let _ = docker::run(ctx, &["rm", "-f", &ctx.cfg.server.name]);
        ctx.ok("контейнер снят");
        Ok(())
    }
}

struct Services;
impl Op<Ctx> for Services {
    fn id(&self) -> &'static str {
        "services"
    }
    fn title(&self) -> &'static str {
        "Службы внутри"
    }
    fn always_run(&self) -> bool {
        true
    }
    fn state(&self, _ctx: &Ctx) -> OpState {
        OpState::Missing
    }
    fn plan(&self, ctx: &Ctx) -> Vec<Action> {
        vec![Action::new(format!(
            "дождаться gitea, xrdp, sshd{} (до двух минут)",
            if ctx.cfg.model.file.is_empty() { "" } else { ", llama-server" }
        ))]
    }
    fn apply(&self, ctx: &Ctx) -> Result<()> {
        if ctx.dry_run {
            return Ok(());
        }
        let mut want = vec!["gitea", "xrdp", "sshd"];
        if !ctx.cfg.model.file.is_empty() {
            want.push("llama-server");
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let mut missing = Vec::new();
            for s in &want {
                // pgrep -x: exact name, otherwise the probe matches itself.
                let o = docker::ask(ctx, &["exec", &ctx.cfg.server.name, "pgrep", "-x", s])?;
                if !o.ok() {
                    missing.push(*s);
                }
            }
            if missing.is_empty() {
                ctx.ok(format!("работают: {}", want.join(", ")));
                return Ok(());
            }
            if std::time::Instant::now() > deadline {
                anyhow::bail!("не поднялись: {} — смотрите {} logs {}", missing.join(", "), ctx.engine(), ctx.cfg.server.name);
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }
}

/// Операции сервера в порядке применения.
pub fn server() -> Vec<Box<dyn Op<Ctx>>> {
    vec![Box::new(Env), Box::new(Image), Box::new(Volumes), Box::new(Model), Box::new(Container), Box::new(Services)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_args_carry_ports_volumes_and_environment() {
        let mut c = Cfg::default();
        c.server.port = 4000;
        c.model.url = "https://m".into();
        c.model.key = "k".into();
        c.model.name = "Q".into();
        let a = run_args(&c).join(" ");
        assert!(a.starts_with("run -d --name dev1c --restart unless-stopped --init"));
        assert!(a.contains("-p 4000:3389 -p 4001:3000 -p 4002:8080 -p 4003:2022"));
        assert!(a.contains("-v dev1c-home:/home -v dev1c-gitea:/var/lib/gitea -v dev1c-models:/opt/models"));
        assert!(a.contains("--gpus all"));
        assert!(a.contains("-e CUSTOM_API_URL=https://m"));
        assert!(a.contains("-e LLAMA_ENABLED=0"));
        assert!(a.ends_with("oktaplus/dev1c:latest"));
    }

    #[test]
    fn no_gpu_and_podman_change_the_flags() {
        let mut c = Cfg::default();
        c.server.gpu = false;
        let a = run_args(&c).join(" ");
        assert!(!a.contains("--gpus"));
        c.server.gpu = true;
        c.server.engine = "podman".into();
        assert!(run_args(&c).join(" ").contains("--device nvidia.com/gpu=all"));
    }

    #[test]
    fn a_container_left_by_a_failed_run_is_missing_not_drifted() {
        assert_eq!(state_before_drift(""), Some(OpState::Missing));
        assert_eq!(state_before_drift("created"), Some(OpState::Missing));
        assert_eq!(state_before_drift("running"), None);
        assert_eq!(state_before_drift("exited"), Some(OpState::Drifted { from: "exited".into(), to: "running".into() }));
    }

    #[test]
    fn a_newer_image_under_the_same_tag_is_drift() {
        assert_eq!(image_drift("sha256:aaa", "sha256:aaa"), None);
        let d = image_drift("sha256:aaa", "sha256:bbb").unwrap();
        assert!(d.0.contains("старого образа") && d.1.contains("загружен"), "{d:?}");
        // No loaded image at all: nothing to compare, the image op will say so.
        assert_eq!(image_drift("sha256:aaa", ""), None);
    }

    #[test]
    fn drift_names_what_differs() {
        let c = Cfg::default();
        assert_eq!(drift_of(&c, "oktaplus/dev1c:latest", "3390", &[]), None);
        let d = drift_of(&c, "oktaplus/dev1c:old", "3390", &[]).unwrap();
        assert!(d.0.contains("old") && d.1.contains("latest"));
        let d = drift_of(&c, "oktaplus/dev1c:latest", "4000", &[]).unwrap();
        assert!(d.0.contains("4000") && d.1.contains("3390"));
        let mut r = Cfg::default();
        r.model.url = "https://m".into();
        r.model.key = "k".into();
        r.model.name = "Q".into();
        // The first difference is reported, so the variables before CUSTOM_MODEL
        // must match for the model to be the one named.
        let env: Vec<String> = ["CUSTOM_API_URL=https://m", "CUSTOM_API_KEY=k", "CUSTOM_MODEL=Old"].iter().map(|s| s.to_string()).collect();
        let d = drift_of(&r, "oktaplus/dev1c:latest", "3390", &env).unwrap();
        assert!(d.0.contains("CUSTOM_MODEL=Old") && d.1.contains("CUSTOM_MODEL=Q"), "{d:?}");
        // A missing variable is named without its value, and the key never shows.
        let d = drift_of(&r, "oktaplus/dev1c:latest", "3390", &[]).unwrap();
        assert_eq!(d.0, "CUSTOM_API_URL не задан");
        let env: Vec<String> = ["CUSTOM_API_URL=https://m", "CUSTOM_API_KEY=other"].iter().map(|s| s.to_string()).collect();
        let d = drift_of(&r, "oktaplus/dev1c:latest", "3390", &env).unwrap();
        assert_eq!(d, ("CUSTOM_API_KEY=…".to_string(), "CUSTOM_API_KEY=…".to_string()));
    }
}
