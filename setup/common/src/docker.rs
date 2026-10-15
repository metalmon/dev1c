//! Разговор с движком контейнеров. Имя движка — параметр контекста (`docker`
//! или `podman`). Запуск всегда через `Command` с отдельными аргументами: нет
//! строки оболочки — нет её правил цитирования.

use crate::{Event, Runner};
use anyhow::{Context, Result};
use std::process::Command;

pub struct Out {
    pub code: i32,
    pub text: String,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
    /// Последние непустые строки вывода — для сообщений об ошибке.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let lines: Vec<&str> = self.text.lines().map(|l| l.trim_end()).filter(|l| !l.is_empty()).collect();
        lines[lines.len().saturating_sub(n)..].iter().map(|s| s.to_string()).collect()
    }
}

/// Запуск без своего окна консоли: без этого Windows рисует чёрное окно на
/// каждый вызов из оконной программы.
pub fn quiet(mut cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Запустить произвольную программу и собрать её вывод (stdout и stderr вместе).
pub fn exec<X: Runner>(ctx: &X, program: &str, args: &[&str]) -> Result<Out> {
    exec_inner(ctx, program, args, false)
}

/// Спросить машину о состоянии. Выполняется ВСЕГДА, в том числе в режиме
/// показа: иначе `--dry-run` докладывал бы, что ничего не установлено.
pub fn probe<X: Runner>(ctx: &X, program: &str, args: &[&str]) -> Result<Out> {
    exec_inner(ctx, program, args, true)
}

/// Командная строка для журнала и плана: значения ключей и паролей скрыты.
/// Журнал лежит в комплекте, а комплект копируют и пересылают; ключ в нём —
/// утечка, даже если тот же ключ лежит в соседнем toml.
pub fn redact(args: &[&str]) -> String {
    let secret_name = |s: &str| {
        let l = s.to_ascii_lowercase();
        l.contains("key") || l.contains("password")
    };
    let mut out = Vec::with_capacity(args.len());
    let mut hide_next = false;
    for a in args {
        if hide_next {
            out.push("…".to_string());
            hide_next = false;
            continue;
        }
        match a.split_once('=') {
            // NAME=value and --flag=value
            Some((k, _)) if secret_name(k) => out.push(format!("{k}=…")),
            // --flag value: the value is the next argument
            None if a.starts_with('-') && secret_name(a) => {
                hide_next = true;
                out.push(a.to_string());
            }
            // The configurator's /P<password>
            None if a.starts_with("/P") && a.len() > 2 => out.push("/P…".to_string()),
            _ => out.push(a.to_string()),
        }
    }
    out.join(" ")
}

fn exec_inner<X: Runner>(ctx: &X, program: &str, args: &[&str], read_only: bool) -> Result<Out> {
    let shown = format!("{program} {}", redact(args));
    ctx.emit(Event::Command(shown.clone()));
    if ctx.dry_run() && !read_only {
        return Ok(Out { code: 0, text: String::new() });
    }
    let output = quiet(Command::new(program))
        .args(args)
        .output()
        .with_context(|| format!("не удалось запустить {program}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    ctx.emit(Event::Output { command: shown, text: text.clone() });
    Ok(Out { code: output.status.code().unwrap_or(-1), text })
}

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
    ask(ctx, &["version", "--format", "{{.Server.Version}}"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.text.trim().to_string())
}

pub fn image_loaded<X: Runner>(ctx: &X, name: &str) -> bool {
    ask(ctx, &["images", "-q", name]).map(|o| o.ok() && !o.text.trim().is_empty()).unwrap_or(false)
}

pub fn volume_exists<X: Runner>(ctx: &X, name: &str) -> bool {
    ask(ctx, &["volume", "ls", "--format", "{{.Name}}"])
        .map(|o| o.text.lines().any(|l| l.trim() == name))
        .unwrap_or(false)
}

/// Состояние контейнера по имени (`running`, `exited`…); пусто, если его нет.
pub fn container_state<X: Runner>(ctx: &X, name: &str) -> String {
    ask(ctx, &["inspect", "-f", "{{.State.Status}}", name])
        .map(|o| if o.ok() { o.text.trim().to_string() } else { String::new() })
        .unwrap_or_default()
}

/// Одно поле `docker inspect` по шаблону Go; пусто, если объекта нет.
pub fn inspect<X: Runner>(ctx: &X, name: &str, format: &str) -> String {
    ask(ctx, &["inspect", "-f", format, name])
        .map(|o| if o.ok() { o.text.trim().to_string() } else { String::new() })
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_masked_before_anything_is_logged() {
        let args = ["run", "-e", "CUSTOM_API_KEY=sk-secret", "-e", "CUSTOM_MODEL=Q", "--db-password=hunter2"];
        let shown = redact(&args);
        assert_eq!(shown, "run -e CUSTOM_API_KEY=… -e CUSTOM_MODEL=Q --db-password=…");
        // Flag followed by its value, and the configurator's /P<password> switch.
        let args = ["--install", "C:\\b", "--db-user", "mcp", "--db-password", "mcp", "DESIGNER", "/Nmcp", "/Phunter2"];
        assert_eq!(redact(&args), "--install C:\\b --db-user mcp --db-password … DESIGNER /Nmcp /P…");
    }

    #[test]
    fn tail_skips_blank_lines() {
        let o = Out { code: 1, text: "a\n\nb\n  \nc\n".into() };
        assert_eq!(o.tail(2), vec!["b".to_string(), "c".to_string()]);
        assert!(!o.ok());
    }
}
