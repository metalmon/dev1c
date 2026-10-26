use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use core::prelude::v1::test;
use crate::session::State;
use common::check::Check;
use common::Phase;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn a_fresh_ui_has_not_run_the_readiness_check_yet() {
    let ui = Ui::for_test();
    assert!(matches!(ui.verdict, Verdict::Checking), "{:?}", ui.verdict);
}

/// Итог прошлой операции не держит подвал вечно: пришёл новый набор —
/// подвал говорит о состоянии, а «Готово за 19 с» остаётся в журнале.
/// Прежде этот слот не снимался никем, и в покое человек читал про
/// операцию, которой не запускал (находка владельца 26.10).
#[test]
fn an_operation_result_outlives_the_next_set_but_not_for_long() {
    let mut ui = Ui::for_test();
    let gen = ui.facts_gen;

    // Свежий итог пересчёт не стирает: за операцией сразу идёт расчёт
    // («почва могла сдвинуться»), и отказ гас бы раньше, чем прочитан.
    set_status(&mut ui, Line { kind: Kind::Fail, text: "это ограничение по числу сеансов".into() }, None);
    apply_facts(&mut ui, gen, vec![Check::fail("проект", "не разложен", "1c-connect install")]);
    assert!(ui.status.is_some(), "свежий отказ операции не должен гаснуть на первом же наборе");

    // Отстоявший своё — уступает слот состоянию.
    ui.status_at = Some(Instant::now() - STATUS_LINGER);
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, vec![Check::fail("проект", "не разложен", "1c-connect install")]);
    assert_eq!(ui.status, None, "итог отстоял своё и больше не закрывает причину");
    assert_eq!(ui.status_tip, None, "подсказка слота уходит вместе с ним");
    assert_eq!(ui.status_at, None);
}

/// Ответ, пришедший уже после того, как обслуживание выключили, не
/// зеленит и не красит карточку своим содержимым: `apply_facts` смотрит
/// `serving` на момент применения, а не на момент старта расчёта.
///
/// `mode = "local"` — обязательно (Important 7 ревью задач 5+6): у
/// `Ui::for_test` режим туннельный и `link = State::Off`, поэтому
/// `serving()` там ложно независимо от `ui.enabled` — тест оставался
/// зелёным и без строки, которую он проверяет. Обратная половина внизу
/// это и сторожит: при поднятом флаге тот же набор применяется.
#[test]
fn a_set_that_arrives_after_the_switch_went_off_is_dropped() {
    let fact = || vec![Check::fail("проект", "не разложен", "install")];
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(false, Ordering::Relaxed);
    assert!(!serving(&ui), "выключено — иначе тест проверяет не то");
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, fact());
    assert!(matches!(ui.verdict, Verdict::Off), "{:?}", ui.verdict);
    assert!(ui.facts.is_empty());

    // А при поднятом флаге тот же набор доезжает — значит отбросил его
    // именно выключенный переключатель, а не что-то ещё.
    ui.enabled.store(true, Ordering::Relaxed);
    assert!(serving(&ui));
    apply_facts(&mut ui, gen, fact());
    assert!(matches!(ui.verdict, Verdict::NotReady(_)), "{:?}", ui.verdict);
    assert_eq!(ui.facts.len(), 1);
}

