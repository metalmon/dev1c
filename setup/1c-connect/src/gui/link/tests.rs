use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use core::prelude::v1::test;
use crate::cfg::Host;
use crate::session::State;
use common::check::Check;
use std::sync::atomic::Ordering;
use std::time::Instant;

/// Important 1 ревью задач 7+8: «выключено значит выключено». Набор
/// сбрасывался в одном месте — `apply_facts`, — а при выключенном
/// обслуживании расчёт не идёт вовсе, и прежний набор оставался живым:
/// серая карточка «MCP выключены» рядом с красным бейджем «Проблем: 1» и
/// вкладка «Проект» со старыми красными точками.
///
/// Оба пути выключения, и оба кончаются `sync_tray`: ручное (щелчок —
/// `flip_enabled_only` + `set_link`, за которым она и стоит) и погашенное
/// замком (`douse_serving`).
#[test]
fn the_switch_going_off_drops_the_set() {
    let bad = || vec![Check::fail("проект", "не разложен", "1c-connect install")];
    // Пока обслуживаем — набор на месте: иначе этот тест был бы зелёным и
    // на безусловном сбросе, то есть на сломанном коде.
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(true, Ordering::Relaxed);
    assert!(serving(&ui));
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, bad());
    assert_eq!(ui.facts.len(), 1, "набор не применился — тест проверяет не то");
    assert_eq!(problem_count(&ui.facts), 1);
    sync_tray(&mut ui, Instant::now());
    assert_eq!(problem_count(&ui.facts), 1, "набор уронили при живом обслуживании");
    assert!(matches!(ui.verdict, Verdict::NotReady(_)), "{:?}", ui.verdict);

    // Путь первый: щелчок по переключателю.
    assert!(!flip_enabled_only(&mut ui), "щелчок выключил");
    ui.link = local_display("local", false, ui.link.clone());
    sync_tray(&mut ui, Instant::now());
    assert!(!serving(&ui));
    assert!(ui.facts.is_empty(), "бейдж и вкладка показывают набор выключенного обслуживания");
    assert_eq!(problem_count(&ui.facts), 0);
    assert_eq!(ui.verdict, Verdict::Off, "{:?}", ui.verdict);
    assert!(ui.facts_at.is_none(), "«проверено в 15:54» при пустом наборе");

    // Путь второй: замок защёлкнулся и погасил обслуживание сам.
    ui.enabled.store(true, Ordering::Relaxed);
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, bad());
    assert_eq!(ui.facts.len(), 1, "набор не вернулся — второй путь проверять нечем");
    douse_serving(&mut ui);
    assert!(!serving(&ui));
    assert!(ui.facts.is_empty(), "замок погасил обслуживание, а набор остался на экране");
    assert_eq!(ui.verdict, Verdict::Off, "{:?}", ui.verdict);
}

/// Minor 3 ревью задач 7+8: час «проверено в 15:54» — это момент, когда
/// набор стал ПОЛНЫМ. Уронённому набору час ни к чему: показывать его
/// там, где показывать нечего, — врать вдвойне.
#[test]
fn the_hour_belongs_to_a_full_set() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(true, Ordering::Relaxed);
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
    assert!(ui.facts_at.is_some(), "набор полный, а часа нет");

    // Пустой набор при живом обслуживании — часа ему не положено
    // (`apply_facts`): час ставится только вместе с настоящим набором.
    assert!(serving(&ui), "иначе час уронило бы выключение, а не эта ветка");
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, Vec::new());
    assert!(ui.facts_at.is_none(), "час остался от набора, которого нет");

    // И выключение (`sync_tray`) уносит час вместе с набором.
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
    assert!(ui.facts_at.is_some());
    ui.enabled.store(false, Ordering::Relaxed);
    ui.link = local_display("local", false, ui.link.clone());
    sync_tray(&mut ui, Instant::now());
    assert!(ui.facts.is_empty());
    assert!(ui.facts_at.is_none(), "«проверено в 15:54» при выключенном обслуживании");
}

#[test]
fn link_lines_say_where_and_as_whom() {
    let h = Host { server: "s:1".into(), workstation: "dev2".into(), ..Default::default() };
    assert_eq!(link_line(&State::Connecting, &h, "tunnel").unwrap().text, "подключаюсь к s:1 как dev2");
    assert_eq!(link_line(&State::Off, &h, "tunnel").unwrap().text, "отключено");
    assert!(link_line(&State::Up { since: Instant::now() }, &h, "tunnel").is_none());
}

