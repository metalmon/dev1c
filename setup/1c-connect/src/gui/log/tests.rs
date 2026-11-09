use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use crate::cfg::cfg_with;
use common::{Event, Phase, Progress};
use core::prelude::v1::test;
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

/// Задача 6 плана «После живого прогона»: замок запуска повторяет то, что
/// человек и так читает в подвале, а не сообщает находку, — значит, не
/// должен поднимать счётчик непрочитанного раньше первого действия
/// человека (находка владельца 9.11: свежий комплект показывал «Журнал 1»
/// ещё до того, как человек хоть что-то сделал).
#[test]
fn a_fresh_kit_has_nothing_unread_before_the_person_does_anything() {
    let mut ui = Ui::for_test();
    let why = ui.gate.clone().expect("в каталоге теста замок обязан быть заперт");
    note_line_seen(&mut ui, gate_line(&why, false));
    assert_eq!(ui.unread, 0, "{:?}", ui.log);
    // Строка всё равно лежит в журнале — почитать её можно, просто не как находку.
    assert_eq!(ui.log.len(), 1);
}

#[test]
fn note_line_appends_marks_and_head_lines_to_the_log_file() {
    let mut ui = Ui::for_test();
    note_line(&mut ui, Line { kind: Kind::Head, text: "— устанавливаю — · 12:34".into() });
    note_line(&mut ui, Line { kind: Kind::Ok, text: "готово".into() });
    let text = std::fs::read_to_string(ui.kit.state_path(LOG_FILE)).unwrap();
    // «Готово» с заглавной: `note_line` поднимает первую букву до записи, и
    // файл журнала говорит ровно то же, что окно. Заголовок раздела
    // начинается с тире — не буква, поднимать нечего.
    assert_eq!(text, "\n— устанавливаю — · 12:34\n✓ Готово\n");
}

/// Начала фраз — с заглавной, но не ценой правды: техническое имя, путь,
/// версия и команда остаются как написаны (владелец 31.10: «либо с большой
/// буквы все начала предложений, либо с маленькой»).
#[test]
fn shown_text_starts_with_a_capital_unless_the_first_word_is_a_technical_name() {
    assert_eq!(capitalized("инструменты комплекта"), "Инструменты комплекта");
    assert_eq!(capitalized("mcp-1c в комплекте"), "mcp-1c в комплекте");
    assert_eq!(capitalized("vrunner-mcp отвечает"), "vrunner-mcp отвечает");
    assert_eq!(capitalized("oscript 2.2.0 из комплекта"), "oscript 2.2.0 из комплекта");
    assert_eq!(capitalized(r"C:\dev1c-connect-kit"), r"C:\dev1c-connect-kit");
    assert_eq!(capitalized("Не зафиксировано"), "Не зафиксировано");
    assert_eq!(capitalized(""), "");
    // Имя проверяется целиком, а не по началу строки: «гитом» — слово, не `git`.
    assert_eq!(capitalized("гитом отправлено"), "Гитом отправлено");
    // Путь и версия — по признаку первого слова, в какой бы фразе ни стояли.
    assert_eq!(capitalized("1.2.3 — версия комплекта"), "1.2.3 — версия комплекта");
    assert_eq!(capitalized("setup.log дописан"), "setup.log дописан");
    // Отпечаток коммита — не слово: «A1b2c3» было бы выдумкой про имя.
    assert_eq!(capitalized("a1b2c3 выгрузка (2 часа назад)"), "a1b2c3 выгрузка (2 часа назад)");
    // Двоеточие в хвосте слова — не путь: «план: …» это начало фразы.
    assert_eq!(capitalized("план: cf_decompile → src"), "План: cf_decompile → src");
    assert_eq!(capitalized("mcp-1c: строка отброшена"), "mcp-1c: строка отброшена");
}

/// Шов: заглавную ставит `note_line` — единственная дорога строки в журнал
/// окна, — а не показ, поэтому и список окна, и `1c-connect.log` несут один
/// текст.
#[test]
fn a_line_arriving_in_the_log_comes_out_with_a_capital() {
    let mut ui = Ui::for_test();
    note_line(&mut ui, Line { kind: Kind::Warn, text: "признак разрешения не записан".into() });
    assert_eq!(ui.log[0].text, "Признак разрешения не записан");
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
    let at = |secs| Running {
        which: Which::Export,
        started: now - Duration::from_secs(secs),
        phase: Phase::Decompile,
        seen: now,
        stale_noted: false,
    };
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
    let fresh =
        Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(299), stale_noted: false };
    assert!(!is_stale(&fresh, now));
    let quiet =
        Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(301), stale_noted: false };
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
    let mut quiet =
        Running { which: Which::Export, started: now, phase: Phase::Push, seen: now - Duration::from_secs(301), stale_noted: false };
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
    ui.progress = Some(Running {
        which: Which::Export,
        started: now,
        phase: Phase::Decompile,
        seen: now - Duration::from_secs(10),
        stale_noted: false,
    });
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
    ui.progress = Some(Running {
        which: Which::Export,
        started: now,
        phase: Phase::Decompile,
        seen: now - Duration::from_secs(10),
        stale_noted: false,
    });
    let line = Line { kind: Kind::Say, text: "выгружаю конфигурацию из 1С (cf_decompile → src)".into() };
    on_line(&mut ui, line, now);
    // Та же строка, и с заглавной: дорога в журнал одна — `note_line`.
    assert_eq!(
        ui.log.last().map(|s| &s.line),
        Some(&Line { kind: Kind::Say, text: "Выгружаю конфигурацию из 1С (cf_decompile → src)".into() })
    );
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
    // Главное здесь — `Warn`, а не `Fail`. Заглавную в журнале ставит
    // `note_line`; подвал держит строку как есть и поднимает её при показе
    // (`panel::foot_left_text`), поэтому человек читает одно и то же.
    assert_eq!(ui.log.last().map(|s| &s.line), Some(&Line { kind: Kind::Warn, text: "Прервано".into() }));
    assert_eq!(ui.status, Some(Line { kind: Kind::Warn, text: "прервано".into() }));
    assert_eq!(crate::gui::panel::foot_left_text(ui.status.as_ref(), None, None, "C:\\kit").0, "Прервано");
}

