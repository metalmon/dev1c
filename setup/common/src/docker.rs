//! Разговор с движком контейнеров. Имя движка — параметр контекста (`docker`
//! или `podman`). Сам запуск — общий (`crate::proc`), здесь только глаголы.

use crate::proc::{exec, probe, Out};
use crate::Runner;
use anyhow::Result;

/// Изменяющая команда движка.
pub fn run<X: Runner>(ctx: &X, args: &[&str]) -> Result<Out> {
    exec(ctx, &ctx.engine(), args)
}

/// Вопрос движку: выполняется и в режиме показа.
pub fn ask<X: Runner>(ctx: &X, args: &[&str]) -> Result<Out> {
    probe(ctx, &ctx.engine(), args)
}

pub fn available<X: Runner>(ctx: &X) -> bool {
    ask(ctx, &["version", "--format", "{{.Server.Version}}"]).map(|o| o.ok()).unwrap_or(false)
}

pub fn version<X: Runner>(ctx: &X) -> Option<String> {
    ask(ctx, &["version", "--format", "{{.Server.Version}}"]).ok().filter(|o| o.ok()).map(|o| o.text.trim().to_string())
}

pub fn image_loaded<X: Runner>(ctx: &X, name: &str) -> bool {
    ask(ctx, &["images", "-q", name]).map(|o| o.ok() && !o.text.trim().is_empty()).unwrap_or(false)
}

pub fn volume_exists<X: Runner>(ctx: &X, name: &str) -> bool {
    ask(ctx, &["volume", "ls", "--format", "{{.Name}}"]).map(|o| o.text.lines().any(|l| l.trim() == name)).unwrap_or(false)
}

/// Состояние контейнера по имени (`running`, `exited`…); пусто, если его нет.
pub fn container_state<X: Runner>(ctx: &X, name: &str) -> String {
    ask(ctx, &["inspect", "-f", "{{.State.Status}}", name])
        .map(|o| if o.ok() { o.text.trim().to_string() } else { String::new() })
        .unwrap_or_default()
}

/// Одно поле `docker inspect` по шаблону Go; пусто, если объекта нет.
pub fn inspect<X: Runner>(ctx: &X, name: &str, format: &str) -> String {
    ask(ctx, &["inspect", "-f", format, name]).map(|o| if o.ok() { o.text.trim().to_string() } else { String::new() }).unwrap_or_default()
}

pub fn load_image<X: Runner>(ctx: &X, archive: &str) -> Result<Out> {
    run(ctx, &["load", "-i", archive])
}

/// Одноразовый контейнер с примонтированным ТОМОМ (движок создаст том сам).
pub fn sh_in_volume<X: Runner>(ctx: &X, image: &str, volume: &str, mount: &str, script: &str) -> Result<Out> {
    let vol = format!("{volume}:{mount}");
    run(ctx, &["run", "--rm", "-v", &vol, "--entrypoint", "sh", image, "-c", script])
}

/// То же, но для чтения: выполняется и в режиме показа.
pub fn sh_in_volume_ro<X: Runner>(ctx: &X, image: &str, volume: &str, mount: &str, script: &str) -> Result<Out> {
    let vol = format!("{volume}:{mount}");
    ask(ctx, &["run", "--rm", "-v", &vol, "--entrypoint", "sh", image, "-c", script])
}
