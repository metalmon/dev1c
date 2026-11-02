use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use crate::session::State;
use common::check::Check;
use common::Phase;
use core::prelude::v1::test;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn a_fresh_ui_has_not_run_the_readiness_check_yet() {
    let ui = Ui::for_test();
    assert!(matches!(ui.verdict, Verdict::Checking), "{:?}", ui.verdict);
}

/// Задача 4: `adopt_cfg` зовётся из `window::reread_and_recheck` на каждый
/// показ окна, и файл на диске к этому моменту мог не измениться ни на байт —
/// тот же нерасшифрованный пароль читается заново. Без метки «уже сказано»
/// (`Ui::undecryptable_noted`) строка сыпалась бы в журнал на каждое открытие
/// окна; эта проверка сторожит именно переход, а не сам текст строки (его
/// сторожат тесты `panel::undecryptable_line`).
#[test]
fn an_undecryptable_password_is_noted_once_per_load_not_per_show() {
    let mut ui = Ui::for_test();
    let warn_count = |ui: &Ui| ui.log.iter().filter(|l| l.kind == Kind::Warn && l.text.contains("рабочего места")).count();

    let mut broken = ui.cfg.clone();
    broken.undecryptable = vec!["password".to_string()];

    adopt_cfg(&mut ui, broken.clone());
    assert_eq!(warn_count(&ui), 1, "первая загрузка с незакрытым паролем — одна строка");

    // Второй показ перечитывает тот же файл: тот же список, строка не повторяется.
    adopt_cfg(&mut ui, broken.clone());
    assert_eq!(warn_count(&ui), 1, "повторный показ того же состояния не добавляет вторую строку");

    // Пароль ввели заново и сохранили — список опустел: молчим, но метку сбрасываем.
    let mut fixed = broken.clone();
    fixed.undecryptable = Vec::new();
    adopt_cfg(&mut ui, fixed);
    assert_eq!(warn_count(&ui), 1, "починка не добавляет строку");

    // Испортилось снова (другая машина, другая учётка) — строка возвращается.
    adopt_cfg(&mut ui, broken);
    assert_eq!(warn_count(&ui), 2, "новая беда после починки — новая строка");
}

/// Minor ревью задачи 4: список меняется не только «пусто ↔ не пусто» — ко
/// второму полю может добавиться беда, когда первое уже не раз было
/// показано. Сравнение в `adopt_cfg` идёт по всему списку (`Vec<String>`
/// целиком через `!=`), и уже поэтому верно, но следующая правка могла бы
/// незаметно сузить его до «список был/не был пуст» — этот тест сторожит
/// именно переход «одно поле → два поля».
#[test]
fn a_second_field_joining_the_list_is_noted_again() {
    let mut ui = Ui::for_test();
    let warn_count =
        |ui: &Ui| ui.log.iter().filter(|l| l.kind == Kind::Warn && (l.text.contains("рабочего места") || l.text.contains("базы"))).count();

    let mut one = ui.cfg.clone();
    one.undecryptable = vec!["password".to_string()];
    adopt_cfg(&mut ui, one);
    assert_eq!(warn_count(&ui), 1, "одно поле — одна строка");

    let mut both = ui.cfg.clone();
    both.undecryptable = vec!["password".to_string(), "db_password".to_string()];
    adopt_cfg(&mut ui, both);
    assert_eq!(warn_count(&ui), 2, "список стал другим — новая строка, хоть первое поле и было уже сказано");
}

