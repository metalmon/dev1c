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
/// Вывод команд тоже идёт в журнал: `docker inspect` отдаёт переменные
/// контейнера, среди них ключ модели. Значения ключей, токенов и паролей
/// заменяются многоточием построчно.
pub fn redact_text(text: &str) -> String {
    text.lines()
        .map(|l| {
            let name = l.split('=').next().unwrap_or("");
            let upper = name.to_ascii_uppercase();
            if l.contains('=') && (upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("PASSWORD")) {
                format!("{}=…", blot_words(name))
            } else {
                blot_words(l)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Секреты внутри строки: `Basic <base64>`, `/P<пароль>`, `--db-pwd=…`,
/// `--password <…>` — значение заменяется многоточием, остальное остаётся.
/// `/P…` задевает и `/Path` — это строки журнала, не команды; цена принята.
fn blot_words(line: &str) -> String {
    let mut out = Vec::new();
    let mut blot_next = false;
    for w in split_respecting_quotes(line) {
        let blotted = if blot_next {
            blot_next = false;
            "…".to_string()
        } else if w == "Basic" || w == "--password" || w == "--db-pwd" || w == "--db-password" {
            blot_next = true;
            w.to_string()
        } else if w.starts_with("/P") && w.len() > 2 {
            "/P…".to_string()
        } else {
            blot_secrets_in_word(w)
        };
        out.push(blotted);
    }
    out.join(" ")
}

/// Разбивает строку на «слова» как `split(' ')`, но кавычки защищают от
/// разбиения: строка подключения 1С кладёт пароль в кавычки
/// (`Pwd="p w";`), и пробел внутри них — часть значения, а не разделитель
/// между словами. Без этого слово обрывается на первом пробеле, и хвост
/// пароля после него остаётся незатёртым словом без `=` в начале.
fn split_respecting_quotes(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out
}

/// Внутри одного «слова» секрет может быть не единственной парой
/// `ключ=значение`: строка подключения 1С пишет несколько через `;`
/// (`File="…";Usr="…";Pwd="…";`), без пробелов. Каждый сегмент между `;`
/// проверяется отдельно, ключ — без учёта регистра (`Pwd`, `PWD`, `pwd`).
fn blot_secrets_in_word(word: &str) -> String {
    if !word.contains('=') {
        return word.to_string();
    }
    word.split(';')
        .map(|seg| match seg.split_once('=') {
            Some((k, _)) if is_secret_key(k) => format!("{k}=…"),
            _ => seg.to_string(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k.ends_with("pwd") || k.ends_with("password")
}

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
    out.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

/// Аргумент для ПЕЧАТИ: напечатанную команду вставляют в оболочку, и
/// аргумент-скрипт без кавычек меняет смысл — `-c ... < /kit/архив`
/// перенаправит файл на той машине, где команду вставили, а не в контейнере.
/// Простые слова оставляем как есть: строки `docker run` и без того длинные,
/// а кавычки вокруг каждого `-v` читать невозможно.
fn quote(a: &str) -> String {
    // Кавычим только то, что меняет смысл в оболочке: пробел, кавычку,
    // перенаправление, подстановку, шаблон, разделитель команд. Обратная
    // косая (windows-пути конфигуратора) и многоточие маскировки остаются
    // как были: кавычки вокруг них — шум, а не защита.
    let plain = !a.is_empty() && a.chars().all(|c| c.is_alphanumeric() || "_-./:=@+,^\\…".contains(c));
    if plain {
        return a.to_string();
    }
    format!("'{}'", a.replace('\'', "'\"'\"'"))
}

fn exec_inner<X: Runner>(ctx: &X, program: &str, args: &[&str], read_only: bool) -> Result<Out> {
    let shown = format!("{program} {}", redact(args));
    // Вопрос машине — не изменение: в журнале он остаётся строкой вывода
    // (`# <команда>` и ответ), а событие Command несёт только изменяющие
    // команды. Из них собирается ручной путь в инструкции администратора, и
    // `docker inspect` рядом с `docker run` там был бы указанием зря.
    if !read_only {
        ctx.emit(Event::Command(shown.clone()));
    }
    if ctx.dry_run() && !read_only {
        return Ok(Out { code: 0, text: String::new() });
    }
    let output = quiet(Command::new(program))
        .args(args)
        .output()
        .with_context(|| format!("не удалось запустить {program}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    ctx.emit(Event::Output { command: shown, text: redact_text(&text) });
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
    fn git_and_platform_secrets_are_blotted_out() {
        let t = "fatal: Authorization: Basic ZGV2MTpkZXYx rejected\n1cv8 DESIGNER /F C:\\b /N Admin /Pqwerty /DumpCfg\nvrunner --db-pwd=secret --db-user=Admin";
        let r = redact_text(t);
        assert!(!r.contains("ZGV2MTpkZXYx") && r.contains("Basic …"), "{r}");
        assert!(!r.contains("qwerty") && r.contains("/P…"), "{r}");
        assert!(!r.contains("secret") && r.contains("--db-pwd=…") && r.contains("--db-user=Admin"), "{r}");
        assert_eq!(redact_text("API_KEY=abc"), "API_KEY=…");
        // mcp-1c --install echoes its argv in an error: the password after
        // --db-password is blotted the same way as after --db-pwd.
        assert_eq!(redact_text("mcp-1c --install C:\\b --db-user mcp --db-password hunter2 failed"), "mcp-1c --install C:\\b --db-user mcp --db-password … failed");
        // docker inspect prints env as indented JSON strings: still blotted.
        let r = redact_text("            \"CUSTOM_API_KEY=f5b8bc7e\",
            \"PATH=/bin\",");
        assert!(!r.contains("f5b8bc7e") && r.contains("PATH=/bin"), "{r}");
        // A secret before the first '=' on a KEY line is not printed either.
        let r = redact_text("Authorization: Basic ZGV2 API_KEY=abc");
        assert!(!r.contains("ZGV2") && !r.contains("abc"), "{r}");
    }

    #[test]
    fn a_1c_connection_string_hides_only_the_password() {
        // 1С пишет несколько key=value через ';' в одном слове, без
        // пробелов, и сам ключ может быть в любом регистре (`Pwd`, не
        // только `pwd`) — оба случая должны находить и прятать пароль.
        let r = redact_text(r#"File="C:\Bases\UNF";Usr="mcp";Pwd="secret";"#);
        assert!(!r.contains("secret"), "{r}");
        assert!(r.contains("Pwd=…"), "{r}");
        assert!(r.contains(r#"File="C:\Bases\UNF""#), "{r}");
    }

    #[test]
    fn a_quoted_password_with_a_space_inside_is_hidden_whole() {
        // Пароль в кавычках со своим пробелом внутри — ровно случай, на
        // котором наивный split(' ') резал кавычку пополам и оставлял
        // второе слово пароля незатёртым.
        let r = redact_text(r#"File="C:\Bases\UNF";Usr="mcp";Pwd="p w";"#);
        assert!(!r.contains("p w") && !r.contains(" w"), "{r}");
        assert!(r.contains("Pwd=…"), "{r}");
        assert!(r.contains(r#"File="C:\Bases\UNF""#) && r.contains(r#"Usr="mcp""#), "{r}");
    }

    #[test]
    fn a_semicolon_right_after_the_closing_quote_still_separates_segments() {
        let r = redact_text(r#"Usr="mcp";Pwd="p w";Next="n""#);
        assert!(!r.contains("p w"), "{r}");
        assert!(r.contains("Pwd=…") && r.contains(r#"Usr="mcp""#) && r.contains(r#"Next="n""#), "{r}");
    }

    #[test]
    fn the_previously_covered_unquoted_forms_still_hide_the_password_and_keep_path_and_user() {
        // Регрессия: тот же набор форм, что покрывали до правки — путь и
        // имя пользователя должны пережить затирку, а пароль — никогда.
        let r = redact_text(r#"path=C:\Bases\UNF --db-user mcp --db-password hunter2 Usr="mcp";Pwd="secret";"#);
        assert!(!r.contains("hunter2") && !r.contains("secret"), "{r}");
        assert!(r.contains(r#"path=C:\Bases\UNF"#) && r.contains("--db-user mcp") && r.contains(r#"Usr="mcp""#), "{r}");
        assert!(r.contains("--db-password …") && r.contains("Pwd=…"), "{r}");
    }

    #[test]
    fn command_output_hides_secret_values_before_the_journal() {
        let text = "CUSTOM_API_URL=http://h/v1\nCUSTOM_API_KEY=f5b8bc7e\nLLAMA_API_KEY=abc\nPATH=/bin\n";
        let r = redact_text(text);
        assert!(r.contains("CUSTOM_API_URL=http://h/v1") && r.contains("PATH=/bin"));
        assert!(r.contains("CUSTOM_API_KEY=…") && r.contains("LLAMA_API_KEY=…"), "{r}");
        assert!(!r.contains("f5b8bc7e") && !r.contains("abc"));
    }


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
