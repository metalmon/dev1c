use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use core::prelude::v1::test;
use common::{Event, Phase, Progress};
use std::time::{Duration, Instant};

#[test]
fn took_prints_seconds_under_a_minute_then_minutes_and_seconds() {
    assert_eq!(took(Duration::from_secs(12)), "12 с");
    assert_eq!(took(Duration::from_secs(72)), "1 мин 12 с");
    assert_eq!(took(Duration::from_secs(0)), "0 с");
}

/// Кнопки окна ведут к тем же четырём операциям, что запрещённые команды
/// (`Which` — ровно `install`/`uninstall`/`export`/`import`), и после
/// срока не начинают работу: `run_op` спрашивает эти ворота первым делом.
/// Пока срок идёт, даже в последний его день, ворота открыты.
#[test]
fn an_expired_licence_refuses_the_window_buttons_that_change_the_machine() {
    let no = op_refusal(crate::licence::State::Expired).expect("после срока операция не начинается");
    assert!(no.kind == Kind::Fail && no.text == crate::licence::REFUSAL, "{no:?}");
    assert!(op_refusal(crate::licence::State::Soon { days: 0 }).is_none());
    assert!(op_refusal(crate::licence::State::Valid).is_none());
}

/// Окно обязано объяснить, что случилось и до какого числа срок был;
/// пока срок идёт — ни строки, журнал не место для пустых напоминаний.
#[test]
fn the_window_explains_an_expired_licence_and_stays_quiet_while_it_runs() {
    let dead = licence_line(crate::licence::State::Expired).expect("истёкший срок — строка в журнале");
    assert!(dead.kind == Kind::Fail && dead.text.contains(&crate::licence::until_text()), "{dead:?}");
    let soon = licence_line(crate::licence::State::Soon { days: 5 }).expect("предупреждение — строка в журнале");
    assert!(soon.kind == Kind::Warn && soon.text.contains('5'), "{soon:?}");
    assert!(licence_line(crate::licence::State::Valid).is_none());
}

#[test]
fn note_file_writes_the_file_and_not_the_window() {
    let mut ui = Ui::for_test();
    note_file(&ui.kit, &Line { kind: Kind::Say, text: "рабочая область и угол флайаута: 2560x1397+0+0 → 2168,965".into() });
    let text = std::fs::read_to_string(ui.kit.state_path(LOG_FILE)).unwrap();
    assert!(text.contains("рабочая область и угол флайаута"), "{text:?}");
    // Главное: служебная строка не попала ни в список окна, ни в счётчик
    // непрочитанных — человеку её читать незачем.
    assert!(ui.log.is_empty(), "{:?}", ui.log);
    assert_eq!(ui.unread, 0);
    // `note_line` рядом — по-прежнему и в файл, и в окно.
    note_line(&mut ui, Line { kind: Kind::Ok, text: "готово".into() });
    assert_eq!(ui.log.len(), 1);
}

#[test]
fn note_line_appends_marks_and_head_lines_to_the_log_file() {
    let mut ui = Ui::for_test();
    note_line(&mut ui, Line { kind: Kind::Head, text: "— устанавливаю — · 12:34".into() });
    note_line(&mut ui, Line { kind: Kind::Ok, text: "готово".into() });
    let text = std::fs::read_to_string(ui.kit.state_path(LOG_FILE)).unwrap();
    assert_eq!(text, "\n— устанавливаю — · 12:34\n✓ готово\n");
}

#[test]
fn command_and_output_never_reach_the_window() {
    assert_eq!(line_of(&Event::Command("x".into())), None);
    assert_eq!(line_of(&Event::Output { command: "x".into(), text: "y".into() }), None);
    assert_eq!(line_of(&Event::Warn("w".into())), Some(Line { kind: Kind::Warn, text: "w".into() }));
}

#[test]
fn the_log_keeps_the_last_two_thousand_lines_and_counts_unread() {
    let mut ui = Ui::for_test();
    for i in 0..LOG_CAP + 5 {
        note_line(&mut ui, Line { kind: Kind::Say, text: i.to_string() });
    }
    assert_eq!(ui.log.len(), LOG_CAP);
    assert_eq!(ui.log[0].text, "5");
    assert_eq!(ui.unread, LOG_CAP + 5);
    ui.tab = Tab::Log;
    ui.unread = 0; // окна нет — строки всё равно непрочитанные
    note_line(&mut ui, Line { kind: Kind::Say, text: "x".into() });
    assert_eq!(ui.unread, 1);
}

