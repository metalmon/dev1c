//! Прогон списка операций: установка, состояние, очистка. Печатает через
//! `ctx.say`, поэтому одинаково работает из терминала и из окна.

use crate::op::{CleanLevel, Op, OpState};
use crate::Runner;
use anyhow::Result;

/// Применить операции. `only` — одна по идентификатору; `changes_only` —
/// только разошедшиеся (команда `change`). Возвращает число тронутых.
pub fn apply_all<X: Runner>(ctx: &X, ops: &[Box<dyn Op<X>>], only: Option<&str>, changes_only: bool) -> Result<usize> {
    if let Some(id) = only {
        anyhow::ensure!(ops.iter().any(|o| o.id() == id), "нет операции {id}");
    }
    let mut touched = 0;
    for op in ops {
        if only.is_some_and(|id| op.id() != id) {
            continue;
        }
        if changes_only && op.always_run() {
            continue;
        }
        let st = op.state(ctx);
        if changes_only && !st.needs_work() {
            continue;
        }
        match &st {
            OpState::Done(_) if only.is_none() && !op.always_run() => {
                ctx.say(format!("  [ ok ] {:<16} уже сделано", op.id()));
                continue;
            }
            OpState::Blocked(why) => {
                ctx.say(format!("  [ -- ] {:<16} пропущено: {why}", op.id()));
                continue;
            }
            _ => {}
        }
        ctx.say("");
        ctx.say(format!("== {} — {}", op.id(), op.title()));
        if let OpState::Drifted { from, to } = &st {
            ctx.say(format!("   было:   {from}"));
            ctx.say(format!("   станет: {to}"));
        }
        for a in op.plan(ctx) {
            ctx.say(format!("   · {}", a.0));
        }
        touched += 1;
        if ctx.dry_run() {
            continue;
        }
        op.apply(ctx)?;
    }
    ctx.say("");
    if ctx.dry_run() {
        ctx.say("План напечатан, на машине ничего не менялось.");
    } else if touched == 0 {
        ctx.say("Менять нечего: всё уже так, как задано.");
    } else {
        // Не «Готово.»: за шагами идёт проверка, и «Готово» рядом с
        // провалившейся проверкой — противоречие в двух соседних строках.
        ctx.say("Все шаги выполнены.");
    }
    Ok(touched)
}

/// Состояние каждой операции, без изменений.
pub fn status<X: Runner>(ctx: &X, ops: &[Box<dyn Op<X>>]) -> Vec<(String, String, OpState)> {
    ops.iter()
        .filter(|o| !o.always_run())
        .map(|o| (o.id().to_string(), o.title().to_string(), o.state(ctx)))
        .collect()
}

/// Откат в обратном порядке. План печатается всегда; выполнение — после
/// подтверждения, которое спрашивает вызывающий.
pub fn clean_plan<X: Runner>(ctx: &X, ops: &[Box<dyn Op<X>>], level: CleanLevel) -> Vec<(String, String, Vec<String>)> {
    // Название шага, а не только идентификатор: «== container» человеку не
    // говорит ничего, а решение он принимает по этому списку.
    ops.iter()
        .rev()
        .filter_map(|o| {
            o.revert(ctx, level)
                .map(|acts| (o.id().to_string(), o.title().to_string(), acts.into_iter().map(|a| a.0).collect()))
        })
        .collect()
}

pub fn clean_apply<X: Runner>(ctx: &X, ops: &[Box<dyn Op<X>>], level: CleanLevel) -> Result<()> {
    for o in ops.iter().rev() {
        if o.revert(ctx, level).is_some() && !ctx.dry_run() {
            o.revert_apply(ctx, level)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::Kit;
    use crate::op::Action;
    use crate::{Event, Sink};
    use std::sync::{Arc, Mutex};

    struct T {
        dry: bool,
        sink: Sink,
        kit: Kit,
    }
    impl Runner for T {
        fn dry_run(&self) -> bool {
            self.dry
        }
        fn sink(&self) -> &Sink {
            &self.sink
        }
        fn kit(&self) -> &Kit {
            &self.kit
        }
    }

    struct Flaky(Mutex<u8>);
    impl Op<T> for Flaky {
        fn id(&self) -> &'static str {
            "flaky"
        }
        fn title(&self) -> &'static str {
            "проба"
        }
        fn state(&self, _: &T) -> OpState {
            if *self.0.lock().unwrap() > 0 {
                OpState::Done("".into())
            } else {
                OpState::Missing
            }
        }
        fn plan(&self, _: &T) -> Vec<Action> {
            vec![Action::new("сделать")]
        }
        fn apply(&self, _: &T) -> Result<()> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn ctx(dry: bool, log: Arc<Mutex<Vec<String>>>) -> T {
        let l = log.clone();
        T {
            dry,
            sink: Arc::new(move |e| {
                if let Event::Say(s) = e {
                    l.lock().unwrap().push(s)
                }
            }),
            kit: Kit::at(std::env::temp_dir()),
        }
    }

    #[test]
    fn dry_run_prints_the_plan_and_changes_nothing() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let ops: Vec<Box<dyn Op<T>>> = vec![Box::new(Flaky(Mutex::new(0)))];
        let n = apply_all(&ctx(true, log.clone()), &ops, None, false).unwrap();
        assert_eq!(n, 1);
        let text = log.lock().unwrap().join("\n");
        assert!(text.contains("· сделать"));
        assert!(text.contains("ничего не менялось"));
        assert_eq!(ops[0].state(&ctx(true, log.clone())), OpState::Missing);
    }

    #[test]
    fn second_install_is_a_no_op() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let ops: Vec<Box<dyn Op<T>>> = vec![Box::new(Flaky(Mutex::new(0)))];
        let c = ctx(false, log.clone());
        assert_eq!(apply_all(&c, &ops, None, false).unwrap(), 1);
        assert_eq!(apply_all(&c, &ops, None, false).unwrap(), 0);
        assert!(log.lock().unwrap().join("\n").contains("Менять нечего"));
    }

    #[test]
    fn unknown_only_is_refused() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let ops: Vec<Box<dyn Op<T>>> = vec![Box::new(Flaky(Mutex::new(0)))];
        assert!(apply_all(&ctx(false, log), &ops, Some("nope"), false).is_err());
    }
}