#[test]
fn local_mode_connecting_line_hides_the_environment_fields() {
    // Адрес среды, пароль и имя рабочего места скрыты в «Параметрах» в
    // локальном режиме и ничего не значат — журнал не должен о них
    // говорить (находка целостного ревью).
    let h = Host { server: "s:1".into(), workstation: "dev2".into(), ..Default::default() };
    let text = link_line(&State::Connecting, &h, "local").unwrap().text;
    assert!(!text.contains("s:1") && !text.contains("dev2"), "{text}");
}

#[test]
fn autoconnect_reads_the_kit_state_with_no_record_meaning_off() {
    let ui = Ui::for_test();
    assert!(!autoconnect(&ui.kit));
    ui.kit.state_set("autoconnect", "1").unwrap();
    assert!(autoconnect(&ui.kit));
    ui.kit.state_set("autoconnect", "0").unwrap();
    assert!(!autoconnect(&ui.kit));
}

#[test]
fn a_stale_session_generation_is_not_current() {
    let mut ui = Ui::for_test();
    ui.session_gen = 2;
    assert!(!link_is_current(&ui, 1), "поколение 1 устарело, текущее — 2");
    assert!(link_is_current(&ui, 2));
}

/// Critical задачи 9: включение после отказа bind обязано пробовать
/// поднять сеанс ЗАНОВО. Прежде «включить» упиралось в сохранённый отказ
/// и выходило молча, а слушатели поднимаются один раз на старте процесса —
/// человек оставался в красном до перезапуска окна (журнал владельца
/// 26.10: десяток «порт 7771 уже занят» на давно свободном порту).
#[test]
fn enabling_after_a_bind_failure_starts_a_fresh_session() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    with_params_file(&ui);
    ui.session = Some(ended_session(&ui));
    let why = "порт 7771 занят, кем — не видно".to_string();
    ui.link = State::Failed(why.clone());
    ui.bind_failed = Some(why);
    let gen_before = ui.session_gen;

    start_session(&mut ui).expect("файл параметров на месте");

    assert_eq!(ui.session_gen, gen_before + 1, "после отказа включение заводит новый сеанс, а не выходит молча");
    assert_eq!(ui.bind_failed, None, "прежний отказ принадлежал кончившемуся сеансу — новый скажет о портах сам");
    let _ = ui.session.take().unwrap().stop();

    // И тот же ответ, когда показанное состояние об отказе не говорит:
    // решает сам сеанс (`Session::finished`), а не запомненный текст и не
    // карточка. Отказ — следствие конца сеанса, а не его причина, и
    // спрашивать надо о конце.
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    with_params_file(&ui);
    ui.session = Some(ended_session(&ui));
    ui.link = State::Off;
    let gen_before = ui.session_gen;

    start_session(&mut ui).expect("файл параметров на месте");

    assert_eq!(ui.session_gen, gen_before + 1, "кончившийся сеанс поднимается заново и без показанного отказа");
    let _ = ui.session.take().unwrap().stop();
}

/// Обратная сторона: живой сеанс вторым поверх себя не поднимается —
/// слушатели в локальном режиме держат порты весь срок процесса, и
/// щелчок переключателем их не рвёт (Zed не переподключается сам).
/// А показанный `Failed` — тот же конец сеанса, только увиденный раньше,
/// чем поток отдал `done`: щелчок, попавший в это окно, тоже не должен
/// проваливаться молча.
#[test]
fn a_live_session_is_not_restarted_but_a_shown_failure_is_reason_enough() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    with_params_file(&ui);
    let (session, quiet) = live_session(&ui);
    ui.session = Some(session);
    let gen_before = ui.session_gen;

    start_session(&mut ui).expect("файл параметров на месте");
    assert_eq!(ui.session_gen, gen_before, "второй сеанс поверх живого не нужен");

    ui.link = State::Failed("порт 7771 занят".into());
    start_session(&mut ui).expect("файл параметров на месте");
    assert_eq!(ui.session_gen, gen_before + 1, "показанный отказ — повод попробовать заново");

    let _ = ui.session.take().unwrap().stop();
    drop(quiet);
}

#[test]
fn flip_enabled_only_touches_the_flag_and_nothing_else() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.session = Some(doomed_session(&ui));
    let gen_before = ui.session_gen;
    assert!(ui.enabled.load(Ordering::Relaxed), "фикстура заводит флаг включённым");
    assert!(!flip_enabled_only(&mut ui));
    assert!(!ui.enabled.load(Ordering::Relaxed));
    assert!(flip_enabled_only(&mut ui));
    assert!(ui.enabled.load(Ordering::Relaxed));
    // `flip_enabled_only` — только флаг: сеанс она не трогает, поколение
    // то же. Отпускание слушателей при выключении живёт в `toggle_link`
    // (с 26.10: выключено значит выключено), и ему нужен `&mut App` —
    // харнесса для него нет, держится ревью.
    assert_eq!(ui.session_gen, gen_before);
    let _ = ui.session.take().unwrap().stop();
}