#[test]
fn progress_text_shows_title_and_elapsed() {
    // Anchored an hour into the future: a fresh CI runner can have under
    // an hour of uptime, and `Instant::now() - Duration::from_secs(3725)`
    // panics (underflow) once the subtracted duration outruns it.
    let now = Instant::now() + Duration::from_secs(3600);
    let at = |secs| Running { which: Which::Export, started: now - Duration::from_secs(secs), phase: Phase::Decompile, seen: now, stale_noted: false };
    let (title, clock) = progress_text(&at(725), now);
    assert_eq!(title, "Выгружаю конфигурацию из 1С");
    assert_eq!(clock, "12:05");
    assert_eq!(progress_text(&at(5), now).1, "0:05");
    assert_eq!(progress_text(&at(65), now).1, "1:05");
    assert_eq!(progress_text(&at(3725), now).1, "1:02:05");
}

#[test]
fn stale_after_five_quiet_minutes() {
    // Same underflow risk as above: anchor `now` well past any real
    // clock value before subtracting minutes from it.
    let now = Instant::now() + Duration::from_secs(3600);
    let fresh = Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(299), stale_noted: false };
    assert!(!is_stale(&fresh, now));
    let quiet = Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(301), stale_noted: false };
    assert!(is_stale(&quiet, now));
    assert_eq!(stale_text(&quiet, now), "нет новостей 5 мин");
}

#[test]
fn stale_line_fires_once_per_operation() {
    // Карточка не показывает застой (заказчик 2026-10-19) — он уходит в
    // журнал одной строкой `Kind::Warn`, не чаще раза на операцию.
    let now = Instant::now() + Duration::from_secs(3600);
    let mut fresh = Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false };
    assert_eq!(stale_line(&mut fresh, now), None);
    let mut quiet = Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(301), stale_noted: false };
    assert_eq!(stale_line(&mut quiet, now), Some(Line { kind: Kind::Warn, text: "нет новостей 5 мин".into() }));
    assert!(quiet.stale_noted);
    // Второй вызов на том же застое — ничего не шлёт.
    assert_eq!(stale_line(&mut quiet, now), None);
}

#[test]
fn a_progress_event_moves_phase_and_seen() {
    let mut ui = Ui::for_test();
    // `on_progress` now takes `now` as a parameter instead of reading
    // the real clock itself: a plain `assert!(r.seen >= old_seen)`
    // against the real clock would hold even if the function never
    // touched `seen` at all, since real time never runs backwards.
    // Anchored, so the `- 10s` below can't underflow a short-lived
    // runner's clock either.
    let now = Instant::now() + Duration::from_secs(3600);
    ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Decompile, seen: now - Duration::from_secs(10), stale_noted: false });
    on_progress(&mut ui, Progress { phase: Phase::Push, done: None, total: None, note: "x".into() }, now);
    let r = ui.progress.as_ref().unwrap();
    assert_eq!(r.phase, Phase::Push);
    assert_eq!(r.seen, now);
    assert!(ui.dirty);
}

#[test]
fn a_line_during_an_operation_reaches_the_journal_and_moves_seen() {
    // B1: `Ev::Line` while `progress.is_some()` used to move the line
    // into `r.note` only — `note_line` (the sole caller of
    // `write_log_file`) was never reached, so the journal (window and
    // `1c-connect.log`) stayed empty for the whole operation.
    let mut ui = Ui::for_test();
    let now = Instant::now() + Duration::from_secs(3600);
    ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Decompile, seen: now - Duration::from_secs(10), stale_noted: false });
    let line = Line { kind: Kind::Say, text: "cf_decompile → src (минуты…)".into() };
    on_line(&mut ui, line.clone(), now);
    assert_eq!(ui.log.last(), Some(&line));
    let r = ui.progress.as_ref().unwrap();
    assert_eq!(r.seen, now);
    assert!(ui.dirty);
}

#[test]
fn a_cancelled_done_is_a_warning_not_a_failure() {
    let mut ui = Ui::for_test();
    let now = Instant::now();
    ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false });
    let line = on_done(&mut ui, Some("прервано".into()), true);
    note_line(&mut ui, line.clone());
    ui.status = Some(line);
    assert_eq!(ui.log.last(), Some(&Line { kind: Kind::Warn, text: "прервано".into() }));
    assert_eq!(ui.status, Some(Line { kind: Kind::Warn, text: "прервано".into() }));
}