/// Круг исправления 4 задачи 3 (проверка безопасности): `adopt_cfg`
/// толкает перечитанный с диска `Cfg` в живой сеанс КАК ЕСТЬ, даже если в
/// сеансе уже лежало что-то другое (здесь — потолок, подменённый напрямую,
/// мимо обычного пути). Это и есть довод финального решения
/// `gui::tray::apply_access` (поправка ведущего, вторая для этой ветки):
/// второй источник истины для потолка рядом с файлом смысла не имел бы —
/// `adopt_cfg` всё равно откатит его на диск при первом же показе окна.
/// Честный откат, не баг (докстрока `adopt_cfg`): другого источника правды
/// для «перечитать файл» нет.
#[test]
fn adopt_cfg_rolls_back_a_live_only_tightening_to_what_is_on_disk() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().mode = "local".into();
    ui.cfg.current_mut().write = "read_write".into();
    // Настоящий `Session` — та же ячейка (`Live`), что и в работающей
    // программе; слушатели ей заводить незачем (`cfg_now`/`update_cfg` не
    // зависят от того, поднялись ли реальные порты).
    let session = crate::session::start(ui.cfg.clone(), ui.kit.clone(), Arc::new(|_| {}), |_| {});
    // Живое ужесточение — то же самое, что `apply_access` делает в ветке
    // отказа записи: клон живого `Cfg` с одним подменённым полем.
    session.update_cfg(session.cfg_now().with_access("read_only"));
    assert_eq!(session.cfg_now().access(), "read_only", "вводная: живое ужесточение применилось");
    ui.session = Some(session);

    // Показ окна перечитывает файл, где `write` всё ещё "read_write" —
    // запись на диск не удалась, файл её не увидел.
    let from_disk = ui.cfg.clone();
    adopt_cfg(&mut ui, from_disk);
    let s = ui.session.take().expect("сеанс потерян");
    assert_eq!(s.cfg_now().access(), "read_write", "откат на то, что в файле — честный, не баг");
    let _ = s.stop();
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
    ui.cfg.current_mut().mode = "local".into();
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
    ui.cfg.current_mut().mode = "local".into();
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
    ui.cfg.current_mut().mode = "local".into();
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
fn a_failed_save_opens_the_tab_that_holds_the_fields() {
    assert_eq!(tab_for_unsaved_switch(), Tab::Link, "поля живут на «Связи», туда и вести");
}