#[test]
fn local_mode_hides_up_behind_the_flag_but_shows_a_failure_regardless() {
    let now = Instant::now();
    assert_eq!(local_display("local", false, State::Up { since: now }), State::Off);
    assert_eq!(local_display("local", false, State::Connecting), State::Off);
    assert_eq!(local_display("local", true, State::Up { since: now }), State::Up { since: now });
    let failed = State::Failed("порт занят".into());
    assert_eq!(local_display("local", false, failed.clone()), failed);
    // Туннельный режим — состояние сеанса идёт как есть, флаг тут ни при чём.
    assert_eq!(local_display("tunnel", false, State::Up { since: now }), State::Up { since: now });
}

/// «Подключаться при запуске» замок не обходит: коннектор поднимается,
/// обслуживание остаётся выключенным, причина уходит строкой в журнал
/// (сама строка — в `launch`, туда теста не дотянуться, как и до всего
/// остального в нём).
#[test]
fn autostart_does_not_enable_while_the_gate_holds() {
    assert!(!autostart_enabled(Some("не задан пользователь базы"), true), "замок заперт — обслуживание не включается");
    assert!(autostart_enabled(None, true), "условия в порядке — галочка работает как раньше");
    assert!(!autostart_enabled(None, false), "галочки нет — включать и нечего");
    assert!(!autostart_enabled(Some("в комплекте нет mcp-1c"), false));
}

/// Проводка замка к мосту: пока он заперт, отказ агенту несёт причину.
/// Сверяем не текст (в каталоге теста замок запирает что придётся, и
/// соседние тесты пишут в тот же процессный глобал), а то, что причина
/// вообще доезжает — без неё отказ советовал бы включить то, чего
/// включить нельзя.
#[test]
fn the_gate_reason_reaches_the_bridge() {
    let mut ui = Ui::for_test();
    refresh_gate(&mut ui);
    assert!(ui.gate.is_some(), "в каталоге теста нет ни параметров, ни инструментов — замок заперт");
    let reason = crate::bridge::off_reason();
    assert!(reason.is_some(), "причина замка не доехала до моста");
    assert!(crate::bridge::disabled_text(reason.as_deref()).starts_with(&format!("{}: ", crate::bridge::DISABLED_PREFIX)));
}

/// То же для отказа публикации (находка N2): человек ничего не выключал,
/// и «включите переключатель в окне» было бы советом не по делу —
/// агент должен услышать, что лежит публикация.
#[test]
fn a_failed_publication_tells_the_agent_what_really_happened() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.gate = None;
    ui.enabled.store(true, Ordering::Relaxed);

    let shown = publication_failure(&mut ui, "расширение не ответило");
    ui.link = shown;
    sync_tray(&mut ui, Instant::now());

    let text = crate::bridge::disabled_text(crate::bridge::off_reason().as_deref());
    assert!(text.contains("расширение не ответило"), "{text}");
    assert_ne!(text, crate::bridge::DISABLED_TEXT, "агенту советуют включить то, чего никто не выключал");

    // А когда выключил человек, прежний совет и есть правда.
    ui.link = State::Off;
    ui.bind_failed = None;
    sync_tray(&mut ui, Instant::now());
    assert_eq!(crate::bridge::disabled_text(crate::bridge::off_reason().as_deref()), crate::bridge::DISABLED_TEXT);
}

/// «Обслуживаем» значит то, что читает мост, — флаг. Туннелю нужен ещё и
/// сеанс, но сеанс сам по себе обслуживанием не является: с погашенным
/// флагом поднятый туннель показывал зелёное «Подключён», пока агенту
/// отказывали на каждом вызове (находка S5 итогового ревью).
#[test]
fn serving_means_the_flag_the_bridge_reads() {
    let mut ui = Ui::for_test();
    assert_eq!(ui.cfg.host.mode, "tunnel");
    ui.link = State::Up { since: Instant::now() };
    ui.enabled.store(false, Ordering::Relaxed);
    assert!(!serving(&ui), "туннель поднят, а обслуживание погашено — это не «обслуживаем»");
    ui.enabled.store(true, Ordering::Relaxed);
    assert!(serving(&ui));
    ui.link = State::Off;
    assert!(!serving(&ui), "в туннеле без сеанса вызову некуда ехать");
    // В локальном режиме слушатели подняты всегда — говорит только флаг.
    ui.cfg.host.mode = "local".into();
    assert!(serving(&ui));
    ui.enabled.store(false, Ordering::Relaxed);
    assert!(!serving(&ui));
}