/// Critical 3 ревью задач 5+6: холодный замок на такт не переезжает
/// (спека, «Решения, принятые мной», п. 5). Защёлкнувшись, он гасит
/// обслуживание и рвёт сеанс — на секундном такте уснувшая флешка с
/// комплектом выключала коннектор под человеком. В каталоге теста замок
/// заперт (ни инструментов, ни платформы), поэтому `refresh_gate` на этом
/// пути сразу виден: `ui.gate` стал бы `Some`.
#[test]
fn the_tick_path_never_asks_the_cold_gate() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    assert!(gate_now(&ui.kit, &ui.cfg).is_some(), "в каталоге теста замок обязан быть заперт");
    ui.gate = None;

    // Защёлка СВОБОДНА, и расчёт правда заводится (Important 3 ревью
    // круга 2): прежняя версия входила с занятой защёлкой и выходила на
    // раннем возврате, так что `refresh_gate`, поставленный после
    // проверки защёлки, тест прошёл бы. Поток расчёта до `Ui` не
    // дотягивается — он шлёт `Ev`, — так что смотреть после возврата
    // можно без гонки.
    assert!(!ui.facts_pending, "защёлка должна быть свободна — иначе тест идёт по раннему возврату");
    assert!(start_full_pass(&mut ui), "расчёт по такту не начался — тест проверяет не то");
    assert!(ui.facts_pending, "защёлка не взведена");
    assert!(ui.gate.is_none(), "такт позвал замок — он может погасить обслуживание под человеком");
    assert!(ui.enabled.load(Ordering::Relaxed), "и он его погасил");

    // А путь событий (включение, «Сохранить», конец операции) замок
    // по-прежнему считает — он остался на своих прежних поводах.
    run_readiness(&mut ui);
    assert!(ui.gate.is_some(), "замок пропал и с событий тоже");
}

/// Поколение набора — та же защита, что у состояния связи
/// (`link_is_current`): набор считается десяток секунд, и ответ прежнего
/// расчёта не должен затереть свежий.
#[test]
fn a_stale_generation_answer_is_dropped() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.facts_gen = 2;
    ui.facts_pending = true;
    assert!(!apply_facts(&mut ui, 2, vec![Check::fail("Gitea", "не отвечает", "dev1c-setup check")]));
    assert!(matches!(ui.verdict, Verdict::NotReady(_)), "{:?}", ui.verdict);

    // Ответ первого поколения приходит позже — вердикт он не трогает.
    // И защёлку тоже: её держит другой, идущий прямо сейчас расчёт, а
    // снять её чужим ответом значило бы пустить вторую пробу инструментов
    // поверх живой (находка N1 повторного ревью).
    ui.facts_pending = true;
    ui.facts_since = Some(Instant::now());
    assert!(!apply_facts(&mut ui, 1, Vec::new()));
    assert!(matches!(ui.verdict, Verdict::NotReady(_)), "{:?}", ui.verdict);
    assert!(ui.facts_pending, "чужой ответ снял защёлку с живого расчёта");
    assert!(ui.facts_since.is_some(), "и отметку времени — сторожу нечем было бы мерить");
    // Ответ владельца снимает и то, и другое.
    assert!(!apply_facts(&mut ui, 2, Vec::new()));
    assert!(!ui.facts_pending && ui.facts_since.is_none());
}

#[test]
fn a_failed_tray_icon_leads_to_the_exit_path_not_a_silent_hang() {
    let ok: Result<(), &str> = Ok(());
    assert_eq!(exit_code_for(&ok), None, "трей построился — выходить незачем");
    let failed: Result<(), &str> = Err("GTK не инициализирован");
    assert_eq!(exit_code_for(&failed), Some(1), "без значка окно не закрыть — код выхода ненулевой");
}

#[test]
fn three_tabs_in_the_order_the_window_draws_them() {
    assert_eq!(Tab::ALL, [Tab::Params, Tab::Project, Tab::Log]);
    assert_eq!(Tab::ALL.len(), 3);
    assert_eq!(Tab::default(), Tab::Params);
    for (i, t) in Tab::ALL.iter().enumerate() {
        assert_eq!(Tab::ALL.iter().position(|x| x == t), Some(i));
    }
}

#[test]
fn kind_marks_are_icons_say_and_head_have_none() {
    assert_eq!(Kind::Ok.mark(), "✓");
    assert_eq!(Kind::Warn.mark(), "!");
    assert_eq!(Kind::Fail.mark(), "✗");
    assert_eq!(Kind::Say.mark(), "");
    assert_eq!(Kind::Head.mark(), "");
}