#[test]
fn tabs_are_named_as_they_are_labelled() {
    assert_eq!(Tab::ALL, [Tab::Link, Tab::Tools, Tab::Log]);
    assert_eq!(Tab::ALL.len(), 3);
    assert_eq!(Tab::default(), Tab::Link, "окно открывается на «Связи» — она первая по контракту");
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

/// Успех говорит — наблюдение владельца 30.10 («по журналу непонятно, успех
/// это или нет»): щелчок уходит в поток и пишет «убираю записи MCP…», а ответ
/// CLI агента идёт 6–8 с, и прежнее молчание на успехе оставляло в журнале
/// незаконченную фразу. Молчание годилось, пока действие было мгновенным.
/// По-прежнему отдельно говорит то, что не совпало с тем, что попросила
/// галочка (`on`), и отказ `merge`/`remove`.
/// `editors::setup`/`teardown` отдают то, что `look` увидел на диске ПОСЛЕ
/// правки, а не факт успеха `merge`/`remove` — любой `Ok`, не совпавший с
/// `on`, обязан назвать, что увидел `look`, и показать это провалом, не
/// молчаливой победой.
#[test]
fn success_is_silent_and_a_mismatch_speaks() {
    let done_on = setup_line("Zed", true, Ok(crate::editors::Found::Ours)).unwrap();
    assert_eq!(done_on.kind, Kind::Ok, "{done_on:?}");
    assert!(done_on.text.contains("на месте"), "{done_on:?}");
    for off in [crate::editors::Found::Absent, crate::editors::Found::Missing] {
        let done_off = setup_line("Zed", false, Ok(off)).unwrap();
        assert_eq!(done_off.kind, Kind::Ok, "{done_off:?}");
        assert!(done_off.text.contains("убраны"), "{done_off:?}");
    }

    let still_absent = setup_line("Zed", true, Ok(crate::editors::Found::Absent)).unwrap();
    assert_eq!(still_absent.kind, Kind::Fail, "{still_absent:?}");
    assert!(still_absent.text.contains("не удалось настроить"), "{still_absent:?}");
    assert!(still_absent.text.contains("серверы не настроены"), "{still_absent:?}");

    let still_there = setup_line("pi", false, Ok(crate::editors::Found::Ours)).unwrap();
    assert!(still_there.text.contains("не удалось убрать записи"), "{still_there:?}");

    // Пункт 3 ревью целого: включение на занятом чужим имени — отказ, а снятие
    // на том же итоге — успех со словами о том, что осталось: наши записи
    // убраны, чужая не наша не только чтобы её писать, но и чтобы ею нас
    // запирать. Молчать на снятии нельзя — человек просил снять записи с этими
    // именами и обязан узнать, какая осталась и почему.
    let foreign_on = setup_line("pi", true, Ok(crate::editors::Found::Foreign(vec!["1c".into()]))).unwrap();
    assert_eq!(foreign_on.kind, Kind::Fail, "{foreign_on:?}");
    assert!(foreign_on.text.contains("занята чужой командой"), "{foreign_on:?}");

    let foreign_off = setup_line("pi", false, Ok(crate::editors::Found::Foreign(vec!["1c".into()]))).unwrap();
    assert_eq!(foreign_off.kind, Kind::Ok, "{foreign_off:?}");
    // Не «наши записи убраны»: в этом состоянии наших записей могло не быть
    // вовсе, и тогда прежний текст сообщал о работе, которой не было
    // (находка 3 ре-ревью четвёртого круга).
    assert!(foreign_off.text.contains("наших записей больше нет"), "{foreign_off:?}");
    assert!(foreign_off.text.contains("1c"), "{foreign_off:?}");
    assert!(!foreign_off.text.contains("не удалось"), "просьбу выполнили — отказом это звать нельзя: {foreign_off:?}");

    let refused = setup_line("Zed", true, Err(vec!["vrunner".into()])).unwrap();
    assert!(refused.text.contains("занята чужой командой"), "{refused:?}");
}

/// Задача 5, решение владельца («руки важнее знания»): скилл отказал (чужая
/// папка на месте скилла), а записи MCP уже поставлены — отказ скилла не
/// отменяет их. Строка журнала обязана назвать ОБА исхода, а не только
/// неудачный, иначе человек, которому сказали половину, второй не узнает.
#[test]
fn a_refused_skill_does_not_cost_the_agent_its_mcp_entries() {
    let mcp = setup_line("Zed", true, Ok(crate::editors::Found::Ours)).unwrap();
    let skill = skill_line(true, Ok(crate::editors::Found::Foreign(vec!["чужая-папка".to_string()])));
    let line = agent_line(mcp, skill);

    // Обе половины в тексте: записи MCP на месте, и имя чужой папки у скилла.
    assert!(line.text.contains("на месте"), "{line:?}");
    assert!(line.text.contains("чужая-папка"), "{line:?}");
    // Половина со скиллом не как просили — строка обязана это показать.
    //
    // Решение владельца 31.10 (повод 2): этой строкой дело и ограничивается.
    // Галочка при этом ОСТАЁТСЯ стоять — она отражает записи MCP, а про
    // навыки говорит вот эта самая строка журнала, и больше ничто. Прежде
    // здесь склеивались обе половины — `editors::both`, — и чужая папка на
    // месте скилла снимала галочку с полностью настроенного агента; самой
    // склейки больше нет (круг исправления 31.10, пункт 3).
    // Утверждения про галочку тут нет нарочно: после правки половину со
    // скиллом в неё некому нести, и проверка свелась бы к пересказу тела
    // `tray::agent_checked` — такая не падает ни в одном состоянии мира.
    assert_eq!(line.kind, Kind::Fail, "{line:?}");
}

/// Important 1 кругового ревью: та же беда, что уже чинили для MCP
/// (`success_is_silent_and_a_mismatch_speaks`, «наших записей больше нет», а
/// не «наши убраны») — здесь могло не быть ни одного нашего скилла вовсе,
/// только чужая папка, и «наш убран» сообщало бы о работе, которой не было.
#[test]
fn a_foreign_skill_left_alone_at_removal_is_not_called_our_own() {
    let line = skill_line(false, Ok(crate::editors::Found::Foreign(vec!["beta".to_string()])));
    assert_eq!(line.kind, Kind::Ok, "{line:?}");
    assert!(line.text.contains("наших больше нет"), "{line:?}");
    assert!(line.text.contains("beta"), "{line:?}");
    assert!(!line.text.contains("наш убран"), "просьбу выполнили — отказом или работой, которой не было, это звать нельзя: {line:?}");
}

/// Пункт 5 ревью целого, первая половина: та же беда в соседней ветке —
/// снятие при ПУСТОМ доме говорило «убран». `remove` отвечает одним и тем же
/// `Found::Absent` и когда унёс наши каталоги, и когда уносить было нечего
/// вовсе, различить эти случаи `skill_line` нечем — значит слово обязано быть
/// правдой в обоих. Падает на возврате к «убран».
#[test]
fn a_removal_with_nothing_of_ours_to_take_does_not_claim_work() {
    let line = skill_line(false, Ok(crate::editors::Found::Absent));
    assert_eq!(line.kind, Kind::Ok, "{line:?}");
    assert!(line.text.contains("наших больше нет"), "{line:?}");
    assert!(!line.text.contains("убран"), "о работе, которой могло не быть, говорить нельзя: {line:?}");

    // Включение по-прежнему говорит о работе: там `Found::Ours` — это и есть
    // доказательство, что наши каталоги на месте.
    let on = skill_line(true, Ok(crate::editors::Found::Ours));
    assert!(on.text.contains("поставлен"), "{on:?}");
}

/// Круг 4, точка 2: причина отказа `skills::state()` обязана дойти до строки
/// журнала щелчка СЛОВАМИ, а не обернуться молчаливым «скиллов нет». Круг 5,
/// пункт 3: держим это на САМОМ решении (`skills_plan`), а не на печати уже
/// готовой ошибки — прежний тест звал `skill_line(true, Err(vec![why]))` и
/// остался бы зелёным, проглоти причину кто-нибудь внутри `skills_half`.
/// Функция чистая, так что в среду не ходим вовсе: правила задачи запрещают
/// править `LOCALAPPDATA`/`HOME` на общем прогоне.
///
/// Падает на обеих подменах, от которых стоит: проглотить причину
/// (`state.ok()`, решение «раскладывать») — падает `expect_err`; подставить
/// вместо неё пустую строку — падает `contains`.
#[test]
fn the_click_log_line_carries_the_state_dir_refusal_in_words() {
    let home = std::path::PathBuf::from("/home/a/.agents/skills");
    let why = crate::ops_host::STATE_DIR_REFUSAL.to_string();

    let line = skills_plan::<()>(true, Some(home.clone()), false, Err(why.clone()))
        .expect_err("отказ состояния — не раскладка: писать отпечатки некуда");
    assert_eq!(line.kind, Kind::Fail, "{line:?}");
    assert!(line.text.contains(&why), "причина обязана быть в строке целиком: {line:?}");

    // Без отказа решение — именно разложить, и в тот самый дом.
    let ready = skills_plan(true, Some(home.clone()), false, Ok(())).expect("состояние годно — раскладываем");
    assert_eq!(ready, (home.clone(), ()), "дом агента обязан дойти до раскладки неизменным");

    // Дома не узнать — ни раскладки, ни молчания.
    let no_home = skills_plan::<()>(true, None, false, Ok(())).expect_err("без дома агента раскладывать некуда");
    assert_eq!(no_home.kind, Kind::Fail, "{no_home:?}");

    // Общий дом ещё нужен соседу: снятие не трогает его тексты, и это не отказ.
    let kept = skills_plan::<()>(false, Some(home.clone()), true, Ok(())).expect_err("общий дом не трогаем");
    assert_eq!(kept.kind, Kind::Ok, "{kept:?}");
    assert!(kept.text.contains("ещё нужен другому агенту"), "{kept:?}");
    // А при ВКЛЮЧЕНИИ тот же сосед раскладке не мешает.
    assert!(skills_plan(true, Some(home.clone()), true, Ok(())).is_ok(), "включение общий дом не отменяет");
}

/// Задача 6 и круг исправления 31.10 (пункт 1): запуск окна человек ни о чём
/// не просил, и строка в журнал появляется только тогда, когда ЕСТЬ О ЧЁМ
/// сказать. Молчание при пустом результате — решение владельца: иначе при
/// каждом старте человек читает одно и то же и перестаёт читать журнал вовсе.
///
/// А вот оставленный каталог — повод говорить. С 31.10 навыки не гасят
/// галочку и не красят набор, журнал остался единственной поверхностью, и
/// молчание здесь означало бы, что человек с неразложенными навыками не
/// узнает о них НИКОГДА (спека 31.10: «Молчаливое „всё хорошо“ при
/// неразложенных навыках — не то, о чём просили»).
///
/// Падает на трёх подменах, от которых стоит: вернуть `kept` в исключение из
/// `Done::quiet` — падает `kept_only`; выкинуть ранний возврат по
/// `Done::quiet` (молчание станет строкой «скиллы — » ни о чём) — падает
/// `is_none` на пустом; перестать называть правленый скилл — падает
/// `contains("beta")`.
#[test]
fn the_startup_delivery_speaks_whenever_there_is_something_to_say() {
    // Менять было нечего И оставлять нечего — молчим.
    assert!(refresh_line("Zed", &crate::skills::Done::default()).is_none(), "пустой результат — молчание");

    // Один только оставленный каталог — работы не было, а сказать есть что:
    // навыки у человека не встали, и больше ему об этом сказать нечему.
    let kept_only = crate::skills::Done { kept: vec!["beta".to_string()], ..Default::default() };
    let kept_line = refresh_line("Zed", &kept_only).expect("неразложенный навык обязан быть назван");
    assert!(kept_line.text.contains("Zed"), "{kept_line:?}");
    assert!(kept_line.text.contains("beta"), "имя каталога обязано быть в строке: {kept_line:?}");
    assert_eq!(kept_line.kind, Kind::Warn, "зелёная строка про неразложенный навык читается как успех: {kept_line:?}");

    // Работа была И что-то оставлено — цвет по худшему: оставленный каталог
    // не становится успехом оттого, что рядом поставили другой.
    let done = crate::skills::Done { added: vec!["1c".to_string()], updated: vec!["cf-edit".to_string()], kept: vec!["beta".to_string()] };
    let line = refresh_line("Zed", &done).expect("работа была — о ней говорят");
    assert_eq!(line.kind, Kind::Warn, "{line:?}");
    assert!(line.text.contains("Zed"), "{line:?}");
    assert!(line.text.contains("1c"), "{line:?}");
    assert!(line.text.contains("cf-edit"), "{line:?}");
    assert!(line.text.contains("beta"), "правленый обязан быть назван: {line:?}");
}

/// Пункт 1 ревью целого (дефект режима): в туннельном режиме тихой доставки
/// нет вовсе — ни опроса агентов, ни раскладки в доме человека, ни строки в
/// журнал. Решение держим на чистой функции (`delivery_needed`), а не на
/// живом проходе: в локальном режиме настоящий проход полез бы в настоящий
/// дом человека и в CLI агентов, что правила задачи запрещают прямо.
///
/// Падает на обеих подменах, от которых стоит: выкинуть режим из условия —
/// падает строка про `tunnel`; выкинуть проверку комплекта — падает строка
/// про комплект без скиллов. Строка `refresh_skills` ниже сторожит, что
/// решение и правда стоит на входе доставки, а не осталось функцией, которую
/// никто не зовёт.
#[test]
fn the_startup_delivery_does_nothing_at_all_in_tunnel_mode() {
    assert!(delivery_needed("local", true), "локальный режим с комплектом — доставка нужна");
    assert!(!delivery_needed("tunnel", true), "в туннельном режиме делать не нужно ничего");
    assert!(!delivery_needed("something-else", true), "незнакомый режим локальным не считаем");
    assert!(!delivery_needed("local", false), "в комплекте нечего доставлять");

    // И сама доставка молчит, не сходив ни к агентам, ни за каталогом
    // отпечатков: комплект здесь со скиллами, то есть пустой ответ может
    // дать только режим.
    let root = crate::project::tempdir::Dir::new();
    std::fs::create_dir_all(root.path().join(crate::skills::KIT_DIR).join("1c")).unwrap();
    std::fs::write(root.path().join(crate::skills::KIT_DIR).join("1c/SKILL.md"), "текст скилла").unwrap();
    let kit = Kit::at(root.path());
    let lines = refresh_skills(&kit, std::path::Path::new("c:/нет-такого/1c-connect.exe"), "tunnel");
    assert!(lines.is_empty(), "в туннельном режиме говорить нечего: {lines:?}");
}

/// Круг исправления задачи 6, пункт 3: `allowed` таблицей — тем же приёмом,
/// которым уже закреплён `skills_plan` чуть выше. Падает на обеих подменах,
/// от которых стоит: `Found::Ours` → `Found::Foreign` пропустила бы индекс
/// 1 (чужую запись сочли бы разрешением); `.is_ok()` пропустила бы и его, и
/// индекс 2 (`Absent` тоже читаемый результат, но разрешения не несёт).
#[test]
fn allowed_lets_through_only_our_own_records() {
    use crate::editors::Found;
    let looks: Vec<Result<Found, String>> = vec![
        Ok(Found::Ours),
        Ok(Found::Foreign(vec!["beta".to_string()])),
        Ok(Found::Absent),
        Ok(Found::Missing),
        Err("не прочитать".to_string()),
        Ok(Found::Ours),
    ];
    assert_eq!(allowed(&looks), vec![0, 5]);
}

/// Итог щелчка из канала, мимо прочих событий (`guarded` шлёт перед паникой
/// ещё и строку в журнал) — тем же приёмом, что `loudness_of_the_pass` в
/// `facts::tests`: поддельный канал `Ui::for_test` и `block_on`.
fn next_agent(rx: &mut futures::channel::mpsc::UnboundedReceiver<Ev>) -> (usize, Option<Line>, Instant) {
    loop {
        match futures::executor::block_on(futures::StreamExt::next(rx)).expect("поток щелчка не ответил") {
            Ev::Agent { i, line, started } => return (i, line, started),
            _ => continue,
        }
    }
}

/// Шов щелчка галочки: защёлка, её срок и ответ потока. Тело поддельное —
/// настоящее (`agent_work`) зовёт CLI агента и пишет в файлы человека;
/// проверяется здесь всё остальное, то есть ровно то, что сломалось бы.
#[test]
fn an_agent_click_is_latched_and_answers_with_its_own_event() {
    let mut ui = Ui::for_test();
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    ui.tx = tx;
    // Якорь в будущем: на свежей машине `Instant::now() - 90 с` паникует.
    let now = Instant::now() + Duration::from_secs(3600);
    let said = Line { kind: Kind::Fail, text: "Zed: не удалось настроить".into() };

    let mine = said.clone();
    assert!(toggle_agent_ui(&mut ui, 0, true, now, move || Some(mine)), "первый щелчок не приняли");
    // Признак работы на принятом щелчке — строка в журнал: щёлкают из трея,
    // при закрытом окне, и до 25 с человеку иначе не видно ничего.
    let started_line = ui.log.last().expect("принятый щелчок не сказал в журнал ни слова").clone();
    assert_eq!(started_line.kind, Kind::Say, "{started_line:?}");
    assert!(started_line.text.contains("настраиваю записи MCP"), "{started_line:?}");

    // Второй щелчок по тому же агенту не принят — и это предупреждение, а не
    // отказ работы: ничего не сломалось, надо дождаться ответа.
    assert!(!toggle_agent_ui(&mut ui, 0, true, now, || unreachable!("второй поток на одного агента")));
    assert_eq!(ui.log.last().unwrap().kind, Kind::Warn, "{:?}", ui.log.last());
    // Сосед при этом не заперт: работы у агентов разные.
    assert!(toggle_agent_ui(&mut ui, 1, false, now, || None), "щелчок по соседнему агенту заперт чужой работой");

    // Оба принятых щелчка отвечают своим событием с тем, что отдало тело;
    // порядок финиша потоков произволен, поэтому разбираем по индексу.
    let mut got = [next_agent(&mut rx), next_agent(&mut rx)];
    got.sort_by_key(|(i, ..)| *i);
    assert_eq!(got[0], (0, Some(said), now));
    assert_eq!(got[1], (1, None, now));

    // Срок отпускает защёлку аварийно: `editors::cli` ждёт `c.output()` без
    // предела, и без этого галочка зависшего агента была бы мертва до
    // перезапуска приложения.
    let later = now + AGENT_CLICK_LOST + Duration::from_secs(1);
    assert!(!agent_click_busy(ui.agent_clicks[0], later), "срок не вышел");
    assert!(toggle_agent_ui(&mut ui, 0, true, later, || None), "по истечении срока щелчок всё равно не пускают");
    assert_eq!(next_agent(&mut rx), (0, None, later), "ответ пришёл с отметкой не своего щелчка");
    // Запоздавший ответ отпущенного щелчка безвреден: защёлку живого он не
    // снимает — сверка по отметке (`on_agent`).
    on_agent(&mut ui, 0, None, now);
    assert_eq!(ui.agent_clicks[0], Some(later), "запоздавший ответ снял защёлку следующего щелчка");
    on_agent(&mut ui, 0, None, later);
    assert_eq!(ui.agent_clicks[0], None, "свой ответ защёлку не снял");

    // Паника в потоке — тоже ответ: иначе агент оставался бы неподатливым до
    // срока без всякой причины.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    assert!(toggle_agent_ui(&mut ui, 2, true, now, || panic!("CLI упал")));
    let panicked = next_agent(&mut rx);
    std::panic::set_hook(prev);
    assert_eq!(panicked, (2, None, now));
    on_agent(&mut ui, 2, None, now);
    assert_eq!(ui.agent_clicks[2], None, "паника оставила галочку агента мёртвой");
}

/// Пункт 7: щелчок и сразу «Выход» могли оборвать процесс между `remove` и
/// `add` и оставить агента вовсе без записи `1c` — человек просил её поставить,
/// а не получил ни старой, ни новой (чужой настройки там не теряется: к CLI с
/// занятым чужим именем мы не идём вовсе). Выход ждёт потоки щелчка тем же
/// порядком, что сеанс, и по истечении уходит всё равно.
#[test]
fn quitting_waits_for_clicks_in_flight_but_not_forever() {
    let live = AtomicUsize::new(1);
    let t0 = Instant::now();
    assert!(!wait_quiet(&live, Duration::from_millis(200)), "ждали вечно");
    assert!(t0.elapsed() >= Duration::from_millis(200), "срок не выдержан");

    live.store(0, Ordering::Relaxed);
    assert!(wait_quiet(&live, Duration::from_secs(5)), "ждать было нечего");
}
