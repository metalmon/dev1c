use super::*;
// `use gpui_kit::*` в `mod.rs` затаскивает в область имя `test` (атрибут
// `#[gpui::test]`), и глоб перекрывает встроенный. Явный импорт возвращает
// обычный `#[test]`: одиночный импорт сильнее глоба.
use crate::cfg::{cfg_with, Connection};
use crate::session::State;
use common::check::Check;
use core::prelude::v1::test;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Important 1 ревью задач 7+8: «выключено значит выключено». Набор
/// сбрасывался в одном месте — `apply_facts`, — а при выключенном
/// обслуживании расчёт не идёт вовсе, и прежний набор оставался живым:
/// серая карточка «MCP выключены» рядом с красным бейджем «Проблем: 1» и
/// вкладка «Инструменты» со старыми красными точками.
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
    ui.cfg.current_mut().mode = "local".into();
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

/// Круг исправления 31.10, пункт 2: уронить набор при выключении — половина
/// дела, вторая половина — чтобы вкладке «Инструменты» было что показать
/// ВМЕСТО него. Самый частый путь ровно этот: запуск при включённых MCP —
/// полный набор ушёл в ветку обслуживания, кит-проход (`spawn_kit_pass`) не
/// звался ни разу, `ui.kit_facts` пуст; человек щёлкает переключателем при
/// открытом окне, набор роняется — и вкладка пустеет до следующего
/// дискретного события. Проверяем сквозь настоящий шов: переход, ответ
/// потока из канала, `panel::tab_facts`.
///
/// Падает на подмене, от которой стоит: убрать `spawn_kit_pass` из ветки
/// `!serving` в `sync_tray` — `Ev::Kit` не придёт, и ожидание упадёт по
/// сроку. Ждать со сроком, а не бесконечно, — ровно для этого.
#[test]
fn the_tab_still_has_the_kit_to_show_the_moment_the_switch_goes_off() {
    let mut ui = Ui::for_test();
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    ui.tx = tx;
    ui.cfg.current_mut().mode = "local".into();
    ui.enabled.store(true, Ordering::Relaxed);
    assert!(serving(&ui));
    let gen = ui.facts_gen;
    apply_facts(&mut ui, gen, vec![Check::pass("база", "есть")]);
    assert_eq!(ui.facts.len(), 1, "набор не применился — тест проверяет не то");
    assert!(ui.kit_facts.is_empty(), "кит-проход уже звали — самый частый путь проверять нечем");

    ui.enabled.store(false, Ordering::Relaxed);
    ui.link = local_display("local", false, ui.link.clone());
    sync_tray(&mut ui, Instant::now());
    assert!(!serving(&ui));
    assert!(ui.facts.is_empty(), "набор выключенного обслуживания остался на экране");

    // Ответ кит-прохода — ровно то, что делает с ним `Ev::Kit`. Ждём со
    // сроком, а не `block_on(rx.next())`: отправитель жив в `ui.tx`, и
    // подмена «убрать `spawn_kit_pass` из `sync_tray`» повесила бы тест
    // навсегда вместо того, чтобы его уронить.
    let deadline = Instant::now() + Duration::from_secs(20);
    let facts = loop {
        match rx.try_recv() {
            Ok(Ev::Kit(facts)) => break facts,
            Ok(_) => continue,
            _ => {
                assert!(Instant::now() < deadline, "кит-проход при выключении не завёлся — вкладке показывать нечего");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    assert!(!facts.is_empty(), "кит-проход ответил пустым набором");
    ui.kit_facts = facts;
    assert!(!crate::gui::panel::tab_facts(&ui.facts, &ui.kit_facts).is_empty(), "вкладка «Инструменты» пуста в момент выключения MCP");
}

/// Minor 3 ревью задач 7+8: час «проверено в 15:54» — это момент, когда
/// набор стал ПОЛНЫМ. Уронённому набору час ни к чему: показывать его
/// там, где показывать нечего, — врать вдвойне.
#[test]
fn the_hour_belongs_to_a_full_set() {
    let mut ui = Ui::for_test();
    ui.cfg.current_mut().mode = "local".into();
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
    let h = Connection { server: "s:1".into(), workstation: "dev2".into(), ..Default::default() };
    assert_eq!(link_line(&State::Connecting, &h, "tunnel").unwrap().text, "подключаюсь к s:1 как dev2");
    assert_eq!(link_line(&State::Off, &h, "tunnel").unwrap().text, "отключено");
    assert!(link_line(&State::Up { since: Instant::now() }, &h, "tunnel").is_none());
}

#[test]
fn local_mode_connecting_line_hides_the_environment_fields() {
    // Адрес среды, пароль и имя рабочего места скрыты в «Связи» в
    // локальном режиме и ничего не значат — журнал не должен о них
    // говорить (находка целостного ревью).
    let h = Connection { server: "s:1".into(), workstation: "dev2".into(), ..Default::default() };
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
    ui.cfg.current_mut().mode = "local".into();
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
    ui.cfg.current_mut().mode = "local".into();
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
    ui.cfg.current_mut().mode = "local".into();
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
    ui.cfg.current_mut().mode = "local".into();
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

/// Строка о запертом переключателе не должна обещать попытку, которой не
/// было: заказчик 31.10.2026 — «что значит „не включены при запуске“? у меня
/// галочки включать при запуске не стояло». Причина звучит в обоих случаях,
/// но про запуск — только когда запуск просили.
#[test]
fn the_gate_line_mentions_startup_only_when_startup_was_asked_for() {
    let asked = gate_line("не указана база", true);
    assert!(asked.text.contains("при запуске"), "{}", asked.text);
    assert!(asked.text.contains("не указана база"), "{}", asked.text);

    let not_asked = gate_line("не указана база", false);
    assert!(!not_asked.text.contains("при запуске"), "галочки не было — про запуск молчим: {}", not_asked.text);
    assert!(not_asked.text.contains("не указана база"), "причина остаётся в обоих случаях: {}", not_asked.text);
    assert_eq!(not_asked.kind, Kind::Warn, "это предупреждение, а не сообщение об успехе");
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
    ui.cfg.current_mut().mode = "local".into();
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
    assert_eq!(ui.cfg.current().mode, "tunnel");
    ui.link = State::Up { since: Instant::now() };
    ui.enabled.store(false, Ordering::Relaxed);
    assert!(!serving(&ui), "туннель поднят, а обслуживание погашено — это не «обслуживаем»");
    ui.enabled.store(true, Ordering::Relaxed);
    assert!(serving(&ui));
    ui.link = State::Off;
    assert!(!serving(&ui), "в туннеле без сеанса вызову некуда ехать");
    // В локальном режиме слушатели подняты всегда — говорит только флаг.
    ui.cfg.current_mut().mode = "local".into();
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
    ui.cfg.current_mut().mode = "local".into();
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
    ui.cfg.current_mut().mode = "local".into();
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

// Задача 1 плана `2026-11-03-interface`: строка списка «Связь» — слово
// состояния, отметка «текущее», признак записи.

/// Спека, «Связь»: «нет» своей строки не имеет вовсе, «тикет выдан»
/// показывается словом «ждёт разрешения», «разрешено» — миг без слова.
/// Третий и четвёртый фолды («нет», «разрешено») докстрока `row` объясняет
/// словами, а не тестом: `State` их не различает вовсе, проверять нечего.
/// «Тикет выдан» здесь — `Some(State::Off)`, а не `None` (задача 16 плана
/// `2026-11-03-interface`: `None` — «состояния не знаем», своё слово, см.
/// тест ниже) — заглушка-поиск такого значения сама не отдаёт, но слово
/// обязано работать и тогда, когда живое хранилище состояний его принесёт.
///
/// Соединение нарочно НЕ локальное (`mode = "tunnel"`): умолчание
/// `Connection::default()` — локальный режим (30.10), а у него своя пара
/// слов, проверенная отдельным тестом ниже.
#[test]
fn row_folds_eight_contract_states_into_five_words() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    assert_eq!(row(&c, false, Some(&State::Up { since: Instant::now() })).state, "подключено");
    assert_eq!(row(&c, false, Some(&State::Failed(String::new()))).state, "разорвано");
    assert_eq!(row(&c, false, Some(&State::Off)).state, "ждёт разрешения");
}

/// Задача 16 плана `2026-11-03-interface`, «третье, оттуда же»: живого
/// хранилища состояний на соединение в продукте пока нет, заглушка-поиск
/// отдаёт `None` для всех карточек, кроме текущей, — и «ждёт разрешения»
/// для сохранённой карточки, которую никто не трогал, неправда на экране.
/// Правило «ждущие — первой строкой» (`list`) при этом не отменяется: оно
/// просто перестаёт срабатывать на всех разом, потому что срабатывать ему
/// больше не на чём.
#[test]
fn a_connection_with_no_known_state_is_not_waiting_for_permission() {
    let c = Connection { mode: "tunnel".into(), ..Default::default() };
    assert_eq!(row(&c, false, None).state, "не подключено");
}

/// Локальному соединению ни одно из пяти слов не годится: подключаться там
/// не к кому, оба конца на этой машине. Словарь берём НЕ новый — тот, что у
/// карточки состояния уже есть («MCP включены»/«MCP выключены»,
/// `panel::card`), в короткой форме: имя соединения строка и так назвала.
/// Вопрос владельца 3.11, из которого эта проверка и взялась.
#[test]
fn a_local_connection_speaks_about_tools_not_about_a_link() {
    let local = Connection { mode: "local".into(), ..Default::default() };
    assert_eq!(row(&local, true, Some(&State::Up { since: Instant::now() })).state, "включено");
    assert_eq!(row(&local, true, None).state, "выключено");
}

/// Окно 380 точек: вторая строка (имя базы) — только у текущего
/// соединения; у остальных её не поместить, не читая — разработчик с
/// десятью заказчиками получил бы двадцать строк.
#[test]
fn only_the_current_row_carries_the_base_name() {
    let c = Connection { base_path: r"C:\Bases\Buh".into(), ..Default::default() };
    assert!(row(&c, true, None).base.is_some());
    assert_eq!(row(&c, false, None).base, None, "окно 380 точек: двадцать строк не поместятся");
}

/// Review Focus брифа, задача 6: переключение текущей на соединение, у
/// которого база на той стороне, — путь к базе пустой законно, и это не
/// повод запирать окно. Здесь — соседний случай, что `row` на пустом пути
/// отдаёт `None`, а не пустую строку.
#[test]
fn a_current_connection_with_the_base_on_the_other_side_shows_no_base_name() {
    let c = Connection { base_path: String::new(), ..Default::default() };
    assert_eq!(row(&c, true, None).base, None);
}

/// Заглушка-поиск состояния по имени: живого хранилища состояний на
/// соединение в продукте пока нет (решение исполнителя задачи 1), и `list`
/// в тестах кормят ею же.
fn none(_: &str) -> Option<State> {
    None
}

/// Одна роль (все соединения принимают) — подписей не бывает: список
/// короткий специально, он проверяет только отсутствие подписей.
fn cfg_all_accept() -> Cfg {
    Cfg {
        connections: vec![
            Connection { name: "Ромашка".into(), mode: "tunnel".into(), role: "accept".into(), ..Default::default() },
            Connection { name: "Лютик".into(), mode: "tunnel".into(), role: "accept".into(), ..Default::default() },
        ],
        ..Default::default()
    }
}

/// По одному соединению на каждую из трёх групп — ровно то смешение,
/// которое спрашивал владелец 3.11.
fn cfg_accept_dial_local() -> Cfg {
    Cfg {
        connections: vec![
            Connection { name: "Ромашка".into(), mode: "tunnel".into(), role: "accept".into(), ..Default::default() },
            Connection { name: "Клевер".into(), mode: "tunnel".into(), role: "dial".into(), ..Default::default() },
            Connection { name: "Эта машина".into(), mode: "local".into(), ..Default::default() },
        ],
        ..Default::default()
    }
}

/// То же смешение, но первым в файле стоит соединение с заданным именем —
/// под заглушкой `named_is_waiting(name)` оно единственное «ждёт
/// разрешения» (задача 16: `None` больше не значит «ждёт», см. докстроку
/// `row`), и тест проверяет, что строка списка на первом месте — ЕГО
/// строка, не подпись и не какая-то из groups.
fn cfg_accept_dial_local_with_waiting(name: &str) -> Cfg {
    Cfg {
        connections: vec![
            Connection { name: name.into(), mode: "tunnel".into(), role: "accept".into(), ..Default::default() },
            Connection { name: "Клевер".into(), mode: "tunnel".into(), role: "dial".into(), ..Default::default() },
            Connection { name: "Эта машина".into(), mode: "local".into(), ..Default::default() },
        ],
        ..Default::default()
    }
}

#[test]
fn captions_appear_only_when_the_list_holds_more_than_one_role() {
    let one = list(&cfg_all_accept(), &none);
    assert!(!one.iter().any(|e| matches!(e, Entry::Caption(_))), "одна роль — подписи не нужны");
    let mixed = list(&cfg_accept_dial_local(), &none);
    let captions: Vec<&str> = mixed
        .iter()
        .filter_map(|e| match e {
            Entry::Caption(c) => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(captions, vec!["Принимаю подключения", "Подключаюсь сам", "На этой машине"]);
}

/// Заглушка-поиск состояния, в которой «ждёт разрешения» (`Some(State::Off)`)
/// получает только именованное соединение — остальные молчат (`None`, то
/// есть «не подключено», задача 16), и подъём строки выше подписей не
/// красит их заодно.
fn named_is_waiting(name: &str) -> Option<State> {
    (name == "Василёк").then_some(State::Off)
}

#[test]
fn a_connection_waiting_for_permission_is_the_very_first_row() {
    let l = list(&cfg_accept_dial_local_with_waiting("Василёк"), &named_is_waiting);
    match &l[0] {
        Entry::Row(r) => assert_eq!(r.name, "Василёк", "выше всех подписей: диалог могли закрыть не решив"),
        Entry::Caption(c) => panic!("первой стоит подпись «{c}», а не ждущее соединение"),
    }
}

/// Две карточки, по одной на accept и dial — ровно столько, чтобы подписи
/// были нужны (`show_captions`), и ни одной лишней: третья роль (local)
/// здесь намеренно отсутствует, иначе «Клевер» не мог бы остаться видимым
/// под своей подписью сам по себе, без помощи третьей группы.
fn cfg_accept_waiting_dial_connected() -> Cfg {
    Cfg {
        connections: vec![
            Connection { name: "Василёк".into(), mode: "tunnel".into(), role: "accept".into(), ..Default::default() },
            Connection { name: "Клевер".into(), mode: "tunnel".into(), role: "dial".into(), ..Default::default() },
        ],
        ..Default::default()
    }
}

/// Заглушка-поиск состояния, которая различает соединения по имени:
/// «Клевер» живой, «Василёк» ждёт разрешения (`Some(State::Off)`, задача
/// 16 — `None` больше не значит «ждёт», см. докстроку `row`). Нужна ровно
/// для того, чтобы отличить «подпись осталась, хотя её единственный член
/// всплыл» от «обе группы всё равно ждут» (та подмена, которую нашло
/// ревью круга починки 1 у прежнего теста).
fn only_clover_is_connected(name: &str) -> Option<State> {
    match name {
        "Клевер" => Some(State::Up { since: Instant::now() }),
        "Василёк" => Some(State::Off),
        _ => None,
    }
}

/// Подпись группы остаётся на месте, даже когда единственный её член ждёт
/// разрешения и всплыл выше подписей: подпись решает полный список, а не
/// то, что осталось видимым под ней (докстрока `list`, правило 2). Dial
/// здесь живой (не ждущий), поэтому тест отличает «подпись осталась
/// несмотря на подъём» от «обе группы ждут и подписей нет вовсе» —
/// находка ревью круга починки 1: прежний тест `&none` красил обе группы
/// одинаково и падал бы точно так же, как уже падает
/// `captions_appear_only_when_the_list_holds_more_than_one_role`, то есть
/// не добавлял покрытия.
#[test]
fn a_caption_survives_even_when_its_only_member_is_waiting() {
    let l = list(&cfg_accept_waiting_dial_connected(), &only_clover_is_connected);
    let captions: Vec<&str> = l
        .iter()
        .filter_map(|e| match e {
            Entry::Caption(c) => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(captions, vec!["Принимаю подключения", "Подключаюсь сам"], "подпись accept пропала вместе со своим единственным членом");
    match &l[0] {
        Entry::Row(r) => assert_eq!(r.name, "Василёк", "выше всех подписей: диалог могли закрыть не решив"),
        Entry::Caption(c) => panic!("первой стоит подпись «{c}», а не ждущее соединение"),
    }
}

/// Задача 11 плана `2026-11-03-interface`: онбординг — пустая «Связь» с
/// одним вопросом и двумя ответами, мастера нет. Слова те же, что у
/// переключателя роли в карточке (задача 13): человек, ответивший здесь,
/// обязан узнать свой ответ, открыв карточку.
#[test]
fn the_two_onboarding_answers_set_the_topology() {
    let a = onboarding_answers();
    assert_eq!(a[0], ("Подключаюсь сам — у меня есть код", "dial"));
    assert_eq!(a[1], ("Принимаю подключения — код выдаю я", "accept"));
}

/// Онбординг кончается СОСТОЯНИЕМ, а не флагом (спека: «Кончается онбординг
/// состоянием, а не флагом»): первое соединение заведено — значит он
/// кончился. Ключа «онбординг пройден» в настройках не заводим.
#[test]
fn onboarding_ends_when_the_first_connection_exists() {
    assert!(onboarding_shown(&cfg_with(&[])));
    assert!(!onboarding_shown(&cfg_with(&["Buh"])), "флага «прошёл онбординг» в настройках не заводим");
}