#[test]
fn the_badge_sits_in_the_corner_and_says_up_or_down() {
    let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
    let up = badge(&base, TrayState::Up);
    assert_eq!(up.dimensions(), base.dimensions());
    let w = up.width() as usize;
    let at = |img: &image::RgbaImage, x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[..3].to_vec();
    let c = w - 1 - (w as f32 * 0.22) as usize;
    assert_eq!(at(&up, c, c), vec![0x2f, 0x8f, 0x4e]);
    let down = badge(&base, TrayState::Off);
    assert_eq!(at(&down, c, c), vec![0x8d, 0x86, 0x7e]);
    // Провал вердикта — свой цвет, не зелёный и не жёлтый «занято».
    let warn = badge(&base, TrayState::Warn);
    assert_eq!(at(&warn, c, c), vec![0xe8, 0x7a, 0x14]);
    let p = at(&down, 2, 2);
    assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
}

#[test]
fn the_busy_badge_is_yellow_and_not_greyed() {
    let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
    let busy = badge(&base, TrayState::Busy);
    let w = busy.width() as usize;
    let at = |img: &image::RgbaImage, x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[..3].to_vec();
    let c = w - 1 - (w as f32 * 0.22) as usize;
    assert_eq!(at(&busy, c, c), vec![0xfa, 0xcc, 0x1f]);
    // Занято — не обесцвечивается, в отличие от «без связи» (та же точка
    // вдали от кружка остаётся такой же, как в исходном PNG).
    assert_eq!(at(&busy, 2, 2), at(&base, 2, 2));
}

/// Прежняя `tray_state` (своё правило значка: «поднята связь — зелёный»)
/// удалена вместе с этим тестом: двух правд о значке быть не должно, а
/// состояние теперь считает точка карточки — `tray_state_now`, и
/// сторожит его `tray::tests::the_tray_follows_the_card_order`.
#[test]
fn tooltip_names_the_phase_and_minutes() {
    let now = Instant::now() + Duration::from_secs(3600);
    let r = Running {
        which: Which::Export,
        started: now - Duration::from_secs(12 * 60),
        phase: Phase::Decompile,
        seen: now,
        stale_noted: false,
    };
    let ok = Verdict::Ready;
    assert_eq!(tray_tooltip(Some(&r), &State::Up { since: now }, &ok, None, now), "1c-connect: выгружаю конфигурацию из 1С, 12 мин");
    let fresh = Running { started: now, seen: now, ..r };
    assert_eq!(tray_tooltip(Some(&fresh), &State::Off, &ok, None, now), "1c-connect: выгружаю конфигурацию из 1С, меньше минуты");
    assert_eq!(tray_tooltip(None, &State::Up { since: now }, &ok, None, now), "1c-connect: подключён");
    assert_eq!(tray_tooltip(None, &State::Off, &ok, None, now), "1c-connect: не подключён");
    // Причина замка — в подсказке значка; идущая операция её побеждает
    // (тот же порядок разрешения, что в карточке).
    let why = Some("не задан пользователь базы");
    assert_eq!(tray_tooltip(None, &State::Off, &ok, why, now), "1c-connect: нельзя включить: не задан пользователь базы");
    assert_eq!(tray_tooltip(Some(&r), &State::Off, &ok, why, now), "1c-connect: выгружаю конфигурацию из 1С, 12 мин");
}

#[test]
fn tick_redraws_when_the_window_is_open_or_something_changed() {
    // Окно открыто и в покое — минуты в карточке связи всё равно не
    // должны стоять, та считает их сама от `since.elapsed()`.
    assert!(tick_redraw(true, false, false, REDRAW_IDLE));
    // Но не чаще прежнего шага такта (Important 4, п. 4): сам такт стал
    // секундным, а перерисовка от этого учащаться не должна.
    assert!(!tick_redraw(true, false, false, Duration::from_secs(1)));
    // Окно спрятано и тихо — перерисовывать нечего.
    assert!(!tick_redraw(false, false, false, Duration::MAX));
    // Спрятано, но пришли новые данные или идёт операция — всё равно да,
    // и срок тут ни при чём.
    assert!(tick_redraw(false, true, false, Duration::ZERO));
    assert!(tick_redraw(false, false, true, Duration::ZERO));
}