#[test]
fn the_httpd_queue_is_named_only_when_there_is_more_than_one_base() {
    // При одной базе очередь не образуется — строка не нужна
    assert_eq!(httpd_queue_note(&cfg_with(&["Buh"])), None);

    // При нескольких базах есть сообщение про задержку
    let note = httpd_queue_note(&cfg_with(&["Buh", "Ромашка"])).expect("сообщение должно быть при двух и более базах");

    // Проверяем смысл: сообщение должно упоминать очередь или задержку,
    // а не ошибку или поломку
    let lower = note.to_lowercase();
    assert!(lower.contains("очереди") || lower.contains("медленнее"), "сообщение должно упоминать очередь или задержку: {note}");
}

/// Лестница владельца 9.11: «Сегодня» → «Вчера» → день недели для остальных
/// дней этой недели → дата для всего, что старше. «Сегодня» теста — суббота,
/// 14 ноября 2026 (придумано для теста, не живые часы): понедельник этой
/// недели — 9 ноября, поэтому среда 11-го ещё внутри недели, а 3 ноября —
/// уже нет.
#[test]
fn the_group_label_climbs_the_ladder() {
    let today = Stamp { year: 2026, month: 11, day: 14, weekday: 5, hour: 0, minute: 0 };
    assert_eq!(group_label(today, today), "Сегодня");
    let yesterday = Stamp { year: 2026, month: 11, day: 13, weekday: 4, hour: 0, minute: 0 };
    assert_eq!(group_label(yesterday, today), "Вчера");
    // Три дня назад, в пределах этой недели — падеж винительный: «в среду».
    let wednesday = Stamp { year: 2026, month: 11, day: 11, weekday: 2, hour: 0, minute: 0 };
    assert_eq!(group_label(wednesday, today), "В среду");
    // Старше этой недели — дата, месяц в родительном падеже.
    let older = Stamp { year: 2026, month: 11, day: 3, weekday: 1, hour: 0, minute: 0 };
    assert_eq!(group_label(older, today), "3 ноября");
}

/// Склейка повторов — находка владельца 9.11 на живом снимке: «Режим
/// работы: Локально…» и «base_path — путь к базе» стояли дважды подряд.
/// Одинаковые подряд (один вид, один текст) — одна запись со счётчиком;
/// разделённые чужой строкой — НЕ склеиваются.
#[test]
fn the_same_line_twice_in_a_row_is_one_line_with_a_count() {
    let day = Stamp { year: 2026, month: 11, day: 9, weekday: 0, hour: 9, minute: 0 };
    let say = |text: &str| Stamped { line: Line { kind: Kind::Say, text: text.into() }, stamp: day };
    let lines =
        vec![say("Режим работы: Локально"), say("Режим работы: Локально"), say("base_path — путь к базе"), say("base_path — путь к базе")];
    let out = entries(&lines, day);
    // Подпись дня (одна, в начале) + две склеенные записи, каждая с ×2.
    assert_eq!(out.len(), 3, "{out:?}");
    assert!(matches!(&out[0], LogEntry::Caption(_)), "{out:?}");
    assert_eq!(out[1], LogEntry::Row(Line { kind: Kind::Say, text: "Режим работы: Локально".into() }, 2));
    assert_eq!(out[2], LogEntry::Row(Line { kind: Kind::Say, text: "base_path — путь к базе".into() }, 2));

    // Та же пара, но разделённая чужой строкой — считается заново, без склейки.
    let interrupted = vec![say("A"), Stamped { line: Line { kind: Kind::Say, text: "B".into() }, stamp: day }, say("A")];
    let out2 = entries(&interrupted, day);
    assert_eq!(out2.len(), 4, "{out2:?}"); // подпись + A + B + A
    assert_eq!(out2[1], LogEntry::Row(Line { kind: Kind::Say, text: "A".into() }, 1));
    assert_eq!(out2[2], LogEntry::Row(Line { kind: Kind::Say, text: "B".into() }, 1));
    assert_eq!(out2[3], LogEntry::Row(Line { kind: Kind::Say, text: "A".into() }, 1));
}

/// Лента из строк двух дней — подпись у каждого дня, в том числе у первого
/// (не только у смены дня внутри ленты).
#[test]
fn a_group_caption_stands_before_the_first_line_of_its_day() {
    let today = Stamp { year: 2026, month: 11, day: 9, weekday: 0, hour: 0, minute: 0 };
    let yesterday = Stamp { year: 2026, month: 11, day: 8, weekday: 6, hour: 0, minute: 0 };
    let lines = vec![
        Stamped { line: Line { kind: Kind::Ok, text: "вчерашняя строка".into() }, stamp: yesterday },
        Stamped { line: Line { kind: Kind::Ok, text: "сегодняшняя строка".into() }, stamp: today },
    ];
    let out = entries(&lines, today);
    assert_eq!(
        out,
        vec![
            LogEntry::Caption("Вчера".into()),
            LogEntry::Row(Line { kind: Kind::Ok, text: "вчерашняя строка".into() }, 1),
            LogEntry::Caption("Сегодня".into()),
            LogEntry::Row(Line { kind: Kind::Ok, text: "сегодняшняя строка".into() }, 1),
        ]
    );
}