/// Отказ публикации гасит обслуживание, а не только красит карточку:
/// иначе показанное («Не подключён») расходится с настоящим, и первый
/// щелчок по переключателю выключает вместо того, чтобы включить
/// (находка S6 итогового ревью).
#[test]
fn a_failed_publication_douses_serving() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.enabled.store(true, Ordering::Relaxed);
    let shown = publication_failure(&mut ui, "расширение не ответило");
    match &shown {
        State::Failed(why) => assert!(why.contains("расширение не ответило"), "{why}"),
        other => panic!("ожидался отказ, а не {other:?}"),
    }
    assert!(!serving(&ui), "карточка говорит «Не подключён», а вызовы обслуживаются");
}

/// Проводка замка до трея (находка S2): `sync_tray` стояла только внутри
/// ветки гашения, поэтому замок, защёлкнувшийся при выключённом
/// обслуживании, оставлял пункт «Включить MCP» нажимаемым — меню обещало
/// то, в чём `toggle_link` потом отказывал.
#[test]
fn a_latching_gate_reaches_the_tray_even_with_nothing_to_douse() {
    let mut ui = Ui::for_test();
    // Гасить нечего — ветка `douse_serving` не берётся вовсе.
    ui.enabled.store(false, Ordering::Relaxed);
    ui.gate = None;
    ui.tray_shown = None;
    refresh_gate(&mut ui);
    assert!(ui.gate.is_some(), "в каталоге теста замок обязан защёлкнуться");
    let shown = ui.tray_shown.clone().expect("замок не дошёл до трея");
    assert!(shown.locked, "пункт меню остался нажимаемым при запертом замке");
    // Пункт трея и переключатель окна читают один и тот же замок.
    assert_eq!(shown.locked, ui.gate.is_some());
}

/// Замок гасит обслуживание везде, где защёлкивается, а не только на
/// «Сохранить». Сценарий заказчика: человек правит toml руками, условие
/// пропадает, окно на показе перечитывает файл и пересчитывает ворота —
/// карточка говорит «MCP выключены», и агента обслуживать больше нельзя.
/// Раньше замок правил одни слова в отказе, а флаг оставался поднятым.
#[test]
fn a_gate_that_latches_douses_serving_once() {
    let mut ui = Ui::for_test();
    ui.cfg.host.mode = "local".into();
    ui.link = State::Up { since: Instant::now() };
    assert!(ui.enabled.load(Ordering::Relaxed), "фикстура заводит обслуживание включённым");
    // Замок был открыт, обслуживание шло — ровно то положение, из которого
    // правка toml руками роняет условие. «Сохранить» здесь никто не жал.
    ui.gate = None;
    let log_before = ui.log.len();
    refresh_gate(&mut ui);
    assert!(ui.gate.is_some(), "в каталоге теста условия не выполнены");
    assert!(!ui.enabled.load(Ordering::Relaxed), "обслуживание погашено пересчётом, а не «Сохранить»");
    assert_eq!(ui.link, State::Off, "карточка и переключатель показывают «выключено»");
    assert_eq!(ui.log.len(), log_before + 1, "о гашении сказано один раз");
    // Второй пересчёт при том же замке молчит: гасить уже нечего.
    let log_after = ui.log.len();
    refresh_gate(&mut ui);
    assert_eq!(ui.log.len(), log_after);
}

/// Обратное направление: включать обслуживание ворота не умеют — это дело
/// человека. Пересчёт при выключенном обслуживании не трогает ни флага,
/// ни журнала; переключатель отпирается сам, по `ui.gate`
/// (`panel::switch_locked`).
#[test]
fn the_gate_never_switches_serving_back_on() {
    let mut ui = Ui::for_test();
    ui.enabled.store(false, Ordering::Relaxed);
    ui.gate = None;
    let log_before = ui.log.len();
    refresh_gate(&mut ui);
    assert!(ui.gate.is_some(), "замок защёлкнулся");
    assert!(!ui.enabled.load(Ordering::Relaxed), "ворота обслуживание не включают");
    assert_eq!(ui.log.len(), log_before, "гасить было нечего — говорить не о чем");
}
