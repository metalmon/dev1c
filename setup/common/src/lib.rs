//! Каркас двух установщиков: dev1c-setup (машина с Docker) и 1c-connect
//! (машина с 1С). Скопирован по образцу volt-setup: вся логика в операциях,
//! окно и командная строка — тонкие оболочки над ними.
//!
//! Движок ничего не печатает сам: он отдаёт события (`Event`) наблюдателю.
//! CLI печатает их в терминал, окно — рисует в списке шагов.

pub mod check;
pub mod console;
pub mod docker;
pub mod integrity;
pub mod kit;
pub mod op;
pub mod run;
pub mod toml_render;

use std::sync::Arc;

/// Что программа делает прямо сейчас.
#[derive(Debug, Clone)]
pub enum Event {
    /// Хорошая новость.
    Ok(String),
    /// Не ошибка, но человеку стоит знать.
    Warn(String),
    /// Ошибка, из-за которой шаг не сделан.
    Fail(String),
    /// Просто строка (адреса, пояснения, план).
    Say(String),
    /// Команда, которую сейчас выполняем (в режиме показа — только печатаем).
    Command(String),
    /// Сырой вывод внешней программы: в терминал не идёт, живёт в журнале.
    Output { command: String, text: String },
}

/// Куда движок отдаёт события.
pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// Как спрашивать у человека. В CLI — чтение строки, в окне — диалог,
/// в режиме `--yes` — всегда значение по умолчанию.
pub trait Prompt: Send + Sync {
    fn ask(&self, question: &str, default: &str) -> String;
    fn confirm(&self, question: &str, default_yes: bool) -> bool;
}

/// Согласие на всё: значения по умолчанию без вопросов (`--yes`).
pub struct AlwaysDefault;

impl Prompt for AlwaysDefault {
    fn ask(&self, _question: &str, default: &str) -> String {
        default.to_string()
    }
    fn confirm(&self, _question: &str, default_yes: bool) -> bool {
        default_yes
    }
}

/// Вопросы с клавиатуры.
pub struct StdinPrompt;

impl Prompt for StdinPrompt {
    fn ask(&self, question: &str, default: &str) -> String {
        use std::io::Write;
        print!("{question} [{default}]: ");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        let s = s.trim();
        if s.is_empty() {
            default.to_string()
        } else {
            s.to_string()
        }
    }
    fn confirm(&self, question: &str, default_yes: bool) -> bool {
        let d = if default_yes { "Д/н" } else { "д/Н" };
        let a = self.ask(question, d).to_lowercase();
        match a.as_str() {
            "д" | "да" | "y" | "yes" => true,
            "н" | "нет" | "n" | "no" => false,
            _ => default_yes,
        }
    }
}

/// То, что операции и обёртки над командами спрашивают у контекста прогона.
/// Каждая программа держит свой контекст (со своей конфигурацией) и реализует
/// это для него.
pub trait Runner: Sync {
    fn dry_run(&self) -> bool;
    fn sink(&self) -> &Sink;
    fn kit(&self) -> &kit::Kit;
    /// Имя движка контейнеров: `docker` или `podman`.
    fn engine(&self) -> String {
        "docker".into()
    }
    fn emit(&self, e: Event) {
        (self.sink())(e);
    }
    fn ok(&self, t: impl Into<String>) {
        self.emit(Event::Ok(t.into()));
    }
    fn warn(&self, t: impl Into<String>) {
        self.emit(Event::Warn(t.into()));
    }
    fn fail(&self, t: impl Into<String>) {
        self.emit(Event::Fail(t.into()));
    }
    fn say(&self, t: impl Into<String>) {
        self.emit(Event::Say(t.into()));
    }
}
