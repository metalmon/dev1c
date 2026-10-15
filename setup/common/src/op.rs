//! Операция — единица работы над машиной.
//!
//! Всё, что умеет установщик, выражено операциями: у каждой есть состояние,
//! план, применение и откат. Из этого получаются установка (применить то, чего
//! не хватает), правка (применить только разошедшееся), очистка (откатить в
//! обратном порядке) и показ состояния (спросить и ничего не делать).

use crate::Runner;
use anyhow::Result;

/// В каком состоянии параметр машины.
#[derive(Debug, Clone, PartialEq)]
pub enum OpState {
    /// Сделано и совпадает с желаемым.
    Done(String),
    /// Ещё не сделано.
    Missing,
    /// Сделано, но не так: на машине одно, в комплекте другое.
    Drifted { from: String, to: String },
    /// Сделать нельзя, и вот почему (нет данных, нет прав, не запущен движок).
    Blocked(String),
}

impl OpState {
    pub fn needs_work(&self) -> bool {
        matches!(self, OpState::Missing | OpState::Drifted { .. })
    }
    /// Короткая подпись для списка операций.
    pub fn label(&self) -> String {
        match self {
            OpState::Done(v) if v.is_empty() => "сделано".into(),
            OpState::Done(v) if v.chars().count() > 40 => {
                format!("сделано: {}…", v.chars().take(36).collect::<String>())
            }
            OpState::Done(v) => format!("сделано: {v}"),
            OpState::Missing => "не сделано".into(),
            OpState::Drifted { from, to } => format!("разошлось: {from} -> {to}"),
            OpState::Blocked(why) => format!("нельзя: {why}"),
        }
    }
}

/// Строка плана: что именно произойдёт. Это же печатает `--dry-run`.
#[derive(Debug, Clone)]
pub struct Action(pub String);

impl Action {
    pub fn new(t: impl Into<String>) -> Self {
        Action(t.into())
    }
}

/// Уровни очистки: каждый следующий включает предыдущий.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CleanLevel {
    /// Снять контейнер. Данные на месте.
    Stack,
    /// Удалить и данные: тома.
    Data,
}

pub trait Op<X: Runner>: Sync {
    /// Устойчивый идентификатор: `image`, `container`. По нему работает `--only`.
    fn id(&self) -> &'static str;
    /// Название для человека.
    fn title(&self) -> &'static str;
    /// Состояние, без изменений на машине.
    fn state(&self, ctx: &X) -> OpState;
    /// Что будет сделано.
    fn plan(&self, ctx: &X) -> Vec<Action>;
    /// Сделать. Обязано быть идемпотентным: повторный вызов ничего не портит.
    fn apply(&self, ctx: &X) -> Result<()>;
    /// Чем это снести. `None` — откатывать нечего.
    fn revert(&self, _ctx: &X, _level: CleanLevel) -> Option<Vec<Action>> {
        None
    }
    /// Сделать откат. Уровень передаётся, потому что одна и та же операция на
    /// разных уровнях сносится по-разному: контейнер снимается всегда, тома
    /// удаляются только на уровне данных.
    fn revert_apply(&self, _ctx: &X, _level: CleanLevel) -> Result<()> {
        Ok(())
    }
    /// Выполняется при каждой установке (проверки окружения). В списке
    /// состояния у неё нет «сделано»: она ничего не оставляет после себя.
    fn always_run(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_tells_when_there_is_work() {
        assert!(!OpState::Done("да".into()).needs_work());
        assert!(OpState::Missing.needs_work());
        assert!(OpState::Drifted { from: "a".into(), to: "b".into() }.needs_work());
        // Заблокированную операцию не «чинят» молча: это повод сказать человеку.
        assert!(!OpState::Blocked("нет образа".into()).needs_work());
    }

    #[test]
    fn labels_are_readable_and_cyrillic_safe() {
        assert_eq!(OpState::Missing.label(), "не сделано");
        assert_eq!(OpState::Drifted { from: "1".into(), to: "2".into() }.label(), "разошлось: 1 -> 2");
        // Обрезка по символам, не по байтам: кириллица в два байта не должна рвать строку.
        let long = "а".repeat(50);
        assert!(OpState::Done(long).label().ends_with('…'));
    }
}