#[test]
fn a_panicking_worker_still_reports_the_end_of_the_work() {
    use super::{guarded, Ev, PANIC_TEXT};
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    guarded(&tx, || -> Ev { Ev::Done { err: None, cancelled: false } }, || -> Ev { unreachable!("тело не паниковало") });
    assert!(matches!(rx.try_recv().unwrap(), Ev::Done { err: None, cancelled: false }));

    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    guarded(&tx, || -> Ev { panic!("ой") }, || Ev::Done { err: Some("сломалось".into()), cancelled: false });
    std::panic::set_hook(prev);
    match rx.try_recv().unwrap() {
        Ev::Line(l) => assert_eq!((l.kind, l.text.as_str()), (Kind::Fail, PANIC_TEXT)),
        _ => panic!("первой должна идти строка в журнал"),
    }
    // Без этого `busy` остался бы навсегда и кнопки — заблокированными.
    assert!(matches!(rx.try_recv().unwrap(), Ev::Done { err: Some(_), cancelled: false }));
}

#[test]
fn which_titles_are_the_old_log_lines() {
    assert_eq!(Which::Install.title(), "— устанавливаю —");
    assert_eq!(Which::Uninstall.title(), "— снимаю расширение —");
    assert_eq!(Which::Export.title(), "— выгружаю из 1С —");
    assert_eq!(Which::Import.title(), "— загружаю в 1С —");
}

#[test]
fn working_is_true_while_busy_connecting_or_a_background_refresh_is_in_flight() {
    use super::Busy;
    let mut ui = Ui::for_test();
    assert!(!ui.working(), "покой");
    ui.busy = Some(Busy::Check);
    assert!(ui.working(), "операция/проверка идёт");
    ui.busy = None;
    ui.link = State::Connecting;
    assert!(ui.working(), "подключение туннеля идёт");
    ui.link = State::Off;
    assert!(!ui.working());
    ui.pending = 1;
    assert!(ui.working(), "фоновое обновление состояния идёт");
    ui.pending = 0;
    assert!(!ui.working());
}

#[test]
fn first_phase_of_each_operation() {
    assert_eq!(first_phase(Which::Export), Phase::Decompile);
    assert_eq!(first_phase(Which::Import), Phase::Pull);
    assert_eq!(first_phase(Which::Install), Phase::Install);
    assert_eq!(first_phase(Which::Uninstall), Phase::Uninstall);
}

/// Отменяются и обмен с git, и работа платформы. Выгрузка и загрузка
/// добавлены 25.10: `local::call` научился замечать флаг и добивать куст
/// процессов через Job Object, а до того самые долгие операции отмены не
/// имели вовсе (STATUS, техдолг 16).
#[test]
fn git_and_platform_phases_can_be_cancelled() {
    for p in [Phase::Push, Phase::Pull, Phase::Decompile, Phase::Load] {
        assert!(can_cancel(p), "{p:?}");
    }
    // Установка расширения и проверка — минуты и секунды, не десятки
    // минут: кнопку им не даём, чтобы она не обещала больше, чем нужно.
    for p in [Phase::Install, Phase::Uninstall, Phase::Check] {
        assert!(!can_cancel(p), "{p:?}");
    }
}

#[test]
fn done_clears_progress_and_renews_cancel() {
    let mut ui = Ui::for_test();
    let now = Instant::now();
    ui.progress = Some(Running { which: Which::Export, started: now, phase: Phase::Push, seen: now, stale_noted: false });
    let old_cancel = ui.cancel.clone();
    old_cancel.store(true, Ordering::Relaxed);
    on_done(&mut ui, None, false);
    assert!(ui.progress.is_none());
    assert!(!ui.cancel.load(Ordering::Relaxed));
    assert!(!Arc::ptr_eq(&old_cancel, &ui.cancel));
}
